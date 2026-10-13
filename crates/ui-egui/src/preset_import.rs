//! The Import Presets review: what the picked files hold, shown before anything is added, and then
//! what the import did.
//!
//! File ▸ Import Profiles & Presets…, the Presets panel's ⋯ menu and dropping preset files on the
//! window read the files without adding anything (`preset.import {dryRun}`), and show the presets
//! grouped as they would be filed, each as the current photo would look with it, with the settings
//! that have no counterpart here and the files that couldn't be read. Import adds them and the same
//! dialog says how it went (what was added and where to find it, what was skipped, what failed).
//! On a phone it is a page like the other dialogs. Scripts and agents skip the review:
//! `file.importPresets` with `paths` or `files` imports at once unless it is given `review: true`.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, Rect, Sense, pos2, vec2};
use lightcraft_develop::{DevelopSettings, Preset};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// Long edge of a preset's preview (px), as in the Presets panel.
const THUMB_EDGE: usize = 128;
/// The side of a preset's preview (points).
const THUMB: f32 = 52.0;
/// Entries listed in a list of files or profiles before "and N more".
const LISTED: usize = 6;

/// A preset the files hold, with the settings of the other editor that have no counterpart here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub preset: Preset,
    pub unmapped: Vec<String>,
}

/// How an import went.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Outcome {
    /// Presets added.
    pub presets: usize,
    /// Where they went: (group, how many), alphabetical.
    pub groups: Vec<(String, usize)>,
    /// Presets that were already there.
    pub skipped: usize,
    /// LUT profiles added.
    pub profiles: usize,
    /// Settings of the other editor that were left out.
    pub not_carried: Vec<String>,
    /// Files that couldn't be read: (file, why).
    pub problems: Vec<(String, String)>,
    /// The import itself failed: nothing was added.
    pub error: Option<String>,
}

/// The review dialog's state (`Dialog::PresetImport`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Review {
    /// What was picked, as the dialog names it: a file's name, or "3 files".
    pub source: String,
    /// The files and folders picked (read again when Import is pressed).
    pub paths: Vec<String>,
    /// Files given by content (the browser's file chooser): `[{name, data}]`.
    pub files: Arc<Vec<Value>>,
    /// The presets that would be added.
    pub items: Arc<Vec<Item>>,
    /// Presets in the files that are already in the library (or repeat another in them).
    pub already: usize,
    /// LUT profiles that would be added: (name, group).
    pub profiles: Vec<(String, String)>,
    /// Files that couldn't be read: (file, why).
    pub problems: Vec<(String, String)>,
    /// Set once Import has run: the dialog then shows how it went.
    pub outcome: Option<Outcome>,
}

impl PartialEq for Review {
    // (the preview is the big part, and is never edited: it is the same one or it isn't)
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.items, &o.items)
            && Arc::ptr_eq(&self.files, &o.files)
            && self.source == o.source
            && self.paths == o.paths
            && self.already == o.already
            && self.profiles == o.profiles
            && self.problems == o.problems
            && self.outcome == o.outcome
    }
}

impl Review {
    /// There is nothing to add: the files hold nothing new, or nothing we can read.
    fn nothing_to_add(&self) -> bool {
        self.items.is_empty() && self.profiles.is_empty()
    }
}

/// The dialog has just one button (Close / Done): there is nothing to choose.
pub fn informational(r: &Review) -> bool {
    match &r.outcome {
        None => r.nothing_to_add(),
        Some(o) => o.presets == 0 && o.profiles == 0,
    }
}

