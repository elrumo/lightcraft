use super::batch::run_batch;
use super::encoding::{encode_gain_map_jpeg, encode_rendered, export_metadata, removes_location};
use super::options::ExportFormat;
use super::params::apple_photos_unsupported;
use super::place::Placer;
use super::*;

/// One exported file.
pub struct Exported {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// Files written next to it, named like it with another extension: `(extension, bytes)` (the
    /// XMP sidecar of an `Original` export).
    pub sidecars: Vec<(&'static str, Vec<u8>)>,
    /// Set when the render was tried on the GPU and fell back to the CPU: why (see
    /// `docs/gpu-pipeline.md`). The file holds the CPU render either way.
    pub gpu_fallback: Option<String>,
}

/// What rendering photo `p` at `w × h` needs in memory on this device: its source (the whole sensor, or fewer pixels when
/// a DNG or a Bayer raw too big for the device is read smaller, [`crate::media::export_binned_edge`]) and the output.
fn export_need_of(session: &crate::Session, p: &lightcraft_catalog::Photo, w: usize, h: usize) -> usize {
    let (long, out_px) = (w.max(h), w.saturating_mul(h));
    let need = crate::memory::export_need(crate::media::export_source_pixels(p, long), out_px);
    match crate::media::export_binned_edge(session, p, long, out_px) {
        Some(edge) => need.min(crate::media::export_binned_need(p, edge, out_px)),
        None => need,
    }
}

/// Output size of photo `p` under `o` (its cropped full size when `o.resize` is `None`).
pub fn output_size(p: &lightcraft_catalog::Photo, o: &ExportOptions) -> (usize, usize) {
    let (w, h) = lightcraft_pipeline::native_output_size(p.width.max(1) as usize, p.height.max(1) as usize, &p.develop);
    // whole pixels first: a crop's float noise (5999.999… for 6000) must not decide a half-pixel tie
    let (w, h) = (w.round().max(1.0), h.round().max(1.0));
    match &o.resize {
        Some(r) => r.apply(w, h),
        None => (w as usize, h as usize),
    }
}

/// Render photo `id` at the requested size and encode it (or copy / convert its original for
/// [`ExportFormat::Original`] / [`ExportFormat::Dng`]).
pub fn export_photo(session: &mut crate::Session, id: lightcraft_catalog::PhotoId, o: &ExportOptions, seq: usize) -> Result<Exported, String> {
    prepare_export(session, id, o, seq)?.run()
}

/// One photo's export, set up from the session ([`prepare_export`]). [`PreparedExport::run`] does
/// the heavy part (read, decode, render, encode) without the session, so it can run on another
/// thread.
pub struct PreparedExport {
    pub photo: lightcraft_catalog::PhotoId,
    pub file_name: String,
    /// The folder of the photo's original on disk ([`DestinationMode::SameAsOriginal`]); `None`
    /// for a demo photo or one in the browser's storage.
    pub source_dir: Option<String>,
    work: Work,
    /// The library's originals, which [`run_batch`] never writes over (shared by a batch).
    pub(super) guard: std::sync::Arc<crate::originals::OriginalGuard>,
    /// Estimated working memory of [`Self::run`] (bytes), held from [`crate::memory::export_gate`]
    /// while it runs beside other photos of a batch.
    pub(super) weight: usize,
}

struct RenderWork {
    job: crate::media::RenderJob,
    /// The HDR rendition of a gain map JPEG (`job` renders its SDR base).
    hdr_job: Option<crate::media::RenderJob>,
    meta: Option<Metadata>,
    opts: ExportOptions,
    /// The size [`output_size`] gives, which a host's picture must match.
    size: (usize, usize),
    /// The picture a host drew in place of `job`'s ([`PreparedExport::with_host_pixels`]).
    host: Option<HostPixels>,
}

/// The most pixels of a host's picture (it is read whole).
const HOST_PIXELS_MAX: usize = 1 << 28;

/// A picture a host drew for a photo, kept in a file of `width × height` RGBA8 pixels (opaque, sRGB, upright, cropped:
/// what a render of the photo at the export's size is). `app.export`'s `hostPixels` hands them over, one per photo, for a
/// frontend with a renderer of its own; the export encodes and places them exactly as it does its own render (sharpening,
/// watermark, metadata, the profile, the name), so only the pixels come from the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPixels {
    pub file: String,
    pub width: usize,
    pub height: usize,
}

