//! The optional AI models as one list: what each is for, whether it is on this device and how
//! much space it takes, where it downloads from, whether the user turned it off. This is what
//! Settings ▸ AI Models shows, and the `models.*` commands expose it to agents.
//!
//! The models themselves are driven by their features (`segment` for SAM 3, `enhance` for
//! Super Resolution); this layer only lists them, downloads and deletes their files (never
//! anything but the files the model is made of), and keeps the user's on / off choice.

use serde::Serialize;

use crate::segment::DownloadStatus;

/// One model as the apps show it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    /// What it does for the user ("Object and Describe masks").
    pub purpose: String,
    /// This build can use the model at all (an iPhone build has no SAM 3).
    pub available: bool,
    /// Its files are on this device.
    pub installed: bool,
    /// The user hasn't turned it off.
    pub enabled: bool,
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

/// The host of a download location ("https://user:pw@huggingface.co/a/b?x" → "huggingface.co").
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority.rsplit('@').next().unwrap_or_default().to_string()
}

#[cfg(any(feature = "sam", feature = "enhance"))]
mod imp {
    use std::path::Path;

    use lightcraft_models::ModelSpec;
    use lightcraft_models::registry::{self, ALL, SAM3};

    use super::{Deleted, ModelInfo, host_of};
    use crate::Session;
    use crate::enhance::Enhancer;
    use crate::segment::Segmenter;

    /// Bytes the model's files (and their `.part` downloads) take in `dir`.
    fn disk_bytes(dir: &Path, spec: &ModelSpec) -> u64 {
        spec.files
            .iter()
            .flat_map(|f| [f.name.to_string(), format!("{}.part", f.name)])
            .filter_map(|n| std::fs::metadata(dir.join(n)).ok())
            .filter(|m| m.is_file())
            .fold(0u64, |t, m| t.saturating_add(m.len()))
    }

