//! The admin page's sign-in end to end: a cookie the browser keeps, for weeks when asked and across a restart of the
//! server, and the guards around it (no script reads it, no other site uses it, a new password ends it).

use std::path::{Path, PathBuf};

use lightcraft_server::{Config, Server, accounts};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-admin-session-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(data: &Path) -> Server {
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = Some(1);
    Server::start(cfg).unwrap()
}

/// What a call to the admin API answered: status, JSON body and the `Set-Cookie` headers.
struct Answer {
    status: u16,
    body: Value,
    cookies: Vec<String>,
}

fn call(server: &Server, method: &str, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> Answer {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let url = format!("http://{}/api/admin/{path}", server.addr());
    let mut r = if method == "GET" {
        let mut req = agent.get(&url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.call().unwrap()
    } else if method == "PUT" {
        let mut req = agent.put(&url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.send(body.map(|b| b.to_string()).unwrap_or_default().as_bytes()).unwrap()
    } else {
        let mut req = agent.post(&url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.send(body.map(|b| b.to_string()).unwrap_or_default().as_bytes()).unwrap()
    };
    let cookies = r.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).map(str::to_string).collect();
    let status = r.status().as_u16();
    Answer { status, body: serde_json::from_slice(&r.body_mut().read_to_vec().unwrap()).unwrap_or(Value::Null), cookies }
}

/// The page's header on a request that changes something.
const PAGE: (&str, &str) = ("X-LightCraft-Admin", "1");

/// Sign in the way the page does: the answer holds a cookie, and no token.
fn sign_in(server: &Server, remember: bool, extra: &[(&str, &str)]) -> (Answer, String) {
    let a = call(server, "POST", "login", extra, Some(json!({"user": "ann", "password": "correct horse", "remember": remember, "cookie": true})));
    let cookie = a.cookies.first().and_then(|c| c.split(';').next()).unwrap_or_default().to_string();
    (a, cookie)
}

fn setup(tag: &str) -> (PathBuf, PathBuf) {
    let root = temp(tag);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    (root, data)
}

#[test]
fn a_kept_sign_in_is_a_cookie_that_survives_a_restart() {
    let (root, data) = setup("kept");
    let server = start(&data);
    let (a, cookie) = sign_in(&server, true, &[]);
    assert_eq!((a.status, a.body["user"].as_str()), (200, Some("ann")), "{:?}", a.body);
    assert!(a.body.get("token").is_none(), "the page never holds the session: {:?}", a.body);
    let set = &a.cookies[0];
    assert!(cookie.starts_with("lc_admin=") && cookie.len() > 20, "{set}");
    for want in ["HttpOnly", "SameSite=Strict", "Path=/api/admin", "Max-Age="] {
        assert!(set.contains(want), "{want} in {set}");
    }
    assert!(!set.contains("Secure"), "plain HTTP: {set}");
    let me = |s: &Server| call(s, "GET", "status", &[("Cookie", &cookie)], None);
    assert_eq!((me(&server).status, me(&server).body["me"].as_str()), (200, Some("ann")));
    drop(server);

    // the server restarts: the browser's cookie is still good
    let server = start(&data);
    assert_eq!(me(&server).status, 200, "kept across a restart");
    // ...and nothing but the hash of its token was written down
    let file = std::fs::read_to_string(data.join(accounts::ADMIN_SESSIONS)).unwrap();
    assert!(!file.contains(cookie.trim_start_matches("lc_admin=")), "{file}");

    // signing out ends it for good, and tells the browser to drop the cookie
    let out = call(&server, "POST", "logout", &[("Cookie", &cookie), PAGE], None);
    assert_eq!(out.status, 200);
    assert!(out.cookies.iter().any(|c| c.starts_with("lc_admin=;") && c.contains("Max-Age=0")), "{:?}", out.cookies);
    assert_eq!(me(&server).status, 401);
    drop(server);
    let server = start(&data);
    assert_eq!(me(&server).status, 401, "still signed out after a restart");
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_sign_in_that_is_not_kept_ends_with_the_browser_and_the_server() {
    let (root, data) = setup("not-kept");
    let server = start(&data);
    let (a, cookie) = sign_in(&server, false, &[]);
    assert_eq!(a.status, 200);
    assert!(!a.cookies[0].contains("Max-Age"), "a cookie that ends with the browser: {}", a.cookies[0]);
    assert_eq!(call(&server, "GET", "status", &[("Cookie", &cookie)], None).status, 200);
    drop(server);
    let server = start(&data);
    let gone = call(&server, "GET", "status", &[("Cookie", &cookie)], None);
    assert_eq!(gone.status, 401, "not kept: it ends with the server");
    assert!(gone.cookies.iter().any(|c| c.starts_with("lc_admin=;")), "the browser is told to drop it: {:?}", gone.cookies);
    // (no cookie at all is just a 401: nothing to drop)
    assert!(call(&server, "GET", "status", &[], None).cookies.is_empty());
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_cookie_changes_things_only_for_requests_from_the_page() {
    let (root, data) = setup("csrf");
    let server = start(&data);
    let (_, cookie) = sign_in(&server, true, &[]);
    let gc = |headers: &[(&str, &str)]| call(&server, "POST", "gc", headers, Some(json!({"dryRun": true}))).status;
    // a form on another site can send the cookie but not the page's header
    assert_eq!(gc(&[("Cookie", &cookie)]), 403);
    assert_eq!(gc(&[("Cookie", &cookie), ("X-LightCraft-Admin", "1")]), 200);
    // reading needs no header
    assert_eq!(call(&server, "GET", "users", &[("Cookie", &cookie)], None).status, 200);
    // a bearer token (the SwiftUI app, scripts) is not a cookie: it is not sent by itself, so it needs no header
    let token =
        call(&server, "POST", "login", &[], Some(json!({"user": "ann", "password": "correct horse"}))).body["token"].as_str().unwrap().to_string();
    let bearer = format!("Bearer {token}");
    assert_eq!(gc(&[("Authorization", &bearer)]), 200);
    // a made-up cookie is no session
    assert_eq!(call(&server, "GET", "status", &[("Cookie", "lc_admin=0123456789abcdef")], None).status, 401);
    assert_eq!(call(&server, "GET", "status", &[("Cookie", "other=1; lc_admin=")], None).status, 401);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_cookie_is_secure_behind_an_https_proxy() {
    let (root, data) = setup("https");
    let server = start(&data);
    let (a, _) = sign_in(&server, true, &[("X-Forwarded-Proto", "https")]);
    assert!(a.cookies[0].contains("; Secure"), "{}", a.cookies[0]);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_new_password_or_losing_admin_ends_the_signed_in_browsers() {
    let (root, data) = setup("password");
    let server = start(&data);
    let (_, here) = sign_in(&server, true, &[]);
    let (_, other) = sign_in(&server, true, &[]);
    let status = |c: &str| call(&server, "GET", "status", &[("Cookie", c)], None).status;
    assert_eq!((status(&here), status(&other)), (200, 200));
    // the admin changes their own password on the page: that browser stays, the other one is signed out
    let r = call(&server, "PUT", "users/ann/password", &[("Cookie", &here), PAGE], Some(json!({"password": "battery staple"})));
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!((status(&here), status(&other)), (200, 401));
    // a new password set from the command line (the server isn't told) signs every browser out
    accounts::set_user(&data, "ann", "another passphrase", true).unwrap();
    assert_eq!(status(&here), 401);
    drop(server);
    let server = start(&data);
    assert_eq!(call(&server, "GET", "status", &[("Cookie", &here)], None).status, 401, "and it stays so after a restart");
    // a user who is no longer an admin is out
    let login = call(&server, "POST", "login", &[], Some(json!({"user": "ann", "password": "another passphrase", "remember": true, "cookie": true})));
    let cookie = login.cookies.first().and_then(|c| c.split(';').next()).unwrap_or_default().to_string();
    assert_eq!(call(&server, "GET", "status", &[("Cookie", &cookie)], None).status, 200);
    accounts::set_user(&data, "bob", "correct horse", false).unwrap();
    accounts::set_admin(&data, "bob", true).unwrap();
    accounts::set_admin(&data, "ann", false).unwrap();
    assert_eq!(call(&server, "GET", "status", &[("Cookie", &cookie)], None).status, 401);
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
