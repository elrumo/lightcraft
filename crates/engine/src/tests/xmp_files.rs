//! XMP files imported into and exported from photos: `photo.inspectXmp`, `photo.applyXmp`, `photo.exportXmp`.

use serde_json::{Value, json};

use super::xmp::{temp_dir, write_png};
use crate::Session;

/// A sidecar in the shape Lightroom writes for a rendered file (values written for this test): sliders it carries
/// over, a profile and a mask kind it doesn't, bookkeeping, a superseded older slider, a field it doesn't know, a crop,
/// a curve and a rating.
const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
  xmp:Rating="4" crs:Version="17.0" crs:ProcessVersion="15.4" crs:WhiteBalance="Custom" crs:IncrementalTemperature="+12" crs:IncrementalTint="-4"
  crs:Exposure2012="+0.65" crs:Contrast2012="0" crs:Highlights2012="-40" crs:Texture="+10" crs:Brightness="+50"
  crs:CameraProfile="Adobe Portrait" crs:EnableColorAdjustments="True" crs:ToneCurveName2012="Custom" crs:FutureSlider="12"
  crs:HasCrop="True" crs:CropLeft="0.1" crs:CropTop="0" crs:CropRight="0.9" crs:CropBottom="1" crs:CropAngle="0">
 <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>128, 140</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
 <crs:MaskGroupBasedCorrections><rdf:Seq>
  <rdf:li><rdf:Description crs:What="Correction" crs:CorrectionAmount="1" crs:LocalExposure2012="-0.25">
   <crs:CorrectionMasks><rdf:Seq>
    <rdf:li crs:What="Mask/Gradient" crs:MaskBlendMode="0" crs:FullX="0.5" crs:FullY="0" crs:ZeroX="0.5" crs:ZeroY="0.6"/>
    <rdf:li crs:What="Mask/Image" crs:MaskSubType="9"/>
   </rdf:Seq></crs:CorrectionMasks>
  </rdf:Description></rdf:li>
 </rdf:Seq></crs:MaskGroupBasedCorrections>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

/// A session with `n` photos imported from PNG files (their frame known), all selected.
fn photos(tag: &str, n: u8) -> (Session, Vec<u64>, std::path::PathBuf) {
    let dir = temp_dir(tag);
    for i in 0..n {
        write_png(&dir.join(format!("p{i}.png")), i);
    }
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut ids: Vec<u64> = s.catalog.photos().map(|p| p.id.0).collect();
    ids.sort_unstable();
    s.execute("library.select", &json!({"ids": ids})).unwrap();
    (s, ids, dir)
}

fn row<'a>(r: &'a Value, field: &str) -> &'a Value {
    r["fields"].as_array().unwrap().iter().find(|f| f["field"] == field).unwrap_or_else(|| panic!("no row {field}: {r}"))
}

