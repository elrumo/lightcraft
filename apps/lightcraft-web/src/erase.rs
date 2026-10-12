//! AI Remove's model in the browser (`lightcraft_ui_egui::erase`): downloaded once, when the user
//! asks, from the model's own repository (Hugging Face sends CORS headers), checked against its
//! pinned size and SHA-256, and kept in the site's storage (`models/migan/migan.onnx`, next to the
//! library; Back Up Library leaves it out, `?reset` removes it). The session checks it again when
//! it loads it.

use lightcraft_ui_egui::erase::{ModelFetch, NOT_STORED};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::backend::Backend;

/// Where the model is kept.
const KEY: &str = "models/migan/migan.onnx";

fn js(e: wasm_bindgen::JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// The host's model fetch.
pub fn exec(backend: Option<Backend>) -> ModelFetch {
    Box::new(move |download, tx, ctx: egui::Context| {
        let backend = backend.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(get(backend.as_ref(), download).await);
            ctx.request_repaint();
        });
    })
}

/// The stored model, or (with `download`) a fresh copy, which is then stored.
async fn get(backend: Option<&Backend>, download: bool) -> Result<Vec<u8>, String> {
    use lightcraft_engine::erase::{BASE_URL, FILE, verify};
    if !download {
        let stored = match backend {
            Some(b) => b.read(KEY).await?,
            None => None,
        };
        return stored.ok_or_else(|| NOT_STORED.to_string());
    }
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_str(&format!("{BASE_URL}/{FILE}"))).await.map_err(js)?.dyn_into().map_err(js)?;
    if !resp.ok() {
        return Err(format!("the model's server answered {}", resp.status()));
    }
    let buf = JsFuture::from(resp.array_buffer().map_err(js)?).await.map_err(js)?;
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    if !verify(&bytes) {
        return Err("the downloaded model is not the expected file; try again later".into());
    }
    // (a session that stores nothing, `?store=memory`, downloads it again next time)
    if let Some(b) = backend {
        b.write(KEY, &bytes).await?;
    }
    Ok(bytes)
}
