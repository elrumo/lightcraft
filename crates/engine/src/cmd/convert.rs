//! Convert to DNG: a raw photo's original is re-encoded as a lossless DNG (its develop settings
//! embedded as XMP) next to it, and the photo is relinked to the DNG. The original stays on
//! disk; undo relinks the photo to it.

use std::path::Path;

use lightcraft_catalog::{MediaKind, Op, PhotoId, Source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_active, has_selection};
use crate::{Result, Session};

/// `<stem>.dng`, then `<stem>-2.dng`, `<stem>-3.dng`… next to `path`.
fn dng_names(path: &str) -> impl Iterator<Item = std::path::PathBuf> {
    let p = Path::new(path);
    let dir = p.parent().unwrap_or(Path::new("")).to_path_buf();
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "photo".into());
    (1u64..).map(move |n| if n == 1 { dir.join(format!("{stem}.dng")) } else { dir.join(format!("{stem}-{n}.dng")) })
}

/// Write the DNG for raw file `path` (develop settings in `packet`) next to it under a free name;
/// returns the new path. An empty `packet` writes no XMP.
///
/// The DNG is checked before anyone relies on it (Convert to DNG relinks the photo to it, Copy as
/// DNG deletes the raw copy): the encoded file must decode to the same raw data, and it is
/// written to a temp file, synced, read back identical and only then given its name — never
/// replacing an existing file. On any failure no DNG is left behind and the raw stays in use.
pub(crate) fn write_dng_for(s: &Session, path: &str, packet: String) -> std::result::Result<String, String> {
    write_dng_with(s.media.file_bytes.as_ref(), path, packet)
}

/// [`write_dng_for`] without the session (imports convert on a worker thread).
pub(crate) fn write_dng_with(file_bytes: Option<&crate::merge::ByteReader>, path: &str, packet: String) -> std::result::Result<String, String> {
    let bytes = match file_bytes {
        Some(r) => r(path)?,
        None => std::fs::read(path).map_err(|e| format!("{path}: {e}"))?,
    };
    let mut raw = lightcraft_raw::decode(&bytes).map_err(|e| format!("{path}: {e}"))?;
    crate::camera_preview::with_starting_colour(&mut raw, &bytes);
    drop(bytes);
    let dng = lightcraft_raw::write_dng(&raw, &lightcraft_raw::DngWriteOptions { xmp: (!packet.is_empty()).then_some(packet), ..Default::default() })
        .map_err(|e| e.to_string())?;
    let back = lightcraft_raw::decode(&dng).map_err(|e| format!("{path}: the DNG written for it doesn't decode ({e}); the raw is kept"))?;
    let same = match (&back.data, &raw.data) {
        (lightcraft_raw::RawData::U16(a), lightcraft_raw::RawData::U16(b)) => a == b,
        (lightcraft_raw::RawData::F32(a), lightcraft_raw::RawData::F32(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
        }
        _ => false,
    };
    if (back.width, back.height, back.cpp) != (raw.width, raw.height, raw.cpp) || !same {
        return Err(format!("{path}: the DNG written for it doesn't hold the same raw data; the raw is kept"));
    }
    drop((raw, back));
    let out = lightcraft_catalog::safe_file::write_new_unique(&mut dng_names(path), &dng)
        .map_err(|e| format!("{path}: could not write its DNG ({e}); the raw is kept"))?;
    Ok(out.to_string_lossy().to_string())
}

fn convert(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.convertToDng";
    let ids = s.targets(p)?;
    let mut ops = Vec::new();
    let mut converted = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        let Source::File { path } = &ph.source else {
            skipped.push(json!([id.0, "not a file"]));
            continue;
        };
        if ph.kind != MediaKind::Raw || ph.format.eq_ignore_ascii_case("DNG") || ph.copy_of.is_some() {
            skipped.push(json!([
                id.0,
                if ph.kind != MediaKind::Raw {
                    "not a raw photo"
                } else if ph.copy_of.is_some() {
                    "a virtual copy"
                } else {
                    "already a DNG"
                }
            ]));
            continue;
        }
        let packet = crate::sidecar::sidecar_packet(&ph, &s.catalog);
        match write_dng_for(s, path, packet) {
            Ok(out) => {
                let file_name = Path::new(&out).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                // virtual copies of this photo follow it to the DNG
                for c in s.catalog.photos().filter(|c| c.copy_of == Some(id)) {
                    ops.push(Op::Relink {
                        id: c.id,
                        file_name: file_name.clone(),
                        source: Source::File { path: out.clone() },
                        format: Some("DNG".into()),
                    });
                }
                ops.push(Op::Relink { id, file_name, source: Source::File { path: out.clone() }, format: Some("DNG".into()) });
                converted.push(json!({"id": id.0, "path": out, "original": path}));
            }
            Err(e) => skipped.push(json!([id.0, e])),
        }
    }
    if !ops.is_empty() {
        let n = converted.len();
        s.commit(&format!("Convert {n} Photo{} to DNG", if n == 1 { "" } else { "s" }), Op::Batch { ops }).map_err(|e| bad(C, e.to_string()))?;
    }
    Ok(json!({"converted": converted, "skipped": skipped}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "photo.convertToDng",
        "Convert to DNG",
        ["Photo"],
        None,
        "{ids?} — write each raw photo as a lossless DNG next to it (settings embedded) and relink the photo to it; originals are kept → {converted: [{id, path, original}], skipped}",
        has_selection,
        convert
    )]
}

/// How Photo ▸ Edit In hands a photo to the editor (`mode` of `photo.editExternal`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditMode {
    /// A copy rendered with the photo's edits (any photo): Edit a Copy with LightCraft Adjustments.
    Render,
    /// A copy of the photo's file, without the edits: Edit a Copy.
    Copy,
    /// The photo's own file: Edit Original. Its edits stay on top of what the editor saves.
    Original,
}

impl EditMode {
    fn name(self) -> &'static str {
        match self {
            EditMode::Render => "render",
            EditMode::Copy => "copy",
            EditMode::Original => "original",
        }
    }
}

/// Whether Photo ▸ Edit In can hand over photo `ph`'s own file (Edit Original, Edit a Copy): an
/// image file a pixel editor opens and saves as it is (TIFF, PSD, JPEG, PNG…), not a raw (developed
/// from its sensor data), a video or a generated demo photo.
pub fn can_edit_original(ph: &lightcraft_catalog::Photo) -> bool {
    ph.kind == MediaKind::Image && matches!(ph.source, Source::File { .. })
}

