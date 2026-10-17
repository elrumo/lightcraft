//! The Hidden album: hidden photos leave every view, tally and album count unless the view asks
//! for them, survive undo and merges, and sync as their own key.

use super::tests::photo;
use super::*;

fn ids(c: &Catalog, f: &Filter) -> Vec<PhotoId> {
    let mut v = c.query(f, &Sort::default());
    v.sort();
    v
}

#[test]
fn a_hidden_photo_is_left_out_unless_the_view_asks_for_it() {
    let mut c = Catalog::new();
    let a = photo(&mut c, "a.jpg", "2026-04-01T10:00:00");
    let b = photo(&mut c, "b.jpg", "2026-04-02T10:00:00");
    let inverse = c.apply(Op::SetHidden { id: a, hidden: true }).unwrap();
    assert!(c.photo(a).unwrap().hidden && !c.photo(a).unwrap().visible() && c.photo(a).unwrap().in_library());

    assert_eq!(ids(&c, &Filter::default()), [b], "locked: left out");
    assert_eq!(ids(&c, &Filter { hidden: HiddenMode::Include, ..Default::default() }), [a, b], "Show Hidden");
    assert_eq!(ids(&c, &Filter { hidden: HiddenMode::Only, ..Default::default() }), [a], "the Hidden album");
    assert_eq!(c.date_groups().iter().map(|g| g.count).sum::<usize>(), 1, "tallies don't count it");

    c.apply(inverse).unwrap();
    assert_eq!(ids(&c, &Filter::default()), [a, b], "undo shows it again");
}

#[test]
fn a_filter_from_outside_never_shows_hidden_photos() {
    // what a filter preset, a saved view or a `catalog.query` caller sends can't name `hidden`
    let f: Filter = serde_json::from_value(serde_json::json!({"hidden": "include", "text": ""})).unwrap();
    assert_eq!(f.hidden, HiddenMode::Exclude);
    let v = serde_json::to_value(Filter { hidden: HiddenMode::Only, ..Default::default() }).unwrap();
    assert!(v.get("hidden").is_none(), "{v}");
}

#[test]
fn albums_hold_hidden_photos_but_dont_count_them() {
    let mut c = Catalog::new();
    let a = photo(&mut c, "a.jpg", "2026-04-01T10:00:00");
    let b = photo(&mut c, "b.jpg", "2026-04-02T10:00:00");
    let album = c.alloc_album_id();
    c.apply(Op::AddAlbum { album: Album { photos: vec![a, b], ..Album::new(album, "Trip") } }).unwrap();
    let smart = c.alloc_album_id();
    let rules = Filter { date: Some("2026-04".into()), ..Default::default() };
    c.apply(Op::AddAlbum { album: Album { smart: Some(Box::new(rules)), ..Album::new(smart, "April") } }).unwrap();
    c.apply(Op::SetHidden { id: a, hidden: true }).unwrap();

    for id in [album, smart] {
        assert_eq!(c.album_count(id), 1, "{id:?}");
        assert_eq!(ids(&c, &Filter { album: Some(id), ..Default::default() }), [b]);
        assert_eq!(ids(&c, &Filter { album: Some(id), hidden: HiddenMode::Include, ..Default::default() }), [a, b], "Show Hidden in an album");
    }
}

#[test]
fn hiding_syncs_as_its_own_key() {
    let op = Op::SetHidden { id: PhotoId(7), hidden: true };
    assert_eq!(sync::key_of(&op).as_deref(), Some("p7.hidden"));
    // and a whole photo carries it
    let mut c = Catalog::new();
    let a = photo(&mut c, "a.jpg", "2026-04-01T10:00:00");
    c.apply(op.clone()).ok();
    c.apply(Op::SetHidden { id: a, hidden: true }).unwrap();
    let json = serde_json::to_value(c.photo(a).unwrap().as_ref()).unwrap();
    assert_eq!(json["hidden"], true);
    let shown = serde_json::to_value(Photo::new(a, Source::Demo { scene: 1 }, "b", "JPEG", 1, 1, "")).unwrap();
    assert!(shown.get("hidden").is_none(), "nothing written for a shown photo");
}
