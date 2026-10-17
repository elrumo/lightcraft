//! The Hidden album and its lock: hidden photos leave every view until Hidden is unlocked, the lock
//! comes back when nothing shows them, and the password is kept with the library.

use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::{ErrorKind, LibrarySource, Selection, Session};

fn hide_first(s: &mut Session) -> PhotoId {
    let id = s.visible()[0];
    s.selection = Selection::single(id);
    s.execute("photo.hide", &json!({})).unwrap();
    id
}

#[test]
fn a_hidden_photo_leaves_the_library_until_hidden_is_unlocked() {
    let mut s = Session::with_demo();
    let total = s.visible().len();
    let id = hide_first(&mut s);
    assert_eq!(s.visible().len(), total - 1);
    assert!(!s.visible().contains(&id));
    assert!(!s.selection.contains(id), "the selection moves on");

    // locked: neither the album nor Show Hidden, and a filter from outside can't ask for them
    for (cmd, p) in [("library.source", json!({"kind": "hidden"})), ("library.showHidden", json!({"on": true}))] {
        let e = s.execute(cmd, &p).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Locked, "{cmd}: {e}");
    }
    let q = s.execute("catalog.query", &json!({"filter": {"hidden": "include"}, "limit": 10_000})).unwrap();
    assert_eq!(q["total"].as_u64(), Some((total - 1) as u64));
    assert_eq!(s.execute("hidden.status", &json!({})).unwrap()["count"], json!(null), "no count while locked");

    s.execute("hidden.setPassword", &json!({"password": "1234"})).unwrap();
    s.execute("library.source", &json!({"kind": "hidden"})).unwrap();
    assert_eq!(s.visible(), [id]);
    assert_eq!(s.execute("hidden.status", &json!({})).unwrap()["count"], 1);

    // leaving the album with Show Hidden off locks it again
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    assert!(!s.hidden.unlocked());
    assert_eq!(s.execute("library.source", &json!({"kind": "hidden"})).unwrap_err().kind(), ErrorKind::Locked);
}

#[test]
fn show_hidden_mixes_them_back_in_until_it_is_turned_off() {
    let mut s = Session::with_demo();
    let total = s.visible().len();
    let id = hide_first(&mut s);
    s.execute("hidden.setPassword", &json!({"password": "secret"})).unwrap();
    s.execute("library.showHidden", &json!({"on": true})).unwrap();
    assert_eq!(s.visible().len(), total);
    assert!(s.visible().contains(&id));
    s.execute("library.showHidden", &json!({"on": false})).unwrap();
    assert_eq!(s.visible().len(), total - 1);
    assert!(!s.hidden.unlocked(), "nothing shows hidden photos any more: locked");

    // unhide from the album: back in the library
    s.execute("hidden.unlock", &json!({"password": "secret"})).unwrap();
    s.execute("library.source", &json!({"kind": "hidden"})).unwrap();
    s.selection = Selection::single(id);
    s.execute("photo.unhide", &json!({})).unwrap();
    assert!(s.visible().is_empty());
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    assert_eq!(s.visible().len(), total);
}

#[test]
fn the_password_is_checked_and_wrong_ones_are_slowed_down() {
    let mut s = Session::with_demo();
    let e = s.execute("hidden.unlock", &json!({"password": "x"})).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Locked, "no password yet: {e}");
    assert!(s.execute("hidden.setPassword", &json!({"password": "abc"})).is_err(), "too short");
    s.execute("hidden.setPassword", &json!({"password": "right one"})).unwrap();
    s.execute("hidden.lock", &json!({})).unwrap();

    // changing it needs the current one
    assert_eq!(s.execute("hidden.setPassword", &json!({"password": "other"})).unwrap_err().kind(), ErrorKind::Locked);
    for _ in 0..4 {
        let e = s.execute("hidden.unlock", &json!({"password": "wrong"})).unwrap_err();
        assert_eq!(e.to_string(), "Wrong password");
    }
    s.execute("hidden.unlock", &json!({"password": "right one"})).unwrap();
    assert!(s.hidden.unlocked());
    s.execute("hidden.lock", &json!({})).unwrap();
    for _ in 0..5 {
        let _ = s.execute("hidden.unlock", &json!({"password": "wrong"}));
    }
    let e = s.execute("hidden.unlock", &json!({"password": "right one"})).unwrap_err();
    assert!(e.to_string().contains("try again in"), "{e}");
    assert!(!s.hidden.unlocked());
    // the password commands never land in the command journal
    assert!(s.journal.iter().all(|(id, p)| !id.starts_with("hidden.") && p.get("password").is_none()), "{:?}", s.journal);
}

#[test]
fn the_password_stays_with_the_library_and_a_relaunch_is_locked() {
    let dir = std::env::temp_dir().join(format!("lc-hidden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::new().with_fs();
    s.open_library(&dir, true).unwrap();
    let id = hide_first(&mut s);
    s.execute("hidden.setPassword", &json!({"password": "kept"})).unwrap();
    s.execute("library.source", &json!({"kind": "hidden"})).unwrap();
    s.selection = Selection::single(id);
    s.close_library().unwrap();
    // (the library's lock goes with the session)
    drop(s);

    let mut s = Session::new().with_fs();
    s.open_library(&dir, false).unwrap();
    assert_eq!(s.source, LibrarySource::All, "Hidden isn't where a relaunch opens");
    assert!(!s.selection.contains(id));
    assert!(!s.hidden.unlocked() && s.hidden.has_password());
    assert!(s.catalog.photo(id).unwrap().hidden);
    s.execute("hidden.unlock", &json!({"password": "kept"})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn statistics_leave_hidden_photos_out_and_say_how_many_only_when_unlocked() {
    let mut s = Session::with_demo();
    let id = hide_first(&mut s);
    let stats = s.execute("catalog.stats", &json!({})).unwrap();
    assert_eq!(stats["hidden"], json!(null), "locked: not even how many");
    let total = stats["photos"].as_u64().unwrap();
    s.hidden_unlock_by_device();
    let stats = s.execute("catalog.stats", &json!({})).unwrap();
    assert_eq!((stats["photos"].as_u64(), stats["hidden"].as_u64()), (Some(total), Some(1)));
    assert!(s.catalog.photo(id).unwrap().hidden);
}

#[test]
fn a_hidden_photo_is_not_found_by_its_file_name() {
    let mut s = Session::with_demo();
    let id = hide_first(&mut s);
    let name = s.catalog.photo(id).unwrap().file_name.clone();
    // (a name search is one word)
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(n, _)| n).split_whitespace().next().unwrap_or_default().to_string();
    let found = |s: &mut Session| -> Vec<u64> {
        let r = s.execute("library.search", &json!({"q": stem, "wait": true}));
        r.map(|v| v["photos"].as_array().into_iter().flatten().filter_map(|p| p["id"].as_u64()).collect()).unwrap_or_default()
    };
    assert!(!found(&mut s).contains(&id.0), "locked: not found by name");
    s.execute("hidden.setPassword", &json!({"password": "1234"})).unwrap();
    s.execute("library.showHidden", &json!({"on": true})).unwrap();
    assert!(found(&mut s).contains(&id.0), "with Show Hidden it is");
}
