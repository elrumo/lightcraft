//! Camera colour profiles of our own, pooled from many photos of one camera model.
//!
//! A supported raw file without colour matrices gets its look fitted to its own embedded camera
//! JPEG (`camera_preview`), but one photo shows too little of some colours: a lime shirt covering a
//! few dozen proxy pixels next to a hillside of foliage at the same hue. `lightcraft-cli calibrate`
//! pools the colour pairs of many photos per model and fits one matrix and hue/saturation/value
//! table; photos of that model then only fit their tone and chroma curves.
//!
//! Profiles are JSON files (`<model>.json`) in [`dir`]: `LIGHTCRAFT_CAMERA_PROFILES`, else
//! `<config>/camera-profiles`; a local profile replaces one from the profile database
//! ([`crate::profile_db`], downloaded from a server), which replaces the one built in ([`BUNDLED`], from
//! `assets/camera-profiles/`). They are read once per process, and again after the profile database
//! changes; a damaged or hostile file is ignored with a warning. They hold aggregate colour statistics
//! only, never image content.
use lightcraft_color::Mat3;
use lightcraft_raw::profile::HsvTable;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Format version of the profile files.
const VERSION: u32 = 1;
/// Largest profile file read (a 5 × 72 × 5 table is ~60 KB of JSON).
const MAX_FILE: u64 = 4 << 20;

/// Profiles built into LightCraft (`assets/camera-profiles/`, see `assets/ATTRIBUTION.md`):
/// `(model, JSON)`.
pub const BUNDLED: &[(&str, &str)] = &[
    ("ILCA-99M2", include_str!("../../../assets/camera-profiles/ILCA-99M2.json")),
    ("ILCE-7CR", include_str!("../../../assets/camera-profiles/ILCE-7CR.json")),
    ("ILCE-7M4", include_str!("../../../assets/camera-profiles/ILCE-7M4.json")),
    ("NIKON D5300", include_str!("../../../assets/camera-profiles/NIKON_D5300.json")),
    ("NIKON Z 6_2", include_str!("../../../assets/camera-profiles/NIKON_Z_6_2.json")),
    ("NIKON Z 8", include_str!("../../../assets/camera-profiles/NIKON_Z_8.json")),
    ("X-H2S", include_str!("../../../assets/camera-profiles/X-H2S.json")),
    ("X-T4", include_str!("../../../assets/camera-profiles/X-T4.json")),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraProfile {
    pub version: u32,
    /// The camera model as the files report it (Exif `Model`), e.g. `ILCE-7M4`.
    pub model: String,
    /// Photos and colour pairs the profile was fitted on.
    pub files: usize,
    pub samples: usize,
    /// White-balanced camera RGB (with the baseline exposure) → linear Rec.2020, rows.
    matrix: [[f64; 3]; 3],
    /// Hue/saturation/value correction after `matrix` (linear ProPhoto RGB).
    pub hue_sat: Option<HsvTable>,
}

impl CameraProfile {
    pub fn new(model: &str, files: usize, samples: usize, matrix: Mat3, hue_sat: Option<HsvTable>) -> CameraProfile {
        CameraProfile { version: VERSION, model: model.to_owned(), files, samples, matrix: matrix.0, hue_sat }
    }

    pub fn matrix(&self) -> Mat3 {
        Mat3(self.matrix)
    }

    /// Whether the data is usable (bounded matrix, table shape matching its data).
    fn valid(&self) -> bool {
        let matrix = self.matrix.iter().flatten().all(|v| v.is_finite() && v.abs() < 8.0) && Mat3(self.matrix).inverse().is_some();
        self.version == VERSION && !self.model.is_empty() && matrix && self.hue_sat.as_ref().is_none_or(table_valid)
    }
}

/// A table's shape matches its data, and its entries are finite with non-negative scales.
fn table_valid(t: &HsvTable) -> bool {
    let dims = (1..=4096).contains(&t.hue_divisions) && (2..=4096).contains(&t.sat_divisions) && (1..=4096).contains(&t.val_divisions);
    let len = t.hue_divisions.checked_mul(t.sat_divisions).and_then(|n| n.checked_mul(t.val_divisions));
    dims && len == Some(t.data.len()) && t.data.iter().flatten().all(|v| v.is_finite()) && t.data.iter().all(|e| e[1] >= 0.0 && e[2] >= 0.0)
}

/// The hue/saturation table a camera model's neutral look adds on top of its measured matrices (Canon CR3s,
/// `camera_preview::neutral_look`), fitted by `lightcraft-cli calibrate --references` to renders of the same photos by
/// another raw converter. Files are `<model>.neutral.json` next to the profiles ([`dir`], replacing a built-in one,
/// [`NEUTRAL_BUNDLED`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NeutralTable {
    pub version: u32,
    /// The camera model as the files report it (Exif `Model`), e.g. `Canon EOS R6`.
    pub model: String,
    /// Photos and colour pairs the table was fitted on.
    pub files: usize,
    pub samples: usize,
    /// Correction of the scene colour through the model's spectral matrices (linear ProPhoto RGB).
    pub hue_sat: HsvTable,
}

