//! Generative Expand: the crop's Expand and Generate (`crop.expand`, `crop.generate`,
//! `crop.expandModels`). Classic needs no download, so most of this runs everywhere; the model
//! paths run when a copy of MI-GAN (`$LIGHTCRAFT_MIGAN`) or LaMa (`$LIGHTCRAFT_LAMA`) is given.

use lightcraft_geom::{CropGeometry, EXPAND_MAX, Rect};
use serde_json::json;

use crate::Session;
use crate::expand::ExpandModel;
use crate::hardware::{DeviceClass, DeviceProfile, GB};

/// A computer with `gb` GB, `gpu`, `cores` threads.
fn computer(gb: u64, gpu: bool, cores: usize) -> DeviceProfile {
    DeviceProfile { class: DeviceClass::Computer, memory_bytes: Some(gb * GB), gpu, cores }
}

/// A demo session on a mid-range computer without a GPU (the tests don't depend on the machine).
fn session() -> Session {
    let mut s = Session::with_demo();
    s.set_device(computer(16, false, 8));
    s
}

fn crop(s: &Session) -> CropGeometry {
    s.develop_of(s.active().unwrap()).unwrap().crop.geometry
}

fn dims(s: &Session) -> (f64, f64) {
    let p = s.catalog.photo(s.active().unwrap()).unwrap();
    let (w, h) = (p.width as f64, p.height as f64);
    if p.develop.orientation.swaps_axes() { (h, w) } else { (w, h) }
}

fn has_fill(s: &Session) -> bool {
    s.develop_of(s.active().unwrap()).unwrap().expand_fill.is_some()
}

