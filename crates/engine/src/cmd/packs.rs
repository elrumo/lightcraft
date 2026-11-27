//! App packs: looks that come with an app instead of living in a library. A pack is a folder of `.cube` 3D LUTs
//! (profiles) and `.lcpreset` files (presets), handed to the engine with `pack.set` each time the app starts.
//! Nothing is copied into the library, saved or synced: a pack's profiles can't be deleted and its presets are
//! built in (they can be hidden, not edited), so an app update changes them for everyone, and a photo edited with
//! one keeps working on every device that has the app.
//!
//! A pack's profile id is `lut:pack-<pack>-<name>` ([`profile_id`]), so presets in the same folder can name its
//! profiles before the engine has read them.
//!
//! An app need not carry every table: a look whose table it fetches when it is first wanted is declared with
//! `pack.set` `absent`. It is a profile like the others (listed, named on the photos that use it), with no file: a
//! render with it draws without the table until the app sets the pack again with the file, `pack.needed` says which
//! of them the library's photos use, and a final output (an export, a print) of such a photo is refused
//! ([`Session::look_here`]) rather than written without its look.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use lightcraft_catalog::{Photo, PhotoId};

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
    /// By name. One with an empty `file` is absent: its table is not on this device (`pack.set` `absent`).
    pub profiles: Vec<LutProfile>,
    pub presets: Vec<Preset>,
    /// Profiles the pack no longer offers, kept so photos edited with them still render: each draws another file's table.
    /// Not listed (no browser entry, no preset), but `profile.lutFile` and the photo's profile name know them. One with
    /// an empty `file` is absent, like a profile.
    pub aliases: Vec<LutProfile>,
}

/// What a render key carries for a LUT profile with no table registered ([`crate::media::MediaCache::camera_profile_salt`]).
pub(crate) const NO_TABLE_SALT: u64 = 0x6e6f_5f74_6162_6c65;

impl Session {
    /// Whether `id` is a preset an app pack brought.
    pub fn is_pack_preset(&self, id: &str) -> bool {
        self.packs.iter().any(|p| p.presets.iter().any(|q| q.id == id))
    }

    /// Whether `group` is a group an app pack's presets are in.
    pub fn is_pack_group(&self, group: &str) -> bool {
        self.packs.iter().any(|p| p.presets.iter().any(|q| q.group == group))
    }

    /// Every pack's retired profiles (`pack.set` `aliases`).
    pub fn pack_aliases(&self) -> impl Iterator<Item = &LutProfile> {
        self.packs.iter().flat_map(|p| p.aliases.iter())
    }

    /// Every pack's profiles.
    pub fn pack_profiles(&self) -> impl Iterator<Item = &LutProfile> {
        self.packs.iter().flat_map(|p| p.profiles.iter())
    }

    /// The pack look `id` names, when its table is not on this device (`pack.set` `absent`, or a retired look whose
    /// table is). `None` for every other id, a pack look nothing was declared about included.
    pub fn absent_look(&self, id: &str) -> Option<&LutProfile> {
        if !is_pack_profile(id) {
            return None;
        }
        self.pack_profiles().chain(self.pack_aliases()).find(|p| p.file.is_empty() && p.id == id)
    }

    /// Refuses a final output of `p` (an export, a print, a contact sheet's file) while the table of its look is not
    /// on this device: it would be written without the look, and nothing would say so. A frontend tells the refusal
    /// by its kind ([`crate::ErrorKind::LookMissing`]), fetches the table, sets the pack again and tries again.
    pub fn look_here(&self, p: &Photo) -> Result<()> {
        match self.absent_look(&p.develop.profile.id) {
            Some(look) => Err(crate::EngineError::LookMissing {
                profile: look.id.clone(),
                msg: format!(
                    "{}: its look, {}, has not been downloaded to this device yet. It is fetched once the device is online",
                    p.file_name, look.name
                ),
            }),
            None => Ok(()),
        }
    }

