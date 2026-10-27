use super::load::WbScaleOf;
use super::preview::embedded_preview_size;
use super::*;

pub(super) fn meta_of(m: &lightcraft_meta::Metadata) -> (Meta, Option<String>) {
    let shutter = m.exposure_time.map(|t| if t >= 1.0 { format!("{t:.0}") } else { format!("1/{:.0}", 1.0 / t) }).unwrap_or_default();
    let camera = [m.make.clone().unwrap_or_default(), m.model.clone().unwrap_or_default()].join(" ").trim().to_string();
    let meta = Meta {
        camera,
        // (a camera that writes no lens name, like Nikon's DSLRs, still says what range the lens covers)
        lens: m.lens_model.clone().or_else(|| m.lens_spec.and_then(lightcraft_meta::lens_spec_name)).unwrap_or_default(),
        focal_mm: m.focal_length.map(|f| f as f32),
        aperture: m.f_number.map(|f| f as f32),
        shutter,
        iso: m.iso,
        location: m.sublocation.clone().unwrap_or_default(),
        city: m.city.clone().unwrap_or_default(),
        state: m.state.clone().unwrap_or_default(),
        country: m.country.clone().unwrap_or_default(),
        alt_text: m.alt_text.clone().unwrap_or_default(),
        extended_description: m.extended_description.clone().unwrap_or_default(),
        gps: m.gps.as_ref().map(|g| (g.latitude, g.longitude)),
        title: m.title.clone().unwrap_or_default(),
        caption: m.caption.clone().unwrap_or_default(),
        copyright: m.copyright.clone().unwrap_or_default(),
        copyright_status: lightcraft_catalog::CopyrightStatus::from_marked(m.copyright_marked),
        usage_terms: m.usage_terms.clone().unwrap_or_default(),
        copyright_url: m.copyright_url.clone().unwrap_or_default(),
        creator: m.artist.clone().unwrap_or_default(),
        // paths from `lr:hierarchicalSubject`, names from `dc:subject` that aren't their levels
        keywords: lightcraft_catalog::keywords::from_file(&m.keywords, &m.hierarchical_keywords),
        regions: m.regions.clone(),
        focus: m.focus.clone(),
    };
    (meta, m.capture_time.as_ref().map(|d| d.to_iso()))
}

/// Lens corrections embedded in a raw's `OpcodeList3` (`WarpRectilinear`, `WarpRectilinear2`, `FixVignetteRadial`: a DNG's own, or the
/// raw reader's equivalent of the camera's correction, e.g. Panasonic / Leica RW2 distortion), re-expressed for the
/// default-cropped, EXIF-oriented image. These are the only "profile" corrections LightCraft applies.
pub fn embedded_lens(raw: &lightcraft_raw::RawInfo) -> Option<lightcraft_develop::EmbeddedLens> {
    use lightcraft_develop::{EmbeddedLens, EmbeddedVignette, EmbeddedWarp, EmbeddedWarpStage, WARP_STAGES};
    use lightcraft_geom::Point;
    use lightcraft_raw::Opcode;
    let (aw, ah) = (raw.active_area.width as f64, raw.active_area.height as f64);
    if aw < 2.0 || ah < 2.0 {
        return None;
    }
    let c = raw.crop.clipped(raw.active_area.width, raw.active_area.height);
    let (cx0, cy0, cw, ch) =
        if c.width == 0 || c.height == 0 { (0.0, 0.0, aw, ah) } else { (c.x as f64, c.y as f64, c.width as f64, c.height as f64) };
    let long = cw.max(ch);
    // opcode centres are relative to the (uncropped) active area, in pixel-index units
    let centre = |rel: [f64; 2]| -> (Point, f64) {
        let (px, py) = (rel[0] * (aw - 1.0), rel[1] * (ah - 1.0));
        let m = [(0.0, 0.0), (aw - 1.0, 0.0), (0.0, ah - 1.0), (aw - 1.0, ah - 1.0)]
            .iter()
            .map(|&(x, y)| (x - px).hypot(y - py))
            .fold(0.0, f64::max)
            .max(1e-9);
        (Point::new((px + 0.5 - cx0) / cw, (py + 0.5 - cy0) / ch), m / long)
    };
    let mut lens = EmbeddedLens::default();
    // opcodes in list order, minus the ones the DNG 1.6 skip rule drops (a WarpRectilinear that follows an optional
    // WarpRectilinear2)
    let ops: Vec<&Opcode> = lightcraft_raw::opcodes::applied_ops(&raw.opcodes.list3).collect();
    // A DNG with a WarpRectilinear2 keeps every warp opcode as a chain of stages (each warps the one before's output);
    // without one the first WarpRectilinear is the lens warp, as it always was.
    let chain = ops.iter().any(|op| matches!(op, Opcode::WarpRectilinear2 { .. }));
    let mut stages = Vec::new();
    for op in ops {
        match op {
            Opcode::WarpRectilinear2 { planes, center, reciprocal, .. } if !planes.is_empty() => {
                let (center, radius) = centre(*center);
                let p = |i: usize| planes[i.min(planes.len() - 1)];
                stages.push(EmbeddedWarpStage { planes: [p(0), p(1), p(2)], center, radius, reciprocal: *reciprocal });
            }
            Opcode::WarpRectilinear { planes, center } if chain && !planes.is_empty() => {
                // WarpRectilinear is the even-power case: kr0 + kr1 r² + kr2 r⁴ + kr3 r⁶, no radius limit
                let (center, radius) = centre(*center);
                let p = |i: usize| {
                    let k = planes[i.min(planes.len() - 1)];
                    let mut s = [0.0; lightcraft_raw::opcodes::WR2_LEN];
                    s[0] = k[0];
                    s[2] = k[1];
                    s[4] = k[2];
                    s[6] = k[3];
                    s[15] = k[4];
                    s[16] = k[5];
                    s[18] = 1e6;
                    s
                };
                stages.push(EmbeddedWarpStage { planes: [p(0), p(1), p(2)], center, radius, reciprocal: false });
            }
            Opcode::WarpRectilinear { planes, center } if !chain && !planes.is_empty() && lens.warp.is_none() => {
                let (center, radius) = centre(*center);
                let p = |i: usize| planes[i.min(planes.len() - 1)];
                lens.warp = Some(EmbeddedWarp { planes: [p(0), p(1), p(2)], center, radius });
            }
            Opcode::FixVignetteRadial { k, center } if lens.vignette.is_none() => {
                let (center, radius) = centre(*center);
                lens.vignette = Some(EmbeddedVignette { k: *k, center, radius });
            }
            _ => {}
        }
    }
    // (more stages than a lens keeps: the first ones; real files carry two)
    for (slot, st) in lens.stages.iter_mut().zip(stages.into_iter().take(WARP_STAGES)) {
        *slot = Some(st);
    }
    if lens.warp.is_none() && lens.vignette.is_none() && !lens.has_stages() {
        return None;
    }
    Some(lightcraft_pipeline::optics::reorient_lens(&lens, raw.orientation, cw, ch))
}

