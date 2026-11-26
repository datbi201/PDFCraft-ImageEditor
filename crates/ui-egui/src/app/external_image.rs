//! Edit with / Edit at: hand a raster page image as a PNG in a private temp folder to an
//! external editor, watch its stamp with two-poll stability, and apply saves as one undoable
//! page-image edit on the active tab.

use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use craft::host::PickRequest;
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::{DocId, Edit, ExternalPageImage, ImageEdit};
use web_time::Instant;

use crate::PdfCraftApp;
use crate::edit_text_ui::ExternalEditor;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const REPAINT_AFTER: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub(crate) modified: std::time::SystemTime,
    pub(crate) len: u64,
}

fn read_stamp(path: &Path) -> Result<FileStamp, std::io::Error> {
    let meta = fs::metadata(path)?;
    let modified = meta.modified()?;
    Ok(FileStamp { modified, len: meta.len() })
}

#[derive(Clone, Debug)]
struct ExternalEditJob {
    id: u64,
    doc_id: DocId,
    page: usize,
    object: ObjRef,
    rect: [f64; 4],
    temp_path: PathBuf,
    last_applied_stamp: Option<FileStamp>,
    last_error_stamp: Option<FileStamp>,
    candidate_stamp: Option<FileStamp>,
    stable_polls: u8,
    pending_bytes: Option<(FileStamp, Vec<u8>)>,
}

/// Manages private temp exports and two-poll file watching for external image editors.
#[derive(Default)]
pub(crate) struct ExternalImageTracker {
    temp_dir: Option<PathBuf>,
    next_seq: u64,
    last_poll: Option<Instant>,
    jobs: Vec<ExternalEditJob>,
}

struct PendingExternalEdit {
    page: usize,
    object: ObjRef,
    rect: [f64; 4],
    stamp: FileStamp,
    bytes: Vec<u8>,
}

impl Drop for ExternalImageTracker {
    fn drop(&mut self) {
        self.cleanup_all();
    }
}

impl ExternalImageTracker {
    fn ensure_temp_dir(&mut self) -> Result<PathBuf, String> {
        if let Some(dir) = &self.temp_dir
            && dir.is_dir()
        {
            return Ok(dir.clone());
        }
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(|e| format!("Couldn't create temp folder: {e}"))?;
        let token = u64::from_le_bytes(raw);
        let dir = std::env::temp_dir().join(format!("pdfcraft-img-{token:016x}"));
        fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create temp folder {}: {e}", dir.display()))?;
        self.temp_dir = Some(dir.clone());
        Ok(dir)
    }

    pub(crate) fn stage_export(&mut self, doc_id: DocId, page: usize, exported: &ExternalPageImage) -> Result<(u64, PathBuf), String> {
        let dir = self.ensure_temp_dir()?;
        self.next_seq = self.next_seq.saturating_add(1);
        let id = self.next_seq;
        // ASCII-only filename inside the private temp directory avoids legacy code-page issues in
        // external editors while keeping the file outside Drive/OneDrive-synced document folders.
        let path = dir.join(format!("edit_{id:04}.png"));
        fs::write(&path, &exported.png).map_err(|e| format!("Couldn't write temporary image {}: {e}", path.display()))?;
        let baseline = read_stamp(&path).ok();
        self.jobs.push(ExternalEditJob {
            id,
            doc_id,
            page,
            object: exported.object,
            rect: exported.rect,
            temp_path: path.clone(),
            last_applied_stamp: baseline,
            last_error_stamp: None,
            candidate_stamp: None,
            stable_polls: 0,
            pending_bytes: None,
        });
        Ok((id, path))
    }

    pub(crate) fn abort_job(&mut self, job_id: u64) {
        if let Some(pos) = self.jobs.iter().position(|j| j.id == job_id) {
            let job = self.jobs.remove(pos);
            let _ = fs::remove_file(&job.temp_path);
        }
    }

    pub(crate) fn close_doc(&mut self, doc_id: DocId) {
        let mut i = 0;
        while i < self.jobs.len() {
            if self.jobs[i].doc_id == doc_id {
                let job = self.jobs.remove(i);
                let _ = fs::remove_file(&job.temp_path);
            } else {
                i += 1;
            }
        }
    }

