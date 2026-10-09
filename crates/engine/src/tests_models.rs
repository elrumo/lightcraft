//! The AI models as one list (Settings ▸ AI Models): what each is, where it stands, turning it
//! off, deleting it, downloading it. No test touches the internet.

use serde_json::json;

use crate::Session;
use crate::models::host_of;

#[test]
fn hosts_are_shown_without_paths_queries_or_credentials() {
    assert_eq!(host_of("https://huggingface.co/Phips/2xNomosUni_span_multijpg/resolve/main"), "huggingface.co");
    assert_eq!(host_of("https://user:secret@cdn.example.com:8443/models/x?sig=abc#frag"), "cdn.example.com:8443");
    assert_eq!(host_of("http://127.0.0.1:9000/nomos"), "127.0.0.1:9000");
    assert_eq!(host_of("mirror.example.org/path"), "mirror.example.org");
    assert_eq!(host_of(""), "");
    assert_eq!(host_of("https://"), "");
}

/// Without any AI feature the commands exist and say so; with one they list the models.
#[test]
fn the_commands_work_in_any_build() {
    let mut s = Session::with_demo();
    let list = s.execute("models.list", &json!({})).unwrap();
    assert!(list["models"].is_array() && list["diskBytes"].is_u64(), "{list}");
    // consent, and clear errors for unknown models and missing parameters
    assert!(s.execute("models.download", &json!({})).is_err());
    assert!(s.execute("models.download", &json!({"id": "nope", "acknowledged": true})).is_err());
    assert!(s.execute("models.delete", &json!({"id": "nope", "confirm": true})).is_err());
    assert!(s.execute("models.setEnabled", &json!({"id": "nope", "enabled": false})).is_err());
    assert!(s.execute("models.setEnabled", &json!({"id": "nomos-span-2x"})).is_err());
    assert_eq!(s.execute("models.cancel", &json!({"id": "nope"})).unwrap()["cancelled"], false);
    assert!(s.models_disabled().is_empty());
}

#[cfg(feature = "enhance")]
mod with_models {
    use serde_json::Value;

    use super::*;
    use crate::enhance::{Enhancer, SUPER_RES_MODEL};