#[test]
fn inspect_lists_every_field_and_what_becomes_of_it() {
    let (mut s, ids, dir) = photos("xmp-inspect", 1);
    let before = s.catalog.photo(crate::PhotoId(ids[0])).unwrap().develop.clone();
    let commits = s.commits();
    let r = s.execute("photo.inspectXmp", &json!({"xmp": LIGHTROOM, "id": ids[0]})).unwrap();
    assert_eq!(r["kind"], "lightroom");
    let exposure = row(&r, "Exposure2012");
    assert_eq!((exposure["status"].as_str(), exposure["section"].as_str()), (Some("applied"), Some("Light")));
    assert_eq!(exposure["to"], json!([{"path": "light.exposure", "label": "Exposure"}]));
    assert_eq!((exposure["value"].as_str(), exposure["default"].as_bool()), (Some("+0.65"), Some(false)));
    assert_eq!(row(&r, "Contrast2012")["default"], true, "0 sets nothing new");
    let temp = row(&r, "IncrementalTemperature");
    assert_eq!((temp["to"][0]["path"].as_str(), temp["section"].as_str()), (Some("wb.temp"), Some("White Balance")));
    assert_eq!(row(&r, "WhiteBalance")["to"][0]["label"], "White Balance");
    let curve = row(&r, "ToneCurvePV2012");
    assert_eq!((curve["to"][0]["path"].as_str(), curve["count"].as_u64()), (Some("curve.master"), Some(3)));
    assert_eq!(row(&r, "CropLeft")["to"][0]["path"], "crop.geometry", "the file's frame is known: the crop applies");
    let masks = row(&r, "MaskGroupBasedCorrections");
    assert_eq!((masks["to"][0]["path"].as_str(), masks["count"].as_u64(), masks["section"].as_str()), (Some("masks"), Some(1), Some("Masks")));
    let mask = row(&r, "Mask");
    assert_eq!((mask["status"].as_str(), mask["reason"].as_str(), mask["detail"].as_str()), (Some("notApplied"), Some("mask"), Some("AI mask (9)")));
    let profile = row(&r, "CameraProfile");
    assert_eq!(
        (profile["status"].as_str(), profile["reason"].as_str(), profile["detail"].as_str()),
        (Some("notApplied"), Some("profile"), Some("Adobe Portrait"))
    );
    let unknown = row(&r, "FutureSlider");
    assert_eq!(
        (unknown["status"].as_str(), unknown["reason"].as_str(), unknown["section"].as_str()),
        (Some("notApplied"), Some("unsupported"), Some("Other"))
    );
    for (field, reason) in
        [("Version", "bookkeeping"), ("EnableColorAdjustments", "panelSwitch"), ("ToneCurveName2012", "bookkeeping"), ("Brightness", "unused")]
    {
        let f = row(&r, field);
        assert_eq!((f["status"].as_str(), f["reason"].as_str()), (Some("info"), Some(reason)), "{field}");
    }
    let rating = row(&r, "Rating");
    assert_eq!((rating["value"].as_str(), rating["reason"].as_str(), rating["section"].as_str()), (Some("4"), Some("metadata"), Some("Metadata")));
    let c = &r["counts"];
    let total = c["applied"].as_u64().unwrap() + c["notApplied"].as_u64().unwrap() + c["info"].as_u64().unwrap();
    assert_eq!(total as usize, r["fields"].as_array().unwrap().len());
    assert_eq!(c["notApplied"], 3, "{r}");
    // looking changes nothing
    assert_eq!((s.commits(), &s.catalog.photo(crate::PhotoId(ids[0])).unwrap().develop), (commits, &before));
    // a file that is no XMP is an error, never a panic
    assert!(s.execute("photo.inspectXmp", &json!({"xmp": "\u{0}not xml <<"})).is_err());
    assert!(s.execute("photo.inspectXmp", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_puts_the_edit_on_every_photo_in_one_undo_step() {
    let (mut s, ids, dir) = photos("xmp-apply", 2);
    let r = s.execute("photo.applyXmp", &json!({"xmp": LIGHTROOM})).unwrap();
    assert_eq!(r["applied"], json!(ids), "{r}");
    for id in &ids {
        let p = s.catalog.photo(crate::PhotoId(*id)).unwrap();
        assert_eq!((p.develop.light.exposure, p.develop.light.highlights, p.develop.effects.texture), (0.65, -40.0, 10.0));
        assert_eq!((p.develop.crop.geometry.rect.x0, p.develop.masks.len()), (0.1, 1));
        assert_eq!(p.rating, 0, "an edit is imported, never the metadata beside it");
        assert_eq!(p.history.last().map(|h| h.label.as_str()), Some("Import XMP"));
    }
    // the same file again changes nothing
    let r = s.execute("photo.applyXmp", &json!({"xmp": LIGHTROOM})).unwrap();
    assert_eq!((r["applied"].as_array().unwrap().len(), r["unchanged"].as_array().unwrap().len()), (0, 2));
    s.execute("edit.undo", &json!({})).unwrap();
    for id in &ids {
        let p = s.catalog.photo(crate::PhotoId(*id)).unwrap();
        assert_eq!((p.develop.light.exposure, p.develop.masks.len()), (0.0, 0), "one undo takes it off every photo");
    }
    // a packet with no edit is an error and changes nothing
    let empty = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="2"/></rdf:RDF></x:xmpmeta>"#;
    let commits = s.commits();
    assert!(s.execute("photo.applyXmp", &json!({"xmp": empty})).is_err());
    assert_eq!(s.commits(), commits);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_exported_edit_round_trips() {
    let (mut s, ids, dir) = photos("xmp-export", 2);
    s.execute("library.select", &json!({"ids": [ids[0]]})).unwrap();
    s.execute("develop.set", &json!({"values": {"light.exposure": 1.25, "color.vibrance": 30, "crop.angle": 3}})).unwrap();
    let out = dir.join("exported.xmp");
    let r = s.execute("photo.exportXmp", &json!({"id": ids[0], "path": out.to_string_lossy()})).unwrap();
    assert_eq!(r["name"], "p0.xmp");
    let xmp = std::fs::read_to_string(&out).unwrap();
    assert_eq!(r["xmp"], xmp.as_str());

    let report = s.execute("photo.inspectXmp", &json!({"xmp": xmp, "id": ids[1]})).unwrap();
    assert_eq!(report["kind"], "lightcraft");
    let exposure = row(&report, "light.exposure");
    assert_eq!(
        (exposure["status"].as_str(), exposure["to"][0]["label"].as_str(), exposure["value"].as_str()),
        (Some("applied"), Some("Exposure"), Some("1.25"))
    );
    assert_eq!(report["counts"]["notApplied"], 0);

    s.execute("photo.applyXmp", &json!({"xmp": xmp, "ids": [ids[1]]})).unwrap();
    let (a, b) = (s.catalog.photo(crate::PhotoId(ids[0])).unwrap(), s.catalog.photo(crate::PhotoId(ids[1])).unwrap());
    assert_eq!(a.develop, b.develop, "LightCraft's own settings come back exactly");
    let _ = std::fs::remove_dir_all(&dir);
}
