//! A raw this build can't decode, developed by another decoder: a native frontend (on Apple platforms, the system's Core Image
//! RAW decoder) hands over the scene-referred linear picture, and the photo becomes a LinearRaw DNG in the
//! library's `Derived` folder, linked like Convert to DNG does (the original stays where it is). From then on
//! it edits, exports and syncs as a raw: white balance, exposure, highlight and shadow latitude that the camera's
//! embedded JPEG (what such a photo shows until then, `Photo::preview_only`) no longer has. See docs/apple-raw9.md.

use std::path::Path;

use lightcraft_catalog::{MediaKind, Op, Source};
use lightcraft_codecs::DecodeOptions;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_selection};
use crate::{Result, Session};

/// Most pixels taken from a host's development. A photo of this size is held three times while it is converted
/// (the decoded picture, its working-space copy, the DNG's samples).
const MAX_PIXELS: u64 = 150_000_000;

/// Long edges the look is fitted at, tried in turn (the fit's verdict depends a little on the picture's size; a
/// DNG that carries the curve it fitted here gives the same look at every render size).
const FIT_EDGES: [usize; 3] = [1600, 1000, 600];

/// The tone curve that takes the host's linear picture `rgb` to its own rendering `preview` (a JPEG), fitted the way a
/// DNG with a preview is fitted when it opens, but once, on small copies, and written into the DNG. `None` when no copy
/// fits (then the DNG keeps the preview and the reader fits it when it opens).
fn fitted_tone(
    rgb: &lightcraft_raster::Rgb32f,
    meta: &lightcraft_meta::Metadata,
    white: Option<lightcraft_color::Xy>,
    baseline: f64,
    preview: &[u8],
) -> Option<lightcraft_raw::profile::ToneCurve> {
    for edge in FIT_EDGES {
        let small = if rgb.width.max(rgb.height) > edge {
            lightcraft_raster::resample::fit(rgb, edge, edge, lightcraft_raster::resample::Filter::Box)
        } else {
            rgb.clone()
        };
        let look = lightcraft_raw::LinearDngLook {
            baseline_exposure: baseline,
            identity_tone: false,
            headroom: true,
            preview: Some(preview),
            compact: false,
            as_shot_white: white,
            tone_curve: None,
        };
        let Ok(bytes) =
            lightcraft_raw::write_linear_dng(small.width, small.height, small.data.as_flattened(), &lightcraft_color::REC2020, meta, None, look)
        else {
            continue;
        };
        let Ok(mut raw) = lightcraft_raw::decode(&bytes) else { continue };
        let (_, _, look, _) = crate::camera_preview::starting_colour(&mut raw, &bytes, None, None);
        if let Some(curve) = look.and_then(|l| crate::camera_preview::dng_tone_curve(&l.tone)) {
            return Some(curve);
        }
    }
    None
}

/// The folder of a library's stand-in DNGs.
fn derived_dir(library: &Path, id_part: &str) -> std::path::PathBuf {
    library.join("Derived").join(id_part)
}

