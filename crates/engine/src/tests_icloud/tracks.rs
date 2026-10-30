//! Recorded tracks through iCloud and a cloud folder: one record per chunk of the `tracks` document.

use super::*;

const T0: i64 = 1_777_629_600_000;

fn record(d: &mut Device, t0: i64, n: usize) -> String {
    let points: Vec<Value> = (0..n).map(|i| json!([t0 + i as i64 * 1000, 46.0 + i as f64 * 1e-4, 7.0 + (i as f64 * 0.7).sin() * 1e-3])).collect();
    d.s.execute("track.create", &json!({"points": points})).unwrap()["session"].as_str().unwrap().to_string()
}

fn sessions(d: &mut Device) -> Vec<(String, Option<String>)> {
    let list = d.s.execute("track.list", &json!({})).unwrap();
    let mut v: Vec<(String, Option<String>)> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["session"].as_str().unwrap().to_string(), t["name"].as_str().map(str::to_string)))
        .collect();
    v.sort();
    v
}

#[test]
fn tracks_travel_through_icloud_one_record_per_chunk() {
    let root = temp_dir("tracks");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    // more than 20 000 points: two chunks, two records
    let long = record(&mut a, T0, 30_000);
    a.sync(&mut cloud);
    let records: Vec<&Stored> = cloud.records.values().filter(|r| r.kind == "tracks").collect();
    assert_eq!(records.len(), 2, "{:?}", cloud.records.keys().collect::<Vec<_>>());
    assert!(records.iter().all(|r| r.data.len() < 8 << 20), "a chunk fits a record");
    b.sync(&mut cloud);
    assert_eq!(sessions(&mut b), vec![(long.clone(), None)]);
    let got = b.s.execute("track.get", &json!({"session": long})).unwrap();
    assert_eq!((got["points"].as_u64(), got["chunks"].as_array().map(Vec::len)), (Some(30_000), Some(2)));
    // added on both: all kept
    b.at(2_000);
    let on_b = record(&mut b, T0 + 7 * 86_400_000, 50);
    a.at(2_000);
    let on_a = record(&mut a, T0 + 9 * 86_400_000, 50);
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    let mut all = vec![long.clone(), on_a.clone(), on_b.clone()];
    all.sort();
    for d in [&mut a, &mut b] {
        assert_eq!(sessions(d).into_iter().map(|s| s.0).collect::<Vec<_>>(), all);
    }
    // renamed on b, later
    b.at(3_000);
    b.s.execute("track.rename", &json!({"session": long, "name": "Two days"})).unwrap();
    b.sync(&mut cloud);
    a.sync(&mut cloud);
    assert!(sessions(&mut a).contains(&(long.clone(), Some("Two days".into()))));
    // deleted on a: gone on b, and from the cloud
    a.at(4_000);
    a.s.execute("track.delete", &json!({"session": long})).unwrap();
    a.sync(&mut cloud);
    b.sync(&mut cloud);
    assert!(sessions(&mut b).iter().all(|s| s.0 != long));
    assert_eq!(cloud.records.values().filter(|r| r.kind == "tracks").count(), 2);
    for d in [&mut a, &mut b] {
        assert_eq!(d.s.icloud_changes(), Default::default(), "nothing left to send");
    }
}

#[test]
fn a_track_record_that_is_not_one_is_left_out() {
    let root = temp_dir("tracks-hostile");
    let mut cloud = Cloud::default();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    let good = record(&mut a, T0, 20);
    a.sync(&mut cloud);
    let real = cloud.records.values().find(|r| r.kind == "tracks").unwrap().clone();
    let mut put = |key: &str, data: String| {
        let name = crate::icloud::record_name("tracks", key);
        cloud.records.insert(name.clone(), Stored { kind: "tracks".into(), key: key.into(), data, modified: 5, tag: 1 });
        cloud.change(&name, false);
    };
    let mut liar: Value = serde_json::from_str(&real.data).unwrap();
    liar["id"] = json!("s1111111111111111-0");
    liar["session"] = json!("s1111111111111111");
    liar["data"] = json!("AAAA");
    put("s1111111111111111-0", liar.to_string());
    put("s3333333333333333-0", "{not json".into());
    put("../../escape", json!({"id": "../../escape", "session": "x", "index": 0, "fmt": 1}).to_string());
    b.sync(&mut cloud);
    assert_eq!(sessions(&mut b), vec![(good, None)], "only the real one");
}

#[test]
fn tracks_travel_through_a_cloud_folder() {
    let root = temp_dir("tracks-folder");
    let shared = root.join("Dropbox");
    std::fs::create_dir_all(&shared).unwrap();
    let (mut a, mut b) = (Device::new(&root, "a"), Device::new(&root, "b"));
    let t = record(&mut a, T0, 500);
    folder_pass(&mut a, &shared);
    folder_pass(&mut b, &shared);
    assert_eq!(sessions(&mut b), vec![(t.clone(), None)]);
    b.at(2_000);
    b.s.execute("track.rename", &json!({"session": t, "name": "By folder"})).unwrap();
    folder_pass(&mut b, &shared);
    folder_pass(&mut a, &shared);
    assert_eq!(sessions(&mut a), vec![(t.clone(), Some("By folder".into()))]);
    a.at(3_000);
    a.s.execute("track.delete", &json!({"session": t})).unwrap();
    folder_pass(&mut a, &shared);
    folder_pass(&mut b, &shared);
    assert!(sessions(&mut b).is_empty(), "a removal sticks");
}
