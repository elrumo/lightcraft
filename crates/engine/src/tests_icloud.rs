//! iCloud sync ([`crate::icloud`]): devices that share presets, settings, a sync server and photo-library
//! edits through a store that orders writes the way CloudKit does (a save over a version the device hasn't
//! seen is refused, and the store's version comes back as fetched).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::icloud::{Incoming, Sent};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-icloud-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(p: &Path, seed: u8) {
    let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
    let b =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
}

/// One record in the store, with the tag a save must name to replace it.
#[derive(Clone, Debug)]
struct Stored {
    kind: String,
    key: String,
    data: String,
    modified: i64,
    tag: u64,
}

/// A private database in memory: records, and a change log devices fetch from.
#[derive(Default)]
struct Cloud {
    records: BTreeMap<String, Stored>,
    /// (seq, name, deleted)
    log: Vec<(u64, String, bool)>,
    seq: u64,
}

impl Cloud {
    fn change(&mut self, name: &str, deleted: bool) {
        self.seq += 1;
        self.log.push((self.seq, name.to_string(), deleted));
    }
}

/// A device: its session (with a library), its clock, and what its transport knows of the store.
struct Device {
    s: Session,
    root: PathBuf,
    clock: Arc<AtomicI64>,
    /// The store's change log read up to here.
    cursor: u64,
    /// The tag of each record as last seen (what CloudKit's cached system fields carry).
    tags: BTreeMap<String, u64>,
    /// The engine's epoch this transport started from.
    epoch: Option<u64>,
}

impl Device {
    fn new(root: &Path, name: &str) -> Device {
        let root = root.join(name);
        let mut s = Session::new().with_fs();
        s.open_library(root.join("lib"), false).unwrap();
        s.set_photo_library_staging(Some(root.join("staging")));
        s.execute("icloud.enable", &json!({"on": true})).unwrap();
        let clock = Arc::new(AtomicI64::new(1_000));
        let c = clock.clone();
        s.icloud_set_clock(Box::new(move || c.load(Ordering::SeqCst)));
        let epoch = s.icloud_epoch();
        Device { s, root, clock, cursor: 0, tags: BTreeMap::new(), epoch }
    }

    fn at(&self, ms: i64) {
        self.clock.store(ms, Ordering::SeqCst);
    }

    fn incoming(name: &str, r: &Stored) -> Incoming {
        Incoming { name: name.to_string(), kind: r.kind.clone(), key: r.key.clone(), data: r.data.clone(), modified: r.modified }
    }

    /// What CKSyncEngine does in one round: fetch what changed, then send what changed here (a save over a
    /// version this device hasn't seen is refused: the store's version is handed over as fetched, and sent
    /// again if the device still has a change).
    fn sync(&mut self, cloud: &mut Cloud) {
        // the engine forgot what the cloud has (the host learns it from `icloud.changes`): start over, as
        // CKSyncEngine does without its saved state
        let _ = self.s.icloud_changes();
        if self.s.icloud_epoch() != self.epoch {
            (self.cursor, self.tags, self.epoch) = (0, BTreeMap::new(), self.s.icloud_epoch());
        }
        // fetch
        let mut records = Vec::new();
        let mut deleted = Vec::new();
        let from = self.cursor;
        for (seq, name, gone) in cloud.log.iter().filter(|(seq, _, _)| *seq > from) {
            self.cursor = *seq;
            match (gone, cloud.records.get(name)) {
                (false, Some(r)) => {
                    self.tags.insert(name.clone(), r.tag);
                    records.push(Self::incoming(name, r));
                }
                _ => {
                    self.tags.remove(name);
                    deleted.push(name.clone());
                }
            }
        }
        if !records.is_empty() || !deleted.is_empty() {
            self.s.icloud_fetched(&records, &deleted);
        }
        // send, until nothing is refused
        for _ in 0..5 {
            let changes = self.s.icloud_changes();
            if changes.save.is_empty() && changes.delete.is_empty() {
                return;
            }
            let mut saved = Vec::new();
            let mut refused = Vec::new();
            for r in self.s.icloud_records(&changes.save) {
                let name = r["name"].as_str().unwrap().to_string();
                if r["gone"] == true {
                    continue;
                }
                let current = cloud.records.get(&name).map(|c| c.tag);
                if let Some(tag) = current.filter(|t| Some(*t) != self.tags.get(&name).copied()) {
                    refused.push(Self::incoming(&name, &cloud.records[&name]));
                    self.tags.insert(name, tag);
                    continue;
                }
                let tag = current.unwrap_or(0) + 1;
                let stored = Stored {
                    kind: r["kind"].as_str().unwrap().into(),
                    key: r["key"].as_str().unwrap().into(),
                    data: r["data"].as_str().unwrap().into(),
                    modified: r["modified"].as_i64().unwrap(),
                    tag,
                };
                saved.push(Sent { name: name.clone(), kind: stored.kind.clone(), key: stored.key.clone(), hash: r["hash"].as_str().unwrap().into() });
                cloud.records.insert(name.clone(), stored);
                cloud.change(&name, false);
                self.tags.insert(name, tag);
            }
            for name in &changes.delete {
                if cloud.records.remove(name).is_some() {
                    cloud.change(name, true);
                }
                self.tags.remove(name);
            }
            self.s.icloud_sent(&saved, &changes.delete);
            if !refused.is_empty() {
                self.s.icloud_fetched(&refused, &[]);
            }
        }
    }

