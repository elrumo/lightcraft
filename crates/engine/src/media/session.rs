use super::quick::{QuickJob, content_key, source_info};
use super::render::{RenderJob, RenderResult};
use super::source::{PreviewLoader, RENDER_CACHE_VERSION, SourceLevel, SourceRef, THUMB_SIZES, face_edge, source_edge_needed, thumb_bucket};
use super::*;

impl crate::Session {
    /// Build a render job for `id` fitting `max_w × max_h`. `before` renders the unedited look.
    pub fn render_job(&mut self, id: PhotoId, max_w: usize, max_h: usize, before: bool, apply_crop: bool) -> Option<RenderJob> {
        self.build_job(id, max_w, max_h, before, apply_crop, None)
    }

    /// A grid/filmstrip thumbnail job: the long edge is rounded up to one of [`THUMB_SIZES`]
    /// (≥ `long_edge`) and the result comes from / goes to the thumbnail cache (memory + disk).
    pub fn thumb_job(&mut self, id: PhotoId, long_edge: usize) -> Option<RenderJob> {
        self.thumb_job_for(id, long_edge, true)
    }

    /// [`Session::thumb_job`] for a cell that is `shown` on screen, or only in the grid's reach (its
    /// preview comes after those on screen).
    pub fn thumb_job_for(&mut self, id: PhotoId, long_edge: usize, shown: bool) -> Option<RenderJob> {
        let b = thumb_bucket(long_edge);
        self.note_thumb_ask(id, shown);
        let mut job = self.build_job(id, b, b, false, true, Some(b))?;
        // (a thumbnail may develop a smaller copy of the raw image that the file keeps)
        if let SourceRef::File { reduced, .. } = &mut job.source {
            reduced.clone_from(&self.media.reduced_loader);
        }
        Some(job)
    }

    /// A synced photo without any preview here was asked for (see [`MediaCache::thumb_asks`]). A host
    /// calls this every frame for each cell it shows that has no picture yet — also when it doesn't build
    /// the cell's render job again, as it doesn't while a render that failed for want of the preview stands
    /// — so the ask lasts exactly as long as the cell is drawn. A photo that has a preview here, or isn't
    /// a synced one, is no ask, and costs a lookup.
    /// (Not while syncing is paused or signed out: the asks would pile up, all as fresh as the last step.)
    pub fn note_thumb_ask(&mut self, id: PhotoId, shown: bool) {
        if self.sync_state().is_none_or(|st| st.config.paused || !st.has_token()) {
            return;
        }
        // (the cell asked a moment ago: no need to look at its files again)
        if let Some(ask) = self.media.thumb_asks.get_mut(&id) {
            self.media.thumb_seq = self.media.thumb_seq.wrapping_add(1);
            *ask = ThumbAsk { tick: self.media.thumb_tick, seq: self.media.thumb_seq, shown };
            return;
        }
        if self.catalog.photo(id).and_then(|p| self.synced_tier(p)) != Some(0) {
            return;
        }
        let m = &mut self.media;
        m.thumb_seq = m.thumb_seq.wrapping_add(1);
        m.thumb_asks.insert(id, ThumbAsk { tick: m.thumb_tick, seq: m.thumb_seq, shown });
    }

    fn build_job(
        &mut self,
        id: PhotoId,
        max_w: usize,
        max_h: usize,
        before: bool,
        apply_crop: bool,
        thumb_bucket: Option<usize>,
    ) -> Option<RenderJob> {
        let p = self.catalog.photo(id)?.clone();
        let settings = if before { Arc::new(self.before_settings(&p)) } else { p.develop.clone() };
        let level = SourceLevel::for_size(if thumb_bucket.is_some() {
            max_w.max(max_h) // (grid thumbnails of tight crops stay cheap)
        } else {
            // the photo fitted into the box, not the box: a tall canvas shows a landscape photo at the
            // canvas's width (an iPad's 2,064 x 2,752 canvas read a 48 MP original for a 2,064 px picture)
            let frame = lightcraft_pipeline::geometry::Frame::with_lens(
                p.width.max(1) as usize,
                p.height.max(1) as usize,
                &settings,
                apply_crop,
                p.embedded_lens.as_ref(),
            );
            let (fw, fh) = frame.fit(max_w, max_h);
            source_edge_needed(&p, &settings, apply_crop, fw.max(fh))
        });
        let source = self.media.source_ref(&p, level);
        let request = RenderRequest { apply_crop, ..RenderRequest::fit(max_w, max_h) };
        // the photo id is part of the key: two photos with the same settings and size must not
        // share a result (a view slot showing photo A would otherwise look current for photo B)
        // …and so is the file's content: a file changed on disk (Reload) renders afresh
        let content_key = content_key(&p);
        let source_key = Hasher128::new().str(&content_key).finish();
        // …and, for a synced photo whose original isn't here, which of its previews it has
        let tier = self.synced_tier(&p).unwrap_or(0);
        let content = (source_key.0 as u64) ^ u64::from(tier).wrapping_mul(0xd6e8_feb8_6659_fd93);
        // Temporary "before" settings must not replace the photo's cached develop hash.
        let settings_hash = if before { settings.hash64() } else { self.media.settings_hashes.get(id, &settings) };
        let salt = self.media.denoise_salt(id, &settings);
        let key = settings_hash
            ^ ((max_w as u64) << 40)
            ^ ((max_h as u64) << 20)
            ^ (apply_crop as u64)
            ^ (level as u64) << 60
            ^ id.0.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ content.rotate_left(17)
            ^ salt;
        let cache = thumb_bucket.map(|b| (self.media.rendered.clone(), Self::thumb_key(&p, settings_hash, b, salt)));
        let cache_larger = thumb_bucket
            .map_or_else(Vec::new, |b| THUMB_SIZES.iter().filter(|&&s| s > b).map(|&s| Self::thumb_key(&p, settings_hash, s, salt)).collect());
        Some(RenderJob {
            request_id: 0,
            cache_generation: self.media.rendered.generation(),
            source_key: Some(source_key),
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: source_info(&p),
            settings,
            request,
            key,
            cache,
            cache_larger,
            stages: None,
            view_cache: None,
            display: None,
            hdr_view: false,
            slide: None,
            preview_cut: None,
            display_fit: None,
        })
    }

