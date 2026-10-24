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
    let edit = s.develop_of(id).and_then(|d| d.video).unwrap_or_default();
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
        "trimStart": edit.trim_start,
        "trimEnd": edit.trim_end,
        "cover": edit.cover,
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn frame(s: &mut Session, p: &Value) -> Result<Value> {
    use base64::Engine as _;
    const C: &str = "video.frame";
    let (id, clip) = clip_of(s, C, p)?;
    let edge = f64_or(p, "maxEdge", 1024.0);
    if !(16.0..=4096.0).contains(&edge) {
        return Err(bad(C, "`maxEdge` must be between 16 and 4096"));
    }
    let edge = edge as usize;
    let time = p.get("time").filter(|v| !v.is_null());
    let cover = s.develop_of(id).and_then(|d| d.video).and_then(|v| v.cover);
    let img = match (time, cover) {
        (Some(t), _) => clip.frame_at(t.as_f64().filter(|t| t.is_finite()).ok_or_else(|| bad(C, "`time` must be a number of seconds"))?, edge),
        // (the poster frame the clip was given, else the library's pick)
        (None, Some(c)) => clip.frame_at(c, edge),
        (None, None) => clip.poster(edge),
    }
    .map_err(|e| bad(C, e.to_string()))?;
    let img = if img.width.max(img.height) > edge {
        lightcraft_raster::resample::fit(&img, edge, edge, lightcraft_raster::resample::Filter::Mitchell)
    } else {
        img
    };
    // (with `edits`, the picture as an export shows it: the develop settings applied and the crop)
    let photo = s.catalog.photo(id).ok_or_else(|| bad(C, "no photo"))?;
    let edits = p.get("edits").and_then(Value::as_bool).unwrap_or(false);
    let out = crate::video_export::finish_picture(
        img,
        &crate::media::source_info(photo),
        &photo.develop,
        &lightcraft_pipeline::RenderRequest::fit(edge, edge),
        edits,
    );
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

/// The clip `p` names (`id`, else the active photo), which must be a video: its id and length.
fn clip_target(s: &Session, c: &str, p: &Value) -> Result<(lightcraft_catalog::PhotoId, f64)> {
    use lightcraft_catalog::{MediaKind, PhotoId};
    let id = match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => PhotoId(v.as_u64().ok_or_else(|| bad(c, "`id` must be a photo id"))?),
        None => s.active().ok_or_else(|| bad(c, "no photo selected: pass `id`"))?,
    };
    let photo = s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
    if photo.kind != MediaKind::Video {
        return Err(bad(c, format!("photo {} is not a video", id.0)));
    }
    let length = photo.duration.filter(|d| d.is_finite() && *d > 0.0).ok_or_else(|| bad(c, "the length of this clip is not known"))?;
    Ok((id, length))
}

/// A time in the clip from `p[key]`: `Ok(None)` for `null`, an error for anything but a number of seconds in `0..=length`.
fn seconds(p: &Value, key: &str, c: &str, length: f64) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let t = v.as_f64().filter(|t| t.is_finite()).ok_or_else(|| bad(c, format!("`{key}` must be a number of seconds")))?;
            if !(0.0..=length).contains(&t) {
                return Err(bad(c, format!("`{key}` {t} is outside the clip (0 to {length:.2} seconds)")));
            }
            Ok(Some(t))
        }
    }
}

/// The shortest part of a clip a trim may keep.
const MIN_KEPT: f64 = 0.1;

fn set_trim(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.setTrim";
    let (id, length) = clip_target(s, C, p)?;
    if p.get("start").is_none() && p.get("end").is_none() {
        return Err(bad(C, "pass `start` and / or `end` (seconds; null puts that end back)"));
    }
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no photo"))?).clone();
    let mut v = d.video.unwrap_or_default();
    if p.get("start").is_some() {
        v.trim_start = seconds(p, "start", C, length)?.filter(|t| *t > 0.0);
    }
    if p.get("end").is_some() {
        v.trim_end = seconds(p, "end", C, length)?.filter(|t| *t < length);
    }
    if v.trim_end.unwrap_or(length) - v.trim_start.unwrap_or(0.0) < MIN_KEPT {
        return Err(bad(C, format!("the kept part must be at least {MIN_KEPT} seconds long")));
    }
    d.video = Some(v).filter(|v| !v.is_empty());
    // (trims of one clip in a row are one undo step: a frontend's trim bar sends one per drag)
    let before = (s.commits(), s.undo.len());
    s.set_develop(id, d, "Trim Video")?;
    s.join_repeated(id, "Trim Video", before);
    Ok(json!({"photo": id.0, "start": v.trim_start, "end": v.trim_end, "length": v.trim_end.unwrap_or(length) - v.trim_start.unwrap_or(0.0)}))
}