/// Edit in an external editor, the engine half. `mode` render (default): render the photo with its
/// edits as a 16-bit TIFF (or PSD, or 8-bit) next to the original (`<name>-Edit.tif`, never
/// overwriting), add it to the library and stack it on top of the original; copy: the same with a
/// plain copy of the file; original: the photo's own file, nothing made. The host opens the files in
/// the editor and keeps `track`, for `photo.checkExternalEdits` to bring back what the editor saves.
fn edit_external(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.editExternal";
    let mut ids = s.targets(p)?;
    ids.dedup();
    if ids.is_empty() {
        return Err(bad(C, "no active photo"));
    }
    if ids.len() > MAX_EDIT_COPIES {
        return Err(bad(C, format!("{} photos selected: edit at most {MAX_EDIT_COPIES} at a time", ids.len())));
    }
    let mode = match super::str_param(p, "mode").unwrap_or("render") {
        "render" => EditMode::Render,
        "copy" => EditMode::Copy,
        "original" => EditMode::Original,
        m => return Err(bad(C, format!("mode {m:?}: use render, copy or original"))),
    };
    if mode != EditMode::Render
        && let Some(ph) = ids.iter().filter_map(|id| s.catalog.photo(*id)).find(|ph| !can_edit_original(ph))
    {
        return Err(bad(
            C,
            format!(
                "{}: only an image file (TIFF, PSD, JPEG, PNG…) can be edited as it is, not a raw, a video or a generated photo: use mode render",
                ph.file_name
            ),
        ));
    }
    let space = match super::str_param(p, "colorSpace").unwrap_or("adobeRgb") {
        "srgb" => lightcraft_pipeline::OutputSpace::Srgb,
        "displayP3" => lightcraft_pipeline::OutputSpace::DisplayP3,
        "prophoto" | "proPhoto" => lightcraft_pipeline::OutputSpace::ProPhoto,
        _ => lightcraft_pipeline::OutputSpace::AdobeRgb,
    };
    let format = match super::str_param(p, "format").unwrap_or("tiff") {
        "tiff" | "tif" => crate::export::ExportFormat::Tiff,
        "psd" => crate::export::ExportFormat::Psd,
        f => return Err(bad(C, format!("format {f:?}: use tiff or psd"))),
    };
    let bit_depth = match p.get("bitDepth").and_then(Value::as_u64).unwrap_or(16) {
        8 => 8,
        16 => 16,
        d => return Err(bad(C, format!("bitDepth {d}: use 8 or 16"))),
    };
    let opts = crate::export::ExportOptions {
        format,
        bit_depth: Some(bit_depth),
        color_space: space,
        naming: "{name}-Edit".into(),
        conflict: crate::export::Conflict::Unique,
        ..Default::default()
    };
    let mut copies = Vec::new();
    for id in ids {
        copies.push(match mode {
            EditMode::Render => edit_copy(s, p, id, &opts, C)?,
            EditMode::Copy => {
                let (new, path) = copy_file_photo(s, C, id, "-Edit", false, "Edit a Copy")?;
                let _ = s.execute("stack.group", &json!({"ids": [new.0, id.0], "top": new.0, "collapsed": false}));
                json!({"path": path, "id": new.0, "original": id.0})
            }
            EditMode::Original => {
                let ph = s.catalog.photo(id).ok_or_else(|| bad(C, format!("no photo {}", id.0)))?;
                let Source::File { path } = &ph.source else { return Err(bad(C, "a generated demo photo has no file")) };
                json!({"path": path, "id": id.0, "original": id.0})
            }
        });
    }
    if mode != EditMode::Original {
        let new: Vec<PhotoId> = copies.iter().filter_map(|c| c["id"].as_u64()).map(PhotoId).collect();
        s.selection = crate::Selection { active: new.first().copied(), ids: new };
    }
    let now = now_ms();
    let track: Vec<TrackedEdit> = copies.iter().filter_map(|c| c["path"].as_str()).map(|path| TrackedEdit::new(path, now)).collect();
    let first = copies.first().cloned().unwrap_or(Value::Null);
    Ok(json!({"path": first["path"], "id": first["id"], "original": first["original"], "mode": mode.name(), "copies": copies, "track": track}))
}

/// Most photos one Edit In renders (each becomes a full-size file on disk and a library photo).
const MAX_EDIT_COPIES: usize = 50;

/// One photo's edit copy for [`edit_external`]: rendered with `opts` next to its original (a demo
/// photo into `dir`), added to the library and stacked on top of the original, expanded.
fn edit_copy(s: &mut Session, p: &Value, id: PhotoId, opts: &crate::export::ExportOptions, cmd: &'static str) -> Result<Value> {
    let ph = s.catalog.photo(id).cloned().ok_or_else(|| bad(cmd, format!("no photo {}", id.0)))?;
    let dir = match &ph.source {
        Source::File { path } => Path::new(path).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
        Source::Demo { .. } => match super::str_param(p, "dir") {
            Some(d) => d.to_string(),
            None => return Err(bad(cmd, "a generated demo photo has no folder: give `dir`")),
        },
    };
    let mut written = Vec::new();
    let mut write = |path: &str, bytes: &[u8]| -> std::result::Result<(), String> {
        // durable: the TIFF becomes a library photo the external editor then changes
        crate::export::write_file_durable(path, bytes)?;
        written.push(path.to_string());
        Ok(())
    };
    let to = crate::export::Destination { dir, exact: None };
    crate::export::export_batch(s, &[id], opts, &to, &mut write, &|path| Path::new(path).exists()).map_err(|e| bad(cmd, e))?;
    let ext = format!(".{}", opts.format.extension());
    let out = written.into_iter().find(|w| w.ends_with(&ext)).ok_or_else(|| bad(cmd, "nothing was written"))?;
    let r = s.execute("library.import", &json!({"paths": [out]}))?;
    let new = r["imported"].get(0).and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the edit copy could not be added"))?;
    // stack: the edit on top of the original, expanded so both show
    let _ = s.execute("stack.group", &json!({"ids": [new, id.0], "top": new, "collapsed": false}));
    Ok(json!({"path": out, "id": new, "original": id.0}))
}

/// A file handed to an external editor (Photo ▸ Edit In), watched for what the editor saves
/// ([`check_external`]). The host keeps the list (the app, in its settings file, so the watch
/// outlives a restart); `photo.editExternal` gives the entries and `photo.checkExternalEdits`
/// returns the list brought up to date.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackedEdit {
    pub path: String,
    /// A file the editor saves beside it under its name after this (ms since the Unix epoch) is its
    /// Save As: when it was handed over, then each time it was checked. `None`: now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// When it was handed over, or last seen changed: watched for [`TRACK_FOR_MS`] after. `None`: now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touched: Option<u64>,
    /// Its size and modification time (ms) when last read: only a change is read again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
}

impl TrackedEdit {
    /// `path`, handed over at `now` (ms), as it is on disk now.
    fn new(path: &str, now: u64) -> TrackedEdit {
        let (size, modified) = stamp(path).unzip();
        TrackedEdit { path: path.to_string(), since: Some(now), touched: Some(now), size, modified }
    }

    /// The host's watch list `list` with `new` (the `track` of `photo.editExternal`) on its end: a file
    /// already watched is replaced, and at most [`MAX_TRACKED`] stay (the oldest are dropped first).
    pub fn merge(list: &mut Vec<TrackedEdit>, new: Vec<TrackedEdit>) {
        list.retain(|t| !new.iter().any(|n| n.path == t.path));
        list.extend(new);
        let extra = list.len().saturating_sub(MAX_TRACKED);
        list.drain(..extra);
    }
}

/// How long a file handed to an external editor is watched after it was handed over or last
/// seen changed: two weeks.
const TRACK_FOR_MS: u64 = 14 * 24 * 3600 * 1000;
/// Most files watched at once ([`TrackedEdit`]): the oldest are dropped first.
pub const MAX_TRACKED: usize = 200;
/// Slack when comparing a file's time with the clock (FAT keeps times to 2 s).
const CLOCK_SLACK_MS: u64 = 2000;
/// What an editor's Save As writes that the library takes in: rendered image formats.
const SAVE_AS_EXTENSIONS: &[&str] = &["tif", "tiff", "psd", "png", "jpg", "jpeg", "webp", "jxl", "heic", "heif", "gif", "bmp"];

