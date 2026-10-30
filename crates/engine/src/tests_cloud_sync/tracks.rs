//! Recorded tracks and the tracking preferences through a sync server: the `tracks` document and the `tracking` item of
//! `prefs` (docs/sync.md, "Recorded tracks").

use super::*;

const T0: i64 = 1_777_629_600_000;

/// A walk of `n` points from `t0` (ms).
fn walk(n: usize, t0: i64) -> Value {
    Value::Array((0..n).map(|i| json!([t0 + i as i64 * 5000, 46.0 + i as f64 * 1e-4, 7.0, 1600.0 + i as f64])).collect())
}

fn record(s: &mut Session, t0: i64) -> String {
    s.execute("track.create", &json!({"points": walk(40, t0)})).unwrap()["session"].as_str().unwrap().to_string()
}

/// The sessions a device has, sorted, with their names.
fn tracks(s: &mut Session) -> Vec<(String, Option<String>)> {
    let list = s.execute("track.list", &json!({})).unwrap();
    let mut v: Vec<(String, Option<String>)> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["session"].as_str().unwrap().to_string(), t["name"].as_str().map(str::to_string)))
        .collect();
    v.sort();
    v
}

fn docs_puts(f: &Fake, name: &str) -> usize {
    f.requests.iter().filter(|r| *r == &format!("PUT /api/docs/{name}")).count()
}