    fn user_presets(&self) -> Vec<(String, String, Value)> {
        self.s.presets.iter().filter(|p| !p.builtin).map(|p| (p.id.clone(), p.name.clone(), p.settings.clone())).collect()
    }

    fn add_preset(&mut self, id: &str, name: &str, exposure: f64) {
        self.s.presets.push(lightcraft_develop::Preset {
            id: id.into(),
            name: name.into(),
            group: "Mine".into(),
            settings: json!({"light": {"exposure": exposure}}),
            favorite: false,
            builtin: false,
        });
    }

    fn keyword_sets(&self) -> Vec<Value> {
        self.s.doc_items("keyword-sets")
    }

    /// The app fetched asset `asset`'s original (a picture made from `seed`) and opened it.
    fn open_asset(&mut self, asset: &str, cloud: &str, seed: u8) -> Value {
        let p = self.root.join("staging").join(format!("{asset}-{seed}")).join(format!("IMG_{seed}.png"));
        png(&p, seed);
        self.s.execute("photoLibrary.open", &json!({"asset": asset, "path": p.to_string_lossy(), "cloud": cloud})).unwrap()
    }
}

fn id_of(v: &Value) -> PhotoId {
    PhotoId(v["id"].as_u64().unwrap())
}

#[test]
fn presets_travel_change_and_go_between_devices() {
    let root = temp_dir("presets");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    a.add_preset("warm", "Warm", 0.3);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert_eq!(b.user_presets(), vec![("warm".into(), "Warm".into(), json!({"light": {"exposure": 0.3}}))]);

    // changed on b
    b.s.presets.iter_mut().filter(|p| p.id == "warm").for_each(|p| p.name = "Warmer".into());
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert_eq!(a.user_presets()[0].1, "Warmer");

    // deleted on a
    a.s.presets.retain(|p| p.id != "warm");
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert!(b.user_presets().is_empty(), "{:?}", b.user_presets());
    assert!(!cloud.records.values().any(|r| r.kind == "presets"), "{:?}", cloud.records.keys().collect::<Vec<_>>());
    // nothing left to send on either
    assert_eq!(a.s.icloud_changes(), Default::default());
    assert_eq!(b.s.icloud_changes(), Default::default());
}

#[test]
fn shared_settings_travel_item_by_item() {
    let root = temp_dir("docs");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    a.s.set_doc_items("keyword-sets", &[json!({"name": "Travel", "keywords": ["beach", "sun"]})]).unwrap();
    b.s.set_doc_items("keyword-sets", &[json!({"name": "Family", "keywords": ["mum"]})]).unwrap();
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    let names = |d: &Device| {
        let mut n: Vec<String> = d.keyword_sets().iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
        n.sort();
        n
    };
    assert_eq!(names(&a), ["Family", "Travel"], "both kept: added on two devices");
    assert_eq!(names(&b), ["Family", "Travel"]);
    // one removed on b goes on a, the other stays
    b.s.set_doc_items("keyword-sets", &[json!({"name": "Travel", "keywords": ["beach", "sun"]})]).unwrap();
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert_eq!(names(&a), ["Travel"]);
}

#[test]
fn the_newer_change_wins_when_both_devices_changed_a_preset() {
    let root = temp_dir("conflict");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    a.add_preset("p", "P", 0.1);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    // both change it offline: a first, b later
    a.at(2_000);
    a.s.presets.iter_mut().filter(|p| p.id == "p").for_each(|p| p.settings = json!({"light": {"exposure": 0.5}}));
    let _ = a.s.icloud_changes();
    b.at(3_000);
    b.s.presets.iter_mut().filter(|p| p.id == "p").for_each(|p| p.settings = json!({"light": {"exposure": 0.9}}));
    let _ = b.s.icloud_changes();
    // a reaches the cloud first; b's save is then refused, and b's change is the newer: it is sent over a's
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    let exposure = |d: &Device| d.user_presets()[0].2["light"]["exposure"].clone();
    assert_eq!((exposure(&a), exposure(&b)), (json!(0.9), json!(0.9)));
    // and nothing goes back and forth any more
    for d in [&mut a, &mut b] {
        assert_eq!(d.s.icloud_changes(), Default::default());
    }
}