    fn spec(id: &str) -> Result<&'static ModelSpec, String> {
        registry::find(id).ok_or_else(|| {
            let known: Vec<&str> = ALL.iter().map(|m| m.id).collect();
            format!("no model `{id}` (known: {})", known.join(", "))
        })
    }

    impl Session {
        /// Every model this build knows, and where it stands.
        pub fn models(&self) -> Vec<ModelInfo> {
            ALL.iter().map(|m| self.model_info(m)).collect()
        }

        fn model_info(&self, m: &ModelSpec) -> ModelInfo {
            let (available, installed, dir, mirrors, download, enabled) = if m.id == SAM3.id {
                let g = &self.segmenter;
                let d = g.download_status();
                (Segmenter::AVAILABLE, g.installed(), g.dir.clone(), g.mirrors(), (d.running || d.error.is_some()).then_some(d), !g.disabled)
            } else {
                let e = &self.enhancer;
                let (fetching, d) = e.download_status();
                let mine = fetching.as_deref() == Some(m.id);
                (
                    Enhancer::AVAILABLE,
                    e.installed(m.id),
                    e.model_dir(m.id),
                    e.mirrors(m.id),
                    (mine && (d.running || d.error.is_some())).then_some(d),
                    !e.disabled.contains(m.id),
                )
            };
            let mut sources: Vec<String> = Vec::new();
            for h in mirrors.iter().map(|u| host_of(u)).filter(|h| !h.is_empty()) {
                if !sources.contains(&h) {
                    sources.push(h);
                }
            }
            ModelInfo {
                id: m.id.into(),
                label: m.label.into(),
                purpose: m.purpose.into(),
                available,
                installed,
                enabled,
                download_bytes: m.bytes(),
                disk_bytes: dir.as_deref().map_or(0, |d| disk_bytes(d, m)),
                dir: dir.map(|d| d.display().to_string()),
                sources,
                licence: m.licence.into(),
                licence_url: m.licence_url.into(),
                credit: m.credit.into(),
                home_url: m.home_url.into(),
                download,
            }
        }

        /// Start downloading model `id` in the background (the caller has the user's consent).
        /// `Ok(false)`: it is installed or a download runs already.
        pub fn start_model_download(&self, id: &str) -> Result<bool, String> {
            if spec(id)?.id == SAM3.id { self.segmenter.start_download() } else { self.enhancer.start_download(id) }
        }

        /// Stop the download of model `id`. Whether one was running for it.
        pub fn cancel_model_download(&self, id: &str) -> bool {
            match spec(id) {
                Ok(m) if m.id == SAM3.id => self.segmenter.cancel_download(),
                Ok(m) if self.enhancer.download_status().0.as_deref() == Some(m.id) => self.enhancer.cancel_download(),
                _ => false,
            }
        }

        /// Delete model `id`'s files from this device: only the files the model is made of (and
        /// their partial downloads), never the folder's other contents. It can be downloaded
        /// again. Refused while it is downloading.
        pub fn delete_model(&self, id: &str) -> Result<Deleted, String> {
            let m = spec(id)?;
            let info = self.model_info(m);
            if info.download.as_ref().is_some_and(|d| d.running) {
                return Err(format!("{} is downloading: cancel the download first", m.label));
            }
            let dir = info.dir.map(std::path::PathBuf::from).ok_or_else(|| format!("no folder is set for {}", m.label))?;
            let mut out = Deleted::default();
            let mut failed = Vec::new();
            for f in m.files {
                for name in [f.name.to_string(), format!("{}.part", f.name)] {
                    let path = dir.join(&name);
                    let Ok(meta) = std::fs::metadata(&path) else { continue };
                    if !meta.is_file() {
                        continue;
                    }
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            out.bytes = out.bytes.saturating_add(meta.len());
                            out.files += 1;
                        }
                        Err(e) => failed.push(format!("{name}: {e}")),
                    }
                }
            }
            // the folder itself only when nothing else is in it
            let _ = std::fs::remove_dir(&dir);
            if failed.is_empty() {
                Ok(out)
            } else {
                Err(format!(
                    "couldn't delete everything ({}): is the model in use? Try again after closing the feature that uses it",
                    failed.join("; ")
                ))
            }
        }

        /// Turn model `id` on or off (what Settings ▸ AI Models toggles): while it is off, the
        /// feature that uses it says so and does nothing; its files stay.
        pub fn set_model_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
            let m = spec(id)?;
            if m.id == SAM3.id {
                self.segmenter.disabled = !enabled;
            } else if enabled {
                self.enhancer.disabled.remove(m.id);
            } else {
                self.enhancer.disabled.insert(m.id.to_string());
            }
            Ok(())
        }

        /// The ids of the models turned off, sorted.
        pub fn models_disabled(&self) -> Vec<String> {
            let mut v: Vec<String> = self.enhancer.disabled.iter().cloned().collect();
            if self.segmenter.disabled {
                v.push(SAM3.id.to_string());
            }
            v.sort();
            v
        }

        /// Set exactly these models off (the saved choice, applied at launch). Unknown ids are
        /// ignored (a model a newer version knew).
        pub fn set_models_disabled(&mut self, ids: &[String]) {
            self.segmenter.disabled = ids.iter().any(|i| i == SAM3.id);
            self.enhancer.disabled = ALL.iter().filter(|m| m.id != SAM3.id && ids.iter().any(|i| i == m.id)).map(|m| m.id.to_string()).collect();
        }
    }
}

#[cfg(not(any(feature = "sam", feature = "enhance")))]
mod imp {
    use super::{Deleted, ModelInfo};
    use crate::Session;

    const NONE: &str = "AI models are not available in this build";

    impl Session {
        pub fn models(&self) -> Vec<ModelInfo> {
            Vec::new()
        }
        pub fn start_model_download(&self, _id: &str) -> Result<bool, String> {
            Err(NONE.into())
        }
        pub fn cancel_model_download(&self, _id: &str) -> bool {
            false
        }
        pub fn delete_model(&self, _id: &str) -> Result<Deleted, String> {
            Err(NONE.into())
        }
        pub fn set_model_enabled(&mut self, _id: &str, _enabled: bool) -> Result<(), String> {
            Err(NONE.into())
        }
        pub fn models_disabled(&self) -> Vec<String> {
            Vec::new()
        }
        pub fn set_models_disabled(&mut self, _ids: &[String]) {}
    }
}