fn ext_upper(name: &str) -> String {
    std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default()
}

/// A raw's `format`: its file name's extension (it tells `RW2` from `RWL`, `3FR` from `IIQ`), unless the name
/// isn't a raw's (a ProRAW the system handed over as `IMG_8918.jpeg`, or no extension): then the container the
/// decoder found, when that names the format (the generic TIFF raws keep the extension).
fn raw_format_name(name: &str, found: lightcraft_raw::RawFormat) -> String {
    use lightcraft_raw::RawFormat as F;
    let ext = ext_upper(name);
    if !matches!(ext.as_str(), "" | "JPG" | "JPEG" | "PNG" | "TIF" | "TIFF" | "HEIC" | "HEIF" | "WEBP") {
        return ext;
    }
    match found {
        F::Dng => "DNG",
        F::Cr2 => "CR2",
        F::Cr3 => "CR3",
        F::Nef => "NEF",
        F::Nrw => "NRW",
        F::Arw => "ARW",
        F::Raf => "RAF",
        F::Orf => "ORF",
        F::Rw2 => "RW2",
        F::Pef => "PEF",
        F::Srw => "SRW",
        F::Crw => "CRW",
        F::Mrw => "MRW",
        F::X3f => "X3F",
        _ => return ext,
    }
    .to_string()
}

/// A preview-only raw's reason. An `.ori` is what OM Digital Solutions writes for the original frames of its High Res
/// and Pro Capture modes: an ORF container with the same compressed coding, which the decoder names an ORF.
fn preview_only_reason(name: &str, why: String) -> String {
    match why.strip_prefix("Olympus compressed ORF") {
        Some(rest) if ext_upper(name) == "ORI" => format!("Olympus compressed ORI (a High Res or Pro Capture original){rest}"),
        _ => why,
    }
}

/// "unrecognized file format" plus what the file holds (its size and first bytes), so a report names
/// an empty, placeholder or wrapped file instead of leaving a guess.
fn unrecognized(bytes: &[u8]) -> String {
    let head: Vec<String> = bytes.iter().take(16).map(|b| format!("{b:02X}")).collect();
    if head.is_empty() {
        return "unrecognized file format (empty file)".to_string();
    }
    format!("unrecognized file format ({} bytes, starts {})", bytes.len(), head.join(" "))
}