    /// A render of `id` with `settings` in place of its own, fitting `max_w × max_h` (the loupe's
    /// temporary preview while hovering a preset or profile; nothing is committed or cached).
    pub fn preview_job(&mut self, id: PhotoId, max_w: usize, max_h: usize, apply_crop: bool, settings: &DevelopSettings) -> Option<RenderJob> {
        let mut job = self.render_job(id, max_w, max_h, false, apply_crop)?;
        job.settings = Arc::new(settings.clone());
        job.key = settings.hash64() ^ ((max_w as u64) << 40) ^ ((max_h as u64) << 20) ^ (apply_crop as u64) ^ (job.level as u64) << 60;
        Some(job)
    }

    /// Cache key of a variant thumbnail ([`Self::variant_job`]).
    pub fn variant_key(p: &Photo, settings: &DevelopSettings, edge: usize) -> Hash128 {
        Hasher128::new()
            .str(&content_key(p))
            .str("variant")
            .u64(settings.hash64())
            .u64(edge as u64)
            .u64(RENDER_CACHE_VERSION)
            .u64(crate::camera_profiles::cache_key())
            .finish()
    }

    /// A thumbnail of `id` rendered with `settings` instead of its own (profile and preset
    /// browsers): from the thumbnail-level source, long edge `edge` (≤ 512), cropped. The result
    /// is cached (memory + the library's disk cache) under the photo's content and the settings
    /// hash, so a variant renders once; `key` is derived from the same hash, so a frontend can
    /// keep one texture per variant.
    pub fn variant_job(&mut self, id: PhotoId, settings: &DevelopSettings, edge: usize) -> Option<RenderJob> {
        self.variant_job_at(id, settings, edge, SourceLevel::Thumb)
    }

    /// [`Self::variant_job`] from the source `level`, long edge `edge` (≤ that level's largest).
    pub fn variant_job_at(&mut self, id: PhotoId, settings: &DevelopSettings, edge: usize, level: SourceLevel) -> Option<RenderJob> {
        let p = self.catalog.photo(id)?.clone();
        let edge = edge.clamp(16, level.max_edge());
        let source = self.media.source_ref(&p, level);
        let ck = Self::variant_key(&p, settings, edge);
        Some(RenderJob {
            request_id: 0,
            cache_generation: self.media.rendered.generation(),
            source_key: Some(Hasher128::new().str(&content_key(&p)).finish()),
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: source_info(&p),
            settings: Arc::new(settings.clone()),
            request: RenderRequest { apply_crop: true, ..RenderRequest::fit(edge, edge) },
            key: (ck.0 as u64) ^ ((ck.0 >> 64) as u64),
            cache: Some((self.media.rendered.clone(), ck)),
            cache_larger: Vec::new(),
            stages: None,
            view_cache: None,
            display: None,
            hdr_view: false,
            slide: None,
            preview_cut: None,
            display_fit: None,
        })
    }

