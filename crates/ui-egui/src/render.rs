//! The egui side of background rendering. [`Scheduler`] (in `lightcraft-apphost`, shared with the
//! SwiftUI iOS app) decides what renders when and what each slot shows; this module turns its
//! pictures into egui textures ([`TexView`]) and asks for repaints while renders are in flight.
//!
//! [`Renderer`] derefs to the scheduler, so the panels use one object for both.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

pub use lightcraft_apphost::render::{DraftBudget, MAX_VARIANT_TEXTURES, RenderOffload, Shown, Slot, VARIANT_TEXTURES};
use lightcraft_apphost::render::{HdrPicture, Present, Scheduler};
use lightcraft_engine::Session;

use crate::hdr::{HdrSink, HdrTex};

/// A slot's picture as egui shows it.
pub struct TexView {
    pub tex: egui::TextureHandle,
    /// CPU copy of the pixels (only with [`Renderer::keep_pixels`]; for headless screenshots).
    pub pixels: Option<Arc<egui::ColorImage>>,
    /// The same render in HDR, drawn instead of `tex` ([`Self::paint_id`]) when the display can show
    /// it (an HDR edit's loupe, with an [`HdrSink`]).
    pub hdr: Option<HdrTex>,
}

impl TexView {
    /// The texture to draw this picture with: its HDR rendition when it has one.
    pub fn paint_id(&self) -> egui::TextureId {
        self.hdr.as_ref().map_or_else(|| self.tex.id(), |h| h.id)
    }
}

/// What a slot shows, with its egui texture (the facts derefs to [`TexView`]).
pub type Tex = Shown<TexView>;

/// The texture of a render: its pixels as egui wants them. Renders are opaque, so the common case
/// is a plain copy; only a pixel with alpha pays for premultiplying. (The generic conversion, on
/// the UI thread, was most of the time a 5 MP zoom window took to show.)
pub(crate) fn color_image(img: &lightcraft_raster::Rgba8) -> egui::ColorImage {
    let pixels = img
        .data
        .iter()
        .map(|p| if p[3] == 255 { egui::Color32::from_rgb(p[0], p[1], p[2]) } else { egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]) })
        .collect();
    egui::ColorImage { size: [img.width, img.height], source_size: egui::vec2(img.width as f32, img.height as f32), pixels }
}

/// The app's render queue: the shared [`Scheduler`] with egui textures.
pub struct Renderer {
    sched: Scheduler<TexView>,
    /// Keep a CPU copy of every texture so the UI can be rasterized headlessly (set when the app
    /// is driven by the control channel).
    pub keep_pixels: bool,
    /// Where HDR pictures become textures: the window's renderer, when it has a surface that can show
    /// them ([`Self::set_hdr_sink`]).
    hdr_sink: Option<Arc<dyn HdrSink>>,
    #[cfg(test)]
    pub(crate) thumb_jobs_built: usize,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::from_scheduler(Scheduler::default())
    }
}

impl Deref for Renderer {
    type Target = Scheduler<TexView>;
    fn deref(&self) -> &Scheduler<TexView> {
        &self.sched
    }
}

impl DerefMut for Renderer {
    fn deref_mut(&mut self) -> &mut Scheduler<TexView> {
        &mut self.sched
    }
}

/// Makes and updates the textures of finished renders.
struct Textures<'a> {
    ctx: &'a egui::Context,
    keep_pixels: bool,
    hdr: Option<&'a Arc<dyn HdrSink>>,
}

impl Present<TexView> for Textures<'_> {
    fn make(&mut self, slot: Slot, image: lightcraft_raster::Rgba8) -> TexView {
        let color = Arc::new(color_image(&image));
        let pixels = self.keep_pixels.then(|| color.clone());
        TexView { tex: self.ctx.load_texture(format!("{slot:?}"), color, egui::TextureOptions::LINEAR), pixels, hdr: None }
    }

    fn update(&mut self, _: Slot, image: lightcraft_raster::Rgba8, view: &mut TexView) {
        let color = Arc::new(color_image(&image));
        view.pixels = self.keep_pixels.then(|| color.clone());
        view.tex.set(color, egui::TextureOptions::LINEAR);
    }

    fn set_hdr(&mut self, _: Slot, hdr: Option<Arc<HdrPicture>>, view: &mut TexView) {
        view.hdr = match (hdr, self.hdr) {
            (Some(picture), Some(sink)) => HdrTex::make(sink, &picture, view.hdr.take()),
            _ => None,
        };
    }
}