/// The dialog's action button (empty: none). With presets and profiles both it is just Import: the
/// summary above says what comes.
pub fn ok_label(r: &Review, compact: bool) -> String {
    let (n, p) = (r.items.len(), r.profiles.len());
    match &r.outcome {
        // (a phone's bar has little room: its title would run under a longer button)
        Some(o) if o.presets > 0 && compact => crate::i18n::tr("Show").to_string(),
        Some(o) if o.presets > 0 => crate::i18n::tr("Show Presets").to_string(),
        Some(_) => crate::i18n::tr("Close").to_string(),
        None if informational(r) => crate::i18n::tr("Close").to_string(),
        // (a phone's bar has little room: the page says what is imported)
        None if compact || (n > 0 && p > 0) => crate::i18n::tr("Import").to_string(),
        None if n > 0 => crate::i18n::tr_format!("Import {n} Preset{}", plural(n), n = n),
        None => crate::i18n::tr_format!("Import {p} Profile{}", plural(p), p = p),
    }
}

/// The dialog's other button.
pub fn cancel_label(r: &Review) -> &'static str {
    if r.outcome.is_some() { "Done" } else { "Cancel" }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "Imported 2 profiles (Profile browser ▸ their groups)": the same message the toast uses.
fn profiles_imported(n: usize) -> String {
    crate::i18n::tr_format!("Imported {profiles} profile{} (Profile browser ▸ their groups)", plural(n), profiles = n)
}

// ------------------------------------------------------------------------------------ reading

/// `.cube` files and what can hold them (folders, zips) go to the profile importer too.
pub fn lut_paths(paths: &[String]) -> Vec<&String> {
    paths
        .iter()
        .filter(|p| {
            let l = p.to_ascii_lowercase();
            l.ends_with(".cube") || l.ends_with(".zip") || std::path::Path::new(p.as_str()).is_dir()
        })
        .collect()
}

/// What the preset importer is given (a `.cube` file alone is no preset).
pub fn preset_paths(paths: &[String]) -> Vec<&String> {
    paths.iter().filter(|p| !p.to_ascii_lowercase().ends_with(".cube")).collect()
}

fn is_cube(f: &Value) -> bool {
    f.get("name").and_then(Value::as_str).is_some_and(|n| n.to_ascii_lowercase().ends_with(".cube"))
}

fn name_of(f: &Value) -> &str {
    f.get("name").and_then(Value::as_str).unwrap_or_default()
}

/// A file picker's `name: reason` line as (name, reason).
fn problem_of(line: &str) -> (String, String) {
    match line.split_once(": ") {
        Some((n, why)) => (n.to_string(), why.to_string()),
        None => (line.to_string(), String::new()),
    }
}

/// `[file, why]` pairs of a command's `failed` list.
fn failures(v: &Value) -> Vec<(String, String)> {
    let text = |x: Option<&Value>| x.and_then(Value::as_str).unwrap_or_default().to_string();
    v["failed"].as_array().into_iter().flatten().map(|f| (text(f.get(0)), text(f.get(1)))).collect()
}

/// A file's name without its folders.
fn short(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path)
}

/// `failed` without the archives that hold LUT profiles: a zip of LUTs has no presets, and that is no
/// failure (the profiles are what it holds). An archive with neither stays.
fn without_profile_archives(app: &mut LightcraftApp, failed: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut keep = Vec::new();
    for (file, why) in failed {
        let holds_luts = why == lightcraft_engine::preset_import::NO_PRESETS_IN_ARCHIVE
            && app
                .session
                .execute("profile.import", &json!({"paths": [&file], "dryRun": true}))
                .is_ok_and(|v| v["imported"].as_array().is_some_and(|a| !a.is_empty()));
        if !holds_luts {
            keep.push((file, why));
        }
    }
    keep
}

