# iOS (iPhone and iPad)

What is missing, with priorities: [`ios-gaps.md`](ios-gaps.md).

**Status: written and type-checked; not yet run from this tree on a phone.** The egui app builds as a static library
(`apps/lightcraft-ios`, `liblightcraft_ios.a`) that an Xcode project (`apps/lightcraft-ios/xcode`) wraps into an
app, with a small Objective-C host (`apps/lightcraft-ios/host`). On a screen narrower than 900 pt the app draws
a touch layout (below); wider screens (an iPad held sideways) get the desktop one. The pieces were first written and
run in another tree (an iPhone and the simulator, iOS 27); this branch re-applies them on top of this one, and what
is verified here is what the next section lists. Run it on a simulator or a device before relying on it.

## What is verified

- `cargo xtask ios` runs `cargo check --target aarch64-apple-ios` for every crate that reaches the app (geometry,
  colour, raster, raw, codecs, meta, develop, pipeline, GPU, catalog, engine, `ui-egui`, MCP, the glue and the app).
  It needs only the Rust target (`rustup target add aarch64-apple-ios`), so it runs on Linux, and it is part of
  `cargo xtask ci` where the target is installed. It proves the dependency tree has no desktop-only crate on the iOS
  path (wgpu builds with its Metal backend). It does **not** prove that an app links or runs: that needs Xcode.
- The Objective-C calls of `crates/ios-host` are checked against the `objc2` bindings' signatures only; selectors
  and framework behaviour are verified only by running them.
- The touch layout is tested headless at phone sizes (`ui-egui`'s `tests/compact*.rs`, `tests/glass.rs`,
  `tests/grid_touch.rs`), driven through the same control channel as everything else, and
  `lightcraft-cli snapshot --demo --touch --size 390x844 --scale 2 --script …` draws it without a window.
- The host's own logic (lifecycle, picks reaching the import review, the keyboard's Return, the staging folders)
  has unit tests in `apps/lightcraft-ios`.

## Build and run (Xcode, on a Mac)

