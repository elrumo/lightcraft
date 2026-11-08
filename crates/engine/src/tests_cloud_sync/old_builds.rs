//! A device going back to a build that keeps its queue in `sync.outbox`, in a form it reads, and coming forward
//! again; and a queue that can't be read (HB-279).

use lightcraft_catalog::sync::Outbox;
use serde::Deserialize;

use super::*;
use crate::cloud_sync::{OLD_OUTBOX, OUTBOX};

/// `sync.outbox` as builds from before 2026-11-08 read it: every entry whole. What they can't read they call damaged
/// and reload the server's library, dropping the photos waiting to go.
#[derive(Deserialize)]
#[allow(dead_code)]
struct OldOutbox {
    entries: Vec<OldEntry>,
    rev: u64,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OldEntry {
    key: String,
    base: Op,
    latest: Op,
    rev: u64,
    #[serde(default)]
    sent: Option<Op>,
}

fn read(dir: &Path, name: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(name)).ok()
}

/// A device that uploaded `a.png` and `b.png`, then imported `c.png` and `d.png` with the server away, and closed.
fn two_photos_waiting(tag: &str, f: &mut Fake) -> (PathBuf, Vec<PhotoId>) {
    let (mut a, root, ids) = first_device(tag, f);
    f.down = true;
    write_png(&root.join("more/c.png"), 3);
    write_png(&root.join("more/d.png"), 4);
    a.execute("library.import", &json!({"paths": [root.join("more").to_string_lossy()]})).unwrap();
    let st = sync(&mut a, f);
    assert_eq!(st["state"], "error", "{st}");
    assert_eq!(f.core.catalog().len(), 2);
    drop(a);
    (root, ids)
}

fn names_on_server(f: &Fake) -> Vec<String> {
    let mut v: Vec<String> = f.core.catalog().photos().map(|p| p.file_name.clone()).collect();
    v.sort();
    v
}

#[test]
fn an_older_build_sees_no_queue_and_a_newer_one_sends_both_queues() {
    let mut f = Fake::new();
    let (root, ids) = two_photos_waiting("old-build", &mut f);
    let dir = root.join("a");
    // the queue holds the photos by id, which an older build can't read: it never looks there
    let queue = read(&dir, OUTBOX).expect("the queue");
    assert!(serde_json::from_slice::<OldOutbox>(&queue).is_err(), "an older build would call it damaged");
    if let Some(old) = read(&dir, OLD_OUTBOX) {
        let o: OldOutbox = serde_json::from_slice(&old).expect("an older build reads its file");
        assert!(o.entries.is_empty(), "and finds nothing waiting");
    }
    // the older build ran: it rated a photo with the server away (its own queue, in its own file)
    let a = open(&dir);
    let mut c = a.catalog.clone();
    drop(a);
    let op = Op::SetRating { id: ids[0], rating: 4 };
    let inverse = c.apply(op.clone()).unwrap();
    let mut theirs = Outbox::default();
    theirs.record(&op, &inverse, &c);
    let theirs = serde_json::to_vec(&theirs).unwrap();
    assert_eq!(serde_json::from_slice::<OldOutbox>(&theirs).unwrap().entries.len(), 1, "a change in the older form");
    std::fs::write(dir.join(OLD_OUTBOX), theirs).unwrap();
    // a newer build again: what both queued reaches the server
    let mut a = open(&dir);
    f.down = false;
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(names_on_server(&f), ["a.png", "b.png", "c.png", "d.png"]);
    assert_eq!(f.core.catalog().photo(ids[0]).unwrap().rating, 4, "the older build's change");
    assert_eq!(a.catalog.photo(ids[0]).unwrap().rating, 4);
    assert!(a.sync_state().unwrap().outbox.is_empty());
    drop(a);
    let old: OldOutbox = serde_json::from_slice(&read(&dir, OLD_OUTBOX).unwrap()).unwrap();
    assert!(old.entries.is_empty(), "the older build's file is left empty, not sent again");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_library_still_on_the_old_file_moves_to_the_new_one() {
    let mut f = Fake::new();
    let (root, _) = two_photos_waiting("old-file", &mut f);
    let dir = root.join("a");
    // the queue as the build of 2026-11-08 wrote it, under the old name
    std::fs::rename(dir.join(OUTBOX), dir.join(OLD_OUTBOX)).unwrap();
    let mut a = open(&dir);
    assert_eq!(a.sync_state().unwrap().outbox.len(), 2, "both photos still wait");
    f.down = false;
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(names_on_server(&f), ["a.png", "b.png", "c.png", "d.png"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_queue_that_cannot_be_read_is_rebuilt_from_the_servers_library() {
    let mut f = Fake::new();
    let (root, ids) = two_photos_waiting("unreadable", &mut f);
    let dir = root.join("a");
    // a later build's format (or a damaged file)
    std::fs::write(dir.join(OUTBOX), br#"{"format":9,"queue":"..."}"#).unwrap();
    // meanwhile another device removed b.png
    f.core.push(f.core.head(), &[Op::RemovePhoto { id: ids[1] }]).unwrap();
    let mut a = open(&dir);
    assert!(a.sync_state().unwrap().config.requeue);
    // a change made before the server answers writes the queue over: the rebuild is still to come after a restart
    a.selection = crate::Selection::single(ids[0]);
    a.execute("photo.rate", &json!({"rating": 3})).unwrap();
    drop(a);
    let mut a = open(&dir);
    assert!(a.sync_state().unwrap().config.requeue, "kept across the restart");
    f.down = false;
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(names_on_server(&f), ["a.png", "c.png", "d.png"], "every photo the server lacked, and not the one removed elsewhere");
    assert!(a.catalog.photo(ids[1]).is_none());
    assert_eq!(f.core.catalog().photo(ids[0]).unwrap().rating, 3);
    assert!(!a.sync_state().unwrap().config.requeue);
    assert!(a.sync_state().unwrap().outbox.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
