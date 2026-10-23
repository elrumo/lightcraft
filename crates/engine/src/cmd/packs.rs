//! App packs: looks that come with an app instead of living in a library. A pack is a folder of `.cube` 3D LUTs
//! (profiles) and `.lcpreset` files (presets), handed to the engine with `pack.set` each time the app starts.
//! Nothing is copied into the library, saved or synced: a pack's profiles can't be deleted and its presets are
//! built in (they can be hidden, not edited), so an app update changes them for everyone, and a photo edited with
//! one keeps working on every device that has the app.
//!
//! A pack's profile id is `lut:pack-<pack>-<name>` ([`profile_id`]), so presets in the same folder can name its
//! profiles before the engine has read them.

use std::collections::HashSet;
use std::path::Path;

use lightcraft_develop::Preset;
use serde_json::{Value, json};

use super::lut_profiles::LutProfile;
use super::{CommandSpec, always, bad, cmd, str_param};
use crate::presets::slug;
use crate::{Result, Session};

/// Every pack profile's id starts with this.
pub const PROFILE_PREFIX: &str = "lut:pack-";

/// Biggest file `pack.set` reads (a 65-point `.cube` is about 6 MB).
const MAX_FILE: u64 = 64 << 20;
/// Most files `pack.set` reads from a folder.
const MAX_FILES: usize = 500;

/// Whether `id` names a pack's profile (known even before its pack is set: favourites and recent profiles of a
/// pack are read from the library before the app sets the pack).
pub fn is_pack_profile(id: &str) -> bool {
    id.starts_with(PROFILE_PREFIX)
}

/// The id of the profile called `name` in pack `pack`.
pub fn profile_id(pack: &str, name: &str) -> String {
    format!("{PROFILE_PREFIX}{}-{}", slug(pack), slug(name))
}

/// One pack as set: its profiles and presets.
#[derive(Clone, Debug, Default)]
pub struct Pack {
    pub id: String,
    pub profiles: Vec<LutProfile>,
    pub presets: Vec<Preset>,
}

impl Session {
    /// Every pack's profiles.
    pub fn pack_profiles(&self) -> impl Iterator<Item = &LutProfile> {
        self.packs.iter().flat_map(|p| p.profiles.iter())
    }

    /// Remember which of the packs' presets are favourites now: the list holds that until a rebuild of the list
    /// (`set_server_presets`, `pack.set`) drops them, and the library only knows what was saved.
    pub(crate) fn fold_pack_favorites(&mut self) {
        let ids: HashSet<&str> = self.packs.iter().flat_map(|p| p.presets.iter().map(|q| q.id.as_str())).collect();
        let (mut on, mut off): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        for p in self.presets.iter().filter(|p| p.builtin && ids.contains(p.id.as_str())) {
            if p.favorite { on.push(p.id.clone()) } else { off.push(p.id.clone()) }
        }
        self.pack_favorites.retain(|f| !on.contains(f) && !off.contains(f));
        self.pack_favorites.extend(on);
    }

    /// Put the packs' presets into the list, after the built-in ones and before the user's (and take the old ones
    /// out first), keeping the favourites. A preset whose id another pack's already has, or a user's preset has, is left out.
    pub(crate) fn insert_pack_presets(&mut self) {
        self.fold_pack_favorites();
        let mine: HashSet<String> = self.packs.iter().flat_map(|p| p.presets.iter().map(|q| q.id.clone())).collect();
        self.presets.retain(|p| !(p.builtin && mine.contains(&p.id)));
        let mut seen: HashSet<String> = self.presets.iter().map(|p| p.id.clone()).collect();
        let fresh: Vec<Preset> = self
            .packs
            .iter()
            .flat_map(|p| p.presets.iter())
            .filter(|p| seen.insert(p.id.clone()))
            .map(|p| Preset { builtin: true, favorite: self.pack_favorites.contains(&p.id), ..p.clone() })
            .collect();
        let at = self.presets.iter().rposition(|p| p.builtin).map_or(0, |i| i + 1);
        self.presets.splice(at..at, fresh);
    }

    /// Take pack `id` out: its profiles stop rendering, its presets leave the list.
    pub fn remove_pack(&mut self, id: &str) {
        let Some(i) = self.packs.iter().position(|p| p.id == id) else { return };
        self.fold_pack_favorites();
        let old = self.packs.remove(i);
        for p in &old.profiles {
            lightcraft_pipeline::lut::unregister(&p.id);
        }
        let gone: HashSet<&str> = old.presets.iter().map(|p| p.id.as_str()).collect();
        self.presets.retain(|p| !(p.builtin && gone.contains(p.id.as_str())));
    }
}

