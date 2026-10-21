//! A profile database kept up to date from a server: camera colour profiles ([`crate::camera_profiles`]) and lens
//! profiles ([`crate::lens_profiles`]) published as static files, downloaded when the user (or the app, on its
//! schedule) asks, and read from the settings folder after that (`docs/profile-db.md`).
//!
//! A feed is a folder on any web server: `index.json`, the [`Manifest`], and `files/<sha256>` for each file it lists
//! (named by content, so a file never changes under its name and an update fetches only what is new). The local copy
//! ([`Store`], `<settings>/profile-db` or `LIGHTCRAFT_PROFILE_DB`) keeps the installed `index.json` and the files it
//! lists; an update downloads the files it lacks, checks each one's size and SHA-256, then replaces `index.json` in one
//! rename, so a reader sees the old set or the new one and never a mix. A feed's files are data (lensfun-format XML,
//! camera profile JSON), parsed with the same bounds as files the user puts in place; nothing in a feed is code.
//!
//! What a feed holds and where it is published is its publisher's choice: lensfun's database is CC-BY-SA 3.0 and may be
//! republished with its credit, which the manifest's [`Source`]s carry for the app to show. Adobe lens or camera
//! profiles are never used.
//!
//! Sources: own design.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_lensdb::LensDb;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::camera_profiles::CameraProfile;

/// The manifest format this build reads.
pub const FORMAT: u32 = 1;
/// The environment variable naming the local copy's folder (default: `<settings>/profile-db`).
pub const ENV_DIR: &str = "LIGHTCRAFT_PROFILE_DB";
/// The environment variable naming a feed's address, for frontends that don't pass one.
pub const ENV_URL: &str = "LIGHTCRAFT_PROFILE_DB_URL";
/// A lensfun-format XML file ([`lightcraft_lensdb::LensDb::parse`]).
pub const KIND_LENS: &str = "lens";
/// A camera colour profile ([`crate::camera_profiles::CameraProfile`]).
pub const KIND_CAMERA: &str = "camera";

/// Largest `index.json` read.
pub const MAX_INDEX: u64 = 1 << 20;
/// Most files one manifest lists.
const MAX_FILES: usize = 4000;
/// Most sources one manifest credits.
const MAX_SOURCES: usize = 64;
/// Largest file (lensfun's biggest is well under 1 MB; a camera profile is ~60 KB).
const MAX_FILE: u64 = 16 << 20;
/// Largest feed: the sum of its files.
const MAX_TOTAL: u64 = 256 << 20;
/// Longest text field.
const MAX_TEXT: usize = 1024;

/// A feed's `index.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: u32,
    /// Grows with every publication; a device never installs a smaller one than it has.
    pub serial: u64,
    /// When it was published (ISO 8601), for display.
    #[serde(default)]
    pub published: String,
    /// Where the files come from, with their licences and credits.
    #[serde(default)]
    pub sources: Vec<Source>,
    pub files: Vec<Entry>,
}

/// One origin of a feed's files, credited in the app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub title: String,
    /// SPDX-style licence, e.g. `CC-BY-SA-3.0`.
    pub licence: String,
    #[serde(default)]
    pub url: String,
    /// The credit the licence asks for, e.g. "© the lensfun contributors".
    #[serde(default)]
    pub credit: String,
    /// The upstream revision the files were taken from.
    #[serde(default)]
    pub revision: String,
}

/// One file of a feed: served as `files/<sha256>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// [`KIND_LENS`] or [`KIND_CAMERA`]; other kinds (a newer feed's) are skipped.
    pub kind: String,
    /// The file's name at its source, for display (`mil-sony.xml`, `ILCE-7M4.json`).
    pub name: String,
    /// A camera profile's camera model (Exif `Model`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The [`Source`] it comes from.
    pub source: String,
    /// SHA-256 of the file, lower-case hex.
    pub sha256: String,
    pub size: u64,
}

impl Entry {
    /// Whether this build reads entries of its kind.
    pub fn known(&self) -> bool {
        self.kind == KIND_LENS || self.kind == KIND_CAMERA
    }
}

fn short(s: &str) -> bool {
    s.len() <= MAX_TEXT
}

