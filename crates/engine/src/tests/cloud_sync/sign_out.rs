//! Signing out of the server: the library waits until everything has been sent, then the device signs out there and the
//! library is emptied for the next account (docs/sync.md, "Signing out").

use super::*;

/// A library file's bytes (`None`: there is none).
fn file(s: &mut Session, name: &str) -> Option<Vec<u8>> {
    s.library.as_mut().unwrap().files_mut().read(name).unwrap()
}

fn library_dir(s: &Session) -> PathBuf {
    s.library.as_ref().unwrap().dir.clone()
}

/// Ask to sign out, as a person who agreed does, and run the sync and the clean-up of files until it has ended.
fn sign_out(s: &mut Session, f: &mut Fake) {
    s.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    s.sync_sign_out_wait_with(10_000, &mut |t| f.handle(t));
}

/// A photo added to the library that no server has yet.
fn add(s: &mut Session, root: &Path, name: &str, seed: u8) -> PhotoId {
    write_png(&root.join("more").join(name), seed);
    s.execute("library.import", &json!({"paths": [root.join("more").join(name).to_string_lossy()]})).unwrap();
    photo_named(s, name)
}

fn sign_in_as(s: &mut Session, user: &str) {
    s.execute("sync.signIn", &json!({"server": "http://fake", "user": user, "password": "pw", "device": "test"})).unwrap();
}

fn names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
}