    pub(crate) fn cleanup_all(&mut self) {
        self.jobs.clear();
        if let Some(dir) = self.temp_dir.take() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    pub(crate) fn poll_due(&mut self, ctx: &egui::Context) {
        if self.jobs.is_empty() {
            return;
        }
        ctx.request_repaint_after(REPAINT_AFTER);
        let now = Instant::now();
        if self.last_poll.is_some_and(|t| now.saturating_duration_since(t) < POLL_INTERVAL) {
            return;
        }
        self.last_poll = Some(now);
        self.poll_now();
    }

    /// Inspect every watched temp PNG once. Two consecutive identical `(modified, len)` stamps
    /// confirm the external editor has finished writing before bytes are read.
    pub(crate) fn poll_now(&mut self) {
        for job in &mut self.jobs {
            poll_job(job);
        }
    }

    pub(crate) fn active_pending_ids(&self, active_doc: DocId) -> Vec<u64> {
        self.jobs.iter().filter(|j| j.doc_id == active_doc && j.pending_bytes.is_some()).map(|j| j.id).collect()
    }

    fn take_pending(&mut self, job_id: u64) -> Option<PendingExternalEdit> {
        let job = self.jobs.iter_mut().find(|j| j.id == job_id)?;
        let (stamp, bytes) = job.pending_bytes.take()?;
        Some(PendingExternalEdit { page: job.page, object: job.object, rect: job.rect, stamp, bytes })
    }

    fn mark_applied(&mut self, job_id: u64, stamp: FileStamp, new_object: Option<ObjRef>) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            job.last_applied_stamp = Some(stamp);
            job.last_error_stamp = None;
            if let Some(obj) = new_object {
                job.object = obj;
            }
        }
    }

    fn mark_error(&mut self, job_id: u64, stamp: FileStamp) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            job.last_error_stamp = Some(stamp);
        }
    }

    #[cfg(test)]
    pub(crate) fn job_count(&self) -> usize {
        self.jobs.len()
    }
}

fn poll_job(job: &mut ExternalEditJob) {
    let stamp = match read_stamp(&job.temp_path) {
        Ok(s) => s,
        Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::PermissionDenied) => {
            // Atomic saves (write temp + rename/ReplaceFile) or AV scans briefly hide or lock the file.
            job.candidate_stamp = None;
            job.stable_polls = 0;
            return;
        }
        Err(_) => {
            job.candidate_stamp = None;
            job.stable_polls = 0;
            return;
        }
    };
    if Some(stamp) == job.last_applied_stamp || Some(stamp) == job.last_error_stamp {
        job.candidate_stamp = None;
        job.stable_polls = 0;
        return;
    }
    if job.pending_bytes.as_ref().is_some_and(|(s, _)| *s == stamp) {
        job.candidate_stamp = None;
        job.stable_polls = 0;
        return;
    }
    if job.candidate_stamp == Some(stamp) {
        job.stable_polls = job.stable_polls.saturating_add(1);
        if job.stable_polls >= 2 {
            match fs::read(&job.temp_path) {
                Ok(bytes) if looks_like_complete_png(&bytes) => {
                    job.pending_bytes = Some((stamp, bytes));
                    job.candidate_stamp = None;
                    job.stable_polls = 0;
                }
                _ => {
                    // Empty, half-written PNG, or sharing violation: retry next poll.
                    job.stable_polls = 0;
                }
            }
        }
    } else {
        job.candidate_stamp = Some(stamp);
        job.stable_polls = 1;
    }
}

fn looks_like_complete_png(bytes: &[u8]) -> bool {
    // Every complete PNG starts with the 8-byte signature and ends with the IEND chunk + CRC.
    const IEND_CHUNK: [u8; 8] = [b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82];
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.windows(8).rev().take(16).any(|w| w == IEND_CHUNK)
}

fn spawn_detached(program: &OsStr, args: &[&OsStr]) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    // Never wait on or kill the spawned process: single-instance editors hand off and exit
    // immediately while the main editor window remains open for many subsequent saves.
    cmd.spawn().map(|_| ()).map_err(|e| format!("Couldn't launch {}: {e}", Path::new(program).display()))
}

