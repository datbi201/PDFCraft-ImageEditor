use craft::pages::ui::THUMBNAIL_ZOOM;
use craft::pages::{Fit, LayoutOptions, PageDisplay, shown_row};

use super::{DocInfo, DocView, PT, SavedDisplay, vec2};

impl DocView {
    /// Whether Select All of the document applies: it belongs to continuous scrolling, and the flag stays set
    /// while the view is paged.
    pub fn all_text_active(&self) -> bool {
        self.session.all_text && self.view.continuous
    }

    /// Full screen, as Acrobat's Full Screen Mode (#379, #1459): one page at a time (or one
    /// spread, when two-page view was on: #1353), fitted to the screen, from the current page.
    /// With [`Self::full_screen_fill`] off it keeps the display it is entered from (#4273), at the
    /// reading position. The display it replaces is kept for [`Self::leave_presentation`]. A
    /// no-op while presenting.
    pub fn enter_presentation(&mut self) {
        if self.presentation.is_some() {
            return;
        }
        self.presentation = Some(SavedDisplay {
            display: self.view.display,
            continuous: self.view.continuous,
            cover: self.view.cover,
            fit: self.view.fit,
            zoom: self.view.zoom,
        });
        self.session.text.selection = None;
        self.session.presenter = Default::default();
        if self.full_screen_fill {
            self.view.continuous = false;
            self.view.fit = Fit::Page;
            self.view.go_to(self.view.current);
        } else {
            self.go_to_reading_position();
        }
    }

    /// Enter full screen with the app's Fill screen preference as it stands (#576, #4273): every
    /// view entering full screen, a document opened in it included, takes the preference here. A
    /// no-op while presenting, so a change made meanwhile waits for the next time.
    pub fn enter_full_screen(&mut self, fill: bool) {
        if self.presentation.is_some() {
            return;
        }
        self.full_screen_fill = fill;
        self.enter_presentation();
    }

    /// Leave full screen: the display from before it comes back, at the page reached meanwhile.
    /// From a display full screen kept, at the reading position.
    pub fn leave_presentation(&mut self) {
        let filled = self.fills_screen();
        let Some(saved) = self.presentation.take() else { return };
        self.session.presenter = Default::default();
        self.view.display = saved.display;
        self.view.continuous = saved.continuous;
        self.view.cover = saved.cover;
        self.view.fit = saved.fit;
        self.view.zoom = saved.zoom;
        if filled {
            self.view.go_to(self.view.current);
        } else {
            self.go_to_reading_position();
        }
    }

    /// Takes the next pass to where the reader is, as of the last pass: the page at the top of the
    /// page view and the fraction of it scrolled past, or the top of the current page when no page
    /// is there.
    fn go_to_reading_position(&mut self) {
        let top = self.viewport.top();
        let at = self.session.placed.iter().find(|p| p.rect.top() <= top && top < p.rect.bottom() && p.rect.height() > 0.0);
        match at {
            Some(p) => {
                let fraction = f64::from(((top - p.rect.top()) / p.rect.height()).clamp(0.0, 1.0));
                self.view.go_to_fraction(p.page, fraction);
            }
            None => self.view.go_to(self.view.current),
        }
    }

    /// The display shown outside full screen: the one presenting replaced, or the current one.
    pub fn normal_display(&self) -> SavedDisplay {
        self.presentation.unwrap_or(SavedDisplay {
            display: self.view.display,
            continuous: self.view.continuous,
            cover: self.view.cover,
            fit: self.view.fit,
            zoom: self.view.zoom,
        })
    }

    /// The display saved before entering full screen, while presenting.
    pub fn saved_display_mut(&mut self) -> Option<&mut SavedDisplay> {
        self.presentation.as_mut()
    }

    /// Whether the view is in full screen's one-page-at-a-time display.
    pub fn presenting(&self) -> bool {
        self.presentation.is_some()
    }

    /// Whether full screen fills the screen with one page (or spread) at a time: then the page
    /// takes the whole screen, with no margin around it (#576).
    pub fn fills_screen(&self) -> bool {
        self.presentation.is_some() && self.full_screen_fill
    }

    /// Whether full screen is playing a page transition (or holding the page before it until the
    /// next one has rendered).
    pub fn presenting_a_transition(&self) -> bool {
        self.session.presenter.turning()
    }

    /// Whether the view scrolls continuously (as opposed to one page or spread at a time).
    pub fn continuous(&self) -> bool {
        self.view.continuous
    }

    /// Whether the cover page setting applies: only two-page view has a cover page.
    pub fn cover_applies(&self) -> bool {
        self.view.display == PageDisplay::TwoUp
    }

    /// Switches to multipage grid (`PageDisplay::Columns`) with continuous scrolling.
    /// When the current fit or zoom shows fewer than 2 columns, sets a custom overview zoom
    /// fitting up to 3 columns across the viewport so the grid is immediately visible.
    pub fn enter_page_grid(&mut self) {
        self.view.display = PageDisplay::Columns;
        self.view.continuous = true;
        let vw = f64::from(self.viewport.width()).max(600.0);
        let gap = if self.view.page_gaps { self.spacing.map_or(16.0, |s| s.gap) } else { 0.0 };
        let side = self.spacing.map_or(16.0, |s| s.side);
        let turned = self.view.rotation % 180 == 90;
        let widest_pt = self.page_sizes.iter().map(|&(w, h)| f64::from(if turned { h } else { w })).fold(0.0_f64, f64::max).max(1.0);
        let widest_px = widest_pt * f64::from(PT);
        let usable = (vw - 2.0 * side - gap).max(widest_px * 0.2);
        let cur_cols = if self.view.fit == Fit::None { ((usable + gap) / (widest_px * self.view.zoom + gap)).floor() as usize } else { 1 };
        if cur_cols < 2 {
            let target_cols = 3.0_f64.min((self.page_count.max(2)) as f64);
            let ideal = ((usable - (target_cols - 1.0) * gap) / (target_cols * widest_px)).clamp(0.15, 0.55);
            self.view.fit = Fit::None;
            self.view.zoom = ideal;
        }
        self.view.go_to(self.view.current);
    }

