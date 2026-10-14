//! The Import Presets review every UI shows: what the picked files hold, read without adding
//! anything (`preset.importReview`), and how the import went (`preset.importRun`). Presets go
//! through `preset.import`, `.cube` LUTs (and folders and zips of them) through `profile.import`.

use std::collections::BTreeMap;
use std::sync::Arc;

use lightcraft_develop::Preset;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Session;

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

/// What the picked files hold, read without adding anything ([`review`]), and how the import went
/// once it has run ([`run`]).
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
    pub fn nothing_to_add(&self) -> bool {
        self.items.is_empty() && self.profiles.is_empty()
    }
}

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
pub fn short(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path)
}

/// `failed` without the archives that hold LUT profiles: a zip of LUTs has no presets, and that is no
/// failure (the profiles are what it holds). An archive with neither stays.
fn without_profile_archives(s: &mut Session, failed: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut keep = Vec::new();
    for (file, why) in failed {
        let holds_luts = why == crate::preset_import::NO_PRESETS_IN_ARCHIVE
            && s.execute("profile.import", &json!({"paths": [&file], "dryRun": true}))
                .is_ok_and(|v| v["imported"].as_array().is_some_and(|a| !a.is_empty()));
        if !holds_luts {
            keep.push((file, why));
        }
    }
    keep
}

/// Read the files, adding nothing: the presets they would add (with the settings that have no
/// counterpart here), the LUT profiles, the files that couldn't be read. `files`: files given by
/// content (`[{name, data}]`); `pick_failed`: `name: reason` for the files a picker couldn't hand over.
pub fn review(s: &mut Session, paths: Vec<String>, files: Vec<Value>, pick_failed: Vec<String>) -> Result<Review, String> {
    let (cubes, preset_files): (Vec<Value>, Vec<Value>) = files.into_iter().partition(is_cube);
    let mut problems: Vec<(String, String)> = pick_failed.iter().map(|l| problem_of(l)).collect();
    // (a LUT given by content can't become a profile: a profile is kept as a file in the library's folder)
    for c in &cubes {
        problems.push((name_of(c).to_string(), "profiles can only be imported in the desktop and iOS apps".to_string()));
    }
    let (luts, presets) = (lut_paths(&paths), preset_paths(&paths));
    let read = if presets.is_empty() && preset_files.is_empty() {
        json!({"imported": [], "failed": [], "skipped": 0})
    } else {
        s.execute("preset.import", &json!({"paths": presets, "files": preset_files, "dryRun": true, "includeSettings": true}))
            .map_err(|e| e.to_string())?
    };
    let mut profiles = Vec::new();
    let mut profile_failed = Vec::new();
    if !luts.is_empty() {
        match s.execute("profile.import", &json!({"paths": luts, "dryRun": true})) {
            Ok(v) => {
                let text = |x: &Value| x.as_str().unwrap_or_default().to_string();
                profiles = v["imported"].as_array().into_iter().flatten().map(|p| (text(&p["name"]), text(&p["group"]))).collect();
                profile_failed = failures(&v);
            }
            Err(e) => problems.push(("Profiles".to_string(), e.to_string())),
        }
    }
    problems.extend(without_profile_archives(s, failures(&read)));
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
        many => format!("{} files", many.len()),
    };
    Ok(Review { source, paths, files: Arc::new(preset_files), items: Arc::new(items), already, profiles, problems, outcome: None })
}

/// Add what the review showed (the files are read again), and say how it went.
pub fn run(s: &mut Session, review: &Review) -> Outcome {
    let mut o = Outcome::default();
    let (luts, presets) = (lut_paths(&review.paths), preset_paths(&review.paths));
    if !luts.is_empty() {
        match s.execute("profile.import", &json!({"paths": luts})) {
            Ok(v) => {
                o.profiles = v["imported"].as_array().map_or(0, Vec::len);
                o.problems.extend(failures(&v));
            }
            Err(e) => o.problems.push(("Profiles".to_string(), e.to_string())),
        }
    }
    if !presets.is_empty() || !review.files.is_empty() {
        match s.execute("preset.import", &json!({"paths": presets, "files": *review.files})) {
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
                o.problems.extend(without_profile_archives(s, failures(&v)));
            }
            Err(e) => o.error = Some(e.to_string()),
        }
    }
    // what the review couldn't read is still not imported
    let unseen: Vec<_> = review.problems.iter().filter(|p| !o.problems.contains(p)).cloned().collect();
    o.problems.extend(unseen);
    if o.presets == 0 && o.profiles == 0 && o.error.is_none() && o.problems.is_empty() && o.skipped == 0 {
        o.error = Some("Nothing was imported".to_string());
    }
    o
}