/// Probe a file's bytes: kind, dimensions (oriented), metadata. No pixel data is decompressed:
/// raws are described from their headers ([`lightcraft_raw::probe_info`]), other images too
/// ([`lightcraft_codecs::read_header`], which refuses truncated files but can't see damage inside
/// compressed data that is all there: such a file imports and shows as unreadable when rendered).
pub fn probe_bytes(name: &str, bytes: &[u8]) -> Result<ProbeInfo, String> {
    let content_hash = Some(lightcraft_preview::hash_bytes(bytes).to_string());
    if lightcraft_video::is_video(bytes) {
        return probe_video(name, bytes, bytes.len() as u64, content_hash);
    }
    let m = lightcraft_meta::extract(bytes);
    let (meta, captured) = meta_of(&m);
    if let Some(found) = lightcraft_raw::probe(bytes) {
        let raw = match lightcraft_raw::probe_info(bytes).map_err(|e| preview_reason(bytes, e)) {
            Ok(r) => r,
            Err(Ok(why)) => {
                // a raw variant we can't decode yet: describe it from its embedded preview
                let (w, h, orientation) = embedded_preview_size(bytes).ok_or(format!("unsupported raw ({why}) without an embedded preview"))?;
                return Ok(ProbeInfo {
                    width: w,
                    height: h,
                    format: raw_format_name(name, found),
                    kind: MediaKind::Raw,
                    file_size: bytes.len() as u64,
                    captured,
                    meta,
                    as_shot_wb: None,
                    content_hash,
                    xmp: lightcraft_meta::embedded(bytes).xmp,
                    preview_only: Some(preview_only_reason(name, why)),
                    // the preview's own frame: Lightroom's crop edges in a sidecar refer to it, like a decoded raw's
                    orientation: Some(orientation),
                    default_crop: (found == lightcraft_raw::RawFormat::Orf).then(|| default_crop_of(bytes, orientation, w, h)).flatten(),
                    ..Default::default()
                });
            }
            Err(Err(e)) => return Err(e),
        };
        let (w, h) = raw.developed_size();
        let (mut w, mut h) = (w.max(1) as u32, h.max(1) as u32);
        if raw.orientation.swaps_axes() {
            std::mem::swap(&mut w, &mut h);
        }
        let scale = WbScaleOf::new(raw.format, &raw.color, raw.wb_multipliers, &raw.metadata);
        let as_shot_wb = Some(scale.as_shot(lightcraft_raw::color::as_shot_white_xy_of(&raw)));
        let embedded_lens = crate::lens_profiles::for_raw(&raw, || m.clone());
        // an Olympus / OM body's aspect-ratio setting (3:2, 16:9, 1:1): the whole 4:3 area is developed, the camera's JPEG is cut
        let default_crop = (found == lightcraft_raw::RawFormat::Orf).then(|| default_crop_of(bytes, raw.orientation, w, h)).flatten();
        return Ok(ProbeInfo {
            embedded_lens,
            default_crop,
            width: w,
            height: h,
            format: raw_format_name(name, found),
            kind: MediaKind::Raw,
            file_size: bytes.len() as u64,
            captured,
            file_modified: None,
            locked: false,
            meta,
            as_shot_wb,
            content_hash,
            xmp: lightcraft_meta::embedded(bytes).xmp,
            preview_only: None,
            orientation: Some(raw.orientation),
            duration: None,
        });
    }
    let Some(fmt) = lightcraft_codecs::sniff(bytes) else { return Err(unrecognized(bytes)) };
    if let Some(why) = fmt.not_decodable() {
        // e.g. "Heif files can't be opened: HEIC/HEIF support isn't included in this build of LightCraft"
        return Err(format!("{fmt:?} files can't be opened: {why}"));
    }
    // headers only (issue #367: decoding the pixels was nearly all of an import's CPU time)
    let header = lightcraft_codecs::read_header(bytes).map_err(|e| e.to_string())?;
    let o = Orientation::from_exif(header.orientation);
    let (mut w, mut h) = (header.width, header.height);
    if o.swaps_axes() {
        std::mem::swap(&mut w, &mut h);
    }
    let format = match ext_upper(name).as_str() {
        "JPG" | "JPEG" => "JPEG".to_string(),
        "" => format!("{fmt:?}").to_uppercase(),
        e => e.to_string(),
    };
    Ok(ProbeInfo {
        width: w,
        height: h,
        format,
        kind: MediaKind::Image,
        file_size: bytes.len() as u64,
        captured,
        file_modified: None,
        locked: false,
        meta,
        as_shot_wb: None,
        content_hash,
        embedded_lens: None,
        xmp: None,
        preview_only: None,
        orientation: Some(o),
        duration: None,
        default_crop: None,
    })
}