impl HostPixels {
    /// The picture as a render, or `None` when the file is gone, empty, too big or not `width × height × 4` bytes.
    fn read(&self) -> Option<lightcraft_pipeline::Rendered> {
        let n = self.width.checked_mul(self.height).filter(|n| (1..=HOST_PIXELS_MAX).contains(n))?;
        if std::fs::metadata(&self.file).ok()?.len() != u64::try_from(n.checked_mul(4)?).ok()? {
            return None;
        }
        let bytes = std::fs::read(&self.file).ok()?;
        let mut data = bytes.as_chunks::<4>().0.to_vec();
        // a photo has no transparency: whatever alpha the host wrote, the file is opaque
        data.iter_mut().for_each(|p| p[3] = 255);
        let image = Rgba8 { width: self.width, height: self.height, data };
        Some(lightcraft_pipeline::Rendered { histogram: lightcraft_raster::Histogram::of_srgb8(&image), image, deep: None })
    }
}

/// An export the sync server renders ([`crate::cloud_sync::render_on_server`]).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))] // (the web build can't make the call)
struct ServerWork {
    target: crate::cloud_sync::RenderTarget,
    request: lightcraft_catalog::sync::proto::Render,
    /// The size asked for (what the export reports).
    size: (usize, usize),
    /// How it waits when the server is busy.
    wait: crate::cloud_sync::RenderWait,
}

enum Work {
    Render(Box<RenderWork>),
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // (the web build can't make the call)
    Server(Box<ServerWork>),
    /// [`ExportFormat::Original`] (the file's bytes + an XMP sidecar) or [`ExportFormat::Dng`] (the
    /// raw data re-encoded as a lossless DNG with the edits in its XMP).
    File {
        path: String,
        read: Option<crate::merge::ByteReader>,
        packet: String,
        dng: Option<DngCompression>,
        /// Lossy DNG: JPEG quality and the long edge to scale to (`None`: full size).
        quality: u8,
        lossy_edge: Option<u32>,
        /// The original must not carry a location ([`removes_location`]).
        remove_location: bool,
        label: String,
        size: (usize, usize),
    },
}

/// Set up the export of photo `id` at 1-based position `seq` of a batch. (For a whole batch use
/// [`prepare_batch`]: it looks at the library's originals once.)
pub fn prepare_export(
    session: &mut crate::Session,
    id: lightcraft_catalog::PhotoId,
    o: &ExportOptions,
    seq: usize,
) -> Result<PreparedExport, String> {
    let guard = std::sync::Arc::new(session.original_guard());
    prepare_guarded(session, id, o, seq, guard)
}

/// [`prepare_export`] for each of `ids` in order (`{seq}` = position, from 1).
pub fn prepare_batch(session: &mut crate::Session, ids: &[lightcraft_catalog::PhotoId], o: &ExportOptions) -> Result<Vec<PreparedExport>, String> {
    let guard = std::sync::Arc::new(session.original_guard());
    ids.iter().enumerate().map(|(i, id)| prepare_guarded(session, *id, o, i + 1, guard.clone())).collect()
}

