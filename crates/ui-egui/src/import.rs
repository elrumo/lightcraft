//! The import review dialog (File → Import Photos… / Import from Folder… / Import from Device):
//! the source it scanned (a scanned folder is *not* added to Local), the files found under it as
//! a grid of thumbnails with checkboxes (duplicates marked and unchecked; Shift-click sets a
//! range), the destination (add in place / copy or move into the library's `Originals/` or a
//! chosen folder, filed by day, by month, into one folder or by a custom folder template,
//! optionally renamed), an album (existing or new), a preset and keywords to apply. Importing
//! runs on a worker thread (files are probed, copied or moved there) and its batches join the
//! catalog between frames, with a row and ✕ in the activity stack (issue #345); the whole import is
//! one undo step.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use lightcraft_engine::import::{
    ImportCandidate, ImportUprightMode, ImportWarning, ImportWarningCode, ScanInput, ScanOutput, ScanProgress, scan_with,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

use crate::LightcraftApp;
use crate::render::Slot;
use crate::theme::Tokens;
use crate::widgets::register;

/// The review dialog's first size (points). It can be resized; the photo grid takes the height.
pub(crate) const DIALOG_SIZE: [f32; 2] = [960.0, 720.0];
/// Memory keys: where the photo grid ended and where the dialog's content (its buttons) ended, last
/// frame. Their difference is the room the grid leaves below it.
const GRID_BOTTOM: &str = "import-grid-bottom";
const CONTENT_BOTTOM: &str = "import-content-bottom";
/// Room below the grid before it has been measured (more than the options and buttons take).
const DEFAULT_BELOW_GRID: f32 = 360.0;

/// Height of the photo grid: what is `available` once `below` (options and buttons) is left free,
/// in whole points so the window never grows by a fraction each frame, and at least one `row`.
pub(crate) fn grid_height(available: f32, below: f32, row: f32) -> f32 {
    let h = (available - below.max(0.0)).floor();
    if h.is_finite() { h.max(row) } else { (DIALOG_SIZE[1] - DEFAULT_BELOW_GRID).max(row) }
}