/// Wall-clock milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A file's time in ms since the Unix epoch (0 before it).
fn millis(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// (size, modified ms) of the file at `path`, `None` when there is no such file.
fn stamp(path: &str) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
    Some((m.len(), m.modified().map_or(0, millis)))
}

/// Whether a library photo shows the file at `path` (one in Recently Deleted included: importing
/// it would bring that photo back).
fn in_library(s: &Session, path: &str) -> bool {
    s.catalog.photos().any(|p| matches!(&p.source, Source::File { path: q } if q == path))
}

/// A folder's files, listed once per check: (name, path, modified ms).
type Listing = Vec<(std::ffi::OsString, String, u64)>;

/// What the editor saved beside `path` under its name with another extension at or after `since`
/// (Save As: `pier-Edit.psd` beside `pier-Edit.tif`), and the library doesn't have.
fn saved_as(s: &Session, path: &str, since: u64, listings: &mut std::collections::HashMap<std::path::PathBuf, Listing>) -> Vec<String> {
    let p = Path::new(path);
    let (Some(dir), Some(stem), Some(name)) = (p.parent(), p.file_stem(), p.file_name()) else { return Vec::new() };
    let listing = listings.entry(dir.to_path_buf()).or_insert_with(|| {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok().filter(std::fs::Metadata::is_file)?;
                Some((e.file_name(), e.path().to_string_lossy().to_string(), m.modified().map_or(0, millis)))
            })
            .collect()
    });
    let mut found: Vec<String> = listing
        .iter()
        .filter(|(n, q, modified)| {
            let q = Path::new(q);
            n.as_os_str() != name
                && q.file_stem() == Some(stem)
                && q.extension().is_some_and(|e| SAVE_AS_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
                && modified.saturating_add(CLOCK_SLACK_MS) >= since
        })
        .map(|(_, q, _)| q.clone())
        .filter(|q| !in_library(s, q))
        .collect();
    found.sort();
    found
}

/// What the external editor saved (Photo ▸ Edit In), for the files handed to it: a changed file
/// is read again (`photo.reload`; only those whose size or time changed are read), and a file
/// saved beside one under its name with another extension (Save As) is imported and stacked on
/// top of its photo. Files that are gone, or not watched any more, drop off the list; files that
/// aren't photos of this library are left as they are (another library may have them).
fn check_external(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.checkExternalEdits";
    let files: Vec<TrackedEdit> = match p.get("files") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| bad(C, format!("files: {e}")))?,
    };
    let now = now_ms();
    let selection = s.selection.clone();
    let mut listings = std::collections::HashMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<TrackedEdit> = Vec::new();
    // (index in `out`, its photos, the stamp read now) for the files that changed
    let mut changed: Vec<(usize, Vec<PhotoId>, (u64, u64))> = Vec::new();
    let mut added = Vec::new();
    let files = files.get(files.len().saturating_sub(MAX_TRACKED)..).unwrap_or_default().to_vec();
    // the library photos showing each file (one pass over the library)
    let wanted: std::collections::HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let mut by_path: std::collections::HashMap<String, Vec<PhotoId>> = std::collections::HashMap::new();
    for ph in s.catalog.photos().filter(|p| p.copy_of.is_none() && !p.deleted) {
        if let Source::File { path } = &ph.source
            && wanted.contains(path.as_str())
        {
            by_path.entry(path.clone()).or_default().push(ph.id);
        }
    }
    for f in files {
        let touched = f.touched.unwrap_or(now);
        if f.path.is_empty() || now.saturating_sub(touched) > TRACK_FOR_MS || !seen.insert(f.path.clone()) {
            continue;
        }
        let Some(now_stamp) = stamp(&f.path) else { continue };
        let photos = by_path.remove(&f.path).unwrap_or_default();
        let Some(&base) = photos.first() else {
            out.push(TrackedEdit { since: Some(f.since.unwrap_or(now)), touched: Some(touched), ..f });
            continue;
        };
        let (path, since) = (f.path.clone(), f.since.unwrap_or(now));
        if (f.size, f.modified) != (Some(now_stamp.0), Some(now_stamp.1)) {
            changed.push((out.len(), photos, now_stamp));
        }
        out.push(TrackedEdit { since: Some(now), touched: Some(touched), ..f });
        // the editor's Save As: on top of the photo's stack
        for path in saved_as(s, &path, since, &mut listings) {
            let imported = match s.execute("library.import", &json!({"paths": [path]})) {
                Ok(r) => r["imported"].get(0).and_then(Value::as_u64),
                Err(e) => {
                    log::warn!("{path}: {e}");
                    None
                }
            };
            let Some(new) = imported else { continue };
            let _ = s.execute("stack.group", &json!({"ids": [new, base.0], "top": new, "collapsed": false}));
            added.push(json!({"path": path, "id": new, "original": base.0}));
            seen.insert(path.clone());
            out.push(TrackedEdit::new(&path, now));
        }
    }
    let ids: Vec<PhotoId> = changed.iter().flat_map(|(_, photos, _)| photos.iter().copied()).collect();
    let (reloaded, unreadable) = if ids.is_empty() { (Vec::new(), Vec::new()) } else { reload_ids(s, &ids)? };
    for (i, photos, (size, modified)) in changed {
        // a file that couldn't be read (still being written) is read again next time
        let Some(t) = out.get_mut(i).filter(|_| !photos.iter().all(|id| unreadable.contains(id))) else { continue };
        (t.size, t.modified, t.touched) = (Some(size), Some(modified), Some(now));
    }
    s.selection = selection;
    let start = out.len().saturating_sub(MAX_TRACKED);
    Ok(json!({
        "reloaded": reloaded.iter().map(|i| i.0).collect::<Vec<_>>(),
        "added": added,
        "files": out.get(start..).unwrap_or_default(),
    }))
}

/// The op that brings a photo up to date with what its file is now (`info`, a fresh probe), or
/// `None` when nothing changed. Covers a raw that became decodable (or stopped being: a different
/// file relinked), i.e. a change of [`lightcraft_catalog::Photo::preview_only`], and a
/// preview-only photo this version hasn't checked yet: its stamp is refreshed so it isn't probed
/// again (see [`lightcraft_catalog::Photo::preview_checked`]).
pub(crate) fn content_op(id: lightcraft_catalog::PhotoId, ph: &lightcraft_catalog::Photo, info: crate::media::ProbeInfo) -> Option<Op> {
    let preview_checked = lightcraft_catalog::preview_stamp(&info.preview_only);
    let same = info.content_hash == ph.content_hash
        && info.file_size == ph.file_size
        && (info.width, info.height) == (ph.width, ph.height)
        && info.preview_only == ph.preview_only
        && preview_checked == ph.preview_checked;
    (!same).then_some(Op::SetContent {
        id,
        width: info.width,
        height: info.height,
        file_size: info.file_size,
        content_hash: info.content_hash,
        preview_only: info.preview_only,
        preview_checked,
    })
}