#[test]
fn the_older_change_gives_way() {
    let root = temp_dir("older");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    a.add_preset("p", "P", 0.1);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    b.at(2_000);
    b.s.presets.iter_mut().filter(|p| p.id == "p").for_each(|p| p.name = "Older".into());
    let _ = b.s.icloud_changes();
    a.at(3_000);
    a.s.presets.iter_mut().filter(|p| p.id == "p").for_each(|p| p.name = "Newer".into());
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert_eq!((a.user_presets()[0].1.as_str(), b.user_presets()[0].1.as_str()), ("Newer", "Newer"));
}

#[test]
fn an_edit_of_a_photos_library_picture_follows_it_to_the_other_device() {
    let root = temp_dir("edits");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    // a opens the picture from the photo library, rates and brightens it: it joins a's library
    let on_a = id_of(&a.open_asset("A-local-1", "CLOUD-1", 1));
    a.s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    a.s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
    a.sync(&mut cloud);
    assert!(cloud.records.keys().any(|n| n.starts_with("edit.")), "{:?}", cloud.records.keys().collect::<Vec<_>>());

    // b has the same picture in its photo library, not opened yet: the edit is held, and says which asset it is
    b.sync(&mut cloud);
    assert_eq!(b.s.execute("icloud.heldAssets", &json!({})).unwrap(), json!({"assets": ["CLOUD-1"]}));
    assert_eq!(b.s.icloud_changes(), Default::default(), "holding it is no change of b's");
    // opened there: it comes in edited, into the library
    let r = b.open_asset("B-local-7", "CLOUD-1", 1);
    assert_eq!((r["local"].as_bool(), r["synced"].as_bool()), (Some(false), Some(true)), "{r}");
    let on_b = id_of(&r);
    let p = b.s.catalog.photo(on_b).unwrap();
    assert_eq!(p.rating, 4);
    assert_eq!(p.develop.light.exposure, 0.7);
    assert!(p.history.iter().any(|h| h.label == crate::icloud::LABEL), "a History step says where it came from");
    assert!(b.s.undo.is_empty(), "no undo step");
    assert_eq!(b.s.execute("icloud.heldAssets", &json!({})).unwrap(), json!({"assets": []}));
    assert_eq!(b.s.icloud_changes(), Default::default(), "taken as it came: nothing to send back");

    // a change on b goes back to a's photo
    b.s.execute("photo.rate", &json!({"rating": 2, "ids": [on_b.0]})).unwrap();
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert_eq!(a.s.catalog.photo(on_a).unwrap().rating, 2);
    assert_eq!(a.s.icloud_changes(), Default::default());
}

#[test]
fn a_photo_only_looked_at_sends_nothing() {
    let root = temp_dir("looked");
    let mut cloud = Cloud::default();
    let mut a = Device::new(&root, "a");
    let id = id_of(&a.open_asset("A1", "C1", 3));
    a.sync(&mut cloud);
    let edits = |c: &Cloud| c.records.values().filter(|r| r.kind == "edit").count();
    assert_eq!(edits(&cloud), 0);
    a.s.execute("photoLibrary.close", &json!({"id": id.0})).unwrap();
    a.sync(&mut cloud);
    assert_eq!(edits(&cloud), 0);
}

