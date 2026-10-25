# HDR display (LR-VIEW-HDR-DISPLAY)

> **Last updated:** 2026-10-25 · **Change:** HDR sources: an iPhone HEIC's and an Ultra HDR JPEG's gain map give the HDR render its highlights (FB-HDR-6); before: the HDR picture renders on the GPU (half floats and the HDR histogram from the GPU pipeline), and the draft budget learns HDR drafts apart: HDR drags at or near full size

When **Edit ▸ Light ▸ HDR** is on and the screen can show more than SDR white, the loupe draws the photo
in HDR: highlights go past white, up to the edit's headroom limit (`hdr.maxEv`). Every other view, and a
screen without headroom, shows the SDR rendition (the `hdr.sdr*` sliders), as before, so an HDR edit is
never shown wrong.

## One render, three layers

1. **The render** (`lightcraft-engine`, `hdr_view`, `RenderJob::with_hdr_display`). The same float HDR
   render an HDR export holds (`OutputDepth::F32Hdr`: SDR white = 1.0, highlights up to `2^maxEv`), in
   **Display P3** primaries (what the screens with headroom show), packed into an `HdrPicture`: RGBA
   half floats encoded with the sRGB curve carried on past 1 (*extended Display P3*; no frontend
   converts: egui writes it as it is, CoreGraphics has the colour space). It renders on the GPU wherever an
   SDR loupe render would (`media::develop_hdr` → `lightcraft_gpu::try_render_hdr`: the per-pixel kernel
   writes the half floats beside the 8-bit picture and a second kernel counts the HDR histogram, within
   half-float rounding of the CPU's float render, [gpu-pipeline](gpu-pipeline.md#correctness-cpu-oracle-and-equivalence-tests)),
   else on the CPU (a refined mask, a LUT profile over 65 points a side, no GPU), with the loupe's stage caches either way. The
   8-bit `image` of the result is its reduction, and the histogram shows its HDR range.
   Only the loupe (`Slot::Main`) and its zoom window (`Slot::Region`) ask for it: the Before views show
   the unedited photo, and everything else is small SDR previews. A diagnostic overlay or a soft proof is
   an SDR picture of its own.
2. **The scheduler** (`lightcraft-apphost`, `render`). `Scheduler::set_hdr_display(true)` says the display
   can show it; `job_for` then turns an HDR edit's loupe job into an HDR one (its own result key, so
   switching back never shows a stale picture), and `Present::set_hdr` hands the `HdrPicture` to the
   frontend, or `None` when an SDR render takes its place. Frontends that don't override it never see HDR.
   The `DraftBudget` that sizes a drag's drafts learns HDR drafts apart from SDR ones (the loupe's latest
   picture says which kind a drag's drafts are), so an HDR drag on the CPU never starts at the size the
   GPU's SDR drafts can afford, nor a GPU one at a CPU one's.
3. **The frontend** puts the picture on the screen:

| Frontend | How | Where it says the display can |
|----------|-----|-------------------------------|
| **egui, macOS** | a `Rgba16Float` texture (`apps/lightcraft/src/hdr_sink.rs`) drawn instead of the 8-bit one (`TexView::paint_id`); the window's surface goes to extended Display P3 (EDR) while it is on screen | `NSScreen.maximumPotentialExtendedDynamicRangeColorComponentValue > 1` of the window's screen, asked twice a second (`hdr_display.rs`) |
| **A native frontend** | an `hdr` lease beside the `image` event (RGBA16F pixels, the edit's `peak` as its content headroom), drawn through the platform's own HDR path ([engine-host.md](engine-host.md)) | the frontend asks the platform and tells the host with `render.hdrDisplay {on}` at start and when it becomes active |

Windows (scRGB / HDR10 swap chains), Wayland (the colour-management protocol) and the web are not driven
yet and report SDR (`display: SDR (…)` in the log).

## HDR sources

A photo that carries its own HDR rendition opens with it. Import (`files::load_bytes_now`) reads the file's gain map
(`lightcraft_codecs::gainmap::read`) and keeps it, turned like the picture, in `SourceInfo::hdr_gain`:

- **Ultra HDR / ISO 21496-1 JPEG** (Pixel, Samsung, Lightroom, LightCraft's own HDR export): the second MPF image with its
  ISO 21496-1 or `hdrgm` XMP metadata (per-channel or single-channel maps, min / max content boost, gamma, offsets, base and
  alternate headroom).
- **iPhone HEIC**: the auxiliary image `urn:com:apple:photo:2020:aux:hdrgainmap` (`lightcraft_heif::gain_map`), sized by Apple's
  maker note (EXIF tags 33 and 48 turned into stops with the published recipe: the map's codes span 0 to that many stops, no
  offsets). A HEIC with an ISO `tmap` item brings its ISO 21496-1 metadata instead. A gain map that cannot be sized or parsed
  leaves the file SDR; nothing panics.

The HDR render (`RenderJob` with an HDR view or an `F32Hdr` export, and the HDR histogram of an HDR edit's preview) develops
`DecodedSource::hdr_input` instead of the base: the base picture with the gain map applied in full (`gainmap::apply`, bilinear
upsample of a lower-resolution map, kept per source like the camera-matched picture), so every edit, crop, rotation and lens
correction treats the highlights as it treats the rest, and values above SDR white reach the `hdr.maxEv` limit. A neutral tone
map passes those values through as they are (it used to roll them off from 0.95 up). The SDR render never sees it: the SDR
look is the base picture's, bit for bit (`tests_hdr_sources`: SDR exports of a HEIC with and without a gain map are equal).

Limits: the gain applies in the working primaries (Rec. 2020), not the base's own, which only matters for a map with
per-channel gain and offsets; a smart preview does not keep the map (an offline original renders SDR); the HDR range overlay
of an SDR preview and a HEIC whose primary item is the `tmap` itself are not covered; Apple's maker-note recipe and
real iPhone, Pixel and Lightroom files were not available to check against, only synthetic ones and LightCraft's own HDR
JPEG export read back (peak and mean luminance above white within a few percent).

## The desktop window's surface

eframe 0.36 picks its wgpu surface format itself, 8-bit only, so no pixel could be brighter than white.
`third_party/egui-wgpu` is the crates.io release with three marked changes (see its `LIGHTCRAFT.md`), and
`third_party/wgpu-hal` is wgpu 30.0.1's Metal layer with one fix: it resolved CoreGraphics' extended Display P3
constant by `dlsym` without reading the pointer, so asking for that colour space crashed (a bus error inside
`CGColorSpaceCreateWithName`). Both are `[patch.crates-io]` path dependencies, not workspace members:

- `SurfaceConfig::hdr_surface` makes the surface `Rgba16Float`. `apps/lightcraft` asks for it when some
  screen has headroom, before the window opens. A float surface presents as plain sRGB, so the window
  looks as it did.
- `set_hdr_active(true)` (a process-wide flag: eframe gives the app a copy of `RenderState`, so a `SurfaceConfig`
  field changed through `Frame::set_wgpu_surface_config` never reaches the painter) presents it as **extended Display P3**: egui writes gamma-encoded values,
  which is what that colour space is, and values above 1 are brighter than white. The app sets it while
  the loupe draws an HDR picture (`LightcraftApp::hdr_on_screen`) and clears it after, so the screen's EDR
  mode (backlight headroom, more power) is only on while it is used. Display P3, not sRGB, because the
  window has always been drawn in the screen's own values (no colour space on the layer; previews go through
  the screen's profile), which is P3 on every screen with headroom: nothing else in the window shifts.
- No dithering on a float surface.

An HDR picture is a `Rgba16Float` texture holding that same encoding; egui's textures aren't sRGB-aware (its
shader writes what it samples), so a value above 1 reaches the surface as it is.

## Switches and diagnostics

- `LIGHTCRAFT_HDR_DISPLAY=0` (desktop) keeps the 8-bit surface and the SDR rendition everywhere.
  `LIGHTCRAFT_HDR_DISPLAY=1|0` forces the capability on or off in the engine host (a simulator reports no
  headroom).
- The desktop log says what was decided: `display: HDR (Rgba16Float, extended Display P3)` or
  `display: SDR (the window surface is 8-bit)`, and `display: HDR pictures on|off` when the window moves to
  a screen with or without headroom.
- `ui.inspect` → `hdr`: `canDraw` (the window has a surface that can), `display` (the screen it is on has headroom), `picture` (the loupe holds an HDR picture) and `onScreen` (the last frame drew it).

## Known limits

- Where the HDR picture renders on the CPU (a refined mask, a LUT profile over 65 points a side, no GPU) a slider drag redraws far
  slower than an SDR one: the CPU's `finish (deep)` takes ~180 ms for a 24 MP CR3's full-size loupe picture
  and ~12 ms for a 0.25 MP draft (M1 Max), so the drafts shrink (the draft budget learns it, apart from SDR's).
- On the GPU an HDR picture still costs more than an SDR one at the same size: its readback is three times
  the bytes (the half floats beside the 8-bit picture: 12 bytes a pixel against 4), copied into fresh memory at
  ~0.25 ms a MB. Measured 2026-10-16 with a 24 MP CR3 on an M1 Max (load 40 to 300). `drag_bench` (`BENCH_HDR=1`:
  the engine host driven as a native frontend drives it, a 1067 × 1600 picture): HDR drafts at full size, ~103 a
  second as SDR's (8 ms a draft against 4); before, on the CPU, they shrank to 213 × 320 to hold ~86 a second,
  and the sharp picture came 139–227 ms after the release (now 16–28 ms). Drawing the GPU's picture without the
  readback (a texture the window shares) would close the rest.
- Headroom beyond what the screen reaches right now is clipped by the system (macOS), not tone-mapped by us.
- Values outside Display P3 are clamped (the render's gamut map is P3).
- The surface stays in HDR output for a second after the last HDR picture (a hover preview, the Before view or
  the next photo are gaps of a moment: each flip is a backlight ramp), so it is not switched off the moment
  you step away from an HDR photo.
- The window's own screenshot cannot read back a float surface, so `ui.screenshot` renders headlessly on a
  window that can draw HDR: it shows the 8-bit pictures.
- Generative Expand stays SDR (its missing area is transparent in the 8-bit picture; an HDR one has no alpha).