/// A raw catalogued as shown from its embedded preview (by a LightCraft that couldn't decode it yet)
/// whose sensor data has just been developed: it and its virtual copies catch up with the file —
/// its size, no more "Preview only" ([`content_op`]); one still as imported gets the raw defaults
/// (as-shot white balance, sharpening, the file's lens correction) — and the thumbnails and views
/// rendered from the embedded JPEG are dropped. Not an undo step: the user did nothing.
pub(crate) fn heal_preview_only(s: &mut Session, id: lightcraft_catalog::PhotoId) {
    let Some(master) = s.catalog.photo(id).map(|p| p.copy_of.unwrap_or(id)) else { return };
    let Some(Source::File { path }) = s.catalog.photo(master).map(|p| p.source.clone()) else { return };
    if s.media.file_probe.is_none() {
        return;
    }
    let Some(Ok(info)) = crate::import::probe_paths(s, std::slice::from_ref(&path)).pop() else { return };
    if info.preview_only.is_some() {
        return;
    }
    let photos: Vec<std::sync::Arc<lightcraft_catalog::Photo>> =
        s.catalog.photos().filter(|p| (p.id == master || p.copy_of == Some(master)) && p.preview_only.is_some()).cloned().collect();
    let mut ops = Vec::new();
    for p in &photos {
        ops.extend(content_op(p.id, p, info.clone()));
        if p.develop_is(p.import_defaults()) {
            let mut fresh = (**p).clone();
            (fresh.preview_only, fresh.as_shot_wb, fresh.embedded_lens) = (None, info.as_shot_wb, info.embedded_lens);
            crate::import::apply_import_defaults(s, &mut fresh);
            crate::import::apply_default_crop(&mut fresh, info.default_crop);
            // ponytail: the catalog keeps no as-shot white balance / lens facts for a healed raw (no op sets them
            // without a catalog format bump), so Reset goes back to the rendered-file defaults; the UI reads the
            // decoder's lens facts instead (`Session::embedded_lens`)
            if fresh.develop != p.develop {
                ops.push(Op::SetDevelop { id: p.id, settings: fresh.develop, label: "Raw Defaults".into(), edited: p.edited.clone() });
            }
        }
    }
    if ops.is_empty() {
        return;
    }
    if let Err(e) = s.apply_quiet(Op::Batch { ops }) {
        log::warn!("can't mark {path} as decodable: {e}");
        return;
    }
    for p in &photos {
        s.forget_renders(p);
    }
}

/// The reverse of [`heal_preview_only`]: a raw catalogued as developed from its sensor data whose file just
/// gave only its embedded JPEG (damaged raw data, such as a copy cut short, or a catalog written by a build or
/// a device that never decoded it). It and its virtual copies get "Preview only" with the decoder's reason and
/// the file's size ([`content_op`]), so every view says the photo is the camera's JPEG, not the raw. Their
/// edits stay. Not an undo step: the user did nothing.
pub(crate) fn flag_preview_only(s: &mut Session, id: lightcraft_catalog::PhotoId) {
    let Some(master) = s.catalog.photo(id).map(|p| p.copy_of.unwrap_or(id)) else { return };
    let Some(Source::File { path }) = s.catalog.photo(master).map(|p| p.source.clone()) else { return };
    if s.media.file_probe.is_none() {
        return;
    }
    let Some(Ok(info)) = crate::import::probe_paths(s, std::slice::from_ref(&path)).pop() else { return };
    if info.preview_only.is_none() {
        return;
    }
    let ops: Vec<Op> = s
        .catalog
        .photos()
        .filter(|p| (p.id == master || p.copy_of == Some(master)) && p.develops_raw())
        .filter_map(|p| content_op(p.id, p, info.clone()))
        .collect();
    if ops.is_empty() {
        return;
    }
    if let Err(e) = s.apply_quiet(Op::Batch { ops }) {
        log::warn!("can't mark {path} as preview only: {e}");
    }
}

/// Ops that fill a photo's empty camera fields (camera, lens, exposure, GPS, capture time) from a fresh probe.
fn fill_missing_meta(id: lightcraft_catalog::PhotoId, ph: &lightcraft_catalog::Photo, info: &crate::media::ProbeInfo) -> Vec<Op> {
    let (mut m, src) = (ph.meta.clone(), &info.meta);
    let before = m.clone();
    if m.camera.is_empty() {
        m.camera = src.camera.clone();
    }
    if m.lens.is_empty() {
        m.lens = src.lens.clone();
    }
    if m.shutter.is_empty() {
        m.shutter = src.shutter.clone();
    }
    m.focal_mm = m.focal_mm.or(src.focal_mm);
    m.aperture = m.aperture.or(src.aperture);
    m.iso = m.iso.or(src.iso);
    m.gps = m.gps.or(src.gps);
    // the camera's focus point: photos imported before it was read get it here
    if m.focus.is_none() {
        m.focus = src.focus.clone();
    }
    let mut ops = Vec::new();
    if m != before {
        ops.push(Op::SetMeta { id, meta: Box::new(m) });
    }
    if ph.captured.is_none() && info.captured.is_some() {
        ops.push(Op::SetCaptured { id, captured: info.captured.clone() });
    }
    ops
}

/// Ops that give a photo the lens data its file carries now (`lens`: a fresh probe's, or what a decode found), or
/// none when it already has them. A catalog from before the lens data was read at import has none, so such a
/// photo's correction stayed off. When the photo gains lens data and its develop settings are still
/// what import gave it, the lens correction is turned on as import would; an edited photo keeps
/// its settings. A vignette alone that a probe doesn't see is kept: it was measured from the pixels when the photo
/// was decoded (a CR3's, `camera_preview::jpeg_vignetting`), and a probe reads headers only. So is a lens database
/// profile a device without the database (or with an older one) doesn't find, or finds an older version of: synced
/// devices would otherwise take it away from each other.
pub(crate) fn lens_ops(id: lightcraft_catalog::PhotoId, ph: &lightcraft_catalog::Photo, lens: Option<lightcraft_develop::EmbeddedLens>) -> Vec<Op> {
    use lightcraft_develop::LensOrigin;
    let kept = |have: &lightcraft_develop::EmbeddedLens| match &lens {
        None => have.measured() || matches!(have.origin, LensOrigin::Database { .. }),
        Some(new) => {
            matches!(new.origin, LensOrigin::Database { .. }) && matches!(have.origin, LensOrigin::Database { .. }) && !have.superseded_by(new)
        }
    };
    if lens == ph.embedded_lens || ph.embedded_lens.as_ref().is_some_and(kept) {
        return Vec::new();
    }
    let mut ops = vec![Op::SetEmbeddedLens { id, lens: lens.map(Box::new) }];
    // (a photo with a default-preset look keeps it: that look is its import state)
    if ph.embedded_lens.is_none() && lens.is_some() && ph.import_look.is_none() && !ph.is_edited() && !ph.develop.optics.lens_profile {
        let mut d = (*ph.develop).clone();
        d.optics.lens_profile = true;
        ops.push(Op::SetDevelop { id, settings: std::sync::Arc::new(d), label: "Lens Corrections".into(), edited: ph.edited.clone() });
    }
    ops
}

