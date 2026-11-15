//! Built-in presets and profiles (all values are our own; no third-party preset content), and
//! preset files: our `.lcpreset` JSON (import + export, groups preserved) and XMP presets
//! (`crs:` fields, read only — see [`crate::crs`]).

use std::path::Path;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Film, FilmFrame, FilmKind, FilmLayer, FrameTone, Preset};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Session;

/// File extension of LightCraft preset files.
pub const LCPRESET_EXT: &str = "lcpreset";
/// The `format` tag of a `.lcpreset` file.
pub const LCPRESET_FORMAT: &str = "lightcraft.preset";

/// A `.lcpreset` file: one or more presets with their groups.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetFile {
    pub format: String,
    pub version: u32,
    pub presets: Vec<Preset>,
}

/// The session's presets (built-in, then the user's) with a generation that changes on every
/// mutable access, so saving and the UI's caches skip their O(presets) work while the list is
/// untouched: a Lightroom collection imported whole is 10 000+ presets and ~40 MB of JSON.
///
/// Reads go through `Deref` (`iter`, `len`, `find`…) and keep the generation; `push`, `iter_mut`,
/// `remove`… go through `DerefMut` and bump it, whether or not anything is then changed.
/// Generations are unique across all lists in the process.
#[derive(Clone, Debug)]
pub struct PresetList {
    items: Vec<Preset>,
    generation: u64,
}

static PRESET_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    PRESET_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl PresetList {
    pub fn new(items: Vec<Preset>) -> PresetList {
        PresetList { items, generation: next_generation() }
    }

    /// Changes whenever the list may have changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Default for PresetList {
    fn default() -> PresetList {
        PresetList::new(Vec::new())
    }
}

impl std::ops::Deref for PresetList {
    type Target = Vec<Preset>;
    fn deref(&self) -> &Vec<Preset> {
        &self.items
    }
}

impl std::ops::DerefMut for PresetList {
    fn deref_mut(&mut self) -> &mut Vec<Preset> {
        self.generation = next_generation();
        &mut self.items
    }
}

impl<'a> IntoIterator for &'a PresetList {
    type Item = &'a Preset;
    type IntoIter = std::slice::Iter<'a, Preset>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<'a> IntoIterator for &'a mut PresetList {
    type Item = &'a mut Preset;
    type IntoIter = std::slice::IterMut<'a, Preset>;
    fn into_iter(self) -> Self::IntoIter {
        self.generation = next_generation();
        self.items.iter_mut()
    }
}

/// Lower-case ASCII slug for ids (`"Warm & Soft"` → `warm-soft`).
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "preset".into() } else { out }
}

/// Serialise presets as a `.lcpreset` file (favourite/built-in flags are not exported).
pub fn to_lcpreset(presets: &[Preset]) -> String {
    let presets = presets.iter().map(|p| Preset { favorite: false, builtin: false, ..p.clone() }).collect();
    serde_json::to_string_pretty(&PresetFile { format: LCPRESET_FORMAT.into(), version: 1, presets }).unwrap_or_default()
}

/// Read a preset file by name: `.lcpreset` (a [`PresetFile`], a bare preset object or an array of
/// presets) or `.xmp` (a `crs:` preset).
pub fn parse_preset_file(name: &str, bytes: &[u8]) -> Result<Vec<Preset>, String> {
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Preset".into());
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    if ext == "xmp" || (ext != LCPRESET_EXT && text.trim_start().starts_with('<')) {
        return crate::crs::preset_from_xmp(text, &stem).map(|p| vec![p]).ok_or_else(|| "no develop settings in this XMP file".into());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a preset file: {e}"))?;
    let list = match &v {
        Value::Object(o) if o.contains_key("presets") => {
            let f: PresetFile = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            if f.format != LCPRESET_FORMAT {
                return Err(format!("unknown preset format `{}`", f.format));
            }
            f.presets
        }
        Value::Array(_) => serde_json::from_value(v).map_err(|e| e.to_string())?,
        _ => vec![serde_json::from_value(v).map_err(|e| e.to_string())?],
    };
    let mut out = Vec::new();
    for mut p in list {
        if !p.settings.is_object() {
            return Err(format!("preset `{}` has no settings object", p.name));
        }
        p.builtin = false;
        p.favorite = false;
        if p.name.trim().is_empty() {
            p.name = stem.clone();
        }
        if p.group.trim().is_empty() {
            p.group = "Imported Presets".into();
        }
        out.push(p);
    }
    Ok(out)
}

/// Expand files/folders (recursively, bounded: see [`crate::walk`]) into preset files
/// (`.lcpreset`, `.xmp`).
pub fn expand_preset_paths(paths: &[String]) -> Vec<String> {
    let preset_file = |p: &Path| {
        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        ext == LCPRESET_EXT || ["xmp", "lrtemplate", "zip", "lmp", "mplumpack", crate::cmd::mask_effect_files::EFFECT_EXT].contains(&ext.as_str())
    };
    let mut out = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            let w = crate::walk::files_in(path, None, crate::walk::Limits::default(), preset_file);
            out.extend(w.files.into_iter().map(|f| f.to_string_lossy().to_string()));
        } else {
            out.push(p.clone());
        }
    }
    out
}