impl Renderer {
    /// Explicit worker count for hosts and bounded validation runs (one to six on native).
    /// Browser hosts continue to use inline jobs or their installed worker offload.
    pub fn with_worker_threads(threads: usize) -> Self {
        Self::from_scheduler(Scheduler::with_worker_threads(threads))
    }

    fn from_scheduler(sched: Scheduler<TexView>) -> Self {
        Renderer {
            sched,
            keep_pixels: false,
            hdr_sink: None,
            #[cfg(test)]
            thumb_jobs_built: 0,
        }
    }

    /// The window can draw HDR pictures, made by `sink` (`None`: it can't, and never could since the
    /// start). Whether they are *asked for* is [`Scheduler::set_hdr_display`]: the display the window
    /// is on can change under a surface that can.
    pub fn set_hdr_sink(&mut self, sink: Option<Arc<dyn HdrSink>>) {
        self.hdr_sink = sink;
    }

    /// Whether [`Self::set_hdr_sink`] gave the window a way to draw HDR.
    pub fn can_draw_hdr(&self) -> bool {
        self.hdr_sink.is_some()
    }

    /// An HDR picture is on screen: the loupe's render of an HDR edit, drawn in HDR.
    pub fn shows_hdr(&self) -> bool {
        self.textures.get(&Slot::Main).is_some_and(|t| t.hdr.is_some())
    }

    /// Collect finished jobs into textures, and ask for a repaint while renders are in flight (or
    /// the idle trim is due). Returns true if anything changed.
    pub fn poll(&mut self, ctx: &egui::Context, session: &mut Session) -> bool {
        let polled = self.sched.poll(session, &mut Textures { ctx, keep_pixels: self.keep_pixels, hdr: self.hdr_sink.as_ref() });
        if let Some(after) = polled.wake_after {
            ctx.request_repaint_after(after);
        }
        polled.changed()
    }

    /// In a slider drag, give the loupe's draft until `budget` to finish, so it is drawn in this
    /// frame ([`Scheduler::wait_for_draft`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait_for_draft(&mut self, ctx: &egui::Context, session: &mut Session, budget: std::time::Duration) {
        self.sched.wait_for_draft(session, &mut Textures { ctx, keep_pixels: self.keep_pixels, hdr: self.hdr_sink.as_ref() }, budget);
    }

    /// What the renderer holds: per-view stage caches (CPU images and GPU buffers) and textures.
    pub fn memory(&self) -> serde_json::Value {
        let mut m = self.sched.memory();
        let copies: usize = self.textures.values().filter_map(|t| t.pixels.as_ref()).map(|p| p.pixels.len() * 4).sum();
        m["textures"]["cpuCopyBytes"] = copies.into();
        m
    }

    /// Show `img` in `slot` (an image the app already has, such as a thumbnail a camera made:
    /// [`Slot::Camera`]).
    pub fn put_image(&mut self, ctx: &egui::Context, slot: Slot, img: &lightcraft_raster::Rgba8) {
        let color = Arc::new(color_image(img));
        let pixels = self.keep_pixels.then(|| color.clone());
        let view = TexView { tex: ctx.load_texture(format!("{slot:?}"), color, egui::TextureOptions::LINEAR), pixels, hdr: None };
        let shown = Shown {
            key: 0,
            photo: lightcraft_catalog::PhotoId(0),
            size: [img.width, img.height],
            histogram: None,
            ms: 0.0,
            quick: None,
            request_id: 0,
            draft: false,
            view,
        };
        self.textures.insert(slot, shown);
    }

    /// CPU copies of the current textures by id (see [`Self::keep_pixels`]).
    pub fn cpu_textures(&self) -> HashMap<egui::TextureId, crate::softpaint::CpuTexture> {
        let mut all = HashMap::new();
        for t in self.textures.values() {
            let Some(pixels) = t.pixels.clone() else { continue };
            // (the headless picture of an HDR texture is its 8-bit one)
            if let Some(h) = &t.hdr {
                all.insert(h.id, crate::softpaint::CpuTexture::linear(pixels.clone()));
            }
            all.insert(t.tex.id(), crate::softpaint::CpuTexture::linear(pixels));
        }
        all
    }
}