#[test]
fn a_library_signed_in_to_a_server_shares_only_the_server() {
    let root = temp_dir("server");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    a.add_preset("warm", "Warm", 0.3);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert_eq!(b.user_presets().len(), 1);

    // a signs in to a server: its presets go through the server now, and the server is offered to b
    let config = crate::cloud_sync::SyncConfig {
        server: "https://photos.example.com".into(),
        user: "ann".into(),
        token: "secret-token".into(),
        ..Default::default()
    };
    a.s.sync = Some(crate::cloud_sync::SyncState::new(config));
    a.add_preset("cool", "Cool", -0.3);
    a.sync(&mut cloud);
    assert!(!cloud.records.values().any(|r| r.key == "cool"), "the server keeps a's presets");
    assert!(cloud.records.values().all(|r| !r.data.contains("secret-token")), "no token travels");
    b.sync(&mut cloud);
    let st = b.s.execute("icloud.status", &json!({})).unwrap();
    assert_eq!(st["servers"], json!([{"server": "https://photos.example.com", "user": "ann"}]), "{st}");
    assert_eq!(b.user_presets().len(), 1, "b keeps what it had");

    // signing out of the server removes the account's presets from a: they come back from iCloud, nothing is
    // deleted there
    a.s.sync = None;
    a.s.presets.retain(|p| p.builtin);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert_eq!(a.user_presets().len(), 1, "{:?}", a.user_presets());
    assert_eq!(b.user_presets().len(), 1);
    // and b forgets the server a left
    let st = b.s.execute("icloud.status", &json!({})).unwrap();
    assert_eq!(st["servers"], json!([]), "{st}");
}

#[test]
fn turning_it_off_and_on_again_merges_without_deleting() {
    let root = temp_dir("again");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    a.add_preset("one", "One", 0.1);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    b.s.execute("icloud.enable", &json!({"on": false})).unwrap();
    assert_eq!(b.s.execute("icloud.status", &json!({})).unwrap(), json!({"on": false}));
    // while off, b deletes it: on again, it comes back rather than being deleted for everyone
    b.s.presets.retain(|p| p.id != "one");
    b.s.execute("icloud.enable", &json!({"on": true})).unwrap();
    b.sync(&mut cloud);
    assert_eq!(b.user_presets().len(), 1);
    assert!(cloud.records.values().any(|r| r.key == "one"));
}

#[test]
fn the_state_is_kept_with_the_library() {
    let root = temp_dir("kept");
    let mut cloud = Cloud::default();
    let mut a = Device::new(&root, "a");
    a.add_preset("one", "One", 0.1);
    a.sync(&mut cloud);
    a.s.execute("icloud.transport", &json!({"state": "opaque engine state"})).unwrap();
    a.s.close_library().unwrap();
    a.s.open_library(a.root.join("lib"), false).unwrap();
    let st = a.s.execute("icloud.status", &json!({"transport": true})).unwrap();
    assert_eq!((st["on"].as_bool(), st["transport"].as_str()), (Some(true), Some("opaque engine state")), "{st}");
    assert_eq!(a.s.icloud_changes(), Default::default(), "what the cloud has is remembered");
}

#[test]
fn records_that_are_not_what_they_say_are_left_out() {
    let root = temp_dir("hostile");
    let mut a = Device::new(&root, "a");
    let name = crate::icloud::record_name("presets", "x");
    let bad = [
        // a name that isn't its kind and key's
        Incoming { name: "presets.0".into(), kind: "presets".into(), key: "x".into(), data: "{}".into(), modified: 0 },
        // an unknown kind
        Incoming { name: crate::icloud::record_name("nope", "x"), kind: "nope".into(), key: "x".into(), data: "{}".into(), modified: 0 },
        // not a preset, or another preset's id
        Incoming { name: name.clone(), kind: "presets".into(), key: "x".into(), data: "[1, 2".into(), modified: 0 },
        Incoming {
            name: name.clone(),
            kind: "presets".into(),
            key: "x".into(),
            data: json!({"id": "y", "name": "Y", "group": "", "settings": {}}).to_string(),
            modified: i64::MAX,
        },
        // too big
        Incoming { name: crate::icloud::record_name("edit", "k"), kind: "edit".into(), key: "k".into(), data: "x".repeat(9 << 20), modified: 0 },
        // not an edit
        Incoming {
            name: crate::icloud::record_name("edit", "k"),
            kind: "edit".into(),
            key: "k".into(),
            data: "{\"xmp\": 5}".into(),
            modified: i64::MAX,
        },
    ];
    a.add_preset("x", "Mine", 0.2);
    a.s.set_doc_items("keyword-sets", &[json!({"name": "Travel", "keywords": ["sun"]})]).unwrap();
    let travel = crate::icloud::record_name("keyword-sets", "travel");
    let mut bad = bad.to_vec();
    // an item another version wrote that this one can't read
    bad.push(Incoming { name: travel, kind: "keyword-sets".into(), key: "travel".into(), data: json!({"name": 7}).to_string(), modified: i64::MAX });
    a.s.icloud_fetched(&bad, &["nothing".into(), String::new()]);
    assert_eq!(a.user_presets().len(), 1, "a preset that can't be read leaves this one as it is");
    assert_eq!(a.keyword_sets().len(), 1, "and so does a keyword set");
    let r = a.s.execute("icloud.fetched", &json!({"records": "not a list"}));
    assert!(r.is_err());
    let r = a.s.execute("icloud.records", &json!({"names": ["unknown", "x".repeat(4000)]})).unwrap();
    assert_eq!(r["records"][0], json!({"name": "unknown", "gone": true}));
}