fn sha_ok(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Manifest {
    /// Parse and check an `index.json`: its format, bounds, hashes and references.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, String> {
        if bytes.len() as u64 > MAX_INDEX {
            return Err(format!("the profile index is {} bytes, more than the {MAX_INDEX} allowed", bytes.len()));
        }
        // the format first, so a newer feed says "update the app" rather than failing on a field this build lacks
        #[derive(Deserialize)]
        struct Head {
            format: u32,
        }
        let head: Head = serde_json::from_slice(bytes).map_err(|e| format!("the profile index is not valid: {e}"))?;
        if head.format != FORMAT {
            return Err(format!("the profile index has format {}; this version of the app reads format {FORMAT} (update the app)", head.format));
        }
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("the profile index is not valid: {e}"))?;
        m.check()?;
        Ok(m)
    }

    fn check(&self) -> Result<(), String> {
        if self.files.len() > MAX_FILES || self.sources.len() > MAX_SOURCES {
            return Err(format!("the profile index lists {} files and {} sources, more than allowed", self.files.len(), self.sources.len()));
        }
        if !short(&self.published) {
            return Err("the profile index's date is too long".into());
        }
        for s in &self.sources {
            if s.id.is_empty() || ![&s.id, &s.title, &s.licence, &s.url, &s.credit, &s.revision].into_iter().all(|t| short(t)) {
                return Err(format!("the profile index has an invalid source {:?}", s.id));
            }
        }
        let mut total: u64 = 0;
        let mut models: HashMap<&str, ()> = HashMap::new();
        for e in &self.files {
            let what = format!("{:?}", e.name);
            if e.name.is_empty() || !short(&e.name) || e.name.chars().any(char::is_control) || !short(&e.kind) {
                return Err(format!("the profile index has a file with an invalid name {what}"));
            }
            if !sha_ok(&e.sha256) {
                return Err(format!("the profile index has an invalid SHA-256 for {what}"));
            }
            if e.size == 0 || e.size > MAX_FILE {
                return Err(format!("the profile index gives {what} a size of {} bytes", e.size));
            }
            if !self.sources.iter().any(|s| s.id == e.source) {
                return Err(format!("the profile index names no source {:?} for {what}", e.source));
            }
            total = total.checked_add(e.size).filter(|t| *t <= MAX_TOTAL).ok_or("the profile database is larger than allowed")?;
            if e.kind == KIND_CAMERA {
                let model = e.model.as_deref().map(str::trim).filter(|m| !m.is_empty() && short(m));
                let Some(model) = model else { return Err(format!("the camera profile {what} names no camera model")) };
                if models.insert(model, ()).is_some() {
                    return Err(format!("the profile index has two camera profiles for {model:?}"));
                }
            }
        }
        Ok(())
    }

    /// The entries this build reads.
    pub fn known(&self) -> impl Iterator<Item = &Entry> {
        self.files.iter().filter(|e| e.known())
    }
}

/// SHA-256 of `bytes`, lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The installed database, read into memory.
#[derive(Debug, Default)]
pub struct Installed {
    pub manifest: Option<Manifest>,
    pub lenses: LensDb,
    /// Camera profiles by model.
    pub cameras: HashMap<String, Arc<CameraProfile>>,
    /// Changes when any camera profile does (a lens-only update leaves rendered pictures alone).
    pub camera_key: u64,
    /// Entries that could not be used, and why.
    pub skipped: Vec<String>,
    /// Bytes of the files read.
    pub bytes: u64,
}

impl Installed {
    pub fn serial(&self) -> u64 {
        self.manifest.as_ref().map_or(0, |m| m.serial)
    }
}

/// A local copy of a feed: `index.json` and `files/<sha256>`.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