fn prepare_guarded(
    session: &mut crate::Session,
    id: lightcraft_catalog::PhotoId,
    o: &ExportOptions,
    seq: usize,
    guard: std::sync::Arc<crate::originals::OriginalGuard>,
) -> Result<PreparedExport, String> {
    let p = session.catalog.photo(id).ok_or("no such photo")?;
    let file_name = o.file_name_for(p, seq);
    let source_dir = original_folder(&p.source);
    // HDR output only for photos edited in HDR; the rest of the batch exports as usual
    let sdr_opts;
    let o = if o.hdr && !p.develop.hdr.enabled {
        sdr_opts = ExportOptions { hdr: false, ..o.clone() };
        &sdr_opts
    } else {
        o
    };
    let (full_px, long) = ((p.width as usize).saturating_mul(p.height as usize), p.width.max(p.height) as usize);
    let (work, weight) = if o.format.is_rendered() {
        let (w, h) = output_size(p, o);
        // what this render needs against what this device may use: over it, the sync server renders (or says why not)
        let need = export_need_of(session, p, w, h);
        let too_big = session.export_limit().is_some_and(|limit| need > limit);
        if o.on_server || too_big {
            let Some(target) = session.sync_render_target(p) else {
                return Err(if o.on_server {
                    format!("{}: the sync server has no original of this photo to render (sign in, and let it upload)", p.file_name)
                } else if session.export_limit().is_some_and(|limit| export_need_of(session, p, w / 2, h / 2) <= limit) {
                    format!(
                        "{}: a {w} × {h} export needs about {} MB, more than this device can allow. Export it smaller, or sign in to a sync server that has the original and it renders there",
                        p.file_name,
                        need >> 20
                    )
                } else {
                    // (the photo's own size is what costs: a smaller export would not fit either)
                    format!(
                        "{}: a {} MP photo needs about {} MB to export, more than this device can allow at any size. Sign in to a sync server that has the original and it renders there",
                        p.file_name,
                        full_px / 1_000_000,
                        need >> 20
                    )
                });
            };
            let settings = serde_json::to_value(&*p.develop).map_err(|e| e.to_string())?;
            let request =
                lightcraft_catalog::sync::proto::Render { hash: target.hash.clone(), name: p.file_name.clone(), settings, export: o.to_json() };
            // (here: the file it downloads)
            let weight = w.saturating_mul(h).saturating_mul(4);
            return Ok(PreparedExport {
                photo: id,
                file_name,
                source_dir,
                work: Work::Server(Box::new(ServerWork {
                    target,
                    request,
                    size: (w, h),
                    wait: crate::cloud_sync::RenderWait::new(session.activity.clone()),
                })),
                guard,
                weight,
            });
        }
        // a browser that doesn't hold the original renders from the photo's preview, up to the preview's size
        if let (Some(store), lightcraft_catalog::Source::File { path }) = (&session.sync_store, &p.source)
            && let Some(hash) = lightcraft_catalog::sync::hash_of_path(path)
            && !store.originals.contains(hash)
            && w.max(h) > crate::media::SourceLevel::Preview.max_edge()
        {
            return Err(format!(
                "{}: this browser has only a preview of it. Choose Photo > Download Originals first, or export at {} px or smaller",
                p.file_name,
                crate::media::SourceLevel::Preview.max_edge()
            ));
        }
        let (nw, nh) = lightcraft_pipeline::native_output_size(p.width.max(1) as usize, p.height.max(1) as usize, &p.develop);
        let native_px = (nw.round().max(1.0) * nh.round().max(1.0)) as usize;
        let label = p.file_name.clone();
        let meta = export_metadata(p, &session.catalog, o);
        let gain_map = o.hdr_output() && o.format == ExportFormat::Jpeg;
        let job = session.export_job(id, w, h, o.effective_space(), o.effective_depth())?;
        let weight = render_weight(full_px, long, job.level, w.saturating_mul(h));
        if let Some(why) = too_big_for_memory((w, h), native_px, weight, crate::memory::total_ram()) {
            return Err(format!("{label}: {why}"));
        }
        let hdr_job = if gain_map { Some(session.export_job(id, w, h, o.effective_space(), OutputDepth::F32Hdr)?) } else { None };
        (Work::Render(Box::new(RenderWork { job, hdr_job, meta, opts: o.clone(), size: (w, h), host: None })), weight)
    } else {
        let lightcraft_catalog::Source::File { path } = &p.source else {
            return Err(format!("{} is a generated demo photo: it has no original file to export", p.file_name));
        };
        let dng = (o.format == ExportFormat::Dng).then_some(o.dng_compression);
        // the file's bytes; a DNG also decodes the raw data (16-bit samples) and writes it again
        let weight = (p.file_size as usize).saturating_add(if dng.is_some() { full_px.saturating_mul(6) } else { 0 });
        let work = Work::File {
            path: path.clone(),
            read: session.media.file_bytes.clone(),
            packet: crate::sidecar::sidecar_packet_with(p, &session.catalog, removes_location(p, &session.catalog, o)),
            dng,
            quality: o.quality,
            lossy_edge: o.resize.is_some().then(|| {
                let (w, h) = output_size(p, o);
                u32::try_from(w.max(h)).unwrap_or(u32::MAX)
            }),
            remove_location: removes_location(p, &session.catalog, o),
            label: p.file_name.clone(),
            size: (p.width as usize, p.height as usize),
        };
        (work, weight)
    };
    Ok(PreparedExport { photo: id, file_name, source_dir, work, guard, weight })
}

