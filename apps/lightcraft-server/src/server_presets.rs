//! The presets an admin shares with every user of this server (`/admin` → Presets): imported from
//! preset files, renamed, moved between groups or deleted there, and the built-in presets and groups the
//! admin hides for everyone. Devices read it as the document `server-presets` (`GET
//! /api/docs/server-presets`; a pull lists its version beside the user's own documents) and never
//! write it: on a device they are presets like the built-in ones, which the user can favourite and hide.
//!
//! `<data>/server-presets.json`: a [`Doc`] whose items are the presets (`{id: "srv.…", name, group,
//! settings, disabled?}`) and at most one item `{id: "hidden", presets: [built-in id], groups: [built-in group]}`.
//! A `PUT` names the version it changed; a stale one gets `412` with the current document.
//!
//! A preset with `disabled: true` stays here (the admin page lists it, dimmed, and can enable it again) but
//! isn't served to devices ([`for_devices`]): to them it is as if it were deleted, until it is enabled. A
//! group is disabled when all its presets are; a preset imported into one starts disabled too. (A flag on the
//! preset, not an item of its own: an older admin app that saves the list keeps it.)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lightcraft_catalog::sync::proto::Doc;
use lightcraft_engine::presets::SERVER_PREFIX;
use serde_json::{Value, json};

/// The document's name, as devices ask for it.
pub const NAME: &str = "server-presets";
/// The item that lists the built-in presets and groups hidden for everyone.
pub const HIDDEN: &str = "hidden";
/// Presets kept at most (a device takes no more).
pub const MAX_PRESETS: usize = 20_000;
/// A name, group or id at most, in characters (as a device takes them).
const NAME_MAX: usize = 200;
const FILE: &str = "server-presets.json";

/// The shared presets, as last written.
pub struct ServerPresets {
    path: PathBuf,
    doc: Doc,
    /// The file couldn't be read: it is served empty and not overwritten until it is fixed or removed.
    damaged: Option<String>,
}

