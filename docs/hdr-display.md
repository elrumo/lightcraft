# HDR display (LR-VIEW-HDR-DISPLAY)

> **Last updated:** 2026-10-16 · **Change:** the SwiftUI loupe draws the float picture as a half-float IOSurface (GPU colour matching)

When **Edit ▸ Light ▸ HDR** is on and the screen can show more than SDR white, the loupe draws the photo
in HDR: highlights go past white, up to the edit's headroom limit (`hdr.maxEv`). Every other view, and a
screen without headroom, shows the SDR rendition (the `hdr.sdr*` sliders), as before, so an HDR edit is
never shown wrong.

## One render, three layers

1. **The render** (`lightcraft-engine`, `hdr_view`, `RenderJob::with_hdr_display`). The same float HDR
   render an HDR export holds (`OutputDepth::F32Hdr`: SDR white = 1.0, highlights up to `2^maxEv`), in
   **Display P3** primaries (what the screens with headroom show), packed into an `HdrPicture`: RGBA
   half floats encoded with the sRGB curve carried on past 1 (*extended Display P3*; no frontend
   converts: egui writes it as it is, CoreGraphics has the colour space). It runs on the CPU (the GPU pipeline has no float output), with the loupe's stage
   caches; the 8-bit `image` of the result is its reduction, and the HDR histogram is measured on it.
   Only the loupe (`Slot::Main`) and its zoom window (`Slot::Region`) ask for it: the Before views show
   the unedited photo, and everything else is small SDR previews. A diagnostic overlay or a soft proof is
   an SDR picture of its own.
2. **The scheduler** (`lightcraft-apphost`, `render`). `Scheduler::set_hdr_display(true)` says the display
   can show it; `job_for` then turns an HDR edit's loupe job into an HDR one (its own result key, so
   switching back never shows a stale picture), and `Present::set_hdr` hands the `HdrPicture` to the
   frontend, or `None` when an SDR render takes its place. Frontends that don't override it never see HDR.
3. **The frontend** puts the picture on the screen:

| Frontend | How | Where it says the display can |
|----------|-----|-------------------------------|
| **egui, macOS** | a `Rgba16Float` texture (`apps/lightcraft/src/hdr_sink.rs`) drawn instead of the 8-bit one (`TexView::paint_id`); the window's surface goes to extended Display P3 (EDR) while it is on screen | `NSScreen.maximumPotentialExtendedDynamicRangeColorComponentValue > 1` of the window's screen, asked twice a second (`hdr_display.rs`) |
| **SwiftUI (iOS, Mac)** | an `hdr` lease beside the `image` event (RGBA16F, `lc_image` as for any picture) → copied on the event pump's thread into an IOSurface (`'RGhA'` half floats, extended Display P3, the edit's `peak` as its content headroom), drawn by a layer with `preferredDynamicRange = .high` (`LoupePicture`): Core Animation colour-matches it on the GPU, where a float `CGImage` was converted on the CPU at every picture of a drag | `UIScreen.potentialEDRHeadroom > 1` (`NSScreen.maximumPotentialExtendedDynamicRangeColorComponentValue` on a Mac), told with `render.hdrDisplay {on}` at start and when the app becomes active |

Windows (scRGB / HDR10 swap chains), Wayland (the colour-management protocol) and the web are not driven
yet and report SDR (`display: SDR (…)` in the log).

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
  `LIGHTCRAFT_HDR_DISPLAY=1|0` in the SwiftUI app forces the capability on or off (the simulator reports no
  headroom).
- The desktop log says what was decided: `display: HDR (Rgba16Float, extended Display P3)` or
  `display: SDR (the window surface is 8-bit)`, and `display: HDR pictures on|off` when the window moves to
  a screen with or without headroom.
- `ui.inspect` → `hdr`: `canDraw` (the window has a surface that can), `display` (the screen it is on has headroom), `picture` (the loupe holds an HDR picture) and `onScreen` (the last frame drew it).

## Known limits

- The HDR render is CPU only: a slider drag on an HDR edit redraws slower than an SDR one (the draft
  budget learns it). A GPU float output would remove it. Measured on a 24 MP CR3 in the SwiftUI Mac app (M1 Max,
  2026-10-16, a busy machine): the CPU's `finish (deep)` took ~180 ms for the full-size picture and ~12 ms for a
  0.25 MP draft, where the GPU's `finish + readback` took ~5 ms full size, so drags ran at 20 to 55 pictures a second
  with drafts of ~590 × 390 against ~115 full-size ones for SDR. The first HDR drag after SDR ones also starts at
  full size (the budget learned SDR's speed): 200 to 350 ms to the first picture.
- Headroom beyond what the screen reaches right now is clipped by the system (macOS), not tone-mapped by us.
- Values outside Display P3 are clamped (the render's gamut map is P3).
- The surface stays in HDR output for a second after the last HDR picture (a hover preview, the Before view or
  the next photo are gaps of a moment: each flip is a backlight ramp), so it is not switched off the moment
  you step away from an HDR photo.
- The window's own screenshot cannot read back a float surface, so `ui.screenshot` renders headlessly on a
  window that can draw HDR: it shows the 8-bit pictures.
- Generative Expand stays SDR (its missing area is transparent in the 8-bit picture; an HDR one has no alpha).
- The SwiftUI path is checked in the simulator for plumbing (it reports no EDR) and, on a Mac, by an HDR window
  capture (ScreenCaptureKit, `hdrLocalDisplay`): the same values above white as the float `CGImage` it replaced.
  How bright it looks needs someone at an EDR screen.
