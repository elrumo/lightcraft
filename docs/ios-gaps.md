# iOS: what is missing

What the iPhone / iPad build of LightCraft still lacks, in two parts: **A.** things we know are unfinished in what
exists (the compact layout and the iOS host), and **B.** what Lightroom's mobile app does that LightCraft does not
yet. Status of the app itself: [`ios.md`](ios.md). Desktop feature parity has its own tracker:
[`parity.md`](parity.md); rows there that are ⬜ also apply on the phone and are not repeated here.

Feature names are our own words (no Adobe text, screenshots or assets; see `CLAUDE.md`, *Assets*). Part B comes from
the mobile app's public behaviour and should be re-checked against the installed app (read-only, `plan/lightroom/`)
before anything is built from it.

**Priority:** **P0** = the app is not usable on a phone without it · **P1** = expected by anyone coming from Lightroom
mobile · **P2** = later, niche or needs large new work (models, services). **Needs** says what blocks it.

**Status** (start of a row): ✅ done and tested · 🟡 built, but not yet run on the simulator or a device (the native
code is type-checked and clippy-clean for `aarch64-apple-ios`; its logic is tested on Linux), or partly done · no
mark: not started.

## A. Known gaps in what exists

### A1. Platform host (nothing here works yet; the Menu → Import / Export commands have nothing to open)

