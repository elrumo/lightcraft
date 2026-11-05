//! "Also open on <device>": two devices showing the same photo notice each other, an edit of it is pulled at once, and
//! presence stays quiet and cheap everywhere else (one device, the background, a drag, an old or calling-for-calm server).

use super::*;

/// Run what is due now without asking for a pull (as the host does between polls): `sync_now_with` pulls every time.
fn step(s: &mut Session, f: &mut Fake) {
    for _ in 0..50 {
        let tasks = s.sync_tasks();
        if tasks.is_empty() {
            break;
        }
        for t in tasks {
            let d = f.handle(&t);
            s.sync_done(d);
        }
    }
}

fn show(s: &mut Session, id: PhotoId) {
    s.selection = crate::Selection::single(id);
    s.sync_set_foreground(true);
}

fn beats(f: &Fake) -> usize {
    f.requests.iter().filter(|r| r.starts_with("POST /api/presence")).count()
}

fn asks(f: &Fake) -> usize {
    f.requests.iter().filter(|r| r.starts_with("GET /api/me")).count()
}

/// Two devices, A and B, synced, with B joined from an empty library.
fn two(tag: &str, f: &mut Fake) -> (Session, Session, PathBuf, Vec<PhotoId>) {
    let (mut a, root, ids) = first_device(tag, f);
    let mut b = open(&root.join("b"));
    sign_in(&mut b);
    sync(&mut b, f);
    sync(&mut a, f);
    (a, b, root, ids)
}

#[test]
fn two_devices_on_the_same_photo_see_each_other_and_an_edit_is_pulled_at_once() {
    let mut f = Fake::new();
    let (mut a, mut b, root, ids) = two("presence-pair", &mut f);
    show(&mut a, ids[0]);
    show(&mut b, ids[0]);
    step(&mut a, &mut f);
    step(&mut b, &mut f);
    assert_eq!(b.sync_also_open_on(), ["Device 1"], "B was told A shows the photo too");
    // A hears about B at its next message
    a.presence_expedite();
    step(&mut a, &mut f);
    assert_eq!(a.sync_also_open_on(), ["Device 2"]);
    // another photo on B: no longer shared
    show(&mut b, ids[1]);
    b.presence_expedite();
    step(&mut b, &mut f);
    assert!(b.sync_also_open_on().is_empty());
    show(&mut b, ids[0]);
    b.presence_expedite();
    step(&mut b, &mut f);
    assert_eq!(b.sync_also_open_on(), ["Device 1"]);

    // B edits and sends it; A's next presence message tells it the log moved, and it pulls without waiting for the poll
    b.execute("develop.set", &json!({"control": "light.exposure", "value": 1.5})).unwrap();
    step(&mut b, &mut f);
    assert_eq!(exposure(&a, ids[0]), 0.0, "A has not pulled yet");
    let before = f.requests.len();
    a.presence_expedite();
    step(&mut a, &mut f);
    assert_eq!(exposure(&a, ids[0]), 1.5, "pulled because the presence answer showed a newer head");
    assert!(f.requests[before..].iter().any(|r| r.starts_with("GET /api/ops")), "{:?}", &f.requests[before..]);
    assert_eq!(a.sync_remote_edits(), 1, "and the frontend can say it was updated from another device");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_account_with_one_device_asks_how_many_devices_it_has_and_then_says_nothing() {
    let mut f = Fake::new();
    let (mut a, root, ids) = first_device("presence-single", &mut f);
    show(&mut a, ids[0]);
    step(&mut a, &mut f);
    assert_eq!((asks(&f), beats(&f)), (1, 0));
    for _ in 0..5 {
        a.presence_expedite();
        step(&mut a, &mut f);
    }
    assert_eq!((asks(&f), beats(&f)), (1, 0), "no more questions, nothing said");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn nothing_is_said_from_the_background_but_one_last_none_when_leaving() {
    let mut f = Fake::new();
    let (mut a, mut b, root, ids) = two("presence-background", &mut f);
    show(&mut a, ids[0]);
    show(&mut b, ids[0]);
    step(&mut a, &mut f);
    step(&mut b, &mut f);
    let said = beats(&f);
    a.sync_set_foreground(false);
    a.presence_expedite();
    step(&mut a, &mut f);
    assert_eq!(beats(&f), said + 1, "one message: none");
    assert_eq!(f.presence.get("t1"), Some(&None), "the server was told A shows nothing");
    for _ in 0..4 {
        a.presence_expedite();
        step(&mut a, &mut f);
    }
    assert_eq!(beats(&f), said + 1, "then silence");
    // B no longer sees A
    b.presence_expedite();
    step(&mut b, &mut f);
    assert!(b.sync_also_open_on().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn nothing_is_said_during_a_slider_drag() {
    let mut f = Fake::new();
    let (mut a, mut b, root, ids) = two("presence-drag", &mut f);
    show(&mut a, ids[0]);
    show(&mut b, ids[0]);
    step(&mut a, &mut f);
    let said = beats(&f);
    a.begin_interaction("Exposure").unwrap();
    a.presence_expedite();
    step(&mut a, &mut f);
    assert_eq!(beats(&f), said, "held for the drag");
    a.end_interaction().unwrap();
    step(&mut a, &mut f);
    assert_eq!(beats(&f), said + 1, "said once it ends");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_server_without_presence_is_asked_once_and_then_left_alone() {
    let mut f = Fake::new();
    f.no_presence = true;
    let (mut a, root, ids) = first_device("presence-old", &mut f);
    show(&mut a, ids[0]);
    step(&mut a, &mut f);
    for _ in 0..5 {
        a.presence_expedite();
        step(&mut a, &mut f);
    }
    assert_eq!(asks(&f) + beats(&f), 1, "{:?}", f.requests);
    assert!(a.sync_also_open_on().is_empty());
    // the library still syncs
    a.execute("photo.rate", &json!({"rating": 2})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_server_asking_for_calm_is_obeyed_with_growing_waits_and_then_quiet() {
    let mut f = Fake::new();
    let (mut a, mut b, root, ids) = two("presence-calm", &mut f);
    show(&mut a, ids[0]);
    show(&mut b, ids[0]);
    f.presence_status = Some(429);
    step(&mut a, &mut f);
    let first = beats(&f);
    assert_eq!(first, 1);
    // however often the host comes back, the back-off holds the next message
    for _ in 0..5 {
        a.presence_expedite();
        step(&mut a, &mut f);
    }
    assert_eq!(beats(&f), first, "held back after a refusal");
    // sync itself is unaffected
    a.execute("photo.rate", &json!({"rating": 5})).unwrap();
    let st = sync(&mut a, &mut f);
    assert_eq!(st["state"], "idle", "{st}");
    sync(&mut b, &mut f);
    assert_eq!(b.catalog.photo(ids[0]).unwrap().rating, 5);
    let _ = std::fs::remove_dir_all(&root);
}