/// `v` as JSON text with every object's keys sorted, however it was built (at most `depth`
/// levels; deeper values are written as `null`).
fn canonical_json(v: &Value, depth: usize, out: &mut String) {
    match v {
        _ if depth == 0 => out.push_str("null"),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical_json(o.get(k).unwrap_or(&Value::Null), depth - 1, out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(x, depth - 1, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The id an imported preset gets: named by its content (name, group and settings), so the same
/// preset imported on two synced devices is one preset there, and two different presets that share
/// a name stay two (an id taken from the file, or from the name, would make one of them replace
/// the other when the devices sync). Stable across builds and platforms (FNV-1a over canonical
/// JSON).
pub fn imported_preset_id(p: &Preset) -> String {
    let mut text = String::new();
    canonical_json(&json!([p.group, p.name, p.settings]), 64, &mut text);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("user.imp.{h:016x}")
}

/// Ids of the presets a sync server shares with all its users (its `server-presets` document) start
/// with this. They are kept like the built-in ones: `builtin`, never sent back, not editable here.
pub const SERVER_PREFIX: &str = "srv.";

/// `p` came from the sync server's shared presets.
pub fn is_server_preset(p: &Preset) -> bool {
    p.builtin && p.id.starts_with(SERVER_PREFIX)
}

/// Presets and preset groups left out of the Presets panels: the ones the user hid (`preset.hide`,
/// `preset.hideGroup`, synced), or the built-in ones a sync server's admin hid for all its users.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Hidden {
    /// Preset ids.
    pub presets: Vec<String>,
    /// Group names.
    pub groups: Vec<String>,
}

impl Hidden {
    /// `p` is hidden, itself or by its group.
    pub fn hides(&self, p: &Preset) -> bool {
        self.presets.contains(&p.id) || self.groups.contains(&p.group)
    }
}

impl Session {
    /// Make the sync server's shared presets `server` (ids [`SERVER_PREFIX`]…) and hide the built-in
    /// presets and groups it hides for all its users: the list is then the built-in presets, the server's,
    /// then the user's. Favourites stay favourites (a built-in one the server hid and shows again doesn't).
    pub fn set_server_presets(&mut self, server: Vec<Preset>, hidden: Hidden) {
        self.fold_pack_favorites();
        let favorites: std::collections::HashSet<String> = self.presets.iter().filter(|p| p.builtin && p.favorite).map(|p| p.id.clone()).collect();
        let user: Vec<Preset> = self.presets.iter().filter(|p| !p.builtin).cloned().collect();
        // (what the server hides is the built-in presets it knows; an app pack's are put back after, and never hidden)
        let mut list: Vec<Preset> = builtin().into_iter().filter(|p| !hidden.hides(p)).collect();
        list.extend(server.into_iter().filter(|p| p.id.starts_with(SERVER_PREFIX)).map(|p| Preset { builtin: true, ..p }));
        for p in &mut list {
            p.favorite = p.favorite || favorites.contains(&p.id);
        }
        list.extend(user);
        *self.presets = list;
        self.server_hidden = hidden;
        self.insert_pack_presets();
    }

    /// Rename preset group `group` (of the user's presets) to `name`; a group already called `name` takes
    /// them in. Its place in the order and its being hidden go with it.
    pub fn rename_preset_group(&mut self, group: &str, name: &str) -> Result<Vec<String>, String> {
        let fixed = self.presets.iter().any(|p| p.group == group && p.builtin);
        if fixed {
            return Err("built-in and server preset groups can't be renamed".into());
        }
        if !self.presets.iter().any(|p| p.group == group) {
            return Err(format!("unknown preset group `{group}`"));
        }
        if group == name {
            return Ok(self.preset_groups());
        }
        for p in self.presets.iter_mut().filter(|p| p.group == group) {
            p.group = name.to_string();
        }
        for list in [&mut self.preset_group_order, &mut self.hidden_presets.groups] {
            let taken = list.iter().any(|g| g == name);
            if taken {
                list.retain(|g| g != group);
            } else {
                list.iter_mut().filter(|g| g.as_str() == group).for_each(|g| *g = name.to_string());
            }
        }
        Ok(self.preset_groups())
    }

    /// Delete the user's presets in group `group` (built-in and server presets stay) → how many.
    pub fn delete_preset_group(&mut self, group: &str) -> Result<usize, String> {
        let gone: Vec<String> = self.presets.iter().filter(|p| p.group == group && !p.builtin).map(|p| p.id.clone()).collect();
        if gone.is_empty() {
            let fixed = self.presets.iter().any(|p| p.group == group);
            return Err(if fixed { "built-in and server presets can't be deleted".into() } else { format!("unknown preset group `{group}`") });
        }
        self.presets.retain(|p| p.builtin || p.group != group);
        self.hidden_presets.presets.retain(|id| !gone.contains(id));
        if !self.presets.iter().any(|p| p.group == group) {
            self.hidden_presets.groups.retain(|g| g != group);
            self.preset_group_order.retain(|g| g != group);
        }
        Ok(gone.len())
    }

    /// The preset groups in the order the Presets panel shows them: the user's order
    /// ([`Session::preset_group_order`]), then the groups it doesn't list, by name.
    pub fn preset_groups(&self) -> Vec<String> {
        let mut rest: Vec<&str> = self.presets.iter().map(|p| p.group.as_str()).collect();
        rest.sort_unstable();
        rest.dedup();
        let mut out: Vec<String> = Vec::new();
        for g in self.preset_group_order.iter().filter(|g| rest.contains(&g.as_str())) {
            if !out.contains(g) {
                out.push(g.clone());
            }
        }
        out.extend(rest.into_iter().filter(|g| !self.preset_group_order.iter().any(|o| o == g)).map(str::to_string));
        out
    }

    /// Move preset group `group` to position `to` of [`Session::preset_groups`] (clamped to the end).
    pub fn move_preset_group(&mut self, group: &str, to: usize) -> Result<Vec<String>, String> {
        let mut order = self.preset_groups();
        let from = order.iter().position(|g| g == group).ok_or_else(|| format!("unknown preset group `{group}`"))?;
        let g = order.remove(from);
        order.insert(to.min(order.len()), g);
        self.preset_group_order = order.clone();
        Ok(order)
    }

    /// Whether a preset with the same name, group and settings is already here (what
    /// [`Session::add_presets`] skips).
    pub fn has_identical_preset(&self, p: &Preset) -> bool {
        self.presets.iter().any(|q| q.name == p.name && q.group == p.group && q.settings == p.settings)
    }

    /// The settings after applying `preset` to photo `id` at `amount`. The settings the preset
    /// names are computed on the photo's import state ([`lightcraft_catalog::Photo::import_defaults`])
    /// when a default preset recorded one (`import_look`); a photo without one (its settings came
    /// from an XMP sidecar or a catalog import, which record none) composes them on its current
    /// settings instead, since the camera defaults are not its import state. Either way, the
    /// settings the preset does not name keep the photo's values (issue #2159). The `preset.apply`
    /// command and the UI's hover previews share this, so hovering shows exactly what a click commits.
    pub fn preset_applied(&self, id: PhotoId, preset: &Preset, amount: f64) -> Option<DevelopSettings> {
        let current = self.develop_of(id)?;
        let photo = self.catalog.photo(id)?;
        // The original is read only below 100 %: the panel asks for every visible row at 100 % on
        // every frame, and then neither the import state nor a copy of the photo's settings is built.
        let look;
        let original: &DevelopSettings = match &photo.import_look {
            Some(_) if (amount - 1.0).abs() >= 1e-9 => {
                look = photo.import_defaults();
                &look
            }
            _ => &current,
        };
        Some(preset.apply_on(&current, original, amount))
    }

    /// Add imported presets. Identical presets (same name, group and settings) already present are
    /// skipped; the others get the id [`imported_preset_id`] names them by (with `-2`, `-3`… in the
    /// rare case it is taken, by a preset edited since it was imported). Returns the ids added.
    pub fn add_presets(&mut self, presets: Vec<Preset>) -> Vec<String> {
        self.add_presets_each(presets).into_iter().flatten().collect()
    }

    /// [`Self::add_presets`] reporting per preset, in order: its id, or `None` when it was skipped
    /// as already present. One call indexes the library once, so an importer adds a whole file's
    /// presets through it rather than one at a time.
    pub fn add_presets_each(&mut self, presets: Vec<Preset>) -> Vec<Option<String>> {
        use std::collections::{HashMap, HashSet};
        // indexed: a collection of 10 000+ presets was quadratic (minutes) to import
        let mut ids: HashSet<String> = self.presets.iter().map(|q| q.id.clone()).collect();
        // (name, group, settings): a preset is looked up rather than compared with every namesake.
        // Every number is keyed as an f64 with -0.0 folded into 0.0: `0` and `0.0` are unequal
        // `Value`s with equal hashes, so a crafted file of them would collide in one bucket.
        // shortcut: integers past 2^53 lose precision in the key, so two such presets are one
        let key_of = |p: &Preset| (p.name.clone(), p.group.clone(), canonical_numbers(&p.settings));
        let mut present: HashSet<(String, String, serde_json::Value)> = self.presets.iter().map(key_of).collect();
        // the next suffix to try per generated id, so a batch of clashes doesn't probe from 2 each time
        let mut next_suffix: HashMap<String, u32> = HashMap::new();
        let mut added = Vec::with_capacity(presets.len());
        for mut p in presets {
            let key = key_of(&p);
            if present.contains(&key) {
                added.push(None);
                continue;
            }
            let base = imported_preset_id(&p);
            let mut id = base.clone();
            let n = next_suffix.entry(base.clone()).or_insert(2);
            while ids.contains(&id) {
                id = format!("{base}-{n}");
                *n += 1;
            }
            p.id = id;
            p.builtin = false;
            ids.insert(p.id.clone());
            present.insert(key);
            added.push(Some(p.id.clone()));
            self.presets.push(p);
        }
        added
    }
}

/// `v` with every number replaced by its f64 (`-0.0` as `0.0`), so equal numbers hash and compare alike.
fn canonical_numbers(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Number(n) => n.as_f64().and_then(|f| serde_json::Number::from_f64(f + 0.0)).map_or_else(|| v.clone(), Value::Number),
        Value::Array(a) => Value::Array(a.iter().map(canonical_numbers).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), canonical_numbers(x))).collect()),
        _ => v.clone(),
    }
}

fn p(group: &str, id: &str, name: &str, settings: serde_json::Value) -> Preset {
    Preset { id: format!("lc.{id}"), name: name.into(), group: group.into(), settings, favorite: false, builtin: true }
}

/// A film effects preset's `film` group (`docs/film-effects.md`): the frame and the marks, bottom to top. Each photo it
/// lands on draws its own marks from these seeds (`film.vary`).
fn film(frame: FilmFrame, frame_tone: FrameTone, marks: Vec<FilmLayer>) -> Value {
    let layers = marks.into_iter().zip(1..).map(|(l, id)| FilmLayer { id, ..l }).collect();
    serde_json::to_value(Film { frame, frame_tone, layers, ..Film::default() }).unwrap_or_default()
}