#[test]
fn three_devices_end_up_the_same() {
    let root = temp_dir("three");
    let mut cloud = Cloud::default();
    let mut d: Vec<Device> = ["a", "b", "c"].iter().map(|n| Device::new(&root, n)).collect();
    for (i, dev) in d.iter_mut().enumerate() {
        dev.at(1_000 + i as i64);
        dev.add_preset(&format!("p{i}"), &format!("P{i}"), i as f64 / 10.0);
        dev.s.set_doc_items("keyword-sets", &[json!({"name": "Shared", "keywords": [format!("k{i}")]})]).unwrap();
    }
    for _ in 0..3 {
        for dev in d.iter_mut() {
            dev.sync(&mut cloud);
        }
    }
    let first = (d[0].user_presets(), d[0].keyword_sets());
    assert_eq!(first.0.len(), 3);
    assert_eq!(first.1.len(), 1, "one set by that name: the newest");
    assert_eq!(first.1[0]["keywords"], json!(["k2"]));
    for dev in &mut d {
        let mut mine = dev.user_presets();
        mine.sort_by(|x, y| x.0.cmp(&y.0));
        let mut want = first.0.clone();
        want.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(mine, want);
        assert_eq!(dev.keyword_sets(), first.1);
        assert_eq!(dev.s.icloud_changes(), Default::default());
    }
}

// ---------------------------------------------------------------- photos kept in iCloud Drive

/// Each device's iCloud Drive folder (a folder of its own, as on a real device: another path, the same files).
fn drive(d: &Device) -> PathBuf {
    d.root.join("drive")
}

/// What iCloud Drive does between the devices: every file one has, the others get.
fn drive_sync(devices: &[&Device]) {
    fn walk(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let r = rel.join(e.file_name());
            if p.is_dir() {
                walk(&p, &r, out);
            } else {
                out.push(r);
            }
        }
    }
    for from in devices {
        let mut files = Vec::new();
        walk(&drive(from), Path::new(""), &mut files);
        for to in devices {
            for rel in &files {
                let dst = drive(to).join(rel);
                if !dst.exists() {
                    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
                    std::fs::copy(drive(from).join(rel), &dst).unwrap();
                }
            }
        }
    }
}

/// A device with Keep Photos in iCloud Drive on.
fn with_drive(root: &Path, name: &str) -> Device {
    let mut d = Device::new(root, name);
    std::fs::create_dir_all(drive(&d)).unwrap();
    d.s.execute("icloud.folder", &json!({"path": drive(&d).to_string_lossy()})).unwrap();
    d.s.execute("icloud.photos", &json!({"on": true})).unwrap();
    d
}

/// Import a picture made from `seed` as the app does (copied into the library).
fn import(d: &mut Device, seed: u8) -> PhotoId {
    let p = d.root.join("camera").join(format!("IMG_{seed}.png"));
    png(&p, seed);
    let r = d.s.execute("library.import", &json!({"paths": [p.to_string_lossy()], "mode": "copy"})).unwrap();
    PhotoId(r["imported"][0].as_u64().unwrap_or_else(|| panic!("{r}")))
}

fn rate(d: &mut Device, id: PhotoId, rating: u8) {
    d.s.selection = crate::Selection::single(id);
    d.s.execute("photo.rate", &json!({"rating": rating, "ids": [id.0]})).unwrap();
}

fn path_of(d: &Device, id: PhotoId) -> String {
    match &d.s.catalog.photo(id).unwrap().source {
        lightcraft_catalog::Source::File { path } => path.clone(),
        other => panic!("{other:?}"),
    }
}

/// Bring every wanted file in, as the host does once iCloud Drive has them.
fn bring(d: &mut Device) -> Value {
    let wanted = d.s.execute("icloud.wanted", &json!({})).unwrap();
    let files: Vec<Value> = wanted["files"].as_array().unwrap().iter().map(|f| f["file"].clone()).collect();
    d.s.execute("icloud.adopt", &json!({"files": files})).unwrap()
}