/// Read a file of at most `max` bytes.
fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if len > max {
        return Err(format!("{}: {len} bytes is more than the {max} allowed", path.display()));
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

impl Store {
    pub fn at(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    /// The folder files are downloaded into.
    pub fn files_dir(&self) -> PathBuf {
        self.dir.join("files")
    }

    fn file_path(&self, e: &Entry) -> PathBuf {
        self.files_dir().join(&e.sha256)
    }

    /// The installed manifest; `None` when nothing is installed.
    pub fn manifest(&self) -> Result<Option<Manifest>, String> {
        let path = self.index_path();
        if !path.is_file() {
            return Ok(None);
        }
        Manifest::parse(&read_capped(&path, MAX_INDEX)?).map(Some)
    }

    /// Whether the file of `e` is in place with its size (its hash is checked when it is read).
    pub fn has(&self, e: &Entry) -> bool {
        std::fs::metadata(self.file_path(e)).is_ok_and(|m| m.is_file() && m.len() == e.size)
    }

    /// The entries of `m` this build reads whose files are not in place.
    pub fn missing<'a>(&self, m: &'a Manifest) -> Vec<&'a Entry> {
        let mut seen = std::collections::HashSet::new();
        m.known().filter(|e| seen.insert(e.sha256.as_str()) && !self.has(e)).collect()
    }

    /// Make `m` (whose `index.json` is `index`) the installed set: every file it lists this build reads must be in
    /// place with its SHA-256; then `index.json` is replaced in one rename and files nothing lists any more are
    /// deleted.
    pub fn install(&self, index: &[u8], m: &Manifest) -> Result<(), String> {
        for e in m.known() {
            let bytes = read_capped(&self.file_path(e), MAX_FILE)?;
            if bytes.len() as u64 != e.size || sha256_hex(&bytes) != e.sha256 {
                let _ = std::fs::remove_file(self.file_path(e));
                return Err(format!("{} does not match the profile index", e.name));
            }
        }
        lightcraft_catalog::safe_file::write_atomic(&self.index_path(), index).map_err(|e| format!("{}: {e}", self.index_path().display()))?;
        self.collect_garbage(m);
        Ok(())
    }

    /// Delete the files `m` doesn't list (and interrupted downloads).
    fn collect_garbage(&self, m: &Manifest) {
        let keep: std::collections::HashSet<&str> = m.known().map(|e| e.sha256.as_str()).collect();
        let Ok(entries) = std::fs::read_dir(self.files_dir()) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !keep.contains(name.as_ref()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Delete the local copy.
    pub fn remove(&self) -> Result<(), String> {
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", self.dir.display())),
        }
    }

    /// Read the installed database. Files that are missing, damaged or unreadable are left out and listed in
    /// [`Installed::skipped`]; only an unreadable `index.json` is an error.
    pub fn load(&self) -> Result<Installed, String> {
        let Some(manifest) = self.manifest()? else { return Ok(Installed::default()) };
        let mut out = Installed { camera_key: camera_key_of(&manifest), ..Installed::default() };
        for e in manifest.known() {
            let bytes = match read_capped(&self.file_path(e), MAX_FILE) {
                Ok(b) if sha256_hex(&b) == e.sha256 => b,
                Ok(_) => {
                    out.skipped.push(format!("{}: does not match the profile index", e.name));
                    continue;
                }
                Err(err) => {
                    out.skipped.push(format!("{}: {err}", e.name));
                    continue;
                }
            };
            out.bytes = out.bytes.saturating_add(bytes.len() as u64);
            if e.kind == KIND_LENS {
                match std::str::from_utf8(&bytes).map_err(|err| err.to_string()).and_then(|xml| LensDb::parse(xml).map_err(|err| err.to_string())) {
                    Ok(db) => out.lenses.extend(db),
                    Err(err) => out.skipped.push(format!("{}: {err}", e.name)),
                }
            } else if let Some(model) = e.model.as_deref().map(str::trim) {
                match crate::camera_profiles::parse(&bytes, &e.name) {
                    Ok(p) if p.model.trim() == model => {
                        out.cameras.insert(model.to_owned(), Arc::new(p));
                    }
                    Ok(p) => out.skipped.push(format!("{}: it is for {:?}, not {model:?}", e.name, p.model)),
                    Err(err) => out.skipped.push(err),
                }
            }
        }
        out.manifest = Some(manifest);
        Ok(out)
    }
}

/// A key that changes when any of `m`'s camera profiles does; 0 when it has none.
fn camera_key_of(m: &Manifest) -> u64 {
    let mut key = lightcraft_preview::Hasher128::new();
    let mut any = false;
    for e in m.known().filter(|e| e.kind == KIND_CAMERA) {
        key.str(e.model.as_deref().unwrap_or_default().trim()).str(&e.sha256);
        any = true;
    }
    if any { key.finish().0 as u64 } else { 0 }
}

/// The local copy's folder: `LIGHTCRAFT_PROFILE_DB`, else `<settings>/profile-db`.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os(ENV_DIR).filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| crate::config::config_dir().map(|d| d.join("profile-db")))
}