    /// A square close-up of `face` (normalized, in the photo's upright frame) for a People card: the
    /// photo's own look with its crop replaced by a square around the face (room for hair and chin),
    /// at least `edge` pixels across (rounded up to [`FACE_EDGE_STEP`], ≤ 512, so a tile size
    /// between two steps reuses the cached one). An unedited raw's tile is cut from the camera's
    /// embedded preview when that holds the square at [`crate::face_tiles::SOFT_LIMIT`] of the
    /// size asked or more ([`crate::face_tiles`]); else, and for every other photo, the source
    /// level is the smallest that keeps the face sharp, up to the preview. Cached (memory and the
    /// library's thumbnail cache on disk) like [`Self::variant_job`], and `key` likewise follows
    /// the content: a later launch shows the tile without reading the original.
    pub fn face_job(&mut self, id: PhotoId, face: lightcraft_geom::Rect, edge: usize) -> Option<RenderJob> {
        // (a synced photo's tile is cut from its preview: shown, so download it like a grid cell's)
        self.note_thumb_ask(id, true);
        let p = self.catalog.photo(id)?.clone();
        // `face` is on the upright (EXIF-oriented) photo; the crop below is in the frame after the
        // user's Rotate Left/Right, so carry the box (and the photo's size) over to it.
        let orient = p.develop.orientation;
        let face = orient.map_norm_rect(face);
        let (w, h) = (f64::from(p.width.max(1)), f64::from(p.height.max(1)));
        let (w, h) = if orient.swaps_axes() { (h, w) } else { (w, h) };
        let side = ((face.x1 - face.x0) * w).max((face.y1 - face.y0) * h) * 1.7;
        // finite, within the photo, and never wider than its short side (so the clamps below are valid)
        let side = if side.is_finite() { side.clamp(1.0, w.min(h)) } else { w.min(h) };
        let (cx, cy) = ((face.x0 + face.x1) / 2.0 * w, (face.y0 + face.y1) / 2.0 * h);
        let (cx, cy) = (if cx.is_finite() { cx } else { w / 2.0 }, if cy.is_finite() { cy } else { h / 2.0 });
        let (x0, y0) = (cx.clamp(side / 2.0, w - side / 2.0) - side / 2.0, cy.clamp(side / 2.0, h - side / 2.0) - side / 2.0);
        let rect = lightcraft_geom::Rect { x0: x0 / w, y0: y0 / h, x1: (x0 + side) / w, y1: (y0 + side) / h };
        let mut settings = (*p.develop).clone();
        settings.crop = lightcraft_develop::Crop { geometry: lightcraft_geom::CropGeometry { rect, angle: 0.0 }, ..Default::default() };
        let edge = face_edge(edge);
        // pixels along the long edge that leave `edge` across the crop
        let needed = (edge as f64 * w.max(h) / side).ceil().min(SourceLevel::Preview.max_edge() as f64);
        let level = SourceLevel::for_size(needed as usize);
        let source = self.media.source_ref(&p, level);
        let ck = Self::variant_key(&p, &settings, edge);
        let preview_cut = self.face_preview_cut(&p, rect, edge, needed as usize);
        Some(RenderJob {
            request_id: 0,
            cache_generation: self.media.rendered.generation(),
            source_key: Some(Hasher128::new().str(&content_key(&p)).finish()),
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: source_info(&p),
            settings: Arc::new(settings),
            request: RenderRequest { apply_crop: true, ..RenderRequest::fit(edge, edge) },
            key: (ck.0 as u64) ^ ((ck.0 >> 64) as u64),
            cache: Some((self.media.rendered.clone(), ck)),
            cache_larger: Vec::new(),
            stages: None,
            view_cache: None,
            display: None,
            hdr_view: false,
            slide: None,
            preview_cut,
            display_fit: None,
        })
    }

    /// Cache key of a grid thumbnail of `p` with settings hashing to `settings_hash` in size bucket `b`
    /// ([`THUMB_SIZES`]), made with the denoised picture `salt` names ([`MediaCache::denoise_salt`]; 0: none).
    fn thumb_key(p: &Photo, settings_hash: u64, b: usize, salt: u64) -> Hash128 {
        let mut k = Hasher128::new();
        k.str(&content_key(p)).u64(settings_hash).u64(b as u64).u64(RENDER_CACHE_VERSION).u64(crate::camera_profiles::cache_key());
        // (only a render that uses a denoised picture has it in its key, so every other thumbnail stays valid)
        if salt != 0 {
            k.u64(salt);
        }
        k.finish()
    }

    /// Drop `p`'s cached thumbnails and view renders for its current settings (memory and disk):
    /// they no longer show what its file renders to.
    pub(crate) fn forget_renders(&self, p: &Photo) {
        let cache = &self.media.rendered;
        let h = p.develop.hash64();
        let salt = self.media.denoise_salt(p.id, &p.develop);
        for b in THUMB_SIZES {
            cache.remove(Self::thumb_key(p, h, b, salt));
        }
        for crop in [true, false] {
            cache.remove(Self::view_key(p, h, crop));
        }
    }

    /// How a face tile of `p` can come from its embedded preview ([`crate::face_tiles`]): only an
    /// unedited raw (whose grid thumbnail is that preview too) not known to be offline, upright as
    /// imported, with a preview loader installed. `rect` is the square around the face, `edge` the
    /// tile, `needed` the source's long edge the develop render would ask for.
    pub(super) fn face_preview_cut(
        &self,
        p: &Photo,
        rect: lightcraft_geom::Rect,
        edge: usize,
        needed: usize,
    ) -> Option<crate::face_tiles::PreviewCut> {
        let (path, loader) = self.embedded_of(p)?;
        // (a photo with an import crop shows a part of its picture: the face rect is in the whole one)
        if p.develop.orientation != lightcraft_geom::Orientation::Normal
            || p.is_cropped()
            || self.media.availability.is_offline(&path)
            || p.width == 0
            || p.height == 0
        {
            return None;
        }
        let want = needed.max(edge).max(1);
        let min = (want as f64 * crate::face_tiles::SOFT_LIMIT).ceil() as usize;
        let aspect = f64::from(p.width) / f64::from(p.height);
        Some(crate::face_tiles::PreviewCut { path, loader, want, min, rect, aspect, edge })
    }

    /// Size-independent cache key of a photo's view render (loupe) for its current settings.
    fn view_key(p: &Photo, settings_hash: u64, apply_crop: bool) -> Hash128 {
        Hasher128::new()
            .str(&content_key(p))
            .str("view")
            .u64(settings_hash)
            .u64(apply_crop as u64)
            .u64(RENDER_CACHE_VERSION)
            .u64(crate::camera_profiles::cache_key())
            .finish()
    }

