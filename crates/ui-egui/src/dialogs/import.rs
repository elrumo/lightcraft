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

mod body;
mod scan;
mod tags;
mod task;

#[cfg(test)]
pub(crate) use body::grid_height;
#[cfg(test)]
use body::{DEFAULT_BELOW_GRID, badge_rect, dup_counts};
pub use body::{DEFAULT_FOLDER_TEMPLATE, body, example_destination, mobile_body, source_summary};
pub(crate) use body::{DIALOG_SIZE, note_dialog_bottom};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use scan::cards_seen;
pub use scan::{ScanTask, browse, open, open_staged, poll_scan, running, scan_progress};
#[cfg(test)]
pub(crate) use scan::{stall_scan, stall_scan_of};
pub(crate) use scan::busy_synchronizing;
pub(crate) use task::import_params;
pub(crate) use tags::{tag_field_width, tag_help, tag_toggle, unknown_tags_warning};
pub use task::{ImportTask, progress, start, start_dropped, start_paths, tick};

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
    "3FR", "ARW", "CR2", "CR3", "CRW", "DCR", "DNG", "ERF", "FFF", "IIQ", "KDC", "MEF", "MOS", "MRW", "NEF", "NRW", "ORF", "ORI", "PEF", "RAF",
    "RAW", "RW2", "RWL", "SR2", "SRF", "SRW", "X3F",
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
        // OM Digital Solutions' High Res / Pro Capture originals
        assert!(is_raw(&ImportCandidate { path: "/card/P5048183.ORI".into(), ..Default::default() }));
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