#[test]
fn expand_lets_the_crop_reach_past_the_photo() {
    let mut s = session();
    let (w, h) = dims(&s);
    // off: a drag stops at the photo's edge
    s.execute("crop.drag", &json!({"handle": "left", "from": [0.0, 0.5], "to": [-0.3, 0.5]})).unwrap();
    assert!(crop(&s).rect.x0 >= 0.0);
    // on: the same drag goes past it, up to the limit
    assert_eq!(s.execute("crop.expand", &json!({})).unwrap()["on"], true);
    s.execute("crop.drag", &json!({"handle": "left", "from": [0.0, 0.5], "to": [-0.3, 0.5]})).unwrap();
    assert!((crop(&s).rect.x0 + 0.3).abs() < 1e-6, "{:?}", crop(&s));
    s.execute("crop.drag", &json!({"handle": "right", "from": [1.0, 0.5], "to": [9.0, 0.5]})).unwrap();
    assert!((crop(&s).rect.x1 - (1.0 + EXPAND_MAX)).abs() < 1e-6);
    assert!(crop(&s).reaches_past(w, h));
    // crop.set keeps a rect within the grown photo, and shrinks one beyond it
    s.execute("crop.set", &json!({"rect": [-0.2, -0.1, 1.1, 1.0]})).unwrap();
    assert_eq!(crop(&s).rect, Rect::new(-0.2, -0.1, 1.1, 1.0));
    s.execute("crop.set", &json!({"rect": [-5.0, 0.0, 1.0, 1.0]})).unwrap();
    assert!(crop(&s).is_within(w, h, true));
    // straightening keeps the crop (the corners it uncovers are for the fill)
    s.execute("crop.set", &json!({"rect": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    s.execute("crop.straighten", &json!({"angle": 8.0})).unwrap();
    assert_eq!(crop(&s).rect, Rect::UNIT);
    // choosing an aspect adds canvas around the whole photo
    s.execute("crop.straighten", &json!({"angle": 0.0})).unwrap();
    s.execute("crop.aspect", &json!({"aspect": "1x1"})).unwrap();
    let r = crop(&s).rect_px(w, h);
    assert!((r.width() - r.height()).abs() < 1e-6 && r.width() >= w.max(h) - 1e-6, "{r:?}");
    // off: back inside the photo, in one undo step
    assert_eq!(s.execute("crop.expand", &json!({"on": false})).unwrap()["changed"], true);
    assert!(crop(&s).is_within_image(w, h));
    assert_eq!(s.execute("crop.expand", &json!({"on": false})).unwrap()["changed"], false);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.develop_of(s.active().unwrap()).unwrap().crop.expand);
}

#[test]
fn classic_generate_fills_the_new_area_as_one_undo_step() {
    let mut s = session();
    let id = s.active().unwrap();
    // nothing to fill yet: Expand off, then nothing past the photo
    let e = s.execute("crop.generate", &json!({"model": "classic"})).unwrap_err().to_string();
    assert!(e.contains("Expand"), "{e}");
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    let e = s.execute("crop.generate", &json!({"model": "classic"})).unwrap_err().to_string();
    assert!(e.contains("nothing past the photo"), "{e}");
    assert!(s.execute("crop.generate", &json!({"model": "dall-e"})).is_err());
    s.execute("crop.set", &json!({"rect": [-0.15, 0.0, 1.15, 1.0]})).unwrap();
    // before: the new area is transparent in a render
    let before = s.render_now(id, 400, 400).unwrap().image;
    assert!(before.data.iter().any(|p| p[3] == 0));
    let done = s.execute("crop.generate", &json!({"model": "classic"})).unwrap();
    assert_eq!(done["model"], "classic");
    assert!(done["width"].as_u64().unwrap() > 0 && done["storedBytes"].as_u64().unwrap() > 0, "{done}");
    assert!(has_fill(&s));
    let d = s.develop_of(id).unwrap();
    let f = d.expand_fill.as_ref().unwrap();
    assert_eq!(f.model, "classic");
    let [x0, _, x1, _] = f.fill.bounds().unwrap();
    assert!(x0 <= -0.15 && x1 >= 1.15);
    // after: nothing transparent, and the history says what happened
    let after = s.render_now(id, 400, 400).unwrap().image;
    assert!(after.data.iter().all(|p| p[3] == 255));
    assert_eq!(s.catalog.photo(id).unwrap().history.last().map(|h| h.label.as_str()), Some("Generative Expand"));
    // the fill is a picture of this photo: Copy Settings leaves it behind
    let copied = lightcraft_develop::extract_groups(&d, &lightcraft_develop::SettingsGroup::ALL);
    assert!(copied.get("expand_fill").is_none());
    // one undo step
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!has_fill(&s));
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(has_fill(&s));
    // Expand off drops it; Reset Crop too
    s.execute("crop.expand", &json!({"on": false})).unwrap();
    assert!(!has_fill(&s));
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("crop.reset", &json!({})).unwrap();
    assert!(!has_fill(&s) && !s.develop_of(id).unwrap().crop.expand);
}

#[test]
fn generate_in_the_background_shows_in_the_activity_stack() {
    let mut s = session();
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    s.execute("crop.set", &json!({"rect": [0.0, -0.2, 1.0, 1.0]})).unwrap();
    let r = s.execute("crop.generate", &json!({"model": "classic", "wait": false})).unwrap();
    assert_eq!(r["started"], true);
    assert!(s.execute("crop.generate", &json!({"model": "classic", "wait": false})).is_err(), "one at a time");
    let models = s.execute("crop.expandModels", &json!({})).unwrap();
    assert_eq!(models["running"]["model"], "classic");
    let mut saw_row = false;
    let res = loop {
        saw_row |= s.activity.list().iter().any(|t| t.kind == "expand");
        if let Some(r) = s.poll_expand() {
            break r;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(res, Ok(ExpandModel::Classic));
    assert!(saw_row, "the job had a row in the activity stack");
    assert!(s.activity.list().iter().all(|t| t.kind != "expand"), "the row is gone");
    assert!(has_fill(&s) && s.expand_running().is_none());
}

#[test]
fn a_cancelled_generate_stores_nothing() {
    let mut s = session();
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    s.execute("crop.set", &json!({"rect": [-0.3, -0.3, 1.3, 1.3]})).unwrap();
    s.start_expand(s.active().unwrap(), Some(ExpandModel::Classic)).unwrap();
    assert!(s.cancel_expand());
    let res = loop {
        if let Some(r) = s.poll_expand() {
            break r;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    // (Classic checks once before it starts: a cancel that lands later lets it finish)
    if let Err(e) = res {
        assert!(e.contains("cancelled"), "{e}");
        assert!(!has_fill(&s));
    }
    assert!(!s.cancel_expand(), "nothing runs now");
}

#[test]
fn the_models_are_offered_per_device_and_a_missing_one_says_so() {
    let mut s = session();
    let list = s.execute("crop.expandModels", &json!({})).unwrap();
    let ids: Vec<&str> = list["models"].as_array().unwrap().iter().filter_map(|m| m["id"].as_str()).collect();
    assert_eq!(ids, ["migan", "lama", "sd15-inpaint", "kandinsky22-inpaint", "sdxl-inpaint", "classic"]);
    let at = |id: &str| list["models"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap().clone();
    let classic = at("classic");
    assert_eq!((classic["ready"].as_bool(), classic["default"].as_bool(), classic["downloadBytes"].as_u64()), (Some(true), Some(true), Some(0)));
    // a 16 GB computer without a GPU: LaMa and the 512- and 768-pixel diffusion models, not SDXL
    let diffusion = crate::enhance::Enhancer::AVAILABLE;
    for (id, available) in [("migan", true), ("lama", true), ("sd15-inpaint", diffusion), ("kandinsky22-inpaint", diffusion), ("sdxl-inpaint", false)]
    {
        assert_eq!(at(id)["available"], available, "{id}");
        assert_eq!(at(id)["ready"], false, "{id}");
    }
    assert_eq!((at("sd15-inpaint")["restricted"].as_bool(), at("kandinsky22-inpaint")["restricted"].as_bool()), (Some(true), Some(false)));
    assert_eq!(at("sdxl-inpaint")["diffusion"], true);
    // Automatic: LaMa on a CPU with 8 threads; nothing is downloaded, so Generate falls back to Classic
    assert_eq!((list["automatic"].as_bool(), list["device"]["class"].as_str()), (Some(true), Some("computer")));
    assert_eq!((at("lama")["recommended"].as_bool(), at("lama")["chosen"].as_bool()), (Some(true), Some(true)));
    assert_eq!(s.default_expand_model(), ExpandModel::Classic);
    s.execute("crop.expand", &json!({"on": true})).unwrap();
    s.execute("crop.set", &json!({"rect": [-0.2, 0.0, 1.0, 1.0]})).unwrap();
    for m in ["migan", "lama", "sd15-inpaint", "kandinsky22-inpaint", "sdxl-inpaint"] {
        let e = s.execute("crop.generate", &json!({"model": m})).unwrap_err().to_string();
        assert!(e.contains("not downloaded") || e.contains("isn't available"), "{m}: {e}");
    }
    if diffusion {
        let e = s.execute("crop.generate", &json!({"model": "sd15-inpaint"})).unwrap_err().to_string();
        assert!(e.contains("2.1 GB") && e.contains("OpenRAIL-M") && e.contains("models.download"), "{e}");
    }
    let e = s.execute("crop.generate", &json!({"model": "sdxl-inpaint"})).unwrap_err().to_string();
    assert!(e.contains("isn't available on this device") && e.contains("16 GB"), "{e}");
    assert!(!has_fill(&s));
    // Settings → AI Models lists every model, offered by the device
    if diffusion {
        let models = s.models();
        let find = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        assert!(find("lama").available && find("migan").available && find("sd15-inpaint").available && !find("sdxl-inpaint").available);
        assert!(find("sd15-inpaint").restricted && !find("kandinsky22-inpaint").restricted);
        assert!(find("sd15-inpaint").download_bytes > 2_000_000_000);
    }
}

/// What each device is offered, and what Automatic picks there.
#[test]
fn the_device_profile_decides_what_is_offered_and_picked() {
    let phone = |gb: u64, gpu: bool| DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(gb * GB), gpu, cores: 6 };
    let browser = DeviceProfile { class: DeviceClass::Browser, memory_bytes: None, gpu: false, cores: 4 };
    let offered = |p: &DeviceProfile| ExpandModel::ALL.into_iter().filter(|m| m.fits(p)).map(|m| m.id()).collect::<Vec<_>>();
    let d = crate::enhance::Enhancer::AVAILABLE && !cfg!(target_arch = "wasm32");
    let with = |ids: &[&'static str], diffusion: &[&'static str]| -> Vec<&'static str> {
        ExpandModel::ALL.into_iter().map(|m| m.id()).filter(|i| ids.contains(i) || (d && diffusion.contains(i))).collect()
    };
    // a phone: MI-GAN always, LaMa with 2 GB, Stable Diffusion 1.5 with a GPU and 4 GB
    assert_eq!(offered(&phone(1, true)), with(&["migan", "classic"], &[]));
    assert_eq!(offered(&phone(3, true)), with(&["migan", "lama", "classic"], &[]));
    assert_eq!(offered(&phone(5, false)), with(&["migan", "lama", "classic"], &[]));
    assert_eq!(offered(&phone(5, true)), with(&["migan", "lama", "classic"], &["sd15-inpaint"]));
    assert_eq!(ExpandModel::recommended(&phone(1, true)), ExpandModel::MiGan);
    assert_eq!(ExpandModel::recommended(&phone(3, true)), ExpandModel::Lama);
    assert_eq!(ExpandModel::recommended(&phone(5, true)), if d { ExpandModel::Sd15 } else { ExpandModel::Lama });
    // an unknown phone memory offers no more than MI-GAN
    assert_eq!(offered(&DeviceProfile { memory_bytes: None, ..phone(5, true) }), with(&["migan", "classic"], &[]));
    // computers
    assert_eq!(offered(&computer(8, false, 4)), with(&["migan", "lama", "classic"], &["sd15-inpaint"]));
    assert_eq!(offered(&computer(16, true, 8)), with(&["migan", "lama", "classic"], &["sd15-inpaint", "kandinsky22-inpaint", "sdxl-inpaint"]));
    assert_eq!(offered(&computer(32, false, 8)), with(&["migan", "lama", "classic"], &["sd15-inpaint", "kandinsky22-inpaint", "sdxl-inpaint"]));
    let pick = |p: DeviceProfile| ExpandModel::recommended(&p);
    assert_eq!(pick(computer(8, false, 4)), ExpandModel::Lama);
    assert_eq!(pick(computer(16, false, 16)), if d { ExpandModel::Sd15 } else { ExpandModel::Lama });
    assert_eq!(pick(computer(16, true, 8)), if d { ExpandModel::Sd15 } else { ExpandModel::Lama });
    assert_eq!(pick(computer(64, true, 12)), if d { ExpandModel::Sdxl } else { ExpandModel::Lama });
    // a computer that doesn't say how much memory it has gets every model offered
    assert_eq!(
        offered(&DeviceProfile { memory_bytes: None, ..computer(1, false, 4) }),
        with(&["migan", "lama", "classic"], &["sd15-inpaint", "kandinsky22-inpaint", "sdxl-inpaint"])
    );
    // the browser: MI-GAN and Classic
    assert_eq!(offered(&browser), ["migan", "classic"]);
    assert_eq!(ExpandModel::recommended(&browser), ExpandModel::MiGan);
    // a phone's host gives its memory limit
    let mut s = Session::with_demo();
    s.set_device(DeviceProfile { memory_bytes: None, ..phone(1, true) });
    s.set_app_memory(Some(5 << 30));
    assert_eq!(s.device.memory_bytes, Some(5 * GB));
    s.set_device(computer(16, false, 8));
    s.set_app_memory(Some(1 << 30));
    assert_eq!(s.device.memory_bytes, Some(16 * GB), "a computer counts its RAM");
}

#[test]
fn the_chosen_model_is_kept_and_automatic_follows_the_device() {
    let mut s = session();
    assert_eq!(s.chosen_expand_model(), ExpandModel::Lama);
    let r = s.execute("crop.expandModel", &json!({"model": "migan"})).unwrap();
    assert_eq!((r["model"].as_str(), r["automatic"].as_bool(), r["ready"].as_bool()), (Some("migan"), Some(false), Some(false)));
    assert_eq!(s.expand.preferred, Some(ExpandModel::MiGan));
    let list = s.execute("crop.expandModels", &json!({})).unwrap();
    assert_eq!(list["automatic"], false);
    assert!(list["models"].as_array().unwrap().iter().any(|m| m["id"] == "migan" && m["chosen"] == true));
    // a model this device doesn't run is refused, an unknown one too
    let e = s.execute("crop.expandModel", &json!({"model": "sdxl-inpaint"})).unwrap_err().to_string();
    assert!(e.contains("doesn't run on this device"), "{e}");
    assert!(s.execute("crop.expandModel", &json!({"model": "dall-e"})).is_err());
    assert!(s.execute("crop.expandModel", &json!({})).is_err());
    assert_eq!(s.expand.preferred, Some(ExpandModel::MiGan));
    // Classic chosen: Generate uses it without a model named
    s.execute("crop.expandModel", &json!({"model": "classic"})).unwrap();
    assert_eq!(s.default_expand_model(), ExpandModel::Classic);
    // back to Automatic; a choice the device stops running falls back to Automatic's
    let r = s.execute("crop.expandModel", &json!({"model": "auto"})).unwrap();
    assert_eq!((r["model"].as_str(), r["automatic"].as_bool()), (Some("lama"), Some(true)));
    s.execute("crop.expandModel", &json!({"model": "lama"})).unwrap();
    s.set_device(DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(GB), gpu: false, cores: 4 });
    assert_eq!(s.chosen_expand_model(), ExpandModel::MiGan);
}

/// With the real models (never part of the repository): each paints the strip and stores it.
/// MI-GAN and LaMa from their files (`$LIGHTCRAFT_MIGAN`, `$LIGHTCRAFT_LAMA`); a diffusion model
/// from a models folder holding its folder (`$LIGHTCRAFT_MODELS/sd15-inpaint/unet/…`), named by
/// `$LIGHTCRAFT_DIFFUSION` (`sd15-inpaint`, `kandinsky22-inpaint`, `sdxl-inpaint`). On a CPU a
/// diffusion model takes minutes.
#[test]
fn the_models_paint_the_new_area() {
    let migan = std::env::var_os("LIGHTCRAFT_MIGAN");
    let lama = std::env::var_os("LIGHTCRAFT_LAMA");
    let diffusion = std::env::var("LIGHTCRAFT_DIFFUSION").ok().and_then(|id| ExpandModel::from_id(&id)).filter(|m| m.diffusion());
    let models = std::env::var_os("LIGHTCRAFT_MODELS");
    let mut todo = vec![(ExpandModel::MiGan, migan), (ExpandModel::Lama, lama)];
    if let (Some(m), Some(dir)) = (diffusion, models) {
        todo.push((m, Some(dir)));
    }
    for (model, path) in todo {
        let Some(path) = path else { continue };
        let mut s = Session::with_demo();
        // (whatever this machine is, it runs every model here)
        s.set_device(computer(64, false, 16));
        match model {
            ExpandModel::MiGan => s.install_eraser(&std::fs::read(&path).unwrap()).unwrap(),
            ExpandModel::Lama => {
                let dir = std::env::temp_dir().join(format!("lc-expand-{}", std::process::id()));
                std::fs::create_dir_all(dir.join("lama")).unwrap();
                let file = dir.join("lama").join(lightcraft_erase::lama::FILE);
                if std::fs::hard_link(&path, &file).is_err() {
                    std::fs::copy(&path, &file).unwrap();
                }
                s.enhancer.dir = Some(dir);
            }
            _ => s.enhancer.dir = Some(path.into()),
        }
        if !s.expand_ready(model) {
            continue;
        }
        s.execute("crop.expand", &json!({"on": true})).unwrap();
        s.execute("crop.aspect", &json!({"aspect": "1x1"})).unwrap();
        let done = s.execute("crop.generate", &json!({"model": model.id()})).unwrap();
        assert_eq!(done["model"], model.id());
        assert!(done["passes"].as_u64().unwrap() >= 1);
        let id = s.active().unwrap();
        assert!(s.render_now(id, 300, 300).unwrap().image.data.iter().all(|p| p[3] == 255));
    }
}
