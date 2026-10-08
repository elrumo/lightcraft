//! Map tiles on the way to the screen: fetched (desktop and iOS: here, with a disk cache; the
//! browser: its own `fetch`, which caches by itself) and decoded to pixels.
//!
//! The UI asks for the tiles it needs ([`TileRequest`]), the host runs them off the UI thread and
//! answers with [`TileResult`]s, so a slow or missing network never holds a frame.

use lightcraft_geo::tiles::TileId;

/// The tile server used until the user picks another (Settings → Map).
pub const DEFAULT_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/// Shown on the map whenever tiles from [`DEFAULT_TILE_URL`] are: the OpenStreetMap licence asks for it.
pub const DEFAULT_ATTRIBUTION: &str = "© OpenStreetMap contributors";

/// Where the attribution links to.
pub const ATTRIBUTION_URL: &str = "https://www.openstreetmap.org/copyright";

/// Tile servers ask to be told who is calling.
pub const USER_AGENT: &str = concat!("LightCraft/", env!("CARGO_PKG_VERSION"), " (photo library; +https://github.com/elrumo/lightcraft)");

/// Tiles older than this are fetched again (when the network allows; otherwise they still show).
pub const MAX_AGE_SECS: u64 = 14 * 24 * 3600;

/// How much of the disk the tile cache may use before the oldest tiles are removed.
pub const CACHE_BYTES: u64 = 256 << 20;

/// One tile to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileRequest {
    pub id: TileId,
    pub url: String,
    /// Where a copy is kept on disk (`None`: no disk cache, e.g. the browser).
    pub cache: Option<std::path::PathBuf>,
    /// May the network be used? If not, only a cached copy can answer.
    pub online: bool,
}

/// A decoded tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row by row.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TileResult {
    pub id: TileId,
    pub tile: Result<Tile, String>,
}

/// The path of a tile in the cache folder: `<dir>/<z>/<x>/<y>.tile`.
pub fn cache_path(dir: &std::path::Path, id: TileId) -> std::path::PathBuf {
    dir.join(id.z.to_string()).join(id.x.to_string()).join(format!("{}.tile", id.y))
}

/// Decode a tile image (PNG, or JPEG from satellite-style servers) to RGBA8. Tiles are small:
/// anything bigger than 1024 px on a side is refused.
pub fn decode(bytes: &[u8]) -> Result<Tile, String> {
    let d = lightcraft_codecs::decode(bytes, lightcraft_codecs::DecodeOptions::default()).map_err(|e| e.to_string())?;
    if d.width == 0 || d.height == 0 || d.width > 1024 || d.height > 1024 {
        return Err(format!("not a map tile ({}×{})", d.width, d.height));
    }
    let px = d.to_srgb8();
    let mut rgba = Vec::with_capacity(px.data.len() * 4);
    for p in &px.data {
        rgba.extend_from_slice(p);
    }
    Ok(Tile { width: d.width, height: d.height, rgba })
}

/// Load one tile: from the disk cache when it is fresh, else from the network (and keep a copy);
/// a stale copy is used when the network fails or is off. Desktop and iOS.
#[cfg(not(target_arch = "wasm32"))]
pub fn load(req: &TileRequest) -> TileResult {
    let id = req.id;
    TileResult { id, tile: crate::guard::catch("map tile", || load_inner(req)).and_then(|r| r) }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_inner(req: &TileRequest) -> Result<Tile, String> {
    use std::time::{Duration, SystemTime};
    let cached = req.cache.as_deref().and_then(|p| {
        let age = std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
        std::fs::read(p).ok().map(|b| (b, age))
    });
    let fresh = |age: &Option<Duration>| age.is_some_and(|a| a.as_secs() < MAX_AGE_SECS);
    if let Some((bytes, age)) = &cached
        && (fresh(age) || !req.online)
        && let Ok(t) = decode(bytes)
    {
        return Ok(t);
    }
    if !req.online {
        return Err("offline".into());
    }
    match fetch(&req.url) {
        Ok(bytes) => {
            let t = decode(&bytes)?;
            if let Some(path) = &req.cache {
                // a cache that can't be written is only slower, never an error
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = lightcraft_catalog::safe_file::write_atomic_nosync(path, &bytes);
                prune_now_and_then(path);
            }
            Ok(t)
        }
        Err(e) => match cached.and_then(|(b, _)| decode(&b).ok()) {
            Some(stale) => Ok(stale),
            None => Err(e),
        },
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut resp = crate::sync::http_agent().get(url).header("User-Agent", USER_AGENT).call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("tile server answered {status}"));
    }
    let mut out = Vec::new();
    resp.body_mut().with_config().limit(4 << 20).reader().read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Every few hundred stored tiles, remove the oldest ones if the cache is over [`CACHE_BYTES`].
#[cfg(not(target_arch = "wasm32"))]
fn prune_now_and_then(written: &std::path::Path) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static WRITES: AtomicU32 = AtomicU32::new(0);
    if WRITES.fetch_add(1, Ordering::Relaxed) % 300 != 299 {
        return;
    }
    // <dir>/<z>/<x>/<y>.tile
    if let Some(root) = written.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
        prune(root, CACHE_BYTES);
    }
}

