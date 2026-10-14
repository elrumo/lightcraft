# The SwiftUI iOS app

**Status: S1 under way (2026-10-14): the phone layout (grid with Months / Years, albums, filter, select mode, the photo
with its tools and every Edit group, Crop, Masking, Remove, export and share, Settings, sync, People, Map) runs on an
iOS 26.5 simulator; not yet on a device.** Row by row: `apps/lightcraft-ios-swift/PARITY.md`. A native SwiftUI app for iPhone and iPad
(iOS 26 and later) that runs the whole Rust core: the engine, the develop pipeline, the catalog, sync, export. It is to
replace the egui iOS app ([ios.md](ios.md)) once it passes its parity gate. The decision and its reasons are in
[ios.md](ios.md) → *Plan*, Phase 0. What iOS still lacks overall: [ios-gaps.md](ios-gaps.md).

What S0 has: the grid (square thumbnails), the loupe (fit, pinch, pan, double tap), the Edit tool's Light group (sliders
built from the engine's control specs, drafts while dragging, one undo step per drag), undo and redo, an All Commands
screen that lists and runs every engine command, and automation (`LIGHTCRAFT_SCRIPT`). The Rust side is tested on Linux
(`cargo xtask ci`); the Swift side is built and tested on macOS by `.github/workflows/ios-swift.yml`, which also runs the
app on a simulator and uploads its screenshots.

## Shape

```
SwiftUI app (@Observable views, MainActor)        apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp
LightCraftCore: Engine, Codable protocol,         apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftCore
  AsyncStream of events, one pump thread
CLightCraftBridge: the C header (a module map)    apps/lightcraft-ios-swift/LightCraft/Sources/CLightCraftBridge
──────────────────────────────── C ABI (lc_*) ────────────────────────────────
apps/lightcraft-ios-swift: Rust static library `lightcraft_bridge`; its only unsafe code is src/abi.rs
crates/apphost (L4): the engine thread that owns the Session, render scheduling, background pumps, events
engine, pipeline, catalog, … (shared with the desktop, web and egui iOS apps)
```

- **One engine thread.** `Session` is `Send` but not `Sync`, so one thread (`lc-engine`, `crates/apphost/src/host`)
  owns it and runs every command, render request and background pump, as the egui app's UI thread does. Swift never
  waits on a lock held by background work.
- **The host drives the background work.** Between requests the engine thread does what the egui app's frame loop does:
  renders, saving, the activity stack, the self-hosted sync (`apphost::sync`: requests on worker threads, answers
  applied on the engine thread, a pull when the app comes back to the front), the face scan's pace (`apphost::faces`),
  exports (`apphost::export::Batch`) and imports (`apphost::import`), all shared with egui.
- **Messages, not calls.** Swift sends JSON requests and reads JSON events from a queue it polls on a thread of its
  own (`EventPump`). Commands go through `Session::execute`, the entry point every frontend uses, so the Swift app gets
  every engine command (363) without binding code per command.
- **Pixels are leases.** A render's RGBA8 buffer stays in Rust until Swift copies it out by its token and releases it.
  The engine keeps only the render's facts (key, size, histogram); a picture nobody took is dropped when a newer one for
  its slot arrives, and untaken pictures are capped at 256 MB.
- **Swift holds presentation only.** No document logic, no edit geometry, no image math in Swift. Logic the Swift app
  needs that still lives in `ui-egui` moves down first (*Extraction rule*).

## Extraction rule

When the Swift app first needs logic that sits in `crates/ui-egui`, the same commit moves it down and switches egui to
it; it is never written twice.

| What | Where it goes |
|---|---|
| Geometry and edit math (crop handles, mask handles, spot pins, curve points, the canvas ↔ image mapping) | `geom`, `develop`, `color` |
| Behaviour of the session (a rule that changes the document) | an engine command |
| Threads, render scheduling, background pumps (import, export, sync, faces…), post-command policy | `apphost` |