/// Read the files (adding nothing) and open the review. `files`: files given by content;
/// `pick_failed`: `name: reason` for the files a picker couldn't hand over.
pub fn open(app: &mut LightcraftApp, paths: Vec<String>, files: Vec<Value>, pick_failed: Vec<String>) -> Result<Value, String> {
    let (cubes, preset_files): (Vec<Value>, Vec<Value>) = files.into_iter().partition(is_cube);
    let mut problems: Vec<(String, String)> = pick_failed.iter().map(|l| problem_of(l)).collect();
    // (a LUT given by content can't become a profile: a profile is kept as a file in the library's folder)
    for c in &cubes {
        problems.push((name_of(c).to_string(), crate::i18n::tr("profiles can only be imported in the desktop and iOS apps").to_string()));
    }
    let (luts, presets) = (lut_paths(&paths), preset_paths(&paths));
    let read = if presets.is_empty() && preset_files.is_empty() {
        json!({"imported": [], "failed": [], "skipped": 0})
    } else {
        app.session
            .execute("preset.import", &json!({"paths": presets, "files": preset_files, "dryRun": true, "includeSettings": true}))
            .map_err(|e| e.to_string())?
    };
    let mut profiles = Vec::new();
    let mut profile_failed = Vec::new();
    if !luts.is_empty() {
        match app.session.execute("profile.import", &json!({"paths": luts, "dryRun": true})) {
            Ok(v) => {
                let text = |x: &Value| x.as_str().unwrap_or_default().to_string();
                profiles = v["imported"].as_array().into_iter().flatten().map(|p| (text(&p["name"]), text(&p["group"]))).collect();
                profile_failed = failures(&v);
            }
            Err(e) => problems.push((crate::i18n::tr("Profiles").to_string(), e.to_string())),
        }
    }
    problems.extend(without_profile_archives(app, failures(&read)));
    problems.extend(profile_failed);
    let items: Vec<Item> = read["imported"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            let text = |k: &str| i[k].as_str().unwrap_or_default().to_string();
            Item {
                preset: Preset {
                    id: text("id"),
                    name: text("name"),
                    group: text("group"),
                    settings: i["settings"].clone(),
                    favorite: false,
                    builtin: false,
                },
                unmapped: i["unmapped"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
            }
        })
        .collect();
    let already = read["skipped"].as_u64().unwrap_or(0) as usize;
    let mut names: Vec<String> = paths.iter().map(|p| short(p).to_string()).collect();
    names.extend(preset_files.iter().chain(&cubes).map(|f| short(name_of(f)).to_string()));
    let source = match names.as_slice() {
        [one] => one.clone(),
        many => crate::i18n::tr_format!("{} files", many.len()),
    };
    let review = Review { source, paths, files: Arc::new(preset_files), items: Arc::new(items), already, profiles, problems, outcome: None };
    let summary = json!({
        "review": true,
        "presets": review.items.len(),
        "already": review.already,
        "profiles": review.profiles.len(),
        "failed": review.problems.len(),
    });
    app.ui.dialog = Some(crate::state::Dialog::PresetImport { review: Box::new(review) });
    Ok(summary)
}

/// The dialog's action: Import (then the dialog shows how it went), or, after that, Show Presets.
/// What the import did goes into the replacement dialog (the caller keeps a dialog this replaced).
pub fn confirm(app: &mut LightcraftApp, review: &Review) -> Result<Value, String> {
    // (nothing to add: Return closes it)
    if informational(review) && review.outcome.is_none() {
        return Ok(Value::Null);
    }
    if let Some(o) = &review.outcome {
        reveal(app, o);
        return Ok(json!({"presets": o.presets, "profiles": o.profiles}));
    }
    let mut done = review.clone();
    let o = run(app, review);
    let summary = json!({"imported": o.presets, "skipped": o.skipped, "profiles": o.profiles, "failed": o.problems.len(), "error": o.error});
    if o.presets > 0
        && let Some((group, _)) = o.groups.first()
    {
        // (a phone's strip shows the new presets' group, whichever way the dialog is left)
        let group = group.clone();
        app.pending_strip_group = Some(group);
    }
    done.outcome = Some(o);
    app.ui.dialog = Some(crate::state::Dialog::PresetImport { review: Box::new(done) });
    Ok(summary)
}