#[test]
fn signing_out_sends_what_is_left_and_then_empties_the_library() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-waits", &mut f);
    // what hasn't reached the server: an edit, and a photo, with the server away
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 4})).unwrap();
    let c = add(&mut a, &root, "c.png", 3);
    f.down = true;
    let asked = a.execute("sync.signOut", &json!({})).unwrap_err().to_string();
    assert!(asked.contains("confirm"), "it asks first: {asked}");
    assert_eq!(a.catalog.len(), 3, "and removes nothing before");
    let r = a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    assert_eq!((r["state"].as_str(), r["signedIn"].as_bool()), (Some("signingOut"), Some(true)), "{r}");
    // the server is away: nothing is removed, and the status says what is left
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "signingOut", "{st}");
    assert_eq!(a.catalog.len(), 3);
    assert!(st["pending"].as_u64() > Some(0), "{st}");
    assert!(a.activity.list().iter().any(|t| t.kind == "signOut" && t.cancellable), "its row in the activity stack: {:?}", a.activity.list());
    assert!(a.execute("sync.pause", &json!({"on": true})).is_err(), "it can't be paused meanwhile");
    // back: the rest goes, the device signs out there, the library is empty
    f.down = false;
    a.sync_sign_out_wait_with(10_000, &mut |t| f.handle(t));
    assert!(a.sync_state().is_none(), "nothing of the account's sync is left");
    assert!(a.catalog.is_empty(), "{:?}", a.catalog.photos().map(|p| p.file_name.clone()).collect::<Vec<_>>());
    assert!(f.requests.iter().any(|r| r == "POST /api/logout"), "the server was told: {:?}", f.requests);
    // the account's library is untouched there: its photos, with the edit (nothing was deleted by emptying this copy)
    assert_eq!(f.core.catalog().len(), 3);
    assert!(f.core.catalog().photo(c).is_some());
    assert_eq!(f.core.catalog().photo(ids[0]).unwrap().rating, 4);
    assert_eq!(file(&mut a, "sync.json").as_deref(), Some(&b"{}"[..]));
    assert_eq!(a.execute("sync.status", &json!({})).unwrap()["state"], "off");
    assert!(a.execute("sync.signOut", &json!({"confirm": true})).is_err(), "there is nothing to sign out of now");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn another_account_signs_in_to_the_emptied_library_and_sees_only_its_own() {
    // ann's library, with a preset
    let mut fa = Fake::new();
    let (mut a, root, ids) = first_device("so-two", &mut fa);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("preset.create", &json!({"name": "Warm"})).unwrap();
    sync(&mut a, &mut fa);
    assert_eq!(fa.presets.version, 1);
    let ann_a = blob_key(a.catalog.photo(photo_named(&a, "a.png")).unwrap()).unwrap();
    // bob's library on another server, made on another device: one photo, numbered like ann's
    let mut fb = Fake::new();
    fb.library = "lib-2".into();
    let mut b = open(&root.join("bob-device"));
    write_png(&root.join("bobs/z.png"), 9);
    b.execute("library.import", &json!({"paths": [root.join("bobs").to_string_lossy()]})).unwrap();
    sign_in_as(&mut b, "bob");
    sync(&mut b, &mut fb);
    assert!(ids.contains(&photo_named(&b, "z.png")), "the two libraries number their photos alike");
    // ann signs out: nothing is left to send, so the library goes at once
    let generation = a.library_generation();
    sign_out(&mut a, &mut fa);
    assert!(a.catalog.is_empty() && a.sync_state().is_none());
    assert!(a.presets.iter().all(|p| p.builtin), "ann's presets went with her library");
    assert_ne!(a.library_generation(), generation, "frontends reset what they show");
    // bob signs in on this device: his library comes whole and none of ann's meets it
    sign_in_as(&mut a, "bob");
    let st = sync(&mut a, &mut fb);
    assert_ne!(st["state"], "conflict", "{st}");
    assert_eq!(a.catalog.photos().map(|p| p.file_name.clone()).collect::<Vec<_>>(), ["z.png"]);
    assert_eq!(a.sync_state().unwrap().config.library, "lib-2");
    assert!(a.presets.iter().all(|p| p.builtin), "ann's presets are not bob's");
    assert_eq!((fb.presets.version, fb.presets.presets.clone()), (0, json!([])), "and were not sent to his account");
    assert_eq!(fb.blobs.len(), 3, "none of ann's files reached his server");
    // a file ann had, added to bob's library, is sent to his server: what ann's device knew of the other one is forgotten
    a.execute("library.import", &json!({"paths": [root.join("in/a.png").to_string_lossy()]})).unwrap();
    sync(&mut a, &mut fb);
    assert!(fb.blobs.contains_key(&("original".to_string(), ann_a)), "{:?}", fb.blobs.keys().collect::<Vec<_>>());
    // ann's library is where she left it
    assert_eq!(fa.core.catalog().len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn signing_out_anyway_clears_the_library_keeps_a_copy_and_leaves_the_users_files_alone() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-force", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("preset.create", &json!({"name": "Warm"})).unwrap();
    sync(&mut a, &mut f);
    let lib = library_dir(&a);
    // what import copies, a previews folder somebody chose, and the files sync and faces made for the account
    std::fs::create_dir_all(lib.join("Originals")).unwrap();
    std::fs::write(lib.join("Originals/mine.jpg"), b"my copy").unwrap();
    let custom = root.join("my-previews");
    std::fs::create_dir_all(&custom).unwrap();
    std::fs::write(custom.join("shared.lcsp"), b"shared with other libraries").unwrap();
    a.smart_previews_dir = Some(custom.clone());
    a.media.smart_dir = Some(custom.clone());
    std::fs::create_dir_all(lib.join("Smart Previews")).unwrap();
    std::fs::write(lib.join("Smart Previews/old.lcsp"), b"x").unwrap();
    std::fs::create_dir_all(lib.join("sync/originals/abc")).unwrap();
    std::fs::write(lib.join("sync/originals/abc/photo.jpg"), b"x").unwrap();
    for name in ["face-scanned.bin", "face-embeddings-model.bin", "face-notes.txt"] {
        std::fs::write(lib.join(name), b"x").unwrap();
    }
    // something the server doesn't have, and the server away
    add(&mut a, &root, "c.png", 3);
    a.selection = crate::Selection::single(ids[0]);
    f.down = true;
    let before = f.requests.len();
    a.execute("sync.signOut", &json!({"force": true})).unwrap();
    a.sync_sign_out_wait_with(10, &mut |t| f.handle(t));
    assert!(a.catalog.is_empty() && a.sync_state().is_none());
    assert_eq!(f.requests.len(), before, "at once: nothing was waited for, nothing was sent");
    assert!(a.previous_active.is_none(), "a photo of the old library isn't the previous one in the next");
    // a copy of the library as it was, and its settings
    let names = names_in(&lib);
    let copy = names.iter().find(|n| n.starts_with("catalog-before-signout-") && n.ends_with(".snap")).unwrap_or_else(|| panic!("{names:?}"));
    assert!(names.iter().any(|n| n.starts_with("presets-before-signout-")), "{names:?}");
    let mut store = MemStore::new();
    lightcraft_catalog::Store::write_atomic(&mut store, lightcraft_catalog::journal::SNAPSHOT, &std::fs::read(lib.join(copy)).unwrap()).unwrap();
    let (_, old, _) = lightcraft_catalog::Journal::open(Box::new(store)).unwrap();
    let mut old_names: Vec<String> = old.photos().map(|p| p.file_name.clone()).collect();
    old_names.sort();
    assert_eq!(old_names, ["a.png", "b.png", "c.png"]);
    // what is the user's stays
    assert!(lib.join("Originals/mine.jpg").is_file(), "the copy import made is never deleted");
    assert!(custom.join("shared.lcsp").is_file(), "a previews folder somebody chose may be shared by libraries");
    assert!(lib.join("face-notes.txt").is_file());
    // what sync and faces made for the account goes
    assert!(!lib.join("Smart Previews").exists() && !lib.join("sync/originals").exists(), "{:?}", names_in(&lib));
    assert!(!lib.join("face-scanned.bin").exists() && !lib.join("face-embeddings-model.bin").exists());
    assert!(a.presets.iter().all(|p| p.builtin));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelling_the_wait_stays_signed_in_and_removes_nothing() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-cancel", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 2})).unwrap();
    f.down = true;
    assert!(a.execute("sync.cancelSignOut", &json!({})).is_err(), "nothing to cancel yet");
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    sync(&mut a, &mut f);
    // ✕ on the row in the activity stack
    let row = a.activity.list().into_iter().find(|t| t.kind == "signOut").expect("its row");
    a.activity.cancel(row.id).unwrap();
    sync(&mut a, &mut f);
    assert!(!a.sync_leaving());
    assert!(!a.activity.list().iter().any(|t| t.kind == "signOut"), "the row goes");
    assert_eq!(a.catalog.len(), 2);
    f.down = false;
    let st = sync(&mut a, &mut f);
    assert_eq!((st["state"].as_str(), st["signedIn"].as_bool()), (Some("idle"), Some(true)), "{st}");
    assert_eq!(f.core.catalog().photo(ids[0]).unwrap().rating, 2, "and the edit went on being sent");
    // and the command does the same
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    assert_eq!(a.execute("sync.cancelSignOut", &json!({})).unwrap()["state"], "idle");
    assert!(a.execute("sync.cancelSignOut", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// A server that signs the device out while it waits must not turn the wait into a removal: only `force` removes what
/// was not sent.
#[test]
fn a_server_that_signs_the_device_out_meanwhile_never_makes_the_wait_remove_anything() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-401", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 5})).unwrap();
    f.revoked = true;
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!((st["state"].as_str(), st["leaving"].as_bool()), (Some("signedOut"), Some(false)), "{st}");
    assert_eq!(a.catalog.len(), 2);
    // reopened (the next launch): the library is as it was, bound to the account and signed out
    drop(a);
    let mut a = open(&root.join("a"));
    assert_eq!(a.catalog.len(), 2, "nothing was removed");
    assert_eq!(a.catalog.photo(ids[0]).unwrap().rating, 5);
    assert_eq!(a.sync_state().map(|st| (st.state(), st.config.leaving, st.config.wiping)), Some(("signedOut", false, false)));
    // nothing can be sent, so signing out says what only `force` does
    let why = a.execute("sync.signOut", &json!({"confirm": true})).unwrap_err().to_string();
    assert!(why.contains("force"), "{why}");
    a.execute("sync.signOut", &json!({"force": true})).unwrap();
    a.sync_sign_out_wait_with(10, &mut |t| f.handle(t));
    assert!(a.catalog.is_empty() && a.sync_state().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_wipe_that_was_cut_short_is_finished_when_the_library_opens() {
    let mut f = Fake::new();
    let (a, root, _) = first_device("so-crash", &mut f);
    drop(a);
    // the marker is on disk, and nothing else was removed yet
    let path = root.join("a/sync.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["token"] = json!("");
    v["wiping"] = json!(true);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let mut a = open(&root.join("a"));
    assert!(a.catalog.is_empty(), "the library was emptied as it opened");
    assert!(a.sync_state().is_none());
    assert_eq!(file(&mut a, "sync.json").as_deref(), Some(&b"{}"[..]));
    // and it stays that way
    drop(a);
    let a = open(&root.join("a"));
    assert!(a.catalog.is_empty() && a.sync_state().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_library_that_never_joined_signs_out_at_once_and_keeps_its_photos() {
    let mut f = Fake::new();
    let (_a, root, _) = first_device("so-never", &mut f);
    let mut c = second_library(&root);
    sign_in(&mut c);
    assert_eq!(sync(&mut c, &mut f)["state"], "conflict");
    let before = f.requests.len();
    // nothing of this library is on the server: no question, no wait, nothing removed
    c.execute("sync.signOut", &json!({})).unwrap();
    assert_eq!(c.sync_state().map(|st| st.state()), Some("signedOut"));
    assert_eq!(c.catalog.len(), 2);
    assert_eq!(f.requests.len(), before, "and the server is told nothing");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn signing_in_waits_while_the_library_is_going() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-wait-in", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 1})).unwrap();
    f.down = true;
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    let why = a.execute("sync.signIn", &json!({"server": "http://fake", "user": "bob", "password": "pw"})).unwrap_err().to_string();
    assert!(why.contains("signing out"), "{why}");
    // and while the files of the account are being removed (a row in the activity stack)
    f.down = false;
    sign_out(&mut a, &mut f);
    assert!(a.sync_state().is_none());
    let row = a.activity.start("signOut", "Removing the library from this device", crate::activity::Cancel::No);
    assert!(a.execute("sync.signIn", &json!({"server": "http://fake", "user": "bob", "password": "pw"})).is_err());
    drop(row);
    a.execute("sync.signIn", &json!({"server": "http://fake", "user": "bob", "password": "pw"})).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// A browser keeps the account's preview files in its own storage: the host is told which keys to delete (once), and
/// the originals it holds stay, as the import copies of a native library do.
#[test]
fn a_browser_is_told_which_preview_files_to_delete_when_the_library_goes() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("so-web", &mut f);
    a.media.smart_dir = None;
    a.sync_store = Some(crate::cloud_sync::BrowserStore {
        prefix: "proxies/".into(),
        proxies: ["h1.lcsp", "h1.lcsm", "h2.lcsm"].iter().map(|n| n.to_string()).collect(),
        originals: ["h1".to_string()].into_iter().collect(),
    });
    assert!(a.take_sync_purge().is_empty(), "nothing to delete while signed in");
    sign_out(&mut a, &mut f);
    assert!(a.catalog.is_empty() && a.sync_state().is_none());
    let mut gone = a.take_sync_purge();
    gone.sort();
    assert_eq!(gone, ["proxies/h1.lcsm", "proxies/h1.lcsp", "proxies/h2.lcsm"]);
    assert!(a.take_sync_purge().is_empty(), "told once");
    let store = a.sync_store.as_ref().unwrap();
    assert!(store.proxies.is_empty(), "the engine no longer thinks they are stored");
    assert!(store.originals.contains("h1"), "the originals stay");
    let _ = std::fs::remove_dir_all(&root);
}

/// What is left to send includes the photo files: a server that can't take them keeps the library where it is, and the
/// wait ends once it has them (the file is on the server before anything is removed).
#[test]
fn a_photo_the_server_cannot_take_yet_is_waited_for_and_goes_up_before_the_library_goes() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("so-files", &mut f);
    let c = add(&mut a, &root, "c.png", 3);
    let key = blob_key(a.catalog.photo(c).unwrap()).unwrap();
    f.put_fails = Some("no space left on the server's disk".into());
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "signingOut", "{st}");
    assert_eq!(a.catalog.len(), 3, "nothing is removed while a file can't be sent");
    assert!(st["unsent"].as_u64() > Some(0), "the status says how many photos are left: {st}");
    assert!(!f.blobs.contains_key(&("original".to_string(), key.clone())));
    // room again
    f.put_fails = None;
    a.sync_sign_out_wait_with(10_000, &mut |t| f.handle(t));
    assert!(a.sync_state().is_none() && a.catalog.is_empty());
    assert!(f.blobs.contains_key(&("original".to_string(), key)), "its original reached the server first: {:?}", f.blobs.keys().collect::<Vec<_>>());
    assert!(f.requests.iter().any(|r| r == "POST /api/logout"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A user preset that the server hasn't got is waited for as well (it would be lost with the library).
#[test]
fn a_preset_the_server_has_not_got_yet_is_waited_for() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-presets", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    f.down = true;
    a.execute("preset.create", &json!({"name": "Warm"})).unwrap();
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "signingOut", "{st}");
    assert!(a.presets.iter().any(|p| !p.builtin), "still here");
    f.down = false;
    a.sync_sign_out_wait_with(10_000, &mut |t| f.handle(t));
    assert!(a.sync_state().is_none() && a.presets.iter().all(|p| p.builtin));
    assert_eq!(f.presets.presets.as_array().map(Vec::len), Some(1), "the server has the preset: {:?}", f.presets);
    let _ = std::fs::remove_dir_all(&root);
}

/// Quitting while it waits: the next launch goes on waiting, and ends the same way.
#[test]
fn quitting_while_it_waits_goes_on_at_the_next_launch() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-quit", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 3})).unwrap();
    f.down = true;
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    assert_eq!(sync(&mut a, &mut f)["state"], "signingOut");
    drop(a);
    let mut a = open(&root.join("a"));
    assert!(a.sync_leaving(), "the wait was saved");
    assert_eq!(a.catalog.len(), 2);
    f.down = false;
    a.sync_sign_out_wait_with(10_000, &mut |t| f.handle(t));
    assert!(a.sync_state().is_none() && a.catalog.is_empty());
    assert_eq!(f.core.catalog().photo(ids[0]).unwrap().rating, 3, "what was waiting went up first");
    let _ = std::fs::remove_dir_all(&root);
}

/// `force` keeps a copy even when nothing is unsent: a server that was set up again, or that signed the device out, can't
/// say what it has.
#[test]
fn signing_out_anyway_always_keeps_a_copy_of_a_library_that_is_a_folder() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("so-copy", &mut f);
    let lib = library_dir(&a);
    a.execute("sync.signOut", &json!({"force": true})).unwrap();
    a.sync_sign_out_wait_with(10, &mut |t| f.handle(t));
    let names = names_in(&lib);
    assert!(names.iter().any(|n| n.starts_with("catalog-before-signout-") && n.ends_with(".snap")), "{names:?}");
    assert!(a.catalog.is_empty() && a.sync_state().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// A sign-out whose wipe could not finish leaves its marker; signing in as another account finishes it first, so the old
/// account's photos are never sent to the new one.
#[test]
fn a_wipe_that_could_not_finish_is_finished_before_another_account_signs_in() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("so-retry", &mut f);
    let path = root.join("a/sync.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["token"] = json!("");
    v["wiping"] = json!(true);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    a.sync = None;
    assert_eq!(a.catalog.len(), 2, "the library still holds the first account's photos");
    sign_in_as(&mut a, "bob");
    assert!(a.catalog.is_empty(), "emptied first");
    assert_eq!(a.sync_state().map(|st| (st.config.user.clone(), st.config.wiping)), Some(("bob".to_string(), false)));
    let _ = std::fs::remove_dir_all(&root);
}

/// A LUT profile too big to be shared (over 4 MiB) was never on the server, so it is the computer's own and stays; the
/// ones that were shared go with the account.
#[test]
fn a_lut_profile_too_big_to_sync_stays_and_the_shared_ones_go() {
    use crate::cmd::lut_profiles::LutProfile;
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("so-lut", &mut f);
    let profiles = library_dir(&a).join("Profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    let small = profiles.join("small.cube");
    std::fs::write(&small, "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
    let big = profiles.join("big.cube");
    std::fs::write(&big, "# padding\n".repeat((4 << 20) / 10 + 10)).unwrap();
    let profile = |slug: &str, file: &Path| LutProfile {
        id: format!("lut:{slug}"),
        name: slug.into(),
        group: "Mine".into(),
        file: file.to_string_lossy().into_owned(),
    };
    a.lut_profiles = vec![profile("small", &small), profile("big", &big)];
    sign_out(&mut a, &mut f);
    assert!(a.sync_state().is_none());
    assert_eq!(a.lut_profiles.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["lut:big"]);
    assert!(big.is_file() && !small.exists(), "the shared one's file went, the other is the only copy of its LUT");
    let _ = std::fs::remove_dir_all(&root);
}

/// Cancelling puts syncing back as it was: paused again if it was, and the downloads dropped while it waited planned afresh.
#[test]
fn cancelling_puts_syncing_back_as_it_was() {
    let mut f = Fake::new();
    let (mut a, root, _) = first_device("so-cancel-pause", &mut f);
    a.execute("sync.pause", &json!({"on": true})).unwrap();
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    assert!(!a.sync_state().unwrap().config.paused, "it can't send what is left while paused");
    assert!(!a.sync_state().unwrap().plan_is_stale(), "the transfers were planned");
    a.execute("sync.cancelSignOut", &json!({})).unwrap();
    let st = a.sync_state().unwrap();
    assert!(st.config.paused, "paused again");
    assert!(st.plan_is_stale(), "the photo files are planned afresh");
    let _ = std::fs::remove_dir_all(&root);
}

/// A ✕ on the activity row is saved at once: quitting right after doesn't bring the wait back.
#[test]
fn a_cancel_from_the_activity_row_is_saved_at_once() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("so-row", &mut f);
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 1})).unwrap();
    f.down = true;
    a.execute("sync.signOut", &json!({"confirm": true})).unwrap();
    sync(&mut a, &mut f);
    let row = a.activity.list().into_iter().find(|t| t.kind == "signOut").expect("its row");
    a.activity.cancel(row.id).unwrap();
    sync(&mut a, &mut f);
    assert!(!a.sync_leaving());
    drop(a);
    let a = open(&root.join("a"));
    assert!(!a.sync_leaving(), "not waiting after a relaunch either");
    assert_eq!(a.catalog.len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}