/// The Film Effects presets: frames, leaks, burns, dust and scratches, with the grain and tone that go with them.
fn film_effects() -> Vec<Preset> {
    use FilmKind::{Burn, Dust, Leak, Scratches, Streak};
    let mark = |kind, seed| FilmLayer::new(0, kind, seed);
    let grain = |amount: f64, size: f64, roughness: f64| json!({"amount": amount, "size": size, "roughness": roughness, "style": "film"});
    let g = "Film Effects";
    vec![
        p(
            g,
            "fx-light-leak",
            "Light Leak",
            json!({"film": film(FilmFrame::None, FrameTone::Dark, vec![FilmLayer { angle: 180.0, amount: 65.0, size: 50.0, color: 10.0, ..mark(Leak, 11) }]), "grain": grain(18.0, 30.0, 55.0)}),
        ),
        p(
            g,
            "fx-first-frame",
            "First Frame",
            json!({"film": film(FilmFrame::None, FrameTone::Dark, vec![
                FilmLayer { angle: 180.0, size: 20.0, ..mark(Burn, 21) },
                FilmLayer { angle: 180.0, amount: 45.0, size: 60.0, color: 30.0, ..mark(Leak, 22) },
            ]), "grain": grain(22.0, 30.0, 55.0), "curve": {"shadows": 15.0}}),
        ),
        p(
            g,
            "fx-dusty-negative",
            "Dusty Negative",
            json!({"film": film(FilmFrame::Filed, FrameTone::Dark, vec![
                FilmLayer { amount: 45.0, ..mark(Dust, 31) },
                FilmLayer { amount: 25.0, angle: 90.0, ..mark(Scratches, 32) },
            ]), "grain": grain(28.0, 35.0, 60.0), "light": {"contrast": 8.0}}),
        ),
        p(
            g,
            "fx-slide",
            "35mm Slide",
            json!({"film": film(FilmFrame::Strip, FrameTone::Dark, vec![FilmLayer { amount: 18.0, size: 30.0, tone: -100.0, fibres: 10.0, ..mark(Dust, 41) }]), "light": {"contrast": 12.0}, "color": {"saturation": 8.0}, "grain": grain(12.0, 20.0, 45.0)}),
        ),
        p(
            g,
            "fx-negative-strip",
            "Negative Strip",
            json!({"film": film(FilmFrame::Strip, FrameTone::Light, vec![
                FilmLayer { amount: 30.0, ..mark(Dust, 51) },
                FilmLayer { amount: 35.0, size: 55.0, angle: 45.0, color: -20.0, ..mark(Leak, 52) },
            ]), "grain": grain(24.0, 30.0, 55.0), "curve": {"shadows": 12.0}}),
        ),
        p(
            g,
            "fx-instant",
            "Instant Print",
            json!({"film": film(FilmFrame::Instant, FrameTone::Light, vec![FilmLayer { amount: 30.0, size: 60.0, angle: 45.0, color: 40.0, ..mark(Leak, 61) }]), "curve": {"shadows": 28.0, "highlights": -12.0}, "color": {"saturation": -10.0}, "light": {"contrast": -8.0}, "grain": grain(14.0, 25.0, 50.0)}),
        ),
        p(
            g,
            "fx-projector",
            "Old Projector",
            json!({"film": film(FilmFrame::None, FrameTone::Dark, vec![
                FilmLayer { amount: 35.0, size: 55.0, tone: -100.0, fibres: 45.0, ..mark(Dust, 71) },
                FilmLayer { amount: 45.0, angle: 90.0, tone: -100.0, ..mark(Scratches, 72) },
                FilmLayer { amount: 25.0, size: 35.0, angle: 90.0, offset: 55.0, color: 60.0, ..mark(Streak, 73) },
            ]), "vignette": {"amount": -28.0}, "color": {"saturation": -15.0}, "grain": grain(32.0, 40.0, 65.0)}),
        ),
    ]
}