    /// The absent pack looks `photos` are developed with (their settings now, and their versions'), sorted.
    fn looks_needed<'a>(&self, photos: impl Iterator<Item = &'a std::sync::Arc<Photo>>) -> Vec<String> {
        let absent: HashSet<&str> = self.pack_profiles().chain(self.pack_aliases()).filter(|p| p.file.is_empty()).map(|p| p.id.as_str()).collect();
        if absent.is_empty() {
            return Vec::new();
        }
        let mut needed = BTreeSet::new();
        let mut note = |id: &str| {
            // (the prefix first: nearly every photo of a big library fails it without a hash)
            if is_pack_profile(id) && absent.contains(id) && !needed.contains(id) {
                needed.insert(id.to_string());
            }
        };
        for p in photos {
            note(&p.develop.profile.id);
            for v in &p.versions {
                note(&v.settings.profile.id);
            }
        }
        needed.into_iter().collect()
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
        for p in old.profiles.iter().chain(&old.aliases) {
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
    if p.get("aliases").is_some_and(|a| !a.is_null() && !a.as_object().is_some_and(|o| o.values().all(|v| v.is_string() || v.is_null()))) {
        return Err(bad(C, "aliases must map profile names to .cube paths (or null: the table is not on this device)"));
    }
    if p.get("absent").is_some_and(|a| !a.is_null() && !a.as_array().is_some_and(|a| a.iter().all(Value::is_string))) {
        return Err(bad(C, "absent must be a list of look names"));
    }
    let absent_names: Vec<&str> = p.get("absent").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).take(MAX_FILES).collect();
    let dir = str_param(p, "dir").map(Path::new);
    let listed: Vec<std::path::PathBuf> = p
        .get("files")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(std::path::PathBuf::from).collect())
        .unwrap_or_default();
    if dir.is_none() && listed.is_empty() && absent_names.is_empty() {
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

    // retired profiles: an old name drawn with another file's table, unless the pack still has a profile of that name
    let mut aliases = Vec::<LutProfile>::new();
    for (alias, file) in p.get("aliases").and_then(Value::as_object).into_iter().flatten().take(MAX_FILES) {
        let pid = profile_id(&id, alias);
        let Some(file) = file.as_str().filter(|f| !f.is_empty()) else {
            // (its table is not on this device: known, and asked for by `pack.needed`, like an absent look)
            if !profiles.iter().chain(&aliases).any(|x| x.id == pid) {
                aliases.push(LutProfile { id: pid, name: alias.clone(), group: group.clone(), file: String::new() });
            }
            continue;
        };
        let path = Path::new(file);
        let usable =
            path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cube")) && path.metadata().is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE);
        if !usable {
            failed.push(json!([alias, "the alias's table is not a readable .cube file"]));
        } else if !profiles.iter().chain(&aliases).any(|x| x.id == pid) {
            aliases.push(LutProfile { id: pid.clone(), name: alias.clone(), group: group.clone(), file: file.to_string() });
            registered.push((pid, path.to_path_buf()));
        }
    }

    // looks whose tables are not on this device: profiles with no file (a name that has a table here is not absent)
    let mut absent = 0;
    for name in absent_names {
        let pid = profile_id(&id, name);
        if name.trim().is_empty() || profiles.iter().chain(&aliases).any(|x| x.id == pid) {
            continue;
        }
        profiles.push(LutProfile { id: pid, name: name.to_string(), group: group.clone(), file: String::new() });
        absent += 1;
    }
    // one order whether a look's table is in the app, was fetched into another folder or is absent: by name
    profiles.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));

    // swap the pack in: the old version's tables and presets out, the new ones registered
    let before: Vec<Option<u64>> = registered.iter().map(|(pid, _)| lightcraft_pipeline::lut::stamp(pid)).collect();
    s.remove_pack(&id);
    for (pid, file) in &registered {
        lightcraft_pipeline::lut::register_file(pid, file);
    }
    // what came (no table before) and what changed (another table in its place: the server replaced it)
    let (mut arrived, mut changed) = (Vec::new(), Vec::new());
    for ((pid, _), was) in registered.iter().zip(before) {
        match was {
            None => arrived.push(pid.clone()),
            Some(old) if Some(old) != lightcraft_pipeline::lut::stamp(pid) => changed.push(pid.clone()),
            Some(_) => {}
        }
    }
    // the photos drawn without the table, or with the one it replaced, are drawn again (their render keys changed with
    // it: `MediaCache::camera_profile_salt`); a host's thumbnails of them follow (`MediaCache::take_rebuilt`)
    if !arrived.is_empty() || !changed.is_empty() {
        let redraw: Vec<PhotoId> = s
            .catalog
            .photos()
            .filter(|p| is_pack_profile(&p.develop.profile.id) && (arrived.contains(&p.develop.profile.id) || changed.contains(&p.develop.profile.id)))
            .map(|p| p.id)
            .collect();
        for photo in redraw {
            s.media.note_look_arrived(photo);
        }
    }
    let counts = json!({"id": id, "profiles": profiles.len(), "presets": presets.len(), "aliases": aliases.len(), "absent": absent, "arrived": arrived, "changed": changed, "failed": failed});
    s.packs.push(Pack { id, profiles, presets, aliases });
    s.insert_pack_presets();
    Ok(counts)
}

/// `pack.needed`: the absent looks the library's photos (or `ids`) are developed with.
fn needed(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("ids").is_some_and(|i| !i.is_null() && !i.is_array()) {
        return Err(bad("pack.needed", "ids must be a list of photo ids"));
    }
    let profiles = match p.get("ids").and_then(Value::as_array) {
        Some(ids) => s.looks_needed(ids.iter().filter_map(Value::as_u64).filter_map(|id| s.catalog.photo(PhotoId(id)))),
        None => s.looks_needed(s.catalog.photos()),
    };
    Ok(json!({"profiles": profiles}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "pack.set",
            "Set App Pack",
            [],
            None,
            "{id: pack name, dir?: folder and/or files?: [paths] of .cube files (profiles `lut:pack-<id>-<name>`, named by their TITLE) and .lcpreset files (built-in presets), group?: the profiles' group (default the folder's name, else the pack's), aliases?: {old profile name: .cube path | null} (looks the pack no longer offers: photos edited with one keep rendering with that table; not listed; null: its table is not on this device), absent?: [look name] (looks of the pack whose tables are not on this device: profiles with no file, listed with `absent: true`; a name with a table in this call is not absent)} → {id, profiles (absent ones included), presets, aliases, absent, arrived: [profile ids that have a table now and had none], changed: [profile ids whose table is another file, or the same file written again, since the last call], failed} — sets the app's own looks, replacing the pack's earlier version; nothing is saved or synced, so call it each time the app starts, and again when an absent look's table has been fetched or a table was replaced (the photos that use it are drawn again). Without `dir`, `files` and `absent` the pack is taken out.",
            always,
            set_pack
        ),
        cmd!(
            query "pack.needed",
            "Looks To Fetch",
            [],
            None,
            "{ids?: [photo]} → {profiles: [id]} — the absent pack looks (`pack.set` `absent`, and retired looks whose table is absent) that the library's photos, or `ids`, are developed with (their settings and their versions'), sorted: what an app fetches without being asked. Changes nothing",
            always,
            needed
        ),
    ]
}
