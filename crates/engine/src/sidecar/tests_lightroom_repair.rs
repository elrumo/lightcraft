//! [`super::repair_op`]: what an import of a Lightroom sidecar from before 2026-10-10 left wrong, put right.

use std::sync::Arc;

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::{DevelopSettings, WbMode, WbScale};
use lightcraft_geom::Orientation;

use super::{StoredFrame, repair_op};

/// The sidecar of the Canon EOS R6 photo of the report (Lightroom 15.4): Custom 5050 K / +27, cropped 9:16 on the stored image.
const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:WhiteBalance="Custom" crs:Temperature="5050" crs:Tint="+27" crs:Exposure2012="+1.37" crs:HasCrop="True" crs:CropTop="0.121077" crs:CropLeft="0" crs:CropBottom="0.964827" crs:CropRight="1" crs:CropAngle="0"/></rdf:RDF></x:xmpmeta>"#;

/// The file is stored 5472 x 3648 and shown turned (EXIF 8).
const FRAME: StoredFrame = StoredFrame { orientation: Orientation::Rotate270, width: 5472.0, height: 3648.0 };

/// The photo as the old import left it: Lightroom's numbers without a scale, the crop's edges unframed.
fn imported() -> Photo {
    let mut p = Photo::new(PhotoId(5), Source::File { path: "/x/2V2A9273.CR3".into() }, "2V2A9273.CR3", "CR3", 3648, 5472, "2026-10-09T10:42:32");
    p.kind = MediaKind::Raw;
    p.meta.camera = "Canon EOS R6".into();
    p.as_shot_wb = Some((4285.0, 24.0));
    let mut d = DevelopSettings::default();
    d.wb.mode = WbMode::Custom;
    (d.wb.temp, d.wb.tint, d.wb.scale) = (5050.0, 27.0, WbScale::Legacy);
    d.light.exposure = 1.37;
    d.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.0, y0: 0.121077, x1: 1.0, y1: 0.964827 };
    p.develop = Arc::new(d);
    p
}

fn repaired(p: &Photo, frame: Option<StoredFrame>) -> Option<DevelopSettings> {
    match repair_op(p, XMP, frame)? {
        Op::SetDevelop { id, settings, label, edited } => {
            assert_eq!((id, label.as_str(), edited), (p.id, super::REPAIR_LABEL, p.edited.clone()));
            Some((*settings).clone())
        }
        op => panic!("{op:?}"),
    }
}

#[test]
fn an_old_lightroom_import_gets_its_kelvin_white_and_its_crop_on_the_stored_image() {
    let d = repaired(&imported(), Some(FRAME)).expect("both were left as the old import wrote them");
    assert_eq!((d.wb.mode, d.wb.temp, d.wb.tint, d.wb.scale), (WbMode::Custom, 5050.0, 27.0, WbScale::Kelvin));
    let r = d.crop.geometry.rect;
    assert_eq!([r.x0, r.y0, r.x1, r.y1].map(|v| (v * 1e6).round() / 1e6), [0.121077, 0.0, 0.964827, 1.0], "3078 x 5472, as Lightroom exports it");
    // the rest of the photo's settings stay as they are
    assert_eq!(d.light.exposure, 1.37);
}

#[test]
fn what_was_changed_since_the_import_is_kept() {
    // a white moved on the photo since (on the relative scale, as it was then) and a crop redrawn
    let mut p = imported();
    let mut d = (*p.develop).clone();
    d.wb.temp = 5600.0;
    d.crop.geometry.rect.y1 = 0.9;
    p.develop = Arc::new(d);
    assert_eq!(repaired(&p, Some(FRAME)), None);
    // a photo already read with the scale: nothing to do
    let mut p = imported();
    let mut d = (*p.develop).clone();
    d.wb.scale = WbScale::Kelvin;
    d.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.121077, y0: 0.0, x1: 0.964827, y1: 1.0 };
    p.develop = Arc::new(d);
    assert_eq!(repaired(&p, Some(FRAME)), None);
}

#[test]
fn without_the_files_frame_only_the_white_is_repaired() {
    let d = repaired(&imported(), None).unwrap();
    assert_eq!(d.wb.scale, WbScale::Kelvin);
    assert_eq!(d.crop.geometry, imported().develop.crop.geometry, "the crop is held where it is rather than guessed");
}