pub fn builtin() -> Vec<Preset> {
    let mut list = vec![
        p(
            "Color",
            "warm-glow",
            "Warm Glow",
            json!({"treatment": "color", "wb": {"mode": "custom", "temp": 7200.0, "tint": 8.0}, "light": {"contrast": 8.0, "shadows": 12.0}, "color": {"vibrance": 18.0}}),
        ),
        p(
            "Color",
            "cool-morning",
            "Cool Morning",
            json!({"treatment": "color", "wb": {"mode": "custom", "temp": 5600.0, "tint": -4.0}, "light": {"highlights": -20.0}, "color": {"vibrance": 10.0}, "grading": {"shadows": {"hue": 215.0, "sat": 12.0, "lum": 0.0}}}),
        ),
        p(
            "Color",
            "teal-orange",
            "Teal & Orange",
            json!({"treatment": "color", "grading": {"shadows": {"hue": 195.0, "sat": 28.0, "lum": 0.0}, "highlights": {"hue": 35.0, "sat": 22.0, "lum": 0.0}, "balance": 10.0}, "color": {"vibrance": 12.0}, "light": {"contrast": 14.0}}),
        ),
        p(
            "Color",
            "vivid-pop",
            "Vivid Pop",
            json!({"treatment": "color", "color": {"vibrance": 40.0, "saturation": 8.0}, "light": {"contrast": 18.0, "whites": 10.0, "blacks": -10.0}, "effects": {"clarity": 12.0}}),
        ),
        p(
            "Color",
            "soft-pastel",
            "Soft Pastel",
            json!({"treatment": "color", "light": {"contrast": -25.0, "highlights": -30.0, "shadows": 35.0, "blacks": 20.0}, "color": {"saturation": -18.0, "vibrance": 10.0}, "curve": {"shadows": 25.0}}),
        ),
        p(
            "Film",
            "faded-matte",
            "Faded Matte",
            json!({"treatment": "color", "curve": {"shadows": 40.0, "highlights": -15.0}, "light": {"contrast": -10.0}, "color": {"saturation": -12.0}, "grain": {"amount": 18.0, "size": 25.0, "roughness": 50.0}}),
        ),
        p(
            "Film",
            "warm-film",
            "Warm Film",
            json!({"treatment": "color", "wb": {"mode": "custom", "temp": 6900.0, "tint": 5.0}, "curve": {"shadows": 22.0}, "grading": {"highlights": {"hue": 45.0, "sat": 15.0, "lum": 0.0}}, "grain": {"amount": 22.0, "size": 30.0, "roughness": 55.0}, "vignette": {"amount": -12.0}}),
        ),
        p(
            "Film",
            "muted-film",
            "Muted Film",
            json!({"treatment": "color", "color": {"saturation": -25.0, "vibrance": 5.0}, "light": {"contrast": 10.0}, "curve": {"shadows": 18.0}, "grain": {"amount": 15.0}}),
        ),
        p(
            "B&W",
            "bw-high-contrast",
            "High Contrast B&W",
            json!({"treatment": "bw", "light": {"contrast": 45.0, "whites": 20.0, "blacks": -25.0}, "effects": {"clarity": 20.0}, "bw_mix": {"blue": -30.0, "red": 15.0}}),
        ),
        p("B&W", "bw-soft", "Soft B&W", json!({"treatment": "bw", "light": {"contrast": -15.0, "shadows": 25.0}, "curve": {"shadows": 15.0}})),
        p(
            "B&W",
            "bw-selenium",
            "Selenium Tone",
            json!({"treatment": "bw", "light": {"contrast": 20.0}, "grading": {"shadows": {"hue": 285.0, "sat": 18.0, "lum": 0.0}, "highlights": {"hue": 40.0, "sat": 10.0, "lum": 0.0}}}),
        ),
        p(
            "Landscape",
            "crisp-landscape",
            "Crisp Landscape",
            json!({"treatment": "color", "light": {"highlights": -45.0, "shadows": 30.0, "contrast": 12.0}, "effects": {"clarity": 22.0, "dehaze": 12.0, "texture": 15.0}, "color": {"vibrance": 25.0}}),
        ),
        p(
            "Landscape",
            "golden-hour",
            "Golden Hour",
            json!({"treatment": "color", "wb": {"mode": "custom", "temp": 7600.0, "tint": 12.0}, "light": {"highlights": -35.0, "shadows": 20.0}, "grading": {"highlights": {"hue": 38.0, "sat": 25.0, "lum": 5.0}}, "vignette": {"amount": -15.0}}),
        ),
        p(
            "Landscape",
            "blue-hour",
            "Blue Hour Boost",
            json!({"treatment": "color", "wb": {"mode": "custom", "temp": 5200.0, "tint": 6.0}, "light": {"shadows": 25.0, "exposure": 0.2}, "mixer": {"blue": {"hue": 0.0, "sat": 25.0, "lum": 0.0}, "purple": {"hue": 0.0, "sat": 15.0, "lum": 0.0}}}),
        ),
        p(
            "Portrait",
            "soft-skin",
            "Soft Skin",
            json!({"treatment": "color", "effects": {"texture": -25.0, "clarity": -10.0}, "light": {"contrast": -5.0, "shadows": 15.0}, "mixer": {"orange": {"hue": 0.0, "sat": -8.0, "lum": 10.0}}}),
        ),
        p(
            "Portrait",
            "bright-airy",
            "Bright & Airy",
            json!({"treatment": "color", "light": {"exposure": 0.45, "contrast": -15.0, "highlights": -40.0, "shadows": 40.0, "whites": 15.0}, "color": {"vibrance": 8.0, "saturation": -8.0}}),
        ),
        p(
            "Style",
            "moody",
            "Moody",
            json!({"treatment": "color", "light": {"exposure": -0.3, "contrast": 20.0, "highlights": -30.0, "blacks": -15.0}, "color": {"saturation": -20.0}, "grading": {"shadows": {"hue": 200.0, "sat": 15.0, "lum": -5.0}}, "vignette": {"amount": -30.0}}),
        ),
        p(
            "Style",
            "cinematic",
            "Cinematic",
            json!({"treatment": "color", "curve": {"shadows": 20.0, "highlights": -10.0}, "grading": {"shadows": {"hue": 190.0, "sat": 25.0, "lum": 0.0}, "highlights": {"hue": 30.0, "sat": 18.0, "lum": 0.0}}, "light": {"contrast": 15.0}, "vignette": {"amount": -20.0}}),
        ),
        // ---- Portrait
        p(
            "Portrait",
            "moody-portrait",
            "Moody Portrait",
            json!({"treatment": "color", "light": {"exposure": -0.25, "contrast": 20.0, "highlights": -30.0, "blacks": -12.0}, "color": {"saturation": -15.0}, "vignette": {"amount": -25.0, "midpoint": 40.0}, "mixer": {"orange": {"sat": 6.0}}}),
        ),
        p(
            "Portrait",
            "golden-glow",
            "Golden Glow",
            json!({"treatment": "color", "grading": {"highlights": {"hue": 40.0, "sat": 18.0, "lum": 0.0}, "midtones": {"hue": 35.0, "sat": 8.0, "lum": 0.0}}, "light": {"shadows": 15.0}, "effects": {"clarity": -8.0}}),
        ),
        p(
            "Portrait",
            "clean-studio",
            "Clean Studio",
            json!({"treatment": "color", "light": {"contrast": 10.0, "whites": 15.0, "blacks": -8.0}, "effects": {"texture": -10.0}, "color": {"vibrance": 5.0}}),
        ),
        // ---- Landscape
        p(
            "Landscape",
            "crisp-vista",
            "Crisp Vista",
            json!({"treatment": "color", "effects": {"dehaze": 15.0, "clarity": 18.0, "texture": 12.0}, "light": {"contrast": 12.0, "highlights": -25.0, "shadows": 18.0}, "color": {"vibrance": 20.0}}),
        ),
        p(
            "Landscape",
            "deep-sky",
            "Deep Sky",
            json!({"treatment": "color", "mixer": {"blue": {"lum": -25.0, "sat": 15.0}, "aqua": {"lum": -10.0}}, "light": {"highlights": -30.0}, "effects": {"dehaze": 10.0}}),
        ),
        p(
            "Landscape",
            "lush-greens",
            "Lush Greens",
            json!({"treatment": "color", "mixer": {"green": {"hue": 12.0, "sat": 18.0, "lum": 8.0}, "yellow": {"hue": 10.0, "sat": 10.0}}, "color": {"vibrance": 12.0}, "effects": {"clarity": 8.0}}),
        ),
        p(
            "Landscape",
            "desert-warmth",
            "Desert Warmth",
            json!({"treatment": "color", "mixer": {"orange": {"sat": 15.0, "lum": 5.0}, "yellow": {"hue": -8.0, "sat": 10.0}, "blue": {"sat": -10.0}}, "grading": {"highlights": {"hue": 38.0, "sat": 12.0, "lum": 0.0}}, "light": {"contrast": 10.0}}),
        ),
        p(
            "Landscape",
            "misty-morning",
            "Misty Morning",
            json!({"treatment": "color", "effects": {"dehaze": -18.0, "clarity": -12.0}, "light": {"contrast": -18.0, "highlights": -10.0}, "grading": {"shadows": {"hue": 210.0, "sat": 10.0, "lum": 0.0}}, "color": {"saturation": -10.0}}),
        ),
        // ---- Urban
        p(
            "Urban",
            "gritty-street",
            "Gritty Street",
            json!({"treatment": "color", "effects": {"clarity": 35.0, "texture": 20.0}, "light": {"contrast": 25.0, "blacks": -15.0}, "color": {"saturation": -30.0}, "vignette": {"amount": -18.0}}),
        ),
        p(
            "Urban",
            "neon-night",
            "Neon Night",
            json!({"treatment": "color", "mixer": {"magenta": {"sat": 20.0}, "purple": {"sat": 18.0}, "blue": {"sat": 12.0}, "aqua": {"sat": 15.0}}, "light": {"contrast": 18.0, "blacks": -12.0}, "color": {"vibrance": 15.0}, "grading": {"shadows": {"hue": 250.0, "sat": 15.0, "lum": 0.0}}}),
        ),
        p(
            "Urban",
            "concrete-cool",
            "Concrete Cool",
            json!({"treatment": "color", "grading": {"shadows": {"hue": 205.0, "sat": 14.0, "lum": 0.0}, "highlights": {"hue": 200.0, "sat": 6.0, "lum": 0.0}}, "color": {"saturation": -20.0}, "light": {"contrast": 12.0}, "effects": {"clarity": 15.0}}),
        ),
        p(
            "Urban",
            "faded-urban",
            "Faded Urban",
            json!({"treatment": "color", "curve": {"shadows": 30.0}, "color": {"saturation": -22.0}, "light": {"contrast": -8.0}, "grain": {"amount": 12.0, "size": 25.0, "roughness": 45.0}}),
        ),
        // ---- Food
        p(
            "Food",
            "fresh-bright",
            "Fresh & Bright",
            json!({"treatment": "color", "light": {"exposure": 0.25, "shadows": 20.0, "whites": 10.0}, "color": {"vibrance": 22.0}, "effects": {"texture": 15.0}}),
        ),
        p(
            "Food",
            "warm-table",
            "Warm Table",
            json!({"treatment": "color", "grading": {"midtones": {"hue": 32.0, "sat": 10.0, "lum": 0.0}}, "mixer": {"orange": {"sat": 10.0}, "red": {"sat": 8.0}}, "light": {"contrast": 8.0}}),
        ),
        p(
            "Food",
            "dark-moody-food",
            "Dark & Moody",
            json!({"treatment": "color", "light": {"exposure": -0.3, "contrast": 22.0, "highlights": -20.0, "blacks": -15.0}, "effects": {"texture": 18.0}, "vignette": {"amount": -22.0}}),
        ),
        // ---- Seasons
        p(
            "Seasons",
            "autumn-gold",
            "Autumn Gold",
            json!({"treatment": "color", "mixer": {"green": {"hue": -30.0, "sat": -10.0}, "yellow": {"hue": -15.0, "sat": 15.0}, "orange": {"sat": 18.0}}, "grading": {"highlights": {"hue": 40.0, "sat": 10.0, "lum": 0.0}}}),
        ),
        p(
            "Seasons",
            "winter-blue",
            "Winter Blue",
            json!({"treatment": "color", "grading": {"shadows": {"hue": 215.0, "sat": 18.0, "lum": 0.0}, "highlights": {"hue": 205.0, "sat": 8.0, "lum": 0.0}}, "color": {"saturation": -12.0}, "light": {"whites": 12.0}}),
        ),
        p(
            "Seasons",
            "spring-fresh",
            "Spring Fresh",
            json!({"treatment": "color", "mixer": {"green": {"hue": 10.0, "sat": 12.0, "lum": 10.0}, "magenta": {"sat": 10.0}}, "light": {"shadows": 15.0}, "color": {"vibrance": 15.0}}),
        ),
        p(
            "Seasons",
            "summer-haze",
            "Summer Haze",
            json!({"treatment": "color", "effects": {"dehaze": -12.0}, "curve": {"shadows": 18.0}, "grading": {"highlights": {"hue": 45.0, "sat": 14.0, "lum": 0.0}}, "light": {"contrast": -10.0}}),
        ),
        // ---- Vintage
        p(
            "Vintage",
            "instant-70s",
            "Instant ’70s",
            json!({"treatment": "color", "curve": {"shadows": 28.0, "highlights": -18.0}, "grading": {"shadows": {"hue": 170.0, "sat": 14.0, "lum": 0.0}, "highlights": {"hue": 45.0, "sat": 18.0, "lum": 0.0}}, "color": {"saturation": -10.0}, "vignette": {"amount": -15.0}, "grain": {"amount": 20.0, "size": 30.0, "roughness": 55.0}}),
        ),
        p(
            "Vintage",
            "cross-process",
            "Cross Process",
            json!({"treatment": "color", "grading": {"shadows": {"hue": 230.0, "sat": 25.0, "lum": 0.0}, "highlights": {"hue": 60.0, "sat": 25.0, "lum": 0.0}}, "light": {"contrast": 22.0}, "color": {"saturation": 10.0}}),
        ),
        p(
            "Vintage",
            "bleach-bypass",
            "Bleach Bypass",
            json!({"treatment": "color", "color": {"saturation": -40.0}, "light": {"contrast": 35.0, "highlights": -15.0}, "effects": {"clarity": 15.0}}),
        ),
        // ---- B&W toners
        p(
            "B&W",
            "bw-sepia",
            "Sepia Tone",
            json!({"treatment": "bw", "grading": {"shadows": {"hue": 32.0, "sat": 22.0, "lum": 0.0}, "highlights": {"hue": 42.0, "sat": 18.0, "lum": 0.0}}, "curve": {"shadows": 10.0}}),
        ),
    ];
    list.extend(film_effects());
    list
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
}