| # | Gap | Pri | Needs |
|---|---|---|---|
| A1.1 | 🟡 Photos library picker: `lightcraft-ios-host` shows PHPicker (File ▸ Import from Photos…, the grid's + button), loads each photo as stored (a ProRAW DNG when there is one, else the HEIC / JPEG), copies it into `tmp/Import/` and the import review moves it into the library. PHPicker runs out of process and needs no photo-library permission, so there is no limited-access case to handle. Checked on the simulator (iOS 27.0): HEIC and JPEG picks, a duplicate, cancelling | P0 | run on a device; a ProRAW and a Live Photo |
| A1.2 | 🟡 Files picker: image files (the picker's own copies) and whole folders (read in place under their security scope, copied off the main thread; at most 5000 photos, 12 levels). Everything is copied into the library, so no security-scoped bookmarks are kept. Checked on the simulator: a file, a folder with a subfolder and a non-photo file, cancelling | P0 | run on a device; iCloud files that aren't downloaded yet; a USB drive |
| A1.3 | 🟡 Export and share: every export goes to a staging folder and opens the share sheet (Save Image to Photos, Save to Files, AirDrop, Mail, other apps; anchored as a popover on iPad); the export dialog has no folder field (`ShareExports`); `NSPhotoLibraryAddUsageDescription` is set for Save Image. Checked on the simulator: the share sheet with the export, Save Image's permission prompt and the photo in Photos | P0 | run on a device; the iPad popover |
| A1.4 | ✅ Library and settings in the app sandbox: the library in `Documents/`, settings in `Library/Application Support/LightCraft`, exports written to a staging folder for the share sheet (`ShareExports`), no desktop "Local" folders; photos handed over by the pickers are *moved* into the library (`file.addPhotos {staged: true}`), never referenced in `tmp/`; a library opened from another folder than last time (iOS gives the container a new path on every app update) re-points the photos stored under its old folder (`location.json`, `library.info` → `relocated`) | P0 | tested on Linux; check on a device after an app update |
| A1.5 | 🟡 HEIC / HEIF (and AVIF) decode through ImageIO: `lightcraft_codecs::set_system_decoder` (tested on Linux with a stand-in decoder); 8- or 16-bit, in the photo's own colour space (Display P3) with its ICC profile, thumbnails through ImageIO's fast path; their EXIF and XMP (capture time, camera, location) are read by `lightcraft-meta`'s HEIF parser (pure Rust, tested). Checked on the simulator: an iPhone X HEIC decodes (thumbnail and full size) with its camera, lens, exposure and capture date | P0 | a device; ProRAW |
| A1.6 | Apple ProRAW / JPEG XL compressed DNG (rejected today) | P1 | codec work in `crates/raw` |
| A1.7 | 🟡 Memory budget for iOS: a third of what the app may allocate at launch (`os_proc_available_memory`), 256 MiB–1 GiB; memory warnings and backgrounding release decoded photos (`Session::release_memory`). Still missing: tiled full-resolution export (48 MP risk). On an iPhone 17 Pro (iOS 27.2): 3373 MB available at launch → a 1024 MB budget; backgrounding released 102 MB | P0 | tiling in `pipeline`; peak memory on a device |
| A1.8 | 🟡 App lifecycle: the library, view and app settings (`ui.json`, now kept on iOS too) are saved when the app resigns active, goes to the background or terminates; the GPU is paused in the background (`gpu::pause`: renders on the CPU, errors meanwhile don't disable it for good) and resumed after; an export in progress asks for background time. The notifications arrive on the simulator and on an iPhone 17 Pro (iOS 27.2: resign, active, background with memory released) | P0 | backgrounding during an export |
| A1.9 | 🟡 On-screen keyboard: a focused field raises it (winit's `becomeFirstResponder`), typing and backspace arrive through `UIKeyInput`, Return confirms the field (the Objective-C host hooks winit's `-insertText:` and the app presses Enter for egui), and the layout moves above the keyboard (its height is added to the safe area). Fixed on the way: dialog fields asked for focus every frame, which on iOS hid and showed the keyboard in a loop. Checked on the simulator (iPhone 18 Pro, iOS 27.0: typing, Return creating an album, layout above the keyboard); not yet on a device. Still no `UITextInput`: no autocorrect, no dictation, no Japanese / Chinese composition | P0 | a device; a winit with `UITextInput` (or a native text field bridge) for composition |
| A1.10 | Scene support without the `-[UIWindow initWithFrame:]` swizzle (winit has none) | P1 | a winit with scenes, or a native UIKit host |
| A1.11 | 🟡 Signed device build: `apps/lightcraft-ios/xtool` (xtool signs with any Apple ID and installs over USB, from Linux too; `run.sh`), not yet run (the Xcode project signs and installs on a device: A1.13); the app icon is the LightCraft lynx (`assets/app-icon/lightcraft-1024.png`). Still missing: TestFlight and App Store review (a paid account, a privacy manifest, an icon without alpha, the usage strings reviewed) | P0 | a device; a paid Apple Developer account for TestFlight |
| A1.12 | Metal limits on iPhone GPUs for the compute path (needs at least 10 storage buffers per stage); the simulator allows 15 inter-stage variables | P1 | spike on devices |
| A1.13 | 🟡 Runs on a real device: an iPhone 17 Pro (iOS 27.2), built and signed with Xcode (`apps/lightcraft-ios/xcode`, a team's wildcard profile) and installed with `devicectl` (`docs/ios.md` → *Xcode, on a Mac*); Metal works (`Apple A19 Pro GPU`), the library opens in Documents, lifecycle notifications arrive. `LIGHTCRAFT_SCRIPT` drives it without touching it. xtool (`run.sh`, Linux / WSL) not yet tried | P0 | the pickers and sheets on the device (someone at the phone); xtool |

### A2. Compact layout and touch

| # | Gap | Pri |
|---|---|---|
| A2.1 | Pinch zoom and two-finger pan are untested (the headless driver cannot inject multi-touch); the pinch focal point is only set when starting from Fit | P0 |
| A2.2 | Painting by finger with the sheet open: brush masks, the Remove tool and the red-eye tool are drawn over a loupe that is half hidden; no finger-sized brush preview offset, no loupe magnifier while painting | P1 |
| A2.3 | Tone curve editor, colour wheels (grading), HSL targeted drag, point colour: built for a mouse, no touch sizing | P1 |
| A2.4 | 🟡 Sheet size: the Edit tool shows one group at a time (A2.22), so the default 40 % holds about four sliders (rows are 60 pt); still to do: a peek height and a drag-down to collapse | P1 |
| A2.5 | 🟡 Dialogs on a phone are pages that slide up (Cancel, the title and the action in the bar, the body scrolling; A2.22); their rows stack the label above the controls, which wrap (Export's format and colour-space chips no longer run off the screen). Checked on the simulator (Export, Import, New Album). Still desktop-shaped inside: egui combo boxes and checkboxes rather than iOS pickers and switches | P1 |
| A2.6 | Landscape phone layout (today: the portrait compact layout, which leaves the loupe small) | P1 |
| A2.7 | Safe area on rotation, Dynamic Island and the home indicator are handled once at startup frames only; not re-checked on rotation / split view | P1 |
| A2.8 | ✅ Multi-select by touch: Select (top bar) or a long press on a photo starts choosing; taps add and remove (check badges), Select All / Deselect All, Cancel or Back ends it (`view.selectMode`); batch actions in A2.9. Still missing: drag across photos to choose a run (a drag scrolls the grid) | P0 |
| A2.9 | ✅ Photo actions without right-click menus: while choosing, an action bar rates, flags, labels, adds to an album, exports, copies / pastes edit settings and deletes the chosen photos; the loupe's star button rates, flags and labels the photo. Still missing: swipe gestures for them (as in Lightroom's rate-and-review mode) | P0 |
| A2.10 | 🟡 Sheets for the desktop panels: Albums (the collection title opens it as a page), Info (a tool), Keywords, Versions and History (the photo's "…" menu), Settings (the grid's "…" menu, as a page); still missing: a sheet for the filter bar's options and for sort / group beyond the Sort menu | P1 |
| A2.11 | ✅ Keywords, Versions and History are in the photo's "…" menu (they open in the tool sheet); a long press on undo opens History | P1 |
| A2.12 | Haptics (slider detents at zero, snapping, mode switches) | P2 |
| A2.13 | Gestures to undo / redo (two- and three-finger tap or swipe) and shake to undo | P1 |
| A2.14 | Press-and-hold the photo to see the original (before) while editing; a before / after split by dragging | P1 |
| A2.15 | Accessibility: VoiceOver on the custom-drawn widgets (egui's accesskit on iOS is unverified), Dynamic Type, Reduce Motion, Increase Contrast | P1 |
| A2.16 | Drag and drop between apps (drag photos in from Photos / Files, out to Mail / Messages) | P2 |
| A2.17 | iPad: keyboard shortcuts and a menu bar for hardware keyboards, pointer hover and right-click, Stage Manager / multiple windows, external display | P1 |
| A2.18 | Apple Pencil: pressure-sensitive brush, hover preview, double-tap tool switch, squeeze | P2 |
| A2.19 | The desktop layout (1024 pt and wider, iPad landscape) still has mouse-sized targets and no touch gestures beyond what egui gives | P1 |
| A2.20 | UI language switching, right-to-left layouts and the Japanese / Chinese fonts have not been checked on iOS (`CRAFT_FONTS_DIR` is not wired into `run.sh` or the Xcode build; `CRAFT_FONTS_DIR=… ./run.sh` should work, since cargo reads it) | P1 |
| A2.21 | Tests: gesture tests need a multi-touch injector in `Headless`; no snapshot tests of the compact layout (they exist only as manual `lightcraft-cli snapshot` runs) | P1 |
| A2.22 | ✅ No desktop menu bar on a phone (`panels/compact.rs`, `panels/mobile.rs`), modelled on Lightroom's mobile app: over the grid the collection is the title (▾ opens the albums list as a page), with filter and sort beside it and Select and "…" (import, new album, sort, filter bar, square thumbnails, settings, help, about) above; over a photo back, undo (a long press: history), share and "…" (rating, flag and label, copy / paste / reset edits, versions, history, keywords, before / after, delete); the tools (Presets, Crop, Edit, Masking, Remove, Info) as icons with the open one on a blue tile; the Edit tool's groups (Auto, Profile, Light, Color, Effects, Detail, Optics, Calibration) in a row that scrolls sideways, one group's sliders at a time. Menus are iOS pull-downs under their button (action sheets at the bottom for the rest), dialogs are pages, and every other menu command is in a searchable All Commands list. Tested headless (`tests_compact`) and checked on the simulator (iPhone 18 Pro, iOS 27.0) | P0 |
| A2.23 | ✅ iOS look in the compact layout (`theme::Tokens::ios`, `theme::apply_layout`): Apple's dark-mode system colours (system blue, grouped backgrounds, separators, label greys), black behind photos and bars, 16–17 pt type, borderless rounded controls, 44 pt rows; sheets and menus slide or fade in, the tool and group tiles fade between states, the + button shrinks under a finger. The desktop look is untouched. Still not native: Inter rather than San Francisco (the UI font is Inter, `CLAUDE.md`), no blur behind bars and menus, no haptics (A2.12) | P1 |
| A2.24 | ✅ Import review on a phone (`import::mobile_body`): the photos as wide as the screen, three across, a round check badge per photo that a tap toggles, Select All / Deselect All, duplicates tagged and left out, the options folded under a disclosure, Add *n* in the bar; the example destination is shown from Originals, not the sandbox path. Checked on the simulator with the Photos picker | P0 |
| A2.25 | ✅ Sliders by touch: the value follows the finger's movement (a tenth of the track is a tenth of the range) instead of jumping to it, and a tap leaves it alone, as Lightroom's mobile app and iOS sliders do (`tests_compact`). Still missing: telling a vertical scroll of the sheet from a slider drag (the sheet scrolls only from between the sliders) | P0 |

## B. Lightroom mobile features LightCraft does not have

### B1. Capture

| # | Feature | Pri | Needs |
|---|---|---|---|
| B1.1 | In-app camera: photo capture with exposure, white balance, focus and ISO / shutter controls | P2 | AVFoundation glue |
| B1.2 | RAW (DNG) capture, including ProRAW, written straight into the library | P2 | AVFoundation, DNG writer |
| B1.3 | HDR capture and in-app multi-frame HDR / panorama merge on the phone | P2 | `crates/merge` has HDR / pano rows as ⬜ on desktop too |
| B1.4 | Camera presets: apply a look live in the viewfinder | P2 | B1.1, GPU preview |
| B1.5 | Import from a camera or card reader over USB (iPad) with a thumbnail grid and selective import | P1 | ImageCaptureCore glue |
| B1.6 | Live Photos / burst handling, Portrait-mode depth data (for depth-based masks) | P2 | ImageIO |
| B1.7 | Video: import, trim, and the same colour edits on video clips | P2 | video pipeline (`ROADMAP.md`: video is out of scope for now) |

### B2. Cloud, sync and sharing

| # | Feature | Pri | Needs |
|---|---|---|---|
| B2.1 | Library sync between devices (edits, ratings, albums, presets) | P1 | wired up, not yet run on iOS: the user's own `lightcraft-server` ([`sync.md`](sync.md)), Menu → Settings → Sync; verify on a simulator and a device (keyboard entry, TLS, backgrounding) |
| B2.2 | Smart previews / originals management: keep previews on the phone and originals elsewhere, download on demand, free up space | P1 | sync has the tiers (mini / smart previews, originals on request, albums kept offline); no free-up-space or size budget yet (`sync.md`, Limits), no compact UI for offline albums |
| B2.3 | Desktop ↔ phone handoff over the local network or iCloud (open the same catalog) | P2 | B2.1 shares one library through the server; no direct device-to-device handoff |
| B2.4 | Shared albums with comments, likes and per-viewer permissions | P2 | the sync server is the place for it; not planned yet (sync v1 is one person's devices) |
| B2.5 | Web galleries (publish an album as a page) | P2 | the sync server could serve them; not planned yet |
| B2.6 | Share edits as a link or as a preset file; share a photo with a preset embedded | P1 | preset export exists (`file.exportPresets`); needs A1.3 |
| B2.7 | Share a before / after image or a time-lapse of the edit history | P2 | |
| B2.8 | 🟡 Save to Photos and send to other apps in the formats / sizes / watermark of the export dialog: exports open the share sheet (A1.3; checked on the simulator) | P0 | run on a device |
| B2.9 | Backup and restore of the catalog from the app | P1 | catalog is a log; a zip export is easy |
| B2.10 | Account, subscription and storage UI | OOS | no LightCraft service or subscription: accounts live on the user's own sync server (its admin page) |

### B3. Library and organisation

| # | Feature | Pri | Needs |
|---|---|---|---|
| B3.1 | Quick Actions: suggested next steps for a photo or an import (cull, stack, add to album, apply a look) | P2 | AI scoring exists in part (`Dialog::Cull`) |
| B3.2 | Best-photo suggestions and automatic sorting of an import into groups | P2 | models |
| B3.3 | Search by content (objects, scenes, text in photos), natural-language search | P2 | embedding model; none shipped |
| B3.4 | Map / location view and geotagging from the phone's GPS or a track | P1 | map tiles are an external service; privacy decision |
| B3.5 | Auto-tagging of people (faces exist: names and grouping) across the library, "People" suggestions | P1 | partly done (`ViewMode::People`) |
| B3.6 | Photos app integration: show the Photos library inside the app without importing, with edits written back as a Photos adjustment | P1 | PhotoKit glue |
| B3.7 | Home-screen and lock-screen widgets, Shortcuts / Siri actions, Spotlight indexing, share extension ("Edit in LightCraft") | P2 | app extensions (xtool supports them: `extensions` in `xtool.yml`) |
| B3.8 | Learn / Discover: in-app tutorials and a community feed of edits | OOS | not a goal |
| B3.9 | Presets marketplace and premium presets | OOS | |
| B3.10 | Adaptive presets (a look that adjusts to the photo's subject) | P2 | masking models |

### B4. Editing

(Desktop rows in [`parity.md`](parity.md) apply too; these are the mobile-specific ones.)

| # | Feature | Pri | Needs |
|---|---|---|---|
| B4.1 | AI subject, sky, background and object selection for masks; people masks with face / body parts | P1 | models; `parity.md` K rows ⬜ |
| B4.2 | Generative remove / object removal beyond clone-heal | P2 | model |
| B4.3 | Lens blur from a depth map (the phone's depth data or estimated) | P2 | partly present (`lens_blur`), depth model missing |
| B4.4 | Denoise and super resolution (Enhance) | P1 | `parity.md` P rows ⬜ |
| B4.5 | Versions and history on the phone: pick a version, tap a history step | P1 | exists on desktop (`RightPanel::Versions`); no compact entry (A2.11) |
| B4.6 | Edit a pre-selected set: copy / paste settings across selected photos, sync, apply the last edit, with checkboxes for what to paste | P1 | engine has it; no compact UI (A2.9) |
| B4.7 | HDR editing and display: edit and export HDR photos (gain map HEIC / AVIF), show them on an HDR display | P1 | `parity.md` Q rows ⬜; EDR output on Metal |
| B4.8 | Geometry on touch: guided Upright by drawing lines with a finger, straighten by dragging the dial, rotate / flip buttons | P1 | tools exist; no touch pass |
| B4.9 | Selective-adjustment gestures: a local adjustment pin you slide to change the amount, with a ring showing the radial / linear region while dragging | P1 | mask overlay exists |
| B4.10 | Colour grading wheels sized for a finger, with a "Reset" per wheel | P1 | A2.3 |
| B4.11 | Calibration and camera matching on phones' own raws (needs camera profiles, `ROADMAP.md`: colour calibration is a core gap) | P1 | camera profiles |
| B4.12 | Lens corrections from a lens database (profile-based) | P1 | clean-room data; no lensfun |
| B4.13 | Watermark and text overlays on export | P1 | exists on desktop; verify on iOS |
| B4.14 | Copy edit to another photo by sharing a "settings" file | P2 | |

### B5. App-level

| # | Feature | Pri | Needs |
|---|---|---|---|
| B5.1 | Onboarding, permissions prompts with explanations (Photos, Camera, Location, Files). Importing needs no permission (PHPicker, the Files picker); saving to Photos asks with LightCraft's own explanation | P0 | onboarding screens |
| B5.2 | Settings suited to a phone (cache size, storage, export defaults, "Include location in export") | P1 | settings dialog is a desktop window; scrolls but is not designed for touch |
| B5.3 | Crash and error reporting that the user can send (never-crash standard: the panic hook writes to a file the user cannot reach on iOS) | P1 | share the log via A1.3 |
| B5.4 | Localisation checks and a localised App Store listing | P2 | |
| B5.5 | Dark / light appearance following the system | P2 | LightCraft is dark only |
| B5.6 | Notifications for long exports, imports and background processing; background tasks | P2 | BGTaskScheduler glue |

## Suggested order

1. **Make it a usable app:** A1.1–A1.5, A1.7–A1.9, A1.13, B2.8, A2.8, A2.9. After this a person can import, cull, edit and export on a device. *Written and checked on the simulator:* all of it; on a device it launches and runs (A1.13), and what remains is the pickers, share sheet and keyboard there, ProRAW / Live Photos, and `UITextInput` for A1.9.
2. **Make editing comfortable:** A2.1–A2.5, A2.11, A2.13–A2.14, B4.5, B4.6, B4.8–B4.10.
3. **Parity with the phone app's core:** B2.1–B2.2 (sync and previews), B3.4, B3.6, B4.1, B4.4, B4.7, A2.15, A2.17.
4. **Later:** capture (B1), generative and search features (B3.2–B3.3, B4.2), widgets and extensions (B3.7), Pencil (A2.18).