/// Remember where the review dialog's content ends this frame (after its buttons), for
/// [`grid_height`] on the next.
pub(crate) fn note_dialog_bottom(ui: &egui::Ui) {
    if !ui.is_sizing_pass() {
        let y = ui.min_rect().bottom();
        ui.ctx().data_mut(|m| m.insert_temp(egui::Id::new(CONTENT_BOTTOM), y));
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportDialog {
    /// The files / folders that were scanned (shown as the source).
    pub sources: Vec<String>,
    pub candidates: Vec<ImportCandidate>,
    pub checked: Vec<bool>,
    /// Copy into the library (else add in place).
    pub copy: bool,
    /// With `copy`: move instead — the originals are removed from the source once each copy is
    /// verified and catalogued (`library.import` mode `move`).
    pub move_files: bool,
    /// Existing album to add to.
    pub album: Option<u64>,
    /// …or a new album with this name.
    pub new_album: String,
    /// Preset id ("" = none).
    pub preset: String,
    /// Comma-separated keywords.
    pub keywords: String,
    /// Copy: destination folder ("" = the library's Originals/).
    pub destination: String,
    /// Copy: `date` (YYYY/YYYY-MM-DD), `month`, `flat` or `custom` ([`ImportDialog::folder_template`]).
    pub organize: String,
    /// Copy, `custom`: the folder template, e.g. `{date:%Y}/{date:%Y%m%d}`.
    pub folder_template: String,
    /// Copy: file-name template for the copies ("" = keep the names).
    pub rename: String,
    /// Metadata preset name ("" = none).
    pub metadata_preset: String,
    /// Copy: raws are copied as DNG.
    pub dng: bool,
    /// The files are the host's temporary copies (iOS pickers hand over copies in the app's tmp
    /// folder, which the system may empty): they are always moved into the library's Originals/,
    /// never added in place.
    pub staged: bool,
    /// Per candidate: the library already has it in Recently Deleted ([`ImportDialog::set_trashed`]).
    pub trashed: Vec<bool>,
    /// What to do with those: `restore` (the default: the photo comes back with its edits), `fresh`
    /// (imported as a new photo, the old record deleted) or "" (leave them) — the `onDeleted` param.
    pub on_deleted: String,
    /// The candidate clicked last: where a Shift-click range starts ([`ImportDialog::click`]).
    #[serde(skip)]
    pub last_clicked: Option<usize>,
    /// The photos are on a camera, phone or card the host reads itself (`Services::cameras`): the
    /// candidates' paths are its files' ids, and Import copies the checked ones off it first
    /// ([`crate::camera_import`]).
    pub camera: Option<crate::camera_import::CameraSource>,
    /// The camera or card the photos are on, by name ("" = not a device): the review shows it
    /// above the photos, with a box that checks all of them.
    pub device: String,
    /// The library the scan was made in ([`lightcraft_engine::Session::library_generation`]): its duplicate
    /// decisions and album ids belong to that catalog, so the review is not shown or confirmed in another.
    #[serde(skip)]
    pub library: Option<u64>,
}

/// A run of candidates taken on the same day (the review lists them by day, newest first).
#[derive(Clone, Debug, PartialEq)]
pub struct DayGroup {
    /// `2026-10-03`; `None`: no capture date.
    pub day: Option<String>,
    pub range: std::ops::Range<usize>,
}

/// The day a candidate was taken, if it says.
fn candidate_day(c: &ImportCandidate) -> Option<&str> {
    lightcraft_engine::devices::capture_day(c.captured.as_deref())
}

/// Raw formats, as a candidate's format or extension names them (its "Raw" tag).
const RAW_FORMATS: &[&str] = &[
    "3FR", "ARW", "CR2", "CR3", "CRW", "DCR", "DNG", "ERF", "FFF", "IIQ", "KDC", "MEF", "MOS", "MRW", "NEF", "NRW", "ORF", "PEF", "RAF", "RAW",
    "RW2", "RWL", "SR2", "SRF", "SRW", "X3F",
];

/// The candidate is a raw file.
pub fn is_raw(c: &ImportCandidate) -> bool {
    RAW_FORMATS.contains(&file_type(c).as_str())
}

/// Order candidates by the day they were taken, newest day first (undated last), keeping their
/// order within a day (a camera numbers its files as it takes them).
pub fn sort_by_day(candidates: &mut [ImportCandidate]) {
    candidates.sort_by(|a, b| match (candidate_day(a), candidate_day(b)) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

/// A candidate's file type, as the review groups them: its format, or its extension when the
/// probe named none (`JPEG` and `JPG` are one type).
fn file_type(c: &ImportCandidate) -> String {
    let f = if c.format.trim().is_empty() {
        std::path::Path::new(&c.path).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        c.format.clone()
    };
    match f.trim().to_uppercase().as_str() {
        "JPEG" => "JPG".into(),
        "TIFF" => "TIF".into(),
        "" => "?".into(),
        other => other.to_string(),
    }
}

impl ImportDialog {
    pub fn new(candidates: Vec<ImportCandidate>) -> Self {
        let checked = candidates.iter().map(|c| c.duplicate.is_none() && c.error.is_none()).collect();
        ImportDialog { candidates, checked, on_deleted: "restore".into(), ..Default::default() }
    }

    /// Whether the library the scan was made in is still the open one (a review that names none counts).
    fn in_library(&self, app: &LightcraftApp) -> bool {
        self.library.is_none_or(|g| g == app.session.library_generation())
    }
    /// Mark the candidates whose photo is in Recently Deleted. They are checked when the choice
    /// for them (`on_deleted`) imports them — by default it restores them, so a photo deleted by
    /// mistake comes back by importing its file again (issue #706).
    pub fn set_trashed(&mut self, trashed: Vec<bool>) {
        self.trashed = trashed;
        for i in 0..self.candidates.len() {
            if !self.is_trashed(i) {
                continue;
            }
            let on = self.checked_by_default(i);
            if let Some(c) = self.checked.get_mut(i) {
                *c = on;
            }
        }
    }
    /// Candidate `i` can be checked: a readable file the library doesn't have, or one whose photo
    /// is in Recently Deleted — even with "Leave them", a click picks it (issue #1285).
    pub fn importable(&self, i: usize) -> bool {
        self.candidates.get(i).is_some_and(|c| c.error.is_none() && (c.duplicate.is_none() || self.is_trashed(i)))
    }
    /// Candidate `i` is checked by Check All and the file type toggles: with "Leave them", a file in
    /// Recently Deleted is only checked by hand.
    pub fn checked_by_default(&self, i: usize) -> bool {
        self.importable(i) && !(self.is_trashed(i) && self.on_deleted.is_empty())
    }
    /// The file types among the candidates (`ARW`, `JPG`…, upper case, sorted) and how many of each.
    pub fn file_types(&self) -> Vec<(String, usize)> {
        let mut types: std::collections::BTreeMap<String, usize> = Default::default();
        for c in &self.candidates {
            *types.entry(file_type(c)).or_default() += 1;
        }
        types.into_iter().collect()
    }
    /// Any file of type `ty` is checked.
    pub fn type_checked(&self, ty: &str) -> bool {
        self.candidates.iter().zip(&self.checked).any(|(c, on)| *on && file_type(c) == ty)
    }
    /// Check (the importable ones) or uncheck every file of type `ty`: the raws only, say, and not
    /// the JPEGs kept beside them (issue #344).
    pub fn check_type(&mut self, ty: &str, on: bool) {
        for i in 0..self.candidates.len() {
            let same = self.candidates.get(i).is_some_and(|c| file_type(c) == ty);
            let importable = self.checked_by_default(i);
            if same && let Some(c) = self.checked.get_mut(i) {
                *c = on && importable;
            }
        }
    }
    /// The candidates in runs of the same day ([`sort_by_day`] makes one run per day).
    pub fn groups(&self) -> Vec<DayGroup> {
        let mut out: Vec<DayGroup> = Vec::new();
        for (i, c) in self.candidates.iter().enumerate() {
            let day = candidate_day(c);
            match out.last_mut() {
                Some(g) if g.day.as_deref() == day => g.range.end = i + 1,
                _ => out.push(DayGroup { day: day.map(str::to_string), range: i..i + 1 }),
            }
        }
        out
    }
    /// Candidate `i` counts for a day's (or the device's) box: one Check All would check, or one
    /// checked by hand (a file in Recently Deleted with "Leave them").
    fn in_box(&self, i: usize) -> bool {
        self.checked_by_default(i) || self.importable(i) && self.checked.get(i).copied().unwrap_or(false)
    }
    /// Whether a box over `range` can be clicked: it has a candidate to check or uncheck.
    pub fn range_enabled(&self, range: std::ops::Range<usize>) -> bool {
        range.into_iter().any(|i| self.in_box(i))
    }
    /// Whether the candidates in `range` that count for its box are checked: all (`Some(true)`),
    /// none (`Some(false)`) or some (`None`). None count: `Some(false)`.
    pub fn range_checked(&self, range: std::ops::Range<usize>) -> Option<bool> {
        let (mut on, mut off) = (false, false);
        for i in range.filter(|i| self.in_box(*i)) {
            if self.checked.get(i).copied().unwrap_or(false) {
                on = true;
            } else {
                off = true;
            }
        }
        match (on, off) {
            (true, true) => None,
            (true, false) => Some(true),
            _ => Some(false),
        }
    }
    /// A day's (or the whole device's) box: checks the candidates in `range` as Check All does, or
    /// unchecks them all when they are all checked.
    pub fn toggle_range(&mut self, range: std::ops::Range<usize>) {
        let on = self.range_checked(range.clone()) != Some(true);
        for i in range {
            let importable = self.checked_by_default(i);
            if let Some(c) = self.checked.get_mut(i) {
                *c = on && importable;
            }
        }
    }
    /// A camera's files as the candidates (as it catalogues them, again and again): supported
    /// formats only, by day, those `existing` finds in the library not importable. Checks made
    /// earlier stay; new files arrive checked when they can be imported.
    pub fn set_camera_files(
        &mut self,
        files: Vec<lightcraft_engine::devices::CameraFile>,
        existing: impl Fn(&lightcraft_engine::devices::CameraFile) -> Option<u64>,
    ) {
        let was = crate::camera_import::checked_by_id(self);
        let mut candidates: Vec<ImportCandidate> = files
            .into_iter()
            .filter(|f| lightcraft_engine::import::is_supported(std::path::Path::new(&f.name)))
            .map(|f| {
                let found = existing(&f);
                crate::camera_import::candidate(f, found)
            })
            .collect();
        sort_by_day(&mut candidates);
        self.checked = candidates.iter().map(|c| c.duplicate.is_none() && was.get(&c.path).copied().unwrap_or(true)).collect();
        self.trashed = vec![false; candidates.len()];
        self.candidates = candidates;
        self.last_clicked = None;
    }
    /// Candidate `i` is a file whose photo is in Recently Deleted.
    pub fn is_trashed(&self, i: usize) -> bool {
        self.trashed.get(i).copied().unwrap_or(false)
    }
    /// Choose what happens to the files in Recently Deleted (`""`, `restore` or `fresh`); they are
    /// checked once there is a choice and unchecked again without one.
    pub fn set_on_deleted(&mut self, choice: &str) {
        // switching between restore and fresh keeps the user's per-cell choices
        let changed = self.on_deleted.is_empty() != choice.is_empty();
        self.on_deleted = choice.to_string();
        for i in (0..self.candidates.len()).filter(|_| changed) {
            if self.is_trashed(i)
                && let Some(c) = self.checked.get_mut(i)
            {
                *c = !choice.is_empty();
            }
        }
    }
    /// A click on candidate `i` toggles its checkbox. With `shift`, every importable candidate
    /// from the last clicked one to `i` takes `i`'s new state, as in a file manager: click the
    /// first, Shift-click the last, and the whole range is checked (or unchecked).
    pub fn click(&mut self, i: usize, shift: bool) {
        if !self.importable(i) {
            return;
        }
        let Some(on) = self.checked.get(i).map(|c| !c) else { return };
        let from = if shift { self.last_clicked.filter(|a| *a < self.checked.len()).unwrap_or(i) } else { i };
        for k in from.min(i)..=from.max(i) {
            if self.importable(k)
                && let Some(c) = self.checked.get_mut(k)
            {
                *c = on;
            }
        }
        self.last_clicked = Some(i);
    }
    pub fn selected_paths(&self) -> Vec<String> {
        self.candidates
            .iter()
            .zip(&self.checked)
            .enumerate()
            .filter(|(i, (_, on))| **on && self.importable(*i))
            .map(|(_, (c, _))| c.path.clone())
            .collect()
    }
    /// The `organize` param of `library.import` (`None` = the default, by day); an unusable
    /// custom folder template is an error.
    pub fn organize_param(&self) -> Result<Option<String>, String> {
        match self.organize.as_str() {
            "" => Ok(None),
            "custom" => {
                let t = self.folder_template.trim();
                if let Some(e) = lightcraft_engine::rename::folder_template_error(t) {
                    return Err(e);
                }
                // a plain folder name ("Imports") is a one-level template too
                Ok(Some(if t.contains(['{', '/', '\\']) { t.to_string() } else { format!("{t}/") }))
            }
            o => Ok(Some(o.to_string())),
        }
    }
    fn keywords(&self) -> Vec<String> {
        self.keywords.split(',').map(str::trim).filter(|k| !k.is_empty()).map(str::to_string).collect()
    }
}

/// A running import (see the module docs). The file-system work — expanding folders, probing,
/// reading sidecars, copying or placing files — runs on a worker thread
/// ([`lightcraft_engine::import::ImportJob`]); each batch it readies is added to the catalog on the
/// UI thread between frames, so a slow drive never stalls the window.
#[derive(Debug, Default)]
pub struct ImportTask {
    /// Files and folders to import (handed to the worker when the task starts).
    queue: Vec<String>,
    pub total: usize,
    pub done: usize,
    params: Value,
    /// What the batches added so far (shared with the engine host).
    pub tally: lightcraft_apphost::import::Tally,
    undo0: usize,
    /// Reading a folder for the Local view: the photos stay out of the library, and nothing is
    /// selected or announced as added.
    browse: bool,
    /// Cancel was pressed: no further files are started; batches already readied are added.
    pub cancelled: bool,
    /// Auto Import (the watched folder): the selection stays as it is, a short toast when done.
    auto: bool,
    /// Auto Import: the selection to keep.
    keep_selection: Option<lightcraft_engine::Selection>,
    /// The selection and source as the import last left them: a change between frames is the
    /// user choosing another photo or view, which the end of the import doesn't take away.
    left: Option<(lightcraft_engine::Selection, lightcraft_engine::LibrarySource)>,
    /// The user chose another photo or view while the import ran.
    moved_on: bool,
    run: Option<lightcraft_apphost::import::Run>,
    /// The import's row in the activity stack (its ✕ and `activity.cancel` set the run's cancel flag).
    guard: Option<lightcraft_engine::activity::TaskGuard>,
    /// Photos dropped on the window: the grid holds a tile for each until it is in the catalog
    /// ([`crate::file_drop`]).
    pending: Option<crate::file_drop::Pendings>,
    /// A staging folder of the app's own the files were copied into (off a camera): removed
    /// when the import ends (what is left there are duplicates and failures, still on the camera).
    pub cleanup: Option<std::path::PathBuf>,
}

impl ImportTask {
    /// An import of `paths` (files or folders) with `library.import` params (without `paths`).
    pub fn new(paths: Vec<String>, params: Value, undo0: usize, browse: bool) -> Self {
        ImportTask { total: paths.len(), queue: paths, params, undo0, browse, ..Default::default() }
    }

    /// An Auto Import (see [`ImportTask::auto`]).
    pub fn auto(mut self) -> Self {
        self.auto = true;
        self
    }

    /// `{done, total, imported, cancelled, pending}` for `ui.inspect`.
    pub fn status(&self) -> Value {
        let pending = self.pending.as_ref().map_or(0, |p| p.len());
        json!({"done": self.done, "total": self.total, "imported": self.tally.imported, "cancelled": self.cancelled, "pending": pending})
    }

    /// The dropped photos not in the catalog yet (none for an import that wasn't a drop).
    pub fn pending(&self) -> Option<&crate::file_drop::Pendings> {
        self.pending.as_ref()
    }

    /// The album the photos are added to.
    pub fn album(&self) -> Option<u64> {
        self.params["album"].as_u64()
    }
}

/// Tells the grid's tiles for dropped photos how the import's worker goes, and wakes the window
/// when a batch is ready.
struct DropWatch {
    pending: Option<crate::file_drop::Pendings>,
    ctx: egui::Context,
}

impl lightcraft_apphost::import::Watch for DropWatch {
    fn listed(&self, files: &[String]) {
        // (the files of dropped folders are known now)
        if let Some(p) = &self.pending {
            p.set(files);
            self.ctx.request_repaint();
        }
    }

    fn readying(&self, n: usize) {
        if let Some(p) = &self.pending {
            p.start(n);
        }
    }

    fn ready(&self) {
        self.ctx.request_repaint();
    }
}

/// A folder scan running on a worker thread (a network share can take minutes to read; the
/// window must keep answering meanwhile).
pub struct ScanTask {
    progress: std::sync::Arc<ScanProgress>,
    rx: std::sync::mpsc::Receiver<ScanOutput>,
    /// Open the review with "copy into the library" checked (a camera / card).
    pub copy: bool,
    /// The camera or card being scanned, by name ([`ImportDialog::device`]).
    pub device: String,
    /// The host's temporary copies ([`ImportDialog::staged`]).
    staged: bool,
    /// Open the review with this album chosen (an album's context menu ▸ Import…).
    pub album: Option<u64>,
    /// Browsing a folder (Local): the photos are read in place instead of opening the review.
    browse: bool,
    /// What is being scanned (the review's source).
    sources: Vec<String>,
    /// The scan's row in the activity stack (its ✕ and `activity.cancel` set `progress.cancel`).
    guard: lightcraft_engine::activity::TaskGuard,
}

/// The activity row of a scan: the review's ("Scanning folder") or the Local view's ("Reading folder").
fn scan_guard(app: &LightcraftApp, progress: &ScanProgress, browse: bool) -> lightcraft_engine::activity::TaskGuard {
    let label = if browse { "Reading folder" } else { "Scanning folder" };
    app.session.activity.start("scan", label, lightcraft_engine::activity::Cancel::Flag(progress.cancel.clone()))
}

/// An import and a Synchronize Folder never run at once: each readies its files against the
/// library as it was when it started, so both could add the same file.
pub(crate) fn busy_synchronizing(app: &LightcraftApp) -> Result<(), String> {
    if app.sync_run.is_some() { Err(crate::i18n::tr("A folder is being synchronized").to_string()) } else { Ok(()) }
}

/// Settings → Import → "Show the import review when a camera or card is connected", called with
/// each device scan while it's on. The first look only notes the cards already in. A card that
/// shows up after that opens Import from Device once nothing else is open, and a card that is
/// taken out and put back in asks again.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn cards_seen(app: &mut LightcraftApp, now: Vec<lightcraft_engine::devices::Device>) {
    let Some(known) = app.ui.known_cards.as_mut() else {
        app.ui.known_cards = Some(now.into_iter().map(|d| d.root).collect());
        return;
    };
    known.retain(|root| now.iter().any(|d| d.root == *root));
    let Some(card) = now.into_iter().find(|d| !known.contains(&d.root)) else { return };
    // busy: ask when the window is free (the card isn't noted until then)
    if app.import.is_some() || app.scan.is_some() || app.sync_run.is_some() || app.lightroom.running() || app.ui.dialog.is_some() {
        return;
    }
    known.push(card.root);
    if let Err(e) = app.run("file.addFromDevice", json!({"path": card.path})) {
        log::warn!("import from {}: {e}", card.name);
    }
}

/// Scan `paths` in the background, then open the review dialog (see [`poll_scan`]).
pub fn open(app: &mut LightcraftApp, paths: Vec<String>) -> Result<Value, String> {
    scan(app, paths, false)
}

/// [`open`] for files the host copied into a temporary folder of its own (iOS: the Photos and
/// Files pickers): the review moves them into the library ([`ImportDialog::staged`]).
pub fn open_staged(app: &mut LightcraftApp, paths: Vec<String>) -> Result<Value, String> {
    if !app.session.library.as_ref().is_some_and(|l| l.on_disk) {
        return Err("photos can't be imported: the library isn't saved on this device (see the log for why it couldn't be opened)".into());
    }
    scan(app, paths, true)
}

fn scan(app: &mut LightcraftApp, paths: Vec<String>, staged: bool) -> Result<Value, String> {
    if app.scan.is_some() {
        return Err("a scan is already running".into());
    }
    if app.review_pending.is_some() {
        return Err("an import review is waiting for the open dialog to close".into());
    }
    let sources = paths.clone();
    let (input, paths) = ScanInput::new(&mut app.session, &paths);
    let progress = std::sync::Arc::new(ScanProgress::default());
    let (tx, rx) = std::sync::mpsc::channel();
    let p = progress.clone();
    let job = move || {
        let _ = tx.send(scan_with(input, &paths, &p));
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(job);
    #[cfg(target_arch = "wasm32")]
    job();
    let guard = scan_guard(app, &progress, false);
    app.scan = Some(ScanTask { progress, rx, copy: staged, device: String::new(), staged, album: None, browse: false, sources, guard });
    Ok(json!({"scanning": true}))
}

/// Show a folder's photos in the Local view (`library.browse` from the UI): the view switches at
/// once, the folder is listed and read on a worker thread (a network share can take minutes),
/// and the photos then join the view in small batches under a progress window. A browse already
/// running is replaced; the import review's scan is not.
pub fn browse(app: &mut LightcraftApp, path: &str, subfolders: Option<bool>) -> Result<Value, String> {
    let dir = std::path::absolute(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    if !dir.is_dir() {
        return Err(format!("{path}: not a folder"));
    }
    if app.scan.as_ref().is_some_and(|t| !t.browse) || app.import.as_ref().is_some_and(|t| !t.browse) {
        return Err("an import is running".into());
    }
    let dir_s = dir.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    let subfolders = subfolders.unwrap_or_else(|| app.session.browse.as_ref().is_some_and(|b| b.subfolders));
    let running = app.scan.as_ref().is_some_and(|t| t.browse) || app.import.as_ref().is_some_and(|t| t.browse);
    if running && app.session.browse.as_ref().is_some_and(|b| b.path == dir_s && b.subfolders == subfolders) {
        // already reading this folder: clicking it again must not restart the progress
        app.session.source = lightcraft_engine::LibrarySource::Folder;
        app.session.keep_active_in_view();
        return Ok(json!({"path": dir_s, "subfolders": subfolders, "scanning": true}));
    }
    if let Some(t) = app.scan.take() {
        t.progress.cancel.store(true, Ordering::Relaxed);
    }
    app.import = None;
    app.session.browse = Some(lightcraft_engine::Browse { path: dir_s.clone(), subfolders });
    app.session.source = lightcraft_engine::LibrarySource::Folder;
    // the photos the library already knows from this folder show at once: keep or pick the active one
    app.session.keep_active_in_view();
    let (input, _) = ScanInput::new(&mut app.session, std::slice::from_ref(&dir_s));
    let progress = std::sync::Arc::new(ScanProgress::default());
    let (tx, rx) = std::sync::mpsc::channel();
    let p = progress.clone();
    let root = dir_s.clone();
    let ignore = app.session.import_defaults.ignore.clone();
    let job = move || {
        let files: Vec<String> = if subfolders {
            lightcraft_engine::import::expand(&[root], None, &ignore)
        } else {
            let mut v: Vec<String> = std::fs::read_dir(&root)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|f| {
                            f.is_file()
                                && lightcraft_engine::import::is_supported(f)
                                && !f.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
                        })
                        .map(|f| f.to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        };
        let _ = tx.send(scan_with(input, &files, &p));
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(job);
    #[cfg(target_arch = "wasm32")]
    job();
    let guard = scan_guard(app, &progress, true);
    app.scan =
        Some(ScanTask { progress, rx, copy: false, device: String::new(), staged: false, album: None, browse: true, sources: Vec::new(), guard });
    app.renderer.forget_imports();
    Ok(json!({"path": dir_s, "subfolders": subfolders, "scanning": true}))
}

/// Collect a finished scan and open the review (called every frame).
pub fn poll_scan(app: &mut LightcraftApp, ctx: &egui::Context) {
    show_pending_review(app, ctx);
    let Some(task) = app.scan.as_ref() else { return };
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let out = match task.rx.try_recv() {
        Ok(o) => o,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            if task.guard.is_cancelled() {
                // ✕: the worker stops at its next file; don't wait for it (a NAS read can take a while)
                app.scan = None;
                return;
            }
            let (total, done) = (task.progress.total.load(Ordering::Relaxed), task.progress.done.load(Ordering::Relaxed));
            task.guard.progress(done as u64, total as u64);
            // while the folders are listed the total isn't known: say what is happening instead
            task.guard.detail(if total == 0 { crate::i18n::tr("Looking for photos…") } else { "" });
            return;
        }
        Err(_) => {
            app.scan = None;
            app.toast(ctx, crate::i18n::tr("Scan failed"));
            return;
        }
    };
    let Some(task) = app.scan.take() else { return };
    if task.progress.cancel.load(Ordering::Relaxed) {
        return;
    }
    app.session.import_probes = out.probes;
    if task.browse {
        // the folder and the ones its photos are in count as browsed now, as `library.browse` records them, so
        // their untouched Local photos aren't forgotten while the folder is in use
        if let Some(root) = app.session.browse.as_ref().map(|b| b.path.clone()) {
            let shown: Vec<String> = out.candidates.iter().map(|c| c.path.clone()).collect();
            app.session.note_browsed(&root, &shown);
        }
        // records of files gone from the folder (deleted or moved by another app) that nothing was changed on go,
        // as `library.browse` forgets them (issue #1392)
        if let Some(b) = app.session.browse.clone() {
            let listed: Vec<String> = out.candidates.iter().map(|c| c.path.clone()).collect();
            app.session.forget_gone_from(&b.path, b.subfolders, &listed);
            // sidecars another app (Lightroom) changed since are read again, and the folder is noted as browsed, as
            // `library.browse` does (the new files read theirs as they are added)
            if let Err(e) = app.session.browse_finished(&b.path, &listed) {
                log::warn!("browse {}: {e}", b.path);
            }
        }
        // what the library doesn't know yet joins the Local view
        let queue: Vec<String> =
            out.candidates.iter().filter(|c| c.duplicate != Some("path".into()) && c.error.is_none()).map(|c| c.path.clone()).collect();
        if !queue.is_empty() {
            let undo0 = app.session.undo.len();
            let total = queue.len();
            let _ = total;
            app.import = Some(ImportTask::new(queue, json!({"mode": "add", "local": true}), undo0, true));
        }
        return;
    }
    if out.candidates.is_empty() {
        app.toast(ctx, crate::i18n::tr("No photos found"));
        return;
    }
    app.renderer.forget_imports();
    let mut candidates = out.candidates;
    sort_by_day(&mut candidates);
    let mut d = ImportDialog::new(candidates);
    let trashed = d
        .candidates
        .iter()
        .map(|c| {
            c.duplicate.is_some()
                && c.existing.is_some_and(|id| app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).is_some_and(|p| p.deleted))
        })
        .collect();
    d.set_trashed(trashed);
    d.copy = task.copy;
    d.staged = task.staged;
    d.move_files = task.staged;
    // only an album the review offers (it may have been deleted while the scan ran)
    d.album = task.album.filter(|id| app.session.catalog.album(lightcraft_catalog::AlbumId(*id)).is_some_and(|a| !a.folder && !a.is_smart()));
    d.sources = task.sources;
    d.device = task.device;
    d.library = Some(app.session.library_generation());
    app.review_pending = Some(crate::state::Dialog::Import { opts: Box::new(d) });
    show_pending_review(app, ctx);
}

/// Open the review of a finished scan once no other dialog is open (a face model's licence, Settings…
/// opened while the scan ran stays), checked again shortly while it waits. A review whose library was
/// closed meanwhile (Settings can open another) is dropped.
fn show_pending_review(app: &mut LightcraftApp, ctx: &egui::Context) {
    if app.review_pending.is_none() {
        return;
    }
    if let Some(crate::state::Dialog::Import { opts }) = &app.review_pending
        && !opts.in_library(app)
    {
        app.review_pending = None;
        return;
    }
    if app.ui.dialog.is_none() {
        app.ui.dialog = app.review_pending.take();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Tests: a review scan that never ends while the returned value lives, so a drop meets it running.
#[cfg(test)]
pub(crate) fn stall_scan(app: &mut LightcraftApp) -> Box<dyn std::any::Any> {
    stall_scan_of(app, false)
}

/// Tests: the same, for a Local folder browse scan (`browse`) instead of the review's scan.
#[cfg(test)]
pub(crate) fn stall_scan_of(app: &mut LightcraftApp, browse: bool) -> Box<dyn std::any::Any> {
    let progress = std::sync::Arc::new(ScanProgress::default());
    let (tx, rx) = std::sync::mpsc::channel::<ScanOutput>();
    let guard = scan_guard(app, &progress, false);
    app.scan = Some(ScanTask { progress, rx, copy: false, album: None, browse, sources: Vec::new(), guard, device: String::new(), staged: false });
    Box::new(tx)
}

/// Whether an import or any scan (the review's, or a Local folder browse) is running, or a finished scan's
/// review waits for a dialog to close, which a drop of photos must not replace: `open` refuses while a scan
/// runs, and a browse that finishes replaces `app.import`.
pub fn running(app: &LightcraftApp) -> bool {
    app.import.is_some() || app.scan.is_some() || app.review_pending.is_some()
}

/// The phone layout's progress window while a folder is being scanned (it has no activity stack,
/// where the desktop shows the scan's row).
pub fn scan_progress(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(task) = &app.scan else { return };
    let t = Tokens::get(ctx);
    let total = task.progress.total.load(Ordering::Relaxed);
    let done = task.progress.done.load(Ordering::Relaxed);
    let text = if total == 0 {
        crate::i18n::tr(if task.browse { "Reading folder…" } else { "Looking for photos…" }).to_string()
    } else {
        crate::i18n::tr_format!("Reading photos… {done} of {total}", done = done, total = total)
    };
    let mut cancel = false;
    egui::Window::new(crate::i18n::tr("Scanning"))
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_BOTTOM, [0.0, -80.0])
        .fixed_size([340.0, 80.0])
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(text).color(t.text));
            ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(320.0));
            let r = ui.button(crate::i18n::tr("Cancel"));
            register(ui.ctx(), "button:scanCancel", r.rect);
            cancel = r.clicked();
        });
    if cancel {
        // the worker stops at its next file; don't wait for it (a NAS read can take a while)
        task.progress.cancel.store(true, Ordering::Relaxed);
        app.scan = None;
    }
}

impl ScanTask {
    /// `{done, total}` for `ui.inspect` (total is 0 while the folders are still being listed).
    pub fn status(&self) -> Value {
        json!({"done": self.progress.done.load(Ordering::Relaxed), "total": self.progress.total.load(Ordering::Relaxed)})
    }
}

/// Start importing the dialog's checked files (the dialog's OK / `ui.dialog.confirm`); on a camera,
/// copying them off it first ([`crate::camera_import::start`]). Refused while another import runs:
/// replacing `app.import` would stop its worker.
pub fn start(app: &mut LightcraftApp, d: &ImportDialog) -> Result<Value, String> {
    if d.camera.is_some() {
        return crate::camera_import::start(app, d);
    }
    busy_synchronizing(app)?;
    if !d.in_library(app) {
        return Err("another library was opened since the photos were scanned: nothing was imported".into());
    }
    // a browse scan that finishes starts its own import, replacing this one
    if app.import.is_some() || app.scan.as_ref().is_some_and(|t| t.browse) {
        return Err(crate::i18n::tr("An import is running").to_string());
    }
    let queue = d.selected_paths();
    if queue.is_empty() {
        return Err("no photos selected".into());
    }
    let undo0 = app.session.undo.len();
    let params = import_params(app, d)?;
    let total = queue.len();
    app.import = Some(ImportTask::new(queue, params, undo0, false));
    app.renderer.forget_imports();
    Ok(json!({"importing": total}))
}

/// The `library.import` params (without `paths`) the review's choices make, creating the new
/// album it names.
pub(crate) fn import_params(app: &mut LightcraftApp, d: &ImportDialog) -> Result<Value, String> {
    let mut album = d.album;
    if album.is_none() && !d.new_album.trim().is_empty() {
        let r = app.session.execute("album.create", &json!({"name": d.new_album.trim()})).map_err(|e| e.to_string())?;
        album = r["id"].as_u64();
    }
    let mode = match (d.copy, d.move_files) {
        _ if d.staged => "move",
        (false, _) => "add",
        (true, false) => "copy",
        (true, true) => "move",
    };
    let mut params = json!({"mode": mode, "keywords": d.keywords()});
    if !d.on_deleted.is_empty() {
        params["onDeleted"] = json!(d.on_deleted);
    }
    if let Some(a) = album {
        params["album"] = json!(a);
    }
    if !d.preset.is_empty() {
        params["preset"] = json!(d.preset);
    }
    if !d.metadata_preset.is_empty() {
        params["metadataPreset"] = json!(d.metadata_preset);
    }
    if d.copy || d.staged {
        if !d.destination.trim().is_empty() {
            params["destination"] = json!(d.destination.trim());
        }
        if let Some(o) = d.organize_param()? {
            params["organize"] = json!(o);
        }
        if !d.rename.trim().is_empty() {
            params["rename"] = json!(d.rename.trim());
            params["renameStart"] = json!(1);
        }
        if d.dng && !d.move_files {
            params["dng"] = json!(true);
        }
    }
    Ok(params)
}

/// Start importing `paths` (files or folders) in the background.
pub fn start_paths(app: &mut LightcraftApp, paths: Vec<String>) -> Result<Value, String> {
    start_with(app, paths, None, false)
}

/// Start importing `paths` dropped on the window, into `album` when one is given. The grid shows a
/// tile for each photo from the moment of the drop until it is in the catalog.
pub fn start_dropped(app: &mut LightcraftApp, paths: Vec<String>, album: Option<u64>) -> Result<Value, String> {
    start_with(app, paths, album, true)
}

fn start_with(app: &mut LightcraftApp, paths: Vec<String>, album: Option<u64>, tiles: bool) -> Result<Value, String> {
    busy_synchronizing(app)?;
    if running(app) {
        return Err(crate::i18n::tr("An import is running").to_string());
    }
    let undo0 = app.session.undo.len();
    let total = paths.len();
    let mut params = json!({"mode": "add"});
    if let Some(a) = album {
        params["album"] = json!(a);
    }
    let mut task = ImportTask::new(paths, params, undo0, false);
    if tiles {
        // the files named are known at once; a dropped folder's are listed by the worker once it has read it
        let pending = crate::file_drop::Pendings::default();
        pending.set(task.queue.iter().filter(|p| !std::path::Path::new(p).is_dir()));
        task.pending = Some(pending);
    }
    app.import = Some(task);
    Ok(json!({"importing": total}))
}

/// Advance the import in progress (called every frame): start its worker, then add the batches it
/// has readied to the catalog.
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(mut task) = app.import.take() else { return };
    if task.run.is_none() {
        let watch: std::sync::Arc<dyn lightcraft_apphost::import::Watch> =
            std::sync::Arc::new(DropWatch { pending: task.pending.clone(), ctx: ctx.clone() });
        let started = lightcraft_apphost::import::Run::begin(&mut app.session, std::mem::take(&mut task.queue), &task.params, Some(watch));
        if task.auto {
            task.keep_selection = Some(app.session.selection.clone());
        }
        match started {
            Ok(run) => {
                let label = if task.browse { "Reading folder" } else { "Importing" };
                task.guard = Some(app.session.activity.start("import", label, lightcraft_engine::activity::Cancel::Flag(run.cancel.clone())));
                task.run = Some(run);
            }
            Err(e) => {
                log::warn!("import: {e}");
                app.toast(ctx, crate::i18n::tr_format!("Import failed: {e}", e = e));
                return;
            }
        }
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let Some(run) = task.run.as_mut() else { return };
    let batches = run.next_batches();
    let finished = batches.is_none();
    if task.left.as_ref().is_some_and(|(sel, source)| *sel != app.session.selection || *source != app.session.source) {
        task.moved_on = true;
    }
    for prepared in batches.into_iter().flatten() {
        commit_batch(app, &mut task, prepared);
        // arriving photos don't take over the selection
        if let Some(sel) = &task.keep_selection {
            app.session.selection = sel.clone();
        }
        // a Local folder still on screen: its first photo becomes active once there is one (#1447)
        if task.browse && app.session.source == lightcraft_engine::LibrarySource::Folder {
            app.session.keep_active_in_view();
        }
    }
    task.left = Some((app.session.selection.clone(), app.session.source));
    if let Some(run) = &task.run {
        task.total = run.total.load(Ordering::Relaxed).max(task.total);
        task.done = run.done.load(Ordering::Relaxed);
    }
    if let Some(guard) = &task.guard {
        guard.progress(task.done as u64, task.total as u64);
        // ✕ in the stack (or `activity.cancel`): no further files are started
        if guard.is_cancelled() {
            task.cancelled = true;
        }
    }
    if !finished {
        app.import = Some(task);
        return;
    }
    finish(app, ctx, task);
}

/// Add one readied batch to the catalog (an undo step merged into the import's at the end).
fn commit_batch(app: &mut LightcraftApp, task: &mut ImportTask, prepared: lightcraft_engine::import::Prepared) {
    let Some(run) = task.run.as_mut() else { return };
    let n = prepared.len();
    let r = lightcraft_apphost::import::commit(&mut app.session, run, prepared);
    // the batch's photos are in the catalog (or not coming): their tiles give way in the same frame
    if let Some(p) = &task.pending {
        p.added(n);
    }
    match r {
        Ok(v) => task.tally.add(&v),
        Err(e) => {
            log::warn!("import: {e}");
            task.tally.failed += n;
        }
    }
}

fn xmp_warning_text(warning: &ImportWarning) -> String {
    let path = &warning.path;
    match warning.code {
        ImportWarningCode::CameraProfile => crate::i18n::tr_format!(
            "XMP profile \"{profile}\" was not applied for {path}; colors may differ from Lightroom.",
            profile = warning.profile.as_deref().unwrap_or_default(),
            path = path,
        ),
        ImportWarningCode::UprightGeometry => match warning.mode.unwrap_or(ImportUprightMode::Unknown) {
            ImportUprightMode::Off => crate::i18n::tr_format!(
                "Lightroom geometry for {path} was not preserved; saved transform is ignored because Upright is Off.",
                path = path,
            ),
            ImportUprightMode::Guided => crate::i18n::tr_format!(
                "Lightroom geometry for {path} was not preserved; Guided Upright saved transform is unsupported.",
                path = path,
            ),
            ImportUprightMode::Auto => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Auto")
            ),
            ImportUprightMode::Level => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Level")
            ),
            ImportUprightMode::Vertical => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Vertical")
            ),
            ImportUprightMode::Full => crate::i18n::tr_format!(
                "Lightroom geometry was not preserved for {path}; {mode} will be recalculated.",
                path = path,
                mode = crate::i18n::tr("Full")
            ),
            ImportUprightMode::Unknown => {
                crate::i18n::tr_format!("Lightroom geometry for {path} was not preserved; saved Upright transform is unsupported.", path = path,)
            }
        },
        ImportWarningCode::UnmappedXmp => {
            crate::i18n::tr_format!("XMP adjustments were not imported for {path}: {fields}.", path = path, fields = warning.fields.join(", "),)
        }
    }
}