    fn model<'a>(list: &'a Value, id: &str) -> &'a Value {
        list["models"].as_array().and_then(|m| m.iter().find(|x| x["id"] == id)).unwrap_or_else(|| panic!("no model {id} in {list}"))
    }

    use crate::segment::Segmenter;
    use crate::tests_enhance::{ready, tmp};

    #[test]
    fn the_list_tells_where_each_model_stands() {
        let dir = tmp("list");
        let mut s = Session::with_demo();
        s.enhancer.dir = Some(dir.join("models"));
        s.enhancer.no_builtin_mirrors = false;
        let list = s.execute("models.list", &json!({})).unwrap();
        let nomos = model(&list, SUPER_RES_MODEL);
        assert_eq!(
            (nomos["available"].as_bool(), nomos["installed"].as_bool(), nomos["enabled"].as_bool()),
            (Some(Enhancer::AVAILABLE), Some(false), Some(true))
        );
        assert_eq!((nomos["downloadBytes"].as_u64(), nomos["diskBytes"].as_u64()), (Some(4_461_056), Some(0)));
        // where it comes from: a host, never a path
        assert_eq!(nomos["sources"], json!(["huggingface.co"]));
        assert!(nomos["dir"].as_str().unwrap().ends_with("nomos-span-2x"));
        assert!(nomos["licence"].as_str().unwrap() == "CC-BY-4.0" && nomos["credit"].as_str().unwrap().contains("Hofmann"));
        assert!(nomos["purpose"].as_str().unwrap().contains("Super Resolution"));
        assert!(nomos["download"].is_null());
        // SAM 3 is listed too, usable only where the build has it
        let sam = model(&list, "sam3");
        assert_eq!(sam["available"].as_bool(), Some(Segmenter::AVAILABLE));
        assert_eq!(sam["downloadBytes"].as_u64(), Some(3_439_938_512));

        // files on disk: the model, and a partial download, both count
        let folder = dir.join("models").join(SUPER_RES_MODEL);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("2xNomosUni_span_multijpg.safetensors"), vec![0u8; 1000]).unwrap();
        std::fs::write(folder.join("2xNomosUni_span_multijpg.safetensors.part"), vec![0u8; 234]).unwrap();
        std::fs::write(folder.join("unrelated.txt"), vec![0u8; 99_999]).unwrap();
        let list = s.execute("models.list", &json!({})).unwrap();
        let nomos = model(&list, SUPER_RES_MODEL);
        assert_eq!(
            (nomos["installed"].as_bool(), nomos["diskBytes"].as_u64()),
            (Some(Enhancer::AVAILABLE), Some(1234)),
            "unrelated files aren't the model's"
        );
        assert!(list["diskBytes"].as_u64().unwrap() >= 1234);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turning_a_model_off_stops_its_feature_and_is_remembered() {
        let (mut s, dir, id) = ready("off", (60, 40));
        let r = s.execute("models.setEnabled", &json!({"id": SUPER_RES_MODEL, "enabled": false})).unwrap();
        assert_eq!(r["disabled"], json!([SUPER_RES_MODEL]));
        assert_eq!(s.models_disabled(), vec![SUPER_RES_MODEL.to_string()]);
        let list = s.execute("models.list", &json!({})).unwrap();
        let nomos = model(&list, SUPER_RES_MODEL);
        assert_eq!((nomos["enabled"].as_bool(), nomos["installed"].as_bool()), (Some(false), Some(true)), "off, but its files stay");
        // the feature says why, and writes nothing
        let e = s.execute("enhance.superRes", &json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains("turned off") && e.contains("Settings"), "{e}");
        assert_eq!(s.catalog.photos().count(), 1);
        // back on: it works
        s.execute("models.setEnabled", &json!({"id": SUPER_RES_MODEL, "enabled": true})).unwrap();
        assert!(s.models_disabled().is_empty());
        assert!(s.execute("enhance.superRes", &json!({"id": id})).is_ok());
        // the saved choice is applied as a whole; ids this version doesn't know are ignored
        s.set_models_disabled(&[SUPER_RES_MODEL.to_string(), "from-the-future".to_string(), "sam3".to_string()]);
        assert_eq!(s.models_disabled(), vec![SUPER_RES_MODEL.to_string(), "sam3".to_string()]);
        let e = s.segmenter.model_dir().unwrap_err();
        assert!(e.contains("turned off") || e.contains("not available"), "{e}");
        s.set_models_disabled(&[]);
        assert!(s.models_disabled().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_removes_only_the_models_files() {
        let (mut s, dir, id) = ready("delete", (60, 40));
        let folder = s.enhancer.model_dir(SUPER_RES_MODEL).unwrap();
        let file = s.enhancer.model_file(SUPER_RES_MODEL).unwrap();
        let size = std::fs::metadata(&file).unwrap().len();
        std::fs::write(folder.join("2xNomosUni_span_multijpg.safetensors.part"), vec![0u8; 77]).unwrap();
        std::fs::write(folder.join("keep-me.txt"), b"mine").unwrap();
        // it needs the user's yes, and the message says what goes
        let e = s.execute("models.delete", &json!({"id": SUPER_RES_MODEL})).unwrap_err().to_string();
        assert!(e.contains("confirm") && e.contains("MB"), "{e}");
        assert!(file.exists());
        let r = s.execute("models.delete", &json!({"id": SUPER_RES_MODEL, "confirm": true})).unwrap();
        assert_eq!((r["deleted"]["bytes"].as_u64(), r["deleted"]["files"].as_u64()), (Some(size + 77), Some(2)));
        assert!(!file.exists() && folder.join("keep-me.txt").exists(), "only the model's own files go");
        assert!(folder.exists(), "a folder with other things in it stays");
        // and the feature knows
        let e = s.execute("enhance.superRes", &json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains("not installed"), "{e}");
        // deleting what isn't there is not an error; an empty folder goes with the model
        std::fs::remove_file(folder.join("keep-me.txt")).unwrap();
        let again = s.execute("models.delete", &json!({"id": SUPER_RES_MODEL, "confirm": true})).unwrap();
        assert_eq!(again["deleted"]["bytes"].as_u64(), Some(0));
        assert!(!folder.exists());
        // a model with no folder set: a clear error
        s.enhancer.dir = None;
        assert!(s.execute("models.delete", &json!({"id": SUPER_RES_MODEL, "confirm": true})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloads_need_consent_route_by_id_and_report_their_progress() {
        use std::io::{BufRead, BufReader, Write};
        use std::time::{Duration, Instant};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/nomos", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut line = String::new();
                while r.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<h1>404</h1>\n");
            }
        });
        let dir = tmp("dl");
        std::fs::write(dir.join(format!("{SUPER_RES_MODEL}-mirrors.txt")), format!("{base}\n")).unwrap();
        let mut s = Session::with_demo();
        s.enhancer.dir = Some(dir.clone());
        s.enhancer.no_builtin_mirrors = true;
        let e = s.execute("models.download", &json!({"id": SUPER_RES_MODEL})).unwrap_err().to_string();
        assert!(e.contains("acknowledged") && e.contains("CC-BY-4.0") && e.contains("MB"), "{e}");
        assert!(!s.enhancer.download_status().1.running, "nothing starts without a yes");
        let r = s.execute("models.download", &json!({"id": SUPER_RES_MODEL, "acknowledged": true})).unwrap();
        assert_eq!(r["started"], true);
        // the list shows the download of that model (and only that one)
        let t = Instant::now();
        loop {
            let list = s.execute("models.list", &json!({})).unwrap();
            assert!(model(&list, "sam3")["download"].is_null());
            let d = &model(&list, SUPER_RES_MODEL)["download"];
            if !d.is_null() && d["running"] == false {
                assert!(d["error"].as_str().unwrap_or_default().contains("not found"), "{d}");
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(30), "the download never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(s.execute("models.cancel", &json!({"id": SUPER_RES_MODEL})).unwrap()["cancelled"], false);
        // SAM 3 goes to its own downloader: here, without a folder or without the feature, a clear error
        assert!(s.execute("models.download", &json!({"id": "sam3", "acknowledged": true})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