/// Our base profiles (rendering looks). Implemented in the pipeline.
pub const PROFILES: &[ProfileInfo] = &[
    ProfileInfo { id: "lc.color", name: "Color", group: "Basic" },
    ProfileInfo { id: "lc.neutral", name: "Neutral", group: "Basic" },
    ProfileInfo { id: "lc.vivid", name: "Vivid", group: "Basic" },
    ProfileInfo { id: "lc.landscape", name: "Landscape", group: "Basic" },
    ProfileInfo { id: "lc.portrait", name: "Portrait", group: "Basic" },
    ProfileInfo { id: "lc.mono", name: "Monochrome", group: "Basic" },
    // the look the camera gave the photo (its picture style), for a raw that starts from another one
    ProfileInfo { id: crate::pipeline::profiles::CAMERA, name: "Camera Look", group: "Camera Matching" },
    ProfileInfo { id: "lc.film.warm-print", name: "Warm Print", group: "Film" },
    ProfileInfo { id: "lc.film.cool-fade", name: "Cool Fade", group: "Film" },
    ProfileInfo { id: "lc.film.golden-hour", name: "Golden Hour", group: "Film" },
    ProfileInfo { id: "lc.film.faded-slide", name: "Faded Slide", group: "Film" },
    ProfileInfo { id: "lc.cine.teal-amber", name: "Teal & Amber", group: "Cinematic" },
    ProfileInfo { id: "lc.cine.night-blue", name: "Night Blue", group: "Cinematic" },
    ProfileInfo { id: "lc.cine.desert-heat", name: "Desert Heat", group: "Cinematic" },
    ProfileInfo { id: "lc.cine.neon-dusk", name: "Neon Dusk", group: "Cinematic" },
    ProfileInfo { id: "lc.muted.matte-soft", name: "Matte Soft", group: "Muted" },
    ProfileInfo { id: "lc.muted.bleached", name: "Bleached", group: "Muted" },
    ProfileInfo { id: "lc.muted.pastel-haze", name: "Pastel Haze", group: "Muted" },
    ProfileInfo { id: "lc.muted.quiet-green", name: "Quiet Green", group: "Muted" },
    ProfileInfo { id: "lc.bw.mono-rich", name: "Mono Rich", group: "B&W" },
    ProfileInfo { id: "lc.bw.red-filter", name: "Mono Red Filter", group: "B&W" },
    ProfileInfo { id: "lc.bw.soft", name: "Mono Soft", group: "B&W" },
    ProfileInfo { id: "lc.bw.sepia", name: "Sepia Tone", group: "B&W" },
];

/// Whether profile `id` has an Amount: every one but Color and Camera Look, which are starting points, not looks
/// laid over one (as in Lightroom, whose Adobe Raw and Camera Matching profiles have none).
pub fn has_amount(id: &str) -> bool {
    id != "lc.color" && id != crate::pipeline::profiles::CAMERA
}

/// Profile groups in menu/browser order.
pub fn profile_groups() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for p in PROFILES {
        if !out.contains(&p.group) {
            out.push(p.group);
        }
    }
    out
}

/// A profile by id.
pub fn profile(id: &str) -> Option<&'static ProfileInfo> {
    PROFILES.iter().find(|p| p.id == id)
}

/// How many recently used profiles the profile menu lists.
pub const RECENT_PROFILES: usize = 5;

impl Session {
    /// A profile by id, looking up built-in profiles first and imported LUT profiles second.
    pub fn profile_info(&self, id: &str) -> Option<(&str, &str)> {
        if let Some(p) = profile(id) {
            return Some((p.name, p.group));
        }
        if let Some(p) = self.lut_profiles.iter().chain(self.pack_profiles()).chain(self.pack_aliases()).find(|p| p.id == id) {
            return Some((p.name.as_str(), p.group.as_str()));
        }
        self.dcp_profiles.iter().find(|p| p.id == id).map(|p| (p.name.as_str(), crate::cmd::dcp_profiles::DCP_GROUP))
    }

    /// The imported profiles as (id, name, group): LUT profiles, then camera profiles (`.dcp`). A camera profile
    /// whose file is missing or damaged (not loaded when the library opened) is left out, so it can't be chosen; its
    /// entry stays and it is offered again once the file is back. (The app's own packs' profiles are not imported.)
    pub fn imported_profiles(&self) -> Vec<(String, String, String)> {
        let luts = self.lut_profiles.iter().map(|p| (p.id.clone(), p.name.clone(), p.group.clone()));
        let dcps = self
            .dcp_profiles
            .iter()
            .filter(|p| self.media.camera_profiles.contains_key(&p.id))
            .map(|p| (p.id.clone(), p.name.clone(), crate::cmd::dcp_profiles::DCP_GROUP.to_string()));
        luts.chain(dcps).collect()
    }

    /// Remember `id` as the most recently applied profile.
    pub fn note_profile_used(&mut self, id: &str) {
        self.profile_recent.retain(|p| p != id);
        self.profile_recent.insert(0, id.to_string());
        self.profile_recent.truncate(RECENT_PROFILES);
    }

    /// The profile menu: favourites, recent, then every group (`profiles.menu`).
    pub fn profile_menu(&self) -> Value {
        let item = |p: &ProfileInfo| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": self.profile_favorites.iter().any(|f| f == p.id), "amount": has_amount(p.id)});
        let imported = self.imported_profiles();
        let pack_item = |id: &str| {
            self.pack_profiles()
                .find(|p| p.id == id)
                .map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": self.profile_favorites.contains(&p.id)}))
        };
        let imported_item = |(id, name, group): &(String, String, String)| {
            let mut v = json!({"id": id, "name": name, "group": group, "favorite": self.profile_favorites.contains(id), "imported": true});
            if let Some(d) = self.dcp_profiles.iter().find(|d| &d.id == id) {
                v["cameraModel"] = json!(d.camera_model);
            }
            v
        };
        let lut_item = |id: &str| imported.iter().find(|p| p.0 == id).map(imported_item).or_else(|| pack_item(id));
        let list = |ids: &[String]| ids.iter().filter_map(|id| profile(id).map(item).or_else(|| lut_item(id))).collect::<Vec<_>>();
        let mut groups: Vec<Value> = profile_groups()
            .into_iter()
            .map(|g| json!({"name": g, "profiles": PROFILES.iter().filter(|p| p.group == g).map(item).collect::<Vec<_>>()}))
            .collect();
        // the app's own packs' profiles (can't be deleted), by their groups, after the built-in ones
        let mut pack_groups: Vec<&str> = Vec::new();
        for p in self.pack_profiles() {
            if !pack_groups.contains(&p.group.as_str()) {
                pack_groups.push(p.group.as_str());
            }
        }
        for g in pack_groups {
            let profiles: Vec<Value> = self
                .pack_profiles()
                .filter(|p| p.group == g)
                .map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": self.profile_favorites.contains(&p.id)}))
                .collect();
            groups.push(json!({"name": g, "profiles": profiles}));
        }
        // imported LUT profiles by their groups, then camera profiles
        let mut lut_groups: Vec<&str> = self.lut_profiles.iter().map(|p| p.group.as_str()).collect();
        lut_groups.sort_unstable();
        lut_groups.dedup();
        // the camera profiles' group once, last (a LUT group of the same name joins it rather than showing twice)
        lut_groups.retain(|g| *g != crate::cmd::dcp_profiles::DCP_GROUP);
        if imported.iter().any(|p| p.2 == crate::cmd::dcp_profiles::DCP_GROUP) {
            lut_groups.push(crate::cmd::dcp_profiles::DCP_GROUP);
        }
        for g in lut_groups {
            let profiles: Vec<Value> = imported.iter().filter(|p| p.2 == g).map(imported_item).collect();
            groups.push(json!({"name": g, "profiles": profiles, "imported": true}));
        }
        json!({
            "favorites": list(&self.profile_favorites),
            "recent": list(&self.profile_recent),
            "groups": groups,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_presets_parse_and_apply() {
        let base = lightcraft_develop::DevelopSettings::default();
        let mut ids = std::collections::HashSet::new();
        for pr in builtin() {
            assert!(ids.insert(pr.id.clone()));
            let out = pr.apply(&base, 1.0);
            assert_ne!(out, base, "{} had no effect", pr.id);
        }
    }

    /// The Film Effects presets put film on the photo: marks that draw (each with its own id), and film's grain.
    #[test]
    fn film_effects_presets_draw_film() {
        let base = lightcraft_develop::DevelopSettings::default();
        let fx = film_effects();
        assert!(fx.len() >= 6);
        for pr in fx {
            let out = pr.apply(&base, 1.0);
            assert!(!out.film.layers.is_empty() && out.film.vary, "{}", pr.id);
            let mut ids: Vec<u32> = out.film.layers.iter().map(|l| l.id).collect();
            ids.dedup();
            assert_eq!(ids, (1..=out.film.layers.len() as u32).collect::<Vec<_>>(), "{}", pr.id);
            assert!(out.film.layers.iter().all(|l| l.visible && l.amount > 0.0 && l.kind.fields().contains(&"amount")), "{}", pr.id);
            assert!(out.grain.amount > 0.0, "{}", pr.id);
            // and at half strength it still parses (the marks' numbers scale, their seeds stay whole)
            assert_eq!(pr.apply(&base, 0.5).film.layers.len(), out.film.layers.len(), "{}", pr.id);
        }
    }

    #[test]
    fn every_profile_has_a_look_and_ids_are_unique() {
        let looks = crate::pipeline::profiles::LOOK_IDS;
        let mut ids = std::collections::HashSet::new();
        for p in PROFILES {
            assert!(ids.insert(p.id), "{}", p.id);
            assert!(!has_amount(p.id) || looks.contains(&p.id), "{} has no look", p.id);
        }
        assert!(PROFILES.len() >= 22);
        assert_eq!(profile_groups(), ["Basic", "Camera Matching", "Film", "Cinematic", "Muted", "B&W"]);
    }

    #[test]
    fn creative_profiles_render_without_moving_the_sliders() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.4})).unwrap();
        let before = s.develop_of(id).unwrap();
        let plain = s.render_now(id, 96, 96).unwrap().image;
        s.execute("develop.profile", &json!({"id": "lc.cine.teal-amber", "amount": 150})).unwrap();
        let after = s.develop_of(id).unwrap();
        assert_eq!((after.profile.id.as_str(), after.profile.amount), ("lc.cine.teal-amber", 150.0));
        let mut same = (*after).clone();
        same.profile = before.profile.clone();
        assert_eq!(same, *before, "only the profile changed");
        let looked = s.render_now(id, 96, 96).unwrap().image;
        assert_ne!(plain.as_bytes(), looked.as_bytes());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Warm & Soft!"), "warm-soft");
        assert_eq!(slug("  "), "preset");
        assert_eq!(slug("B&W/Film 2"), "b-w-film-2");
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-presets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Groups dragged into an order keep it; groups it doesn't name follow by name, names of groups that
    /// are gone are skipped, and a bad move is an error, not a panic.
    #[test]
    fn preset_groups_keep_the_users_order() {
        let mut s = Session::new();
        let by_name = s.preset_groups();
        assert!(by_name.windows(2).all(|w| w[0] < w[1]), "by name until reordered: {by_name:?}");
        let last = by_name.last().unwrap().clone();
        let r = s.execute("preset.moveGroup", &json!({"group": last, "to": 0})).unwrap();
        assert_eq!(r["groups"][0], json!(last));
        assert_eq!(s.preset_groups()[1..], by_name[..by_name.len() - 1]);
        // a group made later goes last; one that's gone is skipped
        s.preset_group_order.insert(1, "Gone".into());
        s.preset_group_order.push(last.clone());
        s.presets.push(Preset { id: "u.a".into(), name: "A".into(), group: "AAA".into(), settings: json!({}), favorite: false, builtin: false });
        let now = s.preset_groups();
        assert_eq!((now.first(), now.last()), (Some(&last), Some(&"AAA".to_string())));
        assert!(!now.contains(&"Gone".to_string()));
        assert_eq!(now.iter().filter(|g| **g == last).count(), 1, "a group named twice shows once");
        // past the end is the end
        s.execute("preset.moveGroup", &json!({"group": last, "to": u64::MAX})).unwrap();
        assert_eq!(s.preset_groups().last(), Some(&last));
        assert!(s.execute("preset.moveGroup", &json!({"group": "Nope", "to": 0})).is_err());
        assert!(s.execute("preset.moveGroup", &json!({"group": last, "to": -1})).is_err());
        assert_eq!(s.execute("presets.groups", &json!({})).unwrap(), json!(s.preset_groups()));
    }

