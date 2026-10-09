//! The optional AI models as one list: what each is for, whether it is on this device and how
//! much space it takes, where it downloads from, whether the user turned it off. This is what
//! Settings → AI Models shows, and the `models.*` commands expose it to agents.
//!
//! The models themselves belong to their features: SAM 3 to `segment`, Super Resolution to
//! `enhance`, search by description, text in photos and people to `vision`. This layer only lists
//! them, starts and cancels their downloads through the feature's own functions, deletes their
//! files (never anything but the files the model is made of), and keeps the user's on / off
//! choice for the models that have one. The search, text and people models are switched on where
//! they are used (the search field, the People view), so they have no switch here.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::Session;
#[cfg(feature = "enhance")]
use crate::enhance::Enhancer;
use crate::segment::{DownloadStatus, Segmenter};

/// One model as the apps show it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    /// What it does for the user ("Object and Describe masks").
    pub purpose: String,
    /// This build and device can use the model at all (a phone has no SAM 3).
    pub available: bool,
    /// Its files are on this device.
    pub installed: bool,
    /// The user hasn't turned it off.
    pub enabled: bool,
    /// The user can turn it off here (the search, text and people models have their own switch
    /// where they are used).
    pub can_disable: bool,
    /// Size of the download.
    pub download_bytes: u64,
    /// Space it takes on this device now (a partial download counts).
    pub disk_bytes: u64,
    /// Its folder on this device.
    pub dir: Option<String>,
    /// Where it downloads from, in order: hosts only (never a path, query or credentials).
    pub sources: Vec<String>,
    pub licence: String,
    pub licence_url: String,
    pub credit: String,
    pub home_url: String,
    /// This model's download while it runs, or why the last one failed.
    pub download: Option<DownloadStatus>,
}

/// What deleting a model freed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    pub bytes: u64,
    pub files: usize,
}

/// The ids of the models whose download this layer can start (besides the enhancement ones).
pub const SAM3: &str = "sam3";
pub const SEARCH: &str = "siglip2";
pub const TEXT: &str = "ppocr";
pub const FACES: &str = "faces";

/// The host of a download location ("https://user:pw@huggingface.co/a/b?x" → "huggingface.co").
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority.rsplit('@').next().unwrap_or_default().to_string()
}

/// Unique hosts, in order.
fn hosts<'a>(urls: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in urls.into_iter().map(|u| host_of(u)).filter(|h| !h.is_empty()) {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// A download's state from the search module's JSON, kept only while it runs or failed.
fn shown(v: &Value) -> Option<DownloadStatus> {
    let d = DownloadStatus {
        running: v["running"].as_bool().unwrap_or(false),
        done: v["done"].as_u64().unwrap_or(0),
        total: v["total"].as_u64().unwrap_or(0),
        file: v["file"].as_str().unwrap_or_default().to_string(),
        error: v["error"].as_str().map(str::to_string),
        finished: v["finished"].as_bool().unwrap_or(false),
    };
    (d.running || d.error.is_some()).then_some(d)
}