/// Bumped whenever the installed database changes in this process.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The installed database as last read, and the generation it was read at.
fn loaded() -> &'static Mutex<Option<(u64, Arc<Installed>)>> {
    static LOADED: Mutex<Option<(u64, Arc<Installed>)>> = Mutex::new(None);
    &LOADED
}

/// Changes whenever the installed database does (an update or a removal in this process).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// The installed database ([`dir`]), read on first use and again after it changes; empty when there is none.
pub fn installed() -> Arc<Installed> {
    let generation = generation();
    let mut slot = loaded().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((g, db)) = slot.as_ref()
        && *g == generation
    {
        return db.clone();
    }
    let db = match dir().map(Store::at).map(|s| s.load()) {
        Some(Ok(db)) => {
            for why in &db.skipped {
                log::warn!("profile database: skipped {why}");
            }
            db
        }
        Some(Err(e)) => {
            log::warn!("profile database: {e}");
            Installed::default()
        }
        None => Installed::default(),
    };
    let db = Arc::new(db);
    *slot = Some((generation, db.clone()));
    db
}

/// [`Installed::camera_key`] of the installed database, from its `index.json` alone (render cache keys ask often;
/// reading the lens files is left to the first lookup that needs them).
pub fn camera_key() -> u64 {
    static KEY: Mutex<Option<(u64, u64)>> = Mutex::new(None);
    let generation = generation();
    let mut slot = KEY.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((g, key)) = *slot
        && g == generation
    {
        return key;
    }
    let key = dir().map(Store::at).and_then(|s| s.manifest().ok().flatten()).map_or(0, |m| camera_key_of(&m));
    *slot = Some((generation, key));
    key
}

/// Read the database again: after an update or a removal.
pub fn changed() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    *loaded().lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// A feed's base address from what the user or the app gives: `https://…` (or plain http to this machine's loopback
/// address, a test server), with a trailing `/` or `/index.json` dropped. `None` when it is not one.
pub fn feed_base(url: &str) -> Option<String> {
    let url = url.trim();
    let url = url.strip_suffix("/index.json").unwrap_or(url).trim_end_matches('/');
    let loopback = url.strip_prefix("http://127.0.0.1").is_some_and(|rest| rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'));
    let https = url.strip_prefix("https://").is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'));
    ((https || loopback) && url.len() <= MAX_TEXT && !url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '?' || c == '#'))
        .then(|| url.to_string())
}

/// What the last update did, kept next to the database (`feed.json`) for the settings page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedState {
    /// The feed last updated from.
    #[serde(default)]
    pub url: String,
    /// When the feed was last checked (seconds since 1970), whatever came of it.
    #[serde(default)]
    pub checked: u64,
    /// When something new was last installed.
    #[serde(default)]
    pub installed: u64,
    /// The last check's error, if it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Store {
    fn state_path(&self) -> PathBuf {
        self.dir.join("feed.json")
    }

    pub fn state(&self) -> FeedState {
        read_capped(&self.state_path(), 64 * 1024).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save_state(&self, state: &FeedState) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let json = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
        lightcraft_catalog::safe_file::write_atomic(&self.state_path(), &json).map_err(|e| format!("{}: {e}", self.state_path().display()))
    }
}

/// How an update ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Nothing new: the installed serial is the feed's.
    UpToDate {
        serial: u64,
    },
    /// A new set is installed: `files` downloaded, `bytes` in all.
    Updated {
        serial: u64,
        files: usize,
        bytes: u64,
    },
    /// Failed, in words the user can act on.
    Failed(String),
    Cancelled,
}

