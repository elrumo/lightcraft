use super::colour::stream_bin_factor;
use super::load::{load_vec_for, load_vec_pair_for, load_vec_reduced_for};
use super::preview::{fit_box, fs_preview_loader, preview_orientation};
#[cfg(not(target_arch = "wasm32"))]
use super::probe::probe_video_path;
use super::probe::{head_of, probe_bytes};
use super::*;

/// A file at least this long is read in part when it is a TIFF ([`read_file`]).
const READ_IN_PART_FROM: u64 = 256 << 20;
/// The longest file [`read_file`] reads whole.
pub const READ_WHOLE_MAX: u64 = 2 << 30;
/// A file read in part is read in at most this many rounds of ranges.
const READ_IN_PART_ROUNDS: usize = 8;

/// Physical memory, asked once (it can mean running a command).
fn ram() -> Option<u64> {
    static RAM: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *RAM.get_or_init(|| crate::memory::total_ram().map(|r| r as u64))
}

/// A file's bytes for describing and decoding it. Whole, up to [`READ_WHOLE_MAX`]; a longer file is
/// read only when it is a TIFF that [`lightcraft_raw::sparse::image_plan`] can read in part (a Photoshop
/// file of several GB is mostly layer data), else it is an error: reading it whole would take more
/// memory than a server may have.
///
/// A TIFF from [`READ_IN_PART_FROM`] up comes back at the file's full length with only what the plan names
/// loaded; the rest stays zero. Its pages are never touched, so they cost no memory until something
/// copies the whole buffer (the metadata reader does, once).
pub fn read_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    read_file_from(path, READ_IN_PART_FROM)
}

pub(super) fn read_file_from(path: &std::path::Path, in_part_from: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let at = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(at)?;
    let total = file.metadata().map_err(at)?.len();
    // (the buffer's length is address space, but a machine that can't back half of it with memory
    // and swap may refuse it, and a refused allocation ends the process)
    let fits = ram().is_none_or(|r| total <= r / 2);
    if total >= in_part_from
        && fits
        && let Some(buf) = read_tiff_in_part(&mut file, path, total)?
    {
        return Ok(buf);
    }
    if total > READ_WHOLE_MAX {
        return Err(format!("larger than {} GiB, and not a TIFF that can be read in part", READ_WHOLE_MAX >> 30));
    }
    let len = usize::try_from(total).map_err(|_| format!("{}: file too large", path.display()))?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(len).map_err(|e| format!("{}: {e}", path.display()))?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)).map_err(at)?;
    file.read_to_end(&mut buf).map_err(at)?;
    Ok(buf)
}

/// [`read_file`]'s TIFF route. `None`: not a TIFF the plan can read in part, or one that is mostly
/// picture anyway (the caller reads it whole).
fn read_tiff_in_part(file: &mut std::fs::File, path: &std::path::Path, total: u64) -> Result<Option<Vec<u8>>, String> {
    use lightcraft_raw::sparse::{HeaderPlan, Ranges, image_plan};
    use std::io::{Read, Seek, SeekFrom};
    let at = |e: std::io::Error| format!("{}: {e}", path.display());
    let Ok(len) = usize::try_from(total) else { return Ok(None) };
    let mut buf = vec![0u8; len];
    let mut loaded = Ranges::new();
    let head = buf.len().min(16);
    let Some(dst) = buf.get_mut(..head) else { return Ok(None) };
    file.seek(SeekFrom::Start(0)).map_err(at)?;
    file.read_exact(dst).map_err(at)?;
    loaded.insert(0, head as u64);
    for _ in 0..READ_IN_PART_ROUNDS {
        match image_plan(&buf, &loaded) {
            HeaderPlan::Need(ranges) => {
                // a file that is mostly picture gains nothing from the detour
                if loaded.total().saturating_add(ranges.total()) > total / 2 {
                    return Ok(None);
                }
                for &(start, end) in ranges.spans() {
                    let (Ok(a), Ok(b)) = (usize::try_from(start), usize::try_from(end)) else { return Ok(None) };
                    let Some(dst) = buf.get_mut(a..b) else { return Ok(None) };
                    file.seek(SeekFrom::Start(start)).map_err(at)?;
                    file.read_exact(dst).map_err(at)?;
                    loaded.insert(start, end);
                }
            }
            HeaderPlan::Ready => return Ok(Some(buf)),
            HeaderPlan::Whole(_) => return Ok(None),
        }
    }
    Ok(None)
}

/// The file's first [`lightcraft_raw::PREVIEW_PREFIX`] bytes (or the whole file when it is shorter),
/// which is what every format [`lightcraft_raw::bounded_preview`] covers keeps its pointer in.
pub(super) fn read_prefix(file: &mut std::fs::File, path: &str, total: u64) -> Result<Vec<u8>, String> {
    let len = lightcraft_raw::PREVIEW_PREFIX.min(usize::try_from(total).unwrap_or(usize::MAX));
    read_prefix_at(file, path, len as u64)
}