    /// The loupe's render job: like [`Self::render_job`], and a full-quality result is kept as the
    /// photo's view preview (memory, and disk with a library) for [`Self::quick_view_job`].
    pub fn loupe_job(&mut self, id: PhotoId, max_w: usize, max_h: usize, apply_crop: bool) -> Option<RenderJob> {
        let mut job = self.render_job(id, max_w, max_h, false, apply_crop)?;
        let p = self.catalog.photo(id)?.clone();
        let h = self.media.settings_hashes.get(id, &p.develop);
        job.view_cache = Some((self.media.rendered.clone(), Self::view_key(&p, h, apply_crop)));
        Some(job)
    }

    /// A render of one window of the loupe's frame, for a view zoomed past what one whole-frame
    /// render can hold: the frame is `full_w × full_h` (the size the loupe would draw it at) and
    /// the result is `window`'s pixels of it, at that scale, from the source level that size needs.
    /// Nothing is cached here (a window is only worth keeping while it is on screen). `None`: no
    /// such photo, or the window reads (spots, Auto Mask strokes) more than one render can hold.
    pub fn region_job(
        &mut self,
        id: PhotoId,
        full_w: usize,
        full_h: usize,
        window: lightcraft_pipeline::PixelWindow,
        apply_crop: bool,
    ) -> Option<RenderJob> {
        self.region_job_of(id, full_w, full_h, window, apply_crop, false)
    }

    /// [`Self::region_job`] of the photo without its edits (its crop kept): the Before side.
    pub fn region_job_before(
        &mut self,
        id: PhotoId,
        full_w: usize,
        full_h: usize,
        window: lightcraft_pipeline::PixelWindow,
        apply_crop: bool,
    ) -> Option<RenderJob> {
        self.region_job_of(id, full_w, full_h, window, apply_crop, true)
    }

    fn region_job_of(
        &mut self,
        id: PhotoId,
        full_w: usize,
        full_h: usize,
        window: lightcraft_pipeline::PixelWindow,
        apply_crop: bool,
        before: bool,
    ) -> Option<RenderJob> {
        let mut job = self.render_job(id, full_w, full_h, before, apply_crop)?;
        // what the window's spots and Auto Mask strokes read must fit in one render: else the
        // caller keeps the whole-frame render (a window alone would come out wrong)
        let p = self.catalog.photo(id)?;
        let frame = lightcraft_pipeline::geometry::Frame::with_lens(
            p.width.max(1) as usize,
            p.height.max(1) as usize,
            &job.settings,
            apply_crop,
            p.embedded_lens.as_ref(),
        );
        let ppl = frame.px_per_long(full_w);
        lightcraft_pipeline::spots::window_for_reads_checked(&job.settings, &frame, full_w, full_h, ppl, window.clamped(full_w, full_h))?;
        job.request.window = Some(window);
        job.key = Hasher128::new()
            .u64(job.key)
            .str("window")
            .u64(window.x as u64)
            .u64(window.y as u64)
            .u64(window.w as u64)
            .u64(window.h as u64)
            .finish()
            .0 as u64;
        Some(job)
    }

    /// The embedded preview of an unedited raw (path, loader), when the app installed a loader. The loader's pictures are
    /// cut to the photo's own frame as shown ([`frame_to`]): a camera's JPEG can show more than the raw's default crop.
    fn embedded_of(&self, p: &Photo) -> Option<(String, PreviewLoader)> {
        // a synced photo's `web/<hash>/…` is no file here: its picture is the server's mini preview, which the grid
        // must keep asking for (an embedded preview taken as final, and failing, left its cell empty until an edit)
        if crate::cloud_sync::is_remote(p) {
            return None;
        }
        match (&p.source, &self.media.preview_loader) {
            (Source::File { path }, Some(l)) if p.kind == MediaKind::Raw && (crate::import::has_import_look(p) || at_camera_frame(p)) => {
                let (width, height) = p.shown_size();
                Some((path.clone(), framed_loader(l, width, height)))
            }
            _ => None,
        }
    }

    /// Whether `p`'s thumbnail can be the camera's own JPEG, so a frontend can skip the sensor
    /// decode behind it.
    ///
    /// The grid shows this JPEG first in any case ([`QuickJob`]: "grid thumbnails of unedited raws
    /// likewise start from the embedded preview"), and then paid for a render of the raw to replace
    /// it. That render decodes the whole sensor — 50 of a 56 MB Fuji RAF's bytes — for a 512 px
    /// thumbnail the 4 416 × 2 944 embedded JPEG already covers, and on a share at 1.2 MB/s it is
    /// 45 s a photo: a 149-photo folder spent 8.3 GB of traffic on thumbnails, and the queue behind
    /// it delayed the render the user was waiting for.
    ///
    /// Only unedited raws answer `true`: any edit must be rendered, and `has_import_look` is what
    /// says the photo still looks the way it did at import. A photo the user edits re-renders
    /// through the ordinary path, because its thumbnail key (settings hash) changes.
    pub fn embedded_thumb_is_final(&self, p: &Photo) -> bool {
        self.embedded_of(p).is_some()
    }