fn xmp_warnings_text(warnings: &[ImportWarning]) -> String {
    crate::i18n::tr_format!(
        "Import warnings ({n}):\n{warnings}",
        n = warnings.len(),
        warnings = warnings.iter().map(xmp_warning_text).collect::<Vec<_>>().join("\n"),
    )
}

/// The import is done (or cancelled): one undo step, show and select the first photo (unless the
/// user chose another meanwhile), say what happened.
fn finish(app: &mut LightcraftApp, ctx: &egui::Context, task: ImportTask) {
    if let Some(dir) = &task.cleanup
        && let Err(e) = std::fs::remove_dir_all(dir)
    {
        log::warn!("{}: {e}", dir.display());
    }
    let steps = app.session.undo.len().saturating_sub(task.undo0);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let label = if task.tally.imported == 0 && task.tally.restored > 0 {
        crate::i18n::tr_format!("Restore {} Photo{}", task.tally.restored, plural(task.tally.restored))
    } else if task.tally.restored > 0 {
        crate::i18n::tr_format!("Add {} Photo{}, restore {}", task.tally.imported, plural(task.tally.imported), task.tally.restored)
    } else {
        crate::i18n::tr_format!("Add {} Photo{}", task.tally.imported, if task.tally.imported == 1 { "" } else { "s" })
    };
    app.session.merge_undo(steps, &label);
    if task.auto {
        if !task.tally.warnings.is_empty() {
            app.toast_for(ctx, xmp_warnings_text(&task.tally.warnings), 8.0);
        } else if task.tally.imported > 0 {
            app.toast(
                ctx,
                crate::i18n::tr_format!("Auto Import: added {} photo{}", task.tally.imported, if task.tally.imported == 1 { "" } else { "s" }),
            );
        }
        return;
    }
    if task.browse {
        if !task.tally.warnings.is_empty() {
            app.toast_for(ctx, xmp_warnings_text(&task.tally.warnings), 8.0);
        } else if task.tally.failed > 0 {
            app.toast(ctx, crate::i18n::tr_format!("{} photo{} not readable", task.tally.failed, if task.tally.failed == 1 { "" } else { "s" }));
        }
        return;
    }
    if let Some(f) = task.tally.first.filter(|_| !task.moved_on) {
        // a view the new photos aren't in (an album, Picks, a Local folder) would leave the active
        // photo off screen: show All Photos, as Lightroom shows the import's own collection (#1446)
        if !app.session.visible().contains(&lightcraft_catalog::PhotoId(f)) {
            let _ = app.run("library.source", json!({"kind": "all"}));
        }
        let _ = app.run("library.select", json!({"ids": [f]}));
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    // nothing new, only files the library already has: show where they are (Recently Deleted is
    // easy to miss, and the side panel that lists it starts collapsed)
    if task.tally.imported == 0
        && task.tally.restored == 0
        && task.tally.failed == 0
        && task.tally.warnings.is_empty()
        && !task.cancelled
        && show_existing(app, ctx, &task.tally.existing)
    {
        return;
    }
    let mut msg = if task.cancelled {
        crate::i18n::tr_format!("Import cancelled · {} photo{} added", task.tally.imported, plural(task.tally.imported))
    } else if task.tally.imported == 0 && task.tally.restored > 0 {
        crate::i18n::tr_format!("Restored {} photo{} from Recently Deleted", task.tally.restored, plural(task.tally.restored))
    } else {
        crate::i18n::tr_format!("Imported {} photo{}", task.tally.imported, plural(task.tally.imported))
    };
    if task.tally.restored > 0 && (task.tally.imported > 0 || task.cancelled) {
        msg.push_str(&crate::i18n::tr_format!(" · {} restored from Recently Deleted", task.tally.restored));
    }
    // (dropped on an album you are not looking at: this is what says where the photos went)
    if let Some(name) = task.album().and_then(|a| app.session.catalog.album(lightcraft_catalog::AlbumId(a))).map(|a| a.name.clone()) {
        msg.push_str(&format!(" · {} “{name}”", crate::i18n::tr("in album")));
    }
    if task.params["mode"] == "move" {
        msg.push_str(&crate::i18n::tr_format!(" · {} moved", task.tally.moved));
        if task.tally.kept > 0 {
            msg.push_str(&crate::i18n::tr_format!(" · {} original{} left at the source", task.tally.kept, plural(task.tally.kept)));
        }
    }
    if task.tally.duplicates > 0 {
        msg.push_str(&crate::i18n::tr_format!(" · {} duplicate{} skipped", task.tally.duplicates, if task.tally.duplicates == 1 { "" } else { "s" }));
        let deleted = deleted_of(app, &task.tally.existing).len();
        if deleted > 0 {
            msg.push_str(&crate::i18n::tr_format!(" ({} in Recently Deleted)", deleted));
        }
    }
    if task.tally.failed > 0 {
        msg.push_str(&crate::i18n::tr_format!(" · {} not readable", task.tally.failed));
    }
    if task.tally.warnings.is_empty() {
        app.toast(ctx, msg);
    } else {
        msg.push_str("\n\n");
        msg.push_str(&xmp_warnings_text(&task.tally.warnings));
        app.toast_for(ctx, msg, 8.0);
    }
}

/// The photos among `ids` that are in Recently Deleted (each once).
fn deleted_of(app: &LightcraftApp, ids: &[u64]) -> Vec<u64> {
    let mut v: Vec<u64> =
        ids.iter().copied().filter(|&id| app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).is_some_and(|p| p.deleted)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Select the library photos an import skipped as duplicates of, in Recently Deleted (side panel
/// opened, with how to get them back) or in All Photos, and say so. `false` when there are none.
fn show_existing(app: &mut LightcraftApp, ctx: &egui::Context, existing: &[u64]) -> bool {
    let deleted = deleted_of(app, existing);
    let (kind, mut ids) = if deleted.is_empty() { ("all", existing.to_vec()) } else { ("recentlyDeleted", deleted) };
    ids.sort_unstable();
    ids.dedup();
    ids.retain(|&id| app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).is_some());
    if ids.is_empty() || app.run("library.source", json!({"kind": kind})).is_err() {
        return false;
    }
    let _ = app.run("library.select", json!({"ids": ids}));
    app.ui.left_panel = true;
    let n = ids.len();
    let plural = if n == 1 { "" } else { "s" };
    let msg = if kind == "all" {
        crate::i18n::tr_format!("Already in the library: {} photo{} selected in All Photos", n, plural)
    } else {
        crate::i18n::tr_format!(
            "Already in the library, in Recently Deleted: {} photo{} selected · right-click ▸ Restore, or Delete Permanently to import afresh",
            n,
            plural
        )
    };
    app.toast_for(ctx, msg, 6.0);
    true
}

/// The phone layout's progress window while an import runs, with Cancel (files already copied or
/// added stay; nothing new is started). The desktop shows the import's row in the activity stack.
pub fn progress(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(task) = &app.import else { return };
    let t = Tokens::get(ctx);
    let frac = task.done as f32 / task.total.max(1) as f32;
    let text = if task.cancelled {
        crate::i18n::tr("Stopping…").to_string()
    } else if task.browse {
        crate::i18n::tr_format!("Reading photos… {} of {}", task.done, task.total)
    } else {
        crate::i18n::tr_format!("Adding photos… {} of {}", task.done, task.total)
    };
    let mut cancel = false;
    egui::Window::new(crate::i18n::tr("Importing"))
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_BOTTOM, [0.0, -80.0])
        .fixed_size([340.0, 80.0])
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(text).color(t.text));
            ui.add(egui::ProgressBar::new(frac).desired_width(320.0));
            let r = ui.add_enabled(!task.cancelled, egui::Button::new(crate::i18n::tr("Cancel")));
            register(ui.ctx(), "button:importCancel", r.rect);
            cancel = r.clicked();
        });
    if cancel && let Some(task) = app.import.as_mut() {
        task.cancelled = true;
        if let Some(run) = &task.run {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Height of a day's heading in the desktop review, and on a phone.
const DAY_HEAD: f32 = 30.0;
const MOBILE_DAY_HEAD: f32 = 52.0;

/// A piece of the review's grid: a day's heading, or the cells of a run of candidates.
enum Block {
    Day { group: DayGroup, top: f32 },
    Cells { range: std::ops::Range<usize>, top: f32 },
}

/// The review's grid as headings and runs of cells (`cols` across, rows `row` apart), with the
/// height it all takes. Without capture dates there are no headings: one run, as before dates.
fn grid_blocks(d: &ImportDialog, cols: usize, head: f32, row: f32) -> (Vec<Block>, f32) {
    let groups = d.groups();
    let headings = groups.iter().any(|g| g.day.is_some());
    let mut out = Vec::with_capacity(groups.len() * 2);
    let mut y = 0.0;
    for g in groups {
        let rows = g.range.len().div_ceil(cols.max(1));
        let range = g.range.clone();
        if headings {
            out.push(Block::Day { group: g, top: y });
            y += head;
        }
        out.push(Block::Cells { range, top: y });
        y += rows as f32 * row;
    }
    (out, y)
}

/// A checkbox that can be partly checked (some of a day's photos): painted at `r`.
fn paint_box(p: &egui::Painter, r: Rect, state: Option<bool>, enabled: bool, t: &Tokens) {
    let filled = state != Some(false);
    let fill = if filled && enabled { t.accent } else { Color32::TRANSPARENT };
    let edge = if enabled { if filled { t.accent } else { t.text_dim } } else { t.text_dim.gamma_multiply(0.5) };
    p.rect(r, 3.0, fill, Stroke::new(1.5, edge), StrokeKind::Inside);
    let ink = Stroke::new(2.0, Color32::WHITE);
    match state {
        Some(true) if enabled => {
            let (a, b, c) = (
                r.left_center() + vec2(r.width() * 0.22, 0.0),
                r.center_bottom() + vec2(-r.width() * 0.08, -r.height() * 0.25),
                r.right_top() + vec2(-r.width() * 0.22, r.height() * 0.28),
            );
            p.line_segment([a, b], ink);
            p.line_segment([b, c], ink);
        }
        None if enabled => {
            p.line_segment([r.left_center() + vec2(r.width() * 0.25, 0.0), r.right_center() - vec2(r.width() * 0.25, 0.0)], ink);
        }
        _ => {}
    }
}

/// A day's heading: a box that checks (or unchecks) its photos, its date, and how many there are.
fn day_row(ui: &mut egui::Ui, d: &mut ImportDialog, g: &DayGroup, rect: Rect, phone: bool) {
    let t = Tokens::get(ui.ctx());
    let key = g.day.clone().unwrap_or_default();
    let resp = ui.interact(rect, egui::Id::new(("import-day", &key)), Sense::click());
    register(ui.ctx(), format!("importDay:{}", if key.is_empty() { "none" } else { key.as_str() }), rect);
    let state = d.range_checked(g.range.clone());
    let enabled = d.range_enabled(g.range.clone());
    let size = if phone { 24.0 } else { 16.0 };
    // (in line with the device's box and name above)
    let b = Rect::from_center_size(pos2(rect.left() + 2.0 + size / 2.0, rect.center().y), vec2(size, size));
    let p = ui.painter();
    paint_box(p, b, state, enabled, &t);
    let label = match &g.day {
        Some(day) => crate::i18n::date_group_label(day, false),
        None => crate::i18n::tr("No Date").to_string(),
    };
    let font = if phone { t.font(19.0) } else { t.semibold(13.0) };
    p.text(pos2(b.right() + 10.0, rect.center().y), Align2::LEFT_CENTER, label, font.clone(), t.text);
    p.text(
        rect.right_center() - vec2(if phone { 0.0 } else { 6.0 }, 0.0),
        Align2::RIGHT_CENTER,
        crate::i18n::grouped(g.range.len()),
        font,
        t.text_dim,
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, state == Some(true), "day"));
    if resp.clicked() && enabled {
        d.toggle_range(g.range.clone());
    }
}

/// The camera or card the photos are on: a box that checks all of them, its name, how much it
/// holds ("17,966.3 MB – 1,719"), and while a camera is still cataloguing, that it is.
fn device_row(ui: &mut egui::Ui, d: &mut ImportDialog, phone: bool) {
    if d.device.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let n = d.candidates.len();
    let bytes: u64 = d.candidates.iter().map(|c| c.file_size).sum();
    let mb = bytes as f64 / 1e6;
    let size =
        format!("{}.{} MB – {}", crate::i18n::grouped(mb.trunc() as usize), ((mb.fract() * 10.0).floor() as u32).min(9), crate::i18n::grouped(n));
    let h = if phone { 52.0 } else { 30.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    register(ui.ctx(), "importDevice", rect);
    let state = d.range_checked(0..n);
    let enabled = d.range_enabled(0..n);
    let bs = if phone { 24.0 } else { 16.0 };
    let b = Rect::from_center_size(pos2(rect.left() + 2.0 + bs / 2.0, rect.center().y), vec2(bs, bs));
    let p = ui.painter();
    paint_box(p, b, state, enabled, &t);
    let font = if phone { t.font(19.0) } else { t.semibold(14.0) };
    let size_w = p.layout_no_wrap(size.clone(), font.clone(), t.text_dim).size().x;
    let name_w = (rect.right() - size_w - 16.0 - (b.right() + 12.0)).max(40.0);
    let name = p.layout(d.device.clone(), font.clone(), t.text, name_w);
    // (one line: a long name is cut, the size keeps its room)
    let clip = Rect::from_min_max(pos2(b.right() + 10.0, rect.top()), pos2(b.right() + 10.0 + name_w, rect.bottom()));
    ui.painter().with_clip_rect(clip).galley(pos2(clip.left(), rect.center().y - name.rows.first().map_or(8.0, |r| r.height()) / 2.0), name, t.text);
    ui.painter().text(rect.right_center(), Align2::RIGHT_CENTER, size, font, t.text_dim);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, state == Some(true), d.device.clone()));
    if resp.clicked() && enabled {
        d.toggle_range(0..n);
    }
    if let Some(cam) = &d.camera {
        let status = match (&cam.error, cam.complete) {
            (Some(e), _) => Some((e.clone(), t.caution)),
            (None, false) => {
                Some((crate::i18n::tr_format!("Reading {name}… {n} photos so far", name = cam.name, n = crate::i18n::grouped(n)), t.text_dim))
            }
            (None, true) if n == 0 => Some((crate::i18n::tr("No photos on it").to_string(), t.text_dim)),
            _ => None,
        };
        if let Some((text, color)) = status {
            let r = ui.label(egui::RichText::new(text).color(color).size(if phone { 15.0 } else { 12.0 }));
            register(ui.ctx(), "label:importCameraStatus", r.rect);
            if !cam.complete {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
    }
}

/// The texture of candidate `i`'s thumbnail: a camera's own, or one rendered from the file (a
/// background job; the grid only asks for the cells in view).
fn cell_texture(
    app: &mut LightcraftApp,
    ctx: &egui::Context,
    d: &ImportDialog,
    i: usize,
    c: &ImportCandidate,
    edge: usize,
) -> Option<(egui::TextureId, [usize; 2])> {
    if let Some(cam) = &d.camera {
        return crate::camera_import::thumbnail(app, ctx, &cam.id, &c.path);
    }
    let slot = Slot::Import(i as u32);
    if !app.renderer.textures.contains_key(&slot)
        && let Some(job) = app.session.candidate_thumb_job(c, edge, i as u64)
    {
        app.renderer.request_quick(slot, job, 4);
    }
    app.renderer.textures.get(&slot).map(|t| (t.tex.id(), t.size))
}

/// The tag a candidate carries: in the library already, a duplicate, unreadable.
fn candidate_tag(app: &LightcraftApp, d: &ImportDialog, i: usize, c: &ImportCandidate) -> Option<&'static str> {
    let in_trash = c.existing.is_some_and(|id| app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).is_some_and(|p| p.deleted));
    let on = d.checked.get(i).copied().unwrap_or(false) && d.importable(i);
    match (&c.duplicate, &c.error) {
        // checked with "Leave them", the file goes without `onDeleted`: the engine restores it
        (Some(_), _) if in_trash && on => Some(if d.on_deleted == "fresh" { "Will be replaced" } else { "Will be restored" }),
        (Some(_), _) if in_trash => Some("In Recently Deleted"),
        (Some(r), _) if r == "path" || r == "library" => Some("In library"),
        (Some(_), _) => Some("Duplicate"),
        (None, Some(_)) => Some("Unreadable"),
        _ => None,
    }
}