/// Neutral-look tables built into LightCraft (`assets/camera-profiles/`, see `assets/ATTRIBUTION.md`):
/// `(model, JSON)`. The EOS R6's is fitted to Lightroom's default exports of the maintainer's own photos (maintainer
/// decision 2026-10-17, `docs/camera-preview-colour.md`).
pub const NEUTRAL_BUNDLED: &[(&str, &str)] = &[("Canon EOS R6", include_str!("../../../assets/camera-profiles/Canon_EOS_R6.neutral.json"))];

/// File name of `model`'s neutral-look table.
pub fn neutral_file_name(model: &str) -> String {
    file_name(model).replace(".json", ".neutral.json")
}

fn parse_neutral(json: &[u8], origin: &str) -> Result<NeutralTable, String> {
    let table: NeutralTable = serde_json::from_slice(json).map_err(|e| format!("{origin}: {e}"))?;
    if table.version != VERSION || table.model.trim().is_empty() || !table_valid(&table.hue_sat) {
        return Err(format!("{origin}: not a usable neutral-look table (version {}, model {:?})", table.version, table.model));
    }
    Ok(table)
}

/// Write `table` to `dir` (created if needed) as `<model>.neutral.json`; returns the path.
pub fn save_neutral(table: &NeutralTable, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(neutral_file_name(&table.model));
    let json = serde_json::to_vec_pretty(table).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The neutral-look table for camera `model`: from [`dir`], else built in; read once per process.
pub fn neutral_table(model: &str) -> Option<Arc<NeutralTable>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<NeutralTable>>>>> = OnceLock::new();
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hit) = cache.get(model) {
        return hit.clone();
    }
    let local = dir().map(|d| d.join(neutral_file_name(model))).filter(|p| p.is_file()).and_then(|path| {
        let read = std::fs::metadata(&path)
            .map_err(|e| e.to_string())
            .and_then(|m| {
                if m.len() > MAX_FILE { Err(format!("{} bytes is too large", m.len())) } else { std::fs::read(&path).map_err(|e| e.to_string()) }
            })
            .and_then(|bytes| parse_neutral(&bytes, &path.display().to_string()))
            .and_then(|t| if t.model.trim() == model { Ok(t) } else { Err(format!("it is for {:?}, not {model:?}", t.model)) });
        read.inspect_err(|e| eprintln!("lightcraft: ignoring neutral-look table {}: {e}", path.display())).ok()
    });
    let table = local
        .or_else(|| {
            let (_, json) = NEUTRAL_BUNDLED.iter().find(|(m, _)| *m == model)?;
            parse_neutral(json.as_bytes(), &format!("built-in neutral-look table {model}"))
                .inspect_err(|e| eprintln!("lightcraft: ignoring {e}"))
                .ok()
        })
        .map(Arc::new);
    cache.insert(model.to_owned(), table.clone());
    table
}