#[test]
fn photos_kept_in_icloud_drive_come_to_the_other_device_with_their_edits() {
    let root = temp_dir("drive");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (with_drive(&root, "a"), with_drive(&root, "b"));
    let on_a = import(&mut a, 7);
    assert!(path_of(&a, on_a).starts_with(&drive(&a).to_string_lossy().to_string()), "imported into iCloud Drive: {}", path_of(&a, on_a));
    rate(&mut a, on_a, 3);
    a.sync(&mut cloud);

    // b learns of it before iCloud Drive has brought the file: wanted, but not there yet
    b.sync(&mut cloud);
    let w = b.s.execute("icloud.wanted", &json!({})).unwrap();
    assert_eq!(w["total"], 1, "{w}");
    let early = bring(&mut b);
    assert_eq!((early["added"].as_u64(), early["missing"].as_u64()), (Some(0), Some(1)), "{early}");
    assert!(b.s.activity.list().iter().any(|t| t.kind == "icloud"), "a row while photos are coming");

    // the file came: it joins b's library where iCloud Drive keeps it, with a's rating, and no undo step
    drive_sync(&[&a, &b]);
    let r = bring(&mut b);
    assert_eq!((r["added"].as_u64(), r["left"].as_u64()), (Some(1), Some(0)), "{r}");
    assert!(!b.s.activity.list().iter().any(|t| t.kind == "icloud"), "the row goes when nothing is left");
    let on_b = b.s.catalog.photos().find(|p| p.in_library()).map(|p| p.id).unwrap();
    assert!(path_of(&b, on_b).starts_with(&drive(&b).to_string_lossy().to_string()), "{}", path_of(&b, on_b));
    assert_eq!(b.s.catalog.photo(on_b).unwrap().rating, 3);
    assert!(b.s.undo.is_empty(), "no undo step");
    assert_eq!(b.s.icloud_changes(), Default::default(), "nothing to send back");

    // edits go both ways
    rate(&mut b, on_b, 5);
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert_eq!(a.s.catalog.photo(on_a).unwrap().rating, 5);

    // Recently Deleted on one is Recently Deleted on the other
    a.s.selection = crate::Selection::single(on_a);
    a.s.execute("photo.delete", &json!({"ids": [on_a.0]})).unwrap();
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert!(b.s.catalog.photo(on_b).unwrap().deleted, "deleted on a: deleted on b");

    // removed for good on one, removed on the other (the file stays)
    a.s.selection = crate::Selection::single(on_a);
    a.s.execute("photo.deletePermanently", &json!({"ids": [on_a.0]})).unwrap();
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert!(b.s.catalog.photo(on_b).is_none(), "removed for good on a: removed on b");
    assert!(Path::new(&drive(&b)).read_dir().unwrap().next().is_some(), "files are never deleted");
    for d in [&mut a, &mut b] {
        assert_eq!(d.s.icloud_changes(), Default::default());
        assert_eq!(d.s.execute("icloud.wanted", &json!({})).unwrap()["total"], 0, "and nothing comes back");
    }
}