    /// The camera's embedded preview of a raw file, for the face scan. Faces are read from the unedited picture, so the
    /// photo's edits do not matter, and the camera's own JPEG (oriented, display-ready) costs a fraction of decoding the
    /// raw. `None` for anything else, for a synced photo whose original is on the server only (the scan renders its
    /// downloaded preview), and where the host installed no loader.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn scan_preview_of(&self, p: &Photo) -> Option<(String, PreviewLoader)> {
        match (&p.source, &self.media.preview_loader) {
            (Source::File { path }, Some(l)) if p.kind == MediaKind::Raw && !crate::cloud_sync::is_remote(p) => Some((path.clone(), l.clone())),
            _ => None,
        }
    }

    /// Something to show in the loupe right away for `id` (see [`QuickJob`]): its cached view
    /// render, else the embedded preview of an unedited raw, else a cached or fresh thumbnail.
    pub fn quick_view_job(&mut self, id: PhotoId, max_edge: usize, apply_crop: bool) -> Option<QuickJob> {
        let p = self.catalog.photo(id)?.clone();
        let small = self.thumb_job(id, THUMB_SIZES[THUMB_SIZES.len() - 1])?;
        // (the thumbnail job itself starts with the cached thumbnail, after the sharper embedded preview)
        let settings_hash = self.media.settings_hashes.get(id, &p.develop);
        let cached = vec![(self.media.rendered.clone(), Self::view_key(&p, settings_hash, apply_crop))];
        let h = Hasher128::new().str(&content_key(&p)).str("quick").u64(settings_hash).u64(apply_crop as u64).finish();
        let key = h.0 as u64;
        let embedded = self.embedded_of(&p).map(|(path, l)| (path, l, max_edge.clamp(1, SourceLevel::Preview.max_edge())));
        Some(QuickJob { request_id: 0, photo: id, key, cached, embedded, small: apply_crop.then(|| Box::new(small)) })
    }

    /// A first grid thumbnail for an unedited raw: the cached render for `job` (the real
    /// thumbnail job, from [`Self::thumb_job`]) if there is one — then the result carries
    /// [`QuickSource::Cached`] and `job.key` and is final — else its embedded preview, else `job`
    /// itself: the grid queues no render behind an unedited raw's quick thumbnail
    /// ([`Self::embedded_thumb_is_final`]), so a preview that can't be read must not leave the
    /// cell empty (issue #1448). `None` for other photos (render them directly).
    pub fn quick_thumb_job(&mut self, job: &RenderJob) -> Option<QuickJob> {
        let p = self.catalog.photo(job.photo)?.clone();
        let (path, l) = self.embedded_of(&p)?;
        let edge = job.request.max_w.max(job.request.max_h);
        Some(QuickJob {
            request_id: 0,
            photo: job.photo,
            key: job.key,
            cached: job.cache.clone().into_iter().collect(),
            embedded: Some((path, l, edge)),
            small: Some(Box::new(job.clone())),
        })
    }

    /// Accept a finished job's loaded source into the cache. What the decode learnt about the file that the catalog
    /// has wrong or lacks is recorded too ([`Self::learn_as_shot_wb`], [`Self::learn_lens`]).
    pub fn accept(&mut self, r: &RenderResult) {
        if let Some(src) = &r.loaded
            && r.source_key.is_none_or(|key| self.catalog.photo(r.photo).is_some_and(|p| Hasher128::new().str(&content_key(p)).finish() == key))
        {
            if let Some(info) = &src.info {
                self.learn_as_shot_wb(r.photo, info);
                self.learn_lens(r.photo, info);
            }
            self.media.insert_source(r.photo, r.level, src.clone());
            // a raw catalogued as preview-only (by a build that couldn't decode it) just developed from its sensor data
            if src.info.as_ref().is_some_and(|i| i.raw) && self.catalog.photo(r.photo).is_some_and(|p| p.preview_only.is_some()) {
                crate::cmd::convert::heal_preview_only(self, r.photo);
            }
            // and one catalogued as developable whose file only gave its embedded JPEG (a decoded source: never a smart preview)
            if src.info.as_ref().is_some_and(|i| !i.raw) && self.catalog.photo(r.photo).is_some_and(|p| p.develops_raw()) {
                crate::cmd::convert::flag_preview_only(self, r.photo);
            }
        }
    }

    /// Record the as-shot white balance a decode of photo `id`'s file read (`info`) when the catalog has another:
    /// a raw imported before its camera's matrices gave it a Kelvin scale keeps 6500 / 0 (the relative scale's
    /// reference) until its file is next decoded (issue #730). Bookkeeping, not an undo step; the look does not
    /// change (the pipeline reads the stored settings against the real as-shot white either way), only what As Shot
    /// says and what a smart preview renders with. Only for raws edited in Kelvin.
    fn learn_as_shot_wb(&mut self, id: PhotoId, info: &SourceInfo) {
        let Some(p) = self.catalog.photo(id) else { return };
        if !info.raw || info.relative_wb || !p.develops_raw() || p.relative_wb() {
            return;
        }
        let read = (info.as_shot_temp, info.as_shot_tint);
        if !(read.0.is_finite() && read.1.is_finite()) || p.as_shot_wb == Some(read) {
            return;
        }
        if let Err(e) = self.apply_system(Op::SetAsShotWb { id, wb: Some(read) }) {
            log::warn!("photo {}: as-shot white balance not recorded: {e}", id.0);
        }
    }

    /// Record the lens data a decode of raw `id` found (`info`) when the catalog has none: the vignetting a CR3's camera
    /// corrected in its JPEG is measured from the pixels ([`crate::camera_preview::jpeg_vignetting`]), so no import
    /// probe sees it. Then smart previews and synced devices render with it, and a photo still as import left it gets
    /// the correction turned on, as import does for a file's own lens data ([`crate::cmd::convert::lens_ops`]).
    /// Bookkeeping, not an undo step. Lens data the catalog has is replaced only by a lens database profile that
    /// supersedes it ([`lightcraft_develop::EmbeddedLens::superseded_by`]: a measured vignette, an older database's
    /// profile), never by another measurement (devices may measure a hair apart).
    fn learn_lens(&mut self, id: PhotoId, info: &SourceInfo) {
        let Some(p) = self.catalog.photo(id) else { return };
        let Some(lens) = info.lens.as_ref() else { return };
        if !info.raw || !p.develops_raw() || p.embedded_lens.as_ref().is_some_and(|have| !have.superseded_by(lens)) {
            return;
        }
        for op in crate::cmd::convert::lens_ops(id, p, info.lens) {
            if let Err(e) = self.apply_system(op) {
                log::warn!("photo {}: lens data not recorded: {e}", id.0);
                return;
            }
        }
    }

    /// Synchronous render (CLI, MCP, tests).
    pub fn render_now(&mut self, id: PhotoId, max_w: usize, max_h: usize) -> Result<Rendered, String> {
        let job = self.render_job(id, max_w, max_h, false, true).ok_or("no such photo")?;
        let r = job.run();
        self.accept(&r);
        r.rendered
    }

    /// Synchronous render for export: like [`Self::render_now`], into colour space `space` with
    /// sample format `depth`.
    pub fn render_export(
        &mut self,
        id: PhotoId,
        max_w: usize,
        max_h: usize,
        space: lightcraft_pipeline::OutputSpace,
        depth: lightcraft_pipeline::OutputDepth,
    ) -> Result<Rendered, String> {
        let r = self.export_job(id, max_w, max_h, space, depth)?.run();
        self.accept(&r);
        r.rendered
    }

    /// The job [`Self::render_export`] runs, to run elsewhere (e.g. a background export thread).
    pub fn export_job(
        &mut self,
        id: PhotoId,
        max_w: usize,
        max_h: usize,
        space: lightcraft_pipeline::OutputSpace,
        depth: lightcraft_pipeline::OutputDepth,
    ) -> Result<RenderJob, String> {
        let mut job = self.render_job(id, max_w, max_h, false, true).ok_or("no such photo")?;
        // An export renders the full image: never a thumbnail's source developed from a smaller copy of the raw
        // image that the file keeps (Lightroom's merged DNGs)
        let reduced = match &job.source {
            SourceRef::Loaded(s) => s.reduced,
            SourceRef::Fit { from, .. } => from.reduced,
            _ => false,
        };
        if reduced {
            let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
            job.source = self.media.source_ref_uncached(&p, job.level);
        }
        // An export is made from the denoised picture its Denoise amount asks for, made when the job runs if the
        // background queue has not got to it. (A source kept in memory may lack the picture, so the job reads the file.)
        if let Some(wanted) = self.denoise_for_export(id, &job.settings)? {
            let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
            job.source = self.media.source_ref_uncached(&p, job.level);
            if let SourceRef::File { denoise, .. } = &mut job.source {
                *denoise = Some(wanted);
            }
        }
        // A DNG exported at half its size or less is read at about the size the export needs (a linear raw — Apple ProRAW —
        // is binned while it is decoded, so a 4096 px export of a 48 MP photo never holds 48 MP of floats).
        if let (Some(p), SourceRef::File { max_edge, .. }) = (self.catalog.photo(id), &mut job.source) {
            let long = max_w.max(max_h);
            let edge = if p.format.eq_ignore_ascii_case("DNG") {
                export_source_edge(p, long)
            } else {
                // a Bayer raw too big to demosaic whole on this device is binned like a preview (`export_binned_edge`)
                export_binned_edge(self, p, long, max_w.saturating_mul(max_h))
            };
            // (only a read of the whole sensor is narrowed: a preview's keeps the edge its level gave it)
            if let Some(edge) = edge.filter(|_| p.format.eq_ignore_ascii_case("DNG") || *max_edge == usize::MAX) {
                *max_edge = edge;
            }
        }
        job.request.space = space;
        job.request.depth = depth;
        job.key ^= (space as u64 + 1).wrapping_mul(0xa076_1d64_78bd_642f) ^ (depth as u64 + 1).wrapping_mul(0xe703_7ed1_a0b4_28db);
        Ok(job)
    }

    /// Prefer decoder facts to header-only metadata for pixel-statistics commands.
    pub fn source_info(&self, id: PhotoId) -> SourceInfo {
        let header = self.catalog.photo(id).map(|p| source_info(p)).unwrap_or_default();
        self.media.source_facts(id, header)
    }

    /// The lens correction photo `id`'s file embeds: the catalog's, else what its decoder found (a raw
    /// imported as preview-only and decodable since has none in the catalog).
    pub fn embedded_lens(&self, id: PhotoId) -> Option<lightcraft_develop::EmbeddedLens> {
        self.catalog.photo(id).and_then(|p| p.embedded_lens).or_else(|| self.source_info(id).lens)
    }

    /// Makes sure a decode of photo `id` is cached, so [`Self::embedded_lens`] sees the lens data the decoder finds (the vignetting
    /// a CR3's camera corrected in its JPEG is measured from the pixels, so a raw not yet decoded has none). A source the media
    /// cache holds already has it; otherwise the photo is decoded at thumbnail size. Changes no edit.
    pub fn ensure_decoded(&mut self, id: PhotoId) -> Result<(), String> {
        let known = self.media.cached_source(id).is_some_and(|s| s.info.is_some() || s.from_proxy);
        if !known {
            self.source_now(id, SourceLevel::Thumb)?;
        }
        Ok(())
    }

    /// The base tone curve photo `id`'s file gives it ([`lightcraft_pipeline::SourceInfo::camera_tone`]): the curve a raw is
    /// shown through before any edit, fitted to the camera's own picture or taken from a DNG's `ProfileToneCurve`. A source
    /// the media cache holds (decoded, or a smart preview's) already knows it; otherwise the photo is decoded at thumbnail
    /// size, which fits the same look as any larger level does. `None` for a photo with no such curve (a rendered file, a
    /// raw nothing was fitted to). A decode records what every render does (the as-shot white balance): no edit.
    pub fn camera_tone_of(&mut self, id: PhotoId) -> Result<Option<lightcraft_pipeline::tone::CameraTone>, String> {
        let known = self.media.cached_source(id).is_some_and(|s| s.info.is_some() || s.from_proxy);
        if !known {
            self.source_now(id, SourceLevel::Thumb)?;
        }
        Ok(self.source_info(id).camera_tone)
    }

    /// Borrow source mattes for selection notices without constructing render facts each frame.
    pub fn source_mattes(&self, id: PhotoId) -> Option<&lightcraft_pipeline::masks::Mattes> {
        self.media.cached_source(id).and_then(|s| s.info.as_ref()).and_then(|info| info.mattes.as_deref())
    }

    /// The source proxy for pixel-statistics commands (auto tone/WB), loading synchronously.
    pub fn source_now(&mut self, id: PhotoId, level: SourceLevel) -> Result<Arc<Rgb32f>, String> {
        let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
        let r = self.media.source_ref(&p, level).load_source()?;
        let image = r.image.clone();
        if let Some(info) = &r.info {
            self.learn_as_shot_wb(id, info);
        }
        self.media.insert_source(id, level, r);
        Ok(image)
    }

    /// [`Self::source_now`] for what a render of photo `id` develops: the picture in the camera's look when the photo has
    /// the Camera Matching profile and with the Denoise it asks for, and the facts to render it against
    /// ([`DecodedSource::develop_input`]). Generative Expand paints from this, so the new area is in the light the
    /// photo is rendered in.
    pub fn develop_source_now(&mut self, id: PhotoId, level: SourceLevel) -> Result<(Arc<Rgb32f>, SourceInfo), String> {
        let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
        let r = self.media.source_ref(&p, level).load_source()?;
        if let Some(info) = &r.info {
            self.learn_as_shot_wb(id, info);
        }
        let input = r.develop_input(&p.develop, source_info(&p));
        self.media.insert_source(id, level, r);
        Ok(input)
    }
}