#[cfg(target_os = "windows")]
fn find_in_path(names: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn find_gimp_executable() -> Option<PathBuf> {
    if let Some(p) = find_in_path(&["gimp-3.exe", "gimp-3.0.exe", "gimp-2.10.exe", "gimp.exe"]) {
        return Some(p);
    }
    let mut roots = Vec::new();
    for env_key in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(val) = std::env::var_os(env_key) {
            let base = PathBuf::from(val);
            roots.push(base.clone());
            roots.push(base.join("Programs"));
        }
    }
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        let mut gimp_dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("gimp")))
            .collect();
        gimp_dirs.sort();
        gimp_dirs.reverse();
        for dir in gimp_dirs {
            let bin = dir.join("bin");
            for exe in ["gimp-3.exe", "gimp-3.0.exe", "gimp-2.10.exe", "gimp.exe"] {
                let candidate = bin.join(exe);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn find_photoshop_executable() -> Option<PathBuf> {
    if let Some(p) = find_in_path(&["Photoshop.exe"]) {
        return Some(p);
    }
    for env_key in ["ProgramFiles", "ProgramFiles(x86)"] {
        let Some(val) = std::env::var_os(env_key) else { continue };
        let adobe = PathBuf::from(val).join("Adobe");
        let Ok(entries) = fs::read_dir(&adobe) else { continue };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains("photoshop")))
            .collect();
        dirs.sort();
        dirs.reverse();
        for dir in dirs {
            let exe = dir.join("Photoshop.exe");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn launch_editor(editor: ExternalEditor, custom_program: Option<&Path>, image_path: &Path) -> Result<(), String> {
    let img_arg = image_path.as_os_str();
    match editor {
        ExternalEditor::Other => {
            let program = custom_program.ok_or_else(|| "No external program was selected.".to_string())?;
            spawn_detached(program.as_os_str(), &[img_arg])
        }
        ExternalEditor::Paint => launch_paint(img_arg),
        ExternalEditor::Gimp => launch_gimp(img_arg),
        ExternalEditor::Photoshop => launch_photoshop(img_arg),
        ExternalEditor::SystemOpenWith => launch_system_open_with(img_arg),
    }
}

#[cfg(target_os = "windows")]
fn launch_paint(img_arg: &OsStr) -> Result<(), String> {
    let sys_paint = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("mspaint.exe")).filter(|p| p.is_file());
    let exe = sys_paint.unwrap_or_else(|| PathBuf::from("mspaint.exe"));
    spawn_detached(exe.as_os_str(), &[img_arg])
}

#[cfg(not(target_os = "windows"))]
fn launch_paint(_img_arg: &OsStr) -> Result<(), String> {
    Err("Paint is only available on Windows; choose GIMP, Photoshop, or Other program….".into())
}

#[cfg(target_os = "windows")]
fn launch_gimp(img_arg: &OsStr) -> Result<(), String> {
    let exe = find_gimp_executable().ok_or_else(|| "Couldn't find GIMP. Use Edit with ▸ Other program… to locate it.".to_string())?;
    spawn_detached(exe.as_os_str(), &[img_arg])
}

#[cfg(target_os = "macos")]
fn launch_gimp(img_arg: &OsStr) -> Result<(), String> {
    spawn_detached(OsStr::new("open"), &[OsStr::new("-a"), OsStr::new("GIMP"), img_arg])
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn launch_gimp(img_arg: &OsStr) -> Result<(), String> {
    spawn_detached(OsStr::new("gimp"), &[img_arg])
}

#[cfg(target_os = "windows")]
fn launch_photoshop(img_arg: &OsStr) -> Result<(), String> {
    let exe = find_photoshop_executable().ok_or_else(|| "Couldn't find Photoshop. Use Edit with ▸ Other program… to locate it.".to_string())?;
    spawn_detached(exe.as_os_str(), &[img_arg])
}

#[cfg(target_os = "macos")]
fn launch_photoshop(img_arg: &OsStr) -> Result<(), String> {
    spawn_detached(OsStr::new("open"), &[OsStr::new("-a"), OsStr::new("Adobe Photoshop"), img_arg])
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn launch_photoshop(_img_arg: &OsStr) -> Result<(), String> {
    Err("Photoshop wasn't found; use Edit with ▸ Other program… to locate an image editor.".into())
}

#[cfg(target_os = "windows")]
fn launch_system_open_with(img_arg: &OsStr) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let rundll = std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("rundll32.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("rundll32.exe"));
    // shell32.dll,OpenAs_RunDLL does not strip surrounding quotes when parsing its path argument,
    // so pass the path via raw_arg without automatic Rust argument quoting.
    let mut cmd = Command::new(&rundll);
    cmd.raw_arg("shell32.dll,OpenAs_RunDLL").raw_arg(img_arg);
    cmd.spawn().map(|_| ()).map_err(|e| format!("Couldn't launch {}: {e}", rundll.display()))
}

#[cfg(target_os = "macos")]
fn launch_system_open_with(img_arg: &OsStr) -> Result<(), String> {
    spawn_detached(OsStr::new("open"), &[img_arg])
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn launch_system_open_with(img_arg: &OsStr) -> Result<(), String> {
    spawn_detached(OsStr::new("xdg-open"), &[img_arg])
}

impl PdfCraftApp {
    /// Edit text & images ▸ Edit with: export a raster figure to a private temp PNG and open it
    /// in an external program.
    pub(crate) fn edit_page_image_with(&mut self, page: usize, index: usize, editor: ExternalEditor) {
        if editor == ExternalEditor::Other {
            self.pick_external_image_editor(page, index);
            return;
        }
        self.start_external_image_edit(page, index, editor, None);
    }

    fn pick_external_image_editor(&mut self, page: usize, index: usize) {
        let mut request = PickRequest::open("external-editor").title(tl!("Choose image editor"));
        #[cfg(target_os = "windows")]
        {
            request = request.filter(tl!("Programs"), &["exe", "bat", "cmd"]);
        }
        #[cfg(target_os = "macos")]
        {
            request = request.filter(tl!("Applications"), &["app"]);
        }
        let target = self.pick_target(self.active_ids().map(|(_, id)| id));
        self.pick_first(request, target, move |app: &mut Self, _ctx: &egui::Context, program: PathBuf| {
            app.start_external_image_edit(page, index, ExternalEditor::Other, Some(program));
        });
    }

    fn start_external_image_edit(&mut self, page: usize, index: usize, editor: ExternalEditor, custom_program: Option<PathBuf>) {
        let Some((_, doc_id)) = self.active_ids() else { return };
        if !self.may_copy(doc_id) {
            return;
        }
        if self.session.get(doc_id).is_some_and(|d| !d.allows_modification()) {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return;
        }
        let dpi = self.edit_image_dpi.as_dpi();
        let exported = match self.session.get(doc_id).map(|d| d.page_image_png(page, index, dpi)) {
            Some(Ok(exported)) => exported,
            Some(Err(e)) => {
                self.notify_fmt("Couldn't export the image for editing: {e}", &[("e", &e)]);
                return;
            }
            None => return,
        };
        let (job_id, temp_path) = match self.external_images.stage_export(doc_id, page, &exported) {
            Ok(staged) => staged,
            Err(e) => {
                self.notify_error(&e);
                return;
            }
        };
        if let Err(e) = launch_editor(editor, custom_program.as_deref(), &temp_path) {
            self.external_images.abort_job(job_id);
            self.notify_error(&e);
        }
    }

    /// Poll watched temporary PNGs and apply any stabilized save belonging to the active tab.
    pub(crate) fn poll_external_images(&mut self, ctx: &egui::Context) {
        self.external_images.poll_due(ctx);
        if self.modal_open() {
            return;
        }
        let Some((_, active_id)) = self.active_ids() else { return };
        let ready = self.external_images.active_pending_ids(active_id);
        for job_id in ready {
            let Some(PendingExternalEdit { page, object, rect, stamp, bytes }) = self.external_images.take_pending(job_id) else {
                continue;
            };
            let resolved = match self.session.get(active_id) {
                Some(doc) => doc.resolve_page_image(page, object, rect),
                None => continue,
            };
            let index = match resolved {
                Ok(idx) => idx,
                Err(msg) => {
                    self.external_images.mark_error(job_id, stamp);
                    self.notify_error(&msg);
                    continue;
                }
            };
            let edit = Edit::EditPageImage { page, index, change: ImageEdit::Replace { name: "edited.png".into(), bytes: Arc::new(bytes) } };
            if self.apply_edit(ctx, edit) {
                let new_obj = self.session.get(active_id).and_then(|d| d.page_images(page).into_iter().nth(index)).and_then(|im| im.object);
                self.external_images.mark_applied(job_id, stamp, new_obj);
            } else {
                self.external_images.mark_error(job_id, stamp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_png(tag: &[u8]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend_from_slice(tag);
        out.extend_from_slice(b"IEND\xAE\x42\x60\x82");
        out
    }

    fn sample_export(obj_num: u32) -> ExternalPageImage {
        ExternalPageImage { png: fake_png(b"initial"), width: 4, height: 4, object: ObjRef::new(obj_num, 0), rect: [10.0, 20.0, 50.0, 60.0] }
    }

    #[test]
    fn two_poll_stability_and_transient_missing_recovery() {
        let mut tracker = ExternalImageTracker::default();
        let doc = DocId(1);
        let (job_id, path) = tracker.stage_export(doc, 0, &sample_export(10)).unwrap();

        // Initial baseline matches: polling yields nothing.
        tracker.poll_now();
        assert!(tracker.active_pending_ids(doc).is_empty());

        // Write updated bytes (different length guarantees distinct composite stamp even on coarse mtime).
        let updated = fake_png(b"updated-longer-payload");
        fs::write(&path, &updated).unwrap();

        // Poll 1: candidate recorded, not yet stable.
        tracker.poll_now();
        assert!(tracker.active_pending_ids(doc).is_empty());

        // Transient NotFound (e.g. atomic replace) resets stability without dropping the watch.
        fs::remove_file(&path).unwrap();
        tracker.poll_now();
        assert_eq!(tracker.job_count(), 1);
        assert!(tracker.active_pending_ids(doc).is_empty());

        // Re-create with updated bytes: 1st poll sets candidate, 2nd poll marks ready.
        fs::write(&path, &updated).unwrap();
        tracker.poll_now();
        assert!(tracker.active_pending_ids(doc).is_empty());
        tracker.poll_now();
        assert_eq!(tracker.active_pending_ids(doc), vec![job_id]);

        let taken = tracker.take_pending(job_id).unwrap();
        assert_eq!(taken.bytes, updated);
        tracker.mark_applied(job_id, taken.stamp, Some(ObjRef::new(11, 0)));
        assert_eq!(tracker.jobs[0].object, ObjRef::new(11, 0));
    }

    #[test]
    fn background_tab_coalesces_saves_and_waits_for_foreground() {
        let mut tracker = ExternalImageTracker::default();
        let bg_doc = DocId(1);
        let fg_doc = DocId(2);
        let (job_id, path) = tracker.stage_export(bg_doc, 0, &sample_export(20)).unwrap();

        let first = fake_png(b"first-save");
        fs::write(&path, &first).unwrap();
        tracker.poll_now();
        tracker.poll_now();
        assert!(tracker.active_pending_ids(fg_doc).is_empty());

        let second = fake_png(b"second-save-even-longer");
        fs::write(&path, &second).unwrap();
        tracker.poll_now();
        tracker.poll_now();
        assert!(tracker.active_pending_ids(fg_doc).is_empty());

        // Returning bg_doc to the foreground delivers only the latest save once.
        assert_eq!(tracker.active_pending_ids(bg_doc), vec![job_id]);
        let pending = tracker.take_pending(job_id).unwrap();
        assert_eq!(pending.bytes, second);

        tracker.close_doc(bg_doc);
        assert_eq!(tracker.job_count(), 0);
        assert!(!path.exists());
    }
}
