//! A real Apple ProRAW (iPhone 17 Pro, JPEG XL tiles) through the whole client and server path: the streamed thumbnail, the
//! upload with server-built previews, a second device reading them, and the server rendering it at two sizes.
//!
//! The file is a personal photo and is never committed: it is read from `corpus/raw/` (git-ignored; `LIGHTCRAFT_CORPUS`
//! overrides the root) and the test skips when it is absent.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_engine::export::{ExportFormat, ExportOptions, Resize, ResizeMode, export_photo};
use lightcraft_raster::resample::{Filter, fit};
use lightcraft_server::{Config, Server, accounts};
use serde_json::json;

const FILE: &str = "dng-apple-iphone17pro-proraw-jxl.dng";

fn corpus_file() -> Option<PathBuf> {
    let root = std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
    Some(root.join("raw").join(FILE)).filter(|p| p.is_file())
}

fn files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |d| d.flatten().map(|e| if e.path().is_dir() { files_in(&e.path()) } else { 1 }).sum())
}

/// Mean of the channels of a decoded export, and its size.
fn mean_and_size(bytes: &[u8]) -> (f64, (usize, usize)) {
    let img = lightcraft_codecs::decode(bytes, lightcraft_codecs::DecodeOptions::fit(10_000, 10_000)).unwrap().to_working();
    let sum: f64 = img.data.iter().map(|p| f64::from(p[0] + p[1] + p[2])).sum();
    (sum / (img.data.len() * 3) as f64, (img.width, img.height))
}

fn long_edge(n: u32) -> Option<Resize> {
    Some(Resize { mode: ResizeMode::LongEdge, value: n as f32, height: 0, dont_enlarge: true })
}

#[test]
fn an_apple_proraw_works_through_the_client_and_the_server() {
    let Some(path) = corpus_file() else {
        eprintln!("skip: {FILE} is not in the corpus");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();

    // 1. a thumbnail read while the tiles decode is what the whole picture shrunk gives
    let (small, _) = lightcraft_engine::files::load_bytes(&bytes, 256).unwrap();
    let (full, _) = lightcraft_engine::files::load_bytes(&bytes, 100_000).unwrap();
    assert_eq!((full.width, full.height), (3024, 4032), "oriented upright: the sensor is 4032 × 3024, shot in portrait");
    let full = fit(&full, small.width, small.height, Filter::Box);
    assert_eq!((small.width, small.height), (full.width, full.height));
    let n = (small.data.len() * 3) as f64;
    let mean = |img: &lightcraft_raster::Rgb32f| img.data.iter().map(|p| f64::from(p[0] + p[1] + p[2])).sum::<f64>() / n;
    let (ms, mf) = (mean(&small), mean(&full));
    assert!((ms - mf).abs() < 0.02 * mf, "streamed thumbnail brightness {ms} vs {mf}");
    let rel: f64 =
        small.data.iter().zip(&full.data).flat_map(|(a, b)| (0..3).map(move |c| f64::from((a[c] - b[c]).abs() / (b[c].abs() + 0.02)))).sum::<f64>()
            / n;
    assert!(rel < 0.05, "mean relative difference {rel}");

    // 2. a device with this photo uploads it; the server builds the previews (the iPhone's setting)
    let root = std::env::temp_dir().join(format!("lc-server-apple-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    accounts::set_user(&data, "ann", "correct horse", false).unwrap();
    let mut cfg = Config::new(data, "127.0.0.1:0");
    cfg.scan_interval = None;
    cfg.preview_threads = 1;
    let server = Server::start(cfg).unwrap();
    let url = format!("http://{}", server.addr());
    std::fs::create_dir_all(root.join("in")).unwrap();
    std::fs::copy(&path, root.join("in").join("IMG_0001.DNG")).unwrap();
    let mut a = Session::new().with_fs();
    a.open_library(root.join("a"), false).unwrap();
    a.execute("library.import", &json!({"paths": [root.join("in").to_string_lossy()]})).unwrap();
    let id = a.catalog.photos().next().unwrap().id;
    assert_eq!(
        (a.catalog.photo(id).unwrap().format.as_str(), a.catalog.photo(id).unwrap().width),
        ("DNG", 3024),
        "the catalog keeps the upright size"
    );
    a.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "iphone"})).unwrap();
    a.execute("sync.serverPreviews", &json!({"on": true})).unwrap();
    assert_eq!(a.execute("sync.now", &json!({"wait": true})).unwrap()["state"], "idle");
    let blobs = root.join("data/users/ann/blobs");
    let waited = Instant::now();
    while (files_in(&blobs.join("smart")) == 0 || files_in(&blobs.join("mini")) == 0) && waited.elapsed() < Duration::from_secs(300) {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(files_in(&blobs.join("smart")) == 1 && files_in(&blobs.join("mini")) == 1, "the server built both previews of the ProRAW");

    // 3. another device gets the previews and renders from them, about as bright as the original renders here
    let mut b = Session::new().with_fs();
    b.open_library(root.join("b"), false).unwrap();
    b.execute("sync.signIn", &json!({"server": url, "user": "ann", "password": "correct horse", "device": "ipad"})).unwrap();
    b.execute("sync.now", &json!({"wait": true})).unwrap();
    let bid = b.catalog.photos().next().unwrap().id;
    let (ra, rb) = (a.render_now(id, 600, 600).unwrap(), b.render_now(bid, 600, 600).unwrap());
    let bright = |img: &lightcraft_raster::Rgba8| {
        img.data.iter().map(|p| f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])).sum::<f64>() / (img.data.len() * 3) as f64
    };
    assert!((bright(&ra.image) - bright(&rb.image)).abs() < 12.0, "a {} vs b {} (of 255)", bright(&ra.image), bright(&rb.image));

    // 4. the server renders it, small and at full size, like the device does
    a.selection = lightcraft_engine::Selection::single(id);
    a.execute("develop.set", &json!({"control": "light.exposure", "value": 0.4})).unwrap();
    for (edge, expect) in [(1024u32, (768, 1024)), (0, (3024, 4032))] {
        let here = ExportOptions { format: ExportFormat::Jpeg, resize: long_edge(edge).filter(|_| edge > 0), ..Default::default() };
        let there = ExportOptions { on_server: true, ..here.clone() };
        let (local, remote) = (export_photo(&mut a, id, &here, 1).unwrap(), export_photo(&mut a, id, &there, 1).unwrap());
        let ((ml, sl), (mr, sr)) = (mean_and_size(&local.bytes), mean_and_size(&remote.bytes));
        assert_eq!((sl, sr), (expect, expect), "long edge {edge}");
        assert!((ml - mr).abs() < 0.01 * ml.max(0.05), "long edge {edge}: local {ml} vs server {mr}");
    }
    drop(server);
    let _ = std::fs::remove_dir_all(&root);
}