/// LightCraft's configuration folder (settings, GPU marker, camera profiles); one helper for the app.
pub use crate::config::config_dir;

/// Where camera profiles are read from and written to.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("LIGHTCRAFT_CAMERA_PROFILES").map(PathBuf::from).or_else(|| config_dir().map(|d| d.join("camera-profiles")))
}

/// File name of `model`'s profile: letters, digits, `-` and `_` kept, anything else `_`.
pub fn file_name(model: &str) -> String {
    let name: String = model.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{name}.json")
}

/// Read and validate a profile file.
pub fn load(path: &Path) -> Result<CameraProfile, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > MAX_FILE {
        return Err(format!("{}: {size} bytes is too large for a camera profile", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes, &path.display().to_string())
}

/// Parse and validate a profile's JSON (`origin` names it in errors).
pub(crate) fn parse(json: &[u8], origin: &str) -> Result<CameraProfile, String> {
    let profile: CameraProfile = serde_json::from_slice(json).map_err(|e| format!("{origin}: {e}"))?;
    if !profile.valid() {
        return Err(format!("{origin}: not a usable camera profile (version {}, model {:?})", profile.version, profile.model));
    }
    Ok(profile)
}

/// The built-in profile for `model`, if any.
fn bundled(model: &str) -> Option<CameraProfile> {
    let (_, json) = BUNDLED.iter().find(|(m, _)| *m == model)?;
    parse(json.as_bytes(), &format!("built-in profile {model}")).inspect_err(|e| eprintln!("lightcraft: ignoring camera profile {e}")).ok()
}

/// Write `profile` to `dir` (created if needed) as `<model>.json`; returns the path.
pub fn save(profile: &CameraProfile, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file_name(&profile.model));
    let json = serde_json::to_vec_pretty(profile).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Profiles read so far, and the profile database generation they were read at.
type Cache = (u64, HashMap<String, Option<Arc<CameraProfile>>>);

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The profile for camera `model`: from [`dir`], else from the profile database, else built in; read once per process
/// (and again after the profile database changes).
pub fn get(model: &str) -> Option<Arc<CameraProfile>> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    let generation = crate::profile_db::generation();
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if cache.0 != generation {
        *cache = (generation, HashMap::new());
    }
    if let Some(hit) = cache.1.get(model) {
        return hit.clone();
    }
    let path = dir().map(|d| d.join(file_name(model)));
    let profile = match path {
        Some(path) if path.is_file() => match load(&path) {
            Ok(p) if p.model.trim() == model => Some(Arc::new(p)),
            Ok(p) => {
                eprintln!("lightcraft: ignoring camera profile {}: it is for {:?}, not {model:?}", path.display(), p.model);
                None
            }
            Err(e) => {
                eprintln!("lightcraft: ignoring camera profile {e}");
                None
            }
        },
        _ => None,
    };
    let profile = profile.or_else(|| crate::profile_db::installed().cameras.get(model).cloned()).or_else(|| bundled(model).map(Arc::new));
    cache.1.insert(model.to_owned(), profile.clone());
    profile
}

/// Changes when the profiles folder's contents change (per process) or the profile database's camera profiles do:
/// part of the render cache keys, so thumbnails rendered before a profile existed are not reused after.
pub fn cache_key() -> u64 {
    static KEY: OnceLock<u64> = OnceLock::new();
    // (0 without database camera profiles: the key is what it was before the database existed)
    *KEY.get_or_init(|| cache_key_for(dir().as_deref())) ^ crate::profile_db::camera_key()
}