#[cfg(test)]
mod thumbnail_tests {
    use super::*;
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    use lightcraft_engine::media::{RenderJob, RenderResult};

    struct Blocked;
    impl RenderOffload for Blocked {
        fn try_start(&mut self, _: Slot, job: RenderJob) -> Option<RenderJob> {
            Some(job)
        }
        fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
            vec![]
        }
    }
    fn photo() -> Arc<Photo> {
        Arc::new(Photo::new(PhotoId(1), Source::File { path: "audit.jpg".into() }, "audit.jpg", "JPEG", 6000, 4000, "2026-01-01"))
    }

    // The memory report adds the CPU copies kept for headless screenshots to the scheduler's
    #[test]
    fn the_renderer_reports_its_stage_budget_and_copies() {
        let r = Renderer::default();
        let m = r.memory();
        assert!(m["stageCaches"]["budgetBytes"].as_u64().unwrap() > 0, "{m}");
        assert_eq!(m["textures"]["cpuCopyBytes"], 0);
    }

    #[test]
    fn a_fast_draft_is_drawn_in_the_frame_that_asked_for_it() {
        let mut s = Session::new();
        s.media.file_loader = Some(Arc::new(|_, _| Ok((lightcraft_raster::Rgb32f::new(8, 8), Default::default()))));
        s.catalog.apply(Op::AddPhoto { photo: Box::new((*photo()).clone()) }).unwrap();
        let mut r = Renderer::with_worker_threads(1);
        let ctx = egui::Context::default();
        let budget = std::time::Duration::from_secs(10);
        // the last loupe render took longer than the budget (a CPU render): not waited for
        r.last_main_ms = 1e6;
        r.request(Slot::Main, s.render_job(PhotoId(1), 64, 64, false, true).unwrap(), 100);
        r.wait_for_draft(&ctx, &mut s, budget);
        assert!(r.is_pending(Slot::Main) && !r.textures.contains_key(&Slot::Main));
        // a fast one (GPU drafts): the texture is the new render when the wait returns
        r.last_main_ms = 1.0;
        r.request(Slot::Main, s.render_job(PhotoId(1), 32, 32, false, true).unwrap(), 100);
        let want = r.wanted(Slot::Main);
        r.wait_for_draft(&ctx, &mut s, budget);
        assert!(!r.is_pending(Slot::Main));
        assert_eq!(r.textures.get(&Slot::Main).map(|t| t.key), want);
    }

    #[derive(Default)]
    struct Work {
        jobs: Vec<(Slot, RenderJob)>,
        finished: Vec<(Slot, RenderResult, f64)>,
    }
    struct Manual(std::rc::Rc<std::cell::RefCell<Work>>);
    impl RenderOffload for Manual {
        fn try_start(&mut self, slot: Slot, job: RenderJob) -> Option<RenderJob> {
            self.0.borrow_mut().jobs.push((slot, job));
            None
        }
        fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
            std::mem::take(&mut self.0.borrow_mut().finished)
        }
    }
    fn fixture() -> (crate::LightcraftApp, std::rc::Rc<std::cell::RefCell<Work>>, egui::Context) {
        fixture_with(Session::new(), (*photo()).clone())
    }
    fn fixture_with(mut s: Session, photo: Photo) -> (crate::LightcraftApp, std::rc::Rc<std::cell::RefCell<Work>>, egui::Context) {
        s.media.file_loader = Some(Arc::new(|_, _| {
            let mut image = lightcraft_raster::Rgb32f::new(8, 8);
            image.data.fill([0.3, 0.2, 0.1]);
            Ok((image, Default::default()))
        }));
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        let mut app = crate::LightcraftApp::new(s, crate::Services::default());
        app.renderer.keep_pixels = true;
        let work = std::rc::Rc::new(std::cell::RefCell::new(Work::default()));
        app.renderer.set_offload(Box::new(Manual(work.clone())));
        let ctx = egui::Context::default();
        app.renderer.poll(&ctx, &mut app.session);
        (app, work, ctx)
    }
    fn take_job(work: &std::rc::Rc<std::cell::RefCell<Work>>) -> (Slot, RenderJob) {
        work.borrow_mut().jobs.pop().unwrap()
    }
    fn finish(app: &mut crate::LightcraftApp, work: &std::rc::Rc<std::cell::RefCell<Work>>, ctx: &egui::Context, slot: Slot, result: RenderResult) {
        work.borrow_mut().finished.push((slot, result, 0.0));
        app.renderer.poll(ctx, &mut app.session);
    }
    fn request(app: &mut crate::LightcraftApp) {
        crate::panels::grid::request_thumb(app, PhotoId(1), 256, 10);
    }

    #[test]
    fn unchanged_ready_and_running_frames_build_no_jobs() {
        let (mut app, work, ctx) = fixture();
        request(&mut app);
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        for _ in 0..180 {
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        let (slot, job) = take_job(&work);
        finish(&mut app, &work, &ctx, slot, job.run());
        for _ in 0..180 {
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        assert!(work.borrow().jobs.is_empty());
    }

    #[test]
    fn a_slider_drag_leaves_the_edited_photos_thumbnail_until_it_ends() {
        let (mut app, work, ctx) = fixture();
        request(&mut app);
        let (slot, job) = take_job(&work);
        finish(&mut app, &work, &ctx, slot, job.run());
        app.session.execute("library.select", &serde_json::json!({"ids": [1], "active": 1})).unwrap();
        app.session.execute("develop.beginInteraction", &serde_json::json!({"label": "Exposure"})).unwrap();
        for v in [0.1, 0.2, 0.3] {
            app.session.execute("develop.set", &serde_json::json!({"control": "light.exposure", "value": v})).unwrap();
            app.renderer.poll(&ctx, &mut app.session);
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1, "the drag's frames render no thumbnail of the photo");
        assert!(work.borrow().jobs.is_empty());
        app.session.execute("develop.endInteraction", &serde_json::json!({})).unwrap();
        request(&mut app);
        assert_eq!(app.renderer.thumb_jobs_built, 2, "the thumbnail follows once the drag ends");
    }

    #[test]
    fn unchanged_queued_frames_build_no_jobs() {
        let (mut app, _, _) = fixture();
        app.renderer.set_offload(Box::new(Blocked));
        request(&mut app);
        for _ in 0..180 {
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        assert_eq!(app.renderer.queued(), 1);
    }

    // A slider drag changes the photo every frame: its thumbnail keeps what it shows until the drag
    // ends, instead of taking a worker from the loupe on every tick
    #[test]
    fn a_drag_renders_no_thumbnail_of_the_photo_it_edits() {
        let (mut app, work, ctx) = fixture();
        request(&mut app);
        let (slot, job) = take_job(&work);
        finish(&mut app, &work, &ctx, slot, job.run());
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        app.run("library.select", serde_json::json!({"ids": [1]})).unwrap();
        app.run("develop.beginInteraction", serde_json::json!({"label": "Exposure"})).unwrap();
        for k in 1..=30 {
            app.run("develop.set", serde_json::json!({"control": "light.exposure", "value": 0.05 * f64::from(k)})).unwrap();
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1, "no thumbnail job per tick");
        assert!(work.borrow().jobs.is_empty());
        app.run("develop.endInteraction", serde_json::json!({})).unwrap();
        request(&mut app);
        assert_eq!(app.renderer.thumb_jobs_built, 2, "the thumbnail catches up once the drag ends");
    }

    #[test]
    fn resizing_the_disk_cache_keeps_textures_and_skips_rebuilds() {
        let lib = std::env::temp_dir().join(format!("lc-ui-thumb-resize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&lib);
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        let (mut app, work, ctx) = fixture_with(s, (*photo()).clone());
        request(&mut app);
        let (slot, job) = take_job(&work);
        finish(&mut app, &work, &ctx, slot, job.run());
        assert!(app.renderer.thumb(PhotoId(1)).is_some());
        let cache = app.session.media.rendered.clone();
        app.session.execute("library.preferences", &serde_json::json!({"cacheMb": 300})).unwrap();
        app.renderer.poll(&ctx, &mut app.session);
        // thumbnails are keyed by content: a new budget leaves them (and the cache) valid
        assert!(app.renderer.thumb(PhotoId(1)).is_some());
        assert!(Arc::ptr_eq(&cache, &app.session.media.rendered));
        for _ in 0..30 {
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        assert!(work.borrow().jobs.is_empty());
        // an explicit clear still drops them
        app.session.execute("library.clearPreviews", &serde_json::json!({})).unwrap();
        app.renderer.poll(&ctx, &mut app.session);
        assert!(app.renderer.thumb(PhotoId(1)).is_none());
        drop(app);
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn raw_quick_path_frames_build_no_jobs() {
        // an unedited raw: the grid shows its embedded preview (held here until released) and stops
        // there — the camera JPEG at 4 416 × 2 944 already covers a 512 px thumbnail, so decoding the
        // whole raw behind it only costs the file read (45 s a photo on a 1.2 MB/s share)
        let gate = Arc::new(std::sync::Mutex::new(()));
        let held = gate.lock().unwrap();
        let mut s = Session::new();
        let g = gate.clone();
        s.media.preview_loader = Some(Arc::new(move |_: &str, _| {
            drop(g.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
            Some(lightcraft_raster::Rgba8 { width: 4, height: 4, data: vec![[200, 100, 50, 255]; 16] })
        }));
        let mut raw = (*photo()).clone();
        raw.kind = lightcraft_catalog::MediaKind::Raw;
        raw.develop = Arc::new(raw.camera_defaults());
        let (mut app, _, ctx) = fixture_with(s, raw);
        app.renderer.set_offload(Box::new(Blocked));
        let quick = Slot::ThumbQuick(PhotoId(1));
        // the stand-in is running
        for _ in 0..180 {
            request(&mut app);
        }
        assert!(app.renderer.is_pending(quick));
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        drop(held);
        let t0 = std::time::Instant::now();
        while !app.renderer.textures.contains_key(&quick) {
            assert!(t0.elapsed() < std::time::Duration::from_secs(20), "the embedded preview never arrived");
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.renderer.poll(&ctx, &mut app.session);
        }
        // the embedded preview is the thumbnail: no real render is queued behind it, and the grid
        // asking every frame neither rebuilds the job nor re-requests the preview
        for _ in 0..180 {
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1, "an unedited raw's embedded preview is the thumbnail");
        assert!(app.renderer.queued() == 0, "nothing is decoded behind it");
        assert!(!app.renderer.is_pending(Slot::Thumb(PhotoId(1))));

        // an edit takes the embedded preview out of the picture: the real thumbnail is rendered then
        let mut edited = (*photo()).clone();
        edited.kind = lightcraft_catalog::MediaKind::Raw;
        edited.develop = Arc::new({
            let mut d = edited.camera_defaults();
            d.light.exposure = 0.5;
            d
        });
        let (mut app, _, _ctx) = fixture_with(Session::new(), edited);
        app.renderer.set_offload(Box::new(Blocked));
        for _ in 0..180 {
            request(&mut app);
        }
        assert_eq!(app.renderer.thumb_jobs_built, 1);
        assert!(app.renderer.is_pending(Slot::Thumb(PhotoId(1))) || !app.renderer.queued() == 0, "an edited raw renders its thumbnail");
    }

    #[test]
    fn closing_import_review_cancels_queued_and_running_identities() {
        let (mut app, work, ctx) = fixture();
        let job = app.session.thumb_job(PhotoId(1), 256).unwrap();
        app.renderer.request(Slot::Import(0), job.clone(), 10);
        let (slot, running) = take_job(&work);
        app.renderer.set_offload(Box::new(Blocked));
        app.renderer.request(Slot::Import(1), job, 10);
        app.renderer.forget_imports();
        assert!(!app.renderer.is_pending(Slot::Import(0)));
        assert!(!app.renderer.is_pending(Slot::Import(1)));
        assert!(app.renderer.queued() == 0);
        work.borrow_mut().finished.push((slot, running.run(), 0.0));
        app.renderer.set_offload(Box::new(Manual(work.clone())));
        app.renderer.poll(&ctx, &mut app.session);
        assert!(!app.renderer.textures.contains_key(&slot));
        assert_eq!(app.session.media.source_usage().0, 0);
    }

    #[test]
    fn loupe_shows_superseded_drafts_newer_than_its_pixels_during_a_drag() {
        let (mut app, work, ctx) = fixture();
        let mut jobs = Vec::new();
        for exposure in [0.0, 1.0, 2.0] {
            let mut edit = lightcraft_develop::DevelopSettings::default();
            edit.light.exposure = exposure;
            app.session.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(edit), label: "drag".into(), edited: None }).unwrap();
            let job = app.session.loupe_job(PhotoId(1), 8, 8, true).unwrap().draft();
            app.renderer.request(Slot::Main, job, 100);
            jobs.push(take_job(&work).1);
        }
        let keys: Vec<u64> = jobs.iter().map(|j| j.key).collect();
        let mut jobs = jobs.into_iter();
        let (first, second, last) = (jobs.next().unwrap(), jobs.next().unwrap(), jobs.next().unwrap());
        // the first draft lands while the newest is still rendering: the loupe moves on
        finish(&mut app, &work, &ctx, Slot::Main, first.run());
        assert_eq!(app.renderer.textures.get(&Slot::Main).map(|t| t.key), Some(keys[0]));
        assert!(app.renderer.is_pending(Slot::Main));
        finish(&mut app, &work, &ctx, Slot::Main, last.run());
        assert_eq!(app.renderer.textures.get(&Slot::Main).map(|t| t.key), Some(keys[2]));
        // a draft older than the pixels shown never replaces them
        finish(&mut app, &work, &ctx, Slot::Main, second.run());
        assert_eq!(app.renderer.textures.get(&Slot::Main).map(|t| t.key), Some(keys[2]));
        assert!(!app.renderer.is_pending(Slot::Main));
    }

    #[test]
    fn a_drag_gets_its_live_hdr_histogram_beside_the_drafts() {
        let (mut app, work, ctx) = fixture();
        let mut jobs = Vec::new();
        for exposure in [1.0, 2.0] {
            let mut edit = lightcraft_develop::DevelopSettings::default();
            edit.hdr.enabled = true;
            edit.light.exposure = exposure;
            app.session.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(edit), label: "drag".into(), edited: None }).unwrap();
            let job = app.session.render_job(PhotoId(1), 32, 32, false, true).unwrap();
            app.renderer.request(Slot::HdrHistogram, job, 95);
            jobs.push(take_job(&work).1);
        }
        let keys: Vec<u64> = jobs.iter().map(|j| j.key).collect();
        let mut jobs = jobs.into_iter();
        let (first, last) = (jobs.next().unwrap(), jobs.next().unwrap());
        // superseded while the drag goes on, it still lands: the histogram keeps moving
        finish(&mut app, &work, &ctx, Slot::HdrHistogram, first.run());
        let live = app.renderer.live_hdr_histogram.clone().expect("a live HDR histogram");
        assert_eq!((live.0, live.1), (PhotoId(1), keys[0]));
        assert!(live.2.hdr.is_some(), "HDR bins");
        finish(&mut app, &work, &ctx, Slot::HdrHistogram, last.run());
        assert_eq!(app.renderer.live_hdr_histogram.as_ref().map(|l| l.1), Some(keys[1]));
        // only the histogram is kept: no texture
        assert!(!app.renderer.textures.contains_key(&Slot::HdrHistogram));
    }

    #[test]
    fn library_reset_rejects_old_job_even_when_photo_id_and_key_are_reused() {
        let (mut app, work, ctx) = fixture();
        request(&mut app);
        let (slot, old) = take_job(&work);
        let old_result = old.run();
        app.renderer.forget_all();
        let photo = app.session.catalog.photo(PhotoId(1)).unwrap().as_ref().clone();
        app.session = lightcraft_engine::Session::new();
        app.session.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        app.session.media.file_loader = Some(Arc::new(|_, _| {
            let mut image = lightcraft_raster::Rgb32f::new(8, 8);
            image.data.fill([0.1, 0.2, 0.9]);
            Ok((image, Default::default()))
        }));
        app.renderer.poll(&ctx, &mut app.session);
        request(&mut app);
        let (_, new) = take_job(&work);
        assert_eq!(old_result.key, new.key);
        assert_ne!(old_result.request_id, new.request_id);
        let expected = new.clone().run().rendered.unwrap().image.as_bytes();
        finish(&mut app, &work, &ctx, slot, new.run());
        finish(&mut app, &work, &ctx, slot, old_result);
        assert_eq!(
            app.renderer.thumb(PhotoId(1)).unwrap().pixels.as_ref().unwrap().pixels.iter().flat_map(|c| c.to_array()).collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn late_results_and_errors_cannot_replace_newest_pixels_or_sources() {
        for order in [0, 1, 2] {
            let (mut app, work, ctx) = fixture();
            request(&mut app);
            let (slot, old) = take_job(&work);
            let mut edit = lightcraft_develop::DevelopSettings::default();
            edit.light.exposure = 1.0;
            app.session.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(edit), label: "edit".into(), edited: None }).unwrap();
            request(&mut app);
            let (_, new) = take_job(&work);
            let expected = new.clone().run().rendered.unwrap().image.as_bytes();
            if order != 0 {
                finish(&mut app, &work, &ctx, slot, new.run());
                // An obsolete source and error must not poison the current successful request.
                let mut stale = old.run();
                if order == 2 {
                    stale.rendered = Err("old failure".into());
                }
                finish(&mut app, &work, &ctx, slot, stale);
            } else {
                finish(&mut app, &work, &ctx, slot, old.run());
                assert!(app.renderer.thumb(PhotoId(1)).is_none());
                assert_eq!(app.session.media.source_usage().0, 0);
                finish(&mut app, &work, &ctx, slot, new.run());
            }
            assert_eq!(
                app.renderer.thumb(PhotoId(1)).unwrap().pixels.as_ref().unwrap().pixels.iter().flat_map(|c| c.to_array()).collect::<Vec<_>>(),
                expected
            );
            assert!(app.renderer.failure(slot).is_none());
        }
    }

    #[test]
    fn undo_to_existing_texture_supersedes_pending_edit() {
        let (mut app, work, ctx) = fixture();
        request(&mut app);
        let (slot, first) = take_job(&work);
        let key = first.key;
        finish(&mut app, &work, &ctx, slot, first.run());
        let original = app.renderer.thumb(PhotoId(1)).unwrap().pixels.clone().unwrap();
        let mut edit = lightcraft_develop::DevelopSettings::default();
        edit.light.exposure = 1.0;
        let undo =
            app.session.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(edit), label: "edit".into(), edited: None }).unwrap();
        request(&mut app);
        let (_, edited) = take_job(&work);
        app.session.catalog.apply(undo).unwrap();
        request(&mut app);
        let (_, restored) = take_job(&work);
        assert_eq!(restored.key, key);
        finish(&mut app, &work, &ctx, slot, restored.run());
        finish(&mut app, &work, &ctx, slot, edited.run());
        assert_eq!(app.renderer.thumb(PhotoId(1)).unwrap().pixels.as_ref().unwrap().pixels, original.pixels);
    }

    #[test]
    fn explicit_clear_repairs_same_key_bad_pixels_and_rejects_old_same_key_job() {
        let (mut app, work, ctx) = fixture();
        let job = app.session.thumb_job(PhotoId(1), 256).unwrap();
        let expected = job.clone().run().rendered.unwrap().image;
        let (cache, key) = job.cache.clone().unwrap();
        let mut bad = expected.clone();
        bad.data.fill([255, 0, 255, 255]);
        cache.put(key, Arc::new(bad.clone()));
        request(&mut app);
        let (slot, corrupt) = take_job(&work);
        finish(&mut app, &work, &ctx, slot, corrupt.run());
        assert_eq!(app.renderer.thumb(PhotoId(1)).unwrap().pixels.as_ref().unwrap().pixels[0].to_array(), [255, 0, 255, 255]);
        // An old job can have the SAME render key and stale decoded pixels.
        let mut old = job.clone();
        old.source = lightcraft_engine::media::SourceRef::Loaded(Box::new(lightcraft_engine::media::DecodedSource::new(
            Arc::new(lightcraft_raster::Rgb32f::new(8, 8)),
            None,
        )));
        app.renderer.textures.clear();
        app.renderer.request(slot, old, 10);
        let (_, old) = take_job(&work);
        app.session.execute("library.clearPreviews", &serde_json::json!({})).unwrap();
        app.renderer.poll(&ctx, &mut app.session);
        assert!(app.renderer.thumb(PhotoId(1)).is_none());
        request(&mut app);
        let (_, repaired) = take_job(&work);
        assert_eq!(repaired.key, job.key);
        finish(&mut app, &work, &ctx, slot, repaired.run());
        finish(&mut app, &work, &ctx, slot, old.run());
        assert_eq!(cache.get(key).unwrap().as_bytes(), expected.as_bytes());
        assert_eq!(
            app.renderer.thumb(PhotoId(1)).unwrap().pixels.as_ref().unwrap().pixels.iter().flat_map(|c| c.to_array()).collect::<Vec<_>>(),
            expected.as_bytes()
        );
    }
}

#[cfg(test)]
mod color_image_tests {
    use super::*;

    // The texture a render becomes is what egui would make of its bytes, whatever the alpha
    #[test]
    fn the_fast_conversion_equals_eguis() {
        let img = lightcraft_raster::Rgba8::from_fn(37, 29, |x, y| {
            let a = match (x + y) % 5 {
                0 => 0,
                1 => 17,
                2 => 128,
                _ => 255,
            };
            [(x * 7) as u8, (y * 9) as u8, (x * y) as u8, a]
        });
        let want = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.as_bytes());
        let got = color_image(&img);
        assert_eq!(got.size, want.size);
        assert_eq!(got.pixels, want.pixels);
    }

    #[test]
    fn an_empty_image_is_an_empty_texture() {
        let img = lightcraft_raster::Rgba8::new(0, 0);
        assert!(color_image(&img).pixels.is_empty());
    }
}

