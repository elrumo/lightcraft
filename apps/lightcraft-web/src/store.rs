//! Imported originals on the web. There is no filesystem: the bytes of every photo picked or
//! dropped in the browser are stored in browser storage as `originals/<content hash>`, and the
//! catalog refers to them by a synthetic path `web/<content hash>/<file name>` (so the decoders
//! still see the file's extension).
//!
//! The engine reads files synchronously ([`FileLoader`]/[`FileProbe`]), so the main thread keeps
//! the bytes it needs in a bounded memory cache ([`Originals`]). A miss (e.g. after a reload)
//! queues an asynchronous load from storage ([`Originals::take_wanted`]) and fails for now; the
//! host prefetches the active photo's original so commands that need its pixels (auto tone,
//! export) find it. Render workers read originals from storage themselves.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lightcraft_engine::files::{load_bytes, probe_bytes};
use lightcraft_engine::media::{FileLoader, FileProbe};

pub use lightcraft_engine::catalog::sync::{PATH_PREFIX, hash_of_path, original_path};

/// Memory budget for original bytes on the main thread.
const MEM_BUDGET: usize = 768 << 20;
/// Memory for synced photos' smart and mini previews on the main thread (about 1 MB each).
const PROXY_BUDGET: usize = 128 << 20;

/// Storage key of an original.
pub fn storage_key(hash: &str) -> String {
    format!("originals/{hash}")
}

/// Hash of a file's bytes (the same as the catalog's `content_hash`).
pub fn content_hash(bytes: &[u8]) -> String {
    lightcraft_preview::hash_bytes(bytes).to_string()
}

#[derive(Default)]
struct Inner {
    /// hash → (bytes, last use).
    bytes: HashMap<String, (Arc<[u8]>, u64)>,
    total: usize,
    clock: u64,
    /// Catalog paths stored since the last [`Originals::take_pending`] (to import on the next frame).
    pending: Vec<String>,
    /// Hashes to load from storage.
    wanted: Vec<String>,
    /// Hashes being loaded.
    loading: std::collections::HashSet<String>,
    /// Hashes storage doesn't hold (a synced photo whose original wasn't downloaded): not asked
    /// for again until [`Originals::retry_absent`] says it is there.
    absent: std::collections::HashSet<String>,
    /// Smart / mini preview files by storage key (`proxies/<hash>.lcsp|.lcsm`) → (bytes, last use).
    proxies: HashMap<String, (Arc<[u8]>, u64)>,
    proxies_total: usize,
    /// Preview keys to load from storage, and the ones being loaded or not in storage.
    proxies_wanted: Vec<String>,
    proxies_loading: std::collections::HashSet<String>,
    /// Preview keys storage doesn't hold (not downloaded yet): not asked for again until sync stores them.
    proxies_absent: std::collections::HashSet<String>,
}

/// Original bytes in memory, shared between the engine hooks and the host (cheap to clone).
#[derive(Clone, Default)]
pub struct Originals(Arc<Mutex<Inner>>);

impl Originals {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keep `bytes` (hash `hash`) in memory, evicting the least recently used others if needed.
    pub fn insert(&self, hash: &str, bytes: Arc<[u8]>) {
        let mut g = self.lock();
        g.clock += 1;
        let clock = g.clock;
        g.loading.remove(hash);
        g.absent.remove(hash);
        if let Some(old) = g.bytes.insert(hash.to_string(), (bytes.clone(), clock)) {
            g.total -= old.0.len();
        }
        g.total += bytes.len();
        while g.total > MEM_BUDGET && g.bytes.len() > 1 {
            let Some(oldest) = g.bytes.iter().filter(|(h, _)| *h != hash).min_by_key(|(_, v)| v.1).map(|(h, _)| h.clone()) else { break };
            if let Some(v) = g.bytes.remove(&oldest) {
                g.total -= v.0.len();
            }
        }
    }

    /// A file was stored (bytes in storage and in memory): queue its path for import.
    pub fn added(&self, name: &str, hash: &str, bytes: Arc<[u8]>) -> String {
        self.insert(hash, bytes);
        let path = original_path(hash, name);
        self.lock().pending.push(path.clone());
        path
    }