/// `photo.attachDecoded {id, path, previewPath?, whiteXY?, baselineExposure?, force?}`: `path` is the host's linear development of
/// the photo's original (a float TIFF in linear light of any primaries, white = 1, values above it are highlights);
/// `previewPath` the same picture rendered the host's default way (a JPEG, any size, sRGB), which gives the raw its
/// starting look: the engine fits its default tone to it, as it does for a DNG with a preview. The host deletes
/// both afterwards. `whiteXY` is the scene white `[x, y]` the host balanced the picture for (its as-shot white): the
/// DNG then holds the picture as the raw it came from, so white balance edits work as on any raw (true Kelvin and tint,
/// As Shot, Auto). Photos with edits are refused unless `force` (their settings were made on the camera's JPEG,
/// a display-referred picture, and mean something else on a linear one).
fn attach(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.attachDecoded";
    let src = super::str_param(p, "path").ok_or_else(|| bad(C, "`path`: the linear picture the host developed (a float TIFF)"))?;
    let ids = s.targets(p)?;
    let [id] = ids.as_slice() else { return Err(bad(C, "name one photo (`id`)")) };
    let photo = s.catalog.photo(*id).cloned().ok_or_else(|| bad(C, format!("no photo {}", id.0)))?;
    let master_id = photo.copy_of.unwrap_or(photo.id);
    let master = s.catalog.photo(master_id).cloned().ok_or_else(|| bad(C, format!("no photo {}", master_id.0)))?;
    let Source::File { path: original } = &master.source else { return Err(bad(C, "a generated demo photo has no file")) };
    if master.kind != MediaKind::Raw || master.preview_only.is_none() {
        return Err(bad(C, format!("{}: only a raw shown from its embedded preview takes a stand-in development", master.file_name)));
    }
    let group: Vec<_> = s.catalog.photos().filter(|g| g.id == master_id || g.copy_of == Some(master_id)).cloned().collect();
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    if !force && group.iter().any(|g| g.is_edited()) {
        return Err(bad(C, format!("{}: it has edits made on the camera's JPEG; `force` keeps them on the raw development", master.file_name)));
    }
    let library = s
        .library
        .as_ref()
        .filter(|l| l.on_disk)
        .map(|l| l.dir.clone())
        .ok_or_else(|| bad(C, "the stand-in DNG is kept in the library's folder: this library isn't on disk"))?;
    let preview = match super::str_param(p, "previewPath") {
        Some(path) => Some(std::fs::read(path).map_err(|e| bad(C, format!("{path}: {e}")))?),
        None => None,
    };
    let white = match p.get("whiteXY") {
        None | Some(Value::Null) => None,
        Some(Value::Array(xy)) => match xy.as_slice() {
            [x, y] => match (x.as_f64(), y.as_f64()) {
                (Some(x), Some(y)) if (0.05..0.8).contains(&x) && (0.05..0.8).contains(&y) => Some(lightcraft_color::Xy::new(x, y)),
                _ => return Err(bad(C, "`whiteXY`: two chromaticity coordinates between 0.05 and 0.8")),
            },
            _ => return Err(bad(C, "`whiteXY`: [x, y]")),
        },
        Some(_) => return Err(bad(C, "`whiteXY`: [x, y]")),
    };
    let baseline = p.get("baselineExposure").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0).clamp(-8.0, 8.0);

    let _task = s.activity.start("develop", "Developing a raw", crate::activity::Cancel::No);
    // the camera facts the original carries (camera, lens, exposure, time) travel into the DNG; its orientation does
    // not: the host's picture is upright already
    let original_bytes = match s.media.file_bytes.as_ref() {
        Some(read) => read(original),
        None => std::fs::read(original).map_err(|e| format!("{original}: {e}")),
    }
    .map_err(|e| bad(C, e))?;
    let mut meta = lightcraft_meta::extract(&original_bytes);
    drop(original_bytes);
    meta.orientation = None;

    let tiff = std::fs::read(src).map_err(|e| bad(C, format!("{src}: {e}")))?;
    let decoded =
        lightcraft_codecs::decode(&tiff, DecodeOptions { max_size: None, max_pixels: MAX_PIXELS }).map_err(|e| bad(C, format!("{src}: {e}")))?;
    drop(tiff);
    let rgb = decoded.to_working();
    drop(decoded);
    if rgb.width == 0 || rgb.height == 0 {
        return Err(bad(C, format!("{src}: an empty picture")));
    }
    let curve = preview.as_deref().and_then(|jpeg| fitted_tone(&rgb, &meta, white, baseline, jpeg));
    // (16-bit integers under lossless JPEG, or half floats under deflate: which is smaller depends on the picture)
    let compact = p.get("compact").and_then(Value::as_bool).unwrap_or(true);
    let look = lightcraft_raw::LinearDngLook {
        baseline_exposure: baseline,
        identity_tone: false,
        headroom: true,
        preview: preview.as_deref(),
        compact,
        as_shot_white: white,
        tone_curve: curve.as_ref(),
    };
    let dng = lightcraft_raw::write_linear_dng(rgb.width, rgb.height, rgb.data.as_flattened(), &lightcraft_color::REC2020, &meta, None, look)
        .map_err(|e| bad(C, e.to_string()))?;
    let (width, height) = (rgb.width, rgb.height);
    drop(rgb);
    // checked before anyone relies on it, like Convert to DNG: the file must read back as the picture that went in
    let back = lightcraft_raw::decode(&dng)
        .map_err(|e| bad(C, format!("the DNG written for {} doesn't decode ({e}); the photo is unchanged", master.file_name)))?;
    if (back.width, back.height, back.cpp) != (width, height, 3) {
        return Err(bad(C, format!("the DNG written for {} has another size than the picture; the photo is unchanged", master.file_name)));
    }
    drop(back);

    let hash = master.content_hash.clone().unwrap_or_else(|| format!("photo-{}", master.id.0));
    let folder = derived_dir(&library, hash.get(..16).unwrap_or(&hash));
    std::fs::create_dir_all(&folder).map_err(|e| bad(C, format!("{}: {e}", folder.display())))?;
    let stem =
        Path::new(&master.file_name).file_stem().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "photo".into());
    let mut names = (1u64..).map(|n| folder.join(if n == 1 { format!("{stem}.dng") } else { format!("{stem}-{n}.dng") }));
    let out = lightcraft_catalog::safe_file::write_new_unique(&mut names, &dng)
        .map_err(|e| bad(C, format!("could not write the DNG ({e}); the photo is unchanged")))?;
    drop(dng);
    let out_path = out.to_string_lossy().to_string();
    let file_name = out.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

    // like Convert to DNG (the photo and its virtual copies follow the file), but not an undo step: the user did nothing
    let ops: Vec<Op> = group
        .iter()
        .map(|g| Op::Relink { id: g.id, file_name: file_name.clone(), source: Source::File { path: out_path.clone() }, format: Some("DNG".into()) })
        .collect();
    s.apply_quiet(Op::Batch { ops }).map_err(|e| bad(C, e.to_string()))?;
    // size, hash and "Preview only" now come from the DNG; an unedited photo gets the raw defaults; renders from the JPEG go
    super::convert::heal_preview_only(s, master_id);
    for g in &group {
        if let Some(fresh) = s.catalog.photo(g.id) {
            s.forget_renders(fresh);
        }
    }
    Ok(json!({"id": id.0, "path": out_path, "original": original, "width": width, "height": height}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "photo.attachDecoded",
        "Develop with System Decoder",
        [],
        None,
        "{id, path, previewPath?, whiteXY?, baselineExposure?, compact?, force?} — a raw shown from its embedded preview (no decoder here) becomes a raw again: `path` is a float TIFF of its linear development by the host's own decoder (on Apple platforms: Core Image's), `previewPath` a JPEG of the same picture as the host renders it (the starting look), `whiteXY` the scene white it balanced for (the as-shot white), written as a LinearRaw DNG in the library's Derived folder and linked like Convert to DNG (the original stays). Refused for a photo with edits unless `force` → {id, path, original, width, height}",
        has_selection,
        attach
    )]
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::PhotoId;
    use lightcraft_codecs::{EncodeImage, EncodeMeta, Samples, TiffCompression, encode_tiff};
    use serde_json::json;

    use crate::files::tests::cr3_with_preview;

    /// A linear float TIFF (what a host's decoder hands over): a left-to-right ramp that runs to `peak`.
    fn linear_tiff(w: u32, h: u32, peak: f32) -> Vec<u8> {
        let px: Vec<f32> = (0..w * h).flat_map(|i| [(i % w) as f32 / w as f32 * peak; 3]).collect();
        encode_tiff(&EncodeImage::new(w, h, 3, Samples::F32(&px)), TiffCompression::None, &EncodeMeta::default()).unwrap()
    }

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 8) as u8, 120, 200]).collect();
        lightcraft_codecs::encode_jpeg(
            &EncodeImage::new(w, h, 3, Samples::U8(&px)),
            90,
            lightcraft_codecs::ChromaSubsampling::S444,
            &EncodeMeta::default(),
        )
        .unwrap()
    }

    fn library(name: &str) -> (crate::Session, std::path::PathBuf, std::path::PathBuf) {
        let dir = crate::tests_xmp::temp_dir(name);
        let files = dir.join("files");
        std::fs::create_dir_all(&files).unwrap();
        let mut s = crate::Session::new().with_fs();
        s.open_library(dir.join("lib"), false).unwrap();
        (s, dir, files)
    }

    fn import_preview_only(s: &mut crate::Session, files: &std::path::Path, name: &str) -> PhotoId {
        let path = files.join(name);
        std::fs::write(&path, cr3_with_preview(96, 64)).unwrap();
        let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
        PhotoId(r["imported"][0].as_u64().unwrap())
    }

    /// A raw shown from its JPEG becomes a raw again from the host's linear development: a DNG in the library's Derived
    /// folder, the photo and its virtual copies linked to it, "Preview only" gone, highlights above white kept, the
    /// original untouched, and no undo step.
    #[test]
    fn a_host_development_turns_a_preview_only_raw_into_a_raw() {
        let (mut s, dir, files) = library("stand-in");
        let id = import_preview_only(&mut s, &files, "DSC_0001.cr3");
        let copy = PhotoId(s.execute("photo.virtualCopy", &json!({"ids": [id.0]})).unwrap()["ids"][0].as_u64().unwrap());
        assert!(s.catalog.photo(id).unwrap().preview_only.is_some() && !s.catalog.photo(id).unwrap().develops_raw());
        let original = std::fs::read(files.join("DSC_0001.cr3")).unwrap();
        let tiff = dir.join("linear.tif");
        std::fs::write(&tiff, linear_tiff(64, 48, 3.0)).unwrap();
        let preview = dir.join("preview.jpg");
        std::fs::write(&preview, jpeg(32, 24)).unwrap();
        let undo = s.undo.len();

        let r =
            s.execute("photo.attachDecoded", &json!({"id": id.0, "path": tiff.to_string_lossy(), "previewPath": preview.to_string_lossy()})).unwrap();
        assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(64), Some(48)), "{r}");
        let out = r["path"].as_str().unwrap().to_string();
        assert!(out.contains("Derived") && std::path::Path::new(&out).is_file(), "{out}");
        for p in [id, copy] {
            let p = s.catalog.photo(p).unwrap();
            assert_eq!((p.preview_only.as_deref(), p.format.as_str(), p.file_name.as_str()), (None, "DNG", "DSC_0001.dng"), "{:?}", p.source);
            assert!(p.develops_raw());
            assert_eq!((p.width, p.height), (64, 48));
            assert_eq!(p.develop.detail.sharpen_amount, 40.0, "an untouched photo gets the raw defaults");
        }
        assert_eq!(s.undo.len(), undo, "not an undo step");
        assert_eq!(std::fs::read(files.join("DSC_0001.cr3")).unwrap(), original, "the original is left alone");
        // the DNG keeps what was above white and says how to read it: peak 3 is stored under 4 with two stops added
        let raw = lightcraft_raw::decode(&std::fs::read(&out).unwrap()).unwrap();
        assert!((raw.color.baseline_exposure - 2.0).abs() < 1e-9, "{}", raw.color.baseline_exposure);
        // the preview travels in it, for the engine's tone fit
        let embedded = lightcraft_raw::embedded_preview(&std::fs::read(&out).unwrap()).expect("the DNG carries the host's preview");
        assert_eq!(lightcraft_codecs::read_header(&embedded).map(|h| (h.width, h.height)).unwrap(), (32, 24));
        assert!(crate::media::source_info(s.catalog.photo(id).unwrap()).raw);
        assert!(s.render_job(id, 48, 48, false, true).unwrap().run().rendered.is_ok());
        // it survives a reopen
        s.persist().unwrap();
        drop(s);
        let mut s = crate::Session::new().with_fs();
        s.open_library(dir.join("lib"), false).unwrap();
        assert!(s.catalog.photo(id).unwrap().develops_raw());
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What can't take a stand-in is refused with a reason and left as it was: an edited photo (its settings mean
    /// something else on a linear picture) unless `force`, a photo that decodes, a missing or hostile file.
    #[test]
    fn a_stand_in_is_refused_where_it_does_not_fit() {
        let (mut s, dir, files) = library("stand-in-refused");
        let id = import_preview_only(&mut s, &files, "DSC_0002.cr3");
        let tiff = dir.join("linear.tif");
        std::fs::write(&tiff, linear_tiff(32, 32, 1.0)).unwrap();
        let p = json!({"id": id.0, "path": tiff.to_string_lossy()});

        s.execute("develop.set", &json!({"ids": [id.0], "control": "light.exposure", "value": 0.8})).unwrap();
        let e = s.execute("photo.attachDecoded", &p).unwrap_err().to_string();
        assert!(e.contains("edits") && e.contains("force"), "{e}");
        assert!(s.catalog.photo(id).unwrap().preview_only.is_some());
        let forced = json!({"id": id.0, "path": tiff.to_string_lossy(), "force": true});
        assert!(s.execute("photo.attachDecoded", &forced).is_ok());
        assert_eq!(s.catalog.photo(id).unwrap().develop.light.exposure, 0.8, "forced: the edits stay");
        // now a raw: nothing more to stand in for
        let e = s.execute("photo.attachDecoded", &forced).unwrap_err().to_string();
        assert!(e.contains("embedded preview"), "{e}");

        let other = import_preview_only(&mut s, &files, "DSC_0003.cr3");
        let missing = json!({"id": other.0, "path": dir.join("nothing.tif").to_string_lossy()});
        assert!(s.execute("photo.attachDecoded", &missing).is_err());
        let junk = dir.join("junk.tif");
        std::fs::write(&junk, b"II*\0not a tiff at all").unwrap();
        assert!(s.execute("photo.attachDecoded", &json!({"id": other.0, "path": junk.to_string_lossy()})).is_err());
        assert!(s.execute("photo.attachDecoded", &json!({"id": other.0})).is_err(), "no path");
        let not_jpeg = dir.join("preview.jpg");
        std::fs::write(&not_jpeg, b"definitely not a jpeg").unwrap();
        let bad_preview = json!({"id": other.0, "path": tiff.to_string_lossy(), "previewPath": not_jpeg.to_string_lossy()});
        assert!(s.execute("photo.attachDecoded", &bad_preview).is_err(), "a preview that isn't a JPEG");
        assert!(s.execute("photo.attachDecoded", &json!({"id": 99999, "path": tiff.to_string_lossy()})).is_err(), "no such photo");
        assert!(s.catalog.photo(other).unwrap().preview_only.is_some(), "every refusal left the photo as it was");
        assert!(!dir.join("lib").join("Derived").join("junk").exists());
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