/// The folder a photo's original is in, for [`DestinationMode::SameAsOriginal`]. None for a demo
/// photo, and on the web, where an original lives in the browser's storage, not in a folder.
fn original_folder(source: &lightcraft_catalog::Source) -> Option<String> {
    let lightcraft_catalog::Source::File { path } = source else { return None };
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let dir = std::path::Path::new(path).parent()?.to_string_lossy().to_string();
    (!dir.is_empty()).then_some(dir)
}

/// Why an export of `out` pixels (`w`, `h`), from a photo of `native_px` pixels at its own size,
/// can't be made on a computer with `ram` bytes, given its working memory `weight`: an export
/// enlarged past the photo's size that needs more memory than the computer has would fail to
/// allocate, which ends the process rather than the export. Full-size and smaller exports, and
/// computers whose memory isn't known, are never refused.
pub(super) fn too_big_for_memory(out: (usize, usize), native_px: usize, weight: usize, ram: Option<usize>) -> Option<String> {
    let ram = ram?;
    if out.0.saturating_mul(out.1) <= native_px || weight <= ram {
        return None;
    }
    let gb = |b: usize| b as f64 / (1u64 << 30) as f64;
    Some(format!(
        "exporting at {} × {} needs about {:.0} GB of memory, more than this computer has ({:.0} GB); choose a smaller size",
        out.0,
        out.1,
        gb(weight).ceil(),
        gb(ram)
    ))
}

/// Working memory of rendering a photo of `full_px` pixels (long edge `long`) from source level
/// `level` into `out_px` output pixels: the decoded source (linear RGB f32, 12 B/px) plus the
/// pipeline's planes and the encoded output (~36 B per output pixel). Measured: a 24 MP export
/// peaks at ~1.2 GB, one at 2048 px from the 2560 px preview at ~150 MB.
pub(super) fn render_weight(full_px: usize, long: usize, level: crate::media::SourceLevel, out_px: usize) -> usize {
    let edge = level.max_edge().min(long.max(1));
    let scale = edge as f64 / long.max(1) as f64;
    let src_px = (full_px as f64 * scale * scale).min(usize::MAX as f64 / 64.0) as usize;
    src_px.saturating_mul(12).saturating_add(out_px.saturating_mul(36))
}

/// A photo of a batch as [`BatchPlacer`] needs to know it (taken with [`PreparedExport::head`]
/// before [`PreparedExport::run`] uses the export up).
#[derive(Clone)]
pub struct ItemHead {
    photo: lightcraft_catalog::PhotoId,
    name: String,
    source_dir: Option<String>,
    guard: std::sync::Arc<crate::originals::OriginalGuard>,
}

/// A rendered export taken apart, for a host that renders it somewhere else (the browser's export
/// worker, which has no [`PreparedExport`] to run): [`encode_render_export`] makes the file from
/// what it renders.
pub struct RenderParts {
    pub job: crate::media::RenderJob,
    /// The HDR rendition of a gain map JPEG (`job` renders its SDR base).
    pub hdr_job: Option<crate::media::RenderJob>,
    pub meta: Option<Metadata>,
    pub opts: ExportOptions,
}

/// The file [`PreparedExport::run`] makes from a photo's render `r` (and, for a gain map JPEG, the
/// HDR rendition `hdr`): the one place a rendered export is encoded, whoever rendered it.
pub fn encode_render_export(
    file_name: String,
    r: lightcraft_pipeline::Rendered,
    hdr: Option<lightcraft_pipeline::Rendered>,
    opts: &ExportOptions,
    meta: Option<&Metadata>,
) -> Result<Exported, String> {
    let bytes = match hdr {
        Some(hr) => {
            let hdr = hr.deep.ok_or("the HDR render has no float samples")?;
            encode_gain_map_jpeg(&r.image, &hdr, opts, meta)?
        }
        None => encode_rendered(&r, opts, meta)?,
    };
    Ok(Exported { file_name, bytes, width: r.image.width, height: r.image.height, sidecars: Vec::new(), gpu_fallback: None })
}

