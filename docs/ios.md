# iOS (iPhone and iPad)

Everything still missing, with priorities: [`ios-gaps.md`](ios-gaps.md).

**Status: a spike that runs on the simulator, with a native host written but not yet run.** The desktop egui app builds
as a static library, wrapped into an app by [xtool](https://github.com/xtool-org/xtool) (*Build and run with xtool*;
from Linux too) or an Xcode project, and it has run on the iOS 27 simulator (through Xcode) with a compact touch layout
(below). Since then the native pieces a usable app needs have been written (`crates/ios-host`, *Native host* below):
import from the Photos and Files pickers, export through the share sheet, HEIC through ImageIO, saving and pausing the
GPU as iOS backgrounds the app, a memory budget from the app's real limit. They type-check and pass clippy for iOS and
their logic is tested on Linux, but **none of them has run on a simulator or a device yet**. Its library, saved on the
device, starts with the procedural demo photos. Sync with your own LightCraft server ([sync.md](sync.md)) is wired up
but not yet run. This page records what is verified, what is not, and the plan.

## What is verified

`cargo xtask ios` runs `cargo check --target aarch64-apple-ios` for every L0–L5 crate (geometry, colour, raster, TIFF,
sysmem, raw, codecs, meta, develop, scenes, pipeline, GPU, catalog, preview, merge, engine, MCP and `ui-egui`) and the
app (`apps/lightcraft-ios`), and is part of `cargo xtask ci`. It needs only the Rust target (`rustup target add aarch64-apple-ios`), not Xcode, so it runs on
Linux. It proves the dependency tree has no desktop-only crate on the iOS path (wgpu builds with its Metal backend).
It does **not** prove an app links or runs: that needs xtool (any OS, with a device) or Xcode. The native host (`lightcraft-ios-host`, checked
with the app) has its Objective-C calls checked against the `objc2` bindings' signatures only; selectors and framework
behaviour are verified only by running it.

## Build and run with xtool