/// The file's first `len` bytes, read from the start.
pub(super) fn read_prefix_at(file: &mut std::fs::File, path: &str, len: u64) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let len = usize::try_from(len).map_err(|_| format!("{path}: file too large"))?;
    let mut prefix = vec![0u8; len];
    file.seek(SeekFrom::Start(0)).map_err(|e| format!("{path}: {e}"))?;
    file.read_exact(&mut prefix).map_err(|e| format!("{path}: {e}"))?;
    Ok(prefix)
}

/// The end of a Fuji RAF's preview block, from the two big-endian u32s every RAF header carries at
/// 84 (offset) and 88 (length) — the same fields `lightcraft_raw`'s RAF reader uses, read here
/// without the rest of the file so the prefix can be made big enough first.
pub(super) fn raf_preview_extent(prefix: &[u8]) -> Option<u64> {
    if !prefix.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let u32_at = |at: usize| prefix.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).map(u64::from);
    let (offset, len) = (u32_at(84)?, u32_at(88)?);
    (offset > 0 && len > 0).then(|| offset.checked_add(len)).flatten()
}

/// The display-ready preview of a raw whose embedded JPEG was already located: the counterpart of
/// [`embedded_preview_srgb`] for [`preview_window`], which does not re-read the container.
/// `adobe_rgb`: the container declares the preview Adobe RGB ([`PreviewRead::adobe_rgb`]), applied
/// as [`decode_raw_preview`] applies it. `None` when the bytes are not a decodable image.
pub(super) fn quick_image_srgb(header: &[u8], window: &[u8], adobe_rgb: bool, max_edge: usize) -> Option<lightcraft_raster::Rgba8> {
    let decoded = if adobe_rgb && window.starts_with(&[0xff, 0xd8]) {
        lightcraft_codecs::decode_jpeg_with_fallback(window, fit_box(max_edge), lightcraft_codecs::NamedSpace::AdobeRgb)
    } else {
        lightcraft_codecs::decode(window, fit_box(max_edge))
    };
    let mut d = decoded.ok()?;
    if d.image.width.max(d.image.height) > max_edge {
        d.image = fit(&d.image, max_edge, max_edge, Filter::Box);
        d.alpha = None;
    }
    // The JPEG's own EXIF orientation, else the raw's (from its header), as the whole-file path does
    Some(d.to_srgb8().oriented(preview_orientation(header, d.orientation)))
}

/// The filesystem-backed [`PairLoader`]: the file is read and decoded once, and developed as usual and from the photo's
/// denoised picture. It holds two pictures at once, so it counts for more at the memory gate than a plain load.
pub fn fs_pair_loader() -> PairLoader {
    Arc::new(|path: &str, max_edge: usize, spec: &DenoiseSpec, catalogued: Catalogued| {
        let len = std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0);
        let weight = len * if max_edge <= crate::media::SourceLevel::Thumb.max_edge() { 4 } else { 10 };
        let gate = crate::memory::work_gate();
        let _permit = if crate::memory::is_background() { gate.acquire(weight) } else { gate.acquire_urgent(weight) };
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let r = load_vec_pair_for(bytes, max_edge, spec, catalogued);
        crate::memory::release();
        r
    })
}

/// Working memory (bytes) of loading `bytes` for a source of at most `max_edge` pixels: its size times a
/// rule of thumb for compressed images; for a raw, what its decode allocates — the samples (16 bits each), some
/// tiles in flight, and unless the load bins the mosaic ([`bin_factor`]) the full-size normalised and RGB float
/// images.
pub(super) fn load_weight(bytes: &[u8], max_edge: usize) -> usize {
    let factor = if max_edge <= crate::media::SourceLevel::Thumb.max_edge() { 3 } else { 6 };
    let by_size = bytes.len().saturating_mul(factor);
    let Ok(info) = lightcraft_raw::probe_info(bytes) else { return by_size };
    let px = info.width.saturating_mul(info.height);
    if let Some(k) = stream_bin_factor(&info, max_edge) {
        // binned while decoding: block sums (18 bytes per block) and the tiles in flight, never the samples
        let (w, h) = info.developed_size();
        return by_size.max((w / k).saturating_mul(h / k).saturating_mul(18).saturating_add(160 << 20));
    }
    let binned = max_edge.saturating_mul(9) <= info.width.max(info.height).saturating_mul(5);
    let develop = if binned { 0 } else { px.saturating_mul(info.cpp * 4 + 12) };
    by_size.max(px.saturating_mul(info.cpp * 2).saturating_add(develop).saturating_add(64 << 20))
}