/// The long edge to read photo `p`'s source at for an export `out_long` pixels long, when that is less than half the
/// photo's own size (so reading it smaller saves much). It leaves every output pixel a source pixel of its own, with a
/// margin for the geometry stage's lens, perspective and straightening.
pub(crate) fn export_source_edge(p: &Photo, out_long: usize) -> Option<usize> {
    let (w, h) = (p.width.max(1) as usize, p.height.max(1) as usize);
    let (nw, nh) = lightcraft_pipeline::native_output_size(w, h, &p.develop);
    let native = nw.max(nh);
    if !native.is_finite() || native < 1.0 {
        return None;
    }
    let long = w.max(h) as f64;
    let wanted = long * (out_long as f64 / native).min(1.0);
    let needed = (wanted * 1.12).ceil();
    if needed * 2.0 <= long {
        return Some(needed as usize);
    }
    // Just over half (a 4096 px export of a 48 MP photo, whose half is 4032 px) still takes the half-size source, a
    // few per cent short of the output: the whole original would cost a phone ~3 GB more than it has.
    let half = (long / 2.0).floor();
    (wanted <= half * 1.05).then_some(half as usize)
}

/// How many pixels the source of an export `out_long` pixels long has: the photo's own, or fewer when a DNG is read
/// at the size the export needs ([`export_source_edge`]).
pub(crate) fn export_source_pixels(p: &Photo, out_long: usize) -> usize {
    let px = (p.width.max(1) as usize).saturating_mul(p.height.max(1) as usize);
    if !p.format.eq_ignore_ascii_case("DNG") {
        return px;
    }
    match export_source_edge(p, out_long) {
        Some(edge) => {
            let long = p.width.max(p.height).max(1) as f64;
            ((px as f64) * (edge as f64 / long).powi(2)).ceil() as usize
        }
        None => px,
    }
}