/// Describe a video clip from `src` (its bytes or its file): size, length, recording date and place come from the
/// header (`moov`), so a clip of any size is never read whole. `content_hash` is the hash of the whole file.
pub fn probe_video<S: lightcraft_video::ByteSource>(name: &str, src: S, size: u64, content_hash: Option<String>) -> Result<ProbeInfo, String> {
    let v = lightcraft_video::probe(src).map_err(|e| e.to_string())?;
    let captured = v.created.as_deref().and_then(lightcraft_meta::DateTime::parse_iso).map(|d| d.to_iso());
    let meta = Meta {
        camera: [v.make.clone().unwrap_or_default(), v.model.clone().unwrap_or_default()].join(" ").trim().to_string(),
        gps: v.gps,
        ..Meta::default()
    };
    let format = match ext_upper(name).as_str() {
        "" => "MP4".to_string(),
        e => e.to_string(),
    };
    Ok(ProbeInfo {
        width: v.width,
        height: v.height,
        format,
        kind: MediaKind::Video,
        file_size: size,
        captured,
        meta,
        content_hash,
        duration: Some(v.duration),
        ..Default::default()
    })
}

/// `path` with a time in the clip after it: a source path that names one picture of a clip rather than its poster frame
/// (`media::source_ref` makes it for a clip with a poster frame of its own, the loader reads it). A NUL separates them: no
/// file name has one.
pub fn at_time(path: &str, seconds: f64) -> String {
    format!("{path}\0{seconds}")
}

/// A source path split into the file and the time in it, if it has one ([`at_time`]).
pub fn split_time(path: &str) -> (&str, Option<f64>) {
    match path.split_once('\0') {
        Some((file, t)) => (file, t.parse::<f64>().ok().filter(|t| t.is_finite() && *t >= 0.0)),
        None => (path, None),
    }
}

/// Is `path` a video clip (MP4 / MOV)? Reads 12 bytes.
pub fn is_video_file(path: &std::path::Path) -> bool {
    head_of(&path.to_string_lossy()).is_some_and(|h| lightcraft_video::is_video(&h))
}

/// [`probe_video`] for the clip at `path`, read in place (the header only), with a `content_hash` the caller already has.
#[cfg(not(target_arch = "wasm32"))]
pub fn probe_video_at(path: &std::path::Path, content_hash: Option<String>) -> Result<ProbeInfo, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    probe_video(&path.to_string_lossy(), f, size, content_hash)
}

/// The first bytes of a file, for sniffing without reading it.
fn head_of(path: &str) -> Option<[u8; 12]> {
    use std::io::Read;
    let mut head = [0u8; 12];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    Some(head)
}

/// [`probe_video`] for a file on disk when it is a video clip (`None` when it is not): the file is hashed in chunks and
/// its header read in place, never held whole.
#[cfg(not(target_arch = "wasm32"))]
fn probe_video_path(path: &str) -> Option<Result<ProbeInfo, String>> {
    use std::io::Read;
    if !lightcraft_video::is_video(&head_of(path)?) {
        return None;
    }
    Some((|| {
        let mut f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let size = f.metadata().map_err(|e| format!("{path}: {e}"))?.len();
        let mut hasher = lightcraft_preview::Hasher128::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    hasher.update(buf.get(..n).unwrap_or(&[]));
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("{path}: {e}")),
            }
        }
        probe_video(path, f, size, Some(hasher.finish().to_string()))
    })())
}

/// A video clip's poster frame as a source image (display orientation, at most about twice `max_edge` before the fit).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_video(path: &str, max_edge: usize, time: Option<f64>) -> Result<(Rgb32f, SourceInfo), String> {
    let clip = lightcraft_video::Clip::open_path(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    let img = match time {
        Some(t) => clip.frame_at(t, max_edge),
        None => clip.poster(max_edge),
    }
    .map_err(|e| e.to_string())?;
    let img = if img.width.max(img.height) > max_edge { fit(&img, max_edge, max_edge, Filter::Mitchell) } else { img };
    Ok((img, SourceInfo::default()))
}

/// Why a raw that failed to decode should show its embedded preview instead (`Ok`), or the error
/// to report (`Err`). Variants we can't decode yet always fall back. So does any CR3 error: the CRX
/// decoder is verified on few bodies, and every CR3 opened from its embedded JPEG before it existed.
/// So does any other failure of a recognised raw (damaged or oversized raw data, unreadable TIFF
/// structure): its preview may still be intact. Only a file that is not a raw reports an error.
pub(super) fn preview_reason(bytes: &[u8], e: lightcraft_raw::RawError) -> Result<String, String> {
    use lightcraft_raw::RawError;
    match e {
        RawError::Unsupported(why) => Ok(why),
        e if lightcraft_raw::probe(bytes) == Some(lightcraft_raw::RawFormat::Cr3) => Ok(format!("CR3 {e}")),
        RawError::NotRaw => Err(RawError::NotRaw.to_string()),
        e => Ok(e.to_string()),
    }
}
