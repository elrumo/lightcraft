# iOS (iPhone and iPad)

Everything still missing, with priorities: [`ios-gaps.md`](ios-gaps.md).

**Status: runs on an iPhone and on the simulator; the native host works on the simulator, partly checked on a
device.** The desktop egui app builds as a static library, wrapped into an app by an Xcode project (*Xcode, on a Mac*)
or [xtool](https://github.com/xtool-org/xtool) (*Build and run with xtool*; written for Linux too, not yet tried).
It has run on an iPhone 17 Pro (iOS 27.2) and the iOS 27 simulator with a compact touch layout (below). The native
pieces (`crates/ios-host`, *Native host* below) — the Photos and Files pickers, export through the share sheet, HEIC
through ImageIO, saving and pausing the GPU as iOS backgrounds the app, a memory budget from the app's real limit —
were checked on the simulator by hand (2026-10-08, iPhone 18 Pro simulator, iOS 27.0); on the device so far: launch,
the GPU, the memory budget, the library on disk and the lifecycle notifications. *Native host* lists what was checked
where. Its library, saved on the device, starts with the procedural demo photos. Sync with your own LightCraft server
([sync.md](sync.md)) is wired up but not yet run. This page records what is verified, what is not, and the plan.

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

**Not yet tried:** xtool itself has not run yet (the same sources, Rust library and frameworks link and run through
Xcode, below; the package and `run.sh` have not been through xtool). Likely first problems: the system libraries the Rust library needs at link time (`cargo rustc
-p lightcraft-ios --target aarch64-apple-ios --crate-type staticlib -- --print native-static-libs` lists them; add any
missing one to `Package.swift`), and whether SwiftPM accepts `unsafeFlags` in the package xtool builds against (it is a
local path dependency there).

**Xcode, on a Mac:** `xcode/project.yml` is an XcodeGen spec over the same host sources: `cd
apps/lightcraft-ios/xcode && xcodegen && open LightCraft.xcodeproj` (a pre-build phase runs `cargo build`, honouring
`CARGO_TARGET_DIR`; set `DEVELOPMENT_TEAM` for a device). This is how it first ran on a device; from the command line
(any Apple ID signed in to Xcode; a team's wildcard profile signs it):

```sh
cd apps/lightcraft-ios/xcode && xcodegen
xcodebuild -project LightCraft.xcodeproj -target LightCraft -sdk iphoneos -configuration Release \
  -allowProvisioningUpdates DEVELOPMENT_TEAM=<team id> SYMROOT=$PWD/build build
xcrun devicectl device install app --device <udid> build/Release-iphoneos/LightCraft.app
xcrun devicectl device process launch --device <udid> --terminate-existing ai.storyteller.lightcraft.ios
```

`xcrun devicectl list devices` gives the UDID (USB or the same Wi-Fi network). The app's logs:
`xcrun devicectl device copy from --device <udid> --domain-type appDataContainer --domain-identifier
ai.storyteller.lightcraft.ios --source tmp --destination ./tmp` (`lightcraft-ios.log`, `lightcraft-panics.log`);
a screenshot: `xcrun devicectl device capture screenshot --device <udid> --destination shot.png` (it captures whatever
is on screen, so only while LightCraft is in front). The release library links with no extra system libraries
(`-lc++` and the frameworks in `project.yml`); the app is 43 MB. Useful for the debugger, Instruments and the simulator; keep its Info.plist keys in
step with `xtool/Info.plist`. This is how the spike first ran on the iOS 27 simulator (the touch layout: *What is
missing*, 1; the panels keep out of the safe area). Three fixes were needed, all in the host: (1) iOS 27 traps apps
without scene lifecycle and winit 0.30/0.31 has none, so `Sources/LightCraftHost/SceneDelegate.m` declares an empty
scene delegate (plus `UIApplicationSceneManifest` in `Info.plist`); (2) winit creates its window with `-[UIWindow
initWithFrame:]`, which a scene-based app never shows, so that file swizzles it to create the window in the connected
scene; (3) egui-wgpu's default device limits ask for 16 inter-stage shader variables, the simulator's Metal adapter
allows 15, so `wgpu_options()` clamps them to the adapter. On the simulator the log file is in the app's data container
(`xcrun simctl get_app_container <device> ai.storyteller.lightcraft.ios data`).

**Driving the app on a device** (no taps needed): launched with `LIGHTCRAFT_SCRIPT=<file>`, the app runs the
control-protocol requests in that file, one JSON object per line (`{"method": "ui.screenshot", "params": {"path":
"…"}}`, see [control-protocol.md](control-protocol.md); `{"sleep": 500}` waits), and appends each reply to the same
path with the extension `.out`; a relative path (the script's, or a screenshot's) is in the app's tmp folder. Copy
the script there, launch with the variable, copy the replies and screenshots back:

```sh
xcrun devicectl device copy to --device <udid> --domain-type appDataContainer \
  --domain-identifier ai.storyteller.lightcraft.ios --source tour.jsonl --destination tmp/tour.jsonl
xcrun devicectl device process launch --device <udid> --terminate-existing \
  --environment-variables '{"LIGHTCRAFT_SCRIPT": "tour.jsonl"}' ai.storyteller.lightcraft.ios
xcrun devicectl device copy from --device <udid> --domain-type appDataContainer \
  --domain-identifier ai.storyteller.lightcraft.ios --source tmp --destination ./device-tmp
```

The file stays in the app's sandbox, so nothing listens on the network. On the simulator, `SIMCTL_CHILD_LIGHTCRAFT_SCRIPT=…
xcrun simctl launch …` does the same.

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

## Native host (run on the simulator; partly on a device)

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

**Checked by hand** (2026-10-08; simulator: iPhone 18 Pro, iOS 27.0; device: iPhone 17 Pro, iOS 27.2, through Xcode,
see *Xcode, on a Mac*):

| What | Simulator | Device |
|---|---|---|
| Launch, Metal (`Apple A19 Pro GPU`, buffers ≤ 4095 MiB on the device; 256 MiB on the simulator), the library in Documents | ✅ | ✅ |
| Memory budget: 3373 MB available → 1024 MB budget (the simulator reports 0: the 768 MB default) | ✅ | ✅ |
| Lifecycle notifications: resign / active / background; decoded photos released in the background | ✅ | ✅ |
| Photos picker: two photos (a HEIC, a JPEG) copied, reviewed with ImageIO thumbnails, moved into `Originals/<date>`; a duplicate tagged; cancelling does nothing | ✅ | — |
| Files picker: one file (the picker's copy); a folder with a HEIC, a JPEG in a subfolder and a `.txt` (skipped); the originals in Files untouched; cancelling | ✅ | — |
| HEIC through ImageIO: a 4032 × 3024 iPhone HEIC in the loupe; camera, lens, exposure and capture date from `lightcraft-meta` | ✅ | — |
| Export → share sheet; Save Image asks with LightCraft's own text and the photo lands in Photos | ✅ | — |
| On-screen keyboard: rises for a field, typing, Return confirms, the layout above it (A1.9) | ✅ | — |
| A library moved by an app update re-points its photos (`location.json`; a reinstall gave the container a new path) | — | ✅ |
| The compact layout (A2.22) draws: the edit view with its group row, sliders and tool bar (an in-app screenshot through `LIGHTCRAFT_SCRIPT`) | ✅ | ✅ |

Not yet checked anywhere: a ProRAW DNG and a Live Photo from a real library, an iCloud file not yet downloaded, a
folder on a USB drive, the share sheet as an iPad popover, backgrounding during an export, a memory warning, sync.
The device column needs someone at the phone (pickers and sheets are system UI); `LIGHTCRAFT_SCRIPT` covers the rest.

## Scope

iPhone and iPad, a library on the device (import, edit and export there), optionally shared with the user's other
devices through their own LightCraft server ([sync.md](sync.md)),
with a layout and behaviour modelled on Lightroom's mobile app. As everywhere in this repo that means imitating
layout and interaction only: no Adobe icons, artwork, fonts, presets or screenshots (see `CLAUDE.md`, *Assets*).

## What is missing

1. **A touch UI (done for the phone and iPad portrait; checked on the simulator, not yet used on a device).** `ui-egui` has a
   compact layout (`panels/compact.rs`, `panels/mobile.rs`) used when the content is narrower than `COMPACT_BELOW_PT`
   (900 pt; `LightcraftApp::compact`), i.e. iPhones and iPads in portrait; iPad landscape (1024 pt and more) keeps the
   desktop layout. It follows Lightroom's mobile app for layout and behaviour and iOS for its look (`Tokens::ios`:
   Apple's dark-mode system colours, 44 pt rows; [ios-gaps.md](ios-gaps.md) A2.22–A2.25), and has **no menu bar**.
   The grid: the collection as its title (▾ opens the albums list as a page), filter and sort beside it, Select and
   "…" (import, new album, sort, settings, help…) above, square tiles three across and edge to edge with the photo
   filling each (Square Thumbnails, the default on a first start; justified rows otherwise) under month headers, and a
   round + (the host's pickers). A photo: back, undo, share and "…" (rating, flag, label, copy / paste / reset edits,
   versions, history, keywords, delete) above it; below it the tools (Presets, Crop, Edit, Masking, Remove, Info),
   the open one on a blue tile, with the Edit tool's groups (Auto, Profile, Light, Color, Effects, Detail, Optics,
   Calibration) in a row of their own and one group's sliders in the sheet (from 600 pt wide: a panel on the right,
   with My Photos as a column on the left). Dialogs are pages sliding up from the bottom (Cancel, title and action
   in the bar), menus are iOS pull-downs or action sheets, and every other command is in a searchable All Commands
   list. Touch: a tap opens a photo, pinch zooms, two fingers pan, a sideways swipe on a fitted photo goes to the
   next / previous one, an up / down swipe rates it (left half) or flags it (right half), double tap zooms, sliders follow a finger's sideways movement (a tap leaves them alone, an up
   or down drag scrolls), crop handles
   and mask / spot pins are about a finger wide, press-and-hold opens context menus (egui's own long-touch) except in
   the grid, where it starts choosing photos (taps then add and remove them; an action bar rates, flags, labels, adds
   to an album, exports, copies / pastes settings or deletes them). The on-screen keyboard rises for text fields,
   Return confirms them, and the layout moves above it. Check it headless with `lightcraft-cli snapshot --demo --size
   390x844 --scale 2` (phone) or `--size 820x1180` (iPad). Pinch and two-finger pan are untested (the headless driver
   injects no multi-touch).
   Dialogs use iOS switches and segmented controls; a vertical drag on a slider scrolls its sheet.
   Still missing: Apple Pencil, tool-specific touch polish (brush strokes with a finger while the sheet is open, the
   curve editor, the colour wheels), iOS pickers instead of drop-down menus, and a landscape phone layout.
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

- **Phase 0, spike:** run the egui app on the simulator (done, through Xcode), an iPhone (done, through Xcode) and an
  iPad (with xtool, from Linux too); log Metal limits; time a preview render and a 24 MP raw decode; record peak
  memory; check text input, long-press and safe areas. Output: a decision between egui and a native shell over FFI,
  with a revised estimate. **Measured on an iPhone 17 Pro** (iOS 27.2, release build, `LIGHTCRAFT_SCRIPT` +
  `LIGHTCRAFT_PROFILE=1` through `devicectl … --console`, 2026-10-08): Metal adapter `Apple A19 Pro GPU`, buffers up
  to 4095 MiB; 3373 MB available to the app at launch (budget 1024 MB); a 24 MP demo photo's preview took 373 ms the
  first time (with the GPU's start-up) and 38–51 ms for the next ones (GPU stages: sampling 25–42 ms, the rest a few
  ms; once 467 ms in white balance / noise reduction); the caches held 114 MB after two previews and 224 MB after
  four; frames took under 0.5 ms of layout, the slowest update 28 ms. Not measured yet: a raw decode (no raw on the
  device without importing the user's photos or the CC0 corpus), the storage-buffer limits (A1.12) and the process's
  peak memory (the caches' total is what is known).
- **Phase 1, platform glue (written; run it):** host crate, pickers, sandbox paths, memory budget, GPU pause/resume,
  HEIC via ImageIO, JPEG/DNG/ProRAW verified on real files.
- **Phase 2, mobile UI:** grid, loupe with gestures, bottom-sheet edit tools, presets, crop, masks, export; adaptive
  iPhone and iPad layout; tests through headless snapshots at phone and tablet sizes.
- **Phase 3, hardening:** memory pressure, tiled full-resolution export, Apple Pencil, multitasking, VoiceOver,
  TestFlight and App Store review.

The project's own priorities (`ROADMAP.md`, *Where we're going*) put camera colour, raw coverage and AI ahead of mobile;
the gaps listed there apply to the phone app too.