#[test]
fn originals_the_library_keeps_move_into_icloud_drive() {
    let root = temp_dir("move-in");
    let mut a = Device::new(&root, "a");
    let id = import(&mut a, 3);
    let other = import(&mut a, 6);
    let old = path_of(&a, id);
    assert!(old.contains("Originals"), "{old}");
    std::fs::write(format!("{old}.xmp"), b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap();
    // turning it on needs the folder
    assert!(a.s.execute("icloud.photos", &json!({"on": true})).is_err());
    std::fs::create_dir_all(drive(&a)).unwrap();
    a.s.execute("icloud.folder", &json!({"path": drive(&a).to_string_lossy()})).unwrap();
    let st = a.s.execute("icloud.photos", &json!({"on": true})).unwrap();
    assert_eq!(st["movable"]["photos"], 2, "{st}");
    let r = a.s.execute("icloud.moveIn", &json!({"limit": 1})).unwrap();
    assert_eq!((r["moved"].as_u64(), r["left"].as_u64()), (Some(1), Some(1)), "{r}");
    assert!(a.s.activity.list().iter().any(|t| t.kind == "icloud"), "a row while some are left");
    let r = a.s.execute("icloud.moveIn", &json!({})).unwrap();
    assert_eq!((r["moved"].as_u64(), r["left"].as_u64()), (Some(1), Some(0)), "{r}");
    assert!(!a.s.activity.list().iter().any(|t| t.kind == "icloud"), "and none once all moved");
    assert!(path_of(&a, other).starts_with(&drive(&a).to_string_lossy().to_string()));
    let new = path_of(&a, id);
    assert!(new.starts_with(&drive(&a).to_string_lossy().to_string()) && Path::new(&new).is_file(), "{new}");
    assert!(!Path::new(&old).exists(), "moved, not copied");
    assert!(Path::new(&format!("{new}.xmp")).is_file(), "the sidecar went with it");
    // (the same folders as in Originals/)
    let rel_old = old.split("Originals").nth(1).unwrap().to_string();
    assert!(new.ends_with(&rel_old), "{new} keeps {rel_old}");
    assert!(a.s.undo.iter().all(|u| !u.label.contains("Rename")), "not an undo step");
    assert_eq!(a.s.execute("icloud.status", &json!({})).unwrap()["inFolder"], 2);
}

#[test]
fn a_file_icloud_drive_took_back_is_wanted_until_it_is_here_again() {
    let root = temp_dir("evicted");
    let mut a = with_drive(&root, "a");
    let id = import(&mut a, 4);
    let path = path_of(&a, id);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let w = a.s.execute("icloud.wanted", &json!({})).unwrap();
    assert_eq!(w["files"][0]["id"], id.0, "{w}");
    let rows = |a: &Device| a.s.activity.list().iter().filter(|t| t.kind == "icloud").count();
    assert_eq!(rows(&a), 1, "a row while it is coming");
    // the host stops waiting (a file deleted in iCloud Drive never comes): the row goes, the file is still wanted
    let w = a.s.execute("icloud.wanted", &json!({"waiting": false})).unwrap();
    assert_eq!((w["total"].as_u64(), rows(&a)), (Some(1), 0), "{w}");
    std::fs::write(&path, bytes).unwrap();
    let r = a.s.execute("icloud.adopt", &json!({"files": [w["files"][0]["file"]]})).unwrap();
    assert_eq!((r["back"].as_u64(), r["left"].as_u64()), (Some(1), Some(0)), "{r}");
    // nothing that could leave the folder is taken
    let r = a.s.execute("icloud.adopt", &json!({"files": ["../lib/catalog.log", "/etc/passwd", "a/./b.png", ""]})).unwrap();
    assert_eq!(r["refused"], 4, "{r}");
}

#[test]
fn photos_stay_out_of_icloud_drive_while_it_is_off() {
    let root = temp_dir("drive-off");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (with_drive(&root, "a"), Device::new(&root, "b"));
    let on_a = import(&mut a, 9);
    rate(&mut a, on_a, 2);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    // b has no iCloud Drive folder: it holds the record, wants nothing, and its imports stay in its library
    assert_eq!(b.s.execute("icloud.wanted", &json!({})).unwrap()["total"], 0);
    let on_b = import(&mut b, 5);
    assert!(path_of(&b, on_b).contains("Originals"));
    assert!(b.s.execute("icloud.adopt", &json!({"files": ["x.png"]})).is_err());
}

// ---------------------------------------------------------------- what was found in the photos

/// Import a picture of one colour (`rgb`), as the app does.
fn import_colour(d: &mut Device, rgb: [u8; 3], name: &str) -> PhotoId {
    let p = d.root.join("camera").join(name);
    let img = lightcraft_raster::Rgba8::from_fn(48, 32, |_, _| [rgb[0], rgb[1], rgb[2], 255]);
    let b =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b).unwrap();
    let r = d.s.execute("library.import", &json!({"paths": [p.to_string_lossy()], "mode": "copy"})).unwrap();
    PhotoId(r["imported"][0].as_u64().unwrap_or_else(|| panic!("{r}")))
}

fn key_of(d: &Device, id: PhotoId) -> String {
    lightcraft_catalog::sync::content_key(d.s.catalog.photo(id).unwrap()).unwrap()
}

fn parts(d: &Device, key: &str) -> u8 {
    d.s.analysis_parts(&[key.to_string()]).get(key).copied().unwrap_or(0)
}

