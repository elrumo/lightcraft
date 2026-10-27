use super::session::frame_to;
use super::*;

/// A picture whose pixel at (x, y) says so, to see which part a cut kept.
fn numbered(w: usize, h: usize) -> Rgba8 {
    Rgba8 { width: w, height: h, data: (0..w * h).map(|i| [(i % w) as u8, (i / w) as u8, 0, 255]).collect() }
}

/// A 4:3 JPEG of a 16:9 shot: the middle 27 rows of 36 are the frame, 4 above and 5 below cut away.
#[test]
fn a_whole_sensor_picture_is_cut_to_the_middle_of_a_wider_frame() {
    let r = frame_to(numbered(48, 36), 16.0 / 9.0);
    assert_eq!((r.width, r.height), (48, 27));
    assert_eq!(r.data[0][..2], [0, 4], "the first kept row is row 4");
    assert_eq!(r.data[26 * 48 + 47][..2], [47, 30], "and the last is row 30");
    // and to the middle of a narrower one
    let r = frame_to(numbered(48, 36), 1.0);
    assert_eq!((r.width, r.height), (36, 36));
    assert_eq!(r.data[0][..2], [6, 0]);
}

#[test]
fn a_picture_that_is_not_a_crop_of_the_frame_is_left_alone() {
    let same = |aspect: f64| {
        let r = frame_to(numbered(48, 36), aspect);
        (r.width, r.height) == (48, 36)
    };
    assert!(same(4.0 / 3.0), "the same shape");
    assert!(same(1.31), "within what resizing rounds");
    assert!(same(3.0 / 4.0), "turned the other way");
    assert!(same(5.0), "a frame that keeps under half of it");
    assert!(same(0.0) && same(f64::NAN) && same(f64::INFINITY) && same(-1.5), "no frame to cut to");
    let empty = frame_to(Rgba8 { width: 0, height: 0, data: Vec::new() }, 1.5);
    assert_eq!((empty.width, empty.height), (0, 0));
}

/// A GH7-like raw: 16:9 as catalogued, its camera JPEG 4:3. The stand-in the grid and the swipe show until the render
/// comes has the photo's own shape (before, it was 4:3 and lost its top and bottom when the page settled).
#[test]
fn the_stand_in_of_an_unedited_raw_has_the_shape_of_its_frame() {
    let mut s = crate::Session::new();
    let mut p = Photo::new(PhotoId(1), Source::File { path: "P1002298.RW2".into() }, "P1002298.RW2", "RW2", 5776, 3248, "2026-01-01");
    p.kind = MediaKind::Raw;
    p.develop = Arc::new(p.camera_defaults());
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    s.media.preview_loader = Some(Arc::new(|_: &str, _: usize| Some(numbered(48, 36))));
    let shape = |q: Option<QuickJob>| {
        let r = q.expect("an unedited raw has a stand-in").run().rendered.unwrap().image;
        (r.width, r.height)
    };
    assert_eq!(shape(s.quick_view_job(PhotoId(1), 1024, true)), (48, 27));
    let job = s.thumb_job(PhotoId(1), 256).unwrap();
    assert_eq!(shape(s.quick_thumb_job(&job)), (48, 27));
    // a photo whose frame is the camera's own (4:3) keeps its JPEG whole
    let mut whole = Photo::new(PhotoId(2), Source::File { path: "P1002299.RW2".into() }, "P1002299.RW2", "RW2", 5776, 4336, "2026-01-01");
    whole.kind = MediaKind::Raw;
    whole.develop = Arc::new(whole.camera_defaults());
    s.catalog.apply(Op::AddPhoto { photo: Box::new(whole) }).unwrap();
    assert_eq!(shape(s.quick_view_job(PhotoId(2), 1024, true)), (48, 36));
}
