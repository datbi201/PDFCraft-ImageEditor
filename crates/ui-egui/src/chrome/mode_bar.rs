//! The top slot: the menus, the mode strip, the document buttons and the command search.

use std::borrow::Cow;
use std::cell::Cell;

use craft::behavior::{DisplayOrder, IconId, ItemSource, SelectionChange};
use craft::keymap::CommandRef;
use craft::menu::Target;
use craft::ui::bars::{Bar, BarKind};
use craft::ui::buttons::{CommandSearch, IconButton};
use craft::ui::contract::{Bind, CollectionEvent, Props as _, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::display::Divider;
use craft::ui::layout_components::Row;
use craft::ui::menu::HelpEntry;
use craft::ui::patterns::panels::CommandPalette;
use craft::ui::tabs::{Tab, TabKind, TabStrip};

use pdfcraft_engine::catalog::{Availability, TOOL_GROUPS};

use crate::{Mode, PdfCraftApp};

/// The modes the strip offers. Read is a View command, not a tab.
const MODES: [(Mode, &str); 4] = [(Mode::AllTools, "All tools"), (Mode::Edit, "Edit"), (Mode::Convert, "Convert"), (Mode::Sign, "E-Sign")];

const SPLIT_BAR_WIDTH: f32 = 820.0;
const COMPACT_SEARCH_WIDTH: f32 = 620.0;
const COMBINED_SEARCH_WIDTH: f32 = 1020.0;

/// The mode labels, keyed by position.
struct ModeTabs;

impl DisplayOrder<usize> for ModeTabs {
    fn len(&self) -> usize {
        MODES.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < MODES.len()).then_some(*key)
    }

    fn text(&self, at: usize) -> Option<Cow<'_, str>> {
        MODES.get(at).map(|(_, label)| Cow::Borrowed(tl!(*label)))
    }
}

impl ItemSource<usize> for ModeTabs {}

pub(super) fn split_menus(app: &PdfCraftApp, width: f32) -> bool {
    let hidden = app.hidden_interface();
    app.native_menu.is_none() && !hidden.menubar && !hidden.toolbar && width < SPLIT_BAR_WIDTH
}

fn drag_enabled(app: &PdfCraftApp) -> bool {
    cfg!(not(target_arch = "wasm32")) && !app.palette_open && !app.modal_open() && !app.full_screen_document()
}

pub(super) fn show_menus(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    window_bar(ui, "window-menus", tl!("Menu bar"), drag_enabled(app), |ui| app.menu_bar_ui(ui), document_buttons);
}

/// Draw the menus, the mode strip and the buttons after it, then the command palette over them.
pub(super) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui, split: bool) {
    let hidden = app.hidden_interface();
    let menus = app.native_menu.is_none() && !hidden.menubar;
    let width = ui.available_width();
    let toolbar = !hidden.toolbar;
    let search_width = if menus && !split { COMBINED_SEARCH_WIDTH } else { COMPACT_SEARCH_WIDTH };
    let compact_search = width < search_width;
    window_bar(
        ui,
        "mode-bar",
        tl!("Modes"),
        drag_enabled(app),
        |ui| {
            if menus && !split {
                app.menu_bar_ui(ui);
                if toolbar {
                    Divider::vertical().show(ui);
                }
            }
            if toolbar {
                mode_tabs(app, ui);
            }
        },
        |ui| {
            if toolbar {
                if compact_search {
                    IconButton::command(IconId::new("search"), "view.palette").label(tl!("Find tools and commands")).show(ui);
                } else {
                    CommandSearch::new("view.palette", tl!("Find tools and commands")).show(ui);
                }
                if !split {
                    document_buttons(ui);
                }
            }
        },
    );
    if app.palette_open {
        let ctx = ui.ctx().clone();
        let entries = palette_entries(app, &ctx);
        CommandPalette::new(tl!("Find tools and commands"), &entries, Bind::bound(&mut app.palette_open)).show(ui);
    }
}

/// Only the space between the measured control groups is a window handle.
fn window_bar(
    ui: &mut egui::Ui,
    key: &'static str,
    label: &str,
    drag_enabled: bool,
    mut leading: impl FnMut(&mut egui::Ui),
    mut trailing: impl FnMut(&mut egui::Ui),
) {
    let start = Cell::new(None);
    let end = Cell::new(None);
    let rtl = ui.layout().prefer_right_to_left();
    let bar = Bar::new(label.to_owned(), BarKind::Tool)
        .id_salt(key)
        .trailing(|ui, _| end.set(Some(Row::new().show(ui, |ui, _| trailing(ui)).output.rect)))
        .show(ui, |ui, _| start.set(Some(Row::new().show(ui, |ui, _| leading(ui)).output.rect)));
    let bounds = bar.output.response.rect;
    let start = start.get().unwrap_or_else(|| bounds.with_max_x(bounds.left()));
    let end = end.get().unwrap_or_else(|| bounds.with_min_x(bounds.right()));
    let (left, right) = if rtl { (end.right(), start.left()) } else { (start.right(), end.left()) };
    let gap = egui::Rect::from_min_max(egui::pos2(left, bounds.top()), egui::pos2(right, bounds.bottom())).intersect(bounds);
    if drag_enabled && gap.width() > 8.0 {
        let handle = ui.interact(gap, ui.scope_id().with((key, "window-drag")), egui::Sense::CLICK | egui::Sense::DRAG);
        if handle.double_clicked_by(egui::PointerButton::Primary) {
            let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        } else if handle.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }
}