    /// Bytes for a catalog path, if in memory. A miss for a stored original queues its load.
    pub fn get(&self, path: &str) -> Option<Arc<[u8]>> {
        let hash = hash_of_path(path)?;
        let mut g = self.lock();
        g.clock += 1;
        let clock = g.clock;
        if let Some(v) = g.bytes.get_mut(hash) {
            v.1 = clock;
            return Some(v.0.clone());
        }
        if !g.loading.contains(hash) && !g.absent.contains(hash) && !g.wanted.iter().any(|w| w == hash) {
            g.wanted.push(hash.to_string());
        }
        None
    }

    pub fn contains(&self, path: &str) -> bool {
        hash_of_path(path).is_some_and(|h| self.lock().bytes.contains_key(h))
    }

    /// Ask for the original behind `path` to be loaded into memory (no-op if it's there).
    pub fn prefetch(&self, path: &str) {
        let _ = self.get(path);
    }

    /// Hashes to load from storage (they count as loading until [`Originals::insert`] or
    /// [`Originals::load_failed`]).
    pub fn take_wanted(&self) -> Vec<String> {
        let mut g = self.lock();
        let w = std::mem::take(&mut g.wanted);
        g.loading.extend(w.iter().cloned());
        w
    }

    pub fn load_failed(&self, hash: &str) {
        self.lock().loading.remove(hash);
    }

    /// Storage doesn't hold this original (a synced photo it wasn't downloaded for): stop asking.
    pub fn load_missing(&self, hash: &str) {
        let mut g = self.lock();
        g.loading.remove(hash);
        g.absent.insert(hash.to_string());
    }

    /// Ask again for the originals (by hash) and previews (by file name, `<hash>.lcsp`) that were missing and
    /// storage now holds.
    pub fn retry_absent(&self, original: impl Fn(&str) -> bool, proxy: impl Fn(&str) -> bool) {
        let mut g = self.lock();
        g.absent.retain(|h| !original(h));
        g.proxies_absent.retain(|k| !proxy(k.strip_prefix("proxies/").unwrap_or(k)));
    }

    /// A preview file read from storage (`proxies/<hash>.lcsp|.lcsm`).
    pub fn insert_proxy(&self, key: &str, bytes: Arc<[u8]>) {
        let mut g = self.lock();
        g.clock += 1;
        let clock = g.clock;
        g.proxies_loading.remove(key);
        if let Some(old) = g.proxies.insert(key.to_string(), (bytes.clone(), clock)) {
            g.proxies_total -= old.0.len();
        }
        g.proxies_total += bytes.len();
        while g.proxies_total > PROXY_BUDGET && g.proxies.len() > 1 {
            let Some(oldest) = g.proxies.iter().filter(|(k, _)| *k != key).min_by_key(|(_, v)| v.1).map(|(k, _)| k.clone()) else { break };
            if let Some(v) = g.proxies.remove(&oldest) {
                g.proxies_total -= v.0.len();
            }
        }
    }

    /// Storage doesn't hold this preview (yet): it is looked for again once sync has stored it
    /// ([`Originals::retry_absent`]).
    pub fn proxy_missing(&self, key: &str) {
        let mut g = self.lock();
        g.proxies_loading.remove(key);
        g.proxies_absent.insert(key.to_string());
    }

    /// Storage has no original with this hash (a synced photo whose file wasn't downloaded), from what sync
    /// says it holds.
    pub fn mark_absent(&self, hash: &str) {
        let mut g = self.lock();
        if !g.bytes.contains_key(hash) {
            g.absent.insert(hash.to_string());
        }
    }

    /// [`Originals::proxy`], but only for a photo whose original storage doesn't hold: the engine's hook
    /// ([`lightcraft_engine::media::ProxyBytes`]). For any other photo it asks for the original and says no.
    pub fn proxy_if_absent(&self, hash: &str, max_edge: usize) -> Option<Arc<[u8]>> {
        if !self.lock().absent.contains(hash) {
            let mut g = self.lock();
            if !g.bytes.contains_key(hash) && !g.loading.contains(hash) && !g.wanted.iter().any(|w| w == hash) {
                g.wanted.push(hash.to_string());
            }
            return None;
        }
        self.proxy(hash, max_edge)
    }