#[test]
fn what_a_computer_found_in_a_photo_reaches_a_phone_that_cannot_find_it() {
    use crate::vision::portable_bits::{FACES, SENSITIVE, TEXT, VECTOR};
    use lightcraft_vision::fake::{FakeClassifier, FakeEmbedder, FakeFaces, FakeReader};
    let root = temp_dir("analysis");
    let mut cloud = Cloud::default();
    // a computer with the models works a red photo out: its words, its faces, its vector, its score
    let mut mac = with_drive(&root, "mac");
    mac.s.vision.set_embedder(Arc::new(FakeEmbedder));
    mac.s.vision.set_text_reader(Arc::new(FakeReader));
    mac.s.vision.set_face_finder(Arc::new(FakeFaces));
    mac.s.vision.set_classifier(Arc::new(FakeClassifier));
    mac.s.vision.text_edge = 192;
    let on_mac = import_colour(&mut mac, [230, 20, 20], "IMG_0042.png");
    for c in ["vision.setText", "vision.setFaces", "vision.setSensitive"] {
        mac.s.execute(c, &json!({"on": true})).unwrap();
    }
    mac.s.execute("vision.index", &json!({"wait": true})).unwrap();
    let key = key_of(&mac, on_mac);
    assert_eq!(parts(&mac, &key), FACES | TEXT | VECTOR | SENSITIVE, "all four were worked out");
    mac.sync(&mut cloud);
    assert!(cloud.records.values().any(|r| r.kind == "analysis" && r.key == key));

    // a phone that runs none of the models (thin; its face engine the computer's, so their faces group together)
    let mut phone = with_drive(&root, "phone");
    phone.s.vision.thin = true;
    phone.s.vision.set_face_finder(Arc::new(FakeFaces));
    phone.sync(&mut cloud);
    drive_sync(&[&mac, &phone]);
    assert_eq!(bring(&mut phone)["added"], 1);
    let on_phone = phone.s.catalog.photos().find(|p| p.in_library()).map(|p| p.id).unwrap();
    assert_eq!(parts(&phone, &key), FACES | TEXT | SENSITIVE, "the vector stays out: a phone can't search by description");
    // the words in it are found without a reader
    let r = phone.s.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap();
    assert!(r["photos"].as_array().unwrap().iter().any(|p| p["id"] == on_phone.0 && p["text"] == true), "{r}");
    assert_eq!(phone.s.execute("vision.model.status", &json!({})).unwrap()["words"], true);
    // it is listed as possibly sensitive (blurred) without the classifier
    assert!(phone.s.sensitive_photos().iter().any(|(id, _)| *id == on_phone));
    // and nothing goes back and forth
    assert_eq!(phone.s.icloud_changes(), Default::default());
    mac.sync(&mut cloud);
    assert_eq!(mac.s.icloud_changes(), Default::default());

    // another computer with the same search model needn't work the vector out again
    let mut mac2 = with_drive(&root, "mac2");
    mac2.s.vision.set_embedder(Arc::new(FakeEmbedder));
    mac2.sync(&mut cloud);
    drive_sync(&[&mac, &mac2]);
    bring(&mut mac2);
    assert_ne!(parts(&mac2, &key) & VECTOR, 0, "the vector came");
    assert_eq!(mac2.s.icloud_changes(), Default::default());
}

#[test]
fn a_device_that_found_more_fills_the_cloud_in_and_keeps_its_own() {
    use crate::vision::portable_bits::{SENSITIVE, TEXT, VECTOR};
    use lightcraft_vision::fake::{FakeClassifier, FakeEmbedder, FakeReader};
    let root = temp_dir("analysis-more");
    let mut cloud = Cloud::default();
    // a computer that only describes
    let mut a = with_drive(&root, "a");
    a.s.vision.set_embedder(Arc::new(FakeEmbedder));
    a.s.vision.text_edge = 192;
    let on_a = import_colour(&mut a, [20, 20, 230], "hotel.png");
    a.s.execute("vision.index", &json!({"wait": true})).unwrap();
    let key = key_of(&a, on_a);
    a.sync(&mut cloud);
    // another that also reads words and checks content: it adds what it found, and keeps its own vector
    let mut b = with_drive(&root, "b");
    b.s.vision.set_embedder(Arc::new(FakeEmbedder));
    b.s.vision.set_text_reader(Arc::new(FakeReader));
    b.s.vision.set_classifier(Arc::new(FakeClassifier));
    b.s.vision.text_edge = 192;
    b.sync(&mut cloud);
    drive_sync(&[&a, &b]);
    bring(&mut b);
    for c in ["vision.setText", "vision.setSensitive"] {
        b.s.execute(c, &json!({"on": true})).unwrap();
    }
    b.s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(parts(&b, &key), TEXT | VECTOR | SENSITIVE);
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert_eq!(parts(&a, &key) & VECTOR, VECTOR, "a keeps its own vector");
    assert_eq!(a.s.icloud_changes(), Default::default());
    assert_eq!(b.s.icloud_changes(), Default::default());
    // a record another device can't read leaves this one's as it is
    let name = crate::icloud::record_name("analysis", &key);
    let junk = Incoming {
        name,
        kind: "analysis".into(),
        key: key.clone(),
        data: json!({"faces": {"engine": "x", "found": "no"}, "vector": {"model": "fake-colour", "e": "!!"}}).to_string(),
        modified: 0,
    };
    a.s.icloud_fetched(&[junk], &[]);
    assert_eq!(parts(&a, &key) & VECTOR, VECTOR);
}