impl ServerPresets {
    /// Read `<data>/server-presets.json` (none: no shared presets yet).
    pub fn load(data: &Path) -> ServerPresets {
        let path = data.join(FILE);
        let empty = Doc { version: 0, items: Value::Array(Vec::new()) };
        let (doc, damaged) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Doc>(&bytes) {
                Ok(d) if d.items.is_array() => (d, None),
                Ok(_) => (empty, Some(format!("{}: `items` is not a list", path.display()))),
                Err(e) => (empty, Some(format!("{} is damaged: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (empty, None),
            Err(e) => (empty, Some(format!("{}: {e}", path.display()))),
        };
        if let Some(why) = &damaged {
            log::error!("shared presets: {why} (fix or remove it to change them from /admin)");
        }
        ServerPresets { path, doc, damaged }
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    pub fn version(&self) -> u64 {
        self.doc.version
    }

    /// Replace the items (checked: see [`check`]) as a change of version `version`. `Ok(Ok(version))`:
    /// written, the new version; `Ok(Err(current))`: someone else changed it first; `Err`: refused
    /// (`storage: …` when it couldn't be written).
    pub fn put(&mut self, version: u64, items: &[Value]) -> Result<Result<u64, Doc>, String> {
        if let Some(why) = &self.damaged {
            return Err(format!("storage: {why}"));
        }
        if version != self.doc.version {
            return Ok(Err(self.doc.clone()));
        }
        let next = Doc { version: self.doc.version.saturating_add(1), items: Value::Array(check(items)?) };
        let body = serde_json::to_vec(&next).map_err(|e| e.to_string())?;
        lightcraft_catalog::safe_file::write_atomic(&self.path, &body).map_err(|e| format!("storage: {}: {e}", self.path.display()))?;
        let v = next.version;
        self.doc = next;
        Ok(Ok(v))
    }

    /// Add the presets in preset files given by content (`[{name, data: base64}]`, what `preset.import`
    /// takes: `.lcpreset`, XMP, `.lrtemplate`, zips…), into group `group` when given. Presets already here
    /// (same name, group and settings) are skipped. → `{version, added, skipped, failed: [[file, why]]}`.
    pub fn import(&mut self, files: Vec<Value>, group: Option<&str>) -> Result<Value, String> {
        let mut s = lightcraft_engine::Session::new();
        let r = s.execute("preset.import", &json!({"files": files, "group": group})).map_err(|e| e.to_string())?;
        let mut items: Vec<Value> = self.doc.items.as_array().cloned().unwrap_or_default();
        let mut ids: HashSet<String> = items.iter().filter_map(|v| v["id"].as_str().map(str::to_string)).collect();
        let (mut added, mut skipped) = (0usize, r["skipped"].as_u64().unwrap_or(0) as usize);
        // (the groups whose presets are all disabled: what is imported into them starts disabled)
        let off: HashSet<String> = off_groups(&items);
        for p in s.presets.iter().filter(|p| !p.builtin) {
            if items.iter().any(|v| v["name"] == p.name.as_str() && v["group"] == p.group.as_str() && v["settings"] == p.settings) {
                skipped += 1;
                continue;
            }
            // (named by content, like a device's imports, so importing the same file twice adds it once)
            let base = format!("{SERVER_PREFIX}{}", p.id.trim_start_matches("user."));
            let (mut id, mut n) = (base.clone(), 2);
            while ids.contains(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            ids.insert(id.clone());
            let mut item = json!({"id": id, "name": p.name, "group": p.group, "settings": p.settings});
            if off.contains(&p.group) {
                item["disabled"] = json!(true);
            }
            items.push(item);
            added += 1;
        }
        let failed = r["failed"].clone();
        let version = if added == 0 {
            self.doc.version
        } else {
            match self.put(self.doc.version, &items)? {
                Ok(v) => v,
                Err(_) => return Err("the shared presets changed meanwhile: import again".into()),
            }
        };
        Ok(json!({"version": version, "added": added, "skipped": skipped, "failed": failed}))
    }
}

/// The presets of `doc` that devices get: the ones not disabled, and the `hidden` item.
pub fn for_devices(doc: &Doc) -> Doc {
    let items = doc.items.as_array().map(Vec::as_slice).unwrap_or_default();
    Doc { version: doc.version, items: Value::Array(items.iter().filter(|v| !is_disabled(v)).cloned().collect()) }
}

fn is_disabled(item: &Value) -> bool {
    item.get("disabled").and_then(Value::as_bool).unwrap_or(false)
}

/// The groups of `items` in which every preset is disabled.
fn off_groups(items: &[Value]) -> HashSet<String> {
    let mut groups: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    for v in items.iter().filter(|v| v["id"] != HIDDEN) {
        let Some(g) = v["group"].as_str() else { continue };
        let all = groups.entry(g).or_insert(true);
        *all = *all && is_disabled(v);
    }
    groups.into_iter().filter(|(_, all)| *all).map(|(g, _)| g.to_string()).collect()
}

fn short(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= NAME_MAX
}

/// The items as kept: each preset `{id: "srv.…", name, group, settings: {…}, disabled?: true}` (ids unique), and at most one
/// [`HIDDEN`] item of names. Anything else is refused, saying which item.
pub fn check(items: &[Value]) -> Result<Vec<Value>, String> {
    if items.len() > MAX_PRESETS {
        return Err(format!("at most {MAX_PRESETS} presets"));
    }
    let mut ids = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for (i, v) in items.iter().enumerate() {
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
        let id = text("id");
        if !ids.insert(id.to_string()) {
            return Err(format!("item {i}: the id `{id}` is used twice"));
        }
        if id == HIDDEN {
            let names = |k: &str| -> Result<Vec<String>, String> {
                let list = v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
                list.iter()
                    .map(|n| n.as_str().filter(|n| short(n)).map(str::to_string).ok_or_else(|| format!("`{HIDDEN}`: `{k}` must be names")))
                    .collect()
            };
            out.push(json!({"id": HIDDEN, "presets": names("presets")?, "groups": names("groups")?}));
            continue;
        }
        if !id.starts_with(SERVER_PREFIX) || !short(id) {
            return Err(format!("item {i}: a preset's id starts with `{SERVER_PREFIX}`"));
        }
        let (name, group) = (text("name").trim(), text("group").trim());
        if !short(name) || !short(group) {
            return Err(format!("item {i} (`{id}`): a name and a group of 1 to {NAME_MAX} characters"));
        }
        let Some(settings) = v.get("settings").filter(|s| s.is_object()) else {
            return Err(format!("item {i} (`{id}`): `settings` must be an object"));
        };
        let mut item = json!({"id": id, "name": name, "group": group, "settings": settings});
        match v.get("disabled") {
            None | Some(Value::Null) => {}
            Some(Value::Bool(true)) => item["disabled"] = json!(true),
            Some(Value::Bool(false)) => {}
            Some(_) => return Err(format!("item {i} (`{id}`): `disabled` must be true or false")),
        }
        out.push(item);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-server-presets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn checks_versions_and_keeps_what_it_wrote() {
        let d = temp("put");
        let mut p = ServerPresets::load(&d);
        assert_eq!(p.version(), 0);
        let items = vec![
            json!({"id": "srv.a", "name": " Warm ", "group": "Team", "settings": {"light": {"exposure": 0.5}}, "extra": 1}),
            json!({"id": "hidden", "presets": ["lc.moody"], "groups": ["B&W"]}),
        ];
        assert_eq!(p.put(0, &items).unwrap(), Ok(1));
        assert_eq!(p.doc().items[0], json!({"id": "srv.a", "name": "Warm", "group": "Team", "settings": {"light": {"exposure": 0.5}}}));
        assert!(matches!(p.put(0, &items).unwrap(), Err(Doc { version: 1, .. })), "a stale version gets the current one");
        let p = ServerPresets::load(&d);
        assert_eq!((p.version(), p.doc().items.as_array().unwrap().len()), (1, 2), "kept");
        for bad in [
            json!({"id": "user.a", "name": "A", "group": "G", "settings": {}}),
            json!({"id": "srv.a", "name": "", "group": "G", "settings": {}}),
            json!({"id": "srv.a", "name": "A", "group": "G", "settings": []}),
            json!({"id": "hidden", "presets": [3]}),
        ] {
            assert!(check(std::slice::from_ref(&bad)).is_err(), "{bad}");
        }
        assert!(
            check(&[
                json!({"id": "srv.a", "name": "A", "group": "G", "settings": {}}),
                json!({"id": "srv.a", "name": "B", "group": "G", "settings": {}})
            ])
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn disabled_presets_are_kept_but_not_served_to_devices() {
        let d = temp("disabled");
        let mut p = ServerPresets::load(&d);
        let items = vec![
            json!({"id": "srv.a", "name": "A", "group": "Team", "settings": {}, "disabled": true}),
            json!({"id": "srv.b", "name": "B", "group": "Team", "settings": {}, "disabled": false}),
            json!({"id": "srv.c", "name": "C", "group": "Old", "settings": {}, "disabled": true}),
            json!({"id": "hidden", "presets": ["lc.moody"], "groups": []}),
        ];
        assert_eq!(p.put(0, &items).unwrap(), Ok(1));
        let kept = p.doc().items.as_array().unwrap();
        assert_eq!(kept[0]["disabled"], true, "kept for the admin");
        assert!(kept[1].get("disabled").is_none(), "false is the same as absent");
        let served = for_devices(p.doc());
        let ids: Vec<&str> = served.items.as_array().unwrap().iter().filter_map(|v| v["id"].as_str()).collect();
        assert_eq!((ids, served.version), (vec!["srv.b", "hidden"], 1), "devices get what is enabled, and the hidden built-ins");
        assert!(check(&[json!({"id": "srv.a", "name": "A", "group": "G", "settings": {}, "disabled": "yes"})]).is_err());
        // a preset imported into a group that is all disabled starts disabled; into one that is not, enabled
        let file = |group: &str| {
            let f = json!({"format": "lightcraft.preset", "version": 1, "presets": [{"id": "x", "name": group, "group": group, "settings": {"light": {"exposure": 0.7}}}]});
            lightcraft_engine::inline_files::to_param([("F.lcpreset", f.to_string().as_bytes())]).as_array().unwrap().clone()
        };
        for (group, off) in [("Old", true), ("Team", false), ("New", false)] {
            p.import(file(group), None).unwrap();
            let it = p.doc().items.as_array().unwrap().iter().find(|v| v["name"] == group).unwrap().clone();
            assert_eq!(it["disabled"] == true, off, "{group}: {it}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_file_is_not_overwritten() {
        let d = temp("damaged");
        std::fs::write(d.join(FILE), b"{not json").unwrap();
        let mut p = ServerPresets::load(&d);
        assert_eq!(p.version(), 0);
        assert!(p.put(0, &[]).unwrap_err().starts_with("storage:"));
        assert_eq!(std::fs::read(d.join(FILE)).unwrap(), b"{not json");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn imports_preset_files_once() {
        let d = temp("import");
        let mut p = ServerPresets::load(&d);
        // our own .lcpreset format, written for this test
        let file = json!({"format": "lightcraft.preset", "version": 1, "presets": [{"id": "x", "name": "Bright", "group": "Team", "settings": {"light": {"exposure": 0.7}}}]});
        let files = lightcraft_engine::inline_files::to_param([("Team.lcpreset", file.to_string().as_bytes())]);
        let r = p.import(files.as_array().unwrap().clone(), None).unwrap();
        assert_eq!((r["added"].as_u64(), r["version"].as_u64()), (Some(1), Some(1)), "{r}");
        let item = &p.doc().items[0];
        assert!(item["id"].as_str().unwrap().starts_with("srv.imp."), "{item}");
        assert_eq!((item["name"].as_str(), item["group"].as_str()), (Some("Bright"), Some("Team")));
        let r = p.import(files.as_array().unwrap().clone(), Some("Other")).unwrap();
        assert_eq!(r["added"], 1, "a group of its own is another preset: {r}");
        let r = p.import(files.as_array().unwrap().clone(), None).unwrap();
        assert_eq!((r["added"].as_u64(), r["skipped"].as_u64(), r["version"].as_u64()), (Some(0), Some(1), Some(2)), "{r}");
        let bad = lightcraft_engine::inline_files::to_param([("Broken.lcpreset", b"{nope".as_slice())]);
        let r = p.import(bad.as_array().unwrap().clone(), None).unwrap();
        assert_eq!(r["failed"].as_array().map(Vec::len), Some(1), "{r}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