#[cfg(test)]
mod hdr_tests {
    use super::*;
    use crate::hdr::recorder::Recorder;

    fn picture() -> Arc<HdrPicture> {
        Arc::new(HdrPicture { width: 4, height: 2, rgba16f: vec![0; 4 * 2 * 4], peak: 8.0 })
    }

    // An HDR picture is drawn instead of the 8-bit one while the slot has it; an SDR render that takes
    // its place (HDR switched off, the window on another display) takes it away
    #[test]
    fn an_hdr_picture_replaces_the_8_bit_one_until_an_sdr_render_does() {
        let ctx = egui::Context::default();
        let rec = Arc::new(Recorder::default());
        let sink: Arc<dyn HdrSink> = rec.clone();
        let mut t = Textures { ctx: &ctx, keep_pixels: true, hdr: Some(&sink) };
        let mut view = t.make(Slot::Main, lightcraft_raster::Rgba8::new(4, 2));
        assert_eq!(view.paint_id(), view.tex.id());
        t.set_hdr(Slot::Main, Some(picture()), &mut view);
        assert_eq!(view.paint_id(), egui::TextureId::User(1));
        // the next one is written into the same texture
        t.set_hdr(Slot::Main, Some(picture()), &mut view);
        assert_eq!(view.paint_id(), egui::TextureId::User(1));
        assert_eq!(rec.log.lock().unwrap().len(), 2, "no new texture, no free");
        t.set_hdr(Slot::Main, None, &mut view);
        assert_eq!(view.paint_id(), view.tex.id());
        assert_eq!(rec.log.lock().unwrap().last().map(String::as_str), Some("free User(1)"));
        // a window with no way to draw HDR never has one
        let mut none = Textures { ctx: &ctx, keep_pixels: false, hdr: None };
        none.set_hdr(Slot::Main, Some(picture()), &mut view);
        assert!(view.hdr.is_none());
    }