/// The "Raw" tag a raw file's thumbnail carries, its top right corner at `corner`.
fn raw_tag(p: &egui::Painter, corner: egui::Pos2, t: &Tokens) {
    let g = p.layout_no_wrap(crate::i18n::tr("Raw").to_string(), t.semibold(10.5), Color32::BLACK);
    let r = Rect::from_min_size(corner - vec2(g.size().x + 8.0, 0.0), g.size() + vec2(8.0, 4.0));
    p.rect(r, 2.0, Color32::from_white_alpha(235), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
    p.galley(r.min + vec2(4.0, 2.0), g, Color32::BLACK);
}

/// Skipped candidates: (already in the library, identical copies within the source, in Recently
/// Deleted). Files of photos in Recently Deleted are not "already in the library", and a file with
/// the same bytes as one earlier in the list (no library photo) is a copy in the source.
fn dup_counts(d: &ImportDialog) -> (usize, usize, usize) {
    let trashed = d.trashed.iter().filter(|t| **t).count();
    let in_source = d.candidates.iter().filter(|c| c.duplicate.as_deref() == Some("content") && c.existing.is_none()).count();
    let dups = d.candidates.iter().filter(|c| c.duplicate.is_some()).count().saturating_sub(trashed).saturating_sub(in_source);
    (dups, in_source, trashed)
}

/// The dialog body: options, then the candidate grid.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &mut ImportDialog) {
    crate::camera_import::refresh(app, d);
    let t = Tokens::get(ui.ctx());
    let n = d.candidates.len();
    let (dups, in_source, trashed) = dup_counts(d);
    let sel = d.selected_paths().len();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr_format!("{n} found · {sel} selected", n = n, sel = sel)).color(t.text));
        if dups > 0 {
            ui.label(egui::RichText::new(crate::i18n::tr_format!("· {dups} already in the library", dups = dups)).color(t.text_dim));
        }
        if in_source > 0 {
            ui.label(egui::RichText::new(crate::i18n::tr_format!("· {in_source} identical copies", in_source = in_source)).color(t.text_dim))
                .on_hover_text(crate::i18n::tr("Files with the same content as another file in this import: only the first is imported."));
        }
        if trashed > 0 {
            ui.label(egui::RichText::new(crate::i18n::tr_format!("· {trashed} in Recently Deleted", trashed = trashed)).color(t.text_dim));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::text_button(ui, "importNone", crate::i18n::tr("Uncheck All"), false).clicked() {
                d.checked.iter_mut().for_each(|c| *c = false);
            }
            if crate::widgets::text_button(ui, "importAll", crate::i18n::tr("Check All"), false).clicked() {
                for i in 0..n {
                    d.checked[i] = d.checked_by_default(i);
                }
            }
        });
    });
    // with more than one file type: a toggle per type checks or unchecks all of its files
    let types = d.file_types();
    if types.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("File types:")).color(t.text_dim));
            for (ty, count) in &types {
                let on = d.type_checked(ty);
                let r = crate::widgets::text_button(ui, &format!("importType:{ty}"), &format!("{ty} · {count}"), on)
                    .on_hover_text(crate::i18n::tr(if on { "Uncheck the files of this type" } else { "Check the files of this type" }));
                if r.clicked() {
                    d.check_type(ty, !on);
                }
            }
        });
    }
    device_row(ui, d, false);
    // candidate grid, under a heading per day (with a box that checks the day's photos)
    let cell = 116.0;
    let avail = ui.available_width();
    let cols = ((avail + 6.0) / (cell + 6.0)).floor().max(1.0) as usize;
    let (blocks, total_h) = grid_blocks(d, cols, DAY_HEAD, cell + 26.0);
    // as tall as the window leaves after the options and buttons below it (measured last frame),
    // so resizing the dialog resizes the grid
    let below = ui.ctx().data(|m| {
        let y = |key: &str| m.get_temp::<f32>(egui::Id::new(key));
        y(CONTENT_BOTTOM).zip(y(GRID_BOTTOM)).map(|(content, grid)| content - grid)
    });
    let grid_h = grid_height(ui.available_height(), below.unwrap_or(DEFAULT_BELOW_GRID), cell + 26.0);
    let grid =
        egui::ScrollArea::vertical().id_salt("import-grid").max_height(grid_h).auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
            let (area, _) = ui.allocate_exact_size(vec2(avail, total_h), Sense::hover());
            for b in &blocks {
                match b {
                    Block::Day { group, top } => {
                        let local = Rect::from_min_size(pos2(0.0, *top), vec2(avail, DAY_HEAD));
                        if local.intersects(viewport) {
                            day_row(ui, d, group, local.translate(area.min.to_vec2()), false);
                        }
                    }
                    Block::Cells { range, top } => {
                        for i in range.clone() {
                            let k = i - range.start;
                            let (c, r) = (k % cols, k / cols);
                            let local = Rect::from_min_size(pos2(c as f32 * (cell + 6.0), top + r as f32 * (cell + 26.0)), vec2(cell, cell + 20.0));
                            if !local.intersects(viewport.expand(cell)) {
                                continue;
                            }
                            let rect = local.translate(area.min.to_vec2());
                            candidate_cell(app, ui, d, i, rect);
                        }
                    }
                }
            }
        });
    if !ui.is_sizing_pass() {
        ui.ctx().data_mut(|m| m.insert_temp(egui::Id::new(GRID_BOTTOM), grid.inner_rect.bottom()));
    }
    ui.add_space(4.0);
    options(app, ui, d);
}

