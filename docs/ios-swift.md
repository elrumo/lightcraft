# The SwiftUI app (iPhone, iPad and Mac)

**Status (2026-10-14): S1 and S2 done on iOS 26.5 simulators (the phone and iPad layouts, every Edit group and tool,
export, sync, People, Map, the menu bar, multiwindow); SW3 (a native iOS 26 look: system bars, an inspector, Photos-style
dials) done on simulators; not yet tried on a device. The same code now builds for macOS (SM0, see [macOS](#macos)): a
native AppKit-backed app, not Catalyst, with the iPad layout until SM2 gives it a Mac window (its editor's panel already has).** Row by row: `apps/lightcraft-ios-swift/PARITY.md`. A native SwiftUI app for iPhone and iPad
(iOS 26 and later, macOS 26 and later) that runs the whole Rust core: the engine, the develop pipeline, the catalog, sync, export. It is to
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
| `lc_image(token)` | an `image` event's pixels (RGBA8, sRGB, straight alpha); for an `hdr` token, RGBA16F (half floats, `stride` = 8 × width) |
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
| `engine.execute` | `{command, params}` | any engine command, and `app.export` (Export Now: on a worker with an activity row, ✕ stops it; answered when it ends, `cancelled` when stopped; `remember: false` keeps it from being what Export with Previous repeats: a photo dragged into another app); `library.inspectLightroom` / `library.importLightroom {path, updateExisting?}` likewise (`apphost::lightroom`, `path` the `.lrcat` or a folder holding it; `{status: true, cancel?}` answers at once); `enhance.superRes` runs the same way (Enlarging; answered with the new photo, or error `cancelled`) |
| `import.scan` | `{paths}` | the files and folders for the import review, scanned on a worker (answered when done: `{candidates}`, each with `inTrash`) |
| `import.start` | `{paths \| indexes, …library.import params, albumName?}` | imports in the background in batches (one undo step; answered when done with what it added) |
| `engine.commands` | | every command with whether it can run now and why not |
| `host.state` | | the state below |
| `host.uiCommands` | | egui's UI-only commands (`apphost::ui_commands`): `[{id, label, shortcut, menu}]`, for the iPad menu bar and keys |
| `host.sidebar` | `{layout?, command?, params?}` | the albums sidebar's sections after one of egui's `view.sidebarSection` / `view.sidebarMoveSection` / `view.sidebarReset` (`apphost::sidebar`) on `layout` (the app keeps it): `{layout: [{id, hidden, collapsed}], shown}` |
| `host.lifecycle` | `{event}` | `resignActive`, `background` (save, pause the GPU, free memory), `active`, `memoryWarning`, `terminate` |
| `host.merge.preview` | `{command: merge.hdr \| merge.panorama \| merge.hdrPanorama, ids?, options?}` | Photo Merge's preview on a worker (`apphost::merge`, shared with egui's dialog): the picture in slot `merge`, then the answer `{info, slot}`; a newer preview answers the older `cancelled` |
| `host.merge.start` / `host.merge.last` | `{command, ids?, options?}` | the full merge in the background (activity row, ✕ stops it); answered `{started}` at once, a message when it ends; `last` uses the options last merged with |
| `host.merge.cancel` | | drops the preview being drawn (the dialog closed) |
| `host.exportPrevious` | | Export with Previous: the last export's options for the photos chosen now, on a worker like `app.export` |
| `host.admin.signIn` / `.call` / `.signOut` / `.session` | `{server, user, password}` / `{method, path: [segment…], body?, server?}` / / | the sync server's admin API (`/api/admin/*`, what its web console does) for Settings ▸ Server Administration (`apphost::host::admin`): the admin session (not the Sync device's: the server keeps them apart) lives in the host, in memory; requests run on a worker thread and are answered when the server has; path segments are percent-encoded there; without a session only `state` and `setup` work (against `server`); a `401` on a signed-in call ends the session (error `unauthorized`) |
| `render.thumb` | `{photo, edge, priority?}` | a grid thumbnail (embedded raw previews first; cheap to repeat). Ask once, when the cell appears: while `render.retain` lists the photo and it has no picture, the host asks again every tick (a synced photo's small preview downloads only while asked for, and its thumbnail renders once it has landed) |
| `render.view` | `{photo?, width, height, draft?, before?, applyCrop?, overlay?: {kind: mask, id, view?, color?, opacity?} \| {kind: allMasks, view?, color?, opacity?} \| {kind: spots, threshold?} \| {kind: pointColor, index} \| {kind: hdr}, proof?: {space, destWarning?, displayWarning?}, zoom?: {drawn, visible: [x0, y0, x1, y1]}}` | the loupe's picture (`main`; a stand-in in `preview` until it is there), with a diagnostic overlay (the selected mask, Visualize Spots…); zoomed in (`drawn`: the photo's long edge as drawn, px; `visible`: the part on screen, as fractions) also the window on screen at the zoom scale (`region`, its image event's `window: {full: [w, h], x, y, w, h}` says where it goes; planned by `apphost::region`) |
| `render.hdrDisplay` | `{on}` | the screen can (or can no longer) show the range above SDR white (`UIScreen.potentialEDRHeadroom > 1`; the app tells at start and when it becomes active; `LIGHTCRAFT_HDR_DISPLAY=1\|0` forces it for the simulator): an HDR edit's `main` and `region` pictures then render in HDR (CPU) and their `image` event carries `hdr: {token, width, height, peak}`, a lease of RGBA16F pixels (Display P3 encoded with the sRGB curve past 1, [hdr-display](hdr-display.md)); the 8-bit picture is then Display P3 too. Without it, or off, the SDR rendition as before |
| `render.prefetch` | `{photos, width, height}` | prepares the neighbours |
| `render.variant` | `{photo?, preset \| profile \| version \| edited: true, amount?, edge?}` | a preview of the photo with a preset, profile or version, or as it is (Compare, Survey, the filmstrip) (slot `variant:<key>`) |
| `render.face` | `{photo?, rect, edge?}` | a face, as People shows it (`rect` as `people.list` and `faces.person` give it; slot `variant:<key>`) |
| `render.candidate` | `{index, edge?}` | an import candidate's thumbnail (slot `import:<index>`) |
| `render.cancel` | `{slots}` | stops rendering for slots |
| `render.retain` | `{thumbs, max?}` | the thumbnails on screen (queued renders of others are dropped) |
| `ui.reply` | `{id, result \| error}` | the app's answer to a `uiRequest` |

Events: `reply`; `image {slot, photo, key, token, width, height, final, draft, quick?, ms, histogram?}` (slots
`thumb:<id>`, `thumbQuick:<id>`, `main`, `preview`, `before`…); `renderFailed {slot, photo, key, error}`; `state {state:
{catalogRevision, visibleGen, visibleCount, libraryGeneration, presetsGeneration, active, selectionCount, undo, redo,
interaction, activeMask, activeSpot, unsaved, sync?, faces?}}` when any of it changes (`presetsGeneration`: changes
with the preset list, including presets the sync brings from another device: the Presets panel reads `presets.list`
again) (`faces`: the face scan's `{pendingPhotos, indexedFaces}` while recognition is on) (`activeMask` / `activeSpot`: the Masking tool's
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
cut-over, so installed libraries carry over). *Not yet tried.* xtool builds iOS apps only; the Mac app is built by Xcode.

**With Xcode, on a Mac:** `open apps/lightcraft-ios-swift/xcode/LightCraft.xcodeproj`, pick a simulator, your
device or My Mac and Run. A pre-build phase (`xcode/build-rust.sh`) builds the Rust library for the SDK being built (debug for
Debug, `--release` for Release). The app target is one multiplatform target (iOS, iPadOS and macOS): it compiles
`LightCraft/Sources/LightCraftApp` as a synchronized folder
(new Swift files there need no project edit) over the package's `LightCraftCore` and `LightCraftImaging`, and takes
its Info.plist keys from `LightCraft/Info.plist`, the file xtool uses. For a device, choose your team under Signing &
Capabilities (or pass `DEVELOPMENT_TEAM=<team>` to `xcodebuild`).

**The Mac app:** `apps/lightcraft-ios-swift/run.sh --mac` builds it with `xcodebuild` and opens it (`--debug` for a
Debug build, `--build-only` to stop after the build); `LIGHTCRAFT_LIBRARY=<folder>` or `LIGHTCRAFT_DEMO=1` in the
environment runs the binary directly on that library (or on the in-memory demo library). Without either it opens
the default library, `~/Pictures/LightCraft Library`, which the egui desktop app also uses and locks: whichever opens it
first has it, and the other says so. The Rust library for each platform goes to its own folder, `LightCraft/.rust/ios/`
or `LightCraft/.rust/macos/`, so a Mac build never links an iOS library (a Linux `cargo xtask swift-test` still uses
`.rust/`). `ci/mac-tour.sh <LightCraft.app> <out-dir>` is the Mac's `ci/sim-tour.sh`.

**Tests:** `apps/lightcraft-ios-swift/run.sh --test` (macOS) builds the library for the Mac and runs `swift test`:
`ProtocolTests` (every event and request shape) and `EngineTests` (the real engine: commands, a thumbnail's pixels, a
drag as one undo step, 200 concurrent requests, a closed engine). The Rust side's tests (`cargo test -p lightcraft-apphost
-p lightcraft-ios-swift`) run everywhere.

The Swift package needs Swift 6.2 (Xcode 26) and links the system frameworks only; `LightCraftApp` compiles for iOS and
macOS (the few places where the two differ are in `App/Platform.swift` or small `#if os(iOS)` islands), so `swift test`
on a Mac builds the whole app too. On iOS the Rust library's log lines reach the system log through
`lightcraft_host_log` (an `@_cdecl` function in `HostLog.swift`), as the egui app's Objective-C host does it; on a Mac they
go to stderr like the egui app's (`LIGHTCRAFT_LOG=info`).

## Driving the app

`LIGHTCRAFT_SCRIPT=<file>` (a file in the app's tmp folder) runs the control-protocol requests in it, one per line
(`{"sleep": ms}` waits), and appends each reply to `<file>.out`; a relative `path` is next to the script. The engine
answers `engine.*`, `render.*` and `host.*` itself; the app answers `ui.screenshot {path}` (the window as a PNG),
`ui.inspect` (screen, photo, state, `perf`: the last render's ms and the slider-to-draft round trip; `activity`; on a Mac `windows`, `active`,
and with `menus: true` the main menu) and `ui.set {screen:
grid | photo | edit | crop | masking | remove | redEye | presets | info | commands | people | map, photo?, person? (with people: their page), span?, group?, editStyle? (automatic | dials | sliders), sheet?,
section?, select?, faceBoxes?, tagFace?, beforeAfter?, visualizeSpots?, removeAI?, histogram?, cropGuide?, cropDraw? (straighten | guides | none), orientation? (landscape | portrait), softProof?, proofSpace?, proofWarning?,
sidebarCommand? + sidebarParams? (a `view.sidebar*` command), searching?, search? (the grid's search field), painting? (the Keyword Painter's keyword; "" stops), secondWindow? (opens Window ▸ Second Window)}` (`ScriptHost.swift` lists
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

**On a Mac** the script is any file (`LIGHTCRAFT_SCRIPT=/absolute/path/tour.jsonl`; replies in `tour.out` beside it) and
`ci/mac-tour.sh <LightCraft.app> <out-dir>` does the rest on the demo library. Mac-only requests: `ui.screenshot {path,
window?: main | settings | second | cover | part of a title, views?: true}` is the window as the window server draws it
(an app may capture its own windows without Screen Recording permission; `views: true`, or when that gives nothing, draws
it from its views, which comes out flat: glass light, lists and thumbnails sometimes empty); `ui.set {windowSize: [w, h]}`
(points); `ui.activate` (comes to the front: macOS often refuses it for a process started from a script, and the menu bar
is the active app's); `ui.quit` (Quit as the menu does it, a moment after its answer: from a run-loop timer, because a
quit that waits for the main actor cannot be called from inside a main-actor task); `ui.inspect {menus: true}`.

## Look and feel

The app should look and work like an app Apple made for iOS 26, while keeping the Lightroom layout: the tools stay
under the photo, it has no separate view and edit modes, and the library is one grid with an album picker (SW3,
2026-10-14).

- **System bars before overlays.**
  - Titles, menus and buttons go in `.toolbar`: the navigation bar, the bottom bar, `ToolbarSpacer` groups and
    `.toolbarTitleMenu`.
  - Search is `.searchable` with `DefaultToolbarItem(kind: .search)` in a phone's bottom bar.
  - A row that belongs under the title (the filter chips) is a `.safeAreaBar`.
  - Draw a bar of your own only where the system has none, such as the mask column over the photo.
- **Glass only where the system floats things over content**: toasts, HUDs, labels over the photo, chips. Panels with
  text are opaque cards. Glass over a busy edge mirrors it.
- **Text styles, not point sizes.**
  - Fixed sizes only for marks proportional to a tile (badges, mask tiles).
  - Colours are semantic, except data colours (slider tracks, mixer bands, guides).
- **Modes**:
  - A tool screen (Crop, Masking, Remove, Red Eye) has Cancel (`ButtonRole.close`) and Done (`.confirm`) in the
    navigation bar, and no back button.
  - Sheets close with the system ✕ or ✓.
- **The photo is on a dark canvas** whatever the app's appearance, as in Photos' editor and Lightroom. A sheet opened
  from the editor is dark too.
- **Wide screens**: on regular width (an iPad) the tools are in the system `.inspector`. Its content is compact width,
  so the `toolColumn` environment value says a view is in a column.
- **First launch**: a four-page welcome (`Onboarding/OnboardingView.swift`, its pictures in `OnboardingHeroes.swift`)
  covers the screen once per device (`Prefs.onboarded`; not under `LIGHTCRAFT_SCRIPT` or `LIGHTCRAFT_DEMO`, so the
  tour's screenshots are clear). It asks for no permission: those are asked where they are needed. Settings ▸ Show
  Welcome opens it again: the sheet closes first and `RootView` presents the cover from the sheet's `onDismiss`,
  because a cover can't come up while a sheet is going down (`AppModel.welcomeAsked`).
  - It is drawn in the app icon's three colours (ink, paper, amber: `assets/app-icon/README.md`) on an ink canvas
    whatever the appearance, like the photo's. The type is the system's sans, in Dynamic Type text styles.
  - The first page shows the lynx from `Resources/Lynx.xcassets` (a copy of the 512 px app icon, an Xcode-target
    resource like the string tables; an xtool build has no catalog and shows a drawn mark). The other pictures are
    painted in code. A mesh gradient follows the swipe, the pictures play when their page arrives, and Reduce Motion
    stops them.
  - A helper in a `View` must not be named like a `View` modifier: `tag("Before")` once resolved to `View.tag`, which
    embedded the whole view in itself and ran the app out of memory.
- **Adjusting** (Settings ▸ Interface ▸ Adjustments):
  - *Dials* is Photos' way: a row of dials, the one in the middle set with a ruler (`Photo/AdjustDials.swift`).
  - *Sliders* is Lightroom's.
  - *Automatic* means dials under the photo and sliders beside it. Both styles are drawn from the same engine control
    specs and edit through the same calls.

## macOS

The same app builds for macOS 26 and later as a native, AppKit-backed SwiftUI app (not Mac Catalyst, not "Designed for
iPad"): one multiplatform Xcode target, arm64 only. Decisions (2026-10-14):
- **Not sandboxed, hardened runtime**, like the egui Mac app (`packaging/macos/entitlements.plist`: only the Apple
  Events entitlement for Export ▸ Add to Apple Photos). Originals are referenced in place (`ImportMode::Add`), so there
  are no security-scoped bookmarks; the Mac App Store would need them (the engine stores absolute paths).
- **A Mac starts the engine by the `desktop` profile** (`OpenConfig.profile`, `apphost::startup::Profile`; the Swift app
  sends it on macOS only): the library in `~/Pictures/LightCraft Library`, the cache budget from the machine's RAM, AI
  models in the settings folder shared with egui and the CLI, and a library that won't open (the egui app has it open:
  it holds an OS lock) is an `openFailed` error with the reason, never the in-memory demo library a phone falls back to.
  The app's tmp folder is its own, `~/Library/Caches/<bundle id>/tmp`: the engine empties `Import` and `Exports` in it at
  every launch, which in a Mac's shared temp folder would not be its own.
- **One main window** (and the Second Window): the model is the app's one (navigation, and the engine's one selection
  and active photo), so a second main window could only mirror the first.
- **Mac-native layout**, not the iPad's push structure with Mac chrome: a persistent sidebar, content and inspector, a
  toolbar, a filmstrip, sliders only. The editor's right-hand side is built (SM2.1) and follows Lightroom's *layout*
  (not its look): the open tool's panel and, along the window's edge, a column of tool icons (`Mac/MacInspector.swift`).
  The Edit tool is a header (Auto, B&W, HDR), the profile, then the groups stacked and collapsible (Light, Color,
  Effects, Detail, Optics, Calibration), with Curve, Color Mixer (it holds the point colours) and Color Grading as boxes inside them
  (`Mac/MacEditPanel.swift`); every control is one row, name | track | value you can type over (double-click resets,
  arrow keys nudge; `Mac/MacSliderRow.swift`); there are no group chips and no dials. The panel is given the column's width and
  never widens it (`Mac/MacSegmented.swift` is the segmented control that shrinks to fit); the inspector is shown or
  hidden from the toolbar. Under the loupe a bar (SM2.2, `Mac/MacPhotoBar.swift`) has the view modes (Grid, Loupe, Compare,
  Survey), the rating and flag, Copy Edit Settings with a gear for the rest, the zoom (Fit, Fill, 100 %, 200 %, 400 %: an
  `editor.zoomAsk` the loupe answers, and `zoomPercent` it reports) and Before / After; a narrow window drops the view modes,
  then the words. It is drawn for the width it has, not with `ViewThatFits`: a window whose AppKit menus and buttons are
  built in several variants to measure them can fail to settle its constraints, and AppKit then aborts the app. The rest of
  SM2 is not built yet. Crop & Rotate (`Mac/MacCropPanel.swift`: Aspect and its lock, an Angle slider, buttons for
  straighten, turn, flip and reset, the overlay, Expand; Geometry as a group of its own) and Masking (`Mac/MacMaskingPanel.swift`:
  the masks and their parts as wrapping tiles at the top, the brush, then Mask, Light, Color, Effects, Detail and Optics stacked
  for the selected mask) are laid out the same way (SM2.3); Remove and Red Eye were already one column of controls. A Mac's
  sections remember whether they are open (`Prefs.sectionStates`), each panel saying how a new one starts.
  The grid (SM2.4, `Mac/MacGridCell.swift`) has no select mode: a click chooses a photo (⌘ adds or takes it out, ⇧ chooses
  the run from the last one: `library.select` modes), a double click opens it, a right click has its menu, and it can be
  dragged out with the others chosen; chosen cells have a ring. Its bar has Years, Months and All, the chosen photos'
  rating and flag, how many are chosen and a thumbnail-size slider (`Prefs.thumbSize`; a pinch changes it too, and the
  window gives as many columns as fit). Still to do: the keyboard layer (rating digits, flags, arrows in the grid), the
  loupe's scroll and pinch, and the keys of SM2.5 and SM2.6.

Where it stands: it builds and launches with the iPad's regular-width layout in a window (SM0), and has a Mac's shell
(SM1): a Settings window (⌘,: a tab per page of the iPhone's list, `Mac/MacSettings.swift`), a menu bar with the app
menu (About, Settings…), a File menu that starts with Import Photos… (⇧⌘I, an open panel for files and folders, copied
in and reviewed as the iPhone's picks are), the engine's Edit, View and Photo menus, a Tools menu for what egui
lists under Window (the Window menu is the system's), Undo and Redo that give ⌘Z to a text field being edited, and F-keys
in shortcuts; quit that asks when a task is running, then saves, closes the library (so the egui app can open it) and
stops the engine (`Mac/MacAppDelegate.swift`); a library that won't open (the egui app has it) shows the reason with Try
Again, Choose Another Library… and Continue Without Saving (`App/StartErrorView.swift`); the slideshow and Lights Out in
a full-screen window of their own. Since the merge with the iPhone's later work (2026-10-15) the Settings window also has a
Server tab (Server Administration, `Admin/`, written once for both platforms through the `Platform.swift` shims), while the
first-launch welcome and the phone's two-finger gesture on the photo (`TwoFingerGesture`) stay iOS only. Not yet: the Mac
input of SM2 (keys, the loupe's scroll and pinch), import in place, an export destination and libraries (SM3).

Two things a Mac's SwiftUI menu bar does that cost a day: it does not follow what an `@Observable` model changes, neither
in a `Commands` body nor in the App's, only an `ObservableObject` the App observes (`MenuChanges`, bumped by the model);
and a top-level menu that is empty when the app starts is never made (File starts with Import Photos…). Menus are also
filled in for the focused scene only: an automation that starts in the background sees only their static items.

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
(SW1.19–25), landscape and iPad portrait
(SW1.26) and the phone audit (`cargo xtask parity --ios-gate S1`, SW1.27).

S2 (`cargo xtask parity --ios-gate S2` passes since 2026-10-14): the iPad split view and library dialogs (SW2.1) and
their words (SW2.2); Compare, Survey, Reference and the other view modes (SW2.3); the menu bar and keys (SW2.4); culling,
track logs, missing files, Export with Previous (SW2.5); soft proofing (SW2.6); sidebar sections (SW2.7, the arrangement →
`apphost::sidebar`); the grid's search (SW2.8); the Keyword Painter (SW2.9, `keyword.toggle`); drag and drop and Apple
Pencil (SW2.10); the Second Window, more windows and the external display (SW2.11); Lightroom catalog import (SW2.12,
the runner → `apphost::lightroom`). Not tried on hardware yet: a real drag, a Pencil, an external display. S3, removing
the egui iOS app, is the maintainer's call.

SW3, a native iOS 26 redesign (not the S3 gate; see *Look and feel*):
- the library's system bars, title menu, bottom bar with search, and select bar (SW3.1);
- the albums as a native sidebar with disclosure groups (SW3.2);
- the photo screen's dark canvas, date title, bottom tool bar, and Cancel/Done in the navigation bar (SW3.3);
- an iPad's tools in the inspector (SW3.4);
- glass group chips, the panel as a card, and disclosure groups (SW3.5);
- the Dials style (SW3.6);
- the sheets' ✓ (SW3.7);
- this page, the parity notes, the words and the CI tour (SW3.8).

SM, the Mac app (see [macOS](#macos); not part of the S gates, which are about replacing the egui iOS app):
- SM0, it builds and launches: the engine host's `desktop` profile (SM0.1, done), the app compiling for macOS (SM0.2,
  done), the Xcode project, Rust build, `run.sh --mac` and `ci/mac-tour.sh` (SM0.3);
- SM1, the Mac shell (done on this Mac): quit that closes the engine and the start-up error (SM1.1), the Settings window,
  the menus and the slideshow window (SM1.2);
- SM2, the Mac window: sidebar, content and inspector, a toolbar and filmstrip, pointer and keyboard (SM2.1, the editor's inspector and the Edit panel, SM2.2, the bar under the loupe, and SM2.3, the Crop and Masking panels, and SM2.4, the grid, done; the keyboard layer and the loupe's input to come);
- SM3, desktop workflows: import in place from the open panel and Finder, an export destination, libraries;
- SM4, tracking and CI: the Mac rows in `PARITY.md`, `ios-swift.yml`, the other docs.

Open questions (not blocking S0): MapKit or the built-in offline map; which AI engine features ship on iOS (binary size);
whether the gate also compares screenshots of the two apps.

## Languages

The app speaks the egui app's languages through iOS's own localisation: each language's
`Sources/LightCraftApp/Localization/<lang>.lproj/Localizable.strings`, generated by `cargo xtask swift-strings` from the
egui catalogs (`crates/ui-egui/locales`) and the app's own (`apps/lightcraft-ios-swift/locales/<code>.json`, for its
wording that egui doesn't have), so a translation is written once. `locales/keys.json` lists the keys the app uses; after
adding text to the app, build it in Xcode with `SWIFT_EMIT_LOC_STRINGS=YES` and run `cargo xtask swift-strings
<derived-data> --missing` to refresh it and list what needs translating. Labels from the engine (controls, commands,
rules) are looked up at run time (`LocalizedStringKey(label)`), and every egui message is in the tables for that.
Dates are formatted by iOS. The language is iOS's per-app setting (Settings ▸ LightCraft ▸ Language), which Settings ▸
Interface ▸ Language opens. The tables are the Xcode target's resources; an xtool build is English only.

## Parity gate

`apps/lightcraft-ios-swift/PARITY.md` has a row for every UI-only command of the egui app (`ui:<id>`, from
`crates/ui-egui/src/menus.rs`), every dialog (`dialog:<Variant>`, from `crates/ui-egui/src/state.rs`) and every phone and
iPad screen (`screen:<name>`), each with its phase, status and evidence. Engine commands need no rows: the All Commands
screen shows and runs every one. `cargo xtask parity` (in `ci`) checks the manifest — a new egui UI command or dialog
fails it until its row says what the SwiftUI app does about it — and `cargo xtask parity --ios-gate S2` is the cut-over
gate: every row done or out of scope with a reason.