/// Places a batch's files one photo at a time, for a host whose photos finish in their own time
/// (the browser, which has no threads: [`run_batch`] is this with the photos run in a loop). Same
/// paths, conflict policy and sidecars as [`run_batch`].
pub struct BatchPlacer<'a>(Placer<'a>);

impl<'a> BatchPlacer<'a> {
    /// `photos`: how many the batch has (an exact file name is for a single photo).
    pub fn new(
        o: &'a ExportOptions,
        to: &'a Destination,
        write: &'a mut dyn FnMut(&str, &[u8]) -> Result<(), String>,
        exists: &'a dyn Fn(&str) -> bool,
        stop_on_error: bool,
        photos: usize,
    ) -> Self {
        let mut placer = Placer::new(o, to, write, exists, stop_on_error);
        placer.single = photos == 1;
        BatchPlacer(placer)
    }

    /// Write `head`'s photo's result and record the outcome. `Err` only when it failed and
    /// `stop_on_error` is set.
    pub fn place(&mut self, head: &ItemHead, result: Result<Exported, String>) -> Result<(), String> {
        self.0.place(head.photo, head.name.clone(), head.source_dir.as_deref(), &head.guard, result)
    }

    /// One JSON object per photo placed, as [`run_batch`] returns them.
    pub fn finish(self) -> Vec<serde_json::Value> {
        self.0.out
    }
}

impl PreparedExport {
    /// What [`BatchPlacer::place`] needs of this photo.
    pub fn head(&self) -> ItemHead {
        ItemHead { photo: self.photo, name: self.file_name.clone(), source_dir: self.source_dir.clone(), guard: self.guard.clone() }
    }

    /// The pieces of a rendered export (not of a copied original or a DNG), cheap to take: the
    /// export itself is left as it is, to run here when the host can't render elsewhere.
    pub fn render_parts(&self) -> Option<RenderParts> {
        match &self.work {
            Work::Render(w) => Some(RenderParts { job: w.job.clone(), hdr_job: w.hdr_job.clone(), meta: w.meta.clone(), opts: w.opts.clone() }),
            _ => None,
        }
    }

    /// Use `host`'s picture for this photo instead of rendering it, when it can stand in for the render: a rendered
    /// format written 8-bit in sRGB (not HDR), at the size [`output_size`] gives (to a pixel). `false`, and the export
    /// renders the photo itself, when it cannot; the same when the file turns out unreadable as the export runs.
    pub fn with_host_pixels(&mut self, host: HostPixels) -> bool {
        let Work::Render(w) = &mut self.work else { return false };
        let o = &w.opts;
        let fits = w.hdr_job.is_none()
            && !o.hdr_output()
            && o.effective_space() == OutputSpace::Srgb
            && o.effective_depth() == OutputDepth::U8
            && host.width.abs_diff(w.size.0) <= 1
            && host.height.abs_diff(w.size.1) <= 1;
        if fits {
            w.host = Some(host);
        }
        fits
    }

    /// Let `cancel` (the export's flag) end a wait for a busy sync server.
    pub fn set_cancel(&mut self, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        if let Work::Server(w) = &mut self.work {
            w.wait.cancel = cancel;
        }
    }