    // The headless painter draws an HDR picture's id with the 8-bit pixels (the control channel's
    // screenshots of a window that shows HDR)
    #[test]
    fn headless_screenshots_draw_the_8_bit_pixels_of_an_hdr_picture() {
        let ctx = egui::Context::default();
        let sink: Arc<dyn HdrSink> = Arc::new(Recorder::default());
        let mut r = Renderer::default();
        r.keep_pixels = true;
        r.set_hdr_sink(Some(sink.clone()));
        assert!(r.can_draw_hdr() && !r.shows_hdr());
        let mut t = Textures { ctx: &ctx, keep_pixels: true, hdr: Some(&sink) };
        let mut view = t.make(Slot::Main, lightcraft_raster::Rgba8::new(4, 2));
        t.set_hdr(Slot::Main, Some(picture()), &mut view);
        let shown = Shown {
            key: 0,
            photo: lightcraft_catalog::PhotoId(1),
            size: [4, 2],
            histogram: None,
            ms: 0.0,
            quick: None,
            request_id: 0,
            draft: false,
            view,
        };
        r.textures.insert(Slot::Main, shown);
        assert!(r.shows_hdr());
        let cpu = r.cpu_textures();
        assert!(cpu.contains_key(&egui::TextureId::User(1)) && cpu.len() == 2);
    }
}