/// One folder of files to publish ([`build`]): lensfun-format `*.xml` ([`KIND_LENS`]) or camera profiles `*.json`
/// ([`KIND_CAMERA`], as `lightcraft-cli calibrate` writes them), all from `source`.
#[derive(Clone, Debug)]
pub struct Input {
    pub source: Source,
    pub kind: &'static str,
    pub dir: PathBuf,
}

/// The credit for a feed that republishes lensfun's database (CC-BY-SA 3.0; <https://lensfun.github.io/>).
pub fn lensfun_source(revision: &str) -> Source {
    Source {
        id: "lensfun".into(),
        title: "lensfun lens database".into(),
        licence: "CC-BY-SA-3.0".into(),
        url: "https://github.com/lensfun/lensfun".into(),
        credit: "Lens profiles © the lensfun contributors, CC BY-SA 3.0".into(),
        revision: revision.into(),
    }
}

/// Now, UTC, as `YYYY-MM-DDTHH:MM:SSZ` (a feed's default publication date).
pub fn now_utc() -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{}Z", lightcraft_catalog::dates::civil(i64::try_from(secs).unwrap_or(0)))
}

/// What [`build`] wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Built {
    pub serial: u64,
    pub files: usize,
    pub bytes: u64,
    pub lenses: usize,
    pub cameras: Vec<String>,
    /// Files new in `files/`.
    pub added: usize,
    /// Files removed from `files/` (with `prune`).
    pub pruned: usize,
    /// Files in the input folders that are not published, and why.
    pub skipped: Vec<String>,
}