/// The op that gives a raw the as-shot white balance its file reads now (`info`, a fresh probe), or none when the
/// catalog already has it or the photo is not a raw edited in Kelvin. A raw imported before its camera's matrices
/// gave it a Kelvin scale keeps the relative scale's 6500 / 0 until a decode or Reload reads the file (issue #730).
pub(crate) fn as_shot_wb_op(id: lightcraft_catalog::PhotoId, ph: &lightcraft_catalog::Photo, info: &crate::media::ProbeInfo) -> Option<Op> {
    let read = info.as_shot_wb.filter(|(t, tint)| t.is_finite() && tint.is_finite())?;
    (ph.develops_raw() && !ph.relative_wb() && ph.as_shot_wb != Some(read)).then_some(Op::SetAsShotWb { id, wb: Some(read) })
}

/// The ops that bring photo `id` and its virtual copies up to date with what their file is now
/// (`info`, a fresh probe), appended to `ops`; the photos they change are appended to `changed`.
/// Reload and the preview-only recheck (`library.recheckPreviewOnly`) share it.
pub(crate) fn reload_ops(
    cat: &lightcraft_catalog::Catalog,
    id: lightcraft_catalog::PhotoId,
    info: &crate::media::ProbeInfo,
    ops: &mut Vec<Op>,
    changed: &mut Vec<lightcraft_catalog::PhotoId>,
) {
    let Some(ph) = cat.photo(id) else { return };
    // camera fields the catalog lacks (e.g. a raw imported before its format was read) are filled in;
    // nothing already set is overwritten
    let meta_ops = fill_missing_meta(id, ph, info);
    let mut mine: Vec<Op> = content_op(id, ph, info.clone())
        .into_iter()
        .chain(meta_ops)
        .chain(lens_ops(id, ph, info.embedded_lens))
        .chain(as_shot_wb_op(id, ph, info))
        .collect();
    if !mine.is_empty() {
        ops.append(&mut mine);
        changed.push(id);
    }
    // virtual copies share the file
    for c in cat.photos().filter(|c| c.copy_of == Some(id)) {
        let theirs: Vec<Op> =
            content_op(c.id, c, info.clone()).into_iter().chain(lens_ops(c.id, c, info.embedded_lens)).chain(as_shot_wb_op(c.id, c, info)).collect();
        if !theirs.is_empty() {
            ops.extend(theirs);
            changed.push(c.id);
        }
    }
}

/// Re-read photos whose files changed on disk (an external editor saved them): new size,
/// dimensions and content hash, cached sources dropped. The app runs the same steps
/// ([`crate::cmd::reload`]) with the files read on a worker thread. → {reloaded: [ids]}
fn reload(s: &mut Session, p: &Value) -> Result<Value> {
    use crate::cmd::reload as r;
    let items = r::plan(s, p)?;
    let probe = r::file_probe(s);
    let results = r::probe(probe.as_ref(), &items, &std::sync::atomic::AtomicBool::new(false), &|_| {});
    let reloaded = r::apply(s, items, results)?;
    Ok(r::to_json(&reloaded))
}

/// [`reload`] for `ids`, through the same steps ([`crate::cmd::reload`], so a newer background
/// Reload is not overwritten): (the photos brought up to date, those whose file couldn't be read).
fn reload_ids(s: &mut Session, ids: &[PhotoId]) -> Result<(Vec<PhotoId>, Vec<PhotoId>)> {
    use crate::cmd::reload as r;
    let items = r::plan(s, &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))?;
    let probe = r::file_probe(s);
    let results = r::probe(probe.as_ref(), &items, &std::sync::atomic::AtomicBool::new(false), &|_| {});
    let unreadable = items.iter().zip(&results).filter(|(_, info)| info.is_err()).map(|(item, _)| item.id).collect();
    let reloaded = r::apply(s, items, results)?;
    Ok((reloaded, unreadable))
}