/// Add what the review showed.
fn run(app: &mut LightcraftApp, review: &Review) -> Outcome {
    let mut o = Outcome::default();
    let (luts, presets) = (lut_paths(&review.paths), preset_paths(&review.paths));
    if !luts.is_empty() {
        match app.session.execute("profile.import", &json!({"paths": luts})) {
            Ok(v) => {
                o.profiles = v["imported"].as_array().map_or(0, Vec::len);
                o.problems.extend(failures(&v));
            }
            Err(e) => o.problems.push((crate::i18n::tr("Profiles").to_string(), e.to_string())),
        }
    }
    if !presets.is_empty() || !review.files.is_empty() {
        match app.session.execute("preset.import", &json!({"paths": presets, "files": *review.files})) {
            Ok(v) => {
                let mut groups: BTreeMap<String, usize> = BTreeMap::new();
                let mut left_out: Vec<String> = Vec::new();
                for i in v["imported"].as_array().into_iter().flatten() {
                    o.presets += 1;
                    *groups.entry(i["group"].as_str().unwrap_or_default().to_string()).or_default() += 1;
                    left_out.extend(i["unmapped"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
                }
                left_out.sort_unstable();
                left_out.dedup();
                o.groups = groups.into_iter().collect();
                o.not_carried = left_out;
                o.skipped = v["skipped"].as_u64().unwrap_or(0) as usize;
                o.problems.extend(without_profile_archives(app, failures(&v)));
            }
            Err(e) => o.error = Some(e.to_string()),
        }
    }
    // what the review couldn't read is still not imported
    let unseen: Vec<_> = review.problems.iter().filter(|p| !o.problems.contains(p)).cloned().collect();
    o.problems.extend(unseen);
    if o.presets == 0 && o.profiles == 0 && o.error.is_none() && o.problems.is_empty() && o.skipped == 0 {
        o.error = Some(crate::i18n::tr("Nothing was imported").to_string());
    }
    o
}

/// Show the new presets: the Presets panel (a phone's sheet), at the group they went to.
fn reveal(app: &mut LightcraftApp, o: &Outcome) {
    if o.presets > 0 {
        app.ui.presets = true;
    }
}

// --------------------------------------------------------------------------------------- body

/// What a preset's settings touch, as labels: `light`, `mixer`… → "Light", "Color Mixer"…
fn sets(settings: &Value) -> String {
    let label = |k: &str| -> String {
        match k {
            "light" => "Light".into(),
            "color" => "Color".into(),
            "curve" => "Curve".into(),
            "mixer" => "Color Mixer".into(),
            "grading" => "Color Grading".into(),
            "detail" => "Detail".into(),
            "effects" => "Effects".into(),
            "grain" => "Grain".into(),
            "vignette" => "Vignette".into(),
            "calibration" => "Calibration".into(),
            "wb" => "White Balance".into(),
            "profile" => "Profile".into(),
            "optics" => "Optics".into(),
            "geometry" => "Geometry".into(),
            "crop" => "Crop".into(),
            "treatment" => "Black & White".into(),
            "masks" => "Masks".into(),
            "spots" => "Spot Removal".into(),
            "hdr" => "HDR".into(),
            other => {
                let mut c = other.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        }
    };
    settings.as_object().map(|o| o.keys().map(|k| crate::i18n::tr(&label(k)).to_string()).collect::<Vec<_>>().join(" · ")).unwrap_or_default()
}

/// One line of text, cut with an ellipsis where it doesn't fit.
fn line(ui: &egui::Ui, text: String, font: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text, font, color, width);
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}

/// The preset as the current photo would look with it (a plain tile without a photo).
fn thumb(app: &mut LightcraftApp, ui: &egui::Ui, rect: Rect, preset: &Preset, photo: Option<(lightcraft_catalog::PhotoId, &DevelopSettings)>) {
    let t = Tokens::get(ui.ctx());
    let mut shown = None;
    if ui.is_rect_visible(rect)
        && let Some((id, current)) = photo
        && let Some(job) = app.session.variant_job(id, &preset.apply(current, 1.0), THUMB_EDGE)
        && let Some(tex) = app.renderer.variant(job)
    {
        shown = Some((tex.tex.id(), crate::panels::presets::cover_uv(rect, tex.size)));
    }
    match shown {
        Some((tex, uv)) => {
            ui.painter().add(egui::epaint::RectShape::filled(rect, 6, Color32::WHITE).with_texture(tex, uv));
        }
        None => {
            ui.painter().rect_filled(rect, 6.0, t.inset);
        }
    }
}

/// A heading line: an icon and a title.
fn heading(ui: &mut egui::Ui, icon: Icon, color: Color32, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        paint(ui.painter(), r.shrink(2.0), icon, color);
        ui.label(egui::RichText::new(text).font(t.semibold(16.0)).color(t.text));
    });
}

fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text.into()).color(t.text_dim));
}

/// `file — why` lines (the first few), for the files that couldn't be read.
fn problem_lines(ui: &mut egui::Ui, problems: &[(String, String)]) {
    let t = Tokens::get(ui.ctx());
    for (file, why) in problems.iter().take(LISTED) {
        let text = if why.is_empty() { short(file).to_string() } else { format!("{} — {why}", short(file)) };
        ui.add(egui::Label::new(egui::RichText::new(text).size(12.5).color(t.caution)).wrap());
    }
    if problems.len() > LISTED {
        note(ui, crate::i18n::tr_format!("+{} more", problems.len() - LISTED));
    }
}

/// The dialog's content.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, r: &mut Review) {
    ui.set_width(ui.available_width());
    match &r.outcome {
        Some(o) => outcome_body(ui, r, o),
        None => review_body(app, ui, r),
    }
}

fn review_body(app: &mut LightcraftApp, ui: &mut egui::Ui, r: &Review) {
    let t = Tokens::get(ui.ctx());
    let (n, g) = (r.items.len(), r.items.iter().map(|i| i.preset.group.as_str()).collect::<std::collections::BTreeSet<_>>().len());
    // what was picked, and what it holds
    ui.label(egui::RichText::new(&r.source).font(t.semibold(15.0)).color(t.text));
    let mut parts = Vec::new();
    if n > 0 {
        parts.push(crate::i18n::tr_format!("{n} preset{} in {g} group{}", plural(n), plural(g), n = n, g = g));
    }
    if !r.profiles.is_empty() {
        parts.push(crate::i18n::tr_format!("{p} profile{}", plural(r.profiles.len()), p = r.profiles.len()));
    }
    // (with nothing to add, the sentence below says it)
    if r.already > 0 && !r.nothing_to_add() {
        parts.push(crate::i18n::tr_format!("{a} already in your library", a = r.already));
    }
    if r.nothing_to_add() {
        let why =
            if r.already > 0 { "Nothing new: everything in these files is already in your library." } else { "There are no presets in these files." };
        note(ui, crate::i18n::tr(why));
    }
    if !parts.is_empty() {
        let text = parts.join(" · ");
        let r = ui.label(egui::RichText::new(text).color(t.text_dim));
        register(ui.ctx(), "presetImport:summary", r.rect);
    }
    // the presets, by group, each as this photo would look with it
    let active = app.session.active();
    let current = active.and_then(|id| app.session.develop_of(id)).map(|d| (*d).clone());
    let photo = active.zip(current.as_ref());
    let mut groups: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    for i in r.items.iter() {
        groups.entry(i.preset.group.as_str()).or_default().push(i);
    }
    for (group, items) in groups {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let h = ui.label(egui::RichText::new(group).font(t.semibold(13.0)).color(t.text_label));
            register(ui.ctx(), format!("presetImport:group:{group}"), h.rect);
            ui.label(egui::RichText::new(items.len().to_string()).color(t.text_dim));
        });
        for it in items {
            let left_out = !it.unmapped.is_empty();
            let h = if left_out { THUMB + 18.0 } else { THUMB + 6.0 };
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
            register(ui.ctx(), format!("presetImport:item:{}", it.preset.id), row);
            let tile = Rect::from_min_size(row.min + vec2(0.0, 3.0), vec2(THUMB, THUMB));
            thumb(app, ui, tile, &it.preset, photo);
            let x = tile.right() + 12.0;
            let width = (row.right() - x).max(40.0);
            let name = line(ui, it.preset.name.clone(), t.font(14.0), t.text, width);
            let sub = line(ui, sets(&it.preset.settings), t.font(12.0), t.text_dim, width);
            let mut y = row.top() + 6.0;
            let drawn = [(name, t.text), (sub, t.text_dim)];
            for (galley, color) in drawn {
                let hgt = galley.size().y;
                ui.painter().galley(pos2(x, y), galley, color);
                y += hgt + 3.0;
            }
            if left_out {
                let text = crate::i18n::tr_format!("Not carried over: {what}", what = it.unmapped.join(", "));
                let galley = line(ui, text, t.font(12.0), t.caution, width);
                ui.painter().galley(pos2(x, y), galley, t.caution);
            }
        }
    }
    if !r.profiles.is_empty() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Profiles")).font(t.semibold(13.0)).color(t.text_label));
        for (name, group) in r.profiles.iter().take(LISTED) {
            note(ui, format!("{name} · {group}"));
        }
        if r.profiles.len() > LISTED {
            note(ui, crate::i18n::tr_format!("+{} more", r.profiles.len() - LISTED));
        }
    }
    if !r.problems.is_empty() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Couldn't be read")).font(t.semibold(13.0)).color(t.caution));
        problem_lines(ui, &r.problems);
    }
}

