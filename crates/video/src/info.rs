//! What a clip is: read from the `moov` box alone.

use filmcraft_isobmff::{AvcConfig, ByteSource, CodecConfig, ColorInfo, HevcConfig, Mp4File, Track, TrackKind};

use crate::{Error, Result, guarded};

/// The facts about a clip the library keeps. Sizes are as displayed (after the rotation).
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// Length in seconds.
    pub duration: f64,
    /// Displayed width and height in pixels.
    pub width: u32,
    pub height: u32,
    /// Clockwise quarter turns the file asks for (an iPhone held upright records 1).
    pub rotation: u8,
    /// Average frames per second (0 when unknown).
    pub fps: f64,
    /// `h264`, `hevc`, `av1`, `vp9`, `prores`, `mjpeg`, `dnxhd`, `apv` or `unknown`.
    pub codec: &'static str,
    /// Bits per sample of the picture (8 when the codec record doesn't say).
    pub bit_depth: u8,
    /// The picture is HDR (PQ or HLG transfer).
    pub hdr: bool,
    pub has_audio: bool,
    /// Number of video samples (frames).
    pub frames: usize,
    /// When it was recorded, as the file writes it (`2026-10-17T14:03:11+0200`), if it says.
    pub created: Option<String>,
    /// Where it was recorded (latitude, longitude in degrees), if it says.
    pub gps: Option<(f64, f64)>,
    pub make: Option<String>,
    pub model: Option<String>,
    /// Why this build can't decode the picture (a codec or format that is not built in); `None` when it can.
    pub unsupported: Option<String>,
}

/// Does `head` (a file's first bytes, 12 or more) start like an MP4 / MOV movie? HEIF stills (`heic`, `mif1`, `avif`)
/// and audio-only files (`M4A `) are not videos.
pub fn is_video(head: &[u8]) -> bool {
    let Some(kind) = head.get(4..8) else { return false };
    if kind == b"ftyp" {
        let Some(brand) = head.get(8..12) else { return false };
        const BRANDS: &[&[u8; 4]] = &[
            b"isom", b"iso2", b"iso3", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1", b"M4V ", b"M4VH", b"M4VP", b"qt  ", b"3gp4", b"3gp5",
            b"3gp6", b"3gp7", b"3g2a", b"mmp4", b"f4v ", b"dash", b"msdh", b"hvc1", b"hev1",
        ];
        return BRANDS.iter().any(|b| brand == &b[..]);
    }
    // QuickTime files from older cameras and editors start with a movie atom rather than `ftyp`
    matches!(kind, b"moov" | b"mdat" | b"wide")
}

/// Read a clip's header from `src`.
pub fn probe<S: ByteSource>(src: S) -> Result<VideoInfo> {
    guarded("reading the header", || {
        let mp4 = filmcraft_isobmff::open(src)?;
        info_of(&mp4)
    })
}

