//! AI Remove's model in the browser (`lightcraft_ui_egui::erase`): downloaded once, when the user
//! asks, from the model's own repository (Hugging Face sends CORS headers), checked against its
//! pinned size and SHA-256, and kept in the site's storage (`models/migan/migan.onnx`, next to the
//! library; Back Up Library leaves it out, `?reset` removes it). The session checks it again when
//! it loads it.

use lightcraft_engine::depth::{DATA_FILE, GRAPH_FILE, Pinned, Size};
use lightcraft_ui_egui::erase::{DepthModelFetch, ModelFetch, NOT_STORED};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::backend::Backend;

/// Where the model is kept.
const KEY: &str = "models/migan/migan.onnx";
/// Where a size of Lens Blur's depth model is kept (the same storage, the same rules): its graph and weights.
fn depth_key(size: Size, file: &str) -> String {
    format!("models/{}/{file}", size.id())
}

fn js(e: wasm_bindgen::JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// The host's model fetch (AI Remove's MI-GAN).
pub fn exec(backend: Option<Backend>) -> ModelFetch {
    use lightcraft_engine::erase::{BASE_URL, FILE, verify};
    exec_model(backend, KEY, format!("{BASE_URL}/{FILE}"), verify)
}

/// The host's fetch of Lens Blur's depth model (Depth Anything 3, in the size asked for): its graph and its weights.
pub fn exec_depth(backend: Option<Backend>) -> DepthModelFetch {
    Box::new(move |size, download, tx, ctx: egui::Context| {
        let backend = backend.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(get_depth(backend.as_ref(), size, download).await);
            ctx.request_repaint();
        });
    })
}

/// Both files of `size`, from the site's storage or downloaded (and kept), and checked together.
async fn get_depth(backend: Option<&Backend>, size: Size, download: bool) -> Result<(Vec<u8>, Vec<u8>), String> {
    let one = |file: &'static str, pinned: Pinned| {
        let key = depth_key(size, file);
        let url = format!("{}/{file}", size.base_url());
        async move { get(backend, download, &key, &url, &|b| pinned.matches(b)).await }
    };
    let graph = one(GRAPH_FILE, size.graph()).await?;
    let data = one(DATA_FILE, size.data()).await?;
    Ok((graph, data))
}

/// A fetch of the model kept at `key`, downloaded from `url` and checked by `verify`.
fn exec_model(backend: Option<Backend>, key: &'static str, url: String, verify: fn(&[u8]) -> bool) -> ModelFetch {
    Box::new(move |download, tx, ctx: egui::Context| {
        let (backend, url) = (backend.clone(), url.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(get(backend.as_ref(), download, key, &url, &verify).await);
            ctx.request_repaint();
        });
    })
}

/// The stored model, or (with `download`) a fresh copy, which is then stored.
async fn get(backend: Option<&Backend>, download: bool, key: &str, url: &str, verify: &dyn Fn(&[u8]) -> bool) -> Result<Vec<u8>, String> {
    if !download {
        let stored = match backend {
            Some(b) => b.read(key).await?,
            None => None,
        };
        return stored.ok_or_else(|| NOT_STORED.to_string());
    }
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_str(url)).await.map_err(js)?.dyn_into().map_err(js)?;
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
        b.write(key, &bytes).await?;
    }
    Ok(bytes)
}
