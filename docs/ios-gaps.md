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

## A. Known gaps in what exists

### A1. Platform host (nothing here works yet; the Menu → Import / Export commands have nothing to open)

| # | Gap | Pri | Needs |
|---|---|---|---|
| A1.1 | Photos library picker (import from the camera roll, with limited-access handling) | P0 | PhotosUI / PHPicker glue crate |
| A1.2 | Files picker for import (folders, external drives, iCloud Drive) with security-scoped bookmarks | P0 | UIDocumentPicker glue, copy into the sandbox |
| A1.3 | Export and share: share sheet, Save to Files, Save to Photos | P0 | UIActivityViewController glue |
| A1.4 | ✅ Library and settings in the app sandbox: the library in `Documents/`, settings in `Library/Application Support/LightCraft`, exports written to a staging folder for the share sheet (`ShareExports`), no desktop "Local" folders; photos handed over by the pickers are *moved* into the library (`file.addPhotos {staged: true}`), never referenced in `tmp/`; a library opened from another folder than last time (iOS gives the container a new path on every app update) re-points the photos stored under its old folder (`location.json`, `library.info` → `relocated`) | P0 | tested on Linux; check on a device after an app update |
| A1.5 | HEIC / HEIF decode (iPhone photos; import accepts `.heic` and then fails) | P0 | ImageIO behind the `FileLoader` hook |
| A1.6 | Apple ProRAW / JPEG XL compressed DNG (rejected today) | P1 | codec work in `crates/raw` |
| A1.7 | Memory budget for iOS (the probe returns nothing, so the 1.5 GiB default is too high); tiled full-resolution export (48 MP risk) | P0 | `crates/engine/src/memory.rs`, tiling in `pipeline` |
| A1.8 | App lifecycle: save on backgrounding, GPU pause / resume instead of disabling the GPU for the process | P0 | `crates/gpu` |
| A1.9 | On-screen keyboard: text fields, search, rename, keyword entry (egui's iOS text input is unverified) | P0 | winit / egui iOS text input, or a native text field bridge |
| A1.10 | Scene support without the `-[UIWindow initWithFrame:]` swizzle (winit has none) | P1 | a winit with scenes, or a native UIKit host |
| A1.11 | Signed device build, TestFlight, App Store review (privacy manifest, photo-library usage strings, icon set) | P0 | Apple developer account; an original app icon (`assets/ATTRIBUTION.md`) |
| A1.12 | Metal limits on iPhone GPUs for the compute path (needs at least 10 storage buffers per stage); the simulator allows 15 inter-stage variables | P1 | spike on devices |
| A1.13 | Verified on a real device (everything so far ran on the simulator and in headless snapshots) | P0 | a device |

### A2. Compact layout and touch

| # | Gap | Pri |
|---|---|---|
| A2.1 | Pinch zoom and two-finger pan are untested (the headless driver cannot inject multi-touch); the pinch focal point is only set when starting from Fit | P0 |
| A2.2 | Painting by finger with the sheet open: brush masks, the Remove tool and the red-eye tool are drawn over a loupe that is half hidden; no finger-sized brush preview offset, no loupe magnifier while painting | P1 |
| A2.3 | Tone curve editor, colour wheels (grading), HSL targeted drag, point colour: built for a mouse, no touch sizing | P1 |
| A2.4 | Sheet default is 40 % of the height: one slider fits with the tall touch rows; it should open larger, and collapse to a peek | P1 |
| A2.5 | Dialog rows with chips (format, colour space…) scroll sideways instead of wrapping; Export's buttons need scrolling to reach | P1 |
| A2.6 | Landscape phone layout (today: the portrait compact layout, which leaves the loupe small) | P1 |
| A2.7 | Safe area on rotation, Dynamic Island and the home indicator are handled once at startup frames only; not re-checked on rotation / split view | P1 |
| A2.8 | Multi-select by touch (tap-and-hold then tap, select-all, drag to select) and batch actions on the selection; the grid has no selection mode | P0 |
| A2.9 | Photo actions without right-click menus: rating, flag and colour label by swipe or toolbar in the loupe and the grid; delete, add to album, copy / paste settings | P0 |
| A2.10 | Sheets for the remaining desktop panels: filter bar, sort / group options, Info and Keywords details on a phone, Versions, Activity, Settings | P1 |
| A2.11 | Keywords, Versions and History have no tab: they are reachable only through Menu → Window | P1 |
| A2.12 | Haptics (slider detents at zero, snapping, mode switches) | P2 |
| A2.13 | Gestures to undo / redo (two- and three-finger tap or swipe) and shake to undo | P1 |
| A2.14 | Press-and-hold the photo to see the original (before) while editing; a before / after split by dragging | P1 |
| A2.15 | Accessibility: VoiceOver on the custom-drawn widgets (egui's accesskit on iOS is unverified), Dynamic Type, Reduce Motion, Increase Contrast | P1 |
| A2.16 | Drag and drop between apps (drag photos in from Photos / Files, out to Mail / Messages) | P2 |
| A2.17 | iPad: keyboard shortcuts and a menu bar for hardware keyboards, pointer hover and right-click, Stage Manager / multiple windows, external display | P1 |
| A2.18 | Apple Pencil: pressure-sensitive brush, hover preview, double-tap tool switch, squeeze | P2 |
| A2.19 | The desktop layout (1024 pt and wider, iPad landscape) still has mouse-sized targets and no touch gestures beyond what egui gives | P1 |
| A2.20 | UI language switching, right-to-left layouts and the Japanese / Chinese fonts have not been checked on iOS (`CRAFT_FONTS_DIR` is not wired into the Xcode build) | P1 |
| A2.21 | Tests: gesture tests need a multi-touch injector in `Headless`; no snapshot tests of the compact layout (they exist only as manual `lightcraft-cli snapshot` runs) | P1 |

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
| B2.8 | Save to Photos and send to other apps in the formats / sizes / watermark of the export dialog | P0 | A1.3 (export dialog exists) |
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
| B3.7 | Home-screen and lock-screen widgets, Shortcuts / Siri actions, Spotlight indexing, share extension ("Edit in LightCraft") | P2 | extensions in the Xcode project |
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
| B5.1 | Onboarding, permissions prompts with explanations (Photos, Camera, Location, Files) | P0 | A1.1 |
| B5.2 | Settings suited to a phone (cache size, storage, export defaults, "Include location in export") | P1 | settings dialog is a desktop window; scrolls but is not designed for touch |
| B5.3 | Crash and error reporting that the user can send (never-crash standard: the panic hook writes to a file the user cannot reach on iOS) | P1 | share the log via A1.3 |
| B5.4 | Localisation checks and a localised App Store listing | P2 | |
| B5.5 | Dark / light appearance following the system | P2 | LightCraft is dark only |
| B5.6 | Notifications for long exports, imports and background processing; background tasks | P2 | BGTaskScheduler glue |

## Suggested order

1. **Make it a usable app:** A1.1–A1.5, A1.7–A1.9, A1.13, B2.8, A2.8, A2.9. After this a person can import, cull, edit and export on a device.
2. **Make editing comfortable:** A2.1–A2.5, A2.11, A2.13–A2.14, B4.5, B4.6, B4.8–B4.10.
3. **Parity with the phone app's core:** B2.1–B2.2 (sync and previews), B3.4, B3.6, B4.1, B4.4, B4.7, A2.15, A2.17.
4. **Later:** capture (B1), generative and search features (B3.2–B3.3, B4.2), widgets and extensions (B3.7), Pencil (A2.18).