    pub fn run(self) -> Result<Exported, String> {
        let file_name = self.file_name;
        match self.work {
            Work::Render(w) => {
                let RenderWork { job, hdr_job, meta, opts, host, .. } = *w;
                if let Some(r) = host.as_ref().and_then(HostPixels::read) {
                    return encode_render_export(file_name, r, None, &opts, meta.as_ref());
                }
                let result = job.run();
                let r = result.rendered?;
                let gpu_fallback = result.gpu_fallback;
                let hdr = match hdr_job {
                    Some(hj) => Some(hj.run().rendered?),
                    None => None,
                };
                Ok(Exported { gpu_fallback, ..encode_render_export(file_name, r, hdr, &opts, meta.as_ref())? })
            }
            #[cfg(not(target_arch = "wasm32"))]
            Work::Server(w) => {
                let bytes = crate::cloud_sync::render_on_server(&w.target, &w.request, &w.wait)?;
                Ok(Exported { file_name, bytes, width: w.size.0, height: w.size.1, sidecars: Vec::new(), gpu_fallback: None })
            }
            #[cfg(target_arch = "wasm32")]
            Work::Server(_) => Err("rendering on the server needs the desktop or iOS app".to_string()),
            Work::File { path, read, packet, dng, quality, lossy_edge, label, size, remove_location } => {
                let bytes = match &read {
                    Some(r) => r(&path)?,
                    None => std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?,
                };
                let Some(dng) = dng else {
                    // the file is copied byte for byte: refuse rather than leak a location it embeds
                    if remove_location {
                        // a PSD or PSB whose metadata can't be read can't be shown to carry no location
                        lightcraft_meta::try_extract(&bytes)
                            .map_err(|e| format!("{label}: can't verify location metadata in this original file: {e}"))?;
                        if lightcraft_meta::embeds_location(&bytes) {
                            // DNG only for what the DNG export below takes
                            let instead = if lightcraft_raw::probe(&bytes).is_some() { "JPEG, TIFF or DNG" } else { "JPEG or TIFF" };
                            return Err(format!(
                                "{label}: can't remove the location from an original file that embeds GPS; export as {instead} instead"
                            ));
                        }
                    }
                    return Ok(Exported {
                        file_name,
                        bytes,
                        width: size.0,
                        height: size.1,
                        sidecars: vec![("xmp", packet.into_bytes())],
                        gpu_fallback: None,
                    });
                };
                if lightcraft_raw::probe(&bytes).is_none() {
                    return Err(format!("{label}: DNG export needs a raw photo"));
                }
                let mut raw = lightcraft_raw::decode(&bytes).map_err(|e| format!("{label}: {e}"))?;
                crate::camera_preview::with_starting_colour(&mut raw, &bytes);
                drop(bytes);
                let compression = match dng {
                    DngCompression::Lossless => lightcraft_raw::DngCompression::Lj92 { tile: 256 },
                    DngCompression::Deflate => lightcraft_raw::DngCompression::Deflate { tile: 256, half: false },
                    DngCompression::Uncompressed => lightcraft_raw::DngCompression::Uncompressed,
                    DngCompression::Lossy => {
                        lightcraft_raw::DngCompression::LossyJpeg { tile: 256, quality: quality.clamp(1, 100), long_edge: lossy_edge }
                    }
                };
                let dng = lightcraft_raw::write_dng(&raw, &lightcraft_raw::DngWriteOptions { xmp: Some(packet), compression, ..Default::default() })
                    .map_err(|e| e.to_string())?;
                Ok(Exported { file_name, bytes: dng, width: raw.width, height: raw.height, sidecars: Vec::new(), gpu_fallback: None })
            }
        }
    }
}

/// How an exported DNG stores its raw data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DngCompression {
    /// Lossless JPEG (the usual DNG compression).
    #[default]
    Lossless,
    /// Deflate (zip) tiles with prediction.
    Deflate,
    Uncompressed,
    /// Lossy DNG: demosaiced linear data as JPEG tiles at the export quality, scaled to the resize
    /// setting when there is one (Lightroom's "Use Lossy Compression").
    Lossy,
}

/// The destination of a batch: a folder (with `ExportOptions::subfolder` and the conflict policy
/// applied to each file together with its sidecars; with [`DestinationMode::SameAsOriginal`] each
/// photo's own folder instead of `dir`), or one exact file path (single photo; an ordinary file
/// already there is replaced). Either way a catalogued original (or its sidecar)
/// is never written over: see [`crate::originals`].
#[derive(Clone, Debug, Default)]
pub struct Destination {
    pub dir: String,
    pub exact: Option<String>,
}

/// What happens after an export has written its files (Lightroom's Post-Processing): nothing, or
/// adding them to Apple Photos ([`ExportOptions::add_to_photos`]). Made by
/// [`crate::Session::after_export`] before the export starts, so an export asking for something
/// this session can't do fails before writing anything; for Photos it holds the import slot
/// (a [`crate::apple_photos::Reservation`]) until the files are handed over, and frees it when
/// dropped unused (the export failed or never ran). `Send`: a background export takes it to its
/// worker.
#[derive(Default)]
pub struct AfterExport {
    #[cfg(not(target_arch = "wasm32"))]
    apple_photos: Option<crate::apple_photos::Reservation>,
}

