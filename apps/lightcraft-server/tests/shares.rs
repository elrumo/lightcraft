//! Album links end to end: a device syncs an album to a real server, makes a link, and a browser with no
//! account (here: plain HTTP) sees it.

use std::path::{Path, PathBuf};

use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::{Selection, Session};
use lightcraft_server::{Config, Server, accounts};
use serde_json::{Value, json};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-server-shares-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const W: usize = 80;
const H: usize = 60;

/// A gradient photo; `seed` is its blue.
fn pixel(i: usize, seed: u8) -> [u8; 4] {
    [(i % W * 3) as u8, (i / W * 4) as u8, seed, 255]
}

fn write_png(path: &Path, seed: u8) {
    let data: Vec<[u8; 4]> = (0..W * H).map(|i| pixel(i, seed)).collect();
    let img = lightcraft_raster::Rgba8 { width: W, height: H, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn call(method: &str, url: &str, token: &str, body: Option<&[u8]>, headers: &[(&str, &str)]) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into();
    let auth = format!("Bearer {token}");
    let mut r = match method {
        "GET" => {
            let mut req = a.get(url);
            if !token.is_empty() {
                req = req.header("Authorization", &auth);
            }
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.call()
        }
        "DELETE" => a.delete(url).header("Authorization", &auth).call(),
        _ => a.post(url).header("Authorization", &auth).send(body.unwrap_or(&[])),
    }
    .unwrap();
    let hs = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let status = r.status().as_u16();
    (status, r.body_mut().read_to_vec().unwrap_or_default(), hs)
}

fn header<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

fn json_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

/// A server with ann (and bob), and ann's device with three photos synced: a, b in the album Trip, c in none.
struct Fixture {
    root: PathBuf,
    server: Option<Server>,
    base: String,
    device: Session,
    token: String,
    photos: [PhotoId; 3],
    trip: u64,
}

fn fixture(tag: &str) -> Fixture {
    let root = temp(tag);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    accounts::set_admin(&data, "ann", true).unwrap();
    accounts::set_user(&data, "bob", "battery staple", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    let server = Server::start(cfg).unwrap();
    let base = format!("http://{}", server.addr());
    for (name, seed) in [("a.png", 10), ("b.png", 90), ("c.png", 170)] {
        write_png(&root.join("in").join(name), seed);
    }
    let mut device = Session::new().with_fs();
    device.open_library(root.join("device"), false).unwrap();
    device.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let id_of = |s: &Session, n: &str| s.catalog.photos().find(|p| p.file_name == n).map(|p| p.id).unwrap();
    let photos = [id_of(&device, "a.png"), id_of(&device, "b.png"), id_of(&device, "c.png")];
    device.selection = Selection { ids: vec![photos[0], photos[1]], active: Some(photos[0]) };
    let trip = device.execute("album.create", &json!({"name": "Trip", "addSelected": true})).unwrap()["id"].as_u64().unwrap();
    device.execute("sync.signIn", &json!({"server": base, "user": "ann", "password": "correct horse", "device": "test"})).unwrap();
    let st = device.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(st["state"], "idle", "{st}");
    let token = device.sync_state().unwrap().config.token.clone();
    Fixture { root, server: Some(server), base, device, token, photos, trip }
}

impl Fixture {
    /// Stop the server and start it again on the same data (a new port: the device keeps its old address).
    fn restart(&mut self) {
        self.server.take();
        let mut cfg = Config::new(self.root.join("data"), "127.0.0.1:0");
        cfg.scan_interval = None;
        cfg.preview_threads = 1;
        let server = Server::start(cfg).unwrap();
        self.base = format!("http://{}", server.addr());
        self.server = Some(server);
    }

    fn share(&self, body: Value) -> (u16, Value) {
        let (s, b, _) = call("POST", &format!("{}/api/shares", self.base), &self.token, Some(body.to_string().as_bytes()), &[]);
        (s, json_of(&b))
    }
    fn page(&self, path: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
        call("GET", &format!("{}{path}", self.base), "", None, &[])
    }
}

/// Mean of the three colour channels of a JPEG.
fn brightness(jpeg: &[u8]) -> f64 {
    let img = lightcraft_preview::decode_jpeg(jpeg).expect("a JPEG");
    let sum: u64 = img.data.iter().map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])).sum();
    sum as f64 / (img.data.len() * 3) as f64
}

