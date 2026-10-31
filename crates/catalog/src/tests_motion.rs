//! Live Photo clips that are videos of their own: paired with their stills by name and time, left out of views.

use super::tests::photo;
use super::*;

fn clip(c: &mut Catalog, name: &str, date: &str, seconds: f64) -> PhotoId {
    let id = photo(c, name, date);
    let mut p = (**c.photo(id).unwrap()).clone();
    p.kind = MediaKind::Video;
    p.duration = Some(seconds);
    // (an op that replaces the photo whole: a test's way to make a video)
    c.apply(Op::RemovePhoto { id }).unwrap();
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

fn ids(c: &Catalog, f: &Filter) -> Vec<PhotoId> {
    let mut v = c.query(f, &Sort::default());
    v.sort();
    v
}

#[test]
fn a_short_clip_with_its_stills_name_is_its_motion_and_leaves_the_views() {
    let mut c = Catalog::new();
    let still = photo(&mut c, "IMG_0001.HEIC", "2026-02-07T06:28:10");
    let motion = clip(&mut c, "img_0001.MOV", "2026-02-07T06:28:11", 2.1);
    let other = photo(&mut c, "IMG_0002.HEIC", "2026-02-07T06:30:00");

    assert_eq!(c.motion_pairs().clip_of(still), Some(motion));
    assert_eq!(c.motion_pairs().still_of(motion), Some(still));
    assert!(c.is_motion_clip(motion) && !c.is_motion_clip(still));
    assert_eq!(ids(&c, &Filter::default()), [still, other], "the clip is not listed");
    assert_eq!(ids(&c, &Filter { motion_clips: true, ..Default::default() }), [still, motion, other], "unless asked for");
}

#[test]
fn what_is_not_certain_stays_a_video() {
    let mut c = Catalog::new();
    let long = photo(&mut c, "IMG_0003.HEIC", "2026-02-07T06:28:10");
    let long_clip = clip(&mut c, "IMG_0003.MOV", "2026-02-07T06:28:10", 40.0);
    let far = photo(&mut c, "IMG_0004.HEIC", "2026-02-07T06:28:10");
    let far_clip = clip(&mut c, "IMG_0004.MOV", "2026-02-07T09:00:00", 2.0);
    let _ = (long, far);
    assert!(!c.is_motion_clip(long_clip), "a 40 s clip is a video");
    assert!(!c.is_motion_clip(far_clip), "taken hours apart");

    // two stills of one name (two folders) and one clip: no way to know which
    let a = photo(&mut c, "IMG_0005.HEIC", "2026-02-07T07:00:00");
    let b = photo(&mut c, "IMG_0005.JPG", "2026-02-07T07:00:01");
    let both = clip(&mut c, "IMG_0005.MOV", "2026-02-07T07:00:00", 2.0);
    let _ = (a, b);
    assert!(!c.is_motion_clip(both));
}

#[test]
fn a_missing_time_does_not_rule_a_pair_out_and_a_deleted_still_unpairs() {
    let mut c = Catalog::new();
    let still = photo(&mut c, "IMG_0006.HEIC", "2026-02-07T07:00:00");
    c.apply(Op::SetCaptured { id: still, captured: None }).unwrap();
    let motion = clip(&mut c, "IMG_0006.MOV", "2026-02-07T07:00:00", 3.0);
    assert!(c.is_motion_clip(motion));

    c.apply(Op::SetDeleted { id: still, deleted: true }).unwrap();
    assert!(!c.is_motion_clip(motion), "without its still the clip is a video again");
    assert_eq!(ids(&c, &Filter::default()), [motion]);
}

#[test]
fn the_pairs_follow_the_catalog() {
    let mut c = Catalog::new();
    let motion = clip(&mut c, "IMG_0007.MOV", "2026-02-07T07:00:00", 3.0);
    assert!(c.motion_pairs().is_empty() && !c.is_motion_clip(motion));
    // the still arrives later (a sync pull): they pair
    let still = photo(&mut c, "IMG_0007.HEIC", "2026-02-07T07:00:00");
    assert_eq!(c.motion_pairs().still_of(motion), Some(still));
}
