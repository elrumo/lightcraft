//! The anonymous heartbeat: a server told where to send it posts its install id, version and coarse counts, and no names.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use lightcraft_server::{Config, Server, accounts};
use serde_json::Value;

/// A site that takes one POST and sends the body on.
fn site() -> (String, mpsc::Receiver<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/api/telemetry/server", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut socket, _)) = listener.accept() else { return };
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        let (head, length) = loop {
            let n = socket.read(&mut chunk).unwrap_or(0);
            raw.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&raw).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                    .unwrap_or(0);
                break (text[..end].to_string(), length);
            }
            if n == 0 {
                return;
            }
        };
        let start = head.len() + 4;
        while raw.len() < start + length {
            let n = socket.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
        }
        let _ =
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}\n");
        let body = serde_json::from_slice(raw.get(start..).unwrap_or_default()).unwrap_or(Value::Null);
        let _ = tx.send((head, body));
    });
    (url, rx)
}

#[test]
fn a_server_sends_a_heartbeat_with_its_install_id_and_coarse_counts_but_no_names() {
    let root = std::env::temp_dir().join(format!("lc-server-telemetry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    accounts::set_user(&data, "secret-name", "correct horse", false).unwrap();
    let (url, rx) = site();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    cfg.telemetry_url = Some(url);
    cfg.telemetry_first = Some(Duration::from_millis(100));
    let server = Server::start(cfg).unwrap();

    let (head, body) = rx.recv_timeout(Duration::from_secs(60)).expect("a heartbeat arrives");
    assert!(head.starts_with("POST /api/telemetry/server"), "{head}");
    let id = body["installId"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(data.join(".lightbites-telemetry-id")).unwrap(), id, "the id is kept in the data folder");
    assert_eq!(body["env"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["env"]["os"], std::env::consts::OS);
    assert_eq!(body["library"]["users"], 1);
    assert_eq!(body["library"]["photos"], "0");
    assert!(body["uptime"].as_u64().is_some());
    assert!(!body.to_string().contains("secret-name") && !body.to_string().contains(root.to_str().unwrap()), "{body}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
