//! The admin page's Overview: what each device asks of the server is counted per device, and the machine is sampled.

use std::path::PathBuf;

use lightcraft_server::{Config, Server, accounts};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-usage-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn call(server: &Server, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let url = format!("http://{}{path}", server.addr());
    let auth = format!("Bearer {token}");
    let mut r = if method == "GET" {
        agent.get(&url).header("Authorization", &auth).call().unwrap()
    } else {
        agent.post(&url).header("Authorization", &auth).send(body.map(|b| b.to_string()).unwrap_or_default().as_bytes()).unwrap()
    };
    (r.status().as_u16(), serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap_or(Value::Null))
}

#[test]
fn the_admin_sees_what_each_device_asked_and_the_machine_sampled() {
    let root = temp("devices");
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::set_user(&data, "bob", "another horse", false).unwrap();
    let mut cfg = Config::new(&data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    let server = Server::start(cfg).unwrap();
    let device = |user: &str, password: &str, name: &str| {
        let (status, v) = call(&server, "POST", "/api/login", "", Some(json!({"user": user, "password": password, "device": name})));
        assert_eq!(status, 200, "{v}");
        v["token"].as_str().unwrap().to_string()
    };
    let (phone, laptop) = (device("ann", "correct horse", "Phone"), device("ann", "correct horse", "Laptop"));
    for _ in 0..3 {
        assert_eq!(call(&server, "GET", "/api/me", &phone, None).0, 200);
    }
    assert_eq!(call(&server, "GET", "/api/nothing-here", &phone, None).0, 404, "a refused request is a request too");
    assert_eq!(call(&server, "GET", "/api/me", &laptop, None).0, 200);
    // (a request that doesn't sign in with a device isn't a device's)
    assert_eq!(call(&server, "GET", "/api/me", "wrong", None).0, 401);

    let admin = call(&server, "POST", "/api/admin/login", "", Some(json!({"user": "ann", "password": "correct horse"}))).1["token"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, u) = call(&server, "GET", "/api/admin/usage", &admin, None);
    assert_eq!(status, 200, "{u}");
    let users = u["users"].as_array().unwrap();
    let find = |user: &str| users.iter().find(|x| x["name"] == user).unwrap_or_else(|| panic!("{user} in {u}"));
    let ann = find("ann")["devices"].as_array().unwrap();
    let by = |name: &str| ann.iter().find(|d| d["name"] == name).unwrap_or_else(|| panic!("{name} in {u}"));
    assert_eq!(by("Phone")["requests"], 4, "{u}");
    assert_eq!(by("Laptop")["requests"], 1, "{u}");
    assert!(by("Phone")["out"].as_u64().unwrap() > 0, "bytes sent: {u}");
    assert!(by("Phone")["lastSeen"].as_u64().is_some(), "{u}");
    assert!(find("bob")["devices"].as_array().unwrap().is_empty(), "a user with no device: {u}");
    // the admin page's own requests are no device's
    assert!(users.iter().flat_map(|x| x["devices"].as_array().unwrap()).map(|d| d["requests"].as_u64().unwrap()).sum::<u64>() == 5, "{u}");

    // the machine: sampled when the server started, with what the platform lets it read
    assert_eq!(u["supported"], cfg!(target_os = "linux"), "{u}");
    assert!(u["cores"].as_u64().unwrap() >= 1 && u["since"].as_u64().unwrap() > 1_700_000_000, "{u}");
    let history = u["history"].as_array().unwrap();
    assert!(!history.is_empty(), "the first sample is taken at the start: {u}");
    if cfg!(target_os = "linux") {
        assert!(history[0]["rss"].as_u64().unwrap() > 0 && history[0]["memUsed"].as_u64().unwrap() > 0, "{u}");
    } else {
        assert!(history[0]["rss"].is_null() && history[0]["cpu"].is_null(), "{u}");
    }
    // only an admin may look
    assert_eq!(call(&server, "GET", "/api/admin/usage", &phone, None).0, 401);

    // a device signed out stops being listed
    let (_, devices) = call(&server, "GET", "/api/admin/users/ann/devices", &admin, None);
    assert_eq!(devices.as_array().unwrap().len(), 2);
    assert_eq!(call(&server, "POST", "/api/logout", &laptop, None).0, 200);
    let u = call(&server, "GET", "/api/admin/usage", &admin, None).1;
    let names: Vec<&str> = find_in(&u, "ann");
    assert_eq!(names, ["Phone"], "{u}");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

fn find_in<'a>(u: &'a Value, user: &str) -> Vec<&'a str> {
    let x = u["users"].as_array().unwrap().iter().find(|x| x["name"] == user).unwrap();
    x["devices"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect()
}