fn cache_key_for(directory: Option<&Path>) -> u64 {
    let entries = directory.and_then(|d| std::fs::read_dir(d).ok());
    let mut files: Vec<(String, u64, std::time::Duration)> = entries
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), meta.len(), modified))
        })
        .collect();
    files.sort();
    let mut h = lightcraft_preview::Hasher128::new();
    for (name, len, modified) in &files {
        h.str(name).u64(*len).u64(modified.as_secs()).u64(u64::from(modified.subsec_nanos()));
    }
    for (model, json) in BUNDLED.iter().chain(NEUTRAL_BUNDLED) {
        h.str(model).str(json);
    }
    h.finish().0 as u64
}

/// Per camera model: the photos read and their pooled colour pairs.
#[derive(Default)]
pub struct Pool {
    models: HashMap<String, (usize, Vec<([f64; 3], [f64; 3])>)>,
}

/// `pairs` in a fixed order: pools filled on several threads arrive in a different order each run, and the fits hold
/// out pairs by position, so without it the same photos gave a different table every time.
fn in_order(pairs: &[([f64; 3], [f64; 3])]) -> Vec<([f64; 3], [f64; 3])> {
    let mut sorted = pairs.to_vec();
    sorted.sort_by(|a, b| {
        a.0.iter().chain(&a.1).zip(b.0.iter().chain(&b.1)).map(|(x, y)| x.total_cmp(y)).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)
    });
    sorted
}

/// Most colour pairs kept per photo, so a few busy photos can't dominate a profile.
const PAIRS_PER_FILE: usize = 4000;