/// The import review on a phone, as Lightroom's mobile app has it: the photos in a grid as wide
/// as the screen, three across, each with a check badge a tap toggles; the options under a
/// disclosure below. The page's bar has Cancel and Add (`dialogs::ok_label`).
pub fn mobile_body(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &mut ImportDialog) {
    crate::camera_import::refresh(app, d);
    let t = Tokens::get(ui.ctx());
    let n = d.candidates.len();
    let dups = d.candidates.iter().filter(|c| c.duplicate.is_some()).count();
    let sel = d.selected_paths().len();
    ui.horizontal(|ui| {
        let r = ui.label(egui::RichText::new(crate::i18n::tr_format!("{n} found · {sel} selected", n = n, sel = sel)).color(t.text_dim));
        register(ui.ctx(), "label:importCount", r.rect);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let all = (0..n).filter(|i| d.importable(*i)).all(|i| d.checked.get(i).copied().unwrap_or(false));
            let (id, label) = if all && sel > 0 { ("importNone", "Deselect All") } else { ("importAll", "Select All") };
            let galley = ui.painter().layout_no_wrap(crate::i18n::tr(label).to_string(), t.font(16.0), t.accent);
            let (r, resp) = ui.allocate_exact_size(galley.size() + vec2(8.0, 20.0), Sense::click());
            register(ui.ctx(), format!("button:{id}"), r);
            ui.painter().galley(
                r.center() - galley.size() / 2.0,
                galley,
                if resp.is_pointer_button_down_on() { t.accent.gamma_multiply(0.6) } else { t.accent },
            );
            if resp.clicked() {
                for i in 0..n {
                    let on = !all && d.importable(i);
                    if let Some(c) = d.checked.get_mut(i) {
                        *c = on;
                    }
                }
            }
        });
    });
    if dups > 0 {
        ui.label(egui::RichText::new(crate::i18n::tr_format!("· {dups} already in the library", dups = dups)).color(t.text_dim).size(13.0));
    }
    device_row(ui, d, true);
    // the grid runs to the page's edges, under a heading per day (as Lightroom's mobile app has it)
    let gap = 2.0;
    let margin = 16.0;
    let w = ui.available_width() + 2.0 * margin;
    let cell = ((w - 2.0 * gap) / 3.0).floor();
    let (blocks, total_h) = grid_blocks(d, 3, MOBILE_DAY_HEAD, cell + gap);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
    let left = area.left() - margin;
    let clip = ui.clip_rect();
    for b in &blocks {
        match b {
            Block::Day { group, top } => {
                let rect = Rect::from_min_size(pos2(area.left(), area.top() + top), vec2(area.width(), MOBILE_DAY_HEAD));
                if rect.intersects(clip) {
                    day_row(ui, d, group, rect, true);
                }
            }
            Block::Cells { range, top } => {
                for i in range.clone() {
                    let k = i - range.start;
                    let (c, r) = (k % 3, k / 3);
                    let rect =
                        Rect::from_min_size(pos2(left + c as f32 * (cell + gap), area.top() + top + r as f32 * (cell + gap)), vec2(cell, cell));
                    if rect.intersects(clip) {
                        mobile_cell(app, ui, d, i, rect);
                    }
                }
            }
        }
    }
    ui.add_space(8.0);
    let r = egui::CollapsingHeader::new(egui::RichText::new(crate::i18n::tr("Options")).size(17.0))
        .id_salt("import-options")
        .show(ui, |ui| options(app, ui, d));
    register(ui.ctx(), "button:importOptions", r.header_response.rect);
}

