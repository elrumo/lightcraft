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
    xmp: Option<&str>,
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
        let Ok(bytes) = lightcraft_raw::write_linear_dng(
            small.width,
            small.height,
            small.data.as_flattened(),
            &lightcraft_color::REC2020,
            meta,
            xmp.map(str::to_string),
            look,
        ) else {
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

/// Where each stand-in's original is, kept in the library's folder (a stand-in's photo is linked to its DNG, so the file
/// the person keeps, which decides the photo's place in its folder, is written down here): the DNG's path below `Derived`
/// (`<hash>/<name>.dng`) → the original's path. The catalog has no field for it ([`lightcraft_catalog::Photo::stand_in_for`]).
const HOMES_FILE: &str = "stand-ins.json";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Homes {
    #[serde(default)]
    homes: std::collections::BTreeMap<String, String>,
}

/// The key of a stand-in DNG in [`Homes`]: its path below the library's `Derived` folder, with `/` for every separator.
fn home_key(derived: &Path, dng: &Path) -> Option<String> {
    let rest = dng.strip_prefix(derived).ok()?;
    Some(rest.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

fn read_homes(s: &mut Session) -> Homes {
    s.read_library_file(HOMES_FILE).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Note that the photos of `group` are stand-ins of the file `original`, now and in the library's file for the next time it
/// opens (a cache of where the files are: a failure to write it is logged, and the photos go back to the DNG's folder then).
fn remember_home(s: &mut Session, derived: &Path, dng: &Path, original: &str, group: impl Iterator<Item = lightcraft_catalog::PhotoId>) {
    s.catalog.note_stand_ins(group.map(|id| (id, Some(original.to_string()))));
    let Some(key) = home_key(derived, dng) else { return };
    let mut homes = read_homes(s);
    homes.homes.insert(key, original.to_string());
    match serde_json::to_vec_pretty(&homes) {
        Ok(bytes) => {
            if let Err(e) = s.write_library_file(HOMES_FILE, &bytes) {
                log::warn!("stand-ins: {HOMES_FILE}: {e}");
            }
        }
        Err(e) => log::warn!("stand-ins: {HOMES_FILE}: {e}"),
    }
}

/// Called when a library opens: the photos linked to a stand-in DNG that [`Homes`] knows are noted as stand-ins of their
/// originals again ([`lightcraft_catalog::Catalog::note_stand_ins`]). A photo since linked elsewhere is not in the file's
/// list (it has another DNG, or none), and so is left as it is.
pub(crate) fn note_stand_ins(s: &mut Session) {
    let Some(derived) = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("Derived")) else { return };
    let homes = read_homes(s).homes;
    if homes.is_empty() {
        return;
    }
    let kept: Vec<_> = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            Source::File { path } => {
                let original = homes.get(&home_key(&derived, Path::new(path))?)?;
                Some((p.id, Some(original.clone())))
            }
            Source::Demo { .. } => None,
        })
        .collect();
    s.catalog.note_stand_ins(kept);
}

/// `photo.attachDecoded {id, path, previewPath?, whiteXY?, baselineExposure?, force?}`: `path` is the host's linear development of
/// the photo's original (a float TIFF in linear light of any primaries, white = 1, values above it are highlights);
/// `previewPath` the same picture rendered the host's default way (a JPEG, any size, sRGB), which gives the raw its
/// starting look: the engine fits its default tone to it, as it does for a DNG with a preview. The host deletes
/// both afterwards. `whiteXY` is the scene white `[x, y]` the host balanced the picture for (its as-shot white): the
/// DNG then holds the picture as the raw it came from, so white balance edits work as on any raw (true Kelvin and tint,
/// As Shot, Auto). `lensCorrected`: the host's decoder corrected the lens (Core Image does, for a camera whose file says how):
/// the DNG says so (`lc:LensCorrected`), and no lens profile corrects that lens again. Photos with edits are refused unless
/// `force` (their settings were made on the camera's JPEG, a display-referred picture, and mean something else on a linear one).
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
    let decoded = lightcraft_codecs::decode(&tiff, DecodeOptions { max_size: None, max_pixels: MAX_PIXELS, half: false })
        .map_err(|e| bad(C, format!("{src}: {e}")))?;
    drop(tiff);
    let rgb = decoded.into_working();
    if rgb.width == 0 || rgb.height == 0 {
        return Err(bad(C, format!("{src}: an empty picture")));
    }
    // (the packet the DNG would get anyway, and the mark: card #538, Core Image had corrected an OM-5's lens and the lens
    // database's profile corrected it again, 3 % enlarged)
    let lens_corrected = p.get("lensCorrected").and_then(Value::as_bool).unwrap_or(false);
    let xmp = lens_corrected.then(|| lightcraft_meta::write_xmp_lc(&meta, &[("LensCorrected", "True")]));
    let curve = preview.as_deref().and_then(|jpeg| fitted_tone(&rgb, &meta, white, baseline, jpeg, xmp.as_deref()));
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
    let dng = lightcraft_raw::write_linear_dng(rgb.width, rgb.height, rgb.data.as_flattened(), &lightcraft_color::REC2020, &meta, xmp, look)
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
    // (the photos stay in the folder the original is in: card #556, a Local folder showed "No Photos" once its raw was exported)
    remember_home(s, &library.join("Derived"), &out, original, group.iter().map(|g| g.id));
    // size, hash and "Preview only" now come from the DNG; an unedited photo gets the raw defaults; renders from the JPEG go
    super::convert::heal_preview_only(s, master_id);
    for g in &group {
        if let Some(fresh) = s.catalog.photo(g.id) {
            s.forget_renders(fresh);
        }
    }
    Ok(json!({"id": id.0, "path": out_path, "original": original, "width": width, "height": height}))
}