#[test]
fn tracks_follow_the_user_to_their_other_devices() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("tracks", &mut f);
    let hike = record(&mut a, T0);
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert_eq!(tracks(&mut b), vec![(hike.clone(), None)], "B has A's track");
    let pa = a.execute("track.get", &json!({"session": hike, "points": true})).unwrap()["points"].clone();
    let pb = b.execute("track.get", &json!({"session": hike, "points": true})).unwrap()["points"].clone();
    assert_eq!(pa, pb, "point for point");
    // each adds one while apart: both are kept everywhere
    let on_b = record(&mut b, T0 + 86_400_000);
    let on_a = record(&mut a, T0 + 2 * 86_400_000);
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    let mut all = vec![hike.clone(), on_a.clone(), on_b.clone()];
    all.sort();
    for s in [&mut a, &mut b] {
        assert_eq!(tracks(s).into_iter().map(|t| t.0).collect::<Vec<_>>(), all);
    }
    // a rename on B reaches A
    b.execute("track.rename", &json!({"session": hike, "name": "Matterhorn"})).unwrap();
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    assert!(tracks(&mut a).contains(&(hike.clone(), Some("Matterhorn".into()))));
    // a deletion on A sticks
    a.execute("track.delete", &json!({"session": on_b})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    for s in [&mut a, &mut b] {
        assert!(tracks(s).iter().all(|t| t.0 != on_b), "deleted everywhere");
        assert_eq!(tracks(s).len(), 2);
    }
    // renamed on both while apart: one name, the same on both
    a.execute("track.rename", &json!({"session": on_a, "name": "From A"})).unwrap();
    b.execute("track.rename", &json!({"session": on_a, "name": "From B"})).unwrap();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    sync(&mut a, &mut f);
    assert_eq!(tracks(&mut a), tracks(&mut b));
    // and quiet when nothing changed
    let before = f.requests.len();
    sync(&mut a, &mut f);
    sync(&mut b, &mut f);
    assert!(f.requests[before..].iter().all(|r| !r.starts_with("PUT /api/docs/")), "{:?}", &f.requests[before..]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tracks_from_the_server_are_checked_item_by_item() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("tracks-hostile", &mut f);
    // a real track, as another device would have sent it
    let good = record(&mut a, T0);
    sync(&mut a, &mut f);
    let sent = f.docs["tracks"].items.as_array().unwrap().clone();
    a.execute("track.delete", &json!({"session": good})).unwrap();
    sync(&mut a, &mut f);
    let mut liar = sent[0].clone();
    liar["id"] = json!("s1111111111111111-0");
    liar["session"] = json!("s1111111111111111");
    liar["points"] = json!(41);
    let mut newer = sent[0].clone();
    newer["id"] = json!("s2222222222222222-0");
    newer["session"] = json!("s2222222222222222");
    newer["fmt"] = json!(9);
    newer["data"] = json!({"packed": "differently"});
    let mut items = vec![sent[0].clone(), liar, newer, json!("junk"), json!({"id": "x".repeat(5 << 20)}), sent[0].clone()];
    items.extend((0..3000).map(|i| json!({"id": format!("s{i:016x}-0"), "session": format!("s{i:016x}"), "index": 0, "fmt": 1, "data": "AAAA"})));
    let version = f.docs["tracks"].version;
    f.docs.insert("tracks".into(), proto::Doc { version: version + 1, items: Value::Array(items) });
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    let list = a.execute("track.list", &json!({})).unwrap();
    let mut kept: Vec<(String, bool)> =
        list["sessions"].as_array().unwrap().iter().map(|t| (t["session"].as_str().unwrap().to_string(), t["unreadable"] == true)).collect();
    let mut want = vec![(good.clone(), false), ("s2222222222222222".to_string(), true)];
    kept.sort();
    want.sort();
    assert_eq!(kept, want, "the real one and the newer one");
    // what this device sends back is what it kept
    let back = f.docs["tracks"].items.as_array().unwrap();
    assert_eq!(back.len(), 2, "{}", back.len());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_older_server_leaves_the_tracks_alone() {
    let mut f = Fake::new();
    f.unknown_docs = vec!["tracks"];
    let (mut a, mut b, root) = two_devices("tracks-old", &mut f);
    let t = record(&mut a, T0);
    a.execute("export.savePreset", &json!({"name": "Web", "params": {"format": "jpeg"}})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    sync(&mut b, &mut f);
    assert!(b.export_presets.iter().any(|p| p.name == "Web"), "the other documents sync as ever");
    assert!(tracks(&mut b).is_empty());
    assert_eq!(tracks(&mut a), vec![(t, None)], "kept on the device");
    let asked = f.requests.iter().filter(|r| r.contains("/api/docs/tracks")).count();
    record(&mut a, T0 + 86_400_000);
    sync(&mut a, &mut f);
    sync(&mut a, &mut f);
    assert_eq!(f.requests.iter().filter(|r| r.contains("/api/docs/tracks")).count(), asked, "not asked again this session");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_privacy_trim_and_prefs_items_of_a_newer_build_follow_the_user() {
    let mut f = Fake::new();
    let (mut a, mut b, root) = two_devices("tracking", &mut f);
    sync(&mut a, &mut f);
    let quiet = docs_puts(&f, "prefs");
    // a device that never chose sends no tracking item
    assert!(a.doc_items("prefs").iter().all(|v| v["id"] != "tracking"));
    a.execute("tracking.settings", &json!({"trim": true, "trimRadiusM": 500})).unwrap();
    sync(&mut a, &mut f);
    assert!(docs_puts(&f, "prefs") > quiet, "sent once set");
    sync(&mut b, &mut f);
    assert_eq!(b.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": true, "trimRadiusM": 500.0}));
    // an item a newer build added to `prefs` is kept and sent on by this one
    let mut doc = f.docs["prefs"].clone();
    doc.items.as_array_mut().unwrap().push(json!({"id": "future", "on": true}));
    doc.version += 1;
    f.docs.insert("prefs".into(), doc);
    sync(&mut b, &mut f);
    b.execute("tracking.settings", &json!({"trimRadiusM": 1000})).unwrap();
    sync(&mut b, &mut f);
    let items = f.docs["prefs"].items.as_array().unwrap().clone();
    assert!(items.iter().any(|v| v["id"] == "future"), "{items:?}");
    assert!(items.iter().any(|v| v["id"] == "tracking" && v["trimRadiusM"] == 1000.0), "{items:?}");
    sync(&mut a, &mut f);
    assert_eq!(a.execute("tracking.settings", &json!({})).unwrap()["trimRadiusM"], 1000.0);
    // kept across a reopen too
    b.close_library().unwrap();
    let dir = b.library.as_ref().unwrap().dir.clone();
    drop(b);
    let b = open(&dir);
    assert!(b.doc_items("prefs").iter().any(|v| v["id"] == "future"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn signing_out_takes_the_tracks_with_the_account() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("tracks-out", &mut f);
    record(&mut a, T0);
    a.execute("tracking.settings", &json!({"trim": true})).unwrap();
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    a.sync_sign_out_wait_with(10_000, &mut |t| f.handle(t));
    assert_eq!(f.docs["tracks"].items.as_array().unwrap().len(), 1, "sent before signing out");
    assert!(tracks(&mut a).is_empty(), "then gone from the library");
    assert_eq!(a.execute("tracking.settings", &json!({})).unwrap(), json!({"trim": false, "trimRadiusM": 300.0}));
    let _ = std::fs::remove_dir_all(&root);
}