/// Filesystem-backed hooks (native). On the web the host installs bytes-based hooks instead.
///
/// Their working memory is bounded by [`crate::memory::work_gate`]: background loads (grid
/// thumbnails, neighbour prefetch: [`crate::memory::in_background`]) and import probes wait while
/// too much is in flight, each counting what decoding its file takes ([`load_weight`]);
/// interactive loads (the loupe, exports) never wait — they are counted, so background work
/// yields to them.
pub fn fs_hooks() -> (FileLoader, FileProbe) {
    let loader: FileLoader = Arc::new(|path: &str, max_edge: usize, catalogued: Catalogued| {
        fs_load(path, max_edge, |bytes| load_vec_for(bytes, max_edge, catalogued), |img, info| (img, info))
    });
    let probe: FileProbe = Arc::new(|path: &str| {
        if let Some(r) = probe_video_path(path) {
            let mut info = r?;
            info.file_modified = file_modified(path);
            info.locked = file_locked(path);
            return Ok(info);
        }
        let len = std::fs::metadata(path).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0);
        let _permit = crate::memory::work_gate().acquire(len);
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let mut info = probe_bytes(path, &bytes)?;
        info.file_modified = file_modified(path);
        info.locked = file_locked(path);
        Ok(info)
    });
    (loader, probe)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn load_video(_: &str, _: usize, _: Option<f64>) -> Result<(Rgb32f, SourceInfo), String> {
    Err("video clips need their file on disk".to_string())
}

#[cfg(target_arch = "wasm32")]
fn probe_video_path(_: &str) -> Option<Result<ProbeInfo, String>> {
    None
}

/// What decoding one clip picture counts against the memory gate: the decoder's reference pictures at 4K, and a margin.
const VIDEO_WEIGHT: usize = 192 << 20;

/// The filesystem [`ReducedLoader`]: [`load_vec_reduced`] of the file, gated like [`fs_hooks`]' loader.
pub fn fs_reduced_loader() -> ReducedLoader {
    Arc::new(|path: &str, max_edge: usize, catalogued: Catalogued| {
        fs_load(path, max_edge, |bytes| load_vec_reduced_for(bytes, max_edge, catalogued), |img, info| (img, info, false))
    })
}

/// Read `path` and `load` it, waiting on [`crate::memory::work_gate`] as [`fs_hooks`] explains. A clip is never read
/// whole: its poster frame is decoded from the file in place (a frame's worth of memory), and `from_video` makes
/// that picture what `load` would have given.
fn fs_load<T>(
    path: &str,
    max_edge: usize,
    load: impl FnOnce(Vec<u8>) -> Result<T, String>,
    from_video: impl FnOnce(Rgb32f, SourceInfo) -> T,
) -> Result<T, String> {
    let (path, time) = split_time(path);
    let gate = crate::memory::work_gate();
    let hold = |weight| if crate::memory::is_background() { gate.acquire(weight) } else { gate.acquire_urgent(weight) };
    if head_of(path).is_some_and(|h| lightcraft_video::is_video(&h)) {
        let _permit = hold(VIDEO_WEIGHT);
        let r = load_video(path, max_edge, time);
        crate::memory::release();
        return r.map(|(img, info)| from_video(img, info));
    }
    let len = std::fs::metadata(path).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0);
    // the file is read while counted at its size; the decode is then counted at what it needs, which the
    // header tells (a JPEG XL ProRAW is 4 MB on disk and 70 MB of samples) — never holding one share while
    // waiting for another, which could deadlock two loads
    let bytes = {
        let _read = hold(len);
        std::fs::read(path).map_err(|e| format!("{path}: {e}"))?
    };
    let _permit = hold(load_weight(&bytes, max_edge));
    let r = load(bytes);
    // the file, the samples and the intermediate images are gone: give their pages back
    crate::memory::release();
    r
}

/// `path`'s modification time as ISO 8601 (see [`ProbeInfo::file_modified`]); `None` when the
/// filesystem doesn't say or the time is before 1970.
fn file_modified(path: &str) -> Option<String> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = i64::try_from(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs()).ok()?;
    Some(crate::import::civil(secs))
}

/// Whether `path` has the Read-Only attribute (cameras set it on the files they protect; cards keep it, and copies of
/// the file keep it as well). `false` when the filesystem doesn't say.
fn file_locked(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.permissions().readonly())
}

/// Whether the process runs in the macOS App Sandbox (it names its container in the environment).
#[cfg(target_os = "macos")]
fn sandboxed() -> bool {
    std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some()
}

impl crate::Session {
    /// Install the filesystem file hooks (desktop, CLI, MCP headless), and on macOS the Apple Photos
    /// runner ([`crate::apple_photos::osascript`]; installed, not run).
    pub fn with_fs(mut self) -> Self {
        let (l, p) = fs_hooks();
        self.media.file_loader = Some(l);
        self.media.reduced_loader = Some(fs_reduced_loader());
        self.media.file_probe = Some(p);
        self.media.preview_loader = Some(fs_preview_loader());
        self.media.denoise.loader = Some(fs_pair_loader());
        // (not in a sandboxed app, the Mac App Store's: macOS gives it no Apple Events to Photos, so the runner would only
        // fail after the Automation prompt that can't be answered; the app adds to Photos itself, or has no such button)
        #[cfg(target_os = "macos")]
        {
            if !sandboxed() {
                self.apple_photos = Some(crate::apple_photos::osascript());
            }
        }
        self
    }
}