The app is built with [xtool](https://github.com/xtool-org/xtool) (MIT), a cross-platform Xcode replacement: it
builds a SwiftPM package into an iOS app, signs it with your Apple ID and installs it on a device over USB, from
**Linux, Windows (WSL) or macOS**. `apps/lightcraft-ios/xtool/` is that package: the Objective-C host
(`Sources/LightCraftHost`: `main`, the scene delegate, the `NSLog` bridge), `xtool.yml` (bundle id, icon) and
`Info.plist` (merged over xtool's defaults). xtool wraps its one library product into the app's executable; the app
itself is the Rust static library `liblightcraft_ios.a` (`apps/lightcraft-ios`), linked by `Package.swift` from `.rust/`.

**One-time setup** (Linux; macOS is the same without usbmuxd):

1. Rust targets: `rustup target add aarch64-apple-ios` (device) and, on a Mac, `aarch64-apple-ios-sim` (simulator).
2. xtool's prerequisites: a Swift 6.x toolchain for Linux (swift.org), `usbmuxd` (`sudo apt-get install usbmuxd
   libimobiledevice-utils`), and `Xcode.xip` downloaded from developer.apple.com (needs an Apple ID).
3. xtool itself: the `xtool-$(uname -m).AppImage` from its GitHub releases, renamed `xtool`, on the `PATH`.
4. `xtool setup`: log in (an API key with a paid Apple Developer Program membership, or any Apple ID by password; a
   free account's apps expire after 7 days and xtool prefixes the bundle id, e.g. `XTL-1234.ai.storyteller.lightcraft.ios`),
   then the path to `Xcode.xip`, from which it builds the iOS Swift SDK (`swift sdk list` shows `darwin`).
5. Connect the iPhone / iPad by USB, trust the computer, and turn on Developer Mode (Settings ▸ Privacy & Security).

**Each run:** `apps/lightcraft-ios/xtool/run.sh` builds the Rust library (`cargo build -p lightcraft-ios --target
aarch64-apple-ios --release`, with `IPHONEOS_DEPLOYMENT_TARGET=16.0`), copies it to `.rust/` and runs `xtool dev run`
(build, sign, install, launch). `--debug` builds Rust in debug (assertions; a much bigger app), `--build-only` stops at
`xtool/LightCraft.app`, `--ipa` makes an `.ipa`, `--simulator` runs on the booted simulator (xtool supports simulators
only on macOS).

**Logs:** every log record goes to the system log through `NSLog` (`idevicesyslog | grep LightCraft` on Linux,
Console.app on a Mac) and to `lightcraft-ios.log` in the app's tmp folder; a panic is also written to
`lightcraft-panics.log` there.

**Not yet tried:** none of this has run yet (the Rust side type-checks for iOS; the package, `run.sh` and the link have
not been through xtool). Likely first problems: the system libraries the Rust library needs at link time (`cargo rustc
-p lightcraft-ios --target aarch64-apple-ios --crate-type staticlib -- --print native-static-libs` lists them; add any
missing one to `Package.swift`), and whether SwiftPM accepts `unsafeFlags` in the package xtool builds against (it is a
local path dependency there).

**Xcode, on a Mac (alternative):** `xcode/project.yml` is an XcodeGen spec over the same host sources: `cd
apps/lightcraft-ios/xcode && xcodegen && open LightCraft.xcodeproj` (a pre-build phase runs `cargo build`; set
`DEVELOPMENT_TEAM` for a device). Useful for the debugger, Instruments and the simulator; keep its Info.plist keys in
step with `xtool/Info.plist`. This is how the spike first ran on the iOS 27 simulator (the touch layout: *What is
missing*, 1; the panels keep out of the safe area). Three fixes were needed, all in the host: (1) iOS 27 traps apps
without scene lifecycle and winit 0.30/0.31 has none, so `Sources/LightCraftHost/SceneDelegate.m` declares an empty
scene delegate (plus `UIApplicationSceneManifest` in `Info.plist`); (2) winit creates its window with `-[UIWindow
initWithFrame:]`, which a scene-based app never shows, so that file swizzles it to create the window in the connected
scene; (3) egui-wgpu's default device limits ask for 16 inter-stage shader variables, the simulator's Metal adapter
allows 15, so `wgpu_options()` clamps them to the adapter. On the simulator the log file is in the app's data container
(`xcrun simctl get_app_container <device> ai.storyteller.lightcraft.ios data`).

Both link PhotosUI, Photos, ImageIO and UniformTypeIdentifiers for the native host and set
`NSPhotoLibraryAddUsageDescription` (Save Image in the share sheet); importing needs no photo-library permission (the
Photos picker runs out of process).

**The library** is `LightCraft Library` in the app's Documents folder (kept across launches and updates, backed up
with the device); a new one starts with the demo photos. `$LIGHTCRAFT_LIBRARY` puts it elsewhere
(`SIMCTL_CHILD_LIGHTCRAFT_LIBRARY=… xcrun simctl launch …`). If it can't be opened the app falls back to the in-memory
demo library (nothing saved, no sync) and logs why.

**Sync** is wired up as on the desktop ([sync.md](sync.md)) but has not been run on a simulator or device yet (it is
type-checked for iOS and its library handling unit-tested on Linux): Menu ▸ Settings ▸ Sync in the compact layout
(Settings ▸ Sync or the cloud icon in the desktop one), the server address
(`photos.example.com` is enough; https is assumed, and a capitalised first letter is fine), user name and password;
the library then fills from the server, previews first, originals on request. The device signs in under the name
`LightCraftMain.m` reads from `UIDevice` ("iPhone", "iPad"; `$LIGHTCRAFT_DEVICE`), which the server's admin page lists. Requests
use the same pure-Rust transport as the desktop app (rustls, Mozilla roots), over plain sockets, so App Transport
Security doesn't apply; `Info.plist` sets `NSLocalNetworkUsageDescription` because a server on the home network needs
the local-network permission. To check on a Mac: typing in the Sync fields (the Mac's keyboard works on the simulator;
on a device the on-screen keyboard is listed as missing below, although egui-winit asks winit for it when a field takes
focus, which on iOS makes winit's view the first responder), the TLS handshake on a device, and sync while the app is
backgrounded (requests fail and are retried on return).

## Native host (written, not yet run)

`crates/ios-host` (`lightcraft-ios-host`) holds the app's Objective-C calls, through `objc2` (Rust bindings to the
system frameworks; no C or Objective-C is compiled). It is allowed `unsafe` like `crates/sysmem` (`CLAUDE.md`, *Never
crash*): every block says why it is sound and failures come back as errors. On other platforms it is an empty shell
whose portable helpers (safe file names, unique paths, the folder walk) are tested on Linux.

- **Import.** File ▸ Import from Photos… / from Files… / Folder from Files…, and the round + button over the phone
  grid (`Services::host_pick`). The Photos picker (PHPicker; no permission needed) hands over each photo as stored (a
  ProRAW DNG when there is one, else the HEIC or JPEG); the Files picker hands over copies; a folder is read in place
  under its security scope (at most 5000 photos, 12 levels deep). Everything is copied into `tmp/Import/` and opens the
  import review, which *moves* it into the library's `Originals/` (`file.addPhotos {staged: true}`: no "add in place").
- **Export.** Every export is written to `tmp/Exports/` (emptied first) and opens the share sheet: Save Image (to
  Photos), Save to Files, AirDrop, Mail, other apps. The export dialog has no folder field (`Services::share_exports`).
- **HEIC / HEIF, AVIF** decode through ImageIO, installed as `lightcraft-codecs`' system decoder (8- or 16-bit, in the
  photo's own colour space with its ICC profile); their EXIF and XMP come from `lightcraft-meta`'s HEIF parser.
- **Lifecycle.** The library, the view and the app settings (`ui.json`, in `Library/Application Support/LightCraft`)
  are saved when the app resigns active, goes to the background or terminates. In the background the GPU is paused
  (`gpu::pause`: renders go to the CPU, and errors meanwhile don't turn the GPU off for the rest of the run) and
  decoded photos are released; an export in progress asks for background time.
- **Memory.** The budget is a third of what the app may allocate at launch (`os_proc_available_memory`), between 256 MiB
  and 1 GiB; a memory warning releases decoded photos.
- **Paths.** A library opened from another folder than last time (iOS gives the container a new path on every app
  update) re-points the photos stored under its old folder (`location.json`).

To check on a device (`run.sh`) or a Mac: each picker (and cancelling it), a ProRAW and a Live Photo from the library, an iCloud file not yet
downloaded, a folder on a USB drive, the share sheet on iPhone and iPad (popover), Save Image's permission prompt,
backgrounding during an export, and a memory warning (Simulator ▸ Debug ▸ Simulate Memory Warning).

## Scope

iPhone and iPad, a library on the device (import, edit and export there), optionally shared with the user's other
devices through their own LightCraft server ([sync.md](sync.md)),
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
   context menus (egui's own long-touch) except in the grid, where it starts choosing photos: taps then add and remove
   them, and an action bar at the bottom rates, flags, labels, adds to an album, exports, copies / pastes settings or
   deletes them (Select in the top bar does the same); the loupe's star button rates, flags and labels the photo; a
   round + over the grid imports (with the host's pickers). Check it headless with `lightcraft-cli snapshot --demo --size 390x844
   --scale 2` (phone) or `--size 820x1180` (iPad). Pinch and two-finger pan are untested (the headless driver injects
   no multi-touch); everything is untested on a real device.
   Still missing: the on-screen keyboard for text fields, Apple Pencil, tool-specific touch polish (brush strokes with
   a finger while the sheet is open, the curve editor, the colour wheels) and a landscape phone layout.
2. **HEIC/HEIF decode: written, not yet run** (*Native host*): ImageIO through `lightcraft_codecs::set_system_decoder`.
3. **Memory: the budget is written, tiling is not.** The budget follows the app's limit (*Native host*), but the
   pipeline works on whole `f32` RGB images (about 288 MB at 24 MP) and does not tile, so previews are fine but
   full-resolution 48 MP export is a risk.
4. **A host: written, not yet run** (*Native host*): lifecycle, pickers, share sheet, sandbox paths. Still none:
   `std::process` open/reveal (Show in Finder, Edit in External Editor) and opening links.
5. **GPU lifecycle: done in the engine** (`gpu::pause` / `resume`, tested); the host calls it on backgrounding.
6. **Unverified:** eframe/winit and accesskit on a device, egui text input and long-press on a phone, sync from a
   device, Metal's
   storage-buffer limits on iPhone (the GPU path needs at least 10 per stage), Apple ProRAW against real files (JPEG XL
   compressed DNG is rejected), and the `rfd` file dialogs the desktop app uses.

Any `unsafe` or Objective-C glue goes in an isolated crate registered in `xtask/src/layers.rs`, like `crates/sysmem`
and `crates/ios-host` (see `CLAUDE.md`, *Never crash*).

## Plan

- **Phase 0, spike:** run the egui app on the simulator (done, through Xcode), an iPhone and an iPad (with xtool, from
  Linux too); log Metal limits; time a preview render and a 24 MP raw decode; record peak memory; check text input,
  long-press and safe areas. Output: a decision between egui and a native shell over FFI, with a revised estimate.
- **Phase 1, platform glue (written; run it):** host crate, pickers, sandbox paths, memory budget, GPU pause/resume,
  HEIC via ImageIO, JPEG/DNG/ProRAW verified on real files.
- **Phase 2, mobile UI:** grid, loupe with gestures, bottom-sheet edit tools, presets, crop, masks, export; adaptive
  iPhone and iPad layout; tests through headless snapshots at phone and tablet sizes.
- **Phase 3, hardening:** memory pressure, tiled full-resolution export, Apple Pencil, multitasking, VoiceOver,
  TestFlight and App Store review.

The project's own priorities (`ROADMAP.md`, *Where we're going*) put camera colour, raw coverage and AI ahead of mobile;
the gaps listed there apply to the phone app too.