/// One photo of the phone's import review: the thumbnail filling a square, a round check badge
/// (filled blue when it will be added), and a tag for duplicates and unreadable files.
fn mobile_cell(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &mut ImportDialog, i: usize, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let Some(c) = d.candidates.get(i).cloned() else { return };
    let ok = d.importable(i);
    let resp = ui.interact(rect, egui::Id::new(("import-cell", i)), Sense::click());
    register(ui.ctx(), format!("import:{i}"), rect);
    let p = ui.painter_at(rect.intersect(ui.clip_rect()));
    p.rect_filled(rect, 0.0, t.cell);
    if let Some((tex, [tw, th])) = cell_texture(app, ui.ctx(), d, i, &c, 256) {
        // fill the square, cropping the long side (as the grid's square thumbnails do)
        let (tw, th) = (tw.max(1) as f32, th.max(1) as f32);
        let uv = if tw > th {
            let k = th / tw;
            Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
        } else {
            let k = tw / th;
            Rect::from_min_max(pos2(0.0, (1.0 - k) / 2.0), pos2(1.0, (1.0 + k) / 2.0))
        };
        p.image(tex, rect, uv, if ok { Color32::WHITE } else { Color32::from_gray(90) });
    }
    let on = d.checked.get(i).copied().unwrap_or(false) && ok;
    if !on {
        p.rect_filled(rect, 0.0, Color32::from_black_alpha(70));
    }
    if ok {
        let c0 = rect.right_bottom() - vec2(16.0, 16.0);
        if on {
            p.circle(c0, 11.0, t.accent, Stroke::new(1.5, Color32::WHITE));
            crate::icons::paint(&p, Rect::from_center_size(c0, vec2(13.0, 13.0)), crate::icons::Icon::Check, Color32::WHITE);
        } else {
            p.circle(c0, 11.0, Color32::from_black_alpha(60), Stroke::new(1.5, Color32::WHITE));
        }
    }
    if let Some(b) = candidate_tag(app, d, i, &c) {
        let g = p.layout_no_wrap(crate::i18n::tr(b).to_string(), t.semibold(11.0), Color32::WHITE);
        let br = Rect::from_min_size(rect.min + vec2(6.0, 6.0), g.size() + vec2(10.0, 6.0));
        p.rect_filled(br, 5.0, Color32::from_black_alpha(170));
        p.galley(br.min + vec2(5.0, 3.0), g, Color32::WHITE);
    }
    if is_raw(&c) {
        raw_tag(&p, rect.right_top() + vec2(-6.0, 6.0), &t);
    }
    if resp.clicked()
        && ok
        && let Some(c) = d.checked.get_mut(i)
    {
        *c = !*c;
    }
}

/// The review's options (where photos go, folders, renaming, album, preset, keywords).
fn options(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &mut ImportDialog) {
    let t = Tokens::get(ui.ctx());
    // (staged: the host's temporary folder means nothing to the user)
    if !d.sources.is_empty() && !d.staged {
        field(ui, "Source", |ui| {
            // (whether each source is a folder is checked off the UI thread, once)
            let joined = d.sources.join("\n");
            let summary = crate::panels::left::fs_cached(ui, "import-source", &joined, f64::INFINITY, |s| {
                inspect_sources(&s.split('\n').map(str::to_string).collect::<Vec<_>>())
            })
            .map(|summary| summary.display())
            .unwrap_or_else(|| crate::i18n::tr_format!("{} item{}", d.sources.len(), if d.sources.len() == 1 { "" } else { "s" }));
            let r = ui.label(egui::RichText::new(summary).color(t.text_label)).on_hover_text(joined);
            register(ui.ctx(), "label:importSource", r.rect);
        });
        ui.label(
            egui::RichText::new(crate::i18n::tr("Importing scans the source for photos; it doesn't add the folder to Local (use Local → Browse Folder… to work in a folder without importing)."))
                .color(t.text_dim)
                .small(),
        );
    }
    if d.staged {
        // the host's temporary copies: into the library, nothing to choose
        let help = if d.camera.is_some() {
            "The photos are copied into the library; the originals on the camera or card are left as they are."
        } else {
            "The photos are copied into the library; the originals in Photos or Files are left as they are."
        };
        let r = ui.label(egui::RichText::new(crate::i18n::tr(help)).color(t.text_dim).small());
        register(ui.ctx(), "label:importModeHelp", r.rect);
    } else {
        transfer_choice(app, ui, d);
    }
    if d.copy {
        // (the host's own copies are moved, but to the user the photos are copied: the originals stay)
        field(ui, if d.move_files && !d.staged { "Move to" } else { "Copy to" }, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let shown = if d.destination.trim().is_empty() { crate::i18n::tr("Library Originals").to_string() } else { d.destination.clone() };
            ui.label(egui::RichText::new(shown).color(Tokens::get(ui.ctx()).text_label));
            if app.services.pick_folder.is_some()
                && crate::widgets::text_button(ui, "importDest", crate::i18n::tr("Choose…"), false).clicked()
                && let Some(f) = app.services.pick_folder.as_mut().and_then(|f| f())
            {
                d.destination = f;
            }
            if !d.destination.is_empty() && crate::widgets::text_button(ui, "importDestReset", crate::i18n::tr("Library"), false).clicked() {
                d.destination.clear();
            }
        });
        field(ui, "Folders", |ui| {
            let opts = [
                ("date", "By day (YYYY/YYYY-MM-DD)"),
                ("month", "By month (YYYY/YYYY-MM)"),
                ("flat", "Into one folder"),
                ("custom", "Custom template…"),
            ];
            let key = if d.organize.is_empty() { "date".to_string() } else { d.organize.clone() };
            let cur = opts.iter().find(|o| o.0 == key).map_or(opts[0].1, |o| o.1);
            let combo = crate::widgets::combo_box("import-organize").selected_text(crate::i18n::tr(cur)).show_ui(ui, |ui| {
                for (k, label) in opts {
                    let r = ui.selectable_label(key == k, crate::i18n::tr(label));
                    register(ui.ctx(), format!("button:importOrganize-{k}"), r.rect);
                    if r.clicked() {
                        d.organize = k.to_string();
                        if k == "custom" && d.folder_template.trim().is_empty() {
                            d.folder_template = DEFAULT_FOLDER_TEMPLATE.into();
                        }
                    }
                }
            });
            register(ui.ctx(), "combo:importOrganize", combo.response.rect);
        });
        if d.organize == "custom" {
            let folders_id = egui::Id::new("import-folder-template");
            let tags_open = field(ui, "Template", |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = tag_field_width(ui);
                let r = ui.add(crate::widgets::text_box(
                    ui,
                    egui::TextEdit::singleline(&mut d.folder_template).id(folders_id).hint_text(DEFAULT_FOLDER_TEMPLATE).desired_width(w),
                ));
                register(ui.ctx(), "field:importFolderTemplate", r.rect);
                tag_toggle(ui, "importFolders")
            });
            if tags_open {
                tag_help(ui, "importFolders", &mut d.folder_template, folders_id);
            }
            let t = Tokens::get(ui.ctx());
            match lightcraft_engine::rename::folder_template_error(&d.folder_template) {
                Some(e) => {
                    ui.label(egui::RichText::new(e).color(t.caution));
                }
                None => unknown_tags_warning(ui, &d.folder_template),
            }
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    "Each / starts a folder level; tags are filled in per photo (a level with missing metadata is \"unknown\").",
                ))
                .color(t.text_dim)
                .small(),
            );
        }
        if !d.move_files {
            field(ui, "Raw files", |ui| {
                let r = crate::widgets::check(ui, &mut d.dng, crate::i18n::tr("Copy as DNG"));
                register(ui.ctx(), "check:importDng", r.rect);
            });
        }
        let rename_id = egui::Id::new("import-rename");
        let tags_open = field(ui, "Rename", |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = tag_field_width(ui);
            let r = ui.add(crate::widgets::text_box(
                ui,
                egui::TextEdit::singleline(&mut d.rename)
                    .id(rename_id)
                    .hint_text(crate::i18n::tr("keep names — or e.g. {date}_{seq:3}"))
                    .desired_width(w),
            ));
            register(ui.ctx(), "field:importRename", r.rect);
            tag_toggle(ui, "importRename")
        });
        if tags_open {
            tag_help(ui, "importRename", &mut d.rename, rename_id);
        }
        unknown_tags_warning(ui, &d.rename);
        if let Some(example) = example_destination(app, d) {
            let t = Tokens::get(ui.ctx());
            let r = ui.label(egui::RichText::new(crate::i18n::tr_format!("Example: {example}", example = example)).color(t.text_dim));
            register(ui.ctx(), "label:importExample", r.rect);
            ui.label(
                egui::RichText::new(crate::i18n::tr("Folders and {date} use the capture time; a photo without one uses today's date."))
                    .color(t.text_dim)
                    .small(),
            );
        }
    }
    if d.trashed.iter().any(|t| *t) {
        field(ui, "In Recently Deleted", |ui| {
            let cur = match d.on_deleted.as_str() {
                "restore" => "Restore them",
                "fresh" => "Import as new",
                _ => "Leave them",
            };
            let mut pick = None;
            crate::widgets::combo_box("import-on-deleted").selected_text(crate::i18n::tr(cur)).show_ui(ui, |ui| {
                for (v, label) in [("restore", "Restore them"), ("fresh", "Import as new"), ("", "Leave them")] {
                    if ui.selectable_label(d.on_deleted == v, crate::i18n::tr(label)).clicked() {
                        pick = Some(v);
                    }
                }
            });
            if let Some(v) = pick {
                d.set_on_deleted(v);
            }
            let t = Tokens::get(ui.ctx());
            ui.label(
                egui::RichText::new(crate::i18n::tr("Restore keeps their edits; Import as new deletes the old record first."))
                    .color(t.text_dim)
                    .small(),
            );
        });
    }
    let mut albums = crate::album_picker::entries_from(&app.session.catalog, |_| None);
    albums.retain(|a| a.folder || !a.smart);
    // Catalog changes while the review is open must not leave a hidden, unselectable destination.
    d.album = d.album.filter(|id| albums.iter().any(|a| a.id == *id && !a.folder));
    field(ui, "Album", |ui| {
        ui.vertical(|ui| {
            let mut chosen = d.album;
            let mut extra = None;
            let empty = crate::i18n::tr(if d.new_album.is_empty() { "None" } else { "New album" });
            let response = crate::album_picker::AlbumPicker::new("import-album", &mut chosen, &albums)
                .width(ui.available_width())
                .empty_text(empty)
                .show_with_choices(ui, |ui| {
                    let none = ui.selectable_label(d.album.is_none() && d.new_album.is_empty(), crate::i18n::tr("None"));
                    register(ui.ctx(), "importAlbum:none", none.rect);
                    if none.clicked() {
                        extra = Some(false);
                    }
                    let new = ui.selectable_label(d.album.is_none() && !d.new_album.is_empty(), crate::i18n::tr("New album…"));
                    register(ui.ctx(), "importAlbum:new", new.rect);
                    if new.clicked() {
                        extra = Some(true);
                    }
                    ui.separator();
                    extra.is_some()
                });
            if response.changed() {
                d.album = chosen;
                d.new_album.clear();
            }
            if let Some(new) = extra {
                d.album = None;
                if !new {
                    d.new_album.clear();
                } else if d.new_album.is_empty() {
                    d.new_album = crate::i18n::tr("Imported Photos").into();
                }
            }
            if d.album.is_none() && !d.new_album.is_empty() {
                let r = ui.add(crate::widgets::text_box(ui, egui::TextEdit::singleline(&mut d.new_album).desired_width(f32::INFINITY)));
                register(ui.ctx(), "field:importAlbumName", r.rect);
            }
        });
    });
    field(ui, "Preset", |ui| {
        let cur = app
            .session
            .presets
            .iter()
            .find(|p| p.id == d.preset)
            .map(|p| crate::i18n::builtin_label(&p.name, p.builtin).to_string())
            .unwrap_or_else(|| crate::i18n::tr("None").into());
        crate::widgets::combo_box("import-preset").selected_text(cur).height(300.0).show_ui(ui, |ui| {
            if ui.selectable_label(d.preset.is_empty(), crate::i18n::tr("None")).clicked() {
                d.preset.clear();
            }
            let selected = (!d.preset.is_empty()).then_some(d.preset.as_str());
            if let Some(pid) = crate::panels::presets::picker_rows(ui, "import-preset", &app.session.presets, selected) {
                d.preset = pid;
            }
        });
    });
    if !app.session.metadata_presets.is_empty() {
        field(ui, "Metadata", |ui| {
            let cur = if d.metadata_preset.is_empty() { crate::i18n::tr("None").to_string() } else { d.metadata_preset.clone() };
            crate::widgets::combo_box("import-metadata").selected_text(cur).show_ui(ui, |ui| {
                if ui.selectable_label(d.metadata_preset.is_empty(), crate::i18n::tr("None")).clicked() {
                    d.metadata_preset.clear();
                }
                for m in &app.session.metadata_presets {
                    if ui.selectable_label(d.metadata_preset == m.name, &m.name).clicked() {
                        d.metadata_preset = m.name.clone();
                    }
                }
            });
        });
    }
    field(ui, "Keywords", |ui| {
        let r = ui.add(crate::widgets::text_box(
            ui,
            egui::TextEdit::singleline(&mut d.keywords).hint_text(crate::i18n::tr("comma, separated")).desired_width(f32::INFINITY),
        ));
        register(ui.ctx(), "field:importKeywords", r.rect);
    });
}