/// Stand-ins made before their DNG said whether the host's decoder had corrected the lens (`lc:LensCorrected`) may hold
/// the lens database's profile in the catalog (a lens refresh or a Reload gave it). A frontend that draws the photo itself
/// applies it on top of the decoder's correction: such a photo exported about 3 % enlarged (card #538). Called when a
/// library opens: the profile goes from each stand-in whose camera's lens the decoder corrects
/// ([`crate::lens_profiles::decoder_corrects`]), journaled and synced, not an undo step. Only the catalog is read, and
/// the lens database only when a stand-in holds a profile.
pub(crate) fn drop_second_lens_corrections(s: &mut Session) {
    if stand_ins_with_a_profile(s).is_empty() {
        return;
    }
    if let Some(db) = crate::lens_profiles::database() {
        drop_second_lens_corrections_with(s, &db);
    }
}

fn drop_second_lens_corrections_with(s: &mut Session, db: &lightcraft_lensdb::LensDb) {
    let ops: Vec<Op> = stand_ins_with_a_profile(s)
        .into_iter()
        .filter(|(_, make, model)| crate::lens_profiles::decoder_corrects(db, make, model))
        .map(|(id, ..)| Op::SetEmbeddedLens { id, lens: None })
        .collect();
    let n = ops.len();
    if n > 0
        && let Err(e) = s.apply_system(Op::Batch { ops })
    {
        log::warn!("stand-ins: can't drop the second lens correction of {n} photos: {e}");
    }
}

