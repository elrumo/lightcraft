//! Development aid: `LIGHTCRAFT_SCRIPT=<file>` runs the control-protocol requests in it, one JSON
//! object per line (`{"method": …, "params": …}`, `docs/control-protocol.md`; `{"sleep": ms}`
//! waits), and appends each reply to `<file>.out`. This is how a Mac drives the app on a device
//! without touching it. The file lives in the app's own sandbox, so nothing is exposed on the
//! network.
//!
//! Requests the engine answers itself (`engine.*`, `render.*`, `host.*`) are answered on its
//! thread; `ui.*` ones go to the frontend as `uiRequest` events.

use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};

use serde_json::{Value, json};

use super::{Msg, Request, SCRIPT_TIMEOUT};

/// Run the script at `path` on a thread of its own, sending its requests to the engine on `tx`.
pub(super) fn run(path: PathBuf, tx: Sender<Msg>) {
    let spawned = std::thread::Builder::new().name("lc-script".into()).spawn(move || {
        use std::io::Write;
        let out_path = path.with_extension("out");
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).ok();
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            log::error!("script {}: {e}", path.display());
            String::new()
        });
        for (n, line) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let v: Value = serde_json::from_str(line).unwrap_or_default();
            let reply = if let Some(ms) = v.get("sleep").and_then(Value::as_u64) {
                std::thread::sleep(std::time::Duration::from_millis(ms.min(600_000)));
                json!({"ok": true})
            } else if let Some(method) = v.get("method").and_then(Value::as_str) {
                let mut params = v.get("params").cloned().unwrap_or_else(|| json!({}));
                // a relative `path` (a screenshot's) is next to the script: the app's tmp folder
                if let Some(rel) = params.get("path").and_then(Value::as_str) {
                    params["path"] = json!(super::beside(&path, rel));
                }
                let (reply_tx, reply_rx) = channel();
                let req = Request { id: n as u64 + 1, method: method.to_string(), params };
                if tx.send(Msg::Call(req, reply_tx)).is_err() {
                    break;
                }
                match reply_rx.recv_timeout(SCRIPT_TIMEOUT) {
                    Ok(super::Event::Reply { result: Some(r), .. }) => json!({"ok": true, "result": r}),
                    Ok(super::Event::Reply { error: Some(e), .. }) => json!({"ok": false, "error": e.message, "code": e.code}),
                    Ok(_) => json!({"ok": true}),
                    Err(_) => json!({"ok": false, "error": "timeout"}),
                }
            } else {
                json!({"ok": false, "error": "not a request"})
            };
            if let Some(f) = out.as_mut() {
                // one write per line, so a reader never sees half a reply
                let _ = f.write_all(format!("{}\n", json!({"request": v, "reply": reply})).as_bytes());
            }
        }
        log::info!("script {} done", path.display());
    });
    if let Err(e) = spawned {
        log::error!("script: {e}");
    }
}