impl AfterExport {
    /// Add the files to Apple Photos with `runner`, into `album` when it isn't blank, reserving
    /// the import slot of `imports` now. (Front ends get this from
    /// [`crate::Session::after_export`], which checks the platform first.)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn apple_photos(runner: crate::apple_photos::Runner, album: &str, imports: &crate::apple_photos::Imports) -> Result<Self, String> {
        Ok(Self { apple_photos: Some(imports.reserve(runner, album)?) })
    }

    /// Nothing to do.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_none(&self) -> bool {
        self.apple_photos.is_none()
    }

    /// Nothing to do.
    #[cfg(target_arch = "wasm32")]
    pub fn is_none(&self) -> bool {
        true
    }

    /// Do it now for an export's per-photo results ([`run_batch`]'s: only the files written
    /// count), waiting for Photos: for a worker thread (the app's background export) or a caller
    /// without a window (MCP, lightcraft-cli). `None` when there was nothing to do, else the
    /// outcome to report as the export's `applePhotos`: the finished job (`{job, running: false,
    /// requested, album}` with `{imported, ids, warning?}`, `{error}`, or `{skipped}` when the
    /// export was cancelled or wrote nothing). Never fails the export: its files are written
    /// either way.
    pub fn run(self, files: &[serde_json::Value], cancelled: bool) -> Option<serde_json::Value> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = self.apple_photos {
            return Some(r.run(files, cancelled));
        }
        let _ = (files, cancelled);
        None
    }

    /// Like [`AfterExport::run`], without waiting: Photos works on a worker thread and the
    /// outcome is the running job (`{job, running: true, requested, album}`; its result comes
    /// from `export.photosImports`, and the desktop app announces it). For a caller on the UI
    /// thread, which must not wait minutes for Photos.
    pub fn start(self, files: &[serde_json::Value]) -> Option<serde_json::Value> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = self.apple_photos {
            return Some(r.start(files));
        }
        let _ = files;
        None
    }

    /// [`AfterExport::start`] where the session answers on a UI thread
    /// ([`crate::Session::apple_photos_background`]), else [`AfterExport::run`].
    pub fn finish(self, s: &crate::Session, files: &[serde_json::Value]) -> Option<serde_json::Value> {
        #[cfg(not(target_arch = "wasm32"))]
        if s.apple_photos_background {
            return self.start(files);
        }
        let _ = s;
        self.run(files, false)
    }
}

impl crate::Session {
    /// What an export with `o` does after writing its files; an error, before anything is written,
    /// when it asks for Apple Photos and this platform or session has no way to reach it, or
    /// another import is reserved or running. For Photos the import slot is reserved from now on.
    pub fn after_export(&self, o: &ExportOptions) -> Result<AfterExport, String> {
        if !o.add_to_photos {
            return Ok(AfterExport::default());
        }
        if let Some(why) = apple_photos_unsupported() {
            return Err(why.into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let runner = self.apple_photos.clone().ok_or_else(|| crate::apple_photos::unavailable().to_string())?;
            let mut after = AfterExport::apple_photos(runner, &o.photos_album, &self.apple_photos_imports)?;
            after.apple_photos = after.apple_photos.map(|r| r.shown_in(self.activity.clone()));
            Ok(after)
        }
        #[cfg(target_arch = "wasm32")]
        Err("Apple Photos is only available on macOS".into())
    }
}

/// Export `ids` in order ([`prepare_batch`] + [`run_batch`]), stopping at the first error.
pub fn export_batch(
    session: &mut crate::Session,
    ids: &[lightcraft_catalog::PhotoId],
    o: &ExportOptions,
    to: &Destination,
    write: &mut dyn FnMut(&str, &[u8]) -> Result<(), String>,
    exists: &dyn Fn(&str) -> bool,
) -> Result<Vec<serde_json::Value>, String> {
    let items = prepare_batch(session, ids, o)?;
    run_batch(items, o, to, write, exists, true, &mut |_, _| true)
}
