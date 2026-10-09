//! Map tiles in the browser: the Map view's requests run with `fetch` (tile servers such as
//! OpenStreetMap's allow cross-origin reads), and the browser's own HTTP cache keeps what it
//! fetched, so a second visit to a place is instant and a repeated one costs the server nothing.

use lightcraft_engine::tiles::{TileRequest, TileResult};
use lightcraft_ui_egui::panels::map::TileExec;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

fn js(e: wasm_bindgen::JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// The host's runner for tile requests.
pub fn exec() -> TileExec {
    Box::new(|req: TileRequest, tx, ctx: egui::Context| {
        wasm_bindgen_futures::spawn_local(async move {
            let id = req.id;
            let tile = load(&req).await;
            let _ = tx.send(TileResult { id, tile });
            ctx.request_repaint();
        });
    })
}

async fn load(req: &TileRequest) -> Result<lightcraft_engine::tiles::Tile, String> {
    // the browser can't be asked for "only what you already have" across origins
    if !req.online {
        return Err("offline".into());
    }
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_str(&req.url)).await.map_err(js)?.dyn_into().map_err(js)?;
    if !resp.ok() {
        return Err(format!("tile server answered {}", resp.status()));
    }
    let buf = JsFuture::from(resp.array_buffer().map_err(js)?).await.map_err(js)?;
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    if bytes.len() > 4 << 20 {
        return Err("tile too large".into());
    }
    lightcraft_engine::tiles::decode(&bytes)
}