Swift keeps view state and view-transform math only (fit, pan clamping, zoom anchor). Moved so far: the render
scheduler (`apphost::render::Scheduler`, egui's `Renderer` is it plus egui textures), the thumbnail request rules
(`Scheduler::request_thumb`), headless Export Now (`apphost::export`, shared with the MCP server) and the mobile start-up
(`apphost::startup`, shared with the egui iOS app).

## The C ABI

Defined in `apps/lightcraft-ios-swift/src/abi.rs`; the header Swift imports is
`apps/lightcraft-ios-swift/LightCraft/Sources/CLightCraftBridge/include/lightcraft_bridge.h`, and the test
`header_matches_exports` checks that the two agree (names, parameters, struct layout, status codes, `LC_ABI_VERSION`).
CI on macOS also checks that the built library exports exactly those symbols.

| Function | What it does |
|---|---|
| `lc_open(config)` | starts an engine from a JSON `OpenConfig` (`home`, `tmp`, `library`, `demo`, `deviceName`, `script`, `renderThreads`, `availableMemory`); answers `{engine, library, photos, demo, warnings}` |
| `lc_submit(engine, request)` | queues a request; its answer comes as a `reply` event |
| `lc_call(engine, request, timeout)` | runs a request and waits for its reply (start-up, tests) |
| `lc_poll(engine, timeout)` | the events since the last poll, as a JSON array (blocks up to the timeout) |
| `lc_wake(engine)` | a waiting `lc_poll` returns now |
| `lc_image(token)` | an `image` event's pixels (RGBA8, sRGB, straight alpha) |
| `lc_release(token)` | frees any buffer the library handed out |
| `lc_close(engine)` | saves, stops the threads, forgets the handle |
| `lc_abi_version()` | the ABI the library speaks |

Handles are keys in a registry, never pointers, so a stale or forged handle is an error; every output is a lease freed
with `lc_release`; every entry point runs under `catch_unwind`, so a panic comes back as `LC_PANIC` rather than aborting
the app. The only `unsafe` is one helper that turns the caller's `(pointer, length)` into a slice after refusing null
and anything over 16 MiB (`CLAUDE.md`, *Never crash*).

## Protocol

Requests are `{"id", "method", "params"}`; each gets a `reply` event with the same `id` (`result`, or `error: {code,
message}` where `code` is the engine's error kind — `unknownCommand`, `disabled`, `badParams`, `notSaved`… — or the
host's: `badRequest`, `unknownMethod`, `panic`, `closed`, `timeout`).

| Method | Params | Does |
|---|---|---|
| `engine.execute` | `{command, params}` | any engine command, and `app.export` (Export Now: on a worker with an activity row, ✕ stops it; answered when it ends, `cancelled` when stopped); `enhance.superRes` runs the same way (Enlarging; answered with the new photo, or error `cancelled`) |
| `import.scan` | `{paths}` | the files and folders for the import review, scanned on a worker (answered when done: `{candidates}`, each with `inTrash`) |
| `import.start` | `{paths \| indexes, …library.import params, albumName?}` | imports in the background in batches (one undo step; answered when done with what it added) |
| `engine.commands` | | every command with whether it can run now and why not |
| `host.state` | | the state below |
| `host.lifecycle` | `{event}` | `resignActive`, `background` (save, pause the GPU, free memory), `active`, `memoryWarning`, `terminate` |
| `render.thumb` | `{photo, edge, priority?}` | a grid thumbnail (embedded raw previews first; cheap to repeat) |
| `render.view` | `{photo?, width, height, draft?, before?, applyCrop?, overlay?: {kind: mask, id, view?, color?, opacity?} \| {kind: spots, threshold?} \| {kind: pointColor, index} \| {kind: hdr}, zoom?: {drawn, visible: [x0, y0, x1, y1]}}` | the loupe's picture (`main`; a stand-in in `preview` until it is there), with a diagnostic overlay (the selected mask, Visualize Spots…); zoomed in (`drawn`: the photo's long edge as drawn, px; `visible`: the part on screen, as fractions) also the window on screen at the zoom scale (`region`, its image event's `window: {full: [w, h], x, y, w, h}` says where it goes; planned by `apphost::region`) |
| `render.prefetch` | `{photos, width, height}` | prepares the neighbours |
| `render.variant` | `{photo?, preset \| profile \| version, amount?, edge?}` | a preview of the photo with a preset, profile or version (slot `variant:<key>`) |
| `render.face` | `{photo?, rect, edge?}` | a face, as People shows it (`rect` as `people.list` and `faces.person` give it; slot `variant:<key>`) |
| `render.candidate` | `{index, edge?}` | an import candidate's thumbnail (slot `import:<index>`) |
| `render.cancel` | `{slots}` | stops rendering for slots |
| `render.retain` | `{thumbs, max?}` | the thumbnails on screen (queued renders of others are dropped) |
| `ui.reply` | `{id, result \| error}` | the app's answer to a `uiRequest` |

Events: `reply`; `image {slot, photo, key, token, width, height, final, draft, quick?, ms, histogram?}` (slots
`thumb:<id>`, `thumbQuick:<id>`, `main`, `preview`, `before`…); `renderFailed {slot, photo, key, error}`; `state {state:
{catalogRevision, visibleGen, visibleCount, libraryGeneration, active, selectionCount, undo, redo, interaction,
activeMask, activeSpot, unsaved, sync?, faces?}}` when any of it changes (`faces`: the face scan's `{pendingPhotos,
indexedFaces}` while recognition is on) (`activeMask` / `activeSpot`: the Masking tool's
selected mask and the Remove tool's selected spot, wherever they were chosen) (`sync`: the server sync for the cloud icon, `{state, server, pending, uploads,
downloads, reading, error}`; absent for a local library); `activity {tasks}` (the activity stack); `message {text,
level}`; `copy {text}` (put it on the clipboard: a share link just made); `ask {dialog, params}` (show a dialog the
engine needs an answer from: `syncChoice`); `uiRequest {id, method, params}`; `closed`. One of each is in `LightCraft/Tests/LightCraftCoreTests/Fixtures/protocol.json`, written by
the Rust test `fixtures_match_the_protocol` (`LIGHTCRAFT_BLESS=1` rewrites it) and decoded by the Swift tests.

A slider drag: `develop.beginInteraction {label}`, then `develop.set {control, value}` with at most one in flight (the
newest value wins) and `render.view {draft: true}` after each, then the last value and `develop.endInteraction` (one
undo step); the full render follows from the `state` event.

## Build and run

**With xtool** (Linux, Windows (WSL) or macOS; set up as for the egui app, [ios.md](ios.md) → *Build and run with
xtool*): `apps/lightcraft-ios-swift/run.sh` builds the Rust library (`cargo build -p lightcraft-ios-swift --target
aarch64-apple-ios --release`, `IPHONEOS_DEPLOYMENT_TARGET=26.0`) into `LightCraft/.rust/` and runs `xtool dev run`;
`--simulator`, `--build-only`, `--ipa`, `--debug` as for the egui app. The bundle id is
`ai.storyteller.lightcraft.swift` while both apps exist (the SwiftUI app takes `ai.storyteller.lightcraft.ios` at the
cut-over, so installed libraries carry over). *Not yet tried.*

**With Xcode, on a Mac:** `open apps/lightcraft-ios-swift/xcode/LightCraft.xcodeproj`, pick a simulator or your
device and Run. A pre-build phase (`xcode/build-rust.sh`) builds the Rust library for the SDK being built (debug for
Debug, `--release` for Release). The app target compiles `LightCraft/Sources/LightCraftApp` as a synchronized folder
(new Swift files there need no project edit) over the package's `LightCraftCore` and `LightCraftImaging`, and takes
its Info.plist keys from `LightCraft/Info.plist`, the file xtool uses. For a device, choose your team under Signing &
Capabilities (or pass `DEVELOPMENT_TEAM=<team>` to `xcodebuild`).

**Tests:** `apps/lightcraft-ios-swift/run.sh --test` (macOS) builds the library for the Mac and runs `swift test`:
`ProtocolTests` (every event and request shape) and `EngineTests` (the real engine: commands, a thumbnail's pixels, a
drag as one undo step, 200 concurrent requests, a closed engine). The Rust side's tests (`cargo test -p lightcraft-apphost
-p lightcraft-ios-swift`) run everywhere.

The Swift package needs Swift 6.2 (Xcode 26) and links the system frameworks only; `LightCraftApp` is compiled for iOS
only (`#if os(iOS)`), so `swift test` on a Mac builds the rest. The Rust library's log lines reach the system log through
`lightcraft_host_log` (an `@_cdecl` function in `HostLog.swift`), as the egui app's Objective-C host does it.

## Driving the app

`LIGHTCRAFT_SCRIPT=<file>` (a file in the app's tmp folder) runs the control-protocol requests in it, one per line
(`{"sleep": ms}` waits), and appends each reply to `<file>.out`; a relative `path` is next to the script. The engine
answers `engine.*`, `render.*` and `host.*` itself; the app answers `ui.screenshot {path}` (the window as a PNG),
`ui.inspect` (screen, photo, state, `perf`: the last render's ms and the slider-to-draft round trip) and `ui.set {screen:
grid | photo | edit | crop | masking | remove | redEye | presets | info | commands | people | map, photo?, span?, group?, sheet?,
section?, select?, faceBoxes?, tagFace?, beforeAfter?, visualizeSpots?, removeAI?, histogram?, cropGuide?}` (`ScriptHost.swift` lists
the values).

```sh
# simulator: ci/sim-tour.sh does all of this (CI uploads its screenshots)
apps/lightcraft-ios-swift/ci/sim-tour.sh path/to/LightCraft.app out/
# device
xcrun devicectl device copy to --device <udid> --domain-type appDataContainer \
  --domain-identifier ai.storyteller.lightcraft.swift --source tour.jsonl --destination tmp/tour.jsonl
xcrun devicectl device process launch --device <udid> --terminate-existing \
  --environment-variables '{"LIGHTCRAFT_SCRIPT": "tour.jsonl"}' ai.storyteller.lightcraft.swift
```

## Plan

One task id per commit. **S0** builds the foundation and a vertical slice; **S1** reaches parity with the egui app's
phone layout (iPhone, iPad portrait), in the order of [ios-gaps.md](ios-gaps.md) → *Suggested order*; **S2** adds the
full iPad layout (menu bar and shortcuts from the command specs, pointer, compare and survey, every dialog, multiwindow,
drag and drop, Apple Pencil); **S3** removes the egui iOS app.

| Task | What | Status |
|---|---|---|
| SW0.1 | Rules (`CLAUDE.md`), layers (`apphost` L4, `ios-swift` with no Rust UI), this page | done |
| SW0.2 | The job pool says when a job finishes; a panicking job no longer takes its worker down | done |
| SW0.3 | `crates/apphost` and the render scheduler moved out of `ui-egui/src/render.rs`; a panicking render fails its slot | done |
| SW0.4 | Shared export and start-up (`apphost::export`, `apphost::startup`, the ImageIO decoder and logger in `ios-host`) | done |
| SW0.5 | The engine thread, protocol, event queue, leases, automation script runner | done |
| SW0.6 | The bridge crate and its C ABI; header check; protocol fixtures | done |
| SW0.7 | The Swift package (`LightCraftCore`, `LightCraftImaging`, the app target), xtool and Xcode projects | done (Xcode; xtool not yet tried) |
| SW0.8 | Vertical slice: grid, loupe, Light sliders from `develop.controls`, undo, All Commands | done (simulator) |
| SW0.9 | Automation: `LIGHTCRAFT_SCRIPT`, `ui.screenshot`, `ui.inspect`, `ui.set` | done (simulator) |
| SW0.10 | Parity manifest (`apps/lightcraft-ios-swift/PARITY.md`) checked by `cargo xtask parity` | done |
| SW0.11 | macOS CI workflow (`.github/workflows/ios-swift.yml`) | done |
| SW0.12 | First run on a simulator and a device; latency against the budgets (slider ≤ 16 ms draft, loupe ≤ 60 ms) | simulator: `ci/tour.jsonl` passes, edits persist across launches; device and release-build latency not yet |

S1, in order (each first moves what it needs out of `ui-egui`): lifecycle and background exports (SW1.1); import from
Photos and Files with the review (SW1.2, the import worker → `apphost`); HEIC / ProRAW on a device (SW1.3); the export
page, share sheet and Save to Photos (SW1.4, the export worker → `apphost`); select mode and the action bar (SW1.5, the
post-command policy → `apphost`); text input (SW1.6); signed builds (SW1.7); then the zoomed loupe, carousel and review
mode (SW1.8, region geometry → `apphost`), every Edit group, curves, mixer and grading (SW1.9–10), the canvas mapping
(SW1.11), Crop, Masking and Remove (SW1.12–14, their handle geometry → `geom` / `develop`), versions, history, copy and
paste, undo gestures, presets (SW1.15–18), sync, albums, filters, map, people, AI tools, HDR, settings and localisation
(SW1.19–25: `cargo xtask ios-strings` turns the egui catalogs into a String Catalog), landscape and iPad portrait
(SW1.26) and the phone audit (`cargo xtask parity --ios-gate S1`, SW1.27).

Open questions (not blocking S0): MapKit or the built-in offline map; iOS per-app language or an in-app override; which
AI engine features ship on iOS (binary size); whether the gate also compares screenshots of the two apps.

## Parity gate

`apps/lightcraft-ios-swift/PARITY.md` has a row for every UI-only command of the egui app (`ui:<id>`, from
`crates/ui-egui/src/menus.rs`), every dialog (`dialog:<Variant>`, from `crates/ui-egui/src/state.rs`) and every phone and
iPad screen (`screen:<name>`), each with its phase, status and evidence. Engine commands need no rows: the All Commands
screen shows and runs every one. `cargo xtask parity` (in `ci`) checks the manifest — a new egui UI command or dialog
fails it until its row says what the SwiftUI app does about it — and `cargo xtask parity --ios-gate S2` is the cut-over
gate: every row done or out of scope with a reason.
