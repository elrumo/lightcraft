//! Possibly sensitive photos: the background pass scores photos with the classifier, the flagged ones are
//! listed best first and can all be hidden at once; blurring them is a synced choice.

use std::sync::Arc;

use lightcraft_raster::Rgba8;
use lightcraft_vision::{Classifier, Error};
use serde_json::json;

use crate::{LibrarySource, Session};

/// Flags every photo whose top-left pixel is brighter than mid-grey (so some are, some aren't).
struct Bright;

impl Classifier for Bright {
    fn engine(&self) -> &str {
        "test-bright"
    }
    fn score(&self, img: &Rgba8) -> Result<f32, Error> {
        let p = img.data.first().copied().unwrap_or([0; 4]);
        Ok(if u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 3 * 128 { 0.95 } else { 0.05 })
    }
}

/// Flags every photo.
struct All;

impl Classifier for All {
    fn engine(&self) -> &str {
        "test-all"
    }
    fn score(&self, _: &Rgba8) -> Result<f32, Error> {
        Ok(0.9)
    }
}

fn session(c: Arc<dyn Classifier>) -> Session {
    let mut s = Session::with_demo();
    s.vision.set_classifier(c);
    s.vision.sensitive = true;
    s.vision.text_edge = 192;
    s
}

#[test]
fn nothing_is_checked_until_it_is_turned_on() {
    let mut s = session(Arc::new(All));
    s.vision.sensitive = false;
    let _ = s.vision_index(None, true);
    assert!(s.sensitive_photos().is_empty());
}

#[test]
fn flagged_photos_are_listed_and_hide_all_takes_them_out_of_the_library() {
    let mut s = session(Arc::new(All));
    let total = s.visible().len();
    s.vision_index(None, true).unwrap();
    assert_eq!(s.sensitive_photos().len(), total, "every photo flagged");
    let rows = s.execute("catalog.query", &json!({"limit": 5})).unwrap();
    assert_eq!(rows["photos"][0]["sensitive"], true, "{rows}");
    s.execute("library.source", &json!({"kind": "sensitive"})).unwrap();
    assert_eq!(s.visible().len(), total);
    let r = s.execute("library.hideSensitive", &json!({})).unwrap();
    assert_eq!(r["changed"].as_u64(), Some(total as u64));
    assert!(s.visible().is_empty(), "hidden: not listed any more");
    assert!(s.sensitive_photos().is_empty(), "nor counted");
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    assert!(s.visible().is_empty());
    // one undo brings them all back
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.visible().len(), total);
}

#[test]
fn only_the_photos_scored_above_the_cut_off_are_flagged() {
    let mut s = session(Arc::new(Bright));
    let total = s.visible().len();
    s.vision_index(None, true).unwrap();
    let n = s.sensitive_photos().len();
    assert!(n < total, "{n} of {total}");
    assert!(s.sensitive_photos().iter().all(|(_, score)| *score >= crate::vision::SENSITIVE_MIN));
    s.execute("library.source", &json!({"kind": "sensitive"})).unwrap();
    assert_eq!(s.source, LibrarySource::Sensitive);
    assert_eq!(s.visible().len(), n);
}

#[test]
fn blurring_is_on_until_turned_off() {
    let mut s = Session::with_demo();
    assert!(s.blur_sensitive);
    assert_eq!(s.execute("library.state", &json!({})).unwrap()["blurSensitive"], true);
    s.execute("library.setBlurSensitive", &json!({"on": false})).unwrap();
    assert!(!s.blur_sensitive);
}

#[test]
fn the_download_needs_the_users_yes() {
    let mut s = Session::with_demo();
    let e = s.execute("vision.sensitive.download", &json!({})).unwrap_err();
    assert!(e.to_string().contains("acknowledged"), "{e}");
}

#[test]
fn a_photo_can_be_marked_sensitive_without_the_model() {
    let mut s = Session::with_demo();
    let id = s.visible()[0];
    assert!(s.sensitive_photos().is_empty());
    let r = s.execute("photo.markSensitive", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(r["changed"], 1);
    assert_eq!(s.sensitive_photos().iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![id]);
    let rows = s.execute("catalog.query", &json!({"limit": 1})).unwrap();
    assert_eq!(rows["photos"][0]["sensitive"], true, "{rows}");
    // marking again changes nothing; marking not sensitive takes it out
    assert_eq!(s.execute("photo.markSensitive", &json!({"ids": [id.0]})).unwrap()["changed"], 0);
    assert_eq!(s.execute("photo.unmarkSensitive", &json!({"ids": [id.0]})).unwrap()["changed"], 1);
    assert!(s.sensitive_photos().is_empty());
}

#[test]
fn marking_not_sensitive_beats_the_model() {
    let mut s = session(Arc::new(All));
    let total = s.visible().len();
    s.vision_index(None, true).unwrap();
    let id = s.visible()[0];
    s.execute("photo.unmarkSensitive", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(s.sensitive_photos().len(), total - 1);
    assert!(!s.sensitive_ids().contains(&id));
    s.execute("photo.markSensitive", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(s.sensitive_photos().len(), total);
}
