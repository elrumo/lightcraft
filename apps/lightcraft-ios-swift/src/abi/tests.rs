use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;

/// The JSON of an answer, released after reading (as the Swift side does).
fn read(buf: LcBuf) -> (i32, Value) {
    assert!(!buf.ptr.is_null() && buf.token != 0, "{buf:?}");
    // SAFETY: a buffer returned by the library is valid until released, which happens below.
    let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) }.to_vec();
    lc_release(buf.token);
    (buf.status, serde_json::from_slice(&bytes).unwrap())
}

fn open_demo() -> u64 {
    let tmp =
        std::env::temp_dir().join(format!("lc-bridge-{}-{}", std::process::id(), crate::BRIDGE.next.load(std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir_all(&tmp).unwrap();
    let config = serde_json::to_vec(&json!({"demo": true, "tmp": tmp.to_string_lossy(), "renderThreads": 2})).unwrap();
    // SAFETY: `config` outlives the call.
    let (status, opened) = read(unsafe { lc_open(config.as_ptr(), config.len()) });
    assert_eq!(status, status::OK, "{opened}");
    assert_eq!(opened["demo"], true);
    opened["engine"].as_u64().unwrap()
}

fn call(engine: u64, id: u64, method: &str, params: Value) -> Value {
    let req = serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap();
    // SAFETY: `req` outlives the call.
    let (status, reply) = read(unsafe { lc_call(engine, req.as_ptr(), req.len(), 30_000) });
    assert_eq!(status, status::OK);
    reply
}

#[test]
fn the_version_is_the_headers() {
    assert_eq!(lc_abi_version(), 1);
}

// The Swift side's whole round trip: open, ask, poll for an image, read its pixels, release, close
#[test]
fn a_frontend_opens_asks_and_reads_pictures() {
    let engine = open_demo();
    let page = call(engine, 1, "engine.execute", json!({"command": "catalog.query", "params": {"limit": 1}}));
    let photo = page["result"]["photos"][0]["id"].as_u64().unwrap();
    let req = serde_json::to_vec(&json!({"id": 2, "method": "render.thumb", "params": {"photo": photo, "edge": 96}})).unwrap();
    // SAFETY: `req` outlives the call.
    assert_eq!(unsafe { lc_submit(engine, req.as_ptr(), req.len()) }, status::OK);
    let t0 = Instant::now();
    let image = 'found: loop {
        let (status, events) = read(lc_poll(engine, 100));
        assert_eq!(status, status::OK);
        for e in events.as_array().unwrap() {
            if e["kind"] == "image" && e["slot"] == format!("thumb:{photo}") {
                break 'found e.clone();
            }
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "no thumbnail");
    };
    let token = image["token"].as_u64().unwrap();
    let px = lc_image(token);
    assert_eq!(px.status, status::OK);
    assert_eq!((px.width as u64, px.height as u64), (image["width"].as_u64().unwrap(), image["height"].as_u64().unwrap()));
    assert_eq!(px.stride, px.width * 4);
    // SAFETY: the picture stays until its token is released, below.
    let pixels = unsafe { std::slice::from_raw_parts(px.pixels, px.len) };
    assert!(pixels.chunks(4).any(|p| p[3] == 255));
    lc_release(token);
    lc_release(token);
    assert_eq!(lc_image(token).status, status::GONE);
    lc_close(engine);
    lc_close(engine);
}

// Wrong input is an answer, never a crash: null and oversized buffers, stale handles, not-JSON
#[test]
fn bad_input_and_stale_handles_are_errors() {
    // SAFETY: null pointers are allowed by the contract (and refused).
    let (status, err) = read(unsafe { lc_open(std::ptr::null(), 10) });
    assert_eq!((status, err["code"].as_str()), (status::BAD_INPUT, Some("badInput")));
    let big = vec![b' '; MAX_INPUT + 1];
    // SAFETY: `big` outlives the call.
    let (status, _) = read(unsafe { lc_open(big.as_ptr(), big.len()) });
    assert_eq!(status, status::BAD_INPUT);
    let junk = b"{\"demo\": \"yes\"}";
    // SAFETY: `junk` outlives the call.
    let (status, err) = read(unsafe { lc_open(junk.as_ptr(), junk.len()) });
    assert_eq!(status, status::BAD_INPUT, "{err}");
    let (status, err) = read(lc_poll(987_654, 0));
    assert_eq!((status, err["code"].as_str()), (status::BAD_HANDLE, Some("badHandle")));
    // SAFETY: null pointers are allowed by the contract (and refused).
    assert_eq!(unsafe { lc_submit(987_654, std::ptr::null(), 0) }, status::BAD_INPUT);
    let req = b"{}";
    // SAFETY: `req` outlives the call.
    assert_eq!(unsafe { lc_submit(987_654, req.as_ptr(), req.len()) }, status::BAD_HANDLE);
    lc_wake(987_654);
    lc_close(987_654);
    lc_release(0);
    let engine = open_demo();
    let not_json = b"hello";
    // SAFETY: `not_json` outlives the call.
    assert_eq!(unsafe { lc_submit(engine, not_json.as_ptr(), not_json.len()) }, status::BAD_INPUT);
    lc_close(engine);
    // SAFETY: `req` outlives the call.
    assert_eq!(unsafe { lc_submit(engine, req.as_ptr(), req.len()) }, status::BAD_HANDLE, "closed: the handle is gone");
}

// A thread blocked in lc_poll comes back when woken
#[test]
fn wake_releases_a_waiting_poll() {
    let engine = open_demo();
    // let the start-up events go first
    let _ = read(lc_poll(engine, 200));
    let waiter = std::thread::spawn(move || {
        let t0 = Instant::now();
        let _ = read(lc_poll(engine, 20_000));
        t0.elapsed()
    });
    std::thread::sleep(Duration::from_millis(100));
    lc_wake(engine);
    let waited = waiter.join().unwrap();
    assert!(waited < Duration::from_secs(10), "{waited:?}");
    lc_close(engine);
}

/// A function declaration: (return type, name, [(parameter type, name)]), in C's words.
type Decl = (String, String, Vec<(String, String)>);

/// The C declarations of a header's functions.
fn header_functions(h: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    for line in h.lines().map(str::trim).filter(|l| !l.starts_with('#') && !l.starts_with("/*") && !l.starts_with('*') && l.ends_with(");")) {
        let Some((head, params)) = line.trim_end_matches(");").split_once('(') else { continue };
        let Some((ret, name)) = head.rsplit_once(' ') else { continue };
        if !name.starts_with("lc_") {
            continue;
        }
        let params = params
            .split(',')
            .map(str::trim)
            .filter(|p| *p != "void" && !p.is_empty())
            .map(|p| {
                let split = p.rfind(['*', ' ']).unwrap();
                (p[..=split].trim().to_string(), p[split + 1..].to_string())
            })
            .collect();
        out.push((ret.trim().to_string(), name.to_string(), params));
    }
    out
}

/// The same from the Rust source, with Rust types written as C ones.
fn rust_functions(src: &str) -> Vec<Decl> {
    let c = |t: &str| match t.trim() {
        "u32" => "uint32_t".to_string(),
        "u64" => "uint64_t".to_string(),
        "i32" => "int32_t".to_string(),
        "usize" => "size_t".to_string(),
        "*const u8" => "const uint8_t *".to_string(),
        other => other.to_string(),
    };
    let mut out = Vec::new();
    for line in src.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("pub extern \"C\" fn ").or_else(|| line.strip_prefix("pub unsafe extern \"C\" fn ")) else { continue };
        let Some((name, rest)) = rest.split_once('(') else { continue };
        let Some((params, rest)) = rest.split_once(')') else { continue };
        let ret = rest.trim().strip_prefix("->").map_or("void".to_string(), |r| c(r.trim_end_matches('{')));
        let params = params
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (n, t) = p.split_once(':').unwrap();
                (c(t), n.trim().to_string())
            })
            .collect();
        out.push((ret, name.to_string(), params));
    }
    out
}