`xcode/project.yml` is an [XcodeGen](https://github.com/yonaskolb/XcodeGen) spec: `cd apps/lightcraft-ios/xcode &&
xcodegen && open LightCraft.xcodeproj`. A pre-build phase (`build-rust.sh`) runs `cargo build -p lightcraft-ios
--target <triple>` for the SDK being built (debug or release by configuration) and copies the library next to the
app. Set `DEVELOPMENT_TEAM` (and change the bundle id, `io.github.storytold.lightcraft.ios`, to one of yours) to run
on a device; the simulator needs the Apple-silicon target (`aarch64-apple-ios-sim`). From the command line:

```sh
cd apps/lightcraft-ios/xcode && xcodegen
xcodebuild -project LightCraft.xcodeproj -target LightCraft -sdk iphoneos -configuration Release \
  -allowProvisioningUpdates DEVELOPMENT_TEAM=<team id> SYMROOT=$PWD/build build
xcrun devicectl device install app --device <udid> build/Release-iphoneos/LightCraft.app
xcrun devicectl device process launch --device <udid> --terminate-existing io.github.storytold.lightcraft.ios
```

**The host** (`apps/lightcraft-ios/host`): `LightCraftMain.m` is the entry point (the Rust crate owns the
`UIApplication`: winit calls `UIApplicationMain` from `lightcraft_ios_main`) and passes the device's name on for sync;
`SceneDelegate.m` declares scene adoption (iOS 27 traps apps without the scene lifecycle, and winit has none, so the
host swizzles `-[UIWindow initWithFrame:]` to create the window in the connected scene) and reports the on-screen
keyboard's Return, which winit hands over as a `"\n"` that egui drops. The crate's `wgpu_options()` clamps wgpu's
default limits to what the Metal adapter has (the simulator allows 15 inter-stage shader variables, the default wants 16).

**Logs.** Every log record goes to the system log through `NSLog` (Console.app on a Mac, `idevicesyslog | grep
LightCraft` elsewhere); a panic is also written to `lightcraft-panics.log` in the app's tmp folder
(`xcrun devicectl device copy from … --domain-type appDataContainer --domain-identifier <bundle id> --source tmp`).

**Driving the app without touching it.** Launched with `LIGHTCRAFT_SCRIPT=<file>`, the app runs the
control-protocol requests in that file, one JSON object per line (`{"method": "ui.screenshot", "params": {"path":
"…"}}`, see [control-protocol.md](control-protocol.md); `{"sleep": 500}` waits), and appends each reply to the same
path with the extension `.out`; a relative path (the script's, or a screenshot's) is in the app's tmp folder. The file
stays in the app's sandbox, so nothing listens on the network.

```sh
xcrun devicectl device copy to --device <udid> --domain-type appDataContainer \
  --domain-identifier <bundle id> --source tour.jsonl --destination tmp/tour.jsonl
xcrun devicectl device process launch --device <udid> --terminate-existing \
  --environment-variables '{"LIGHTCRAFT_SCRIPT": "tour.jsonl"}' <bundle id>
```

On the simulator, `SIMCTL_CHILD_LIGHTCRAFT_SCRIPT=… xcrun simctl launch …` does the same.

**The library** is `LightCraft Library` in the app's Documents folder (kept across launches and updates, backed up
with the device); a new one starts with the demo photos. `$LIGHTCRAFT_LIBRARY` puts it elsewhere. If it can't be
opened the app falls back to the in-memory demo library (nothing saved, no sync) and logs why. The app's settings
(`ui.json`) are in `Library/Application Support/LightCraft`.

**Light and dark.** View ▸ Appearance (Auto, Dark, Light) chooses how the layouts are coloured; Auto follows the
device (the host reads UIKit's style, as winit ignores the theme on iOS, and forces the same style on its windows so
the status bar, keyboard and system sheets match).

**Sync** is the desktop's ([sync.md](sync.md)): Settings ▸ Sync (or the cloud button), the server address, user name
and password; the library then fills from the server, previews first, originals on request. The device signs in
under the name `LightCraftMain.m` reads from `UIDevice` ("iPhone", "iPad"; `$LIGHTCRAFT_DEVICE`), which the server's
admin page lists. Requests use the same pure-Rust transport as the desktop app (rustls, Mozilla roots), so App
Transport Security doesn't apply; `NSLocalNetworkUsageDescription` is set because a server on the home network needs
the local-network permission.

## The native host (`crates/ios-host`)

`lightcraft-ios-host` holds the app's Objective-C calls through `objc2` (Rust bindings to the system frameworks; no C or
Objective-C is compiled). It is allowed `unsafe` like `crates/sysmem` (`CLAUDE.md`, *Never crash*): every block says
why it is sound and failures come back as errors. On other platforms it is an empty shell whose portable helpers (safe
file names, unique paths, the folder walk) are tested on Linux.

- **Import.** File ▸ Import from Photos… / from Files… / Folder from Files…, and the round + button over the phone
  grid (`Services::host_pick`). The Photos picker (PHPicker; no permission needed) hands over each photo as stored (a
  ProRAW DNG when there is one, else the HEIC or JPEG); the Files picker hands over copies; a folder is read in place
  under its security scope (at most 5000 photos, 12 levels deep). Everything is copied into `tmp/Import/` and opens the
  import review, which *moves* it into the library's `Originals/` (`file.addPhotos {staged: true}`: never "add in
  place", so the system emptying that folder loses nothing).
- **Presets.** File ▸ Import Profiles & Presets… and the Presets panel's ⋯ ▸ Import Presets… open the Files picker
  for preset files (`PickKind::Documents`, a type per extension); `file.importPresets` reads the copies where they land.
- **Export.** Every export is written to `tmp/Exports/` (emptied first) and opens the share sheet: Save Image (to
  Photos), Save to Files, AirDrop, Mail, other apps; the export dialog has no folder field
  (`Services::share_exports`). **Save to Photos** (the share sheet's row, the photo's save button;
  `Services::save_to_photos`) skips the share sheet and adds the exported files to the library through PhotoKit with
  *add-only* access (the system asks the first time, `NSPhotoLibraryAddUsageDescription`), all or none; JPEG, PNG,
  TIFF and DNG only.
- **HEIC / HEIF, AVIF** decode through ImageIO, installed as `lightcraft-codecs`' system decoder (8- or 16-bit, in the
  photo's own colour space with its ICC profile); their EXIF and XMP come from `lightcraft-meta`'s HEIF parser.
- **Haptics and the clipboard.** `Services::haptic` plays the Taptic Engine for touch gestures and choices;
  `Services::clipboard_text` is Paste in a text field's touch edit menu.
- **Lifecycle.** The library, the view and the settings are saved when the app resigns active, goes to the background
  or terminates. In the background the GPU is paused (`gpu::pause`: renders go to the CPU, and errors meanwhile don't
  turn the GPU off for the rest of the run) and decoded photos are released; an export in progress asks for
  background time. A tap on the status bar scrolls the grid to its top (`view.scrollToTop`).
- **Memory.** iOS ends an app by its footprint. The cache budget is a third of what the app may allocate at launch
  (`os_proc_available_memory`), between 256 MiB and 1 GiB; a memory warning releases decoded photos; the global
  allocator is `lightcraft_sysmem::ReturningAlloc`, so freed image buffers leave the footprint at once.
- **Paths.** A library opened from another folder than last time (iOS gives the container a new path on every app
  update) re-points the photos stored under its old folder (`location.json`).

## The touch layout (`ui-egui`)

Opt-in: the host sets `Services::touch`, and a window narrower than `COMPACT_BELOW_PT` (900) gets `app.compact`.
Desktop hosts and wide windows never see it; every touch branch is behind `crate::is_compact(ctx)`. The shell is
`panels/compact.rs` (grid, tab bar, the tool sheet, pages) with `panels/mobile.rs` (pull-down menus, action sheets,
pages, the text edit menu), `glass.rs` (springs and the translucent look) and `theme::apply_touch` (44 pt rows over
whichever theme the user chose). The tools have screens of their own: `crop_mobile.rs`, `remove_mobile.rs`,
`mask_mobile.rs`; the others are the desktop panels in the sheet, with touch sliders, switches and bigger handles.
Dialogs are pages (`dialogs::show`), a yes-or-no question is an alert (`panels/alert.rs`). Anything that two frontends
would need goes below the UIs (`CLAUDE.md`, *Shared frontend logic*): `library.chooseRun`, `Catalog::date_blocks`.

## Scope

iPhone and iPad, a library on the device (import, edit and export there), optionally shared with the user's other
devices through their own LightCraft server ([sync.md](sync.md)), with a layout and behaviour modelled on Lightroom's
mobile app. As everywhere in this repository that means imitating layout and behaviour, never copying Adobe's assets
(`CLAUDE.md`, *Assets*); the touch layout's drawing and words are our own.