/// Add in place / Copy / Move, and what the choice means.
fn transfer_choice(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &mut ImportDialog) {
    let t = Tokens::get(ui.ctx());
    field(ui, "Transfer", |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if crate::widgets::text_button(ui, "importAdd", crate::i18n::tr("Add in place"), !d.copy)
            .on_hover_text(crate::i18n::tr("Reference the files where they are"))
            .clicked()
        {
            d.copy = false;
            d.move_files = false;
        }
        let can_copy = app.session.library.as_ref().is_some_and(|l| l.on_disk) || app.services.pick_folder.is_some();
        let r =
            ui.add_enabled_ui(can_copy, |ui| crate::widgets::text_button(ui, "importCopy", crate::i18n::tr("Copy"), d.copy && !d.move_files)).inner;
        if r.on_hover_text(crate::i18n::tr("Copy the files (into the library's Originals/, or a folder you choose)")).clicked() {
            d.copy = true;
            d.move_files = false;
        }
        let r =
            ui.add_enabled_ui(can_copy, |ui| crate::widgets::text_button(ui, "importMove", crate::i18n::tr("Move"), d.copy && d.move_files)).inner;
        if r.on_hover_text(crate::i18n::tr("Move the files (into the library's Originals/, or a folder you choose), removing them from the source"))
            .clicked()
        {
            d.copy = true;
            d.move_files = true;
        }
    });
    let r = ui.label(
        egui::RichText::new(crate::i18n::tr(match (d.copy, d.move_files) {
            (true, false) => {
                "Copy: the files are copied to the folder below and the library uses the copies; the originals are left as they are."
            }
            (true, true) => {
                "Move: the files are moved to the folder below; each original (and its XMP sidecar) is removed from the source only after its copy is verified. Duplicates and files that fail stay where they are."
            }
            _ => "Add in place: the library references the files where they are; nothing is copied or moved.",
        }))
        .color(if d.copy && d.move_files { t.caution } else { t.text_dim })
        .small(),
    );
    register(ui.ctx(), "label:importModeHelp", r.rect);
}

fn candidate_cell(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &mut ImportDialog, i: usize, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let c = d.candidates[i].clone();
    let ok = d.importable(i);
    let img = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width()));
    let resp = ui.interact(img, egui::Id::new(("import-cell", i)), Sense::click());
    crate::widgets::announce(&resp, c.name.clone());
    register(ui.ctx(), format!("import:{i}"), img);
    let p = ui.painter();
    p.rect_filled(img, 3.0, t.skeleton);
    if let Some((tex, [tw, th])) = cell_texture(app, ui.ctx(), d, i, &c, 192) {
        let s = ((img.width() - 8.0) / tw.max(1) as f32).min((img.height() - 8.0) / th.max(1) as f32);
        let fit = Rect::from_center_size(img.center(), vec2(tw as f32 * s, th as f32 * s));
        let tint = if ok { Color32::WHITE } else { Color32::from_gray(110) };
        ui.painter().image(tex, fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
    }
    let p = ui.painter();
    let on = d.checked[i] && ok;
    if on {
        p.rect_stroke(img, 3.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    }
    // checkbox
    let cb = Rect::from_min_size(img.min + vec2(6.0, 6.0), vec2(16.0, 16.0));
    p.rect(cb, 3.0, if on { t.accent } else { Color32::from_black_alpha(150) }, Stroke::new(1.0, Color32::from_gray(200)), StrokeKind::Inside);
    if on {
        p.line_segment([cb.left_center() + vec2(3.5, 0.5), cb.center_bottom() + vec2(-1.0, -4.0)], Stroke::new(2.0, Color32::WHITE));
        p.line_segment([cb.center_bottom() + vec2(-1.0, -4.0), cb.right_top() + vec2(-3.5, 4.0)], Stroke::new(2.0, Color32::WHITE));
    }
    if is_raw(&c) {
        raw_tag(p, img.right_bottom() + vec2(-6.0, -6.0 - 15.0), &t);
    }
    if let Some(b) = candidate_tag(app, d, i, &c) {
        let g = p.layout_no_wrap(crate::i18n::tr(b).to_string(), t.semibold(10.0), Color32::WHITE);
        let br = badge_rect(img, cb, g.size());
        p.rect_filled(br, 3.0, Color32::from_rgba_unmultiplied(170, 60, 50, 220));
        p.galley(br.min + vec2(4.0, 2.0), g, Color32::WHITE);
    }
    let name = if c.name.chars().count() > 18 { format!("{}…", c.name.chars().take(17).collect::<String>()) } else { c.name.clone() };
    p.text(pos2(rect.left() + 2.0, img.bottom() + 9.0), Align2::LEFT_CENTER, name, t.font(10.5), if ok { t.text_label } else { t.text_dim });
    let tip = format!(
        "{}\n{} × {} · {} · {:.1} MB{}",
        if d.camera.is_some() { &c.name } else { &c.path },
        c.width,
        c.height,
        c.format,
        c.file_size as f64 / 1e6,
        c.captured.as_deref().map(|d| format!("\n{}", d.replace('T', " "))).unwrap_or_default()
    );
    let resp = resp.on_hover_text(tip);
    if resp.clicked() && ok {
        let shift = ui.input(|input| input.modifiers.shift);
        d.click(i, shift);
    }
}

/// Where a cell's badge goes: top right, or under the checkbox when the text is too wide to sit
/// beside it — drawn over the checkbox it hid it (issue #1285).
fn badge_rect(img: Rect, cb: Rect, text: Vec2) -> Rect {
    let size = text + vec2(8.0, 4.0);
    let left = img.right() - size.x - 4.0;
    let top = if left < cb.right() + 4.0 { cb.bottom() + 4.0 } else { img.top() + 6.0 };
    Rect::from_min_size(pos2(left, top), size)
}

/// Reserve the actual translated tag button width beside a template field.
pub(crate) fn tag_field_width(ui: &egui::Ui) -> f32 {
    let t = Tokens::get(ui.ctx());
    let galley = ui.painter().layout_no_wrap(crate::i18n::tr("Tags").to_string(), t.semibold(11.5), t.text);
    (ui.available_width() - (galley.size().x + 18.0).max(37.0) - ui.spacing().item_spacing.x).max(80.0)
}

/// The "Tags" toggle beside a template field (`button:<key>Tags`); returns whether the tag help
/// ([`tag_help`]) is open.
pub(crate) fn tag_toggle(ui: &mut egui::Ui, key: &str) -> bool {
    let id = egui::Id::new((key, "tags-open"));
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let r = crate::widgets::text_button(ui, &format!("{key}Tags"), crate::i18n::tr("Tags"), open)
        .on_hover_text(crate::i18n::tr("Show the template tags; click one to insert it"));
    if r.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    open
}

/// "Unknown tag {x} stays as typed" under a template field, when it has one.
pub(crate) fn unknown_tags_warning(ui: &mut egui::Ui, template: &str) {
    let unknown = lightcraft_engine::rename::unknown_tokens(template);
    if !unknown.is_empty() {
        let s = if unknown.len() == 1 { "" } else { "s" };
        let text = crate::i18n::tr_format!(
            "Unknown tag{s} {} stay{} as typed (see Tags)",
            unknown.join(" "),
            if unknown.len() == 1 { "s" } else { "" },
            s = s
        );
        ui.label(egui::RichText::new(text).color(Tokens::get(ui.ctx()).caution));
    }
}

/// Insert `tag` into `text` at the text cursor of the field `edit_id` (replacing its selection);
/// appended when the field never had a cursor. The cursor ends up after the tag.
pub(crate) fn insert_at_cursor(ctx: &egui::Context, edit_id: egui::Id, text: &mut String, tag: &str) {
    use egui::text::{CCursor, CCursorRange};
    let mut state = egui::text_edit::TextEditState::load(ctx, edit_id).unwrap_or_default();
    let n = text.chars().count();
    let (a, b) = state.cursor.char_range().map_or((n, n), |r| {
        let (x, y) = (r.primary.index.0.min(n), r.secondary.index.0.min(n));
        (x.min(y), x.max(y))
    });
    let byte = |c: usize| text.char_indices().nth(c).map_or(text.len(), |(i, _)| i);
    let (ba, bb) = (byte(a), byte(b));
    text.replace_range(ba..bb, tag);
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(a + tag.chars().count()))));
    state.store(ctx, edit_id);
    ctx.memory_mut(|m| m.request_focus(edit_id));
}

/// The template tag help: every tag (from the engine's one list, `rename::TOKENS`) with its
/// meaning and an example, the `{date:…}` directives and how templates behave. Clicking a tag
/// (`button:<key>Tag-<i>`) inserts it at the cursor of the field `edit_id`.
pub(crate) fn tag_help(ui: &mut egui::Ui, key: &str, text: &mut String, edit_id: egui::Id) {
    use lightcraft_engine::rename::{DATE_DIRECTIVES, TEMPLATE_NOTES, TOKENS, token_example};
    let t = Tokens::get(ui.ctx());
    egui::Frame::new().fill(t.inset).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr("Click a tag to insert it. Examples are for IMG_0042.CR3 taken 14 Jan 2026, 05:58:48."))
                .color(t.text_dim)
                .small(),
        );
        egui::ScrollArea::vertical().id_salt((key, "tags")).max_height(170.0).show(ui, |ui| {
            egui::Grid::new((key, "tag-grid")).num_columns(3).spacing([10.0, 2.0]).show(ui, |ui| {
                for (i, tok) in TOKENS.iter().enumerate() {
                    let r = ui.add(egui::Button::new(egui::RichText::new(tok.tag).monospace().color(t.text)).small()).on_hover_text(
                        if tok.aliases.is_empty() {
                            crate::i18n::tr("Insert at the cursor").to_string()
                        } else {
                            crate::i18n::tr_format!("Insert at the cursor (also written {})", tok.aliases.join(", "))
                        },
                    );
                    register(ui.ctx(), format!("button:{key}Tag-{i}"), r.rect);
                    if r.clicked() {
                        insert_at_cursor(ui.ctx(), edit_id, text, tok.tag);
                    }
                    ui.label(egui::RichText::new(crate::i18n::tr(tok.meaning)).color(t.text_label));
                    ui.label(egui::RichText::new(token_example(tok.tag)).monospace().color(t.text_dim));
                    ui.end_row();
                }
            });
            let directives: Vec<String> = DATE_DIRECTIVES.iter().map(|(d, m)| format!("{d} {}", crate::i18n::tr(m))).collect();
            ui.label(egui::RichText::new(crate::i18n::tr_format!("{{date:…}} directives: {}", directives.join(" · "))).color(t.text_dim).small());
            for n in TEMPLATE_NOTES {
                ui.label(egui::RichText::new(format!("• {}", crate::i18n::tr(n))).color(t.text_dim).small());
            }
        });
    });
}

/// The folder template the Custom choice starts with (`2026/20260114/`).
pub const DEFAULT_FOLDER_TEMPLATE: &str = "{date:%Y}/{date:%Y%m%d}";

/// Where the first selected photo would be copied to (destination, folders, name), for the
/// dialog's example line; `None` without a photo or with an unusable folder template.
pub fn example_destination(app: &LightcraftApp, d: &ImportDialog) -> Option<String> {
    let c = d.candidates.iter().zip(&d.checked).enumerate().find(|(i, (_, on))| **on && d.importable(*i)).map(|(_, (c, _))| c)?;
    let organize = match d.organize_param().ok()? {
        Some(o) => lightcraft_engine::import::Organize::parse(&o)?,
        None => Default::default(),
    };
    let mut q = lightcraft_catalog::Photo::new(
        lightcraft_catalog::PhotoId(0),
        lightcraft_catalog::Source::File { path: c.path.clone() },
        &c.name,
        &c.format,
        0,
        0,
        // a photo without a capture time is dated by the import (now)
        &(app.session.clock)(),
    );
    q.captured = c.captured.clone();
    // the probe's metadata, so {camera}, {title}… preview as they will import
    if let Some(info) = app.session.import_probes.get(&c.path) {
        q.meta = info.meta.clone();
    }
    let name = if d.rename.trim().is_empty() { c.name.clone() } else { lightcraft_engine::rename::expand(d.rename.trim(), &q, 1) };
    let root = if d.staged {
        // the host's copies go into the library, whose folder in the app's sandbox means nothing to
        // the user: shown from Originals
        "Originals".to_string()
    } else if d.destination.trim().is_empty() {
        app.session.library.as_ref().map_or_else(|| "Originals".to_string(), |l| l.dir.join("Originals").to_string_lossy().to_string())
    } else {
        d.destination.trim().trim_end_matches(['/', '\\']).to_string()
    };
    let sep = std::path::MAIN_SEPARATOR_STR;
    let mut parts = vec![root];
    parts.extend(organize.folders(&q));
    parts.push(name);
    Some(parts.join(sep))
}