fn outcome_body(ui: &mut egui::Ui, r: &Review, o: &Outcome) {
    let t = Tokens::get(ui.ctx());
    let failed = o.presets == 0 && o.profiles == 0 && (o.error.is_some() || !o.problems.is_empty());
    let (what, icon, color) = if failed {
        (crate::i18n::tr("Import failed").to_string(), Icon::Close, t.reject)
    } else if o.presets == 0 && o.profiles == 0 {
        (crate::i18n::tr("Nothing new to import").to_string(), Icon::Info, t.text_dim)
    } else if o.presets > 0 {
        (crate::i18n::tr_format!("Imported {n} preset{}", plural(o.presets), n = o.presets), Icon::Check, t.accent)
    } else {
        (profiles_imported(o.profiles), Icon::Check, t.accent)
    };
    heading(ui, icon, color, &what);
    ui.label(egui::RichText::new(&r.source).color(t.text_dim));
    if let Some(e) = &o.error {
        ui.add(egui::Label::new(egui::RichText::new(e).color(t.reject)).wrap());
    }
    if o.presets > 0 {
        ui.add_space(2.0);
        let r = ui.label(egui::RichText::new(crate::i18n::tr("Find them in Presets:")).color(t.text));
        register(ui.ctx(), "presetImport:where", r.rect);
        for (group, n) in &o.groups {
            ui.label(egui::RichText::new(format!("{group} · {n}")).font(t.semibold(13.0)).color(t.text_label));
        }
    }
    if o.profiles > 0 && o.presets > 0 {
        note(ui, profiles_imported(o.profiles));
    }
    if o.skipped > 0 {
        note(ui, crate::i18n::tr_format!("{a} already in your library", a = o.skipped));
    }
    if !o.not_carried.is_empty() {
        let shown: Vec<&str> = o.not_carried.iter().take(4).map(String::as_str).collect();
        let what = format!("{}{}", shown.join(", "), if o.not_carried.len() > 4 { "…" } else { "" });
        ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tr_format!("Not carried over: {what}", what = what)).color(t.caution)).wrap());
    }
    if !o.problems.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Couldn't be read")).font(t.semibold(13.0)).color(t.caution));
        problem_lines(ui, &o.problems);
    }
}
