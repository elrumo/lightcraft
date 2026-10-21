//! Video clips: `video.info` (what the file says about itself) and `video.frame` (one picture of it).
//!
//! Both read the clip from its file in place, so they work for any size of clip and only on a computer that holds the
//! file (a photo that lives on a sync server is not a local file). The pictures are the clip as recorded: develop
//! settings are not applied to them yet.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, f64_or, str_param};
use crate::{Result, Session};

/// The clip `p` names (`id`, else the active photo) and the file it is in.
#[cfg(not(target_arch = "wasm32"))]
fn clip_of(s: &Session, c: &str, p: &Value) -> Result<(lightcraft_catalog::PhotoId, lightcraft_video::Clip<std::fs::File>)> {
    use lightcraft_catalog::{MediaKind, PhotoId, Source};
    let id = match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => PhotoId(v.as_u64().ok_or_else(|| bad(c, "`id` must be a photo id"))?),
        None => s.active().ok_or_else(|| bad(c, "no photo selected: pass `id`"))?,
    };
    let photo = s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
    if photo.kind != MediaKind::Video {
        return Err(bad(c, format!("photo {} is not a video", id.0)));
    }
    let Source::File { path } = &photo.source else { return Err(bad(c, "this clip is not a file on this computer")) };
    let clip = lightcraft_video::Clip::open_path(std::path::Path::new(path)).map_err(|e| bad(c, e.to_string()))?;
    Ok((id, clip))
}

#[cfg(not(target_arch = "wasm32"))]
fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, clip) = clip_of(s, "video.info", p)?;
    let v = clip.info();
    Ok(json!({
        "photo": id.0,
        "duration": v.duration,
        "width": v.width,
        "height": v.height,
        "rotation": v.rotation,
        "fps": v.fps,
        "codec": v.codec,
        "bitDepth": v.bit_depth,
        "hdr": v.hdr,
        "hasAudio": v.has_audio,
        "frames": v.frames,
        "created": v.created,
        "gps": v.gps,
        "make": v.make,
        "model": v.model,
        // why this version can't show the clip's pictures (null: it can)
        "unsupported": v.unsupported,
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn frame(s: &mut Session, p: &Value) -> Result<Value> {
    use base64::Engine as _;
    use lightcraft_raster::Rgba8;
    const C: &str = "video.frame";
    let (id, clip) = clip_of(s, C, p)?;
    let edge = f64_or(p, "maxEdge", 1024.0);
    if !(16.0..=4096.0).contains(&edge) {
        return Err(bad(C, "`maxEdge` must be between 16 and 4096"));
    }
    let edge = edge as usize;
    let time = p.get("time").filter(|v| !v.is_null());
    let img = match time {
        Some(t) => clip.frame_at(t.as_f64().filter(|t| t.is_finite()).ok_or_else(|| bad(C, "`time` must be a number of seconds"))?, edge),
        None => clip.poster(edge),
    }
    .map_err(|e| bad(C, e.to_string()))?;
    let img = if img.width.max(img.height) > edge {
        lightcraft_raster::resample::fit(&img, edge, edge, lightcraft_raster::resample::Filter::Mitchell)
    } else {
        img
    };
    // linear Rec.2020 → sRGB
    let m = lightcraft_color::WORKING.to_space(&lightcraft_color::SRGB).to_f32();
    let mut out = Rgba8::new(img.width, img.height);
    for (o, px) in out.data.iter_mut().zip(&img.data) {
        let q = [0, 1, 2].map(|r| m[r][0] * px[0] + m[r][1] * px[1] + m[r][2] * px[2]);
        let e = lightcraft_color::transfer::encode_srgb8;
        *o = [e(q[0]), e(q[1]), e(q[2]), 255];
    }
    let path = str_param(p, "path");
    let jpeg = path.is_some_and(|f| f.to_lowercase().ends_with(".jpg") || f.to_lowercase().ends_with(".jpeg"));
    let format = if jpeg { crate::export::ExportFormat::Jpeg } else { crate::export::ExportFormat::Png };
    let bytes =
        crate::export::encode_image(&out, &crate::export::ExportOptions { format, ..Default::default() }).map_err(|e| bad(C, e.to_string()))?;
    let mut r = json!({"photo": id.0, "time": time, "width": out.width, "height": out.height});
    match path {
        Some(f) => {
            lightcraft_catalog::safe_file::write_atomic(std::path::Path::new(f), &bytes).map_err(|e| bad(C, format!("{f}: {e}")))?;
            r["path"] = json!(f);
        }
        None => r["png"] = json!(base64::engine::general_purpose::STANDARD.encode(&bytes)),
    }
    Ok(r)
}

#[cfg(target_arch = "wasm32")]
fn info(_: &mut Session, _: &Value) -> Result<Value> {
    Err(bad("video.info", "reading a clip needs its file on disk"))
}

#[cfg(target_arch = "wasm32")]
fn frame(_: &mut Session, _: &Value) -> Result<Value> {
    Err(bad("video.frame", "reading a clip needs its file on disk"))
}

/// Where a clip can be played from: its file on this computer, or (for a synced clip whose original has not come yet)
/// nothing until `sync.downloadOriginals` brings it. A frontend with a system player plays `path`.
fn source(s: &mut Session, p: &Value) -> Result<Value> {
    use lightcraft_catalog::{MediaKind, PhotoId, Source};
    const C: &str = "video.source";
    let id = match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => PhotoId(v.as_u64().ok_or_else(|| bad(C, "`id` must be a photo id"))?),
        None => s.active().ok_or_else(|| bad(C, "no photo selected: pass `id`"))?,
    };
    let photo = s.catalog.photo(id).ok_or_else(|| bad(C, format!("no photo {}", id.0)))?;
    if photo.kind != MediaKind::Video {
        return Err(bad(C, format!("photo {} is not a video", id.0)));
    }
    let Source::File { path } = &photo.source else { return Err(bad(C, "this clip is not a file")) };
    // (a synced photo's path names its place on the server until its original is downloaded)
    let remote = path.starts_with(lightcraft_catalog::sync::PATH_PREFIX);
    let here = !remote && std::path::Path::new(path).is_file();
    Ok(json!({"photo": id.0, "path": here.then_some(path), "available": here, "remote": remote, "duration": photo.duration}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "video.source",
            "Video Source",
            [],
            None,
            "{id?} → {photo, path, available, remote, duration} — where a video clip can be played from: `path` is its file on this computer (null when it is not here: `remote` says it is a synced clip whose original is still on the server, which `sync.downloadOriginals` brings)",
            always,
            source
        ),
        cmd!(
            query "video.info",
            "Video Info",
            [],
            None,
            "{id?} → {photo, duration, width, height, rotation, fps, codec, bitDepth, hdr, hasAudio, frames, created, gps, make, model, unsupported} — what a video clip's file says about itself, read in place (the active photo when `id` is left out). `unsupported` says why this version can't show its pictures (a codec that isn't built in), else null",
            always,
            info
        ),
        cmd!(
            query "video.frame",
            "Video Frame",
            [],
            None,
            "{id?, time?: seconds, maxEdge?: 1024 (16-4096), path?} → {photo, time, width, height, png: base64 | path} — one picture of a video clip as recorded (no develop settings): the one at `time`, else the poster frame (a key frame about a tenth of the way in, skipping black ones). With `path` the picture is written there (PNG, or JPEG for .jpg) instead of returned",
            always,
            frame
        ),
    ]
}