/// [`probe`] for a file on disk (reads only the box headers and `moov`).
#[cfg(not(target_arch = "wasm32"))]
pub fn probe_path(path: &std::path::Path) -> Result<VideoInfo> {
    let f = std::fs::File::open(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    probe(f)
}

/// The main video track: the first enabled one that has pictures.
pub(crate) fn video_track(mp4: &Mp4File) -> Option<(usize, &Track)> {
    mp4.tracks.iter().enumerate().find(|(_, t)| t.kind == TrackKind::Video && t.enabled && !t.samples.is_empty())
}

pub(crate) fn info_of(mp4: &Mp4File) -> Result<VideoInfo> {
    let (_, t) = video_track(mp4).ok_or_else(|| Error::Malformed("no video track".into()))?;
    let rotation = t.display_rotation().unwrap_or(0);
    let (mut w, mut h) = (t.width, t.height);
    if w == 0 || h == 0 {
        let v = t.video();
        (w, h) = v.map_or((0, 0), |v| (u32::from(v.width), u32::from(v.height)));
    }
    if w == 0 || h == 0 {
        return Err(Error::Malformed("the video track has no size".into()));
    }
    if rotation % 2 == 1 {
        std::mem::swap(&mut w, &mut h);
    }
    let track_secs = if t.timescale > 0 { t.duration as f64 / f64::from(t.timescale) } else { 0.0 };
    let movie_secs = if mp4.timescale > 0 { mp4.duration as f64 / f64::from(mp4.timescale) } else { 0.0 };
    let duration = [movie_secs, track_secs].into_iter().find(|d| d.is_finite() && *d > 0.0).unwrap_or(0.0);
    let fps = if track_secs > 0.0 { t.samples.len() as f64 / track_secs } else { 0.0 };
    let meta = &mp4.metadata;
    let color = t.video().and_then(|v| v.color.as_ref());
    let transfer = match color {
        Some(ColorInfo::Nclx { transfer, .. } | ColorInfo::Nclc { transfer, .. }) => *transfer,
        _ => 2,
    };
    let (codec, bit_depth, unsupported) = match t.codec() {
        Some(CodecConfig::Avc(c)) => ("h264", avc_depth(c), avc_unsupported(c)),
        Some(CodecConfig::Hevc(c)) => ("hevc", c.bit_depth_luma.max(8), hevc_unsupported(c)),
        Some(CodecConfig::Av1(_)) => ("av1", 8, Some("AV1 video is not supported yet".to_string())),
        Some(CodecConfig::Vp9(_)) => ("vp9", 8, Some("VP9 video is not supported yet".to_string())),
        Some(CodecConfig::ProRes { .. }) => ("prores", 10, Some("ProRes video is not supported yet".to_string())),
        Some(CodecConfig::Jpeg { .. }) => ("mjpeg", 8, Some("Motion JPEG video is not supported yet".to_string())),
        Some(CodecConfig::Dnx { .. }) => ("dnxhd", 8, Some("DNxHD video is not supported yet".to_string())),
        Some(CodecConfig::Apv(_)) => ("apv", 10, Some("APV video is not supported yet".to_string())),
        _ => ("unknown", 8, Some("this video codec is not supported".to_string())),
    };
    let text = |k: &[&str]| k.iter().find_map(|k| meta.get(k)).map(str::to_string);
    Ok(VideoInfo {
        duration,
        width: w,
        height: h,
        rotation,
        fps: if fps.is_finite() { fps } else { 0.0 },
        codec,
        bit_depth,
        hdr: matches!(transfer, 16 | 18),
        has_audio: mp4.tracks.iter().any(|a| a.kind == TrackKind::Audio && !a.samples.is_empty()),
        frames: t.samples.len(),
        created: text(&["com.apple.quicktime.creationdate"]),
        gps: text(&["com.apple.quicktime.location.ISO6709", "©xyz"]).and_then(|s| parse_iso6709(&s)),
        make: text(&["com.apple.quicktime.make", "©mak"]),
        model: text(&["com.apple.quicktime.model", "©mod"]),
        unsupported,
    })
}

fn avc_depth(c: &AvcConfig) -> u8 {
    // High 10 / 4:2:2 / 4:4:4 profiles carry the depth in the record's extension: chroma byte, luma byte, chroma byte
    match c.profile {
        110 | 122 | 244 => c.ext.get(1).map_or(10, |b| (b & 7) + 8),
        _ => 8,
    }
}

fn avc_unsupported(c: &AvcConfig) -> Option<String> {
    if !cfg!(feature = "h264") {
        return Some("H.264 video is not built into this version".into());
    }
    match c.profile {
        // Baseline, Main, Extended, High: 8-bit 4:2:0
        66 | 77 | 88 | 100 => None,
        110 => Some("10-bit H.264 video is not supported yet".into()),
        122 => Some("4:2:2 H.264 video is not supported yet".into()),
        244 | 44 => Some("4:4:4 H.264 video is not supported yet".into()),
        p => Some(format!("H.264 profile {p} is not supported")),
    }
}

fn hevc_unsupported(c: &HevcConfig) -> Option<String> {
    if !cfg!(feature = "hevc") {
        return Some("HEVC video is not built into this version".into());
    }
    if c.chroma_format_idc != 1 {
        return Some("only 4:2:0 HEVC video is supported".into());
    }
    if c.bit_depth_luma > 10 || c.bit_depth_chroma > 10 {
        return Some("HEVC video above 10 bits is not supported".into());
    }
    None
}

/// `+48.8584+002.2945+010.000/` (ISO 6709, decimal degrees) → (latitude, longitude).
pub(crate) fn parse_iso6709(s: &str) -> Option<(f64, f64)> {
    let s = s.trim().trim_end_matches('/');
    let marks: Vec<usize> = s.char_indices().filter(|(_, c)| *c == '+' || *c == '-').map(|(i, _)| i).collect();
    let part = |n: usize| -> Option<f64> {
        let from = *marks.get(n)?;
        let to = marks.get(n + 1).copied().unwrap_or(s.len());
        s.get(from..to)?.parse::<f64>().ok().filter(|v| v.is_finite())
    };
    let (lat, lon) = (part(0)?, part(1)?);
    (lat.abs() <= 90.0 && lon.abs() <= 180.0).then_some((lat, lon))
}