/// Write a feed into `out` (`index.json`, `files/<sha256>`) from `inputs`, each file checked with the parsers the app
/// uses. `serial` defaults to the one `out` has plus 1 and must grow. Files of earlier publications stay in `files/`
/// unless `prune` (a device that read the old index a moment ago can still fetch them).
pub fn build(out: &Path, inputs: &[Input], serial: Option<u64>, published: &str, prune: bool) -> Result<Built, String> {
    let feed = Store::at(out);
    let previous = feed.manifest().map_err(|e| format!("{}: {e}", out.display()))?.map_or(0, |m| m.serial);
    let serial = serial.unwrap_or(previous.saturating_add(1));
    if serial <= previous {
        return Err(format!("the serial must grow: {} has {previous}", out.display()));
    }
    let files_dir = feed.files_dir();
    std::fs::create_dir_all(&files_dir).map_err(|e| format!("{}: {e}", files_dir.display()))?;
    let mut m = Manifest { format: FORMAT, serial, published: published.into(), sources: Vec::new(), files: Vec::new() };
    let mut built = Built { serial, ..Built::default() };
    for input in inputs {
        if !m.sources.iter().any(|s| s.id == input.source.id) {
            m.sources.push(input.source.clone());
        }
        let ext = if input.kind == KIND_LENS { "xml" } else { "json" };
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&input.dir)
            .map_err(|e| format!("{}: {e}", input.dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
            .collect();
        paths.sort();
        let before = m.files.len();
        for path in paths {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let bytes = read_capped(&path, MAX_FILE)?;
            let model = if input.kind == KIND_LENS {
                let xml = std::str::from_utf8(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                let db = LensDb::parse(xml).map_err(|e| format!("{}: {e}", path.display()))?;
                if db.is_empty() {
                    built.skipped.push(format!("{name}: no cameras or lenses"));
                    continue;
                }
                built.lenses += db.lenses.len();
                None
            } else if name.ends_with(".neutral.json") {
                built.skipped.push(format!("{name}: neutral-look tables are not published"));
                continue;
            } else {
                let p = crate::camera_profiles::parse(&bytes, &path.display().to_string())?;
                built.cameras.push(p.model.trim().to_owned());
                Some(p.model.trim().to_owned())
            };
            let sha = sha256_hex(&bytes);
            let target = files_dir.join(&sha);
            if !target.is_file() {
                lightcraft_catalog::safe_file::write_atomic(&target, &bytes).map_err(|e| format!("{}: {e}", target.display()))?;
                built.added += 1;
            }
            built.bytes += bytes.len() as u64;
            m.files.push(Entry { kind: input.kind.into(), name, model, source: input.source.id.clone(), sha256: sha, size: bytes.len() as u64 });
        }
        if m.files.len() == before {
            return Err(format!("{}: no {ext} files to publish", input.dir.display()));
        }
    }
    let index = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    // the app's own checks, so a feed it would refuse is never written
    Manifest::parse(&index)?;
    lightcraft_catalog::safe_file::write_atomic(&out.join("index.json"), &index).map_err(|e| format!("{}: {e}", out.display()))?;
    if prune {
        let keep: std::collections::HashSet<&str> = m.files.iter().map(|e| e.sha256.as_str()).collect();
        for entry in std::fs::read_dir(&files_dir).into_iter().flatten().flatten() {
            if !keep.contains(entry.file_name().to_string_lossy().as_ref()) && std::fs::remove_file(entry.path()).is_ok() {
                built.pruned += 1;
            }
        }
    }
    built.files = m.files.len();
    built.cameras.sort();
    Ok(built)
}

#[cfg(not(target_arch = "wasm32"))]
pub use fetch::update;

#[cfg(not(target_arch = "wasm32"))]
mod fetch {
    use std::sync::atomic::AtomicBool;

    use super::*;

    /// Check the feed at `base` ([`feed_base`]) and install what is new into `store`: `index.json` first, then each
    /// file it lists that `store` lacks, checked against its size and SHA-256, then the new set in one step
    /// ([`Store::install`]). A feed older than the installed set (a smaller serial) is refused. `progress` gets bytes
    /// done and expected.
    pub fn update(store: &Store, base: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<Outcome, String> {
        let base = feed_base(base).ok_or("the profile database address must start with https://")?;
        let staging = store.dir().join(".staging");
        std::fs::create_dir_all(&staging).map_err(|e| format!("could not create {}: {e}", staging.display()))?;
        let index_name = "index.json";
        let _ = std::fs::remove_file(staging.join(index_name));
        let opts = lightcraft_fetch::Options::default();
        let index = lightcraft_fetch::FileSpec { name: index_name, size: None, sha256: None, max: MAX_INDEX };
        lightcraft_fetch::download_url(&index, &format!("{base}/index.json"), &staging, &opts, cancel, &mut |_| ()).map_err(|e| e.to_string())?;
        let bytes = read_capped(&staging.join(index_name), MAX_INDEX)?;
        let _ = std::fs::remove_dir_all(&staging);
        let m = Manifest::parse(&bytes)?;
        let have = store.manifest().ok().flatten();
        if let Some(have) = &have
            && m.serial < have.serial
        {
            return Err(format!(
                "the server's profile database (number {}) is older than the one on this device (number {}); remove the downloaded profiles to go back to it",
                m.serial, have.serial
            ));
        }
        let missing = store.missing(&m);
        if have.as_ref().is_some_and(|h| h.serial == m.serial && *h == m) && missing.is_empty() {
            return Ok(Outcome::UpToDate { serial: m.serial });
        }
        let total: u64 = missing.iter().map(|e| e.size).sum();
        let files_dir = store.files_dir();
        std::fs::create_dir_all(&files_dir).map_err(|e| format!("could not create {}: {e}", files_dir.display()))?;
        let mut done = 0u64;
        progress(0, total);
        for e in &missing {
            let spec = lightcraft_fetch::FileSpec { name: &e.sha256, size: Some(e.size), sha256: Some(&e.sha256), max: e.size };
            lightcraft_fetch::download_url(&spec, &format!("{base}/files/{}", e.sha256), &files_dir, &opts, cancel, &mut |p| {
                progress(done.saturating_add(p.done.min(e.size)), total)
            })
            .map_err(|err| match err {
                lightcraft_fetch::DownloadError::Cancelled => "cancelled".to_string(),
                err => format!("{}: {err}", e.name),
            })?;
            done = done.saturating_add(e.size);
            progress(done, total);
        }
        store.install(&bytes, &m)?;
        Ok(Outcome::Updated { serial: m.serial, files: missing.len(), bytes: total })
    }
}

#[cfg(test)]
#[path = "tests_profile_db.rs"]
mod tests;