/// Duplicate: copy the file next to itself (`<name>-copy`, never overwriting) and add it with the
/// same settings, metadata, rating, flag, label and albums. → {ids}
fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.duplicate";
    let mut made = Vec::new();
    for id in s.targets(p)? {
        if s.catalog.photo(id).is_none() {
            continue;
        }
        let (new, _) = copy_file_photo(s, C, id, "-copy", true, "Duplicate")?;
        made.push(new);
    }
    s.merge_undo(made.len(), "Duplicate");
    if let Some(last) = made.last() {
        s.selection = crate::Selection { ids: made.clone(), active: Some(*last) };
    }
    Ok(json!({"ids": made.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

/// Copy photo `id`'s file next to it as `<name><suffix>` (then `<name><suffix>-2`…, never
/// overwriting) and add the copy as a photo of its own, one undo step named `label`, with the same
/// metadata, rating, flag, label and albums, and the photo's develop settings (`keep_edits`) or
/// those import gave it. → (the new photo, its file)
fn copy_file_photo(s: &mut Session, cmd: &str, id: PhotoId, suffix: &str, keep_edits: bool, label: &str) -> Result<(PhotoId, String)> {
    let ph = s.catalog.photo(id).map(|p| (**p).clone()).ok_or_else(|| bad(cmd, format!("no photo {}", id.0)))?;
    let Source::File { path } = &ph.source else { return Err(bad(cmd, "a generated demo photo has no file to copy")) };
    let src = Path::new(path);
    let stem = src.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "photo".into());
    let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = src.parent().unwrap_or(Path::new(""));
    let mut dst = dir.join(format!("{stem}{suffix}{ext}"));
    let mut n = 2;
    while dst.exists() {
        dst = dir.join(format!("{stem}{suffix}-{n}{ext}"));
        n += 1;
    }
    std::fs::copy(src, &dst).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    let new = s.catalog.alloc_photo_id();
    let mut q = ph.clone();
    q.id = new;
    q.source = Source::File { path: dst.to_string_lossy().to_string() };
    q.file_name = dst.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    // the same bytes, but its own photo: no duplicate-content clash, no shared history
    q.content_hash = q.content_hash.map(|h| format!("{h}:dup{}", new.0));
    q.copy_of = None;
    q.copy_name = None;
    q.history.clear();
    if keep_edits {
        q.versions.retain(|v| !v.auto);
    } else {
        q.develop = std::sync::Arc::new(ph.import_defaults());
        q.edited = None;
        q.versions.clear();
    }
    let mut ops = vec![Op::AddPhoto { photo: Box::new(q) }];
    for a in s.catalog.albums().filter(|a| !a.is_smart() && !a.folder && a.photos.contains(&id)) {
        let mut photos = a.photos.clone();
        photos.push(new);
        ops.push(Op::SetAlbumPhotos { id: a.id, photos });
    }
    s.commit(label, Op::Batch { ops })?;
    Ok((new, dst.to_string_lossy().to_string()))
}

/// Render to DNG (Lightroom Classic 15.5): see [`crate::render_dng`]. Here all in one go; more
/// than one photo shows in the activity stack and can be cancelled between photos (the copies made
/// until then stay). The desktop app runs the renders on a worker instead.
fn render_to_dng(s: &mut Session, p: &Value) -> Result<Value> {
    let job = crate::render_dng::prepare(s, p)?;
    let n = job.len();
    let task = (n > 1).then(|| s.activity.start("renderDng", "Rendering to DNG", crate::activity::Cancel::Yes));
    let (done, cancelled) = job.run(&mut |path, bytes| crate::export::write_file_durable(path, bytes), &mut |i| {
        let Some(t) = &task else { return true };
        t.progress(i as u64, n as u64);
        !t.is_cancelled()
    });
    drop(task);
    crate::render_dng::finish(s, done, cancelled)
}

pub fn edit_specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "photo.duplicate",
            "Duplicate",
            ["Photo"],
            None,
            "{ids?} — copy each photo's file next to it (`-copy`) and add it with the same settings, metadata and albums (a real file, unlike a virtual copy) → {ids}",
            has_selection,
            duplicate
        ),
        cmd!(
            "photo.reload",
            "Reload from Disk",
            ["Photo"],
            None,
            "{ids?} — re-read photos whose files changed on disk (e.g. saved by an external editor), or raws shown from their embedded preview that can be decoded now; camera fields the catalog lacks are filled in, and so is the lens data the file carries (an untouched photo then gets its lens correction turned on as import would; an edited one keeps its settings); in the app the files are read in the background (activity row `reload`, cancellable) and the reply is {background: true, count} unless `wait: true` → {reloaded}",
            has_selection,
            reload
        ),
        cmd!(
            "photo.editExternal",
            "Edit Copy for External Editor",
            [],
            None,
            "{ids?, mode?: render (default) | copy | original, colorSpace?: adobeRgb (default) | proPhoto | displayP3 | srgb, format?: tiff (default) | psd, bitDepth?: 16 (default) | 8, dir?} — the files the external editor opens for each selected photo (or `ids`, at most 50). render: the photo with its edits as `<name>-Edit.tif` (or `.psd`, flat, which PhotoCraft saves back to in place) next to it; copy: a plain copy of its file, `<name>-Edit.<ext>`, without the edits; either is added stacked on its original and the copies are selected. original: the photo's own file (its edits stay on top of what the editor saves). copy and original take image files only (TIFF, PSD, JPEG, PNG…), not raws → {path, id, original (the first file), mode, copies: [{path, id, original}], track: [{path, since, touched, size, modified}] (the files to watch with photo.checkExternalEdits)} (the app then opens them in the external editor)",
            has_active,
            edit_external
        ),
        cmd!(
            "photo.checkExternalEdits",
            "Check External Edits",
            [],
            None,
            "{files: [{path, since?, touched?, size?, modified?}]} — bring back what the external editor saved to the files photo.editExternal handed it (`track`; times in ms since the Unix epoch): a file whose size or time changed is read again (as photo.reload), and a file it saved beside one under its name with another extension (Save As: `pier-Edit.psd` beside `pier-Edit.tif`) since `since` is imported and stacked on top of that photo. Files are watched for two weeks after they were handed over or last changed, at most 200; a file that is gone drops off, one that isn't a photo of this library is left as it is → {reloaded: [photoId], added: [{path, id, original}], files: (the list to keep)}",
            always,
            check_external
        ),
        cmd!(
            "photo.renderToDng",
            "Render to DNG",
            ["Photo"],
            None,
            "{ids?, colorSpace?: proPhoto (default) | rec2020 | adobeRgb | displayP3 | srgb, dir?} — render each selected photo (or `ids`, at most 500) with its edits as a linear 16-bit float DNG `<name>-Rendered.dng` next to it (a demo photo into `dir`), add each stacked on its original and select them; a batch shows in the activity stack and can be cancelled between photos → {copies: [{path, id, original}], failed: [{id, error}], cancelled}",
            has_selection,
            render_to_dng
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_catalog::{Photo, PhotoId, Source};

    /// Reload fills camera fields a photo lacks (a CR3 imported before CR3 metadata was read) and keeps
    /// everything already set.
    #[test]
    fn reload_fills_only_missing_camera_fields() {
        let mut ph = Photo::new(PhotoId(1), Source::File { path: "/x.cr3".into() }, "x.cr3", "CR3", 1620, 1080, "2026-10-06T00:00:00");
        ph.meta.title = "Mine".into();
        ph.meta.iso = Some(800);
        let mut info = crate::media::ProbeInfo { captured: Some("2026-10-04T08:16:11".into()), ..Default::default() };
        info.meta.camera = "Canon EOS R6 Mark III".into();
        info.meta.lens = "RF24-105mm".into();
        info.meta.iso = Some(400);
        info.meta.aperture = Some(6.3);
        let ops = fill_missing_meta(PhotoId(1), &ph, &info);
        let Some(Op::SetMeta { meta, .. }) = ops.first() else { panic!("{ops:?}") };
        assert_eq!((meta.camera.as_str(), meta.lens.as_str(), meta.aperture), ("Canon EOS R6 Mark III", "RF24-105mm", Some(6.3)));
        assert_eq!((meta.title.as_str(), meta.iso), ("Mine", Some(800)), "set fields are kept");
        assert!(matches!(ops.get(1), Some(Op::SetCaptured { captured: Some(_), .. })));
        // nothing missing → nothing to do
        ph.meta = (**meta).clone();
        ph.captured = Some("2026-10-04T08:16:11".into());
        assert!(fill_missing_meta(PhotoId(1), &ph, &info).is_empty());
    }

    /// Reload from Disk gives a photo imported before focus points were read its camera focus point,
    /// and never replaces one it has.
    #[test]
    fn reload_fills_a_missing_focus_point() {
        let fp = |x: f64| lightcraft_meta::FocusPoint {
            rect: lightcraft_meta::Rect { x0: x, y0: 0.4, x1: x + 0.02, y1: 0.42 },
            mode: Some("AF-C".into()),
            area: None,
            tracking: None,
        };
        let mut ph = Photo::new(PhotoId(1), Source::File { path: "/x.arw".into() }, "x.arw", "ARW", 8640, 5760, "2026-10-06T00:00:00");
        ph.captured = Some("2026-10-04T08:16:11".into());
        let mut info = crate::media::ProbeInfo { captured: ph.captured.clone(), ..Default::default() };
        info.meta.focus = Some(fp(0.3));
        let ops = fill_missing_meta(PhotoId(1), &ph, &info);
        let Some(Op::SetMeta { meta, .. }) = ops.first() else { panic!("{ops:?}") };
        assert_eq!(meta.focus, Some(fp(0.3)));
        ph.meta.focus = Some(fp(0.1));
        assert!(fill_missing_meta(PhotoId(1), &ph, &info).is_empty(), "a known focus point is kept");
    }

    fn lens() -> lightcraft_develop::EmbeddedLens {
        lightcraft_develop::EmbeddedLens {
            warp: Some(lightcraft_develop::EmbeddedWarp { planes: [[1.0, -0.1, 0.03, 0.0, 0.0, 0.0]; 3], ..Default::default() }),
            vignette: None,
            ..Default::default()
        }
    }
    fn old_rw2() -> lightcraft_catalog::Photo {
        // as an older build catalogued it: no lens data, correction off
        lightcraft_catalog::Photo::new(
            lightcraft_catalog::PhotoId(1),
            Source::File { path: "/x.rw2".into() },
            "x.rw2",
            "RW2",
            4592,
            3448,
            "2026-10-06T00:00:00",
        )
    }

    #[test]
    fn reload_adds_lens_data_and_turns_the_correction_on_for_an_untouched_photo() {
        let ph = old_rw2();
        let info = crate::media::ProbeInfo { embedded_lens: Some(lens()), ..Default::default() };
        let ops = lens_ops(ph.id, &ph, info.embedded_lens);
        assert!(matches!(ops.first(), Some(Op::SetEmbeddedLens { lens: Some(_), .. })), "{ops:?}");
        let Some(Op::SetDevelop { settings, .. }) = ops.get(1) else { panic!("{ops:?}") };
        assert!(settings.optics.lens_profile);
        // once applied, the photo is still "unedited" and has what import would have given it
        let mut c = lightcraft_catalog::Catalog::default();
        c.apply(Op::AddPhoto { photo: Box::new(ph) }).unwrap();
        for op in ops {
            c.apply(op).unwrap();
        }
        let after = c.photo(lightcraft_catalog::PhotoId(1)).unwrap();
        assert_eq!(after.embedded_lens, Some(lens()));
        assert!(!after.is_edited());
        assert_eq!(*after.develop, after.camera_defaults());
        assert!(lens_ops(after.id, after, info.embedded_lens).is_empty(), "nothing more to do the second time");
    }

    #[test]
    fn reload_adds_lens_data_but_keeps_an_edited_photos_settings() {
        let mut ph = old_rw2();
        let mut d = (*ph.develop).clone();
        d.light.exposure = 0.7;
        ph.develop = std::sync::Arc::new(d);
        assert!(ph.is_edited());
        let info = crate::media::ProbeInfo { embedded_lens: Some(lens()), ..Default::default() };
        let ops = lens_ops(ph.id, &ph, info.embedded_lens);
        assert!(matches!(ops.as_slice(), [Op::SetEmbeddedLens { lens: Some(_), .. }]), "{ops:?}");
    }

    /// A vignette measured from the pixels when the photo was decoded (a CR3's camera correction) is not in a probe:
    /// Reload keeps it, while a probe that does read lens data still replaces what the catalog has.
    #[test]
    fn reload_keeps_a_vignette_measured_at_decode() {
        let mut ph = old_rw2();
        let measured = lightcraft_develop::EmbeddedLens { vignette: Some(Default::default()), ..Default::default() };
        ph.embedded_lens = Some(measured);
        assert!(lens_ops(ph.id, &ph, None).is_empty());
        assert!(matches!(lens_ops(ph.id, &ph, Some(lens())).as_slice(), [Op::SetEmbeddedLens { lens: Some(_), .. }]));
        // a file's own warp that a probe no longer finds is dropped as before
        ph.embedded_lens = Some(lens());
        assert!(matches!(lens_ops(ph.id, &ph, None).as_slice(), [Op::SetEmbeddedLens { lens: None, .. }]));
    }

    /// Lens database profiles: one replaces a measured vignette and an older database's profile; a device without the
    /// database, or with an older one, leaves a newer profile alone (synced devices would take it from each other).
    #[test]
    fn reload_takes_newer_lens_database_profiles_only() {
        use lightcraft_develop::{EmbeddedLens, LensOrigin};
        let from_db = |serial| EmbeddedLens { origin: LensOrigin::Database { serial }, ..lens() };
        let set = |ops: Vec<Op>| match ops.as_slice() {
            [Op::SetEmbeddedLens { lens, .. }, ..] => Some(lens.as_deref().copied()),
            _ => None,
        };
        let mut ph = old_rw2();
        ph.embedded_lens = Some(EmbeddedLens { vignette: Some(Default::default()), origin: LensOrigin::Measured, ..Default::default() });
        assert_eq!(set(lens_ops(ph.id, &ph, Some(from_db(3)))), Some(Some(from_db(3))), "a profile replaces a measured vignette");
        ph.embedded_lens = Some(from_db(3));
        assert_eq!(set(lens_ops(ph.id, &ph, None)), None, "a device without the database keeps it");
        assert_eq!(set(lens_ops(ph.id, &ph, Some(from_db(2)))), None, "and one with an older database");
        assert_eq!(set(lens_ops(ph.id, &ph, Some(from_db(3)))), None);
        assert_eq!(set(lens_ops(ph.id, &ph, Some(from_db(4)))), Some(Some(from_db(4))), "a newer database's profile replaces it");
        assert_eq!(set(lens_ops(ph.id, &ph, Some(lens()))), Some(Some(lens())), "the file's own correction always wins");
        // a measured vignette doesn't replace a profile; the file's own correction is never replaced
        let measured = EmbeddedLens { vignette: Some(Default::default()), origin: LensOrigin::Measured, ..Default::default() };
        assert!(!from_db(3).superseded_by(&measured) && !lens().superseded_by(&from_db(9)) && !lens().superseded_by(&measured));
        assert!(measured.superseded_by(&from_db(0)) && from_db(1).superseded_by(&from_db(2)));
        // a vignette recorded before origins were counts as measured
        let legacy = EmbeddedLens { vignette: Some(Default::default()), ..Default::default() };
        assert!(legacy.measured() && legacy.superseded_by(&from_db(1)));
        // and the origin survives the catalog's JSON, left out for the file's own
        let json = serde_json::to_value(from_db(7)).unwrap();
        assert_eq!(json["origin"], json!({"database": {"serial": 7}}));
        assert_eq!(serde_json::from_value::<EmbeddedLens>(json).unwrap(), from_db(7));
        assert!(serde_json::to_value(lens()).unwrap().get("origin").is_none());
    }

    #[test]
    fn reload_leaves_a_photo_whose_user_switched_the_correction_off_alone() {
        let mut ph = old_rw2();
        ph.embedded_lens = Some(lens());
        let mut d = ph.camera_defaults();
        d.optics.lens_profile = false;
        ph.develop = std::sync::Arc::new(d);
        let info = crate::media::ProbeInfo { embedded_lens: Some(lens()), ..Default::default() };
        assert!(lens_ops(ph.id, &ph, info.embedded_lens).is_empty());
    }

    /// End to end on a public Panasonic raw (skipped without the corpus): a catalog from before the lens data was read
    /// gets it on Reload, the render changes to the corrected one, and Undo takes it all back.
    #[test]
    fn corpus_reload_corrects_an_rw2_catalogued_without_lens_data() {
        let root = std::env::var_os("LIGHTCRAFT_CORPUS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
        let f = root.join("raw/rw2-panasonic-gx80.rw2");
        if !f.exists() {
            lightcraft_testkit::skip!(Corpus, "{} absent", f.display());
            return;
        }
        let mut s = Session::new().with_fs();
        s.execute("library.import", &json!({"paths": [f.to_string_lossy()]})).unwrap();
        let id = s.active().unwrap();
        let ph = s.catalog.photo(id).unwrap().clone();
        assert!(ph.embedded_lens.is_some() && ph.develop.optics.lens_profile, "import reads it");
        // back to how 0.4.0 catalogued it
        let mut old = (*ph.develop).clone();
        old.optics.lens_profile = false;
        s.catalog.apply(Op::SetEmbeddedLens { id, lens: None }).unwrap();
        s.catalog.apply(Op::SetDevelop { id, settings: std::sync::Arc::new(old), label: "x".into(), edited: None }).unwrap();
        s.media.forget(id);
        let before = s.render_now(id, 160, 120).unwrap().image.data;
        let r = s.execute("photo.reload", &json!({"ids": [id.0]})).unwrap();
        assert_eq!(r["reloaded"], json!([id.0]));
        let now = s.catalog.photo(id).unwrap().clone();
        assert_eq!(now.embedded_lens, ph.embedded_lens);
        assert!(now.develop.optics.lens_profile && !now.is_edited());
        assert_ne!(s.render_now(id, 160, 120).unwrap().image.data, before, "the corrected render differs");
        s.execute("edit.undo", &json!({})).unwrap();
        let undone = s.catalog.photo(id).unwrap();
        assert!(undone.embedded_lens.is_none() && !undone.develop.optics.lens_profile, "one undo step");
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-render-dng-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn render_to_dng_adds_stacked_linear_dngs() {
        let dir = scratch("stack");
        let mut s = Session::with_demo();
        let ids: Vec<_> = s.visible().iter().copied().take(2).collect();
        let r = s.execute("photo.renderToDng", &json!({"ids": [ids[0].0, ids[1].0], "dir": dir.to_string_lossy()})).unwrap();
        assert_eq!(r["cancelled"], false);
        let copies = r["copies"].as_array().unwrap();
        assert_eq!(copies.len(), 2, "{r}");
        for (c, orig) in copies.iter().zip(&ids) {
            let path = c["path"].as_str().unwrap();
            assert!(path.ends_with("-Rendered.dng"), "{path}");
            let raw = lightcraft_raw::decode(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!((raw.cpp, raw.cfa.is_none()), (3, true));
            let new = PhotoId(c["id"].as_u64().unwrap());
            assert_eq!(c["original"].as_u64(), Some(orig.0));
            let st = s.catalog.stack_of(new).unwrap();
            assert_eq!((st.photos.first(), st.photos.contains(orig)), (Some(&new), true), "stacked on top of its original");
        }
        assert_eq!(s.selection.ids.len(), 2, "the new DNGs are selected");
        assert!(s.activity.list().is_empty(), "the activity row ends with the command");
        // a second run never overwrites
        let again = s.execute("photo.renderToDng", &json!({"ids": [ids[0].0], "dir": dir.to_string_lossy()})).unwrap();
        assert_ne!(again["copies"][0]["path"], copies[0]["path"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_to_dng_rejects_bad_params_and_cancels_between_photos() {
        let mut s = Session::with_demo();
        let id = s.visible()[0];
        assert!(s.execute("photo.renderToDng", &json!({"ids": [id.0]})).is_err(), "a demo photo needs `dir`");
        assert!(s.execute("photo.renderToDng", &json!({"ids": [id.0], "dir": "/x", "colorSpace": "cmyk"})).is_err());
        let dir = scratch("cancel");
        let ids: Vec<u64> = s.visible().iter().take(6).map(|i| i.0).collect();
        let activity = s.activity.clone();
        let stop = std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            while t0.elapsed().as_secs() < 30 {
                if let Some(t) = activity.list().into_iter().find(|t| t.kind == "renderDng" && t.cancellable) {
                    activity.cancel(t.id).unwrap();
                    return true;
                }
                std::thread::yield_now();
            }
            false
        });
        let r = s.execute("photo.renderToDng", &json!({"ids": ids, "dir": dir.to_string_lossy()})).unwrap();
        assert!(stop.join().unwrap(), "the batch showed a cancellable activity row");
        assert_eq!(r["cancelled"], true, "{r}");
        assert!(r["copies"].as_array().unwrap().len() < 6);
        assert!(s.activity.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End to end on a public CR3 (skipped without the corpus; EOS M50, EF-M 15-45 mm at 15 mm f/4, its camera JPEG
    /// with Peripheral Illumination Correction): it opens with lens corrections on and no lens data; its first decode
    /// measures the vignetting the camera corrected and records it on the photo, which Reload keeps; the corners
    /// render brighter than with the correction off while the centre stays; and a DNG of it carries the vignette.
    #[test]
    fn corpus_cr3_records_the_vignetting_its_camera_corrected() {
        let root = std::env::var_os("LIGHTCRAFT_CORPUS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
        let f = root.join("raw/cr3-canon-m50-craw.cr3");
        if !f.exists() {
            lightcraft_testkit::skip!(Corpus, "{} absent", f.display());
            return;
        }
        let mut s = Session::new().with_fs();
        s.execute("library.import", &json!({"paths": [f.to_string_lossy()]})).unwrap();
        let id = s.active().unwrap();
        let ph = s.catalog.photo(id).unwrap().clone();
        assert!(ph.embedded_lens.is_none() && ph.develop.optics.lens_profile && !ph.is_edited(), "import: on, nothing measured yet");
        let (w, h) = (192, 128);
        let on = s.render_now(id, w, h).unwrap().image;
        let learnt = s.catalog.photo(id).unwrap().embedded_lens;
        let v = learnt.and_then(|l| l.vignette).expect("measured when decoded");
        assert!(learnt.is_some_and(|l| l.warp.is_none() && !l.has_stages()));
        s.execute("photo.reload", &json!({"ids": [id.0]})).unwrap();
        assert_eq!(s.catalog.photo(id).unwrap().embedded_lens, learnt, "Reload keeps it");
        let mut off = (*ph.develop).clone();
        off.optics.lens_profile = false;
        s.catalog.apply(Op::SetDevelop { id, settings: std::sync::Arc::new(off), label: "x".into(), edited: None }).unwrap();
        let off = s.render_now(id, w, h).unwrap().image;
        let mean = |img: &lightcraft_raster::Rgba8, x0: usize, y0: usize| {
            let mut sum = 0.0;
            for y in y0..y0 + 8 {
                for x in x0..x0 + 8 {
                    sum += f64::from(img.get(x, y)[1]);
                }
            }
            sum / 64.0
        };
        let (iw, ih) = (on.width, on.height);
        let centre = mean(&on, iw / 2 - 4, ih / 2 - 4) / mean(&off, iw / 2 - 4, ih / 2 - 4);
        assert!((centre - 1.0).abs() < 0.03, "centre {centre:.3}");
        for (x, y) in [(0, 0), (iw - 8, 0), (0, ih - 8), (iw - 8, ih - 8)] {
            let corner = mean(&on, x, y) / mean(&off, x, y);
            assert!(corner > 1.08, "corner ({x}, {y}) {corner:.3}");
        }
        // the DNG opens with the same vignette (its own `FixVignetteRadial`)
        let bytes = std::fs::read(&f).unwrap();
        let mut raw = lightcraft_raw::decode(&bytes).unwrap();
        crate::camera_preview::with_starting_colour(&mut raw, &bytes);
        let dng = lightcraft_raw::write_dng(&raw, &Default::default()).unwrap();
        let in_dng = crate::files::probe_bytes("x.dng", &dng).unwrap().embedded_lens.and_then(|l| l.vignette).expect("in the DNG");
        let (fw, fh) = (f64::from(ph.width), f64::from(ph.height));
        for p in [lightcraft_geom::Point::new(0.0, 0.0), lightcraft_geom::Point::new(fw * 0.7, fh * 0.3)] {
            let gain = |v: &lightcraft_develop::EmbeddedVignette| lightcraft_pipeline::optics::embedded_vignette_gain(v, p, fw, fh);
            let (a, b) = (gain(&v), gain(&in_dng));
            assert!((a / b - 1.0).abs() < 0.02, "at {p:?}: photo {a:.3}, DNG {b:.3}");
        }
    }
}
