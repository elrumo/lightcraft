//! Development aid: `LIGHTCRAFT_SCRIPT=<file>` runs the control-protocol requests in it, one JSON
//! object per line (`{"method": …, "params": …}`, `docs/control-protocol.md`; `{"sleep": ms}`
//! waits), and appends each reply to `<file>.out`. This is how a Mac drives the app on a device
//! without touching it (`docs/ios.md`). The file lives in the app's own sandbox, so nothing is
//! exposed on the network.

use std::path::{Path, PathBuf};

use serde_json::json;

pub fn run_script(path: PathBuf, ctx: egui::Context) -> std::sync::mpsc::Receiver<lightcraft_ui_egui::ControlRequest> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("lc-script".into()).spawn(move || {
        use std::io::Write;
        let out_path = path.with_extension("out");
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).ok();
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            log::error!("script {}: {e}", path.display());
            String::new()
        });
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            let reply = if let Some(ms) = v.get("sleep").and_then(serde_json::Value::as_u64) {
                std::thread::sleep(std::time::Duration::from_millis(ms.min(600_000)));
                json!({"ok": true})
            } else if let Some(method) = v.get("method").and_then(serde_json::Value::as_str) {
                let mut params = v.get("params").cloned().unwrap_or(json!({}));
                // a relative `path` (a screenshot's) is next to the script: the app's tmp folder
                if let Some(rel) = params.get("path").and_then(serde_json::Value::as_str).filter(|p| Path::new(p).is_relative())
                    && let Some(dir) = path.parent()
                {
                    params["path"] = json!(dir.join(rel).to_string_lossy());
                }
                let (req, reply) = lightcraft_ui_egui::ControlRequest::new(method, params);
                if tx.send(req).is_err() {
                    break;
                }
                ctx.request_repaint();
                reply.recv_timeout(std::time::Duration::from_secs(120)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"}))
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
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_drives_the_app_and_keeps_the_replies() {
        let dir = std::env::temp_dir().join(format!("lc-ios-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tour.jsonl");
        std::fs::write(&script, "{\"sleep\": 1}\n\nnot json\n{\"method\": \"ui.screenshot\", \"params\": {\"path\": \"shot.png\"}}\n").unwrap();
        let rx = run_script(script, egui::Context::default());
        let req = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(req.method, "ui.screenshot");
        assert_eq!(req.params["path"], json!(dir.join("shot.png").to_string_lossy()), "relative to the script");
        req.reply.send(json!({"ok": true, "result": 7})).unwrap();
        let mut out = String::new();
        for _ in 0..200 {
            out = std::fs::read_to_string(dir.join("tour.out")).unwrap_or_default();
            if out.ends_with('\n') && out.lines().count() == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        assert!(lines[1].contains("not a request") && lines[2].contains("\"result\":7"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