/// The stand-ins (photos whose file is in the library's Derived folder, where only `photo.attachDecoded` writes) that
/// hold a lens database profile: id, camera make and model (the catalog keeps them joined, make first, as the file named
/// them: the lens lookup takes them apart, and a model that starts with the maker's own word, "OM-5", reads wrong joined).
fn stand_ins_with_a_profile(s: &Session) -> Vec<(lightcraft_catalog::PhotoId, String, String)> {
    let Some(derived) = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("Derived")) else { return Vec::new() };
    s.catalog
        .photos()
        .filter(|p| p.embedded_lens.as_ref().is_some_and(|l| matches!(l.origin, lightcraft_develop::LensOrigin::Database { .. })))
        .filter(|p| matches!(&p.source, Source::File { path } if Path::new(path).starts_with(&derived)))
        .map(|p| {
            let make = p.meta.details().make.clone();
            let model = p.meta.camera.strip_prefix(make.as_str()).unwrap_or(&p.meta.camera).trim().to_string();
            (p.id, make, model)
        })
        .collect()
}

pub fn specs() -> Vec<CommandSpec> {
    // (`explicit_targets`: a call that names its photo is not refused for an empty selection, and `attach` checks that photo itself. A native
    // frontend's export develops the preview-only raws it names before drawing them, whatever is selected: refused, a compressed ORF exported
    // from a script, the control channel or a sheet given its photos went out at its embedded preview's 3200 x 2400, not its 5184 x 3888)
    vec![CommandSpec {
        explicit_targets: true,
        ..cmd!(
            "photo.attachDecoded",
            "Develop with System Decoder",
            [],
            None,
            "{id, path, previewPath?, whiteXY?, baselineExposure?, compact?, lensCorrected?, force?} — a raw shown from its embedded preview (no decoder here) becomes a raw again: `path` is a float TIFF of its linear development by the host's own decoder (on Apple platforms: Core Image's), `previewPath` a JPEG of the same picture as the host renders it (the starting look), `whiteXY` the scene white it balanced for (the as-shot white), `lensCorrected` that its decoder corrected the lens (then no lens profile corrects it again), written as a LinearRaw DNG in the library's Derived folder and linked like Convert to DNG (the original stays). Refused for a photo with edits unless `force` → {id, path, original, width, height}",
            has_selection,
            attach
        )
    }]
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

        let r = s
            .execute(
                "photo.attachDecoded",
                &json!({"id": id.0, "path": tiff.to_string_lossy(), "previewPath": preview.to_string_lossy(), "whiteXY": [0.40, 0.38]}),
            )
            .unwrap();
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
        // the white the host balanced for is the as-shot white, so white balance edits start from the camera's own
        let white = lightcraft_raw::color::as_shot_white_xy(&raw);
        assert!((white.x - 0.40).abs() < 2e-3 && (white.y - 0.38).abs() < 2e-3, "{white:?}");
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

    /// A host whose decoder corrected the lens says so (`lensCorrected`): the DNG carries it (`lc:LensCorrected`), which keeps a lens
    /// profile from correcting that lens again (card #538). A host that doesn't say: no mark.
    #[test]
    fn a_lens_the_host_corrected_is_marked_in_the_dng() {
        let (mut s, dir, files) = library("stand-in-lens");
        let tiff = dir.join("linear.tif");
        std::fs::write(&tiff, linear_tiff(32, 32, 1.0)).unwrap();
        let preview = dir.join("preview.jpg");
        std::fs::write(&preview, jpeg(32, 32)).unwrap();
        for (name, corrected) in [("DSC_0005.cr3", Some(true)), ("DSC_0006.cr3", None), ("DSC_0007.cr3", Some(false))] {
            let id = import_preview_only(&mut s, &files, name);
            let mut p = json!({"id": id.0, "path": tiff.to_string_lossy(), "previewPath": preview.to_string_lossy()});
            if let Some(c) = corrected {
                p["lensCorrected"] = json!(c);
            }
            let r = s.execute("photo.attachDecoded", &p).unwrap();
            let dng = std::fs::read(r["path"].as_str().unwrap()).unwrap();
            let read = lightcraft_meta::extract(&dng);
            assert_eq!(read.lens_corrected, corrected == Some(true), "{name}");
        }
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in made before the mark that a lens refresh gave the lens database's profile loses it when the library opens,
    /// when its camera's lens the decoder corrects (an OM-5's): it exported about 3 % enlarged (card #538). A Four Thirds
    /// DSLR's stand-in and a photo that is not a stand-in keep theirs; nothing is an undo step, and it lasts.
    #[test]
    fn a_stand_in_s_second_lens_correction_goes_when_the_library_opens() {
        use lightcraft_catalog::Op;
        use lightcraft_develop::{EmbeddedLens, LensOrigin};
        let (mut s, dir, files) = library("stand-in-old-lens");
        let tiff = dir.join("linear.tif");
        std::fs::write(&tiff, linear_tiff(32, 32, 1.0)).unwrap();
        let db = lightcraft_lensdb::LensDb::parse(
            r#"<lensdatabase>
              <camera><maker>OM Digital Solutions</maker><model>OM-5</model><mount>Micro 4/3 System</mount><cropfactor>2</cropfactor></camera>
              <camera><maker>Olympus</maker><model>E-510</model><mount>4/3 System</mount><cropfactor>2</cropfactor></camera>
            </lensdatabase>"#,
        )
        .unwrap();
        let profile = EmbeddedLens { warp: Some(Default::default()), origin: LensOrigin::Database { serial: 3 }, ..Default::default() };
        let give = |s: &mut crate::Session, id: PhotoId, make: &str, model: &str| {
            let mut meta = s.catalog.photo(id).unwrap().meta.clone();
            meta.camera = format!("{make} {model}");
            meta.edit_details(|d| d.make = make.into());
            s.apply_quiet(Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
            s.apply_quiet(Op::SetEmbeddedLens { id, lens: Some(Box::new(profile)) }).unwrap();
        };
        // (each attached before the next import: the same test file again would be a duplicate of an unattached one)
        let stand_in = |s: &mut crate::Session, name: &str| {
            let id = import_preview_only(s, &files, name);
            s.execute("photo.attachDecoded", &json!({"id": id.0, "path": tiff.to_string_lossy()})).unwrap();
            id
        };
        let om5 = stand_in(&mut s, "P5010001.cr3");
        let e510 = stand_in(&mut s, "P5010002.cr3");
        let not_a_stand_in = import_preview_only(&mut s, &files, "P5010003.cr3");
        give(&mut s, om5, "OM Digital Solutions", "OM-5");
        give(&mut s, e510, "Olympus", "E-510");
        give(&mut s, not_a_stand_in, "OM Digital Solutions", "OM-5");
        let undo = s.undo.len();

        super::drop_second_lens_corrections_with(&mut s, &db);
        let lens = |s: &crate::Session, id: PhotoId| s.catalog.photo(id).unwrap().embedded_lens.as_deref().copied();
        assert_eq!(lens(&s, om5), None, "the decoder corrected the OM-5's lens: no second correction");
        assert_eq!(lens(&s, e510), Some(profile), "the decoder did not correct a Four Thirds DSLR's lens");
        assert_eq!(lens(&s, not_a_stand_in), Some(profile), "a photo not in the Derived folder is not a stand-in");
        assert_eq!(s.undo.len(), undo, "not an undo step");
        // journaled: still gone after a reopen, and nothing more to do
        s.persist().unwrap();
        drop(s);
        let mut s = crate::Session::new().with_fs();
        s.open_library(dir.join("lib"), false).unwrap();
        assert_eq!(lens(&s, om5), None);
        assert!(super::stand_ins_with_a_profile(&s).iter().all(|(id, ..)| *id == e510));
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A raw shown from a Local folder keeps its place in that folder once the host developed it (card #556: after an export
    /// the folder showed "No Photos", the photo being linked to its DNG in the library's Derived folder): the folder lists it and
    /// its sibling, browsing the folder again finds it and adds nothing, and so does a reopened library.
    #[test]
    fn a_local_raw_stays_in_its_folder_after_its_stand_in() {
        let (mut s, dir, files) = library("stand-in-folder");
        let trip = files.join("trip");
        std::fs::create_dir_all(&trip).unwrap();
        std::fs::write(trip.join("DSC_0001.cr3"), cr3_with_preview(96, 64)).unwrap();
        std::fs::write(trip.join("DSC_0002.cr3"), cr3_with_preview(80, 60)).unwrap();
        let tiff = dir.join("linear.tif");
        std::fs::write(&tiff, linear_tiff(64, 48, 1.0)).unwrap();
        let browse = |s: &mut crate::Session| s.execute("library.browse", &json!({"path": trip.to_string_lossy()})).unwrap();
        let named = |s: &mut crate::Session| {
            let mut v: Vec<String> = s.visible().to_vec().iter().filter_map(|id| s.catalog.photo(*id)).map(|p| p.file_name.clone()).collect();
            v.sort();
            v
        };

        let r = browse(&mut s);
        assert_eq!((r["photos"].as_u64(), r["new"].as_u64()), (Some(2), Some(2)), "{r}");
        let first = s.catalog.photos().find(|p| p.file_name == "DSC_0001.cr3").map(|p| p.id).unwrap();
        assert!(s.catalog.photo(first).unwrap().local);
        s.execute("photo.attachDecoded", &json!({"id": first.0, "path": tiff.to_string_lossy()})).unwrap();

        let p = s.catalog.photo(first).unwrap();
        assert_eq!(p.format, "DNG", "the photo is linked to the stand-in");
        assert!(p.local, "and is still a record of the folder");
        // (the DNG is named after the raw, the other photo is as it was)
        assert_eq!(named(&mut s), ["DSC_0001.dng", "DSC_0002.cr3"], "the folder shows both photos");
        // browsing the folder again finds the photo by its original: no second record of the file
        let again = browse(&mut s);
        assert_eq!((again["photos"].as_u64(), again["new"].as_u64()), (Some(2), Some(0)), "{again}");
        assert_eq!(s.catalog.photos().count(), 2);

        // it lasts: the library opens with the photo in its folder still
        s.persist().unwrap();
        drop(s);
        let mut s = crate::Session::new().with_fs();
        s.open_library(dir.join("lib"), false).unwrap();
        let r = browse(&mut s);
        assert_eq!((r["photos"].as_u64(), r["new"].as_u64()), (Some(2), Some(0)), "{r}");
        assert_eq!(named(&mut s), ["DSC_0001.dng", "DSC_0002.cr3"]);
        assert_eq!(s.catalog.photos().count(), 2);
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A call that names its photo is taken with nothing selected (an export from a script or the control channel names its photos and
    /// selects none); one that names none still needs a selection.
    #[test]
    fn a_photo_named_by_id_takes_a_stand_in_with_nothing_selected() {
        let (mut s, dir, files) = library("stand-in-unselected");
        let id = import_preview_only(&mut s, &files, "DSC_0004.cr3");
        let tiff = dir.join("linear.tif");
        std::fs::write(&tiff, linear_tiff(32, 32, 1.0)).unwrap();
        s.selection.ids.clear();
        s.selection.active = None;
        let e = s.execute("photo.attachDecoded", &json!({"path": tiff.to_string_lossy()})).unwrap_err().to_string();
        assert!(e.contains("no photos selected"), "{e}");
        let r = s.execute("photo.attachDecoded", &json!({"id": id.0, "path": tiff.to_string_lossy()})).unwrap();
        assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(32), Some(32)), "{r}");
        assert!(s.catalog.photo(id).unwrap().preview_only.is_none());
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
        for white in [json!([0.3]), json!([2.0, 2.0]), json!("warm"), json!([null, 0.3])] {
            let p = json!({"id": other.0, "path": tiff.to_string_lossy(), "whiteXY": white});
            assert!(s.execute("photo.attachDecoded", &p).is_err(), "{white}");
        }
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
