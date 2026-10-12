//! The browser's file chooser for commands that take files by content (File ▸ Import Profiles &
//! Presets…, the Presets panel's Import Presets…): the app's [`HostPicker`]. The chosen files are
//! read into memory and handed over through a channel; the command then runs again with them
//! ([`lightcraft_ui_egui::pick::ask_host`]).
//!
//! [`HostPicker`]: lightcraft_ui_egui::pick::HostPicker

/// A file larger than this isn't read (a preset is a few KB; a "DNG preset" a few MB): reading it
/// whole into the page's memory could take more than the browser allows.
pub const MAX_FILE_BYTES: f64 = 200.0 * 1024.0 * 1024.0;

/// The chooser's `accept` list for these extensions (`.xmp,.zip,…`). `.cube` is left out: a LUT
/// profile is kept as a file in a library folder, which the browser has none of.
pub fn accept(extensions: &[&str]) -> String {
    extensions.iter().filter(|e| !e.eq_ignore_ascii_case("cube")).map(|e| format!(".{e}")).collect::<Vec<_>>().join(",")
}

/// Why `name` (of `size` bytes) isn't read, if it is too large.
pub fn too_large(name: &str, size: f64) -> Option<String> {
    let readable = size.is_finite() && size <= MAX_FILE_BYTES;
    (!readable).then(|| format!("{name}: too large to import here ({:.0} MB)", size / 1e6))
}

#[cfg(target_arch = "wasm32")]
pub use dom::pick;

#[cfg(target_arch = "wasm32")]
mod dom {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::{Receiver, Sender, channel};

    use lightcraft_ui_egui::pick::{HostPicked, PickKind, PickRequest, PickedFile};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    /// Show the browser's file chooser for `req` (files only: the browser has no folder or save
    /// dialog for this). Nothing chosen (the chooser's `cancel`) hands over nothing.
    pub fn pick(req: PickRequest, ctx: egui::Context) -> Result<Receiver<HostPicked>, String> {
        if !matches!(req.kind, PickKind::Files | PickKind::File) {
            return Err("the browser can only open files here".into());
        }
        let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
        let input: web_sys::HtmlInputElement = doc.create_element("input").map_err(|e| format!("no file chooser: {e:?}"))?.unchecked_into();
        input.set_type("file");
        input.set_multiple(req.kind == PickKind::Files);
        if let Some((_, extensions)) = &req.filter {
            input.set_accept(&super::accept(extensions));
        }
        let (tx, rx) = channel();
        // answered once: by `change` (the files) or by `cancel`
        let answer: Rc<RefCell<Option<Sender<HostPicked>>>> = Rc::new(RefCell::new(Some(tx)));
        let (inp, a, c) = (input.clone(), answer.clone(), ctx.clone());
        let on_change = Closure::<dyn FnMut()>::new(move || {
            let Some(tx) = a.try_borrow_mut().ok().and_then(|mut a| a.take()) else { return };
            let list = inp.files();
            let files: Vec<web_sys::File> = list.map(|l| (0..l.length()).filter_map(|i| l.get(i)).collect()).unwrap_or_default();
            let ctx = c.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let _ = tx.send(read(files).await);
                ctx.request_repaint();
            });
        });
        let on_cancel = Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = answer.try_borrow_mut().ok().and_then(|mut a| a.take()) {
                let _ = tx.send(HostPicked::default());
                ctx.request_repaint();
            }
        });
        input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
        // (a browser without the event leaves the pick waiting: asking again replaces it)
        let _ = input.add_event_listener_with_callback("cancel", on_cancel.as_ref().unchecked_ref());
        on_change.forget();
        on_cancel.forget();
        input.click();
        Ok(rx)
    }

    /// The chosen files' contents, or why each couldn't be read.
    async fn read(files: Vec<web_sys::File>) -> HostPicked {
        let mut picked = HostPicked::default();
        for f in files {
            let name = f.name();
            if let Some(why) = super::too_large(&name, f.size()) {
                picked.failed.push(why);
                continue;
            }
            match wasm_bindgen_futures::JsFuture::from(f.array_buffer()).await {
                Ok(buf) => picked.files.push(PickedFile { bytes: js_sys::Uint8Array::new(&buf).to_vec(), name }),
                Err(e) => picked.failed.push(format!("{name}: couldn't be read ({e:?})")),
            }
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chooser_offers_the_preset_files_the_browser_can_import() {
        assert_eq!(accept(&["lcpreset", "xmp", "zip", "cube", "CUBE"]), ".lcpreset,.xmp,.zip");
        assert_eq!(accept(lightcraft_ui_egui::pick::PRESET_EXTENSIONS), ".lcpreset,.xmp,.lrtemplate,.zip,.dng,.lmp,.mplumpack");
        assert_eq!(accept(&[]), "");
        assert!(too_large("a.xmp", 4096.0).is_none());
        assert!(too_large("big.dng", MAX_FILE_BYTES + 1.0).is_some_and(|w| w.starts_with("big.dng: too large")));
        assert!(too_large("odd", f64::NAN).is_some(), "a size the browser doesn't know isn't read");
    }
}
