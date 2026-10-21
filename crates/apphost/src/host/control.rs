//! A live control channel for a native frontend (`LIGHTCRAFT_CONTROL_PORT=<port>`): the egui app's
//! `--control` protocol (`docs/control-protocol.md`) on `127.0.0.1`, one JSON request per line in,
//! one reply line out (`{"id", "ok", "result" | "error", "code"?}`), so `lightcraft-cli mcp --connect`
//! and plain `nc` drive the SwiftUI app as they drive the egui one.
//!
//! It is the socket twin of [`super::script`]: requests go to the engine thread as `Msg::Call`, which
//! answers `engine.*`, `render.*` and `host.*` itself and forwards `ui.*` to the frontend as
//! `uiRequest`s. Loopback only, off unless the port is asked for, and a first line that is not a
//! request (an HTTP verb from a web page) closes the connection before anything runs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};

use serde_json::{Value, json};

use super::{Event, Msg, Request, SCRIPT_TIMEOUT};

/// Longest accepted request line (bytes, without the newline).
const MAX_LINE: usize = 4 * 1024 * 1024;
/// Connections served at once; further ones get an error line and are closed.
const MAX_CONNECTIONS: usize = 16;

/// Listen on `127.0.0.1:port` (`0`: any free port) and serve connections on threads of their own.
/// Returns the address bound.
pub(super) fn start(port: u16, tx: Sender<Msg>) -> Result<SocketAddr, String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("control channel: 127.0.0.1:{port}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| format!("control channel: {e}"))?;
    std::thread::Builder::new().name("lc-control".into()).spawn(move || accept_loop(&listener, &tx)).map_err(|e| format!("control channel: {e}"))?;
    log::info!("control channel listening on {addr}");
    Ok(addr)
}

fn accept_loop(listener: &TcpListener, tx: &Sender<Msg>) {
    let open = Arc::new(AtomicUsize::new(0));
    let ids = Arc::new(AtomicU64::new(1 << 32)); // apart from a script's and the frontend's own ids
    for mut stream in listener.incoming().flatten() {
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            let _ = writeln!(stream, "{}", json!({"ok": false, "error": "too many control connections"}));
            continue;
        }
        let (tx, ids, done) = (tx.clone(), Arc::clone(&ids), Arc::clone(&open));
        let spawned = std::thread::Builder::new().name("lc-control-conn".into()).spawn(move || {
            serve(&stream, &tx, &ids);
            done.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            open.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Serve one connection until the peer closes, sends something that is not a request, or the
/// engine has stopped.
fn serve(stream: &TcpStream, tx: &Sender<Msg>, ids: &AtomicU64) {
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    loop {
        let reject = match read_line(&mut reader, &mut buf) {
            Ok(false) => return,
            Err(e) => e,
            Ok(true) => match parse(&buf) {
                Ok(None) => continue,
                Err(e) => e,
                Ok(Some((id, method, params))) => {
                    let (reply_tx, reply_rx) = channel();
                    let req = Request { id: ids.fetch_add(1, Ordering::SeqCst), method, params };
                    if tx.send(Msg::Call(req, reply_tx)).is_err() {
                        return;
                    }
                    let mut reply = match reply_rx.recv_timeout(SCRIPT_TIMEOUT) {
                        Ok(event) => reply_json(event),
                        Err(_) => json!({"ok": false, "error": "timeout", "code": "timeout"}),
                    };
                    reply["id"] = id;
                    if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
                        return;
                    }
                    continue;
                }
            },
        };
        let _ = writeln!(out, "{}", json!({"ok": false, "error": reject})).and_then(|()| out.flush());
        return;
    }
}

/// The control protocol's reply object for the engine's answer.
fn reply_json(event: Event) -> Value {
    match event {
        Event::Reply { result: Some(r), .. } => json!({"ok": true, "result": r}),
        Event::Reply { error: Some(e), .. } => json!({"ok": false, "error": e.message, "code": e.code}),
        _ => json!({"ok": true}),
    }
}

/// `(id, method, params)` of a request line, `None` for a blank one, or why the line is refused.
fn parse(bytes: &[u8]) -> Result<Option<(Value, String, Value)>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not a control request (invalid UTF-8); closing the connection")?.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let msg: Value =
        serde_json::from_str(text).map_err(|e| format!("bad JSON ({e}); expected one JSON request object per line; closing the connection"))?;
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .ok_or("not a control request (expected an object with a string `method`); closing the connection")?;
    Ok(Some((msg.get("id").cloned().unwrap_or(Value::Null), method.to_string(), msg.get("params").cloned().unwrap_or_else(|| json!({})))))
}

/// Read one `\n`-terminated line of at most [`MAX_LINE`] bytes into `buf` (newline stripped).
/// `Ok(false)` at end of stream; `Err` when the line is too long or the read fails.
fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> Result<bool, String> {
    buf.clear();
    // `take` bounds what one line may buffer: a peer cannot make us allocate more
    let n = reader.take(u64::try_from(MAX_LINE).unwrap_or(u64::MAX).saturating_add(1)).read_until(b'\n', buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    } else if buf.len() > MAX_LINE {
        return Err(format!("request line longer than {MAX_LINE} bytes; closing the connection"));
    }
    Ok(true)
}

/// The port `LIGHTCRAFT_CONTROL_PORT` names, if it is set to a number.
pub(super) fn port_from_env() -> Option<u16> {
    let v = std::env::var("LIGHTCRAFT_CONTROL_PORT").ok()?;
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    match v.parse() {
        Ok(p) => Some(p),
        Err(_) => {
            log::error!("LIGHTCRAFT_CONTROL_PORT={v:?} is not a port number; the control channel stays off");
            None
        }
    }
}
