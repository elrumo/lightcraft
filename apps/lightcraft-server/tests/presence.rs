//! "Also open on": the server tells a user's devices which of the others show the same photo. Over real HTTP: the
//! route, the per-device rate limit, the admin's switch, and a crowd of devices that must not slow ordinary sync.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lightcraft_server::{Config, Server, accounts};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-presence-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

thread_local! {
    // (one connection kept per thread: a new one for each call would use up the machine's ports in the loops below)
    static AGENT: ureq::Agent = agent();
}

fn call(server: &Server, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let agent = AGENT.with(Clone::clone);
    let url = format!("http://{}{path}", server.addr());
    let auth = format!("Bearer {token}");
    let mut r = match method {
        "GET" => agent.get(&url).header("Authorization", &auth).call().unwrap(),
        "PUT" => agent.put(&url).header("Authorization", &auth).send(body.map(|b| b.to_string()).unwrap_or_default().as_bytes()).unwrap(),
        _ => agent.post(&url).header("Authorization", &auth).send(body.map(|b| b.to_string()).unwrap_or_default().as_bytes()).unwrap(),
    };
    (r.status().as_u16(), serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap_or(Value::Null))
}

fn start(tag: &str, users: &[&str]) -> (Server, PathBuf) {
    let root = temp(tag);
    let data = root.join("data");
    for u in users {
        accounts::set_user(&data, u, "correct horse", false).unwrap();
    }
    accounts::set_admin(&data, users[0], true).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    (Server::start(cfg).unwrap(), root)
}

fn device(server: &Server, user: &str, name: &str) -> String {
    let (status, v) = call(server, "POST", "/api/login", "", Some(json!({"user": user, "password": "correct horse", "device": name})));
    assert_eq!(status, 200, "{v}");
    v["token"].as_str().unwrap().to_string()
}

#[test]
fn devices_on_one_photo_see_each_other_and_an_admin_can_switch_it_off() {
    let (server, root) = start("pair", &["ann", "bob"]);
    let (phone, laptop) = (device(&server, "ann", "Ann's iPhone"), device(&server, "ann", "Ann's Mac"));
    let bob = device(&server, "bob", "Bob's iPhone");

    let (status, v) = call(&server, "POST", "/api/presence", &phone, Some(json!({"photo": "p7"})));
    assert_eq!(status, 200, "{v}");
    assert_eq!((v["others"].as_array().unwrap().len(), v["next"].clone()), (0, json!(8)), "{v}");
    assert!(v["head"].as_u64().is_some(), "the sync log's head rides along: {v}");
    // the Mac shows the same photo: it sees the phone, by name, and is asked to speak sooner
    let (_, v) = call(&server, "POST", "/api/presence", &laptop, Some(json!({"photo": "p7"})));
    assert_eq!(v["others"][0]["name"], "Ann's iPhone", "{v}");
    assert_eq!(v["next"], 2, "{v}");
    // someone else's device never shows up, whatever photo it shows
    let (_, v) = call(&server, "POST", "/api/presence", &bob, Some(json!({"photo": "p7"})));
    assert!(v["others"].as_array().unwrap().is_empty(), "{v}");

    // a device may speak once a second: faster is refused, and nothing it said changes
    let (status, _) = call(&server, "POST", "/api/presence", &phone, Some(json!({"photo": "p9"})));
    assert_eq!(status, 429);
    std::thread::sleep(Duration::from_millis(1100));
    let (status, v) = call(&server, "POST", "/api/presence", &laptop, Some(json!({"photo": "p7"})));
    assert_eq!((status, v["others"][0]["name"].clone()), (200, json!("Ann's iPhone")), "still on p7: {v}");

    // hostile bodies are answered, not crashed on
    for body in [json!({"photo": ""}), json!({"photo": "x".repeat(200)}), json!({"photo": 5}), json!([1])] {
        std::thread::sleep(Duration::from_millis(1100));
        let (status, _) = call(&server, "POST", "/api/presence", &laptop, Some(body));
        assert_eq!(status, 400);
    }
    assert_eq!(call(&server, "POST", "/api/presence", "wrong", Some(json!({"photo": "p1"}))).0, 401);

    // only an admin may switch it off; off, devices are told so and nothing is kept
    let admin = call(&server, "POST", "/api/admin/login", "", Some(json!({"user": "ann", "password": "correct horse"}))).1["token"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, s) = call(&server, "GET", "/api/admin/presence", &admin, None);
    assert_eq!((status, s["enabled"].clone()), (200, json!(true)), "{s}");
    assert_ne!(call(&server, "PUT", "/api/admin/presence", &phone, Some(json!({"enabled": false}))).0, 200, "a device is no admin");
    let (status, s) = call(&server, "PUT", "/api/admin/presence", &admin, Some(json!({"enabled": false})));
    assert_eq!((status, s["enabled"].clone(), s["entries"].clone()), (200, json!(false), json!(0)), "{s}");
    let (status, v) = call(&server, "POST", "/api/presence", &phone, Some(json!({"photo": "p7"})));
    assert_eq!((status, v["off"].clone(), v["next"].clone()), (200, json!(true), json!(60)), "{v}");
    assert_eq!(call(&server, "PUT", "/api/admin/presence", &admin, Some(json!({"enabled": "yes"}))).0, 400);
    let _ = std::fs::remove_dir_all(root);
}

