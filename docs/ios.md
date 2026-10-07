# iOS (iPhone and iPad)

**Status: no app yet.** The engine and the egui shell type-check for iOS; nothing has been built, linked or run on a
device or simulator. This page records what is verified, what is not, and the plan.

## What is verified

`cargo xtask ios` runs `cargo check --target aarch64-apple-ios` for every L0–L5 crate (geometry, colour, raster, TIFF,
sysmem, raw, codecs, meta, develop, scenes, pipeline, GPU, catalog, preview, merge, engine, MCP and `ui-egui`) and is
part of `cargo xtask ci`. It needs only the Rust target (`rustup target add aarch64-apple-ios`), not Xcode, so it runs on
Linux. It proves the dependency tree has no desktop-only crate on the iOS path (wgpu builds with its Metal backend).
It does **not** prove an app links or runs: that needs macOS and Xcode.

## Xcode project (phase 0 spike)

`apps/lightcraft-ios` is the egui app as a static library (in-memory demo library, no pickers). `xcode/project.yml`
is an XcodeGen spec: `cd apps/lightcraft-ios/xcode && xcodegen && open LightCraft.xcodeproj`. A pre-build phase runs
`cargo build` for the SDK being built (Apple-silicon simulator or device; set `DEVELOPMENT_TEAM` for a device).
It runs on the iOS 27 simulator: the desktop UI renders (not touch-adapted, ignores the safe area). Three fixes were
needed, all in the host: (1) iOS 27 traps apps without scene lifecycle and winit 0.30/0.31 has none, so
`xcode/Sources/SceneDelegate.m` declares an empty scene delegate (plus `UIApplicationSceneManifest` in `project.yml`);
(2) winit creates its window with `-[UIWindow initWithFrame:]`, which a scene-based app never shows, so that file
swizzles it to create the window in the connected scene; (3) egui-wgpu's default device limits ask for 16 inter-stage
shader variables, the simulator's Metal adapter allows 15, so `wgpu_options()` clamps them to the adapter. Log output
goes to `lightcraft-ios.log` in the app's tmp dir (`xcrun simctl get_app_container <device> ai.storyteller.lightcraft.ios data`).
Not yet tried on a device.

## Scope

iPhone and iPad, a standalone library on the device (import, edit and export there; cloud sync stays out of scope),
with a layout and behaviour modelled on Lightroom's mobile app. As everywhere in this repo that means imitating
layout and interaction only: no Adobe icons, artwork, fonts, presets or screenshots (see `CLAUDE.md`, *Assets*).

## What is missing

1. **A touch UI (done for the spike, unverified on a device).** `ui-egui` has a compact layout (`panels/compact.rs`) used
   when the content is narrower than `COMPACT_BELOW_PT` (900 pt; `LightcraftApp::compact`), i.e. iPhones and iPads in
   portrait; iPad landscape (1024 pt and more) keeps the desktop layout. Slim top bar with a Menu button (the whole
   command menu: import, export, settings…), the grid (about three tiles across, month headers) or the loupe, a bottom
   tab bar (Presets, Edit, Crop, Remove, Masking, Info) and the active tool as a bottom sheet (from 600 pt wide: a
   panel on the right, with My Photos as a column on the left; on a phone My Photos is a page of its own). The sheets
   reuse the desktop panel bodies. Touch: a tap opens a photo, pinch zooms, two fingers pan, a sideways swipe on a
   fitted photo goes to the next / previous one, double tap zooms, sliders have 68 pt rows and a 48 pt grab zone,
   egui-drawn rows are 44 pt, crop handles and mask / spot pins are about a finger wide, and press-and-hold opens the
   context menus (egui's own long-touch). Check it headless with `lightcraft-cli snapshot --demo --size 390x844
   --scale 2` (phone) or `--size 820x1180` (iPad). Pinch and two-finger pan are untested (the headless driver injects
   no multi-touch); everything is untested on a real device.
   Still missing: the native pickers and share sheet that make Menu → Import / Export work (item 4), the on-screen
   keyboard for text fields, Apple Pencil, tool-specific touch polish (brush strokes with a finger while the sheet is
   open, the curve editor, the colour wheels) and a landscape phone layout.
2. **HEIC/HEIF decode.** iPhone photos are HEIC; `crates/codecs` recognises the format but cannot decode it, and import
   currently accepts `.heic` files it then fails on. The pragmatic route is ImageIO behind the `FileLoader` hook
   (`crates/engine/src/media.rs`).
3. **A memory budget.** The RAM probe (`crates/engine/src/memory.rs`) returns nothing on iOS, so the budget defaults to
   1.5 GiB, too high for iOS memory limits. The pipeline works on whole `f32` RGB images (about 288 MB at 24 MP) and
   does not tile, so previews are fine but full-resolution 48 MP export is a risk.
4. **A host.** App entry and lifecycle, Files and Photos pickers, export through the share sheet, sandbox paths (the
   library and config directories assume `$HOME`), and no `std::process` open/reveal. Originals are referenced by
   absolute path; on iOS import must copy into the library.
5. **GPU lifecycle.** A device error disables the GPU for the rest of the process (`crates/gpu/src/lib.rs`); backgrounding
   needs to pause and resume it instead.
6. **Unverified:** eframe/winit and accesskit on iOS, egui text input and long-press on a phone, Metal's
   storage-buffer limits on iPhone (the GPU path needs at least 10 per stage), Apple ProRAW against real files (JPEG XL
   compressed DNG is rejected), and the `rfd` file dialogs the desktop app uses.

Any `unsafe` or Objective-C glue goes in an isolated crate registered in `xtask/src/layers.rs`, like `crates/sysmem`
(see `CLAUDE.md`, *Never crash*).

## Plan

- **Phase 0, spike (needs a Mac):** run the egui app on the simulator, an iPhone and an iPad; log Metal limits; time a
  preview render and a 24 MP raw decode; record peak memory; check text input, long-press and safe areas. Output: a
  decision between egui and a native shell over FFI, with a revised estimate.
- **Phase 1, platform glue:** host crate, pickers, sandbox paths, memory budget, GPU pause/resume, HEIC via ImageIO,
  JPEG/DNG/ProRAW verified on real files.
- **Phase 2, mobile UI:** grid, loupe with gestures, bottom-sheet edit tools, presets, crop, masks, export; adaptive
  iPhone and iPad layout; tests through headless snapshots at phone and tablet sizes.
- **Phase 3, hardening:** memory pressure, tiled full-resolution export, Apple Pencil, multitasking, VoiceOver,
  TestFlight and App Store review.

The project's own priorities (`ROADMAP.md`, *Where we're going*) put camera colour, raw coverage and AI ahead of mobile;
the gaps listed there apply to the phone app too.
