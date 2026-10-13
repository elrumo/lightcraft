//! `app.export` without a window: Export Now with the desktop app's parameters, for frontends that
//! have no export dialog of their own yet (the MCP server's headless backend, the SwiftUI iOS app).

use std::path::Path;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::Session;
use lightcraft_engine::export::{Destination, ExportFormat, ExportOptions, Resize, prepare_batch, run_batch};
use serde_json::{Value, json};

/// The parameters `app.export` takes (as `engine.commands` lists them).
pub const PARAMS: &str = "{path?: output file (.jpg/.png/.tif/.webp/.avif/.jxl/.psd/.dng) | dir?, ids?, preset?, format?: jpeg|png|tiff|webp|avif|jxl|psd|dng|original, longEdge?|shortEdge?|width?|height?|megapixels?|percent? (default longEdge 3000; longEdge 0 = full size), dontEnlarge?, ppi?, quality?: 1..100, jxlLossless?, jxlEffort?: fast|normal, jxlProgressive?, limitKb?, colorSpace?: srgb|displayP3|adobeRgb|proPhoto|rec2020, bitDepth?: 8|10|16|32, hdr? (HDR output for HDR edits: gain map JPEG, PQ AVIF / PNG / JPEG XL, float TIFF), sharpen?: none|screen|matte|glossy, sharpenAmount?: low|standard|high, metadata?: all|allExceptCamera|copyright|none, removeLocation?, naming?, startNumber?, subfolder?, conflict?: unique|overwrite|skip, tiffCompression?: none|lzw|zip, dngCompression?: lossless|deflate|uncompressed, addToPhotos?: bool (macOS: then add the files written to Apple Photos; outcome in `applePhotos`), photosAlbum?: top-level album name (made when missing), onServer?: render on the sync server, watermark?: text | {text?, vertical? (upright columns, right to left), size? (text height, 0.005..0.5 of the short edge; default 0.035), opacity? (0..1; 0.7), anchor?: topLeft|top|topRight|left|center|right|bottomLeft|bottom|bottomRight, insetX? (margin from the left/right edge, 0..0.4 of the short edge; 0.025), insetY? (from the top/bottom edge; 0.025), inset? (sets both), rotation?: 0|90|180|270 (degrees clockwise; 0), color? [r,g,b] sRGB, shadow?, image? (graphic drawn instead of the text), imageWidth? (0.01..1 of the photo's width; 0.2)}} — an unknown parameter, an out-of-range watermark size or a value of the wrong kind is an error, not a default";

/// `app.export` as an entry of `engine.commands`.
pub fn command_info(session: &Session) -> Value {
    json!({"id": "app.export", "label": "Export Now", "menu": [], "shortcut": null, "params": PARAMS, "enabled": session.active().is_some()})
}

/// Export the photos `p` names (`ids`; else the selection, in grid order, when more than one photo
/// is selected; else `id` or the active photo) with the options in `p`, as the desktop app's
/// Export Now does. `progress(done, total, next name)` returning false stops before the next photo
/// (the answer is then the error `cancelled`). Without a size, the long edge is 3000 px.
pub fn export_now(session: &mut Session, p: &Value, progress: &mut dyn FnMut(usize, usize, &str) -> bool) -> Result<Value, String> {
    let p = &session.export_params(p)?;
    let mut opts = ExportOptions::from_params(p).map_err(|e| e.to_string())?;
    if !ExportOptions::has_size_param(p) {
        opts.resize = Some(Resize::long_edge(3000));
    }
    let exact = p.get("path").and_then(Value::as_str);
    if let (Some(path), None) = (exact, p.get("format")) {
        let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        opts.format = ExportFormat::parse(&ext).unwrap_or(ExportFormat::Png);
    }
    let ids: Vec<PhotoId> = match p.get("ids").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect(),
        // like the desktop app: the selection (in grid order), else the given / active photo
        None if p.get("id").is_none() && session.selection.ids.len() > 1 => {
            let sel: std::collections::HashSet<_> = session.selection.ids.iter().copied().collect();
            session.visible_cloned().into_iter().filter(|id| sel.contains(id)).collect()
        }
        None => {
            let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(session.active());
            vec![id.ok_or("no photo (import one or pass `id`)")?]
        }
    };
    let dir = p.get("dir").and_then(Value::as_str).unwrap_or("");
    let write = &mut lightcraft_engine::export::write_file;
    // Add to Apple Photos: refused before anything is written when it can't be done
    let after = session.after_export(&opts)?;
    let items = prepare_batch(session, &ids, &opts)?;
    let total = items.len();
    let mut cancelled = false;
    let files = run_batch(
        items,
        &opts,
        &Destination { dir: dir.to_string(), exact: exact.map(str::to_string) },
        write,
        &|path| Path::new(path).exists(),
        true,
        &mut |done, name| {
            cancelled = !progress(done, total, name);
            !cancelled
        },
    )?;
    if !cancelled {
        cancelled = !progress(total, total, "");
    }
    if cancelled {
        return Err("cancelled".into());
    }
    // Single-photo exports also report path/width/height at the top level (back-compat).
    let mut out = files.first().cloned().unwrap_or_else(|| json!({}));
    if let Some(photos) = after.run(&files, false) {
        out["applePhotos"] = photos;
    }
    out["files"] = json!(files);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Export Now writes the active photo at the default size, reports it, and can be stopped
    #[test]
    fn export_now_writes_reports_and_stops_when_asked() {
        let dir = std::env::temp_dir().join(format!("lc-apphost-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::with_demo();
        let first = s.visible_cloned()[0];
        s.execute("library.select", &json!({"ids": [first.0]})).unwrap();
        let p = json!({"dir": dir.to_string_lossy(), "longEdge": 64, "format": "png"});
        let mut calls = Vec::new();
        let out = export_now(&mut s, &p, &mut |done, total, _| {
            calls.push((done, total));
            true
        })
        .unwrap();
        assert_eq!(out["files"].as_array().map(Vec::len), Some(1), "{out}");
        assert!(Path::new(out["path"].as_str().unwrap()).is_file());
        assert_eq!(calls.last(), Some(&(1, 1)));
        assert!(command_info(&s)["enabled"].as_bool().unwrap());
        let e =
            export_now(&mut s, &json!({"dir": dir.to_string_lossy(), "longEdge": 64, "format": "png", "conflict": "unique"}), &mut |_, _, _| false);
        assert_eq!(e.unwrap_err(), "cancelled");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