#[test]
fn an_album_link_shows_the_album_to_anyone_with_the_pictures_as_they_are_edited() {
    let mut fx = fixture("page");
    let [a, b, c] = fx.photos;
    let (s, share) = fx.share(json!({"album": fx.trip}));
    assert_eq!(s, 200, "{share}");
    let token = share["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 64, "256 random bits");
    assert_eq!((share["name"].as_str(), share["originals"].as_bool(), share["expires"].clone()), (Some("Trip"), Some(false), Value::Null));

    // no account: the album's page
    let (s, body, hs) = fx.page(&format!("/s/{token}"));
    let html = String::from_utf8(body).unwrap();
    assert_eq!(s, 200, "{html}");
    assert!(header(&hs, "content-type").is_some_and(|t| t.starts_with("text/html")));
    assert!(html.contains("<h1>Trip</h1>") && html.contains("2 photos"), "{html}");
    for id in [a, b] {
        assert!(html.contains(&format!("/s/{token}/{}/thumb", id.0)), "{html}");
    }
    assert!(!html.contains(&format!("/{}/thumb", c.0)), "c isn't in the album");
    assert!(!html.contains("/original"), "no originals on this link");
    // what the page says about itself
    assert!(header(&hs, "x-robots-tag").is_some_and(|v| v.contains("noindex")));
    assert!(header(&hs, "content-security-policy").is_some_and(|v| v.contains("default-src 'none'") && v.contains("frame-ancestors 'none'")));
    assert_eq!(header(&hs, "referrer-policy"), Some("no-referrer"));

    // the pictures: JPEG, no larger than asked, rendered from the photo's edits as they are now
    let (s, thumb, hs) = fx.page(&format!("/s/{token}/{}/thumb", a.0));
    assert_eq!((s, header(&hs, "content-type")), (200, Some("image/jpeg")));
    let before = brightness(&thumb);
    let (s, view, _) = fx.page(&format!("/s/{token}/{}/view", a.0));
    assert_eq!(s, 200);
    assert!(lightcraft_preview::decode_jpeg(&view).is_some());
    fx.device.selection = Selection::single(a);
    fx.device.execute("develop.set", &json!({"control": "light.exposure", "value": 1.5})).unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    let (_, thumb2, _) = fx.page(&format!("/s/{token}/{}/thumb", a.0));
    assert!(brightness(&thumb2) > before * 1.2, "the edit shows: {before} -> {}", brightness(&thumb2));

    // only the album's photos, only the pictures a link offers
    assert_eq!(fx.page(&format!("/s/{token}/{}/thumb", c.0)).0, 404, "a photo of another album");
    assert_eq!(fx.page(&format!("/s/{token}/999999/thumb")).0, 404);
    assert_eq!(fx.page(&format!("/s/{token}/{}/original", a.0)).0, 404, "originals weren't offered");
    assert_eq!(fx.page(&format!("/s/{token}/{}/huge", a.0)).0, 404);
    assert_eq!(fx.page(&format!("/s/{token}/not-a-number/thumb")).0, 404);
    assert_eq!(fx.page(&format!("/s/{token}/{}/thumb/extra", a.0)).0, 404);
    assert_eq!(fx.page(&format!("/s/{token}/..%2f..%2fapi%2fme")).0, 404);
    // a link is read-only
    let (s, ..) = call("POST", &format!("{}/s/{token}", fx.base), "", Some(b"{}"), &[]);
    assert_eq!(s, 405);
    // the link's owner sees it listed, others don't
    let (_, list, _) = call("GET", &format!("{}/api/shares", fx.base), &fx.token, None, &[]);
    assert_eq!(json_of(&list)["shares"].as_array().map(Vec::len), Some(1));
    let bob = {
        let body = json!({"user": "bob", "password": "battery staple", "device": "t"}).to_string();
        json_of(&call("POST", &format!("{}/api/login", fx.base), "", Some(body.as_bytes()), &[]).1)["token"].as_str().unwrap().to_string()
    };
    let (_, list, _) = call("GET", &format!("{}/api/shares", fx.base), &bob, None, &[]);
    assert_eq!(json_of(&list)["shares"], json!([]));
    assert_eq!(
        call("DELETE", &format!("{}/api/shares/{}", fx.base, share["id"].as_str().unwrap()), &bob, None, &[]).0,
        404,
        "only the owner revokes"
    );
    assert_eq!(call("GET", &format!("{}/api/shares", fx.base), "", None, &[]).0, 401);
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn links_can_end_be_taken_back_and_only_cover_albums_of_photos() {
    let mut fx = fixture("ends");
    // not an album of photos / not an album at all
    let folder = fx.device.execute("album.create", &json!({"name": "Folder", "folder": true})).unwrap()["id"].as_u64().unwrap();
    let smart = fx.device.execute("album.createSmart", &json!({"name": "Picks", "rating": 1})).unwrap()["id"].as_u64().unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(fx.share(json!({"album": folder})).0, 400);
    assert_eq!(fx.share(json!({"album": smart})).0, 400, "smart albums aren't shared");
    assert_eq!(fx.share(json!({"album": 987654321})).0, 404);
    assert_eq!(fx.share(json!({"nope": 1})).0, 404, "no album 0");
    // a link that ends
    let (s, share) = fx.share(json!({"album": fx.trip, "expiresDays": 3}));
    assert_eq!(s, 200);
    let (made, ends) = (share["created"].as_u64().unwrap(), share["expires"].as_u64().unwrap());
    assert_eq!(ends - made, 3 * 86_400);
    let token = share["token"].as_str().unwrap();
    assert_eq!(fx.page(&format!("/s/{token}")).0, 200);
    // taken back: gone at once, for everyone
    let id = share["id"].as_str().unwrap();
    assert_eq!(call("DELETE", &format!("{}/api/shares/{id}", fx.base), &fx.token, None, &[]).0, 200);
    assert_eq!(fx.page(&format!("/s/{token}")).0, 404);
    assert_eq!(call("DELETE", &format!("{}/api/shares/{id}", fx.base), &fx.token, None, &[]).0, 404);
    // an album that is deleted takes its link's page with it
    let other = fx.device.execute("album.create", &json!({"name": "Other"})).unwrap()["id"].as_u64().unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    let (s, doomed) = fx.share(json!({"album": other}));
    assert_eq!(s, 200, "{doomed}");
    let doomed = doomed["token"].as_str().unwrap().to_string();
    assert_eq!(fx.page(&format!("/s/{doomed}")).0, 200);
    fx.device.execute("album.delete", &json!({"id": other})).unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(fx.page(&format!("/s/{doomed}")).0, 404);
    // links survive a restart of the server (the one that ended with its album still doesn't work)
    let (_, share) = fx.share(json!({"album": fx.trip}));
    let token = share["token"].as_str().unwrap().to_string();
    fx.restart();
    assert_eq!(fx.page(&format!("/s/{token}")).0, 200);
    assert_eq!(fx.page(&format!("/s/{doomed}")).0, 404);
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn originals_come_only_when_the_link_allows_them_and_resume() {
    let mut fx = fixture("originals");
    let [a, ..] = fx.photos;
    let (_, share) = fx.share(json!({"album": fx.trip, "originals": true}));
    let token = share["token"].as_str().unwrap();
    assert_eq!(share["originals"], true);
    let (_, body, _) = fx.page(&format!("/s/{token}"));
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains(&format!("/s/{token}/{}/original", a.0)), "{html}");
    let bytes = std::fs::read(fx.root.join("in/a.png")).unwrap();
    let (s, got, hs) = fx.page(&format!("/s/{token}/{}/original", a.0));
    assert_eq!((s, got == bytes), (200, true));
    assert!(header(&hs, "content-disposition").is_some_and(|d| d.starts_with("attachment; filename=\"a.png\"")), "{hs:?}");
    let (s, part, hs) = call("GET", &format!("{}/s/{token}/{}/original", fx.base, a.0), "", None, &[("Range", "bytes=4-11")]);
    assert_eq!((s, part.as_slice()), (206, &bytes[4..12]));
    assert!(header(&hs, "content-range").is_some_and(|r| r.starts_with("bytes 4-11/")));
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn guessing_links_is_throttled_and_removed_users_lose_theirs() {
    let mut fx = fixture("guess");
    fx.share(json!({"album": fx.trip}));
    // the admin page can list and revoke a user's links
    let login = json_of(
        &call(
            "POST",
            &format!("{}/api/admin/login", fx.base),
            "",
            Some(json!({"user": "ann", "password": "correct horse"}).to_string().as_bytes()),
            &[],
        )
        .1,
    );
    let admin = login["token"].as_str().unwrap().to_string();
    let (s, list, _) = call("GET", &format!("{}/api/admin/users/ann/shares", fx.base), &admin, None, &[]);
    assert_eq!((s, json_of(&list).as_array().map(Vec::len)), (200, Some(1)));
    let (_, users, _) = call("GET", &format!("{}/api/admin/users", fx.base), &admin, None, &[]);
    assert_eq!(json_of(&users).as_array().unwrap().iter().find(|u| u["name"] == "ann").map(|u| u["shares"].clone()), Some(json!(1)));
    let (_, other, _) = call("POST", &format!("{}/api/shares", fx.base), &fx.token, Some(json!({"album": fx.trip}).to_string().as_bytes()), &[]);
    let id = json_of(&other)["id"].as_str().unwrap().to_string();
    assert_eq!(call("DELETE", &format!("{}/api/admin/users/ann/shares/{id}", fx.base), &admin, None, &[]).0, 200);
    assert_eq!(call("GET", &format!("{}/api/admin/users/ann/shares", fx.base), &admin, None, &[]).0, 200);
    assert_eq!(call("GET", &format!("{}/api/admin/users/ann/shares", fx.base), &fx.token, None, &[]).0, 401, "a device token isn't an admin session");
    // wrong tokens: not found, and after a few the address has to wait
    let mut statuses = Vec::new();
    for i in 0..9 {
        statuses.push(fx.page(&format!("/s/{}{i}", "0".repeat(63))).0);
    }
    assert_eq!(statuses[0], 404);
    assert!(statuses.contains(&429), "{statuses:?}");
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn a_device_makes_lists_and_takes_back_links_with_its_commands() {
    let mut fx = fixture("device");
    // the commands can't make a link for what can't be shared, or take back what isn't a link
    let folder = fx.device.execute("album.create", &json!({"name": "Folder", "folder": true})).unwrap()["id"].as_u64().unwrap();
    assert!(fx.device.execute("album.share", &json!({"id": folder})).is_err());
    assert!(fx.device.execute("album.share", &json!({"id": 424242})).is_err());
    assert!(fx.device.execute("album.unshare", &json!({"id": "../x"})).is_err());
    // make one: the request goes out with the next sync, and the address is handed over once
    fx.device.execute("album.share", &json!({"id": fx.trip, "expiresDays": 7, "originals": true})).unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    let url = fx.device.sync_take_new_link().expect("the link just made");
    assert!(url.starts_with(&format!("{}/s/", fx.base)), "{url}");
    assert_eq!(fx.device.sync_take_new_link(), None, "handed over once");
    assert_eq!(call("GET", &url, "", None, &[]).0, 200, "it opens without an account");
    // listed (asked of the server, then kept)
    fx.device.execute("shares.list", &json!({})).unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    let list = fx.device.execute("shares.list", &json!({})).unwrap();
    let links = list["shares"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{list}");
    assert_eq!(
        (links[0]["url"].as_str(), links[0]["name"].as_str(), links[0]["originals"].as_bool()),
        (Some(url.as_str()), Some("Trip"), Some(true))
    );
    assert!(links[0]["expires"].as_u64().is_some());
    // taken back
    let id = links[0]["id"].as_str().unwrap().to_string();
    fx.device.execute("album.unshare", &json!({"id": id})).unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    assert_eq!(call("GET", &url, "", None, &[]).0, 404);
    assert_eq!(fx.device.execute("shares.list", &json!({})).unwrap()["shares"], json!([]));
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

/// Any request: method, URL, headers, body → (status, body, headers).
fn send(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let a: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into();
    let r = match method {
        "GET" => {
            let mut req = a.get(url);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.call()
        }
        "PUT" => {
            let mut req = a.put(url);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.send(body.unwrap_or(&[]))
        }
        _ => {
            let mut req = a.post(url);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            req.send(body.unwrap_or(&[]))
        }
    };
    let mut r = r.unwrap();
    let hs = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let status = r.status().as_u16();
    (status, r.body_mut().read_to_vec().unwrap_or_default(), hs)
}

fn text(b: Vec<u8>) -> String {
    String::from_utf8(b).unwrap()
}

#[test]
fn a_link_with_a_password_asks_for_it_and_then_opens_with_a_cookie() {
    let mut fx = fixture("password");
    let [a, ..] = fx.photos;
    assert_eq!(fx.share(json!({"album": fx.trip, "password": "abc"})).0, 400, "too short");
    let (s, share) = fx.share(json!({"album": fx.trip, "password": "open sesame"}));
    assert_eq!(s, 200, "{share}");
    assert_eq!(share["password"], true);
    assert!(!share.to_string().contains("argon2"), "the hash stays on the server: {share}");
    let token = share["token"].as_str().unwrap().to_string();
    // locked: a form, not the album (not even its name), and none of its pictures
    let (s, body, hs) = fx.page(&format!("/s/{token}"));
    let html = text(body);
    assert_eq!(s, 401, "{html}");
    assert!(html.contains(&format!("action=\"/s/{token}/unlock\"")) && html.contains("type=\"password\""), "{html}");
    assert!(!html.contains("Trip") && !html.contains("/thumb"), "{html}");
    assert!(header(&hs, "content-security-policy").is_some_and(|c| c.contains("form-action 'self'")));
    assert_eq!(fx.page(&format!("/s/{token}/{}/thumb", a.0)).0, 401);
    assert_eq!(fx.page(&format!("/s/{token}/{}", a.0)).0, 401);
    let unlock = format!("{}/s/{token}/unlock", fx.base);
    let form = [("Content-Type", "application/x-www-form-urlencoded")];
    // a wrong password: the form again, saying so
    let (s, body, hs) = send("POST", &unlock, &form, Some(b"password=open+says+me"));
    assert_eq!(s, 401);
    assert!(text(body).contains("isn't the password") && header(&hs, "set-cookie").is_none());
    // the right one: a cookie for this link, and back to the album
    let (s, _, hs) = send("POST", &unlock, &form, Some(b"password=open+sesame"));
    assert_eq!(s, 303);
    assert_eq!(header(&hs, "location"), Some(format!("/s/{token}").as_str()));
    let set = header(&hs, "set-cookie").expect("a cookie").to_string();
    assert!(set.contains(&format!("Path=/s/{token}")) && set.contains("HttpOnly") && set.contains("SameSite=Lax"), "{set}");
    assert!(!set.contains("Secure"), "plain HTTP here");
    let cookie = set.split(';').next().unwrap().to_string();
    let with = [("Cookie", cookie.as_str())];
    let (s, body, _) = send("GET", &format!("{}/s/{token}", fx.base), &with, None);
    let html = text(body);
    assert_eq!(s, 200, "{html}");
    assert!(html.contains("<h1>Trip</h1>") && !html.contains("og:image"), "no link preview for a link with a password: {html}");
    assert_eq!(send("GET", &format!("{}/s/{token}/{}/thumb", fx.base, a.0), &with, None).0, 200);
    // behind HTTPS (a proxy says so) the cookie is Secure
    let (_, _, hs) = send("POST", &unlock, &[form[0], ("X-Forwarded-Proto", "https")], Some(b"password=open+sesame"));
    assert!(header(&hs, "set-cookie").is_some_and(|c| c.contains("; Secure")));
    // the cookie opens this link only: another link with the same password asks again
    let (_, other) = fx.share(json!({"album": fx.trip, "password": "open sesame"}));
    let other = other["token"].as_str().unwrap();
    assert_eq!(send("GET", &format!("{}/s/{other}", fx.base), &with, None).0, 401);
    // made from a device's command, listed with its password flag
    fx.device.execute("album.share", &json!({"id": fx.trip, "password": "from the app", "upload": true})).unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    let url = fx.device.sync_take_new_link().unwrap();
    assert_eq!(call("GET", &url, "", None, &[]).0, 401);
    fx.device.execute("shares.list", &json!({"refresh": true})).unwrap();
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    let list = fx.device.execute("shares.list", &json!({})).unwrap();
    let mine = list["shares"].as_array().unwrap().iter().find(|l| l["url"] == url.as_str()).cloned().unwrap();
    assert_eq!((mine["password"].as_bool(), mine["upload"].as_bool()), (Some(true), Some(true)), "{list}");
    assert!(fx.device.execute("album.share", &json!({"id": fx.trip, "password": "abc"})).is_err(), "too short, said at once");
    // guessing passwords is throttled
    let statuses: Vec<u16> = (0..9).map(|_| send("POST", &format!("{}/s/{other}/unlock", fx.base), &form, Some(b"password=nope")).0).collect();
    assert!(statuses.contains(&429), "{statuses:?}");
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn an_album_page_goes_by_day_with_a_page_per_photo_a_slideshow_and_a_link_preview() {
    let mut fx = fixture("pages");
    let [a, b, c] = fx.photos;
    let (_, share) = fx.share(json!({"album": fx.trip}));
    let token = share["token"].as_str().unwrap().to_string();
    let (s, body, _) =
        send("GET", &format!("{}/s/{token}", fx.base), &[("X-Forwarded-Proto", "https"), ("X-Forwarded-Host", "photos.example.com")], None);
    let html = text(body);
    assert_eq!(s, 200);
    assert!(html.contains("<section class=\"day\"><h2>"), "photos by day: {html}");
    assert!(html.contains(&format!("href=\"/s/{token}/{}\"", a.0)), "a page per photo: {html}");
    assert!(html.contains("?play=1\"") && html.contains("Slideshow"), "{html}");
    assert!(!html.contains("/download") && !html.contains("action="), "no originals, no uploads on this link");
    // what a chat app shows for the link: the album and its first picture, at the address the visitor used
    assert!(html.contains("<meta property=\"og:title\" content=\"Trip\">"), "{html}");
    assert!(html.contains(&format!("content=\"https://photos.example.com/s/{token}/")), "{html}");
    // the photo pages: one after the other, ← → by script (allowed by its hash only)
    let order: Vec<u64> = [a, b].iter().map(|p| p.0).collect();
    let (s, body, hs) = fx.page(&format!("/s/{token}/{}", order[0]));
    let first = text(body);
    assert_eq!(s, 200, "{first}");
    assert!(first.contains(&format!("src=\"/s/{token}/{}/view\"", order[0])) && first.contains("1 of 2"), "{first}");
    assert!(first.contains(&format!("id=\"next\" rel=\"next\" href=\"/s/{token}/{}\"", order[1])) && !first.contains("id=\"prev\""), "{first}");
    assert!(first.contains("<script>"));
    let csp = header(&hs, "content-security-policy").unwrap();
    assert!(csp.contains("script-src 'sha256-") && !csp.contains("script-src 'unsafe-inline'"), "{csp}");
    let (_, body, _) = fx.page(&format!("/s/{token}/{}", order[1]));
    let second = text(body);
    assert!(second.contains(&format!("id=\"prev\" rel=\"prev\" href=\"/s/{token}/{}\"", order[0])) && !second.contains("id=\"next\""), "{second}");
    // the slideshow moves on by itself, and goes round
    let (_, body, _) = fx.page(&format!("/s/{token}/{}?play=1", order[1]));
    let playing = text(body);
    assert!(playing.contains(&format!("http-equiv=\"refresh\" content=\"5;url=/s/{token}/{}?play=1\"", order[0])), "{playing}");
    assert!(playing.contains("Pause"));
    // only the album's photos have pages
    assert_eq!(fx.page(&format!("/s/{token}/{}", c.0)).0, 404);
    assert_eq!(fx.page(&format!("/s/{token}/123456789")).0, 404);
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn download_all_is_a_zip_of_the_originals_on_a_link_that_offers_them() {
    let mut fx = fixture("zip");
    let (_, plain) = fx.share(json!({"album": fx.trip}));
    assert_eq!(fx.page(&format!("/s/{}/download", plain["token"].as_str().unwrap())).0, 404, "not without the originals");
    let (_, share) = fx.share(json!({"album": fx.trip, "originals": true}));
    let token = share["token"].as_str().unwrap().to_string();
    assert!(text(fx.page(&format!("/s/{token}")).1).contains(&format!("href=\"/s/{token}/download\"")));
    let (s, zip, hs) = fx.page(&format!("/s/{token}/download"));
    assert_eq!((s, header(&hs, "content-type")), (200, Some("application/zip")));
    assert_eq!(header(&hs, "content-length").and_then(|l| l.parse::<usize>().ok()), Some(zip.len()), "its length is known before it is sent");
    assert!(header(&hs, "content-disposition").is_some_and(|d| d.contains("Trip.zip")), "{hs:?}");
    assert!(zip.starts_with(b"PK\x03\x04"));
    // stored as they are: each original is in it whole, under its name
    for name in ["a.png", "b.png"] {
        let bytes = std::fs::read(fx.root.join("in").join(name)).unwrap();
        assert!(zip.windows(bytes.len()).any(|w| w == bytes.as_slice()), "{name}");
        assert!(zip.windows(name.len()).filter(|w| *w == name.as_bytes()).count() >= 2, "{name}: local and central headers");
    }
    assert!(!zip.windows(5).any(|w| w == b"c.png"), "only the album's photos");
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

/// A `multipart/form-data` body: (field, file name, bytes).
fn multipart(boundary: &str, parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut b = Vec::new();
    for (name, file, data) in parts {
        b.extend(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some(f) => b.extend(
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n")
                    .as_bytes(),
            ),
            None => b.extend(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes()),
        }
        b.extend_from_slice(data);
        b.extend(b"\r\n");
    }
    b.extend(format!("--{boundary}--\r\n").as_bytes());
    b
}

#[test]
fn visitors_add_photos_to_an_album_whose_link_takes_them() {
    let mut fx = fixture("guests");
    let [a, _, c] = fx.photos;
    let boundary = "----lcTestBoundary7MA4YWxkTrZu0gW";
    let ctype = format!("multipart/form-data; boundary={boundary}");
    write_png(&fx.root.join("guest").join("d.png"), 230);
    let d = std::fs::read(fx.root.join("guest/d.png")).unwrap();
    let (a_bytes, c_bytes) = (std::fs::read(fx.root.join("in/a.png")).unwrap(), std::fs::read(fx.root.join("in/c.png")).unwrap());
    let body = multipart(
        boundary,
        &[
            ("name", None, b"Bob"),
            ("photos", Some("d.png"), &d),
            ("photos", Some("C:\\fakepath\\copy of c.png"), &c_bytes),
            ("photos", Some("a.png"), &a_bytes),
            ("photos", Some("notes.txt"), b"not a photo at all"),
        ],
    );
    // a link that doesn't take photos refuses them, and shows no form
    let (_, plain) = fx.share(json!({"album": fx.trip}));
    let plain = plain["token"].as_str().unwrap().to_string();
    assert_eq!(send("POST", &format!("{}/s/{plain}/upload", fx.base), &[("Content-Type", &ctype)], Some(&body)).0, 403);
    let (_, share) = fx.share(json!({"album": fx.trip, "upload": true}));
    assert_eq!(share["upload"], true);
    let token = share["token"].as_str().unwrap().to_string();
    let page = text(fx.page(&format!("/s/{token}")).1);
    assert!(page.contains(&format!("action=\"/s/{token}/upload\"")) && page.contains("enctype=\"multipart/form-data\""), "{page}");
    // not a form
    assert_eq!(send("POST", &format!("{}/s/{token}/upload", fx.base), &[("Content-Type", "text/plain")], Some(b"hi")).0, 400);
    // d is new, c is in the library (not in the album), a is in the album already, the text isn't a photo
    let (s, _, hs) = send("POST", &format!("{}/s/{token}/upload", fx.base), &[("Content-Type", &ctype)], Some(&body));
    assert_eq!(s, 303);
    assert_eq!(header(&hs, "location"), Some(format!("/s/{token}?added=2&skipped=1&already=1").as_str()));
    let page = text(fx.page(&format!("/s/{token}?added=2&skipped=1&already=1")).1);
    assert!(page.contains("4 photos") && page.contains("2 photos were added") && page.contains("1 file was left out"), "{page}");
    assert!(page.contains("1 photo was in the album already"), "{page}");
    // the owner's devices get them, in the album
    fx.device.execute("sync.now", &json!({"wait": true})).unwrap();
    let cat = &fx.device.catalog;
    let new = cat.photos().find(|p| p.file_name == "d.png").expect("the visitor's photo");
    assert_eq!((new.meta.creator.as_str(), new.width, new.height), ("Bob", W as u32, H as u32));
    let trip = cat.album(lightcraft_engine::catalog::AlbumId(fx.trip)).unwrap();
    assert!(trip.photos.contains(&new.id) && trip.photos.contains(&c) && trip.photos.contains(&a), "{:?}", trip.photos);
    assert_eq!(trip.photos.len(), 4, "a wasn't put in twice");
    assert_eq!(cat.photos().filter(|p| p.file_name.ends_with("c.png")).count(), 1, "the copy of c is c");
    // its original is on the server by its content, and its previews are built there
    let hash = lightcraft_preview::hash_bytes(&d).to_string();
    assert_eq!(call("GET", &format!("{}/api/blobs/original/{hash}", fx.base), &fx.token, None, &[]).1, d);
    let mut built = false;
    for _ in 0..200 {
        if call("GET", &format!("{}/api/blobs/mini/{hash}", fx.base), &fx.token, None, &[]).0 == 200 {
            built = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(built, "the server builds the previews of what visitors add");
    // and the visitors see it on the page, as everyone else's
    assert_eq!(fx.page(&format!("/s/{token}/{}/thumb", new.id.0)).0, 200);
    // an empty form adds nothing
    let empty = multipart(boundary, &[("photos", Some(""), b"")]);
    let (s, _, hs) = send("POST", &format!("{}/s/{token}/upload", fx.base), &[("Content-Type", &ctype)], Some(&empty));
    assert_eq!((s, header(&hs, "location")), (303, Some(format!("/s/{token}?skipped=0").as_str())));
    // a form that breaks off adds nothing either
    let cut = &body[..body.len() / 2];
    assert_eq!(send("POST", &format!("{}/s/{token}/upload", fx.base), &[("Content-Type", &ctype)], Some(cut)).0, 400);
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}

#[test]
fn a_storage_limit_stops_uploads_from_devices_and_visitors_and_devices_are_told() {
    let mut fx = fixture("quota");
    let data = fx.root.join("data");
    let admin = json_of(
        &call(
            "POST",
            &format!("{}/api/admin/login", fx.base),
            "",
            Some(json!({"user": "ann", "password": "correct horse"}).to_string().as_bytes()),
            &[],
        )
        .1,
    )["token"]
        .as_str()
        .unwrap()
        .to_string();
    let quota = |v: Value| {
        send(
            "PUT",
            &format!("{}/api/admin/users/ann/quota", fx.base),
            &[("Authorization", &format!("Bearer {admin}"))],
            Some(json!({"quota": v}).to_string().as_bytes()),
        )
    };
    assert_eq!(quota(json!(1000)).0, 400, "too small to hold anything");
    assert_eq!(quota(json!(20_000_000)).0, 200);
    assert_eq!(
        send(
            "PUT",
            &format!("{}/api/admin/users/nobody/quota", fx.base),
            &[("Authorization", &format!("Bearer {admin}"))],
            Some(b"{\"quota\":null}")
        )
        .0,
        404
    );
    let users = json_of(&call("GET", &format!("{}/api/admin/users", fx.base), &admin, None, &[]).1);
    assert_eq!(users.as_array().unwrap().iter().find(|u| u["name"] == "ann").map(|u| u["quota"].clone()), Some(json!(20_000_000)));
    assert_eq!(json_of(&call("GET", &format!("{}/api/usage", fx.base), &fx.token, None, &[]).1)["quota"], json!(20_000_000), "devices are told");
    // (below what ann keeps already: set in the file, as an admin who sets a limit under the use would)
    let mut f: Value = serde_json::from_slice(&std::fs::read(data.join("users.json")).unwrap()).unwrap();
    f["users"]["ann"]["quota"] = json!(1000);
    std::fs::write(data.join("users.json"), serde_json::to_vec(&f).unwrap()).unwrap();
    // a device's upload is refused, saying why
    let preview = b"LCSP1\nsomething";
    let (s, body, _) = send(
        "PUT",
        &format!("{}/api/blobs/mini/0123456789abcdef0123456789abcdef", fx.base),
        &[("Authorization", &format!("Bearer {}", fx.token))],
        Some(preview),
    );
    assert_eq!(s, 507);
    assert!(json_of(&body)["error"].as_str().is_some_and(|e| e.contains("storage limit")), "{}", String::from_utf8_lossy(&body));
    // and so is a visitor's, before anything is kept
    let (_, share) = fx.share(json!({"album": fx.trip, "upload": true}));
    let token = share["token"].as_str().unwrap();
    let boundary = "b0undary";
    write_png(&fx.root.join("guest/e.png"), 33);
    let e = std::fs::read(fx.root.join("guest/e.png")).unwrap();
    let body = multipart(boundary, &[("photos", Some("e.png"), &e)]);
    let (s, page, _) = send(
        "POST",
        &format!("{}/s/{token}/upload", fx.base),
        &[("Content-Type", &format!("multipart/form-data; boundary={boundary}"))],
        Some(&body),
    );
    assert_eq!(s, 507);
    assert!(text(page).contains("no room left"));
    // no limit again: uploads go through
    assert_eq!(quota(Value::Null).0, 200);
    let (s, ..) = send(
        "POST",
        &format!("{}/s/{token}/upload", fx.base),
        &[("Content-Type", &format!("multipart/form-data; boundary={boundary}"))],
        Some(&body),
    );
    assert_eq!(s, 303);
    fx.server.take();
    let _ = std::fs::remove_dir_all(&fx.root);
}