/// Many devices saying their photo again and again: the table stays inside its bounds and an ordinary sync request
/// (pulling the log) is still answered quickly meanwhile.
#[test]
fn a_crowd_of_devices_does_not_slow_sync_or_grow_the_table() {
    let (server, root) = start("crowd", &["ann"]);
    let tokens: Vec<String> = (0..40).map(|i| device(&server, "ann", &format!("Device {i}"))).collect();
    let addr = server.addr().to_string();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // 40 devices, each hammering as fast as it likes (the server answers 429 to most)
    let handles: Vec<_> = tokens
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let (addr, stop) = (addr.clone(), stop.clone());
            std::thread::spawn(move || {
                let agent = agent();
                let mut answers = (0u32, 0u32, 0u32);
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let r = agent
                        .post(format!("http://{addr}/api/presence"))
                        .header("Authorization", format!("Bearer {t}"))
                        .send(json!({"photo": format!("p{}", i % 3)}).to_string().as_bytes());
                    // (the body is read away, or the connection can't be used again)
                    let r = r.map(|mut r| {
                        let _ = r.body_mut().read_to_vec();
                        r
                    });
                    match r.map(|r| r.status().as_u16()) {
                        Ok(200) => answers.0 += 1,
                        Ok(429) => answers.1 += 1,
                        _ => answers.2 += 1,
                    }
                    // (a device that doesn't wait at all; still not a storm of new connections)
                    std::thread::sleep(Duration::from_millis(20));
                }
                answers
            })
        })
        .collect();
    // meanwhile, ordinary sync: pull the log, many times, and time the slowest
    let ann = device(&server, "ann", "Sync device");
    let mut slowest = Duration::ZERO;
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(3) {
        let t = Instant::now();
        let (status, _) = call(&server, "GET", "/api/ops?since=0&limit=10", &ann, None);
        assert_eq!(status, 200);
        slowest = slowest.max(t.elapsed());
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let (mut ok, mut refused, mut other) = (0, 0, 0);
    for h in handles {
        let (a, b, c) = h.join().unwrap();
        (ok, refused, other) = (ok + a, refused + b, other + c);
    }
    eprintln!("presence under load: {ok} answered, {refused} told to wait, {other} otherwise; slowest sync pull {slowest:?}");
    // (16 devices are stored and answered about once a second each; the other 24 are sent away with 503, every time)
    assert!(ok > 0 && ok < 16 * 10 && refused > ok, "the limits bite: {ok} ok, {refused} told to wait, {other} sent away");
    assert!(other > 24, "the devices over the per-user cap were refused, not answered: {other}");
    assert!(slowest < Duration::from_secs(2), "sync stayed responsive: {slowest:?}");
    // 40 devices of one user: the per-user cap (16) held, the rest were told to back off, not stored
    let admin = call(&server, "POST", "/api/admin/login", "", Some(json!({"user": "ann", "password": "correct horse"}))).1["token"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, s) = call(&server, "GET", "/api/admin/presence", &admin, None);
    assert!(s["entries"].as_u64().unwrap() <= 16, "{s}");
    let _ = std::fs::remove_dir_all(root);
}