/// The files a model is made of: each folder and the names in it (for its size and for deleting it).
type Files = Vec<(PathBuf, Vec<&'static str>)>;

struct Entry {
    info: ModelInfo,
    files: Files,
}

/// Bytes the model's files (and their `.part` downloads) take.
fn disk_bytes(files: &Files) -> u64 {
    files
        .iter()
        .flat_map(|(dir, names)| names.iter().flat_map(move |n| [dir.join(n), dir.join(format!("{n}.part"))]))
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter(|m| m.is_file())
        .fold(0u64, |t, m| t.saturating_add(m.len()))
}

/// What a model is, as its feature knows it.
struct Facts {
    id: &'static str,
    label: &'static str,
    purpose: &'static str,
    credit: &'static str,
    home_url: &'static str,
    licence: &'static str,
    licence_url: &'static str,
    download_bytes: u64,
    available: bool,
    installed: bool,
    enabled: bool,
    can_disable: bool,
    dir: Option<PathBuf>,
    files: Files,
    mirrors: Vec<String>,
    download: Option<DownloadStatus>,
}

impl Facts {
    fn entry(self) -> Entry {
        let disk_bytes = disk_bytes(&self.files);
        Entry {
            info: ModelInfo {
                id: self.id.into(),
                label: self.label.into(),
                purpose: self.purpose.into(),
                available: self.available,
                installed: self.installed,
                enabled: self.enabled,
                can_disable: self.can_disable,
                download_bytes: self.download_bytes,
                disk_bytes,
                dir: self.dir.map(|d| d.display().to_string()),
                sources: hosts(&self.mirrors),
                licence: self.licence.into(),
                licence_url: self.licence_url.into(),
                credit: self.credit.into(),
                home_url: self.home_url.into(),
                download: self.download,
            },
            files: self.files,
        }
    }
}

/// A folder and the files in it: `[(dir, names)]`, or nothing when there is no folder.
fn files_in(dir: Option<&Path>, names: Vec<&'static str>) -> Files {
    dir.map(|d| vec![(d.to_path_buf(), names)]).unwrap_or_default()
}

/// The folders and names of a download made of parts.
#[cfg(feature = "vision")]
fn files_of(parts: &[lightcraft_vision::models::Download]) -> Files {
    parts.iter().map(|(files, _, dir)| (dir.clone(), files.iter().map(|f| f.name).collect())).collect()
}

impl Session {
    fn sam3_entry(&self) -> Entry {
        let g = &self.segmenter;
        #[cfg(feature = "sam")]
        let names: Vec<&'static str> = lightcraft_segment::fetch::SAM3_FILES.iter().map(|f| f.name).collect();
        #[cfg(not(feature = "sam"))]
        let names: Vec<&'static str> = Vec::new();
        Facts {
            id: SAM3,
            label: "SAM 3",
            purpose: "Object and Describe masks (Masking panel)",
            credit: "SAM 3 by Meta",
            home_url: "https://github.com/facebookresearch/sam3",
            licence: crate::segment::LICENSE_NAME,
            licence_url: crate::segment::LICENSE_URL,
            download_bytes: crate::segment::MODEL_BYTES,
            available: Segmenter::AVAILABLE,
            installed: g.installed(),
            enabled: !g.disabled,
            can_disable: true,
            dir: g.dir.clone(),
            files: files_in(g.dir.as_deref(), names),
            mirrors: g.mirrors(),
            download: Some(g.download_status()).filter(|d| d.running || d.error.is_some()),
        }
        .entry()
    }

    fn search_entry(&self) -> Entry {
        let v = &self.vision;
        #[cfg(feature = "vision")]
        let names: Vec<&'static str> = lightcraft_vision::models::SIGLIP_FILES.iter().map(|f| f.name).collect();
        #[cfg(not(feature = "vision"))]
        let names: Vec<&'static str> = Vec::new();
        Facts {
            id: SEARCH,
            label: "SigLIP 2",
            purpose: "Search by description (the Describe switch on the search field)",
            credit: "SigLIP 2 by Google",
            home_url: crate::vision::LICENSE_URL,
            licence: crate::vision::LICENSE_NAME,
            licence_url: crate::vision::LICENSE_URL,
            download_bytes: crate::vision::MODEL_BYTES,
            available: v.local_available(),
            installed: v.installed(),
            enabled: true,
            can_disable: false,
            dir: v.dir.clone(),
            files: files_in(v.dir.as_deref(), names),
            mirrors: v.mirrors(),
            download: shown(&v.download_status()),
        }
        .entry()
    }

    fn text_entry(&self) -> Entry {
        let v = &self.vision;
        let dir = v.text_dir();
        #[cfg(feature = "vision")]
        let parts = dir.as_deref().map(|d| {
            let env = std::env::var(lightcraft_vision::models::MIRRORS_ENV).ok();
            lightcraft_vision::models::ocr_downloads(d, env.as_deref(), v.mirrors_file.as_deref())
        });
        #[cfg(feature = "vision")]
        let (files, mirrors) = parts.map_or((Vec::new(), Vec::new()), |p| (files_of(&p), p.into_iter().flat_map(|(_, m, _)| m).collect()));
        #[cfg(not(feature = "vision"))]
        let (files, mirrors): (Files, Vec<String>) = (Vec::new(), Vec::new());
        Facts {
            id: TEXT,
            label: "PP-OCRv6 small",
            purpose: "Search the words in photos (opt-in, People and search settings)",
            credit: "PP-OCRv6 by Baidu (PaddleOCR)",
            home_url: crate::vision::TEXT_LICENSE_URL,
            licence: crate::vision::TEXT_LICENSE_NAME,
            licence_url: crate::vision::TEXT_LICENSE_URL,
            download_bytes: crate::vision::TEXT_BYTES,
            available: v.text_available(),
            installed: v.text_installed(),
            enabled: true,
            can_disable: false,
            dir,
            files,
            mirrors,
            download: shown(&v.text_download_status()),
        }
        .entry()
    }

    fn faces_entry(&self) -> Entry {
        let v = &self.vision;
        let dir = v.faces_dir();
        #[cfg(feature = "vision")]
        let parts = dir.as_deref().map(|d| {
            let env = std::env::var(lightcraft_vision::models::MIRRORS_ENV).ok();
            lightcraft_vision::models::face_downloads(d, env.as_deref(), v.mirrors_file.as_deref())
        });
        #[cfg(feature = "vision")]
        let (files, mirrors) = parts.map_or((Vec::new(), Vec::new()), |p| (files_of(&p), p.into_iter().flat_map(|(_, m, _)| m).collect()));
        #[cfg(not(feature = "vision"))]
        let (files, mirrors): (Files, Vec<String>) = (Vec::new(), Vec::new());
        let [(yunet, yunet_url), (sface, _)] = crate::vision::FACE_LICENSES;
        let _ = sface;
        Facts {
            id: FACES,
            label: "YuNet + SFace",
            purpose: "Find faces and group them into people (opt-in, People view)",
            credit: "YuNet and SFace from the OpenCV Zoo",
            home_url: yunet_url,
            licence: yunet,
            licence_url: yunet_url,
            download_bytes: crate::vision::FACE_BYTES,
            available: v.faces_available(),
            installed: v.faces_installed(),
            enabled: true,
            can_disable: false,
            dir,
            files,
            mirrors,
            download: shown(&v.faces_download_status()),
        }
        .entry()
    }

    /// The enhancement models (Super Resolution), when this build has them.
    #[cfg(feature = "enhance")]
    fn enhance_entries(&self) -> Vec<Entry> {
        let e = &self.enhancer;
        let (fetching, d) = e.download_status();
        Enhancer::models()
            .map(|m| {
                let dir = e.model_dir(m.id);
                let mine = fetching.as_deref() == Some(m.id);
                Facts {
                    id: m.id,
                    label: m.label,
                    purpose: m.purpose,
                    credit: m.credit,
                    home_url: m.home_url,
                    licence: m.licence,
                    licence_url: m.licence_url,
                    download_bytes: m.bytes(),
                    available: Enhancer::AVAILABLE,
                    installed: e.installed(m.id),
                    enabled: !e.disabled.contains(m.id),
                    can_disable: true,
                    files: files_in(dir.as_deref(), m.files.iter().map(|f| f.name).collect()),
                    dir,
                    mirrors: e.mirrors(m.id),
                    download: Some(d.clone()).filter(|d| mine && (d.running || d.error.is_some())),
                }
                .entry()
            })
            .collect()
    }

    fn model_entries(&self) -> Vec<Entry> {
        #[allow(unused_mut)]
        let mut v = vec![self.sam3_entry(), self.search_entry(), self.text_entry(), self.faces_entry()];
        #[cfg(feature = "enhance")]
        v.extend(self.enhance_entries());
        v
    }

    /// Every model this build knows, and where it stands.
    pub fn models(&self) -> Vec<ModelInfo> {
        self.model_entries().into_iter().map(|e| e.info).collect()
    }

    fn model_entry(&self, id: &str) -> Result<Entry, String> {
        let entries = self.model_entries();
        let known: Vec<String> = entries.iter().map(|e| e.info.id.clone()).collect();
        entries.into_iter().find(|e| e.info.id == id).ok_or_else(|| format!("no model `{id}` (known: {})", known.join(", ")))
    }

    /// Start downloading model `id` in the background (the caller has the user's consent).
    /// `Ok(false)`: it is installed or a download runs already.
    pub fn start_model_download(&self, id: &str) -> Result<bool, String> {
        self.model_entry(id)?;
        match id {
            SAM3 => self.segmenter.start_download(),
            SEARCH => self.vision.start_download(),
            TEXT => self.vision.start_text_download(),
            FACES => self.vision.start_faces_download(),
            other => self.enhancer.start_download(other),
        }
    }

    /// Stop the download of model `id`. Whether one was running for it.
    pub fn cancel_model_download(&self, id: &str) -> bool {
        match id {
            SAM3 => self.segmenter.cancel_download(),
            SEARCH => self.vision.cancel_download(),
            TEXT => self.vision.cancel_text_download(),
            FACES => self.vision.cancel_faces_download(),
            other => self.enhancer.download_status().0.as_deref() == Some(other) && self.enhancer.cancel_download(),
        }
    }

    /// Delete model `id`'s files from this device: only the files the model is made of (and
    /// their partial downloads), never the folder's other contents. It can be downloaded
    /// again. Refused while it is downloading.
    pub fn delete_model(&self, id: &str) -> Result<Deleted, String> {
        let Entry { info, files } = self.model_entry(id)?;
        if info.download.as_ref().is_some_and(|d| d.running) {
            return Err(format!("{} is downloading: cancel the download first", info.label));
        }
        if files.is_empty() {
            return Err(format!("{} has no folder on this device", info.label));
        }
        let mut out = Deleted::default();
        let mut failed = Vec::new();
        for (dir, names) in &files {
            for name in names {
                for file in [name.to_string(), format!("{name}.part")] {
                    let path = dir.join(&file);
                    let Ok(meta) = std::fs::metadata(&path) else { continue };
                    if !meta.is_file() {
                        continue;
                    }
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            out.bytes = out.bytes.saturating_add(meta.len());
                            out.files += 1;
                        }
                        Err(e) => failed.push(format!("{file}: {e}")),
                    }
                }
            }
        }
        // the folders themselves only when nothing else is in them (deepest first)
        let mut dirs: Vec<&PathBuf> = files.iter().map(|(d, _)| d).collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in dirs {
            let _ = std::fs::remove_dir(d);
        }
        if failed.is_empty() {
            Ok(out)
        } else {
            Err(format!("couldn't delete everything ({}): is the model in use? Try again after closing the feature that uses it", failed.join("; ")))
        }
    }

    /// Turn model `id` on or off (what Settings → AI Models toggles): while it is off, the
    /// feature that uses it says so and does nothing; its files stay.
    pub fn set_model_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        let entry = self.model_entry(id)?;
        if !entry.info.can_disable {
            return Err(format!("{} is turned on or off where it is used (the search field, the People view), not here", entry.info.label));
        }
        if id == SAM3 {
            self.segmenter.disabled = !enabled;
        } else if enabled {
            self.enhancer.disabled.remove(id);
        } else {
            self.enhancer.disabled.insert(id.to_string());
        }
        Ok(())
    }

    /// The ids of the models turned off here, sorted.
    pub fn models_disabled(&self) -> Vec<String> {
        let mut v: Vec<String> = self.enhancer.disabled.iter().cloned().collect();
        if self.segmenter.disabled {
            v.push(SAM3.to_string());
        }
        v.sort();
        v
    }

    /// Set exactly these models off (the saved choice, applied at launch). Ids this build
    /// doesn't know (a model a newer version had) are ignored.
    pub fn set_models_disabled(&mut self, ids: &[String]) {
        self.segmenter.disabled = ids.iter().any(|i| i == SAM3);
        #[cfg(feature = "enhance")]
        {
            self.enhancer.disabled = Enhancer::models().filter(|m| ids.iter().any(|i| i == m.id)).map(|m| m.id.to_string()).collect();
        }
        #[cfg(not(feature = "enhance"))]
        self.enhancer.disabled.clear();
    }
}