impl Pool {
    /// Add one raw file's colour pairs. `Ok(None)` when the file can't contribute (not a vendor
    /// format handled by `camera_preview::file_local_look`, has its own colour matrices, no usable
    /// camera JPEG, or too little colour).
    pub fn add(&mut self, bytes: &[u8]) -> Result<Option<String>, String> {
        let mut raw = lightcraft_raw::decode(bytes).map_err(|e| e.to_string())?;
        // colour only: geometric lens corrections would stop the sensor proxy from binning (the pairs still follow
        // the warp the camera applied to its JPEG)
        let lens = crate::camera_preview::lens_geometry(&raw.opcodes.list3);
        raw.opcodes.list3.retain(|op| !op.is_lens_correction());
        let Some(model) = raw.metadata.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) else { return Ok(None) };
        let Some(pairs) = crate::camera_preview::profile_pairs(&raw, bytes, lens.as_ref()) else { return Ok(None) };
        let step = pairs.len().div_ceil(PAIRS_PER_FILE).max(1);
        let entry = self.models.entry(model.to_owned()).or_default();
        entry.0 += 1;
        entry.1.extend(pairs.into_iter().step_by(step));
        Ok(Some(model.to_owned()))
    }

    /// Add one raw file's neutral-look pairs against `reference`, a render of the same photo by another raw converter
    /// (`camera_preview::neutral_pairs`). `Ok(None)` when the file can't contribute (not a CR3 of a model with
    /// measured matrices, a render that doesn't line up).
    pub fn add_reference(&mut self, bytes: &[u8], reference: &[u8]) -> Result<Option<String>, String> {
        let mut raw = lightcraft_raw::decode(bytes).map_err(|e| e.to_string())?;
        raw.opcodes.list3.retain(|op| !op.is_lens_correction());
        let Some(model) = raw.metadata.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned) else { return Ok(None) };
        let Some(pairs) = crate::camera_preview::neutral_pairs(&mut raw, reference) else { return Ok(None) };
        let step = pairs.len().div_ceil(PAIRS_PER_FILE).max(1);
        let entry = self.models.entry(model.clone()).or_default();
        entry.0 += 1;
        entry.1.extend(pairs.into_iter().step_by(step));
        Ok(Some(model))
    }

    /// Fit a neutral-look table per model with at least `min_files` photos (pools filled by [`Pool::add_reference`]).
    pub fn fit_neutral(&self, min_files: usize) -> Vec<Result<NeutralTable, String>> {
        let mut models: Vec<_> = self.models.iter().collect();
        models.sort_by(|a, b| a.0.cmp(b.0));
        models
            .into_iter()
            .filter(|(_, (files, _))| *files >= min_files)
            .map(|(model, (files, pairs))| {
                let hue_sat =
                    crate::camera_preview::fit_neutral_table(&in_order(pairs)).ok_or_else(|| format!("{model}: no usable hue/saturation fit"))?;
                Ok(NeutralTable { version: VERSION, model: model.clone(), files: *files, samples: pairs.len(), hue_sat })
            })
            .collect()
    }

    /// Fit a profile per model with at least `min_files` photos.
    pub fn fit(&self, min_files: usize) -> Vec<Result<CameraProfile, String>> {
        let mut models: Vec<_> = self.models.iter().collect();
        models.sort_by(|a, b| a.0.cmp(b.0));
        models
            .into_iter()
            .filter(|(_, (files, _))| *files >= min_files)
            .map(|(model, (files, pairs))| {
                let (matrix, hue_sat) =
                    crate::camera_preview::fit_profile(&in_order(pairs)).ok_or_else(|| format!("{model}: no usable colour fit"))?;
                Ok(CameraProfile::new(model, *files, pairs.len(), matrix, hue_sat))
            })
            .collect()
    }

    /// Add another pool's photos and pairs (pools filled on separate threads).
    pub fn merge(&mut self, other: Pool) {
        for (model, (files, pairs)) in other.models {
            let entry = self.models.entry(model).or_default();
            entry.0 += files;
            entry.1.extend(pairs);
        }
    }

    /// Photos read per model.
    pub fn files(&self) -> Vec<(String, usize)> {
        let mut v: Vec<_> = self.models.iter().map(|(m, (n, _))| (m.clone(), *n)).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> CameraProfile {
        let table = HsvTable { hue_divisions: 4, sat_divisions: 2, val_divisions: 1, data: vec![[5.0, 1.2, 1.0]; 8], srgb_value: false };
        CameraProfile::new("ILCE-7M4", 12, 3456, Mat3([[1.6, -0.5, -0.1], [-0.2, 1.3, -0.1], [-0.1, -0.2, 1.3]]), Some(table))
    }

    /// The same photos give the same fit whatever order their pairs were pooled in.
    #[test]
    fn pooled_pairs_are_fitted_in_a_fixed_order() {
        let pairs: Vec<([f64; 3], [f64; 3])> = (0..50).map(|i| ([i as f64, 0.5, 1.0 / (i + 1) as f64], [0.1 * i as f64, 0.2, 0.3])).collect();
        let mut shuffled = pairs.clone();
        shuffled.reverse();
        shuffled.swap(3, 40);
        assert_eq!(in_order(&pairs), in_order(&shuffled));
    }

    #[test]
    fn neutral_tables_round_trip_and_the_bundled_ones_load() {
        let dir = std::env::temp_dir().join(format!("lc-neutral-tables-{}", std::process::id()));
        let table = NeutralTable { version: VERSION, model: "Canon EOS R6".into(), files: 3, samples: 99, hue_sat: profile().hue_sat.unwrap() };
        let path = save_neutral(&table, &dir).unwrap();
        assert_eq!(path.file_name().unwrap(), "Canon_EOS_R6.neutral.json");
        assert_eq!(parse_neutral(&std::fs::read(&path).unwrap(), "test").unwrap(), table);
        // a table whose shape doesn't match its data, or a newer version, is refused
        let mut bad = table.clone();
        bad.hue_sat.data.pop();
        assert!(parse_neutral(&serde_json::to_vec(&bad).unwrap(), "test").is_err());
        let newer = NeutralTable { version: VERSION + 1, ..table };
        assert!(parse_neutral(&serde_json::to_vec(&newer).unwrap(), "test").is_err());
        assert!(parse_neutral(b"{not json", "test").is_err());
        std::fs::remove_dir_all(&dir).ok();
        for (model, json) in NEUTRAL_BUNDLED {
            let t = parse_neutral(json.as_bytes(), model).unwrap();
            assert_eq!(t.model, *model);
            assert_eq!((t.hue_sat.hue_divisions, t.hue_sat.sat_divisions, t.hue_sat.val_divisions), (72, 5, 5));
        }
    }

    #[test]
    fn profiles_round_trip_and_reject_bad_files() {
        let dir = std::env::temp_dir().join(format!("lc-camera-profiles-{}", std::process::id()));
        let path = save(&profile(), &dir).unwrap();
        assert_eq!(path.file_name().unwrap(), "ILCE-7M4.json");
        assert_eq!(load(&path).unwrap(), profile());
        let bad = |p: CameraProfile| {
            let path = dir.join("bad.json");
            std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
            load(&path).is_err()
        };
        let mut p = profile();
        p.matrix[0][0] = f64::NAN;
        assert!(bad(p), "non-finite matrix");
        let mut p = profile();
        p.matrix = [[0.0; 3]; 3];
        assert!(bad(p), "singular matrix");
        let mut p = profile();
        if let Some(t) = p.hue_sat.as_mut() {
            t.data.truncate(3);
        }
        assert!(bad(p), "table data doesn't match its shape");
        let mut p = profile();
        p.version = 99;
        assert!(bad(p), "unknown version");
        std::fs::write(dir.join("junk.json"), b"{not json").unwrap();
        assert!(load(&dir.join("junk.json")).is_err());
        assert!(load(&dir.join("missing.json")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_profiles_are_valid_and_named_after_their_model() {
        assert!(!BUNDLED.is_empty());
        for (model, json) in BUNDLED {
            let p = parse(json.as_bytes(), model).unwrap();
            assert_eq!(p.model, *model);
            assert!(p.files >= 5 && p.hue_sat.is_some(), "{model}: {} photos", p.files);
            assert_eq!(bundled(model), Some(p));
        }
        assert!(bundled("No Such Camera").is_none());
    }

    #[test]
    fn same_second_profile_replacement_changes_render_cache_key() {
        let dir = std::env::temp_dir().join(format!("lc-camera-profile-cache-{}", std::process::id()));
        let mut first = profile();
        first.model = "ILCE-7CR".into();
        let path = save(&first, &dir).unwrap();
        let set_modified = |millis| {
            let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000) + std::time::Duration::from_millis(millis);
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
            file.metadata().unwrap()
        };
        let before = set_modified(100);
        assert_eq!(load(&path).unwrap(), first);
        let before_key = cache_key_for(Some(&dir));

        let mut second = first;
        second.matrix[0][0] = 1.7;
        save(&second, &dir).unwrap();
        let after = set_modified(350);
        assert_eq!(load(&path).unwrap(), second);
        assert_eq!(before.len(), after.len());
        let before_time = before.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
        let after_time = after.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
        assert_eq!(before_time.as_secs(), after_time.as_secs());
        assert_ne!(before_time.subsec_nanos(), after_time.subsec_nanos());
        let after_key = cache_key_for(Some(&dir));
        let _ = std::fs::remove_dir_all(dir);

        assert_ne!(before_key, after_key, "a changed profile must not reuse the previous render cache");
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("ILCE-7M4"), "ILCE-7M4.json");
        assert_eq!(file_name(" DSC-RX100M3 "), "DSC-RX100M3.json");
        assert_eq!(file_name("../../etc/passwd"), "______etc_passwd.json");
        assert_eq!(file_name("Ω 1/2"), "__1_2.json");
    }

    #[test]
    fn pool_skips_files_that_cannot_contribute() {
        let mut pool = Pool::default();
        assert!(pool.add(b"not a raw file").is_err());
        assert!(pool.fit(1).is_empty());
        assert!(pool.files().is_empty());
    }
}
