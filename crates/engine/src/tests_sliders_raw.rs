//! FB-SLIDER on real raws (the corpus; skipped without it): each develop control, at both ends, changes the
//! picture the engine renders for the photo, with the photo's own raw defaults, Kelvin white balance, camera
//! matrices and profile. `LIGHTCRAFT_CORPUS` names the corpus folder (a worktree has none of its own).

use lightcraft_develop::{CONTROLS, DevelopSettings, controls};
use serde_json::json;

use crate::Session;

/// Controls a preview of a raw can't show, and why.
fn preview_cannot_show(id: &str, has_lens: bool, scale: f64) -> Option<&'static str> {
    if id == "enhance.denoise" || id.starts_with("lensBlur.") || id.starts_with("hdr.") {
        return Some("needs a model, a depth map or an HDR display");
    }
    if id.starts_with("optics.profile") && !has_lens {
        return Some("the photo has no lens correction to scale");
    }
    // capture sharpening sizes its radius in the original's pixels (issue #960): a preview a fifth of the original is
    // sharpened a few percent as much, and its Detail slider moves that by less than one 8-bit level
    if id == "detail.sharpenDetail" && scale < 0.5 {
        return Some("capture sharpening is faint in a small preview");
    }
    None
}

#[test]
fn every_control_changes_a_real_raw() {
    let corpus = std::env::var_os("LIGHTCRAFT_CORPUS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
        .join("raw");
    let mut failures: Vec<String> = Vec::new();
    for file in ["dng-apple-iphone17pro-proraw-jxl.dng", "cr3-canon-r6-raw.cr3"] {
        let path = corpus.join(file);
        if !path.exists() {
            lightcraft_testkit::skip!(Corpus, "{} absent", path.display());
            continue;
        }
        let lib = crate::tests_xmp::temp_dir(&format!("sweep-{}", file.split('-').nth(1).unwrap_or("raw")));
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
        let id = s.catalog.photos().next().unwrap().id;
        s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
        // (the photo's lens correction is recorded when its file is first decoded)
        s.render_now(id, 64, 64).unwrap();
        let (native, has_lens) = {
            let p = s.catalog.photo(id).unwrap();
            (p.width.max(p.height) as f64, s.embedded_lens(id).is_some())
        };
        let edge = 960usize;
        eprintln!("{file}: {native} px long, lens correction: {has_lens}");
        let render = |s: &mut Session| s.render_now(id, edge, edge).unwrap().image;
        let own = (*s.develop_of(id).unwrap()).clone();
        for c in CONTROLS {
            if preview_cannot_show(c.id, has_lens, edge as f64 / native).is_some() {
                continue;
            }
            // the photo's own settings, with what the control needs, the control at its default
            let mut base: DevelopSettings = own.clone();
            controls::prepare_to_show(&mut base, c.id);
            controls::set(&mut base, c.id, c.default);
            s.execute("develop.merge", &json!({"settings": base.to_json()})).unwrap();
            let before = render(&mut s);
            for (name, end) in [("min", c.min), ("max", c.max)] {
                if end == c.default || (c.id.starts_with("grading.") && c.id.ends_with(".hue") && end == c.max) {
                    continue;
                }
                s.execute("develop.set", &json!({"control": c.id, "value": end})).unwrap();
                let after = render(&mut s);
                s.execute("edit.undo", &json!({})).unwrap();
                if before.data == after.data {
                    failures.push(format!("{file}: {} {name} ({end}) changes nothing", c.id));
                }
            }
        }
        let _ = std::fs::remove_dir_all(&lib);
    }
    assert!(failures.is_empty(), "{} problems:\n{}", failures.len(), failures.join("\n"));
}
