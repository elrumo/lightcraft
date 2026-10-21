//! The models at work by themselves: a frontend's tick has the ready models look at the photos they haven't
//! (`Session::vision_catch_up`), with a row in the activity stack; a phone's app pauses the run when it leaves the
//! screen; a phone that may use too little memory runs no search model; idle models leave memory when it is short.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use lightcraft_raster::Rgba8;
use lightcraft_vision::fake::FakeEmbedder;
use lightcraft_vision::{Classifier, Embedder, Error};

use super::search_unfit;
use crate::Session;
use crate::hardware::{DeviceClass, DeviceProfile, GB};

/// The stand-in model, slow enough that a run is still going when the test looks.
struct Slow;

impl Embedder for Slow {
    fn model_id(&self) -> &str {
        FakeEmbedder.model_id()
    }
    fn dim(&self) -> usize {
        FakeEmbedder.dim()
    }
    fn encode_text(&self, t: &str) -> Result<Vec<f32>, Error> {
        FakeEmbedder.encode_text(t)
    }
    fn encode_images(&self, i: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error> {
        std::thread::sleep(Duration::from_millis(150));
        FakeEmbedder.encode_images(i)
    }
}

/// Flags nothing.
struct Calm;

impl Classifier for Calm {
    fn engine(&self) -> &str {
        "test-calm"
    }
    fn score(&self, _: &Rgba8) -> Result<f32, Error> {
        Ok(0.05)
    }
}

fn app(model: Arc<dyn Embedder>) -> Session {
    let mut s = Session::with_demo();
    s.vision.set_embedder(model);
    s.vision.background = true;
    s.vision.text_edge = 192;
    s
}

fn finished(s: &Session) -> bool {
    s.vision.job().is_some_and(|j| j.finished.load(Ordering::Relaxed))
}

fn wait_until_finished(s: &Session) {
    let end = Instant::now() + Duration::from_secs(120);
    while !finished(s) {
        assert!(Instant::now() < end, "the run never ended");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn index_rows(s: &Session) -> Vec<crate::activity::TaskInfo> {
    s.activity.list().into_iter().filter(|t| t.kind == "index").collect()
}

/// As if the few seconds between two looks had passed.
fn later(s: &mut Session) {
    s.vision.catch_up.checked = None;
}

#[test]
fn a_ready_model_looks_at_the_photos_by_itself_with_a_row_and_once() {
    let mut s = app(Arc::new(FakeEmbedder));
    let total = s.vision_photo_count();
    assert!(total > 0);
    assert!(s.vision_catch_up(), "the search model is ready and nothing is indexed");
    let rows = index_rows(&s);
    assert_eq!(rows.len(), 1, "a row in the activity stack while it runs");
    assert_eq!(rows[0].label, "Indexing photos");
    assert!(rows[0].cancellable, "with ✕");
    wait_until_finished(&s);
    let polled = s.vision_poll();
    assert!(polled.messages.iter().any(|m| m.starts_with("Indexed ")), "{polled:?}");
    assert!(index_rows(&s).is_empty(), "the row ends with the run");
    assert_eq!(s.vision.indexed(), total);
    // nothing changed: nothing is started again, however often it is asked
    assert!(!s.vision_catch_up(), "it looks every few seconds, not every tick");
    later(&mut s);
    assert!(!s.vision_catch_up(), "the same photos and models: nothing to do");
    assert!(s.vision_poll().messages.is_empty());

    // another model becomes ready (sensitive content turned on): it looks at the photos, the descriptions stay
    s.vision.set_classifier(Arc::new(Calm));
    s.vision.sensitive = true;
    later(&mut s);
    assert!(s.vision_catch_up(), "a newly ready model gets to work");
    wait_until_finished(&s);
    s.vision_poll();
    assert_eq!(s.vision.indexed(), total);
    assert!(s.vision.job().is_some_and(|j| j.total == total), "only the new model's work: {total} photos");
}

#[test]
fn nothing_ready_starts_nothing() {
    let mut s = Session::with_demo();
    s.vision.background = true;
    assert_eq!(s.vision_ready_parts(), 0);
    assert!(!s.vision_catch_up());
    assert!(s.vision.job().is_none() && index_rows(&s).is_empty());
}

#[test]
fn a_run_with_nothing_to_do_shows_no_row_and_says_nothing() {
    let mut s = app(Arc::new(FakeEmbedder));
    s.vision.background = false;
    s.vision_index(None, true).unwrap();
    s.vision.background = true;
    // (a run is started, finds every photo done, and ends quietly)
    s.vision_catch_up();
    assert!(index_rows(&s).is_empty());
    wait_until_finished(&s);
    assert!(s.vision_poll().messages.is_empty(), "no \"Indexed 0 photos\"");
}

#[test]
fn a_cancelled_run_stays_cancelled_until_something_changes() {
    let mut s = app(Arc::new(Slow));
    assert!(s.vision_catch_up());
    // the row's ✕
    let id = index_rows(&s)[0].id;
    s.activity.cancel(id).unwrap();
    wait_until_finished(&s);
    s.vision_poll();
    assert!(s.vision.indexed() < s.vision_photo_count(), "stopped part way");
    later(&mut s);
    assert!(!s.vision_catch_up(), "the user stopped it: not started again by itself");
}

#[test]
fn leaving_the_screen_pauses_the_run_quietly_and_it_resumes_where_it_was() {
    let mut s = app(Arc::new(Slow));
    let total = s.vision_photo_count();
    assert!(s.vision_catch_up());
    assert!(s.vision_pause(), "a run was going");
    wait_until_finished(&s);
    let polled = s.vision_poll();
    assert!(polled.messages.is_empty(), "no \"cancelled\" notice for a pause: {polled:?}");
    assert!(index_rows(&s).is_empty());
    let first = s.vision.indexed();
    assert!(first < total, "{first} of {total}");
    assert!(!s.vision_pause(), "nothing left to pause");
    // back on screen: the run starts again, with the photos still to do
    assert!(s.vision_catch_up());
    assert_eq!(s.vision.job().map(|j| j.total), Some(total - first));
    wait_until_finished(&s);
    s.vision_poll();
    assert_eq!(s.vision.indexed(), total);
}

#[test]
fn a_phone_that_may_use_too_little_memory_runs_no_search_model() {
    let phone = |gb: Option<u64>| DeviceProfile { class: DeviceClass::Phone, memory_bytes: gb.map(|g| g * GB), gpu: true, cores: 6 };
    let why = search_unfit(&phone(Some(2))).unwrap();
    assert!(why.contains("3.25 GB") && why.contains("2 GB"), "{why}");
    assert!(search_unfit(&phone(Some(4))).is_none());
    assert!(search_unfit(&phone(None)).is_some(), "a phone that doesn't say: not offered");
    // an iPhone 17 Pro's 3373 MB at launch is enough, with room to spare; a phone that may use 3 GB is not
    assert!(search_unfit(&DeviceProfile { memory_bytes: Some(3373 << 20), ..phone(None) }).is_none());
    assert!(search_unfit(&DeviceProfile { memory_bytes: Some(3200 << 20), ..phone(None) }).is_none());
    assert!(search_unfit(&DeviceProfile { memory_bytes: Some(3 * GB), ..phone(None) }).is_some());
    let computer = DeviceProfile { class: DeviceClass::Computer, memory_bytes: Some(GB), gpu: false, cores: 4 };
    assert!(search_unfit(&computer).is_none(), "a computer swaps: always offered");

    // the session keeps it with the device: set by the host at launch from what the app may use
    let mut s = Session::with_demo();
    s.set_device(phone(None));
    assert!(s.vision.unfit.is_some());
    s.set_app_memory(Some(4 << 30));
    assert!(s.vision.unfit.is_none());
    s.set_app_memory(Some(2 << 30));
    assert!(s.vision.unfit.is_some());
    s.set_device(computer);
    assert!(s.vision.unfit.is_none());

    // on such a phone the model is listed as not available, with why, and its download is refused
    s.set_device(phone(Some(2)));
    let list = s.execute("models.list", &serde_json::json!({})).unwrap();
    let siglip = list["models"].as_array().unwrap().iter().find(|m| m["id"] == "siglip2").unwrap().clone();
    assert_eq!(siglip["available"], false, "{siglip}");
    let err = s.execute("models.download", &serde_json::json!({"id": "siglip2", "acknowledged": true})).unwrap_err().to_string();
    assert!(err.contains("isn't available on this device"), "{err}");
    if cfg!(feature = "vision") {
        assert!(siglip["unavailable"].as_str().is_some_and(|w| w.contains("3.25 GB")), "{siglip}");
        assert!(err.contains("3.25 GB"), "{err}");
        assert!(s.vision_status()["unfit"].as_str().is_some());
    }
    // the other models are small: their availability doesn't depend on the phone's memory
    let text = list["models"].as_array().unwrap().iter().find(|m| m["id"] == "ppocr").unwrap();
    assert_eq!(text["available"].as_bool(), Some(cfg!(feature = "vision")), "{text}");
    assert!(text.get("unavailable").is_none());

    // a model a host supplies (not the 1.5 GB one) still runs there
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    assert!(s.vision.local_available() && s.vision.unfit().is_none());
}

#[test]
fn short_of_memory_the_idle_models_are_unloaded_and_busy_ones_kept() {
    let mut s = app(Arc::new(FakeEmbedder));
    s.vision.background = false;
    s.vision_index(None, true).unwrap();
    let sh = s.vision.shared.clone();
    // (a stand-in model isn't loaded from a file: mark the slots as a loaded model leaves them)
    sh.loaded.store(true, Ordering::SeqCst);
    sh.touch();
    assert!(sh.index.lock().unwrap().is_some(), "the index is in memory after the run");
    {
        let _busy = super::Busy::new(&sh);
        s.release_memory();
        assert!(sh.loaded.load(Ordering::SeqCst), "a model at work stays");
    }
    s.release_memory();
    assert!(!sh.loaded.load(Ordering::SeqCst), "an idle one goes at once, not after ten minutes");
    assert!(sh.index.lock().unwrap().is_none());
    // and it comes back on the next request
    s.vision_index(None, true).unwrap();
    assert_eq!(s.vision.indexed(), s.vision_photo_count());
}