/// Raw formats that are Bayer mosaics, which [`lightcraft_raw::RawImage::develop_binned_masked`] bins by even blocks (a DNG
/// has a rule of its own, above; Fujifilm's RAF can be X-Trans, which only bins by 3).
fn bayer_mosaic(p: &Photo) -> bool {
    ["ARW", "NEF", "NRW", "RW2", "RWL", "CR3", "CR2", "PEF", "SRW", "ORF", "ORI", "ERF", "MRW"].iter().any(|f| p.format.eq_ignore_ascii_case(f))
}

/// The long edge the source of an export `out_long` pixels long (of `out_px` pixels) is read at, for a Bayer raw. Up to a
/// preview's size that is the preview's (the loader has always binned the mosaic for it, whatever the device). A bigger
/// export reads the whole sensor, except on a device whose memory limit ([`crate::Session::export_limit`]) the whole sensor
/// would not fit, when an export of at most half the photo's size is read binned too (a 42 MP raw's 2.5 GB becomes 1 GB).
/// `None`: not a Bayer raw, or the whole sensor. `export_job` narrows its read to this and the memory check
/// ([`export_binned_need`]) is made of it, so what is promised is what is done.
pub(crate) fn export_binned_edge(session: &crate::Session, p: &Photo, out_long: usize, out_px: usize) -> Option<usize> {
    if !bayer_mosaic(p) {
        return None;
    }
    let level = SourceLevel::for_size(out_long);
    if level != SourceLevel::Full {
        return Some(level.max_edge());
    }
    let limit = session.export_limit()?;
    let edge = export_source_edge(p, out_long)?;
    (crate::memory::export_need(export_source_pixels(p, out_long), out_px) > limit).then_some(edge)
}