    #[test]
    fn lcpreset_export_import_roundtrip_keeps_groups() {
        let d = dir("rt");
        let mut s = Session::with_demo();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.6})).unwrap();
        s.execute("preset.create", &json!({"name": "Bright", "group": "Travel"})).unwrap();
        s.execute("develop.set", &json!({"control": "effects.clarity", "value": 30})).unwrap();
        s.execute("preset.create", &json!({"name": "Crisp", "group": "Street"})).unwrap();
        let mine: Vec<Preset> = s.presets.iter().filter(|p| !p.builtin).cloned().collect();
        assert_eq!(mine.len(), 2);
        let path = d.join("mine");
        let r = s.execute("preset.export", &json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(r["count"], 2);
        let file = d.join("mine.lcpreset");
        assert!(file.is_file());
        let one = d.join("travel.lcpreset");
        s.execute("preset.export", &json!({"path": one.to_string_lossy(), "group": "Travel"})).unwrap();
        assert!(s.execute("preset.export", &json!({"path": one.to_string_lossy(), "group": "Nope"})).is_err());

        // a fresh session imports both files from the folder; the duplicate is skipped
        let mut t = Session::new();
        let r = t.execute("preset.import", &json!({"paths": [d.to_string_lossy()]})).unwrap();
        assert_eq!(r["imported"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["skipped"], 1, "{r}");
        for p in &mine {
            let q = t.presets.iter().find(|q| q.name == p.name).unwrap();
            assert_eq!((&q.group, &q.settings, q.builtin), (&p.group, &p.settings, false));
        }
        // importing into the session that has them: everything is skipped, nothing duplicated
        let n = s.presets.len();
        let r = s.execute("preset.import", &json!({"paths": [file.to_string_lossy()]})).unwrap();
        assert_eq!((r["skipped"].as_u64(), s.presets.len()), (Some(2), n));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn id_clashes_get_fresh_ids_and_bad_files_are_reported() {
        let d = dir("clash");
        let builtin = builtin().remove(0);
        let mut clash = builtin.clone();
        clash.settings = json!({"light": {"exposure": 1.0}});
        std::fs::write(d.join("a.lcpreset"), serde_json::to_string(&clash).unwrap()).unwrap();
        std::fs::write(d.join("b.lcpreset"), "{not json").unwrap();
        std::fs::write(d.join("c.lcpreset"), r#"{"format": "other", "version": 1, "presets": []}"#).unwrap();
        std::fs::write(d.join("notes.txt"), "ignored inside folders").unwrap();
        let mut s = Session::new();
        let r = s.execute("preset.import", &json!({"paths": [d.to_string_lossy()]})).unwrap();
        assert_eq!(r["failed"].as_array().unwrap().len(), 2, "{r}");
        let id = r["imported"][0]["id"].as_str().unwrap();
        assert_ne!(id, builtin.id);
        assert!(id.starts_with("user."));
        assert!(s.presets.iter().find(|p| p.id == builtin.id).unwrap().builtin, "built-in untouched");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An XMP preset (hand-written for these tests, not a third-party preset).
    fn xmp_preset(exposure: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
            crs:PresetType="Normal" crs:Exposure2012="{exposure}" crs:Contrast2012="+10"/></rdf:RDF></x:xmpmeta>"#
        )
    }

    /// Files handed over by content (the browser's file chooser has no paths) import like files
    /// on disk: zips and all, a folder in the name gives the group, and bad entries are reported.
    #[test]
    fn presets_import_from_files_given_by_content() {
        let lc = to_lcpreset(&[Preset {
            id: "user.s7-3.mine".into(),
            name: "Mine".into(),
            group: "Travel".into(),
            settings: json!({"color": {"vibrance": 12.0}}),
            favorite: true,
            builtin: false,
        }]);
        let zip = crate::preset_import::tests::zip(&[("Film Pack/Warm.xmp", xmp_preset("+0.40").as_bytes())], true);
        let files = crate::inline_files::to_param([
            ("Pack/Soft.xmp", xmp_preset("+0.20").as_bytes()),
            ("mine.lcpreset", lc.as_bytes()),
            ("bundle.zip", zip.as_slice()),
            ("notes.txt", b"not a preset".as_slice()),
        ]);
        let mut bad = files.as_array().unwrap().clone();
        bad.push(json!({"name": "broken.xmp", "data": "%%%"}));
        let mut s = Session::with_demo();
        let r = s.execute("preset.import", &json!({"files": bad})).unwrap();
        let got: Vec<(&str, &str)> =
            r["imported"].as_array().unwrap().iter().map(|i| (i["name"].as_str().unwrap(), i["group"].as_str().unwrap())).collect();
        assert_eq!(got, [("Soft", "Pack"), ("Mine", "Travel"), ("Warm", "Film Pack")], "{r}");
        let failed: Vec<&str> = r["failed"].as_array().unwrap().iter().map(|f| f[0].as_str().unwrap()).collect();
        assert_eq!(failed, ["notes.txt", "broken.xmp"], "{r}");
        assert_eq!(r["imported"][1]["file"], "mine.lcpreset");
        let soft = r["imported"][0]["id"].as_str().unwrap().to_string();
        s.execute("preset.apply", &json!({"id": soft})).unwrap();
        assert_eq!(s.develop_of(s.active().unwrap()).unwrap().light.exposure, 0.2);
        // the same files again: nothing new
        let r = s.execute("preset.import", &json!({"files": files})).unwrap();
        assert_eq!((r["imported"].as_array().unwrap().len(), r["skipped"].as_u64()), (0, Some(3)), "{r}");
        // paths and files together; neither is an error
        assert!(s.execute("preset.import", &json!({})).is_err());
        assert!(s.execute("preset.import", &json!({"paths": [], "files": []})).is_err());
    }

    /// Imported presets are named by their content: the same preset gets the same id in any
    /// library (so synced devices see one), different presets with the same name get different ones.
    #[test]
    fn imported_presets_are_named_by_their_content() {
        let one = |exposure: &str| {
            let mut s = Session::new();
            let files = crate::inline_files::to_param([("Look.xmp", xmp_preset(exposure).as_bytes())]);
            let dry = s.execute("preset.import", &json!({"files": files, "dryRun": true})).unwrap();
            let r = s.execute("preset.import", &json!({"files": files})).unwrap();
            let id = r["imported"][0]["id"].as_str().unwrap().to_string();
            assert_eq!(dry["imported"][0]["id"].as_str(), Some(id.as_str()), "a dry run names the same id");
            // (once it is there, a dry run reports it as skipped, as a real run would)
            let again = s.execute("preset.import", &json!({"files": files, "dryRun": true})).unwrap();
            assert_eq!((again["imported"].as_array().map(Vec::len), again["skipped"].as_u64()), (Some(0), Some(1)), "{again}");
            (s, id)
        };
        let (mut a, id) = one("+0.30");
        let (_, same) = one("+0.30");
        let (_, other) = one("+0.60");
        assert_eq!(id, same);
        assert_ne!(id, other);
        assert!(id.starts_with("user.imp.") && id.len() == "user.imp.".len() + 16, "{id}");
        // key order doesn't matter, the values do
        let p = |settings: Value| Preset { id: String::new(), name: "N".into(), group: "G".into(), settings, favorite: false, builtin: false };
        assert_eq!(
            imported_preset_id(&p(json!({"light": {"exposure": 0.3, "contrast": 10.0}, "color": {}}))),
            imported_preset_id(&p(json!({"color": {}, "light": {"contrast": 10.0, "exposure": 0.3}})))
        );
        // edited after its import, then imported again: kept apart under a fresh id
        let i = a.presets.iter().position(|x| x.id == id).unwrap();
        a.presets[i].settings = json!({"light": {"exposure": 1.0}});
        let files = crate::inline_files::to_param([("Look.xmp", xmp_preset("+0.30").as_bytes())]);
        let r = a.execute("preset.import", &json!({"files": files})).unwrap();
        assert_eq!(r["imported"][0]["id"].as_str(), Some(format!("{id}-2").as_str()), "{r}");
    }

    /// A preset's Amount drag (HB-445): it begins from before the preset's step with the photo as shown, each step goes over
    /// that photo (not over the last step), nothing is stored until the end, and the end is one step in the preset's place.
    #[test]
    fn preset_amount_drag_is_live_and_one_step() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        s.presets.push(lightcraft_develop::Preset {
            id: "t.look".into(),
            name: "Look".into(),
            group: "User".into(),
            settings: json!({"light": {"exposure": 1.0}}),
            favorite: false,
            builtin: false,
        });
        let exposure = |s: &Session| s.develop_of(id).unwrap().light.exposure;
        let start = exposure(&s);
        s.execute("preset.apply", &json!({"id": "t.look"})).unwrap();
        let steps = s.undo.len();
        assert_eq!((exposure(&s), s.undo.last().unwrap().label.as_str()), (1.0, "Preset: Look"));
        s.execute("develop.beginInteraction", &json!({"label": "Preset: Look", "replacing": true})).unwrap();
        assert_eq!(exposure(&s), 1.0, "the photo stays as shown when the drag begins");
        for amount in [50.0, 30.0] {
            s.execute("preset.apply", &json!({"id": "t.look", "amount": amount})).unwrap();
            assert!((exposure(&s) - start - (1.0 - start) * amount / 100.0).abs() < 1e-9, "{amount}: {}", exposure(&s));
            assert_eq!(s.undo.len(), steps - 1, "no step during the drag");
        }
        s.execute("develop.endInteraction", &json!({})).unwrap();
        assert_eq!((s.undo.len(), s.undo.last().unwrap().label.as_str()), (steps, "Preset: Look"));
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(exposure(&s), start, "one undo takes the preset off");
        // (another step on top: nothing is taken back)
        s.execute("develop.beginInteraction", &json!({"label": "Preset: Look", "replacing": true})).unwrap();
        s.execute("develop.endInteraction", &json!({})).unwrap();
        assert_eq!(exposure(&s), start);
    }

    #[test]
    fn xmp_preset_file_imports_and_applies() {
        let d = dir("xmp");
        // Hand-written for this test (not a third-party preset).
        let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
            crs:PresetType="Normal" crs:Exposure2012="+0.30" crs:Shadows2012="+20" crs:GrainAmount="12">
            <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Test Group</rdf:li></rdf:Alt></crs:Group>
          </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        std::fs::write(d.join("Soft Lift.xmp"), x).unwrap();
        let mut s = Session::with_demo();
        let r = s.execute("preset.import", &json!({"paths": [d.join("Soft Lift.xmp").to_string_lossy()]})).unwrap();
        let id = r["imported"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(r["imported"][0]["name"], "Soft Lift", "file name when crs:Name is missing");
        assert_eq!(r["imported"][0]["group"], "Test Group");
        s.execute("preset.apply", &json!({"id": id})).unwrap();
        let dv = s.develop_of(s.active().unwrap()).unwrap();
        assert_eq!((dv.light.exposure, dv.light.shadows, dv.grain.amount), (0.3, 20.0, 12.0));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Hiding (built-in presets too), renaming and deleting groups of the user's presets; the hidden
    /// state and the group order follow a renamed group.
    #[test]
    fn presets_and_groups_hide_rename_and_delete() {
        let mut s = Session::new();
        let mine = |s: &mut Session, id: &str, group: &str| {
            s.presets.push(Preset { id: id.into(), name: id.into(), group: group.into(), settings: json!({}), favorite: false, builtin: false })
        };
        mine(&mut s, "user.a", "Trips");
        mine(&mut s, "user.b", "Trips");
        mine(&mut s, "user.c", "Mine");
        assert_eq!(s.execute("preset.hide", &json!({"id": "lc.moody"})).unwrap()["hidden"], true);
        assert_eq!(s.execute("preset.hide", &json!({"id": "lc.moody"})).unwrap()["hidden"], false, "toggles");
        s.execute("preset.hide", &json!({"id": "user.a", "hidden": true})).unwrap();
        assert!(s.execute("preset.hide", &json!({"id": "nope"})).is_err());
        s.execute("preset.hideGroup", &json!({"group": "Trips"})).unwrap();
        assert!(s.execute("preset.hideGroup", &json!({"group": "Nope"})).is_err());
        let list = s.execute("presets.list", &json!({})).unwrap();
        let a = list.as_array().unwrap().iter().find(|p| p["id"] == "user.a").unwrap();
        assert_eq!(a["hidden"], true);
        s.execute("preset.moveGroup", &json!({"group": "Trips", "to": 0})).unwrap();
        // renamed: its place and its being hidden go with it
        assert!(s.execute("preset.renameGroup", &json!({"group": "B&W", "name": "Mono"})).is_err(), "built-in groups stay");
        let r = s.execute("preset.renameGroup", &json!({"group": "Trips", "name": "Travel"})).unwrap();
        assert_eq!(r["groups"][0], "Travel");
        assert_eq!(s.hidden_presets.groups, ["Travel"]);
        assert!(s.presets.iter().filter(|p| p.group == "Travel").count() == 2);
        let details = s.execute("presets.groups", &json!({"details": true})).unwrap();
        assert_eq!(details[0], json!({"name": "Travel", "hidden": true, "builtin": false, "user": true}));
        assert!(details.as_array().unwrap().iter().any(|g| g["name"] == "B&W" && g["builtin"] == true && g["user"] == false));
        // into a group that is already there: merged
        s.execute("preset.renameGroup", &json!({"group": "Mine", "name": "Travel"})).unwrap();
        assert_eq!(s.presets.iter().filter(|p| p.group == "Travel").count(), 3);
        // deleted: the user's presets go, with what was hidden of them
        assert!(s.execute("preset.deleteGroup", &json!({"group": "Film"})).is_err(), "built-in presets stay");
        assert_eq!(s.execute("preset.deleteGroup", &json!({"group": "Travel"})).unwrap()["deleted"], 3);
        assert!(s.presets.iter().all(|p| p.builtin));
        assert!(s.hidden_presets.presets.is_empty() && s.hidden_presets.groups.is_empty());
    }

    /// The server's presets join the list like built-in ones, and the built-ins it hides leave it.
    #[test]
    fn server_presets_join_and_hide() {
        let mut s = Session::new();
        s.presets.push(Preset { id: "user.a".into(), name: "A".into(), group: "Mine".into(), settings: json!({}), favorite: true, builtin: false });
        s.execute("preset.favorite", &json!({"id": "lc.bw-soft", "favorite": true})).unwrap();
        let server = vec![
            Preset { id: "srv.one".into(), name: "One".into(), group: "Team".into(), settings: json!({}), favorite: false, builtin: false },
            Preset { id: "user.x".into(), name: "Not".into(), group: "Team".into(), settings: json!({}), favorite: false, builtin: false },
        ];
        s.set_server_presets(server, Hidden { presets: vec!["lc.moody".into()], groups: vec!["Film".into()] });
        assert!(s.presets.iter().any(|p| p.id == "srv.one" && p.builtin && is_server_preset(p)));
        assert!(!s.presets.iter().any(|p| p.id == "user.x" || p.id == "lc.moody" || p.group == "Film"));
        assert!(s.presets.iter().any(|p| p.id == "lc.bw-soft" && p.favorite), "favourites stay");
        assert!(s.presets.iter().any(|p| p.id == "user.a" && p.favorite && !p.builtin), "the user's presets stay");
        s.set_server_presets(Vec::new(), Hidden::default());
        assert!(s.presets.iter().any(|p| p.id == "lc.moody") && !s.presets.iter().any(|p| p.id == "srv.one"));
    }

    /// Issue #2159: a color preset after a B&W one brings the colors back. The treatment and the
    /// settings the color preset names take its values; a setting it does not name keeps the
    /// value the B&W preset gave it.
    #[test]
    fn a_color_preset_after_a_bw_one_returns_the_colors() {
        let mut s = Session::with_demo();
        s.execute("preset.apply", &json!({"id": "lc.bw-high-contrast"})).unwrap();
        let dv = s.develop_of(s.active().unwrap()).unwrap();
        assert_eq!(dv.treatment, lightcraft_develop::Treatment::Bw);
        s.execute("preset.apply", &json!({"id": "lc.warm-glow"})).unwrap();
        let dv = s.develop_of(s.active().unwrap()).unwrap();
        assert_eq!(dv.treatment, lightcraft_develop::Treatment::Color, "the color preset restores the treatment");
        assert_eq!(dv.light.contrast, 8.0, "the color preset's values, not the B&W preset's");
        assert_eq!(dv.light.whites, 20.0, "a setting the color preset doesn't name keeps the B&W preset's value");
    }

    /// A manual edit to a setting a preset does not name survives it, though the preset styles
    /// the same group (Bright & Airy names exposure, contrast, highlights, shadows and whites).
    #[test]
    fn a_preset_keeps_a_setting_it_does_not_name_in_the_group_it_styles() {
        let mut s = Session::with_demo();
        s.execute("develop.set", &json!({"control": "light.blacks", "value": -30.0})).unwrap();
        s.execute("preset.apply", &json!({"id": "lc.bright-airy"})).unwrap();
        let dv = s.develop_of(s.active().unwrap()).unwrap();
        assert_eq!(dv.light.blacks, -30.0, "blacks is not named by the preset");
        assert_eq!(dv.light.exposure, 0.45, "exposure is named by the preset and takes its value");
    }

    /// Colours a partial preset does not name keep the photo's values. Deep Sky names only the
    /// blue and aqua mixer colours, and only their luminance and saturation.
    #[test]
    fn a_preset_keeps_the_mixer_colours_it_does_not_name() {
        let mut s = Session::with_demo();
        s.execute("develop.set", &json!({"control": "mixer.red.sat", "value": 20.0})).unwrap();
        s.execute("develop.set", &json!({"control": "mixer.blue.hue", "value": 15.0})).unwrap();
        s.execute("preset.apply", &json!({"id": "lc.deep-sky"})).unwrap();
        let dv = s.develop_of(s.active().unwrap()).unwrap();
        assert_eq!(dv.mixer.red.sat, 20.0, "a colour the preset doesn't name");
        assert_eq!(dv.mixer.blue.hue, 15.0, "a setting of a named colour that the preset doesn't name");
        assert_eq!(dv.mixer.blue.lum, -25.0, "a setting the preset names takes its value");
    }

    /// Settings that came from an XMP sidecar or a catalog import record no import look, so the
    /// camera defaults are not their original: a preset that names only some of their group keeps
    /// the rest.
    #[test]
    fn xmp_imported_exposure_whites_and_blacks_survive_crisp_landscape() {
        let mut s = Session::with_demo();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
        s.execute("develop.set", &json!({"control": "light.whites", "value": 10.0})).unwrap();
        s.execute("develop.set", &json!({"control": "light.blacks", "value": -20.0})).unwrap();
        let id = s.active().unwrap();
        assert!(s.catalog.photo(id).unwrap().import_look.is_none());
        s.execute("preset.apply", &json!({"id": "lc.crisp-landscape"})).unwrap();
        let dv = s.develop_of(id).unwrap();
        assert_eq!((dv.light.exposure, dv.light.whites, dv.light.blacks), (0.7, 10.0, -20.0));
        assert_eq!(dv.light.contrast, 12.0, "a setting the preset names takes its value");
    }

    /// Amount 0 applies nothing to a photo with no import look: the photo keeps every value, also
    /// the exposure that Bright & Airy names (a camera-default original would reset it to 0).
    #[test]
    fn amount_zero_applies_nothing_to_a_photo_without_an_import_look() {
        let mut s = Session::with_demo();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
        let id = s.active().unwrap();
        let before = s.develop_of(id).unwrap();
        s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 0})).unwrap();
        assert_eq!(s.develop_of(id).unwrap(), before);
    }

    /// With no import look, a preset's amount moves the named settings from the photo's current
    /// values: 50 % of Bright & Airy's exposure (0.45) from 2.0 is 1.225.
    #[test]
    fn a_preset_amount_without_an_import_look_is_measured_from_the_current_settings() {
        let mut s = Session::with_demo();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
        s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 50})).unwrap();
        let dv = s.develop_of(s.active().unwrap()).unwrap();
        assert!((dv.light.exposure - 1.225).abs() < 1e-9, "{}", dv.light.exposure);
    }

    /// A photo whose default preset recorded an import look measures a preset's amount from that
    /// look: 50 % is halfway from the look to the preset whatever the photo was edited to, and
    /// 0 % gives the look's value for the settings the preset names.
    #[test]
    fn a_preset_amount_is_measured_from_the_import_look_when_there_is_one() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        let mut p = (**s.catalog.photo(id).unwrap()).clone();
        let mut look = p.camera_defaults();
        look.light.exposure = 0.5;
        look.light.blacks = -10.0;
        p.import_look = Some(std::sync::Arc::new(look.clone()));
        p.develop = std::sync::Arc::new(look);
        s.catalog.apply(lightcraft_catalog::Op::RemovePhoto { id: p.id }).unwrap();
        s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
        s.execute("develop.set", &json!({"control": "light.blacks", "value": -30.0})).unwrap();
        s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 50})).unwrap();
        let dv = s.develop_of(id).unwrap();
        assert!((dv.light.exposure - 0.475).abs() < 1e-9, "{}", dv.light.exposure);
        assert_eq!(dv.light.blacks, -30.0, "a setting the preset doesn't name keeps the photo's edit");
        s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 0})).unwrap();
        let dv = s.develop_of(id).unwrap();
        assert_eq!(dv.light.exposure, 0.5, "amount 0 gives the import look's value for a setting the preset names");
    }

    /// A photo whose import look records a free crop aspect (null) but whose edits locked one
    /// gets the import value back at 0 % from a preset that names the aspect: the lock is not
    /// kept (the baseline of the amount contract), the cleared option is restored.
    #[test]
    fn amount_zero_restores_a_null_import_aspect() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        let mut p = (**s.catalog.photo(id).unwrap()).clone();
        let look = p.camera_defaults();
        p.import_look = Some(std::sync::Arc::new(look.clone()));
        p.develop = std::sync::Arc::new(look);
        s.catalog.apply(lightcraft_catalog::Op::RemovePhoto { id: p.id }).unwrap();
        s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
        s.execute("crop.aspect", &json!({"aspect": [3, 2]})).unwrap();
        assert!(s.develop_of(id).unwrap().crop.aspect.is_some(), "the aspect is locked for this test");
        let added = s.add_presets(vec![lightcraft_develop::Preset {
            id: "user.free-aspect".into(),
            name: "Free aspect".into(),
            group: "Test".into(),
            settings: json!({"crop": {"aspect": null}}),
            favorite: false,
            builtin: false,
        }]);
        s.execute("preset.apply", &json!({"id": added[0], "amount": 0})).unwrap();
        let dv = s.develop_of(id).unwrap();
        assert_eq!(dv.crop.aspect, None, "amount 0 gives the import look's free aspect back");
    }
}