fn document_buttons(ui: &mut egui::Ui) {
    IconButton::command(IconId::new("info"), "file.properties").show(ui);
    IconButton::command(IconId::new("save"), "file.save").show(ui);
    IconButton::command(IconId::new("printer"), "print.dialog").show(ui);
}

fn mode_tabs(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let strip = TabStrip::new(tl!("Modes")).kind(TabKind::Text).natural_height(true);
    // An active mode with its sidebar closed still needs an actionable tab.
    let active = app.left_open().then(|| MODES.iter().position(|(mode, _)| *mode == app.mode)).flatten();
    let strip = match active {
        Some(at) => strip.shown(at),
        None => strip,
    };
    let out = strip.show_items(ui, &ModeTabs, &mut |key, _| Tab::new(tl!(MODES[*key].1)));
    let requested = out.picked.or_else(|| {
        out.response.events.iter().find_map(|event| match event {
            CollectionEvent::Selection(SelectionChange::Set(keys)) => keys.last().copied(),
            CollectionEvent::Activate(at) => Some(*at),
            _ => None,
        })
    });
    if let Some((mode, _)) = requested.and_then(|at| MODES.get(at)) {
        app.switch_mode(ui.ctx(), *mode, true);
    }
}

/// What the palette searches: the menus' commands, each ready tool group, and the catalogue items
/// that are not commands of their own.
fn palette_entries(app: &PdfCraftApp, ctx: &egui::Context) -> Vec<HelpEntry> {
    let mut entries = app.menu_bar(ctx).help_entries(Target::current(), &crate::menus::layout_options());
    for group in TOOL_GROUPS {
        if group.availability == Availability::Ready {
            // PLAN: assumed `tool.group` reads a `group` parameter (pdf-chrome-w9)
            let select = CommandRef::with_params("tool.group", serde_json::json!({ "group": group.id }));
            entries.push(HelpEntry::new(select, tl!(group.label)).with_path([tl!("Tool")]));
        }
        for section in group.sections {
            for item in section.items {
                if item.availability == Availability::Ready && pdfcraft_engine::commands::command(item.command).is_none() {
                    entries.push(HelpEntry::new(item.command, tl!(item.label)).with_path([tl!(group.label)]));
                }
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::PdfCraftApp;

    #[test]
    fn titlebar_buttons_keep_default_cursor_and_mode_bar_aligns() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
            PdfCraftApp::install_runtime(&cc.egui_ctx);
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        });
        h.run_steps(4);

        // Hovering Preferences on the title bar must keep the normal pointer, not the window-drag Grab cursor.
        let prefs_center = h.get_by_label_contains("Preferences").rect().center();
        h.hover_at(prefs_center);
        h.run_steps(2);
        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Default);

        // MenuBar ("File") and ModeTabs ("All tools") sit in the same Bar with aligned vertical centers.
        let file_rect = h.get_by_label("File").rect();
        let all_tools_rect = h.get_by_role_and_label(egui::accesskit::Role::Tab, "All tools").rect();
        let dy = (file_rect.center().y - all_tools_rect.center().y).abs();
        assert!(dy <= 2.5, "MenuBar 'File' ({file_rect:?}) and 'All tools' ({all_tools_rect:?}) vertical centers differ by {dy}");
    }

    #[test]
    fn minimum_window_keeps_menus_and_mode_tabs_clear_and_reopens_tools() {
        use egui::accesskit::Role;

        let mut h = Harness::builder().with_size(egui::vec2(480.0, 520.0)).build_eframe(|cc| {
            PdfCraftApp::install_runtime(&cc.egui_ctx);
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            app.open_bytes(&cc.egui_ctx, "sample.pdf", None, include_bytes!("../../tests/data/form.pdf").to_vec()).unwrap();
            app.set_left_open(false);
            app
        });
        h.run_steps(6);
        let file = h.get_by_role_and_label(Role::MenuItem, "File").rect();
        let help = h.get_by_role_and_label(Role::MenuItem, "Help").rect();
        let all_tools = h.get_by_role_and_label(Role::Tab, "All tools").rect();
        let sign = h.get_by_role_and_label(Role::Tab, "E-Sign").rect();
        let search = h.get_by_role_and_label(Role::Button, "Find tools and commands").rect();
        let navigation = h.get_by_role_and_label(Role::Toolbar, "Navigation").rect();
        assert!(file.bottom() <= all_tools.top());
        assert!(help.right() <= 480.0 && all_tools.left() >= 0.0);
        assert!(sign.right() <= search.left() && search.right() <= 480.0);
        assert!(navigation.top() > sign.bottom() && navigation.bottom() <= 520.0);

        for _ in 0..3 {
            h.get_by_role_and_label(Role::Tab, "All tools").click();
            h.run_steps(4);
            assert!(h.state().left_open());
            h.state_mut().set_left_open(false);
            h.run_steps(4);
        }
    }
}