    /// The smart or mini preview of the photo with this content hash for pixels no more than `max_edge` long, if in
    /// memory; a miss queues the load (the mini preview first when little is wanted, else the smart one).
    pub fn proxy(&self, hash: &str, max_edge: usize) -> Option<Arc<[u8]>> {
        let keys = crate::wire::proxy_keys(hash, max_edge);
        let mut g = self.lock();
        g.clock += 1;
        let clock = g.clock;
        for k in &keys {
            if let Some(v) = g.proxies.get_mut(k) {
                v.1 = clock;
                return Some(v.0.clone());
            }
        }
        for k in keys {
            if !g.proxies_loading.contains(&k) && !g.proxies_absent.contains(&k) && !g.proxies_wanted.contains(&k) {
                g.proxies_wanted.push(k);
            }
        }
        None
    }

    /// Preview keys to load from storage (they count as loading until [`Originals::insert_proxy`] or
    /// [`Originals::proxy_missing`]).
    pub fn take_wanted_proxies(&self) -> Vec<String> {
        let mut g = self.lock();
        let w = std::mem::take(&mut g.proxies_wanted);
        g.proxies_loading.extend(w.iter().cloned());
        w
    }

    /// Paths added since the last call.
    pub fn take_pending(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().pending)
    }

    /// (originals in memory, bytes).
    pub fn usage(&self) -> (usize, usize) {
        let g = self.lock();
        (g.bytes.len(), g.total)
    }

    /// Engine hooks that decode/probe from memory.
    pub fn hooks(&self) -> (FileLoader, FileProbe) {
        let s = self.clone();
        let loader: FileLoader = Arc::new(move |path: &str, max_edge: usize| {
            let bytes = s.get(path).ok_or_else(|| format!("{path}: the original is still loading from browser storage"))?;
            load_bytes(&bytes, max_edge)
        });
        let s = self.clone();
        let probe: FileProbe = Arc::new(move |path: &str| {
            let bytes = s.get(path).ok_or_else(|| format!("{path}: not loaded"))?;
            probe_bytes(path, &bytes)
        });
        (loader, probe)
    }

    /// Install the hooks into a session.
    pub fn install(&self, session: &mut lightcraft_engine::Session) {
        let (l, p) = self.hooks();
        session.media.file_loader = Some(l);
        session.media.file_probe = Some(p);
        let s = self.clone();
        session.media.preview_loader =
            Some(Arc::new(move |path: &str, max_edge: usize| lightcraft_engine::files::embedded_preview_srgb(&s.get(path)?, max_edge)));
        // a synced photo whose original wasn't downloaded: auto tone and the like read its preview
        let s = self.clone();
        session.media.proxy_bytes = Some(Arc::new(move |hash: &str, max_edge: usize| s.proxy_if_absent(hash, max_edge)));
    }
}

/// File name part of an export path (the browser download name).
pub fn download_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or("export.png")
}