fn set_pack(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pack.set";
    let name = str_param(p, "id").ok_or_else(|| bad(C, "missing id"))?;
    let id = slug(name);
    // (a `dir` or `files` of the wrong kind is an error, not "take the pack out")
    if p.get("dir").is_some_and(|d| !d.is_null() && !d.is_string()) {
        return Err(bad(C, "dir must be a folder path"));
    }
    if p.get("files").is_some_and(|f| !f.is_null() && !f.as_array().is_some_and(|a| a.iter().all(Value::is_string))) {
        return Err(bad(C, "files must be a list of paths"));
    }
    let dir = str_param(p, "dir").map(Path::new);
    let listed: Vec<std::path::PathBuf> = p
        .get("files")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(std::path::PathBuf::from).collect())
        .unwrap_or_default();
    if dir.is_none() && listed.is_empty() {
        s.remove_pack(&id);
        return Ok(json!({"id": id, "profiles": 0, "presets": 0, "failed": []}));
    }
    let group = str_param(p, "group")
        .map(str::to_string)
        .or_else(|| dir.and_then(|d| d.file_name()).and_then(|n| crate::preset_import::group_from_dir(&n.to_string_lossy())))
        .unwrap_or_else(|| name.to_string());
    let mut files = listed;
    if let Some(dir) = dir {
        files.extend(
            std::fs::read_dir(dir)
                .map_err(|e| bad(C, format!("{}: {e}", dir.display())))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|f| f.is_file()),
        );
    }
    files.retain(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("cube") || e.eq_ignore_ascii_case(crate::presets::LCPRESET_EXT)));
    files.sort();
    files.dedup();
    files.truncate(MAX_FILES);
    // ids a pack may not take: the built-in presets', the sync server's and the user's imports'
    let reserved: HashSet<String> = crate::presets::builtin().into_iter().map(|p| p.id).collect();

    let (mut registered, mut profiles, mut presets, mut failed) = (Vec::new(), Vec::<LutProfile>::new(), Vec::<Preset>::new(), Vec::new());
    for f in files {
        let file_name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext != "cube" && ext != crate::presets::LCPRESET_EXT {
            continue;
        }
        if f.metadata().map_or(true, |m| m.len() > MAX_FILE) {
            failed.push(json!([file_name, "too big or unreadable"]));
            continue;
        }
        if ext == "cube" {
            // only the header is read: the table is parsed when a render first needs it (a pack's tables are big)
            let mut head = [0u8; 16 << 10];
            let n = std::fs::File::open(&f).and_then(|mut h| std::io::Read::read(&mut h, &mut head)).unwrap_or(0);
            match lightcraft_pipeline::lut::cube_header(&String::from_utf8_lossy(&head[..n])) {
                Ok((_, title)) => {
                    let stem = f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let display = title.filter(|t| !t.trim().is_empty()).unwrap_or(stem);
                    let pid = profile_id(&id, &display);
                    if profiles.iter().any(|x| x.id == pid) {
                        failed.push(json!([file_name, format!("the profile `{display}` is in the pack already")]));
                    } else {
                        profiles.push(LutProfile { id: pid.clone(), name: display, group: group.clone(), file: f.to_string_lossy().to_string() });
                        registered.push((pid, f.clone()));
                    }
                }
                Err(e) => failed.push(json!([file_name, e])),
            }
        } else {
            let list = std::fs::read(&f).map_err(|e| e.to_string()).and_then(|bytes| crate::presets::parse_preset_file(&file_name, &bytes));
            match list {
                Ok(list) => {
                    for q in list {
                        if reserved.contains(&q.id) || q.id.starts_with(crate::presets::SERVER_PREFIX) || q.id.starts_with("user.") {
                            failed.push(json!([file_name, format!("the preset id `{}` is not the pack's to take", q.id)]));
                        } else if presets.iter().any(|x| x.id == q.id) {
                            failed.push(json!([file_name, format!("the preset id `{}` is in the pack already", q.id)]));
                        } else {
                            presets.push(Preset { builtin: true, favorite: false, ..q });
                        }
                    }
                }
                Err(e) => failed.push(json!([file_name, e])),
            }
        }
    }

    // swap the pack in: the old version's tables and presets out, the new ones registered
    s.remove_pack(&id);
    for (pid, file) in registered {
        lightcraft_pipeline::lut::register_file(&pid, file);
    }
    let counts = json!({"id": id, "profiles": profiles.len(), "presets": presets.len(), "failed": failed});
    s.packs.push(Pack { id, profiles, presets });
    s.insert_pack_presets();
    Ok(counts)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "pack.set",
        "Set App Pack",
        [],
        None,
        "{id: pack name, dir?: folder and/or files?: [paths] of .cube files (profiles `lut:pack-<id>-<name>`, named by their TITLE) and .lcpreset files (built-in presets), group?: the profiles' group (default the folder's name, else the pack's)} → {id, profiles, presets, failed} — sets the app's own looks, replacing the pack's earlier version; nothing is saved or synced, so call it each time the app starts. Without `dir` and `files` the pack is taken out.",
        always,
        set_pack
    )]
}