/// A struct's fields in the header, as (C type, name).
fn header_struct(h: &str, name: &str) -> Vec<(String, String)> {
    let start = h.find(&format!("typedef struct {name} {{")).unwrap();
    let body = &h[start..h[start..].find('}').unwrap() + start];
    body.lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| l.ends_with(';'))
        .map(|l| {
            let l = l.trim_end_matches(';');
            let split = l.rfind(['*', ' ']).unwrap();
            (l[..=split].trim().to_string(), l[split + 1..].to_string())
        })
        .collect()
}

// The header Swift imports says exactly what this library exports (`docs/ios-swift.md`)
#[test]
fn header_matches_exports() {
    let header = include_str!("../../LightCraft/Sources/CLightCraftBridge/include/lightcraft_bridge.h");
    let source = include_str!("../abi.rs");
    let mut h = header_functions(header);
    let mut r = rust_functions(source);
    h.sort_by(|a, b| a.1.cmp(&b.1));
    r.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(h.len(), 9, "{h:?}");
    assert_eq!(h, r);
    assert!(header.contains(&format!("#define LC_ABI_VERSION {}u", crate::ABI_VERSION)));
    for (name, value) in [
        ("OK", status::OK),
        ("ERROR", status::ERROR),
        ("BAD_HANDLE", status::BAD_HANDLE),
        ("BAD_INPUT", status::BAD_INPUT),
        ("PANIC", status::PANIC),
        ("CLOSED", status::CLOSED),
        ("GONE", status::GONE),
    ] {
        assert!(header.contains(&format!("#define LC_{name} {value}\n")), "LC_{name}");
    }
    assert_eq!(
        header_struct(header, "LcBuf"),
        [("const uint8_t *", "ptr"), ("size_t", "len"), ("uint64_t", "token"), ("int32_t", "status")].map(|(t, n)| (t.to_string(), n.to_string()))
    );
    assert_eq!(
        header_struct(header, "LcImage"),
        [
            ("const uint8_t *", "pixels"),
            ("size_t", "len"),
            ("uint32_t", "width"),
            ("uint32_t", "height"),
            ("uint32_t", "stride"),
            ("int32_t", "status")
        ]
        .map(|(t, n)| (t.to_string(), n.to_string()))
    );
    // the layout the header's _Static_asserts promise (64-bit targets: every iOS device and Mac)
    assert_eq!(std::mem::size_of::<LcBuf>(), 32);
    assert_eq!(std::mem::size_of::<LcImage>(), 32);
    assert_eq!(
        [std::mem::offset_of!(LcBuf, ptr), std::mem::offset_of!(LcBuf, len), std::mem::offset_of!(LcBuf, token), std::mem::offset_of!(LcBuf, status)],
        [0, 8, 16, 24]
    );
    assert_eq!(
        [
            std::mem::offset_of!(LcImage, pixels),
            std::mem::offset_of!(LcImage, len),
            std::mem::offset_of!(LcImage, width),
            std::mem::offset_of!(LcImage, height),
            std::mem::offset_of!(LcImage, stride),
            std::mem::offset_of!(LcImage, status)
        ],
        [0, 8, 16, 20, 24, 28]
    );
    assert!(header.contains("_Static_assert(sizeof(LcBuf) == 32") && header.contains("_Static_assert(sizeof(LcImage) == 32"));
}