fn set_cover(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.setCover";
    let (id, length) = clip_target(s, C, p)?;
    if p.get("time").is_none() {
        return Err(bad(C, "pass `time` (seconds; null puts the library's own pick back)"));
    }
    let time = seconds(p, "time", C, length)?;
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(C, "no photo"))?).clone();
    let mut v = d.video.unwrap_or_default();
    v.cover = time;
    d.video = Some(v).filter(|v| !v.is_empty());
    s.set_develop(id, d, "Set Poster Frame")?;
    // (the pictures decoded for the old frame are of no use)
    s.media.forget(id);
    Ok(json!({"photo": id.0, "cover": time}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "video.export";
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(bad(C, "video export needs the desktop app, a phone or lightcraft-cli"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
        crate::video_export::start(s, p, wait).map_err(|e| bad(C, e))
    }
}

fn export_status(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(s.video_export.as_ref().map_or_else(|| json!({"running": false}), |r| r.json()))
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
    let edit = photo.develop.video.unwrap_or_default();
    Ok(json!({
        "photo": id.0,
        "path": here.then_some(path),
        "available": here,
        "remote": remote,
        "duration": photo.duration,
        "trimStart": edit.trim_start,
        "trimEnd": edit.trim_end,
        "cover": edit.cover,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "video.setTrim",
            "Trim Video",
            [],
            None,
            "{id?, start?: seconds | null, end?: seconds | null} → {photo, start, end, length} — keep only the part of a video clip between `start` and `end` (a missing key is left as it is, null puts that end back); at least 0.1 s must stay. Trims of one clip in a row, with nothing else between, are one undo step; the file is not touched, playback and export honour it",
            always,
            set_trim
        ),
        cmd!(
            "video.setCover",
            "Set Poster Frame",
            [],
            None,
            "{id?, time: seconds | null} → {photo, cover} — the picture that stands for a video clip in the grid and the loupe, as the clip shows at `time`; null puts the library's own pick back. One undo step",
            always,
            set_cover
        ),
        cmd!(
            "video.export",
            "Export Video",
            [],
            None,
            "{id?, path: \"….mp4\", maxEdge?: 64–8192 (the long edge; default the clip's own), quality?: \"smaller\"|\"balanced\"|\"better\", edits?: true (apply the develop settings to every picture), trim?: true (keep only the trimmed part), wait?: false} → {taskId, background, path, frames, width, height} (with `wait`: {path, frames, width, height, seconds, bytes, audio}) — write a video clip again as an H.264 MP4 with its edits, the part kept and its sound; in the background with a row in the activity stack (`activity.cancel {id}` stops it, leaving no file); `video.exportStatus` reads how it goes. Never overwrites an original",
            always,
            export
        ),
        cmd!(
            query "video.exportStatus",
            "Video Export Status",
            [],
            None,
            "{} → {running, taskId, path, done, total, cancelled, result?, error?} — the video export running or the last one",
            always,
            export_status
        ),
        cmd!(
            query "video.source",
            "Video Source",
            [],
            None,
            "{id?} → {photo, path, available, remote, duration, trimStart, trimEnd, cover} — where a video clip can be played from (and, from the library alone, how it is trimmed): `path` is its file on this computer (null when it is not here: `remote` says it is a synced clip whose original is still on the server, which `sync.downloadOriginals` brings)",
            always,
            source
        ),
        cmd!(
            query "video.info",
            "Video Info",
            [],
            None,
            "{id?} → {photo, duration, width, height, rotation, fps, codec, bitDepth, hdr, hasAudio, frames, created, gps, make, model, unsupported, trimStart, trimEnd, cover} — what a video clip's file says about itself, read in place (the active photo when `id` is left out). `unsupported` says why this version can't show its pictures (a codec that isn't built in), else null",
            always,
            info
        ),
        cmd!(
            query "video.frame",
            "Video Frame",
            [],
            None,
            "{id?, time?: seconds, maxEdge?: 1024 (16-4096), edits?: false, path?} → {photo, time, width, height, png: base64 | path} — one picture of a video clip: the one at `time`, else the poster frame (a key frame about a tenth of the way in, skipping black ones). As recorded, or with `edits` as an export shows it (develop settings and crop applied). With `path` the picture is written there (PNG, or JPEG for .jpg) instead of returned",
            always,
            frame
        ),
    ]
}
