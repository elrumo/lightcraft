use super::quick::{QuickSource, shrink};
use super::source::{DecodedSource, SourceLevel, SourceRef, VIEW_REUSE_SLACK};
use super::{
    Arc, DevelopSettings, Hash128, Histogram, PhotoId, PreviewCache, Quality, RenderRequest, Rendered, Rgba8, Source, SourceInfo, StageCache,
};
use lightcraft_raster::SourcePixels;

/// Everything needed to render one photo, detached from the session.
#[derive(Clone)]
pub struct RenderJob {
    /// Assigned by a frontend to distinguish late completions, even for an identical render key.
    pub request_id: u64,
    pub cache_generation: u64,
    /// Content identity for validating a decoded source returned after a reload.
    pub source_key: Option<Hash128>,
    pub photo: PhotoId,
    pub level: SourceLevel,
    pub source: SourceRef,
    /// Where the photo's pixels come from (for executors that can't share `source`, e.g. a web
    /// worker).
    pub origin: Source,
    pub info: SourceInfo,
    pub settings: Arc<DevelopSettings>,
    pub request: RenderRequest,
    /// Identifies the result for caching: hash of settings + request.
    pub key: u64,
    /// Rendered-thumbnail cache and this job's key in it.
    pub cache: Option<(Arc<PreviewCache>, Hash128)>,
    /// The same thumbnail's keys at the larger [`THUMB_SIZES`], smallest first: a miss at this size scales one of
    /// those down instead of rendering again (a grid zoomed out asks for smaller thumbnails than it had).
    pub cache_larger: Vec<Hash128>,
    /// Intermediate results of this view's previous renders (set by the frontend for the loupe):
    /// slider drags then only redo the stages the changed setting feeds.
    pub stages: Option<Arc<StageCache>>,
    /// Keep a full-quality result here as the photo's view preview ([`crate::Session::loupe_job`]).
    pub view_cache: Option<(Arc<PreviewCache>, Hash128)>,
    /// Render for this monitor profile ([`Self::with_display`]).
    pub display: Option<Arc<crate::display::Display>>,
    /// Render the HDR picture beside the 8-bit one ([`Self::with_hdr_display`]).
    pub hdr_view: bool,
    /// Shown on screen at once and not kept: the result may stay on the GPU as a texture
    /// ([`Self::on_screen`]).
    pub on_screen: bool,
    /// Compose the render into a slideshow slide ([`Self::with_slide`]).
    pub slide: Option<Arc<crate::slide::SlideCompose>>,
    /// A face tile that can be cut from the raw's embedded preview, tried before `source`
    /// ([`crate::Session::face_job`]; `None` for every other job).
    pub preview_cut: Option<crate::face_tiles::PreviewCut>,
    /// A face tile the sync server cuts, tried after `preview_cut` and before `source` ([`crate::face_tiles::ServerTile`]; `None` for
    /// every other job).
    pub server_tile: Option<crate::face_tiles::ServerTile>,
    /// Shrink a full-quality result to fit this size (physical pixels) with a Lanczos filter once
    /// it is rendered. The loupe renders a little larger than it is drawn (its edge is rounded up
    /// to a 64 px step) and the window would otherwise shrink the texture with 2-tap bilinear sampling, which blurs fine detail at
    /// small ratios and aliases it at large ones. The caches keep the full render.
    pub display_fit: Option<(usize, usize)>,
}

/// `img` shrunk to fit `max_w × max_h` with a Lanczos filter (see [`RenderJob::display_fit`]).
pub fn fit_for_display(img: &Rgba8, max_w: usize, max_h: usize) -> Rgba8 {
    use lightcraft_raster::resample::{Filter, fit};
    let f = lightcraft_raster::Image {
        width: img.width,
        height: img.height,
        data: img.data.iter().map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2]), f32::from(p[3])]).collect(),
    };
    let small = fit(&f, max_w.max(1), max_h.max(1), Filter::Lanczos3);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Rgba8 { width: small.width, height: small.height, data: small.data.iter().map(|p| [c(p[0]), c(p[1]), c(p[2]), c(p[3])]).collect() }
}

/// A render left on the GPU for the window to draw ([`RenderResult::frame`]). Never made in this fork: upstream's
/// window-device display path of `lightcraft-gpu` is not merged (the fork's GPU renderer reads back), so every
/// render arrives as pixels.
#[derive(Clone, Debug)]
pub struct ScreenFrame {
    pub width: usize,
    pub height: usize,
}