/// MIME type for a download, by extension.
pub fn mime_for(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("tif" | "tiff") => "image/tiff",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
pub(crate) fn png_bytes(w: usize, h: usize) -> Vec<u8> {
    let bytes: Vec<u8> = (0..w * h).flat_map(|i| [(i * 7) as u8, 128, 200, 255]).collect();
    let img = lightcraft_raster::Rgba8::from_bytes(w, h, &bytes).unwrap();
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths() {
        let p = original_path("abc123", "C:\\x\\a.png");
        assert_eq!(p, "web/abc123/a.png");
        assert_eq!(hash_of_path(&p), Some("abc123"));
        assert_eq!(hash_of_path("mem/1/a.png"), None);
        assert_eq!(storage_key("abc"), "originals/abc");
    }

    #[test]
    fn miss_queues_a_load_once() {
        let s = Originals::default();
        assert!(s.get("web/h1/a.png").is_none());
        assert!(s.get("web/h1/a.png").is_none());
        assert_eq!(s.take_wanted(), vec!["h1".to_string()]);
        assert!(s.get("web/h1/a.png").is_none(), "still loading");
        assert!(s.take_wanted().is_empty());
        s.insert("h1", Arc::from(vec![1u8, 2]));
        assert_eq!(&*s.get("web/h1/b.jpg").unwrap(), &[1, 2]);
    }

    #[test]
    fn an_original_storage_does_not_hold_is_not_asked_for_every_frame() {
        let s = Originals::default();
        assert!(s.get("web/h1/a.png").is_none());
        assert_eq!(s.take_wanted(), vec!["h1".to_string()]);
        s.load_missing("h1");
        assert!(s.get("web/h1/a.png").is_none());
        assert!(s.take_wanted().is_empty(), "not asked for again");
        // sync stored it: asked for again
        s.retry_absent(|h| h == "h1", |_| false);
        assert!(s.get("web/h1/a.png").is_none());
        assert_eq!(s.take_wanted(), vec!["h1".to_string()]);
        // and a read that failed (not "missing") is retried
        s.load_failed("h1");
        assert!(s.get("web/h1/a.png").is_none());
        assert_eq!(s.take_wanted(), vec!["h1".to_string()]);
    }

    #[test]
    fn previews_stand_in_for_an_original_that_was_not_downloaded() {
        let s = Originals::default();
        // a miss asks for the preview that fits (the mini one for little), then the other
        assert!(s.proxy("abc", 400).is_none());
        assert_eq!(s.take_wanted_proxies(), vec!["proxies/abc.lcsm".to_string(), "proxies/abc.lcsp".to_string()]);
        assert!(s.proxy("abc", 400).is_none());
        assert!(s.take_wanted_proxies().is_empty(), "being loaded");
        s.proxy_missing("proxies/abc.lcsm");
        s.insert_proxy("proxies/abc.lcsp", Arc::from(vec![1u8, 2, 3]));
        // the smart preview serves a small request too while the mini one isn't there
        assert_eq!(&*s.proxy("abc", 400).unwrap(), &[1, 2, 3]);
        assert_eq!(&*s.proxy("abc", 2560).unwrap(), &[1, 2, 3]);
        // a preview storage doesn't hold is not looked for again until sync has stored it
        assert!(s.proxy("def", 2560).is_none());
        let wanted = s.take_wanted_proxies();
        wanted.iter().for_each(|k| s.proxy_missing(k));
        assert!(s.proxy("def", 2560).is_none());
        assert!(s.take_wanted_proxies().is_empty());
        s.retry_absent(|_| false, |name| name == "def.lcsp");
        assert!(s.proxy("def", 2560).is_none());
        assert_eq!(s.take_wanted_proxies(), vec!["proxies/def.lcsp".to_string()]);
    }

    #[test]
    fn previews_in_memory_are_bounded() {
        let s = Originals::default();
        let third: Arc<[u8]> = Arc::from(vec![0u8; PROXY_BUDGET / 3 + 1]);
        let held = |h: &str| s.lock().proxies.contains_key(&format!("proxies/{h}.lcsp"));
        s.insert_proxy("proxies/a.lcsp", third.clone());
        s.insert_proxy("proxies/b.lcsp", third.clone());
        assert!(s.proxy("a", 2560).is_some(), "a is used after b");
        // three don't fit: the one used longest ago goes
        s.insert_proxy("proxies/c.lcsp", third);
        assert!(held("a") && !held("b") && held("c"));
    }

    #[test]
    fn import_and_render_from_memory() {
        let store = Originals::default();
        let mut session = lightcraft_engine::Session::new();
        store.install(&mut session);
        let bytes = png_bytes(40, 30);
        let hash = content_hash(&bytes);
        let path = store.added("tiny.png", &hash, bytes.into());
        assert_eq!(store.take_pending(), vec![path.clone()]);
        let r = session.execute("library.import", &json!({"paths": [path]})).unwrap();
        let id = lightcraft_engine::catalog::PhotoId(r["imported"][0].as_u64().unwrap());
        let p = session.catalog.photo(id).unwrap();
        assert_eq!((p.width, p.height), (40, 30));
        assert_eq!(p.file_name, "tiny.png");
        assert_eq!(p.content_hash.as_deref(), Some(hash.as_str()), "storage key = catalog content hash");
        let out = session.render_now(id, 20, 20).unwrap();
        assert_eq!(out.image.width, 20);
    }

    #[test]
    fn download_names() {
        assert_eq!(download_name("dir/x-lightcraft.png"), "x-lightcraft.png");
        assert_eq!(download_name("y.png"), "y.png");
        assert_eq!(mime_for("a.JPG"), "image/jpeg");
        assert_eq!(mime_for("a"), "application/octet-stream");
        assert_eq!(download_name(""), "export.png");
    }
}