    /// The page (or spread) the view shows first, as `FitWindow` sizes the window to it
    /// ([`crate::viewer_prefs::window_commands`]). `None` when there is no such page.
    ///
    /// The row is the framework's own spread rule. The margins are the spacing the last layout used: the
    /// gutter at each side, the gap between a spread's pages, and the margin above and below.
    pub(crate) fn first_page_now(&self, info: &DocInfo) -> Option<crate::viewer_prefs::PageNow> {
        info.pages.get(self.view.current)?;
        let spacing = self.spacing?;
        let options = LayoutOptions {
            display: self.view.display,
            cover: self.view.cover,
            continuous: self.view.continuous,
            page_gaps: self.view.page_gaps,
            ..LayoutOptions::ACROBAT
        };
        let row = shown_row(info.pages.len(), self.view.current, &options);
        let turned = self.view.rotation % 180 == 90;
        let sizes: Vec<(f32, f32)> =
            info.pages.get(row).unwrap_or_default().iter().map(|p| if turned { (p.height, p.width) } else { (p.width, p.height) }).collect();
        let w: f32 = sizes.iter().map(|s| s.0).sum();
        let h = sizes.iter().map(|s| s.1).fold(0.0, f32::max);
        let gap = if self.view.page_gaps { spacing.gap as f32 } else { 0.0 };
        let across = gap * sizes.len().saturating_sub(1) as f32;
        Some(crate::viewer_prefs::PageNow {
            size: vec2(w * PT, h * PT),
            margins: vec2(2.0 * spacing.side as f32 + across, 2.0 * spacing.margin as f32),
            zoom: (self.view.fit == Fit::None).then_some(self.view.zoom as f32),
        })
    }

    /// Scale of the thumbnails in the Pages inspector (1.0 is the usual size).
    pub fn thumbnail_zoom(&self) -> f32 {
        self.thumbnail_zoom
    }

    /// Resize the inspector thumbnails.
    /// Out-of-range values are clamped to the framework's thumbnail zoom; non-finite values are ignored.
    pub fn set_thumbnail_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            let range = THUMBNAIL_ZOOM.range();
            self.thumbnail_zoom = zoom.clamp(*range.start() as f32, *range.end() as f32);
        }
    }

    /// A zoom typed as a percentage ("150", "150%", " 62.5 % ") as a zoom factor; `None` unless it is a
    /// finite, positive number. The page view clamps it to the supported range.
    pub fn parse_zoom_percent(typed: &str) -> Option<f32> {
        let percent = typed.trim().trim_end_matches('%').trim_end().parse::<f32>().ok()?;
        (percent.is_finite() && percent > 0.0).then_some(percent / 100.0)
    }
}

#[cfg(test)]
mod tests {
    use craft::ui::runtime::commands::CommandRef;
    use egui_kittest::Harness;

    use crate::PdfCraftApp;
    use crate::canvas::PageDisplay;
    use crate::commands::catalog;

    const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 180] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn page_grid_layout_adapts_columns_to_zoom_and_viewport_width() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
            PdfCraftApp::install_runtime(&cc.egui_ctx);
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            app.open_bytes(&cc.egui_ctx, "pages.pdf", None, PAGES.to_vec()).unwrap();
            app.set_option(&cc.egui_ctx, "left", "closed").unwrap();
            app.set_option(&cc.egui_ctx, "panel", "none").unwrap();
            app
        });
        h.run_steps(6);

        let ctx = h.ctx.clone();
        h.state_mut().execute(&ctx, "view.layout.columns");
        h.run_steps(4);
        assert_eq!(h.state().views[0].view.display, PageDisplay::Columns);
        assert!(h.state().views[0].continuous());
        assert_eq!(catalog::checked(h.state(), &CommandRef::new("view.layout.columns")), Some(true));
        assert_eq!(catalog::checked(h.state(), &CommandRef::new("view.layout.single")), Some(false));

        // Smart overview zoom fits multiple pages on the first row when entering Page grid.
        let p0 = h.state().views[0].page_screen_rect(0).expect("page 1 placed");
        let p1 = h.state().views[0].page_screen_rect(1).expect("page 2 placed");
        assert!((p0.min.y - p1.min.y).abs() < 1.0, "pages 1 and 2 sit on the same grid row: p0={p0:?}, p1={p1:?}");
        assert!(p1.min.x > p0.max.x, "page 2 sits to the right of page 1");

        // Zooming in far enough drops the grid down to 1 column per row.
        h.state_mut().set_option(&ctx, "zoom", "180").unwrap();
        h.state_mut().views[0].go_to_page(0);
        h.run_steps(4);
        let z0 = h.state().views[0].page_screen_rect(0).expect("page 1 zoomed");
        let z1 = h.state().views[0].page_screen_rect(1).expect("page 2 zoomed");
        assert!(z1.min.y > z0.max.y, "zoomed in stacks page 2 below page 1: z0={z0:?}, z1={z1:?}");

        // Layout and default-layout tokens round-trip through "columns".
        h.state_mut().set_option(&ctx, "layout", "columns").unwrap();
        h.state_mut().set_option(&ctx, "default-layout", "columns").unwrap();
        assert_eq!(h.state().view_defaults.display, PageDisplay::Columns);
    }
}