pub struct RenderResult {
    pub request_id: u64,
    pub source_key: Option<Hash128>,
    pub photo: PhotoId,
    pub level: SourceLevel,
    pub key: u64,
    pub rendered: Result<Rendered, String>,
    /// Set when this job's render was tried on the GPU and fell back to the CPU: why
    /// ([`lightcraft_gpu::Fallback::Failed`]). The result is the CPU render either way.
    pub gpu_fallback: Option<String>,
    /// A source that was loaded by this job (to be inserted into the cache).
    pub loaded: Option<DecodedSource>,
    /// Set for a [`QuickJob`]'s stand-in: where the image came from.
    pub quick: Option<QuickSource>,
    /// The display profile (id) the image is encoded for; `None`: sRGB (see
    /// [`crate::display::present`]).
    pub display: Option<u64>,
    /// The HDR render, for a job asked with [`RenderJob::with_hdr_display`] (`image` is its 8-bit
    /// reduction).
    pub hdr: Option<Arc<crate::hdr_view::HdrPicture>>,
    /// The result as a texture on the window's GPU device ([`RenderJob::on_screen`]); `rendered`
    /// then holds its histogram and an empty image.
    pub frame: Option<ScreenFrame>,
}

impl RenderJob {
    /// Render at draft quality (interactive drags). Gets its own result key.
    /// Shrink the full-quality result to fit `w × h` pixels with a Lanczos filter (see
    /// [`Self::display_fit`]). Gets its own result key, so a change of drawn size renders again.
    pub fn with_display_fit(mut self, w: usize, h: usize) -> Self {
        self.display_fit = Some((w, h));
        self.key ^= ((w as u64) << 32 | h as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self
    }

    pub fn draft(mut self) -> Self {
        if self.request.quality != Quality::Draft {
            self.request.quality = Quality::Draft;
            self.key ^= 0x9e37_79b9_7f4a_7c15;
        }
        self
    }

    /// Draw a diagnostic overlay over the result (e.g. Point Color's visualized range). Gets its
    /// own result key.
    pub fn with_overlay(mut self, overlay: lightcraft_pipeline::Overlay) -> Self {
        if self.request.overlay != overlay {
            self.key ^= self.request.overlay.key().wrapping_mul(0x9e37_79b9_7f4a_7c15);
            self.request.overlay = overlay;
            self.key ^= overlay.key().wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
        // a diagnostic view (visualized range / spots) must never become the photo's cached preview
        if overlay != lightcraft_pipeline::Overlay::None {
            self.view_cache = None;
        }
        self
    }

    /// Render the HDR picture of an HDR edit ([`RenderResult::hdr`]) for a display that can show
    /// it: the float HDR render in Display P3 primaries (what an HDR-capable screen shows; the system
    /// colour-manages an HDR picture, so no monitor profile), encoded with the extended sRGB curve
    /// ([`crate::hdr_view::HdrPicture`]), whose 8-bit reduction is `image`. Other
    /// jobs are left as they are (an SDR edit, a diagnostic overlay, a soft proof, a thumbnail). Gets
    /// its own result key and never becomes the photo's cached preview.
    pub fn with_hdr_display(mut self) -> Self {
        let plain = self.request.depth == lightcraft_pipeline::OutputDepth::U8
            && self.request.overlay == lightcraft_pipeline::Overlay::None
            && self.request.proof.is_none()
            && self.cache.is_none();
        // (Generative Expand's missing area is transparent in the 8-bit picture, and an HDR one has no alpha)
        if !plain || self.hdr_view || !self.settings.hdr.enabled || self.settings.crop.expand {
            return self;
        }
        self.key ^= 0x4844_525f_5649_4557u64.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.hdr_view = true;
        self.request.space = lightcraft_pipeline::OutputSpace::DisplayP3;
        self.request.display = None;
        self.display = None;
        self.view_cache = None;
        self
    }

    /// Soft-proof the result (see [`lightcraft_pipeline::Proof`]). Gets its own result key and is
    /// never kept as the photo's cached preview.
    pub fn with_proof(mut self, proof: Option<lightcraft_pipeline::Proof>) -> Self {
        if self.request.proof != proof {
            let k = |p: Option<lightcraft_pipeline::Proof>| p.map_or(0, |p| p.key()).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
            self.key ^= k(self.request.proof) ^ k(proof);
            self.request.proof = proof;
        }
        if proof.is_some() {
            self.view_cache = None;
        }
        self
    }

    /// Render without the film effects' frame (`film.frame`), for a screen that maps taps onto the photo (Masking, Remove, Red Eye): the frame
    /// shrinks the photo into its window, so a tap would land elsewhere on it. The photo's settings are untouched; the result gets its own key
    /// and is never cached as the photo's preview or thumbnail.
    pub fn with_film_frame(mut self, shown: bool) -> Self {
        if shown || self.settings.film.frame == lightcraft_develop::FilmFrame::None {
            return self;
        }
        let mut s = (*self.settings).clone();
        s.film.frame = lightcraft_develop::FilmFrame::None;
        self.key ^= s.hash64() ^ self.settings.hash64() ^ 0x5bd1_e995_f1f2_a3b4;
        self.settings = Arc::new(s);
        self.cache = None;
        self.view_cache = None;
        self.preview_cut = None;
        self.server_tile = None;
        self
    }

    /// The result is only drawn (an interactive draft of a view): when the GPU renderer runs on the
    /// window's device it may hand back a texture ([`RenderResult::frame`]) instead of reading the
    /// pixels back. Ignored where pixels are needed: thumbnails (cached), a full-quality render
    /// kept as the view preview, and a monitor profile (corrected on the CPU).
    pub fn on_screen(mut self) -> Self {
        self.on_screen = true;
        self
    }

    /// Render with some colour grading wheels (shadows, midtones, highlights) switched off for
    /// viewing (the wheel eyes in the Edit panel). The photo's settings are untouched; the result
    /// gets its own key and is never cached as the photo's preview or thumbnail.
    pub fn with_hidden_grading(mut self, hidden: [bool; 3]) -> Self {
        if !hidden.iter().any(|h| *h) {
            return self;
        }
        let mut s = (*self.settings).clone();
        let g = &mut s.grading;
        for (off, wheel) in hidden.into_iter().zip([&mut g.shadows, &mut g.midtones, &mut g.highlights]) {
            if off {
                *wheel = lightcraft_develop::Wheel::default();
            }
        }
        let bits = hidden.iter().enumerate().fold(0u64, |b, (i, h)| b | ((*h as u64) << i));
        self.key ^= s.hash64() ^ self.settings.hash64() ^ bits.wrapping_mul(0xd6e8_feb8_6659_fd93);
        self.settings = Arc::new(s);
        self.cache = None;
        self.view_cache = None;
        self.preview_cut = None;
        self.server_tile = None;
        self
    }

    /// Reuse `stages` across this view's renders.
    pub fn with_stages(mut self, stages: Arc<StageCache>) -> Self {
        self.stages = Some(stages);
        self
    }

    /// Render for a monitor profile: into the display's primaries (its gamut, not sRGB's), ready
    /// to show ([`RenderResult::display`]). The histogram and the cached view preview stay sRGB.
    /// Gets its own result key. Thumbnail jobs (rendered-thumbnail cache) are left as they are:
    /// [`crate::display::present`] converts their sRGB results.
    pub fn with_display(mut self, display: Option<Arc<crate::display::Display>>) -> Self {
        // (a slide is composed in sRGB, backdrop colours included; the frontend presents it)
        if self.cache.is_some() || self.hdr_view || self.slide.is_some() || self.request.depth != lightcraft_pipeline::OutputDepth::U8 {
            return self;
        }
        let k = |d: &Option<Arc<crate::display::Display>>| d.as_ref().map_or(0, |d| d.id()).wrapping_mul(0xd6e8_feb8_6659_fd93);
        self.key ^= k(&self.display) ^ k(&display);
        self.request.display = display.as_ref().map(|d| d.space);
        self.display = display;
        self
    }

    /// Render. A panic inside (a decoder or pipeline bug) is this job's failure: the view waiting
    /// for it is told, instead of waiting forever on a worker that died (the pool doesn't replace
    /// it, so a few such panics used to stop every render).
    pub fn run(self) -> RenderResult {
        let failed = self.failed(String::new());
        crate::guard::catch("the render", || self.run_unguarded()).unwrap_or_else(|e| RenderResult { rendered: Err(e), ..failed })
    }

    /// [`Self::run`] for an export too big to render whole on this device, in bands of rows: each band is a window of the
    /// output ([`lightcraft_pipeline::PixelWindow`], the zoom view's own region render) with some context above and below
    /// it, rendered and cut to its rows, so the pipeline's working memory is a band's, not the whole picture's. The
    /// source is decoded once and the bands are stitched into the 8-bit result. Only for a job [`bandable`] says can be.
    pub fn run_banded(self) -> RenderResult {
        let failed = self.failed(String::new());
        crate::guard::catch("the render", || self.run_banded_unguarded()).unwrap_or_else(|e| RenderResult { rendered: Err(e), ..failed })
    }

    fn run_banded_unguarded(self) -> RenderResult {
        if !bandable(&self.settings, &self.request) {
            return self.run_photo();
        }
        let source = match self.source.load_source() {
            Ok(s) => s,
            Err(e) => return self.failed(e),
        };
        let (src, info) = source.develop_input(&self.settings, self.info.clone());
        // The whole output's size, and the settings every band renders with (Upright and the profile resolved once, not
        // by each of the bands).
        let (w, h, settings) = {
            let plan = lightcraft_pipeline::plan(&src, &info, &self.settings, &self.request);
            (plan.w, plan.h, plan.settings.into_owned())
        };
        let (rows, halo) = band_shape(w, h);
        let mut out = Rgba8::new(w, h);
        let mut fallback = None;
        let mut y = 0;
        while y < h {
            let keep = rows.min(h - y);
            let (top, bottom) = (y.saturating_sub(halo), (y + keep + halo).min(h));
            let request = RenderRequest { window: Some(lightcraft_pipeline::PixelWindow { x: 0, y: top, w, h: bottom - top }), ..self.request };
            let (band, why) = develop_image(&src, &info, &settings, &request, None, self.cache.is_none());
            fallback = fallback.or(why);
            let at = y - top;
            if band.image.width != w || band.image.height < at + keep {
                return self.failed(format!("a band of the render came back {}×{} for {w} px wide rows", band.image.width, band.image.height));
            }
            for row in 0..keep {
                out.row_mut(y + row).copy_from_slice(band.image.row(at + row));
            }
            y += keep;
        }
        let histogram = Histogram::of_srgb8(&out);
        RenderResult { rendered: Ok(Rendered { image: out, histogram, deep: None }), gpu_fallback: fallback, ..self.failed(String::new()) }
    }

    /// This job's result when it fails with `e`.
    fn failed(&self, e: String) -> RenderResult {
        RenderResult {
            request_id: self.request_id,
            source_key: self.source_key,
            photo: self.photo,
            level: self.level,
            key: self.key,
            rendered: Err(e),
            gpu_fallback: None,
            loaded: None,
            quick: None,
            display: None,
            hdr: None,
            frame: None,
        }
    }

    /// Compose the render into a slideshow slide (layout, options, backdrop): the result is the
    /// slide, not the photo. Gets its own result key; the photo's cached view preview stays the
    /// photo. The slide stays sRGB ([`Self::with_display`] leaves it alone).
    pub fn with_slide(mut self, slide: Arc<crate::slide::SlideCompose>) -> Self {
        let k = |s: &Option<Arc<crate::slide::SlideCompose>>| s.as_ref().map_or(0, |s| s.key()).wrapping_mul(0xa24b_aed4_963e_e407);
        self.key ^= k(&self.slide);
        self.key ^= k(&Some(slide.clone()));
        self.slide = Some(slide);
        self
    }

    /// This thumbnail from the cache: at its own size, else scaled down from a larger one (and kept at its own size).
    fn cached(&self) -> Option<Rgba8> {
        let (cache, key) = self.cache.as_ref()?;
        if let Some(img) = cache.get_at(self.cache_generation, *key) {
            return Some(Arc::unwrap_or_clone(img));
        }
        let img = self.cache_larger.iter().find_map(|k| cache.get_at(self.cache_generation, *k))?;
        let edge = self.request.max_w.max(self.request.max_h);
        let small = shrink(&img, edge, edge);
        cache.put_at(self.cache_generation, *key, Arc::new(small.clone()));
        Some(small)
    }

    /// [`Self::run_photo`], composed into its slide when this is a slide job (inside [`Self::run`]'s guard).
    fn run_unguarded(self) -> RenderResult {
        let slide = self.slide.clone();
        let mut r = self.run_photo();
        if let (Some(slide), Ok(rendered)) = (slide, r.rendered.as_mut()) {
            rendered.image = slide.compose(&rendered.image);
            rendered.deep = None;
        }
        r
    }

    /// The view render kept for this job's photo and settings ([`Self::view_cache`]), fitted to the
    /// size asked for, when it is at least that big, or unchanged when it covers the job's display
    /// fit (else `None`: render). Only for what the cached
    /// view is: a full-quality, 8-bit, whole-frame SDR render (the cache holds sRGB; a display
    /// profile is applied to it like to any cached picture, [`crate::display::present`]). Not for a display whose
    /// histogram counts the picture as shown ([`crate::display::Display::screen_histogram`], a Display P3 screen): its
    /// pictures are rendered into its primaries, where the sRGB cache would clip the colours outside sRGB and give the
    /// sRGB picture's histogram.
    pub fn cached_view(&self) -> Option<Rgba8> {
        let (cache, key) = self.view_cache.as_ref()?;
        if self.request.quality != Quality::Full
            || self.request.depth != lightcraft_pipeline::OutputDepth::U8
            || self.request.window.is_some()
            || self.settings.hdr.enabled
            || self.slide.is_some()
            || self.display.as_ref().is_some_and(|d| d.screen_histogram)
        {
            return None;
        }
        let img = cache.get_at(self.cache_generation, *key)?;
        fit_cached(&img, self.request.max_w, self.request.max_h).or_else(|| {
            // the loupe at rest asks for at least the preview level and shows the render shrunk to
            // the drawn size (the display fit): a cached view that covers the drawn size is enough
            let (fw, fh) = self.display_fit?;
            let (w, h) = (img.width, img.height);
            let s = (fw as f64 / w.max(1) as f64).min(fh as f64 / h.max(1) as f64);
            (w > 0 && h > 0 && fw > 0 && fh > 0 && s <= 1.0 + VIEW_REUSE_SLACK).then(|| Arc::unwrap_or_clone(img))
        })
    }

    /// The photo's render (without slide composition).
    fn run_photo(self) -> RenderResult {
        // the loupe: a cached view render of these settings at least this big is the render (built
        // previews and the photos prepared ahead are used, not rendered again; PERF-PREVIEW-REUSE)
        if let Some(mut image) = self.cached_view() {
            // as for a render: the histogram of the full picture, then the display fit
            let histogram = Histogram::of_srgb8(&image);
            if let Some((fw, fh)) = self.display_fit
                && (image.width > fw.max(1) || image.height > fh.max(1))
            {
                image = fit_for_display(&image, fw, fh);
            }
            return RenderResult {
                request_id: self.request_id,
                source_key: self.source_key,
                photo: self.photo,
                level: self.level,
                key: self.key,
                rendered: Ok(Rendered { image, histogram, deep: None }),
                gpu_fallback: None,
                loaded: None,
                quick: Some(QuickSource::Cached),
                display: None,
                hdr: None,
                frame: None,
            };
        }
        if let Some(image) = self.cached() {
            let histogram = Histogram::of_srgb8(&image);
            return RenderResult {
                request_id: self.request_id,
                source_key: self.source_key,
                photo: self.photo,
                level: self.level,
                key: self.key,
                rendered: Ok(Rendered { image, histogram, deep: None }),
                gpu_fallback: None,
                loaded: None,
                quick: None,
                display: None,
                hdr: None,
                frame: None,
            };
        }
        // a face tile of an unedited raw: cut from the camera's embedded preview when it is big enough
        // (the sensor data is never read for it); of a synced photo with no preview here: cut by the server, a few KB
        // instead of its whole preview; else rendered from `source` below
        let tile = self.preview_cut.as_ref().and_then(|c| c.cut()).or_else(|| self.server_tile.as_ref().and_then(|t| t.fetch()));
        if let Some(image) = tile {
            if let Some((cache, key)) = &self.cache {
                cache.put_at(self.cache_generation, *key, Arc::new(image.clone()));
            }
            let histogram = Histogram::of_srgb8(&image);
            return RenderResult {
                request_id: self.request_id,
                source_key: self.source_key,
                photo: self.photo,
                level: self.level,
                key: self.key,
                rendered: Ok(Rendered { image, histogram, deep: None }),
                gpu_fallback: None,
                loaded: None,
                quick: None,
                display: None,
                hdr: None,
                frame: None,
            };
        }
        let was_loaded = matches!(self.source, SourceRef::Loaded(_));
        // The app is closing (issue #620): nobody is left to show this, and quitting waits for the
        // jobs that are running. Without this, one that lost the GPU mid-way would render on the CPU.
        let closing = || lightcraft_gpu::shutting_down().then(|| "LightCraft is closing".to_string());
        let source = match closing() {
            Some(e) => Err(e),
            None => self.source.load_source().and_then(|s| closing().map_or(Ok(s), Err)),
        };
        match source {
            Ok(source) => {
                // the plain picture, or (Denoise amount above 0 and a denoised picture cached) the mix the amount asks for,
                // in the camera's own look for the Camera Matching profile
                let (sdr_src, info) = source.develop_input(&self.settings, self.info);
                // An HDR render (the loupe's HDR picture, an HDR export) of a file with a gain map starts from the
                // highlights that map holds, of a display-referred file without one from those SDR to HDR lifts; an SDR
                // render, and a raw, from the base picture.
                let wants_hdr = self.hdr_view || self.request.depth == lightcraft_pipeline::OutputDepth::F32Hdr;
                let hdr_src = (wants_hdr || self.settings.hdr.enabled).then(|| source.hdr_input(&sdr_src, &info, &self.settings.hdr));
                let src = if wants_hdr { hdr_src.as_ref().unwrap_or(&sdr_src) } else { &sdr_src };
                // Views and exports use the GPU when there is one (waiting for the device if it is
                // still being created). Thumbnails (grid, filmstrip, preset and profile previews)
                // use it once it is ready - ~20-30 % faster than the CPU at 256 px with six jobs
                // side by side (`crates/gpu/tests/it/thumbs_bench.rs`), and the cores stay free for
                // decoding - but never wait for it: at startup they render on the CPU.
                let gpu = self.cache.is_none() || lightcraft_gpu::ready();
                // a clipping preview tests sRGB values, like the histogram: rendered in sRGB, then
                // converted for the display below (an HDR view is never a diagnostic overlay, see `with_hdr_display`);
                // for a display whose histogram is of the picture shown, the display's values, as the histogram does
                let clip_view = self.request.overlay.is_clipping() && !self.display.as_ref().is_some_and(|d| d.screen_histogram);
                let request = if clip_view { RenderRequest { display: None, ..self.request } } else { self.request };
                if let Err(e) = lightcraft_pipeline::check_brush_budget(src, &info, &self.settings, &request) {
                    return RenderResult {
                        request_id: self.request_id,
                        source_key: self.source_key,
                        photo: self.photo,
                        level: self.level,
                        key: self.key,
                        rendered: Err(e),
                        gpu_fallback: None,
                        loaded: None,
                        quick: None,
                        display: None,
                        hdr: None,
                        frame: None,
                    };
                }
                // (an HDR view needs its HDR picture's pixels)
                let pixels_needed = self.hdr_view
                    || self.cache.is_some()
                    || self.display.is_some()
                    || (self.view_cache.is_some() && self.request.quality == Quality::Full);
                let hdr_or_src = hdr_src.as_ref().unwrap_or(src);
                let frame = (self.on_screen && !pixels_needed && gpu && gpu_renders(&self.settings, &request))
                    .then(|| develop_on_screen(src, hdr_or_src, &info, &self.settings, &request, self.stages.as_deref()));
                let (mut rendered, gpu_fallback, hdr) = match frame {
                    Some(OnScreen::Frame(frame, histogram)) => {
                        return RenderResult {
                            request_id: self.request_id,
                            source_key: self.source_key,
                            photo: self.photo,
                            level: self.level,
                            key: self.key,
                            rendered: Ok(Rendered {
                                image: lightcraft_raster::Rgba8 { width: 0, height: 0, data: Vec::new() },
                                histogram,
                                deep: None,
                            }),
                            gpu_fallback: None,
                            loaded: (!was_loaded).then_some(source),
                            quick: None,
                            display: None,
                            hdr: None,
                            frame: Some(frame),
                        };
                    }
                    Some(OnScreen::Pixels(r)) => (r, None, None),
                    None if self.hdr_view => {
                        let request = RenderRequest { depth: lightcraft_pipeline::OutputDepth::F32Hdr, ..request };
                        develop_hdr(src, &info, &self.settings, &request, self.stages.as_deref(), gpu)
                    }
                    None => {
                        let (r, f) = develop_reporting_hdr(src, hdr_or_src, &info, &self.settings, &request, self.stages.as_deref(), gpu);
                        (r, f, None)
                    }
                };
                // for a display profile: the image goes to the display's values, the histogram
                // and the cached view preview stay sRGB
                let mut srgb = None;
                let display = match (&self.display, self.request.display) {
                    // a diagnostic view: the histogram stays the photo's (taken before the overlay
                    // was drawn), and nothing goes to the view cache
                    (Some(d), Some(_)) if self.request.overlay != lightcraft_pipeline::Overlay::None => {
                        let converted = if clip_view { d.profile.from_srgb(&mut rendered.image) } else { d.profile.correct(&mut rendered.image) };
                        if let Err(e) = converted {
                            log::warn!("display profile: {e}");
                        }
                        Some(d.id())
                    }
                    (Some(d), Some(_)) => {
                        let keep = self.view_cache.is_some() && self.request.quality == Quality::Full;
                        let (s, h) = crate::display::finish_render(&mut rendered.image, d, keep);
                        // (an HDR edit's HDR histogram stays)
                        if let Some(h) = h.filter(|_| rendered.histogram.hdr.is_none()) {
                            rendered.histogram = h;
                        }
                        srgb = s;
                        Some(d.id())
                    }
                    _ => None,
                };
                if let Some((cache, key)) = &self.cache {
                    cache.put_at(self.cache_generation, *key, Arc::new(rendered.image.clone()));
                }
                if let Some((cache, key)) = &self.view_cache
                    && self.request.quality == Quality::Full
                    && (display.is_none() || srgb.is_some())
                {
                    cache.put_deferred_at(self.cache_generation, *key, Arc::new(srgb.unwrap_or_else(|| rendered.image.clone())));
                }
                // the display fit, after the caches have the full render
                if let Some((fw, fh)) = self.display_fit
                    && self.request.quality == Quality::Full
                {
                    let (w, h) = (rendered.image.width, rendered.image.height);
                    if w > fw.max(1) || h > fh.max(1) {
                        rendered.image = fit_for_display(&rendered.image, fw, fh);
                    }
                }
                RenderResult {
                    request_id: self.request_id,
                    source_key: self.source_key,
                    photo: self.photo,
                    level: self.level,
                    key: self.key,
                    rendered: Ok(rendered),
                    gpu_fallback,
                    loaded: (!was_loaded).then_some(source),
                    quick: None,
                    display,
                    hdr,
                    frame: None,
                }
            }
            Err(e) => self.failed(e),
        }
    }
}

/// [`develop_reporting`] for the HDR picture of a [`RenderJob::with_hdr_display`] job (`req` asks for
/// the float HDR render): on the GPU where an SDR render would be (it writes the picture's half floats
/// itself), else the CPU's float render, packed. The 8-bit image is the picture's reduction; the
/// histogram shows its HDR range, as for an HDR edit's SDR previews. Also says why the GPU render fell
/// back to the CPU, as [`develop_reporting`].
pub fn develop_hdr(
    src: impl Into<SourcePixels>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    gpu: bool,
) -> (Rendered, Option<String>, Option<Arc<crate::hdr_view::HdrPicture>>) {
    use crate::hdr_view::HdrPicture;
    let src: SourcePixels = src.into();
    let peak = s.hdr.peak();
    let mut fallback = None;
    if gpu && gpu_renders(s, req) {
        match lightcraft_gpu::try_render_hdr(&src, info, s, req, stages) {
            Ok((r, halves)) => match HdrPicture::from_halves(r.image.width, r.image.height, halves, peak) {
                // (its histogram is the picture's HDR one)
                Some(p) => return (r, None, Some(Arc::new(p))),
                None => fallback = Some(format!("{}×{}: the GPU's HDR picture has the wrong size", r.image.width, r.image.height)),
            },
            Err(lightcraft_gpu::Fallback::Failed(why)) => fallback = Some(why),
            Err(lightcraft_gpu::Fallback::NotTried) => {}
        }
    }
    let (mut rendered, _) = develop_image(&src, info, s, req, stages, false);
    let deep = rendered.deep.take();
    if let Some(lightcraft_pipeline::DeepSamples::F32(v)) = deep.as_ref().map(|d| &d.samples) {
        rendered.histogram = Histogram::of_hdr(v.as_chunks::<3>().0, lightcraft_develop::Hdr::MAX_EV_LIMIT as f32);
    }
    (rendered, fallback, deep.and_then(|d| HdrPicture::from_deep(&d, peak)).map(Arc::new))
}

/// Whether a render of `s` for `req` can go to the GPU: masks with Shift Edge or Feather have no GPU stage, nor has a
/// LUT profile bigger than the kernel takes ([`lightcraft_pipeline::lut::GPU_MAX_SIZE`]). (LUT profiles, Refine Edges
/// and soft proofs render on the GPU too; the GPU renderer declines by itself what it can't draw.)
fn gpu_renders(s: &DevelopSettings, req: &RenderRequest) -> bool {
    lightcraft_pipeline::lut::get(&s.profile.id).is_none_or(|l| l.size <= lightcraft_pipeline::lut::GPU_MAX_SIZE)
        && !s.masks.iter().any(|m| m.visible && (m.shift != 0.0 || m.feather > 0.0))
        // (the GPU renderer draws film effects onto its 8-bit picture only: a deep render with them is the CPU's)
        && (req.depth == lightcraft_pipeline::OutputDepth::U8 || s.film.is_default())
}

/// `img` fitted into `max_w × max_h` (its aspect kept), when it is big enough to need no
/// enlarging (within [`VIEW_REUSE_SLACK`]): as it is, or shrunk by averaging.
pub fn fit_cached(img: &Rgba8, max_w: usize, max_h: usize) -> Option<Rgba8> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || max_w == 0 || max_h == 0 {
        return None;
    }
    let s = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    if !s.is_finite() || s > 1.0 + VIEW_REUSE_SLACK {
        return None;
    }
    let (nw, nh) = (((w as f64 * s).round() as usize).clamp(1, w), ((h as f64 * s).round() as usize).clamp(1, h));
    if nw == w && nh == h {
        return Some(img.clone());
    }
    Some(shrink_rgba(img, nw, nh))
}

/// `img` shrunk to `nw × nh` (no larger than it), each pixel the average of the ones it covers.
fn shrink_rgba(img: &Rgba8, nw: usize, nh: usize) -> Rgba8 {
    let (w, h) = (img.width, img.height);
    let (nw, nh) = (nw.clamp(1, w.max(1)), nh.clamp(1, h.max(1)));
    // the source columns / rows each output column / row covers: [start, end)
    let spans = |n: usize, of: usize| -> Vec<(usize, usize)> {
        (0..n)
            .map(|i| {
                let a = i.saturating_mul(of) / n;
                let b = (i.saturating_add(1).saturating_mul(of) / n).max(a.saturating_add(1)).min(of);
                (a, b)
            })
            .collect()
    };
    let (xs, ys) = (spans(nw, w), spans(nh, h));
    let mut data = Vec::with_capacity(nw.saturating_mul(nh));
    let mut row = vec![[0u32; 4]; nw];
    for &(y0, y1) in &ys {
        row.iter_mut().for_each(|r| *r = [0; 4]);
        for y in y0..y1 {
            let Some(line) = img.data.get(y * w..(y + 1) * w) else { continue };
            for (acc, &(x0, x1)) in row.iter_mut().zip(&xs) {
                for px in line.get(x0..x1).unwrap_or(&[]) {
                    for (a, v) in acc.iter_mut().zip(px) {
                        *a += u32::from(*v);
                    }
                }
            }
        }
        for (acc, &(x0, x1)) in row.iter().zip(&xs) {
            let n = (x1.saturating_sub(x0).saturating_mul(y1.saturating_sub(y0))).max(1) as u32;
            data.push(acc.map(|a| ((a + n / 2) / n).min(255) as u8));
        }
    }
    Rgba8 { width: nw, height: nh, data }
}

/// Render `src`: on the GPU when `gpu` is set and a GPU is available (`lightcraft_gpu`), else on
/// the CPU, reusing `stages` either way. Both produce the same image within 1–3 LSB (see
/// `docs/gpu-pipeline.md`); `LIGHTCRAFT_GPU=0` or [`lightcraft_gpu::set_enabled`] forces the CPU.
pub fn develop(
    src: impl Into<SourcePixels>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    gpu: bool,
) -> Rendered {
    develop_reporting(src, info, s, req, stages, gpu).0
}

/// [`develop`], also saying why the render fell back from the GPU to the CPU when it did (`None`:
/// it rendered on the GPU, or the GPU was not tried). The reason is this render's own, not the
/// process-wide [`lightcraft_gpu::last_fallback`].
pub fn develop_reporting(
    src: impl Into<SourcePixels>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    gpu: bool,
) -> (Rendered, Option<String>) {
    let src: SourcePixels = src.into();
    develop_reporting_hdr(&src, &src, info, s, req, stages, gpu)
}

/// [`develop_reporting`] for a source that has an HDR picture of its own (`hdr_src`, see
/// [`DecodedSource::hdr_input`]): the render is of `src`, the HDR histogram of an HDR edit's preview measures `hdr_src`.
pub fn develop_reporting_hdr(
    src: &SourcePixels,
    hdr_src: &SourcePixels,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    gpu: bool,
) -> (Rendered, Option<String>) {
    let (mut rendered, fallback) = develop_image(src, info, s, req, stages, gpu);
    // an HDR edit's preview is its SDR rendition: its histogram shows the HDR range instead
    if let Some(h) = lightcraft_pipeline::hdr_histogram(hdr_src, info, s, req) {
        rendered.histogram = h;
    }
    (rendered, fallback)
}

/// Pixels one band of a banded export renders, context included: the working memory of a 4 MP render, whatever the
/// size of the export.
const BAND_PIXELS: usize = 4 << 20;

/// The rows a band of an export `w × h` keeps and the rows of context rendered above and below them. The wide stages
/// (the base, clarity and dehaze planes) are made from the whole frame at reduced size and do not need the context;
/// the spatial ones (sharpening, noise reduction) need a few hundred pixels at most, a sixtieth of the frame's long edge
/// up to 256.
pub(crate) fn band_shape(w: usize, h: usize) -> (usize, usize) {
    let halo = (w.max(h) / 60).clamp(64, 256);
    ((BAND_PIXELS / w.max(1)).saturating_sub(2 * halo).max(64), halo)
}

/// Pixels a band of an export `w × h` renders (its rows and context, all the width).
pub(crate) fn band_pixels(w: usize, h: usize) -> usize {
    let (rows, halo) = band_shape(w, h);
    (rows + 2 * halo).min(h).saturating_mul(w)
}

/// Whether an export of `s` at `depth` can be rendered in bands ([`RenderJob::run_banded`]): an 8-bit picture that a
/// region render carries. Not a deep (16 bit, float) or HDR one, which a window does not render, nor one with Lens
/// Blur, whose reach grows a window past what a band holds.
pub(crate) fn bandable(s: &DevelopSettings, req: &RenderRequest) -> bool {
    bandable_at(s, req.depth)
        && req.window.is_none()
        && req.proof.is_none()
        && req.display.is_none()
        && req.overlay == lightcraft_pipeline::Overlay::None
}

/// [`bandable`] for the settings and sample format of an export that is not set up yet.
pub(crate) fn bandable_at(s: &DevelopSettings, depth: lightcraft_pipeline::OutputDepth) -> bool {
    depth == lightcraft_pipeline::OutputDepth::U8 && !s.hdr.enabled && !s.lens_blur.enabled
}

fn develop_image(
    src: &SourcePixels,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    gpu: bool,
) -> (Rendered, Option<String>) {
    let fallback = if gpu && gpu_renders(s, req) {
        match lightcraft_gpu::try_render(src, info, s, req, stages) {
            Ok(r) => return (r, None),
            Err(lightcraft_gpu::Fallback::Failed(why)) => Some(why),
            Err(lightcraft_gpu::Fallback::NotTried) => None,
        }
    } else {
        None
    };
    let rendered = match stages {
        Some(st) => lightcraft_pipeline::render_cached(src.clone(), info, s, req, st),
        None => lightcraft_pipeline::render(src, info, s, req),
    };
    (rendered, fallback)
}

/// What [`develop_on_screen`] made.
enum OnScreen {
    /// A texture on the window's device, and the histogram (never in the browser build).
    #[allow(dead_code)] // the GPU display path is not merged: see `ScreenFrame`
    Frame(ScreenFrame, Histogram),
    /// Pixels (GPU or CPU).
    Pixels(Rendered),
}

/// [`develop`] for a render that is only drawn: a texture on the window's device when the GPU
/// renderer can hand one over, else the pixels. `hdr_src` is what an HDR edit's histogram measures
/// ([`develop_reporting_hdr`]).
fn develop_on_screen(
    src: &SourcePixels,
    hdr_src: &SourcePixels,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
) -> OnScreen {
    OnScreen::Pixels(develop_reporting_hdr(src, hdr_src, info, s, req, stages, false).0)
}
