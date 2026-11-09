//! Film effects' commands end to end: the layer stack (add, show/hide, duplicate, reorder, shuffle, delete) as undo
//! steps, the indexed sliders, and Copy / Paste carrying or clearing the group.

use serde_json::{Value, json};

use crate::{PhotoId, Session};

fn film(s: &Session) -> lightcraft_develop::Film {
    s.develop_of(s.active().unwrap()).unwrap().film.clone()
}

fn ids(s: &Session) -> Vec<u32> {
    film(s).layers.iter().map(|l| l.id).collect()
}

fn select(s: &mut Session, ids: &[PhotoId]) {
    s.execute("library.select", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()})).unwrap();
}

#[test]
fn the_stack_is_edited_one_undo_step_at_a_time() {
    let mut s = Session::with_demo();
    let a = s.execute("film.add", &json!({"kind": "leak", "seed": 5})).unwrap();
    assert_eq!((a["id"].as_u64(), a["index"].as_u64()), (Some(1), Some(0)));
    s.execute("film.add", &json!({"kind": "dust"})).unwrap();
    assert_eq!(ids(&s), [1, 2]);
    let f = film(&s);
    assert_eq!(f.layers[0].seed, 5);
    assert_eq!(f.layers[1].kind, lightcraft_develop::FilmKind::Dust);
    assert!(f.layers[1].tone > 0.0, "dust starts light, as on a negative scan");

    // the sliders are indexed controls
    s.execute("develop.set", &json!({"control": "filmLayer.1.size", "value": 80})).unwrap();
    assert_eq!(film(&s).layers[1].size, 80.0);

    let d = s.execute("film.duplicate", &json!({"id": 1})).unwrap();
    assert_eq!(d["index"].as_u64(), Some(1), "the copy sits right above the original");
    assert_eq!(ids(&s), [1, 3, 2]);
    assert_ne!(film(&s).layers[1].seed, 5, "a copy draws its own pattern");

    s.execute("film.move", &json!({"id": 1, "delta": 5})).unwrap();
    assert_eq!(ids(&s), [3, 2, 1], "a delta past the end stops at the top");
    s.execute("film.move", &json!({"id": 1, "to": 0})).unwrap();
    assert_eq!(ids(&s), [1, 3, 2]);

    s.execute("film.visible", &json!({"id": 2})).unwrap();
    assert!(!film(&s).layers[2].visible);
    s.execute("film.shuffle", &json!({"id": 2, "seed": 99})).unwrap();
    assert_eq!(film(&s).layers[2].seed, 99);

    s.execute("film.delete", &json!({"id": 3})).unwrap();
    assert_eq!(ids(&s), [1, 2]);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(ids(&s), [1, 3, 2], "a delete comes back with one undo");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_ne!(film(&s).layers[2].seed, 99, "and the shuffle before it");

    // bad requests are errors, not panics
    assert!(s.execute("film.add", &json!({"kind": "smudge"})).is_err());
    assert!(s.execute("film.delete", &json!({"id": 404})).is_err());
    assert!(s.execute("film.move", &json!({"id": 1})).is_err());
    assert!(s.execute("film.add", &json!({"kind": "leak", "seed": -1})).is_err());
    assert!(s.execute("film.move", &json!({"id": 1, "delta": i64::MIN})).is_ok());

    // at most eight layers
    while film(&s).layers.len() < lightcraft_develop::MAX_FILM_LAYERS {
        s.execute("film.add", &json!({"kind": "scratches"})).unwrap();
    }
    assert!(s.execute("film.add", &json!({"kind": "leak"})).is_err());
    assert!(s.execute("film.duplicate", &json!({"id": 1})).is_err());

    // the list command answers the group
    let l = s.execute("film.list", &json!({})).unwrap();
    assert_eq!(l["layers"].as_array().map(Vec::len), Some(lightcraft_develop::MAX_FILM_LAYERS));

    // Reset Section clears the whole group
    s.execute("develop.resetSection", &json!({"section": "film"})).unwrap();
    assert!(film(&s).is_default());
}

#[test]
fn copy_and_paste_carry_the_film_or_clear_it() {
    let mut s = Session::with_demo();
    let photos: Vec<PhotoId> = s.visible_cloned().into_iter().take(3).collect();
    let (src, plain, target) = (photos[0], photos[1], photos[2]);
    select(&mut s, &[src]);
    s.execute("film.add", &json!({"kind": "burn", "seed": 11})).unwrap();
    s.execute("develop.merge", &json!({"settings": {"film": {"frame": "strip", "vary": false}}})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["film"]})).unwrap();
    select(&mut s, &[target]);
    s.execute("develop.paste", &json!({})).unwrap();
    let pasted = s.develop_of(target).unwrap().film.clone();
    assert_eq!(pasted.frame, lightcraft_develop::FilmFrame::Strip);
    assert_eq!(pasted.layers.len(), 1);
    assert_eq!(pasted.layers[0].seed, 11, "the seed travels as it is: the renderer varies it per photo");

    // copying a photo without film effects and pasting clears them
    select(&mut s, &[plain]);
    s.execute("develop.copy", &json!({"groups": ["film"]})).unwrap();
    select(&mut s, &[target]);
    s.execute("develop.paste", &json!({})).unwrap();
    assert!(s.develop_of(target).unwrap().film.is_default());
    let json: Value = s.develop_of(target).unwrap().to_json();
    assert!(json.get("film").is_none(), "a cleared group leaves the JSON as it was");
}