/// The import source in a few words: a folder's name, a file's name, or "N files and folders".
pub fn source_summary(sources: &[String]) -> String {
    inspect_sources(sources).display()
}

/// Facts cached by the filesystem worker, formatted in the UI thread's active language.
/// Caching a translated String on the worker would always use its default language (English)
/// and also retain that language after the user changes the UI language.
#[derive(Clone)]
enum SourceSummary {
    Folder(String),
    File(String),
    Many { files: usize, folders: usize },
}

impl SourceSummary {
    fn display(&self) -> String {
        match self {
            Self::Folder(name) => crate::i18n::tr_format!("Folder “{}” (and its subfolders)", name),
            Self::File(name) => name.clone(),
            Self::Many { files, folders } => match (files, folders) {
                (_, 0) => crate::i18n::tr_format!("{} files", files),
                (0, f) => crate::i18n::tr_format!("{f} folders (and their subfolders)", f = f),
                (_, f) => crate::i18n::tr_format!("{} files and {f} folder{}", files, if *f == 1 { "" } else { "s" }, f = f),
            },
        }
    }
}

fn inspect_sources(sources: &[String]) -> SourceSummary {
    let name = |p: &str| std::path::Path::new(p).file_name().map_or_else(|| p.to_string(), |n| n.to_string_lossy().to_string());
    match sources {
        [one] if std::path::Path::new(one).is_dir() => SourceSummary::Folder(name(one)),
        [one] => SourceSummary::File(name(one)),
        many => {
            let folders = many.iter().filter(|p| std::path::Path::new(p.as_str()).is_dir()).count();
            SourceSummary::Many { files: many.len().saturating_sub(folders), folders }
        }
    }
}

/// A labelled row (fixed label column).
fn field<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    if crate::is_compact(ui.ctx()) {
        // a phone (iOS forms): the label above its controls, which wrap to the page's width
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr(label)).color(t.text_dim).size(13.0));
        return ui.horizontal_wrapped(add).inner;
    }
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(78.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(78.0);
            ui.label(egui::RichText::new(crate::i18n::tr(label)).color(t.text_label));
        });
        add(ui)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_height_follows_the_window() {
        // the grid takes what the options and buttons leave, in whole points
        assert_eq!(grid_height(700.0, 300.0, 142.0), 400.0);
        assert_eq!(grid_height(1000.5, 300.0, 142.0), 700.0);
        // never less than one row of thumbnails
        assert_eq!(grid_height(200.0, 300.0, 142.0), 142.0);
        // a bad measurement doesn't take the grid away or make it endless
        assert_eq!(grid_height(700.0, -50.0, 142.0), 700.0);
        assert_eq!(grid_height(700.0, f32::NAN, 142.0), 700.0);
        assert_eq!(grid_height(f32::INFINITY, 300.0, 142.0), DIALOG_SIZE[1] - DEFAULT_BELOW_GRID);
    }

    fn dated(name: &str, captured: Option<&str>) -> ImportCandidate {
        ImportCandidate {
            path: format!("/card/{name}"),
            name: name.into(),
            format: "JPG".into(),
            captured: captured.map(str::to_string),
            ..Default::default()
        }
    }

    /// Candidates are listed by day, newest first, undated last; each day's box checks or unchecks
    /// the day's importable photos, and says when only some are checked.
    #[test]
    fn candidates_are_grouped_by_day() {
        let mut c = vec![
            dated("a.jpg", Some("2026-08-03T09:00:00")),
            dated("x.jpg", None),
            dated("b.jpg", Some("2026-10-03T10:00:00")),
            dated("c.jpg", Some("2026-08-03T09:05:00")),
            dated("d.jpg", Some("2026-10-03 11:00:00")),
        ];
        sort_by_day(&mut c);
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["b.jpg", "d.jpg", "a.jpg", "c.jpg", "x.jpg"], "newest day first, order kept within a day");
        c[3].duplicate = Some("library".into());
        let mut d = ImportDialog::new(c);
        let g = d.groups();
        assert_eq!(
            g.iter().map(|g| (g.day.clone(), g.range.clone())).collect::<Vec<_>>(),
            [(Some("2026-10-03".to_string()), 0..2), (Some("2026-08-03".to_string()), 2..4), (None, 4..5)]
        );
        assert_eq!(d.range_checked(2..4), Some(true), "the duplicate doesn't count");
        d.toggle_range(2..4);
        assert_eq!(d.checked, [true, true, false, false, true]);
        d.click(0, false);
        assert_eq!(d.range_checked(0..2), None, "some");
        d.toggle_range(0..2);
        assert_eq!(d.range_checked(0..2), Some(true), "some: the box checks them all");
        d.toggle_range(3..4);
        assert!(!d.checked[3], "a duplicate is never checked");
        assert!(is_raw(&ImportCandidate { path: "/card/x.CR3".into(), ..Default::default() }) && !is_raw(&d.candidates[0]));
    }

    /// A day's box checks as Check All does: with "Leave them", a file in Recently Deleted stays out
    /// unless it was clicked, and the box still reads as all checked (issue #1285).
    #[test]
    fn a_days_box_leaves_out_recently_deleted_files_as_check_all_does() {
        let mut d = ImportDialog::new(vec![dated("a.jpg", Some("2026-08-03T09:00:00")), dated("b.jpg", Some("2026-08-03T09:05:00"))]);
        d.candidates[1].duplicate = Some("content".into());
        d.set_trashed(vec![false, true]);
        d.set_on_deleted("");
        assert_eq!(d.checked, [true, false]);
        assert_eq!(d.range_checked(0..2), Some(true), "the left-out file doesn't count");
        d.toggle_range(0..2);
        assert_eq!(d.checked, [false, false]);
        d.toggle_range(0..2);
        assert_eq!(d.checked, [true, false], "the box doesn't pick it");
        d.click(1, false);
        assert_eq!(d.range_checked(0..2), Some(true), "picked by hand, it counts");
        d.toggle_range(0..2);
        assert_eq!(d.checked, [false, false], "and the box unchecks it");
        assert!(!d.range_enabled(1..2), "a box over it alone has nothing to check");
        d.click(1, false);
        assert!(d.range_enabled(1..2), "once it is clicked, the box can uncheck it");
    }

    /// A camera's files as they arrive: supported formats only, by day; the user's checks stay as
    /// more arrive; files the library has are not importable.
    #[test]
    fn camera_files_keep_their_checks_as_more_arrive() {
        let file = |name: &str, day: &str| lightcraft_engine::devices::CameraFile {
            id: format!("id-{name}"),
            name: name.into(),
            size: 10,
            captured: Some(format!("{day}T10:00:00")),
            ..Default::default()
        };
        let mut d = ImportDialog::new(Vec::new());
        d.set_camera_files(vec![file("A.JPG", "2026-08-03"), file("B.JPG", "2026-08-03")], |_| None);
        d.click(0, false);
        assert_eq!(d.selected_paths(), ["id-B.JPG"]);
        let all = vec![file("A.JPG", "2026-08-03"), file("B.JPG", "2026-08-03"), file("C.CR3", "2026-10-03"), file("MVI_0001.THM", "2026-10-03")];
        d.set_camera_files(all, |f| (f.name == "B.JPG").then_some(7));
        let names: Vec<&str> = d.candidates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["C.CR3", "A.JPG", "B.JPG"]);
        assert_eq!(d.selected_paths(), ["id-C.CR3"], "A stays unchecked, B is in the library, C arrives checked");
        assert_eq!(d.candidates[2].existing, Some(7));
        assert!(!d.importable(2));
    }

    /// Six candidates, all checked; candidate 3 is a duplicate (unchecked, not importable).
    fn dialog() -> ImportDialog {
        let mut candidates: Vec<ImportCandidate> = (0..6).map(|i| ImportCandidate { path: format!("img{i}.jpg"), ..Default::default() }).collect();
        candidates[3].duplicate = Some("content".into());
        ImportDialog::new(candidates)
    }

    /// Identical copies inside the source folder are not "already in the library" (a fresh library
    /// said "8 already in the library").
    #[test]
    fn copies_within_the_source_are_not_counted_as_in_the_library() {
        let mut d = dialog();
        assert_eq!(dup_counts(&d), (0, 1, 0), "same bytes as an earlier file, no library photo");
        d.candidates[4].duplicate = Some("content".into());
        d.candidates[4].existing = Some(7);
        d.candidates[5].duplicate = Some("path".into());
        d.candidates[5].existing = Some(8);
        assert_eq!(dup_counts(&d), (2, 1, 0));
    }

    /// A toggle per file type: the raws only, without the JPEGs beside them (issue #344).
    #[test]
    fn file_types_check_and_uncheck_together() {
        let mut candidates: Vec<ImportCandidate> =
            ["a.ARW", "a.JPG", "b.arw", "b.jpeg", "c.png"].iter().map(|p| ImportCandidate { path: p.to_string(), ..Default::default() }).collect();
        candidates[2].format = "ARW".into();
        candidates[3].format = "JPEG".into();
        candidates[2].duplicate = Some("content".into());
        let mut d = ImportDialog::new(candidates);
        assert_eq!(d.file_types(), [("ARW".to_string(), 2), ("JPG".to_string(), 2), ("PNG".to_string(), 1)]);
        assert!(d.type_checked("JPG"));
        d.check_type("JPG", false);
        assert_eq!(d.checked, [true, false, false, false, true]);
        assert!(!d.type_checked("JPG"));
        d.check_type("PNG", false);
        d.check_type("ARW", true);
        // the duplicate stays unchecked: it isn't importable
        assert_eq!(d.checked, [true, false, false, false, false]);
        d.check_type("JPG", true);
        assert_eq!(d.checked, [true, true, false, true, false]);
        assert_eq!(d.selected_paths(), ["a.ARW", "a.JPG", "b.jpeg"]);
        // an unknown type changes nothing
        d.check_type("CR3", false);
        assert_eq!(d.checked, [true, true, false, true, false]);
    }

    #[test]
    fn click_toggles_one_candidate() {
        let mut d = dialog();
        d.click(1, false);
        assert_eq!(d.checked, [true, false, true, false, true, true]);
        d.click(1, false);
        assert_eq!(d.checked, [true, true, true, false, true, true]);
        d.click(3, false);
        assert_eq!(d.checked, [true, true, true, false, true, true], "a duplicate can't be checked");
    }

    #[test]
    fn shift_click_sets_the_range_to_the_clicked_state() {
        let mut d = dialog();
        d.click(1, false);
        d.click(4, true);
        assert_eq!(d.checked, [true, false, false, false, false, true], "1..=4 unchecked, the duplicate left alone");
        assert_eq!(d.selected_paths(), ["img0.jpg", "img5.jpg"]);
        // backwards from the new anchor (4) re-checks 2..=4
        d.click(2, true);
        assert_eq!(d.checked, [true, false, true, false, true, true]);
    }

    #[test]
    fn shift_click_without_an_earlier_click_toggles_one() {
        let mut d = dialog();
        d.click(2, true);
        assert_eq!(d.checked, [true, true, false, false, true, true]);
    }

    /// With "Leave them", a file in Recently Deleted stays out of Check All and the type toggles,
    /// but a click still checks it: it is imported (restored) on its own (issue #1285).
    #[test]
    fn leave_them_keeps_a_trashed_file_out_until_it_is_clicked() {
        let mut d = dialog();
        d.set_trashed(vec![false, false, false, true, false, false]);
        assert!(d.checked[3], "restored by default");
        d.set_on_deleted("");
        assert!(!d.checked[3]);
        d.check_type("JPG", true);
        assert!(!d.checked[3], "the type toggle leaves it out");
        d.click(3, false);
        assert!(d.checked[3], "a click checks it");
        assert!(d.selected_paths().contains(&"img3.jpg".to_string()));
        d.click(3, false);
        assert!(!d.checked[3]);
        // a plain duplicate (not in Recently Deleted) still can't be checked
        d.set_trashed(vec![false; 6]);
        d.click(3, false);
        assert!(!d.checked[3]);
    }

    /// A badge too wide to fit beside the checkbox goes under it instead of covering it (issue #1285).
    #[test]
    fn a_badge_never_covers_the_checkbox() {
        let img = Rect::from_min_size(pos2(100.0, 50.0), vec2(116.0, 116.0));
        let cb = Rect::from_min_size(img.min + vec2(6.0, 6.0), vec2(16.0, 16.0));
        // "Will be restored" fits beside it, top right
        let short = badge_rect(img, cb, vec2(76.0, 12.0));
        assert_eq!(short.right(), img.right() - 4.0);
        assert_eq!(short.top(), img.top() + 6.0);
        // "In Recently Deleted" doesn't
        let long = badge_rect(img, cb, vec2(96.0, 12.0));
        assert!(!long.intersects(cb), "{long:?} covers {cb:?}");
        assert_eq!(long.right(), img.right() - 4.0);
        assert!(img.contains_rect(long));
    }

    #[test]
    fn shift_click_with_a_stale_anchor_toggles_one() {
        let mut d = dialog();
        d.last_clicked = Some(99);
        d.click(2, true);
        assert_eq!(d.checked, [true, true, false, false, true, true]);
    }
}
