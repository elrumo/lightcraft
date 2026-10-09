//! Self-hosted sync in the browser (`docs/sync.md`): the engine's sync requests run with `fetch`
//! (same origin as the server that serves this page), photo files come from and go to browser
//! storage (`originals/<hash>`, `proxies/<hash>.lcsp|.lcsm`), and a photo's smart and mini
//! previews are built here when this browser uploads its original.

use std::sync::mpsc::Sender;

use lightcraft_engine::sync::{Body, Done, Task};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::backend::Backend;

/// Where the browser keeps synced previews (the engine's `Store` proxy layout).
pub const PROXIES: &str = "proxies/";

fn js(e: wasm_bindgen::JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// The host's runner for sync tasks.
pub fn exec(backend: Option<Backend>) -> lightcraft_ui_egui::sync_ui::SyncExec {
    Box::new(move |task: Task, tx: Sender<Done>, ctx: egui::Context| {
        let backend = backend.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let id = task.id();
            let done = match run(&task, backend.as_ref()).await {
                Ok(d) => d,
                Err(e) => Done::failed(id, e),
            };
            let _ = tx.send(done);
            ctx.request_repaint();
        });
    })
}

async fn run(task: &Task, backend: Option<&Backend>) -> Result<Done, String> {
    let backend = backend.ok_or("nothing is stored in this browser session (?store=memory)")?;
    match task {
        Task::Proxies { id, original, smart, mini } => {
            let bytes = backend.read(original).await?.ok_or_else(|| format!("{original}: not in browser storage"))?;
            let (s, m) = lightcraft_engine::smart::encode_pair(&bytes, lightcraft_engine::sync::MINI_EDGE)?;
            backend.write(smart, &s).await?;
            backend.write(mini, &m).await?;
            Ok(Done { id: *id, status: 200, body: String::new() })
        }
        Task::Http { id, method, url, token, body, save_to } => {
            let init = web_sys::RequestInit::new();
            init.set_method(method);
            let headers = web_sys::Headers::new().map_err(js)?;
            headers.set("Authorization", &format!("Bearer {token}")).map_err(js)?;
            match body {
                Body::Empty => {}
                Body::Json(j) => {
                    headers.set("Content-Type", "application/json").map_err(js)?;
                    init.set_body(&wasm_bindgen::JsValue::from_str(j));
                }
                Body::File(key) => {
                    let bytes = backend.read(key).await?.ok_or_else(|| format!("{key}: not in browser storage"))?;
                    headers.set("Content-Type", "application/octet-stream").map_err(js)?;
                    init.set_body(&js_sys::Uint8Array::from(bytes.as_slice()));
                }
                Body::FileFrom(key, offset) => {
                    let bytes = backend.read(key).await?.ok_or_else(|| format!("{key}: not in browser storage"))?;
                    headers.set("Content-Type", "application/octet-stream").map_err(js)?;
                    // the rest of a broken upload (from the start when the offset isn't inside the file)
                    let from = usize::try_from(*offset).ok().filter(|f| *f > 0 && *f < bytes.len());
                    if let Some(from) = from {
                        headers.set("Content-Range", &format!("bytes {from}-{}/{}", bytes.len() - 1, bytes.len())).map_err(js)?;
                    }
                    init.set_body(&js_sys::Uint8Array::from(bytes.get(from.unwrap_or(0)..).unwrap_or_default()));
                }
            }
            init.set_headers(&headers);
            let req = web_sys::Request::new_with_str_and_init(url, &init).map_err(js)?;
            let window = web_sys::window().ok_or("no window")?;
            let resp: web_sys::Response = JsFuture::from(window.fetch_with_request(&req)).await.map_err(js)?.dyn_into().map_err(js)?;
            let status = resp.status();
            if *method == "HEAD" {
                // (a HEAD has no body: the engine reads the bytes of a broken upload from here)
                let offset = resp.headers().get("Upload-Offset").ok().flatten().unwrap_or_default();
                return Ok(Done { id: *id, status, body: offset });
            }
            if let (Some(dest), true) = (save_to, resp.ok()) {
                let buf = JsFuture::from(resp.array_buffer().map_err(js)?).await.map_err(js)?;
                backend.write(dest, &js_sys::Uint8Array::new(&buf).to_vec()).await?;
                return Ok(Done { id: *id, status, body: String::new() });
            }
            let text = JsFuture::from(resp.text().map_err(js)?).await.map_err(js)?.as_string().unwrap_or_default();
            Ok(Done { id: *id, status, body: text })
        }
    }
}