/// What an export read at [`export_binned_edge`] needs: the mosaic binned by the largest even block that keeps 90 % of
/// `edge` (the loader's rule, `bin_factor`; a sensor too small for any block is demosaiced whole), plus the samples and file
/// the decode holds before it bins (about 4 bytes a pixel of the sensor).
pub(crate) fn export_binned_need(p: &Photo, edge: usize, out_px: usize) -> usize {
    let (w, h) = (p.width.max(1) as usize, p.height.max(1) as usize);
    let keep = edge.saturating_mul(9) / 10;
    let Some(k) = [8usize, 6, 4, 2].into_iter().find(|k| w.max(h) / k >= keep) else {
        return crate::memory::export_need(w.saturating_mul(h), out_px);
    };
    crate::memory::export_need((w / k).saturating_mul(h / k), out_px).saturating_add(w.saturating_mul(h).saturating_mul(4))
}

/// `loader` with the pictures it gives cut to the shape of a `width × height` frame ([`frame_to`]).
fn framed_loader(loader: &PreviewLoader, width: f64, height: f64) -> PreviewLoader {
    if !(width > 0.0 && height > 0.0) {
        return loader.clone();
    }
    let (inner, aspect) = (loader.clone(), width / height);
    Arc::new(move |path: &str, edge: usize| inner(path, edge).map(|img| frame_to(img, aspect)))
}

/// A Panasonic raw imported whole whose only departure from the camera's defaults is the camera's own aspect-ratio frame,
/// its import crop ([`CameraCrop`], [`crate::import::commit_prepared`]): still as imported, so its camera JPEG (the whole
/// area) cut to that frame ([`frame_to`]) is a fair stand-in, as for a raw with no crop. Not another maker's: their
/// frames are part of how their decoders and JPEGs work today.
fn at_camera_frame(p: &Photo) -> bool {
    let Some(look) = &p.import_look else { return false };
    let mut defaults = p.camera_defaults();
    defaults.crop.geometry.rect = look.crop.geometry.rect;
    p.format.eq_ignore_ascii_case("RW2") && p.develop_is(defaults)
}

/// A camera's embedded JPEG cut to the frame of the photo it belongs to (`aspect`, width over height). Some cameras show
/// the whole sensor in their JPEG while the raw's default crop is the in-camera aspect ratio (Panasonic: a 16:9 shot is
/// the middle of its 4:3 sensor); a thumbnail made from the JPEG then showed another frame than the render that replaced
/// it, and a photo changed shape when its page settled. The cut is around the centre, where the in-camera crops are.
/// Left alone: a picture of the frame's shape within what resizing rounds, one turned the other way, and a frame that
/// would keep less than half of it (that is no aspect-ratio setting).
pub(super) fn frame_to(img: Rgba8, aspect: f64) -> Rgba8 {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || !(aspect.is_finite() && aspect > 0.0) {
        return img;
    }
    let have = w as f64 / h as f64;
    if (have / aspect - 1.0).abs() < 0.02 || (have - 1.0) * (aspect - 1.0) < 0.0 {
        return img;
    }
    let (cw, ch) =
        if aspect < have { (((h as f64 * aspect).round() as usize).clamp(1, w), h) } else { (w, ((w as f64 / aspect).round() as usize).clamp(1, h)) };
    if cw.saturating_mul(ch).saturating_mul(2) < w.saturating_mul(h) {
        return img;
    }
    img.into_crop((w - cw) / 2, (h - ch) / 2, cw, ch)
}
