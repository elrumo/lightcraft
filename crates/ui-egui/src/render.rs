//! The egui side of background rendering. [`Scheduler`] (in `lightcraft-apphost`, shared with native
//! frontends) decides what renders when and what each slot shows; this module turns its
//! pictures into egui textures ([`TexView`]) and asks for repaints while renders are in flight.
//!
//! [`Renderer`] derefs to the scheduler, so the panels use one object for both.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

pub use lightcraft_apphost::render::{DraftBudget, MAX_VARIANT_TEXTURES, RenderOffload, Shown, Slot, VARIANT_TEXTURES, geometry_look};
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
            look: None,
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
mod thumbnail_tests;

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
        let mut r = Renderer { keep_pixels: true, ..Default::default() };
        r.set_hdr_sink(Some(sink.clone()));
        assert!(r.can_draw_hdr() && !r.shows_hdr());
        let mut t = Textures { ctx: &ctx, keep_pixels: true, hdr: Some(&sink) };
        let mut view = t.make(Slot::Main, lightcraft_raster::Rgba8::new(4, 2));
        t.set_hdr(Slot::Main, Some(picture()), &mut view);
        let shown = Shown {
            look: None,
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