/// Remove the least recently written tiles under `root` until the cache fits `budget` bytes.
#[cfg(not(target_arch = "wasm32"))]
pub fn prune(root: &std::path::Path, budget: u64) {
    fn walk(dir: &std::path::Path, depth: u8, out: &mut Vec<(std::time::SystemTime, u64, std::path::PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() && depth < 3 {
                walk(&path, depth + 1, out);
            } else if md.is_file() && path.extension().is_some_and(|x| x == "tile") {
                out.push((md.modified().unwrap_or(std::time::UNIX_EPOCH), md.len(), path));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, 0, &mut files);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= budget {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 PNG, written by hand (stored deflate blocks) so the test needs no encoder.
    fn tiny_png() -> Vec<u8> {
        fn crc(b: &[u8]) -> u32 {
            let mut c = 0xffff_ffffu32;
            for &x in b {
                c ^= u32::from(x);
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc(&body).to_be_bytes());
        }
        // two rows of (filter 0, 2 × RGB): red, green / blue, yellow
        let rows = [0u8, 255, 0, 0, 0, 255, 0, 0, 0, 0, 255, 255, 255, 0];
        let mut z = vec![0x78, 0x01, 0x01, rows.len() as u8, 0, !(rows.len() as u8), 0xff];
        z.extend_from_slice(&rows);
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &rows {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut out, b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 2, 8, 2, 0, 0, 0]);
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn decodes_a_tile() {
        let t = decode(&tiny_png()).unwrap();
        assert_eq!((t.width, t.height, t.rgba.len()), (2, 2, 16));
        assert_eq!(&t.rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode(b"not an image").is_err());
        assert!(decode(&[]).is_err());
        let mut p = tiny_png();
        p.truncate(40);
        assert!(decode(&p).is_err());
    }

    #[test]
    fn cache_layout() {
        let p = cache_path(std::path::Path::new("/c"), TileId { z: 5, x: 16, y: 10 });
        assert_eq!(p, std::path::Path::new("/c/5/16/10.tile"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn offline_serves_the_cache_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("lc-tiles-{}", std::process::id()));
        let id = TileId { z: 3, x: 4, y: 2 };
        let path = cache_path(&dir, id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let req = |online| TileRequest { id, url: "http://127.0.0.1:9/never".into(), cache: Some(path.clone()), online };
        assert_eq!(load(&req(false)).tile, Err("offline".into()), "nothing cached, no network");
        std::fs::write(&path, tiny_png()).unwrap();
        assert_eq!(load(&req(false)).tile.unwrap().width, 2);
        assert_eq!(load(&req(true)).tile.unwrap().width, 2, "fresh copy: no request is made");
        std::fs::write(&path, b"damaged").unwrap();
        assert!(load(&req(false)).tile.is_err(), "a damaged copy is an error, not a panic");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One real request to the default tile server (the whole-world tile), to see that the client,
    /// TLS, the user agent and the decoder work against the real thing:
    /// `cargo test -p lightcraft-engine --lib -- --ignored live_default_tile`
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "needs the internet"]
    fn live_default_tile() {
        let dir = std::env::temp_dir().join(format!("lc-live-tile-{}", std::process::id()));
        let id = TileId { z: 0, x: 0, y: 0 };
        let req = TileRequest { id, url: id.url(DEFAULT_TILE_URL), cache: Some(cache_path(&dir, id)), online: true };
        let t = load(&req).tile.expect("the default tile server answers");
        assert_eq!((t.width, t.height), (256, 256));
        // the world at zoom 0: ocean in the corners, not all one colour
        let first = &t.rgba[..4];
        assert!(t.rgba.chunks(4).any(|p| p != first), "a map, not a flat colour");
        // the second load comes from the disk cache, offline
        let again = load(&TileRequest { online: false, ..req });
        assert_eq!(again.tile.expect("served from the cache").rgba, t.rgba);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn pruning_removes_the_oldest_first() {
        let dir = std::env::temp_dir().join(format!("lc-prune-{}", std::process::id()));
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let p = dir.join("1").join("0").join(format!("{name}.tile"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs((10 - i as u64) * 100);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        }
        std::fs::write(dir.join("1").join("0").join("keep.txt"), b"not a tile").unwrap();
        prune(&dir, 150);
        let left: Vec<_> =
            std::fs::read_dir(dir.join("1").join("0")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(left.contains(&"c.tile".to_string()) && !left.contains(&"a.tile".to_string()), "{left:?}");
        assert!(left.contains(&"keep.txt".to_string()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
