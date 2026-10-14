# SwiftUI app parity

What the SwiftUI iOS app (`apps/lightcraft-ios-swift`) does of what the egui app does: the gate for
removing the egui iOS app (`docs/ios-swift.md` → *Parity gate*). One row for every UI-only command of
the egui app (`ui:`, from `crates/ui-egui/src/menus.rs`), every dialog (`dialog:`, from
`crates/ui-egui/src/state.rs`) and every screen (`screen:`). Engine commands need no rows: the All
Commands screen lists and runs every one.

`cargo xtask parity` (part of `ci`) checks this file: every UI command and dialog has a row and no row
names one that is gone (a new egui UI command needs a decision here in the same change), phases are
S0, S1 or S2, cited paths exist, a ✅ row cites its evidence, a 🚫 row says why.
`cargo xtask parity --ios-gate S1` fails until every S0 and S1 row is ✅ or 🚫 (S2: every row).

**Status:** ✅ done · 🟡 partly · ⬜ not yet · 🚫 out of scope on iOS (with the reason).
**Phase:** S0 foundation · S1 phone parity (iPhone, iPad portrait) · S2 the full iPad layout.

<!-- ios-parity:summary -->
| Phase | ✅ | 🟡 | ⬜ | 🚫 | Done or out of scope |
|---|---:|---:|---:|---:|---:|
| S0 | 15 | 0 | 0 | 0 | 15/15 |
| S1 | 164 | 0 | 0 | 3 | 167/167 |
| S2 | 65 | 0 | 13 | 24 | 89/102 |
| **Total** | 244 | 0 | 13 | 27 | 271/284 |
<!-- /ios-parity:summary -->

## Screens

| Id | Phase | Status | Evidence | Notes |
|---|---|---|---|---|
| screen:compact.grid | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | square tiles paged from the engine, month/year headers, pinch 1–14 columns, scrubber, badges, title → albums, sort, filter, + menu; the filter chips with ×, Clear All and "14 of 120 photos" (`library.filterChips`); a sideways drag chooses a run (`library.chooseRun`, shared with egui) |
| screen:compact.loupe | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift`, `crates/apphost/src/region.rs`, `crates/apphost/src/host/tests.rs` | fit, pinch (1–8×), pan, double tap; zoomed in, the part on screen comes as a sharp window at the zoom scale (`render.view {zoom}`, planned by `apphost::region`, shared with egui) drawn over the fitted render; swipe between photos; review mode |
| screen:compact.edit.light | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | drafts while dragging, one undo step per drag |
| screen:allCommands | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/AllCommandsView.swift`, `apps/lightcraft-ios-swift/LightCraft/Tests/LightCraftCoreTests/EngineTests.swift` | every engine command, grouped, searchable, runnable; why a disabled one is |
| screen:activity | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the running tasks over the screen with Cancel; a tap opens the Activity page: every task with what it is on, its progress (counts or bytes) and Cancel |
| screen:automation | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/ScriptHost.swift`, `crates/apphost/src/host/script.rs` | `LIGHTCRAFT_SCRIPT`: engine requests, `ui.screenshot`, `ui.inspect`, `ui.set` |
| screen:undoRedo | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | Undo and Redo in the top bar's menu (it stays open), Reset, Versions, History |
| screen:compact.albums | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/DateFolderLists.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | All Photos, Recently Added, People, Picks, Recently Deleted, the album tree with covers and counts, album menus (offline, share link, export, move, rename, delete after asking, edit smart album), Create New (album, smart album, from filter, folder); By Date, Folders and Keywords pages; lift an album and carry it into a folder or onto the Albums heading (drag and drop: not checked on the simulator, whose synthetic touches open the menu instead) |
| screen:compact.filterSheet | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/FilterSheet.swift`, `crates/engine/src/tests_libops.rs` | stars with ≥ = ≤, flags, labels; Type, Edited, Camera, People, Location, Keyword lists with counts (`library.facets`) |
| screen:compact.select | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SelectionBar.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | long press or Select; a sideways drag chooses (or unchooses) a run; action bar: rating, flag, label, add to album, export, more (copy, paste, paste selected), delete (album choice + confirm); the album + ▸ LightCraft flow |
| screen:compact.reviewMode | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a tap hides the bars; up/down on the left half sets stars (44 pt a star), on the right the flag (56 pt a step); HUD; Auto Advance |
| screen:compact.carousel | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a paging scroll view over the grid; follows Auto Advance |
| screen:compact.beforeHold | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | hold 0.4 s: the Before (`render.view {before}`), medium haptic |
| screen:compact.pullToDismiss | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the zoom navigation transition: the photo grows from its tile and a pull down flies it back into it; checked with a swipe on the simulator after opening a photo from the grid (a photo opened with no tile to return to closes with Back or the edge swipe) |
| screen:compact.gridPinch | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | pinch through 1, 2, 3, 4, 5, 7, 10, 14 columns, selection haptic per step |
| screen:compact.gridDates | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/DateBlocksView.swift`, `crates/engine/src/tests_libops.rs` | Years / Months / All capsule; Months and Years from `library.blocks` (moved from egui) |
| screen:compact.scrubber | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | over 200 photos: a handle down the right edge with the month in a bubble |
| screen:compact.tool.presets | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/Panels.swift`, `crates/apphost/src/host/tests.rs` | groups, favourites, user presets with live previews of the photo (`render.variant`), Amount 0–200 %, create, update, delete |
| screen:compact.tool.crop | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CropOverlay.swift`, `crates/engine/src/tests_libops.rs` | frame with handles over the whole photo (`crop.drag`, rotate outside, double tap resets), dial, auto, Straighten (draw along the horizon: `crop.straighten {line}`), lock, rotate, flips, aspect tiles and ratios, Geometry tab (Upright incl. Guided: guides drawn on the photo, `geometry.guides {shown}`, Clear Guides; sliders, Constrain Crop), ✕ undoes to where it opened (`edit.undoTo`) |
| screen:compact.tool.masking | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskCanvas.swift` | the masks' column over the photo (create, select, a mask's menu, its parts with their menus: invert, duplicate, add / subtract / intersect, rename, delete; ± to combine), the groups (Mask: amount, invert, a part's own controls via `mask.shapeControls`, refine edges, overlay options; Light, Color, Effects, Detail, Optics), the overlay, pins, radial and linear handles, the brush, colour range sampling; Select Object and Describe need SAM, which iOS builds don't have |
| screen:compact.tool.remove | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/RemoveCanvas.swift` | Remove / Heal / Clone, size, feather, opacity, paint on the photo, pins (mode, new source, delete, move), Find Dust, Visualize Spots with its threshold (`render.view {overlay: spots}`), Generative AI (MI-GAN: offer, download, AI-filled strokes), Delete All |
| screen:compact.tool.info | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/Panels.swift` | file, date, size, camera, lens, exposure, place (→ Map), rating, flag, label, title, caption, keywords |
| screen:compact.edit.profile | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | profile browser: groups, favourites, recent, live thumbnails (`render.variant`), Amount |
| screen:compact.edit.color | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift`, `crates/engine/src/tests_libops.rs` | white balance presets, picker (tap → `develop.wbPick`), B&W, Temp/Tint on the photo's own scale (`develop.controls`: Kelvin for raws, relative for rendered files), Vibrance, Saturation, flyouts |
| screen:compact.edit.effects | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Texture, Clarity, Dehaze, vignette style and sliders, grain |
| screen:compact.edit.detail | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | every Detail slider from the engine (AI Denoise isn't in the iOS build) |
| screen:compact.edit.optics | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | every Optics slider from the engine |
| screen:compact.edit.calibration | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | every Calibration slider from the engine |
| screen:compact.edit.curve | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CurveEditor.swift`, `crates/engine/src/tests_color.rs` | parametric sliders; point curve (master, R, G, B) to drag, tap to add, double tap to remove, drawn from `curve.sample`; presets, reset |
| screen:compact.edit.mixer | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | eight bands with Hue / Saturation / Luminance; B&W mixer with Auto |
| screen:compact.edit.grading | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Shadows / Midtones / Highlights / Global wheel (one undo step a drag, double tap resets), luminance, Blending, Balance |
| screen:compact.edit.pointColor | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CurveEditor.swift`, `crates/engine/src/tests_color.rs` | eyedropper (tap the photo), swatches from `pointColor.list`, shifts and ranges, delete |
| screen:compact.undoMenu | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/HistorySheet.swift` | Undo and Redo side by side (the menu stays open), Reset, Versions, History |
| screen:compact.export | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | the photos, Share (system share sheet), Save to Photos (add-only), export presets (choose, save), file type, size, quality, watermark, metadata, location, More Options (colour space, bit depth, HDR, JPEG XL, output sharpening, compression, file names); exports run in the background with progress and Cancel |
| screen:compact.importReview | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportReview.swift`, `crates/apphost/src/host/tests.rs` | Photos / Files / folder picks scanned (`import.scan`), three across with checks and thumbnails, duplicates, in library and Recently Deleted marked; album (or a new one), keywords, preset, what to do with deleted ones; imported in the background (`import.start`, one undo step) |
| screen:compact.settings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/FacesSettings.swift` | the library, General, Import, Performance, Interface, Sync, Faces, AI Models |
| screen:compact.sync | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SyncStorageView.swift`, `crates/apphost/src/sync.rs` | sign in, account, Sync Now, pause, On This Device options, shared links, sign out, the cloud button and status page, the conflict choice, Storage (`sync.usage`: the server's files, limit and disk, this device's); the host drives the sync (shared with egui); checked against a local lightcraft-server on the simulator |
| screen:compact.people | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift`, `crates/engine/src/tests_libops.rs` | the library's named people (`people.named`), groups found on this device (`people.list`), unnamed faces (`faces.unnamed`) to name; a person's page: their faces (a tap opens the photo), rename everyone (`people.rename`), look-alikes with ✓ / ✕ and Confirm All; the scan's progress; empty state points to Settings ▸ Faces |
| screen:compact.map | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift`, `crates/geo/src/cluster.rs` | MapKit with clustered markers that picture up to four of their photos (`map.points` → `pictured`, chosen in `lightcraft_geo` for both apps), place search (`map.place`), a tap shows them in the grid, a long press: Show in Grid, Select, Zoom to These Photos |
| screen:compact.alerts | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the phone's one yes-or-no alert, Move to Recently Deleted (from the select bar or a photo's menu), with egui's words; messages as glass capsules; other questions are confirmation dialogs or sheets, as egui's are pages |
| screen:compact.haptics | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/HapticFeedback.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/AppModel.swift` | egui's haptic moments: selection on pinch steps, groups, tools, tabs, choosing, sliders at the default and the ends, the crop dial, masks, aspect lock; light on undo / redo, Reset, crop reset and Auto, double-tap zoom, the carousel, naming a face; medium on Before; warning on deletes. All through `haptic(…)`, which Settings ▸ General ▸ Haptics turns off. (The simulator plays none: not yet felt on a device.) |
| screen:compact.appearance | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | System / Light / Dark |
| screen:compact.landscape | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | a phone held sideways (compact height): with a tool open, the photo beside a 316 pt side panel (the tool's own screen, or the Edit groups, its controls and the tool bar), as egui's; the loupe full width over the tool bar; the grid twice the columns. Checked in Edit, Crop, Masking and Remove on the simulator (`ui.set {orientation}`) |
| screen:compact.ipadPortrait | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | egui's wide compact layout (from 600 pt): an open tool in a 340 pt side panel beside the photo, the grid's columns scaled to the width; checked on an iPad Pro 11" simulator. The albums list stays a sheet (egui's albums column comes with the full iPad layout, S2) |
| screen:compact.accessibility | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | every icon-only control has a VoiceOver label; grid photos are buttons named by title or file with their stars, pick or reject and edited as the value; text follows Dynamic Type (the tool, group and mask bars and the grid badges through `@ScaledMetric`, capped at xxxLarge so the bars keep their shape); checked at accessibility-extra-large on the simulator |
| screen:compact.localization | S1 | ✅ | `xtask/src/swift_strings.rs`, `apps/lightcraft-ios-swift/locales/keys.json`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Localization/es.lproj/Localizable.strings` | the egui languages through iOS localisation: `cargo xtask swift-strings` makes each `Localizable.strings` from the egui catalogs and the app's own; engine labels looked up at run time; dates formatted by iOS |
| screen:ipad.splitView | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | an iPad: the albums list as a sidebar column beside the grid (NavigationSplitView); it steps aside while a photo is open; checked on an iPad Pro 11" simulator |
| screen:ipad.inspector | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | an iPad's tools in a column beside the photo (the side panel, 340 pt), as egui's right panel |
| screen:ipad.menuBar | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/MenuBar.swift`, `crates/apphost/src/ui_commands.rs` | the iPadOS menu bar: File, Edit, Photo, View, Window, Help with the engine's menu commands and egui's UI commands this app has (`host.uiCommands`, the table moved from egui into `apphost`), enabled for what is selected; built without a crash on the iPad simulator |
| screen:ipad.keyboard | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/MenuBar.swift` | egui's default keys on the menu items (a hardware keyboard; ⌘ held lists them); the system's own keys stay the system's and a shared key goes to the first item (UIKit refuses duplicates) |
| screen:ipad.pointer | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the pointer lifts a grid photo and highlights a tool under it; system controls follow the pointer themselves |
| screen:ipad.compare | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | Compare: the Select beside a Candidate, Swap, Make Select, the candidates one by one; a tap makes a side active (`render.variant {edited}`); checked on the iPad simulator |
| screen:ipad.survey | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | Survey: the chosen photos as large as the screen allows; a tap makes one active, ✕ takes it out, a double tap opens it; checked on the iPad simulator |
| screen:ipad.multiwindow | S2 | ⬜ |  |  |
| screen:ipad.dragDrop | S2 | ⬜ |  |  |
| screen:ipad.pencil | S2 | ⬜ |  |  |
| screen:ipad.externalDisplay | S2 | ⬜ |  |  |

## UI commands

| Id | Phase | Status | Evidence | Notes |
|---|---|---|---|---|
| ui:app.appearance.system | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Appearance |
| ui:app.appearance.light | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Appearance |
| ui:app.appearance.dark | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Appearance |
| ui:ui.dock | S2 | 🚫 |  | no dockable panels on iOS: tools are sheets (phone) and an inspector (iPad) |
| ui:ui.dock.reset | S2 | 🚫 |  | no dockable panels on iOS |
| ui:modelSetup.cancel | S1 | 🚫 |  | it cancels a photo action waiting for the AI Denoise model; iOS builds have no AI Denoise or SAM 3: the engine's `denoise` and `sam` features are off for iOS (the egui iOS app has only `enhance` too) |
| ui:view.photoGrid | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the grid is the app's first screen: Back (or a pull down) returns to it from a photo |
| ui:view.squareGrid | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | the grid is square tiles |
| ui:view.gridToggle | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/MenuBar.swift` | View ▸ Grid (G): between the grid and the photo |
| ui:tool.guidedUpright | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CropOverlay.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Crop ▸ Geometry ▸ Upright ▸ Guided: guides drawn on the photo (SW1.15) |
| ui:view.detail | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | tap a photo in the grid |
| ui:view.compare | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SelectionBar.swift` | select bar "…" ▸ Compare (two or more photos) |
| ui:view.survey | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SelectionBar.swift` | select bar "…" ▸ Survey |
| ui:view.people | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift` | albums ▸ People, grid "…" ▸ People |
| ui:view.map | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift` | grid "…" ▸ Map, Info ▸ the place |
| ui:view.person | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift` | a named person opens their page |
| ui:view.faceBoxes | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/FacesOverlay.swift` | the photo's "…" ▸ Faces: boxes from `photo.faces`, names or `faces.suggest` guesses under them, a tap names one |
| ui:view.reference | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | the photo's "…" ▸ Reference View: the reference photo beside the one being edited; checked on the iPad simulator |
| ui:photo.setReference | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the photo's "…" ▸ Set as Reference |
| ui:compare.swap | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | Compare ▸ Swap |
| ui:compare.set | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | Compare: a tap on a side makes it the active photo; the arrows choose the Candidate |
| ui:compare.makeSelect | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | Compare ▸ Make Select (the next Candidate comes up) |
| ui:view.autoAdvance | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ General ▸ Auto Advance |
| ui:view.filmstrip | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the photo's "…" ▸ Filmstrip: the grid's photos in a row under the photo; a tap shows one |
| ui:view.leftPanel | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | the sidebar button, or a tap on the collection's name, shows or hides the albums column |
| ui:view.beforeAfter | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the photo's "…" ▸ Before / After: side by side on a wide screen, stacked on a tall one |
| ui:view.beforeAfterSplit | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the photo's "…" ▸ Before / After Layout ▸ Split Left / Right (a line to drag); checked on the iPad simulator |
| ui:view.beforeAfterTopBottom | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | Before / After Layout ▸ Top / Bottom (also Left / Right, Automatic) |
| ui:view.beforeAfterSplitTopBottom | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | Before / After Layout ▸ Split Top / Bottom |
| ui:view.showOriginal | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | hold the photo |
| ui:view.zoomFit | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | double tap back, or pinch out, to fit |
| ui:view.zoom100 | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | pinch to any zoom up to 8×, sharp at 1:1 and beyond (the window is the photo's own pixels) |
| ui:view.zoomToggle | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | double tap: 2× and back |
| ui:view.clickZoom | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | double tap zooms to the ratio in Settings ▸ Interface ▸ Double Tap Zooms To |
| ui:view.navigate | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | drag pans the zoomed photo |
| ui:view.zoomIn | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | pinch out |
| ui:view.zoomOut | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | pinch in |
| ui:view.clipping | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a tap on the histogram: red around a photo with whites clipped, blue with blacks (the host sends the clipped shares) |
| ui:view.visualizeHdr | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Edit ▸ Light ▸ HDR ▸ Visualize HDR (`render.view {overlay: hdr}`) |
| ui:view.softProof | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the photo's "…" ▸ Soft Proofing, Proof As, Destination Gamut Warning (`render.view {proof}`): paper-white surround, "Proof Preview · <space>" |
| ui:view.histogram | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the photo's "…" ▸ Histogram: luminance and R, G, B over the photo, with the clipping triangles |
| ui:view.maskOverlay | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Masking ▸ eye button (`render.view {overlay}`) |
| ui:view.maskOverlayMode | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Masking ▸ ⋯ ▸ Overlay Options ▸ Show As |
| ui:view.maskOverlayColor | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Masking ▸ ⋯ ▸ Overlay Options ▸ Color, Opacity |
| ui:view.maskPins | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskCanvas.swift` | Masking ▸ ⋯ ▸ Show Pins |
| ui:view.visualizeSpots | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Remove ▸ ⋯ ▸ Visualize Spots, and its Threshold |
| ui:view.cropOverlay | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CropOverlay.swift`, `crates/geom/src/guides.rs` | Crop ▸ ⋯ ▸ Overlay: thirds, grid, golden ratio, diagonal, golden triangle, golden spiral, none (`crop.guides`, geometry shared with egui) |
| ui:view.cropOverlayOrientation | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Crop ▸ ⋯ ▸ Cycle Overlay Orientation (triangle and spiral) |
| ui:view.back | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the navigation stack |
| ui:tool.done | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | a tool screen's ✓ |
| ui:view.filterBar | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/FilterSheet.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | the filter sheet (stars, flags, labels, type, camera, people, place, keyword) and the chips over the grid do the filter bar's work |
| ui:local.addRoot | S2 | 🚫 |  | no Local folders in the iOS sandbox (photos are copied into the library) |
| ui:local.hide | S2 | 🚫 |  | no Local folders in the iOS sandbox |
| ui:local.restoreHidden | S2 | 🚫 |  | no Local folders in the iOS sandbox |
| ui:view.fullScreenPreview | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a tap on the photo hides everything but it (review mode) |
| ui:view.lightsOut | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | the photo's "…" ▸ Lights Out: the photo alone on black; a tap ends it |
| ui:view.enterFullScreen | S2 | 🚫 |  | iOS apps are always full screen |
| ui:view.infoOverlay | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Info Overlay |
| ui:view.navigator | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | zoomed in: the whole photo small in a corner with the part on screen marked (the photo's "…" ▸ Navigator turns it off) |
| ui:panel.edit | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.profiles | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Edit ▸ Profile |
| ui:panel.crop | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.remove | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.masking | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.redeye | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/RedEyeCanvas.swift` | the photo's "…" ▸ Red Eye: drag a box around an eye (`redeye.add`), red or pet eyes, the chosen one's pupil size and darkening, catchlight, delete |
| ui:panel.presets | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.info | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.keywords | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/HistorySheet.swift` | photo "…" ▸ Keywords |
| ui:panel.versions | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/HistorySheet.swift` | undo menu or "…" ▸ Versions |
| ui:panel.activity | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/HistorySheet.swift` | undo menu or "…" ▸ History |
| ui:view.syncStatus | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the grid's cloud button |
| ui:dialog.syncChoice | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | the host asks (`ask {dialog: syncChoice}`); Settings ▸ Sync |
| ui:panel.close | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a tap on the open tool closes its panel; ✕ / ✓ close a tool screen |
| ui:section.light | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | sliders from `develop.controls` |
| ui:section.color | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Edit ▸ Color |
| ui:section.effects | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Edit ▸ Effects |
| ui:section.detail | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Edit ▸ Detail |
| ui:section.optics | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Edit ▸ Optics |
| ui:tool.brush | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskCanvas.swift` | Masking ▸ + ▸ Brush |
| ui:tool.linear | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift` | Masking ▸ + ▸ Linear Gradient |
| ui:tool.radial | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift` | Masking ▸ + ▸ Radial Gradient |
| ui:tool.wbPicker | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Color ▸ eyedropper, then a tap on the photo |
| ui:tool.none | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift` | the brush's Done |
| ui:brush.smaller | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift` | the brush's Size slider (no keyboard on a phone) |
| ui:brush.larger | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift` | the brush's Size slider |
| ui:brush.featherLess | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift` | the brush's Feather slider |
| ui:brush.featherMore | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift` | the brush's Feather slider |
| ui:dialog.newAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | albums ▸ +, grid "…", Add to Album |
| ui:dialog.newFolder | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | albums ▸ + ▸ Folder |
| ui:dialog.smartAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SmartAlbumSheet.swift` | an album's ⋯ ▸ Edit Smart Album…: name, match, rules and groups from `album.ruleFields` (with each operator's starting value), checked with `album.checkRules` |
| ui:view.photoCounts | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Show Counts in Albums |
| ui:view.sidebarSection | S2 | ⬜ |  |  |
| ui:view.sidebarMoveSection | S2 | ⬜ |  |  |
| ui:view.sidebarReset | S2 | ⬜ |  |  |
| ui:view.slideshow | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ViewModes.swift` | grid or photo "…" ▸ Slideshow: the grid's photos over black, crossfading every few seconds; a tap pauses |
| ui:view.secondWindow | S2 | ⬜ |  |  |
| ui:tool.keywordPainter | S2 | ⬜ |  |  |
| ui:view.gridInfo | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | grid "…" ▸ Grid Info: the file name over each photo |
| ui:view.appearance | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Appearance |
| ui:view.theme | S2 | 🚫 |  | the system appearance (`app.appearance.*`) is the theme on iOS |
| ui:dialog.allMetadata | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/Panels.swift` | Info ▸ All Metadata…: EXIF groups and XMP (`photo.allMetadata`), searchable |
| ui:dialog.faceModel | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/FacesSettings.swift` | Settings ▸ Faces ▸ Add a Model File… (Files), then its terms to accept (`faces.models.inspect`, `faces.models.install`) |
| ui:dialog.newSmartAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | Albums ▸ + ▸ Smart Album from Filter: a name, then the grid's view becomes the album (`album.createSmart`) |
| ui:dialog.createPreset | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | Presets ▸ +: name, group (yours or a new one), the settings groups from `develop.settingsGroups` |
| ui:dialog.autoStack | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | grid "…" ▸ Auto-Stack by Capture Time…: the gap, with how many stacks it makes as it changes (`stack.auto {preview}`) |
| ui:dialog.copySettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | photo "…" ▸ Copy Settings… |
| ui:dialog.pasteSettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | select ▸ More ▸ Paste Selected Settings… |
| ui:dialog.shareLink | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | an album's menu ▸ Share Link… (signed in) |
| ui:view.focusSearch | S2 | ⬜ |  |  |
| ui:dialog.export | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | the Share page with presets, options, More Options and progress |
| ui:dialog.contactSheet | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | select bar "…" ▸ Contact Sheet…: paper, orientation, columns, rows, names → PDF (`export.contactSheet`), the share sheet |
| ui:app.contactSheet | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | as Contact Sheet… |
| ui:photo.editInPhotoCraft | S2 | 🚫 |  | no external editors on iOS (the egui iOS app does not offer it either) |
| ui:photo.editInExternal | S2 | 🚫 |  | no external editors on iOS |
| ui:dialog.mergeHdr | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/MergeSheet.swift`, `crates/apphost/src/merge.rs` | select bar "…" ▸ Photo Merge ▸ HDR…: options and a live preview (`host.merge.preview`, the runner shared with egui), Merge in the background |
| ui:dialog.mergePanorama | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/MergeSheet.swift`, `crates/apphost/src/merge.rs` | Photo Merge ▸ Panorama… |
| ui:dialog.mergeHdrPanorama | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/MergeSheet.swift`, `crates/apphost/src/merge.rs` | Photo Merge ▸ HDR Panorama… |
| ui:merge.hdrLast | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SelectionBar.swift`, `crates/apphost/src/merge.rs` | Photo Merge ▸ HDR with Last Settings (`host.merge.last`) |
| ui:merge.panoramaLast | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SelectionBar.swift`, `crates/apphost/src/merge.rs` | Photo Merge ▸ Panorama with Last Settings |
| ui:merge.hdrPanoramaLast | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SelectionBar.swift`, `crates/apphost/src/merge.rs` | Photo Merge ▸ HDR Panorama with Last Settings |
| ui:dialog.superRes | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/SuperResSheet.swift`, `crates/apphost/src/superres.rs` | the photo's "…" ▸ Super Resolution…: the model's size, credit and licence, Download (only when pressed), progress; then the size before and after and Enlarge, in the background (`apphost::superres`, shared with egui) |
| ui:superRes.cancel | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/SuperResSheet.swift` | the Enlarging row's ✕ in the activity stack; Cancel Download while the model downloads |
| ui:view.selectMode | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | Select, or a long press |
| ui:view.reviewMode | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a tap on the photo |
| ui:view.filterSheet | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/FilterSheet.swift` | the grid header's filter button |
| ui:view.scrollToTop | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | the status bar tap (a native scroll view) |
| ui:view.tagFace | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/FacesOverlay.swift` | the photo's "…" ▸ Tag a Face: a tap marks a box there (`photo.addRegion {shown}`) and asks who it is |
| ui:view.gridColumns | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | pinch |
| ui:view.gridSpan | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | Years / Months / All |
| ui:file.addPhotos | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportModel.swift` | the library's Import menu (Photos, Files, a folder), then the review |
| ui:file.importFromPhotos | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportModel.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportReview.swift` | the Photos picker (originals), then the review |
| ui:file.importFromFiles | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportModel.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportReview.swift` | the Files picker, then the review |
| ui:file.importFolderFromFiles | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportModel.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportReview.swift` | a folder (≤ 5000 photos, 12 levels), then the review |
| ui:file.addFolder | S2 | 🚫 |  | the desktop folder dialog; iOS uses `file.importFolderFromFiles` |
| ui:file.importLightroom | S2 | ⬜ |  |  |
| ui:file.addFromDevice | S2 | ⬜ |  |  |
| ui:file.findMissing | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/DateFolderLists.swift` | Albums ▸ Folders ▸ Find Missing Photos…: a folder from Files (`library.findMissing`; it searches on the engine thread, where egui uses a worker) |
| ui:file.backupLibrary | S2 | 🚫 |  | the web app's only (its library lives in the browser): on iOS the library is a folder in the app's Documents, kept by the device's and iCloud backups; the egui iOS app has none either |
| ui:file.restoreLibrary | S2 | 🚫 |  | as Back Up Library: web only |
| ui:photo.locate | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | a photo's "…" ▸ Locate Missing File…: its file from Files (`photo.relink`) |
| ui:dialog.saveMetadataPreset | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | select bar or a photo's "…" ▸ Metadata Presets…: apply, delete, save the photo's as a preset |
| ui:app.quit | S2 | 🚫 |  | iOS apps are not quit by the app |
| ui:file.importPresets | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PresetImportSheet.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/Panels.swift` | Presets ▸ ⊕ ▸ Import Presets… (Files): the review, then Import (`preset.importReview`, `preset.importRun`, shared with egui) |
| ui:file.exportPresets | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/Panels.swift` | Presets ▸ ⊕ ▸ Export Presets…: yours (the group shown, if it is yours) as a `.lcpreset` file to share or save |
| ui:file.importKeywords | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | Keywords ▸ "…" ▸ Import Keywords… (a file from Files, `keyword.import`) |
| ui:file.exportKeywords | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | Keywords ▸ "…" ▸ Export Keywords… (`keyword.export`, the share sheet) |
| ui:file.importCurvePresets | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CurveEditor.swift` | the curve's presets menu ▸ Import Curve Presets… (.lccurve / JSON from Files) |
| ui:file.exportCurvePresets | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CurveEditor.swift` | the curve's presets menu ▸ Export Curve Presets… (the share sheet); also Save Point Curve as Preset… |
| ui:app.settings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | grid "…" ▸ Settings |
| ui:app.newLibrary | S2 | 🚫 |  | one library in the app sandbox (`docs/ios-gaps.md` B5.2) |
| ui:app.openLibrary | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.recentLibraries | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.clearRecentLibraries | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.displayProfile | S2 | 🚫 |  | iOS colour-manages the display itself |
| ui:app.about | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | grid "…" ▸ About LightCraft |
| ui:app.systemInfo | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | About ▸ System Info (`about.systemInfo`, plus the app's own rows) |
| ui:app.openLogFolder | S2 | 🚫 |  | no folders to open on iOS; sharing the log is `docs/ios-gaps.md` B5.3 |
| ui:app.whatsNew | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | About ▸ What's New (`about.whatsNew`) |
| ui:dialog.cull | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | select bar "…" ▸ Assisted Culling…: reject below a focus score, pick the sharpest of each burst (`photo.analyze`); checked on the simulator |
| ui:app.help | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | grid "…" ▸ Help |
| ui:app.discord | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | About ▸ the links (`about.links`) |
| ui:app.feedback | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | About ▸ Send Feedback… |
| ui:app.website | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | About ▸ LightCraft Website |
| ui:app.github | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | About ▸ LightCraft on GitHub |
| ui:app.artcraft | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | About ▸ ArtCraft Website |
| ui:app.shortcuts | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/MenuBar.swift` | Help ▸ Keyboard Shortcuts: every key the menus answer, by menu (choosing other keys is egui's only) |
| ui:app.setShortcut | S2 | 🚫 |  | iPadOS has no per-app shortcut editor; shortcuts come from the command specs |
| ui:app.resetShortcuts | S2 | 🚫 |  | no shortcut editor on iPadOS |
| ui:app.export | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift`, `crates/apphost/src/export.rs` | Share ▸ Share / Save to Photos (`app.export` in the background); All Commands runs it too |
| ui:app.showInFinder | S2 | 🚫 |  | no Finder on iOS |
| ui:dialog.rename | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | select bar or a photo's "…" ▸ Rename…: template with its tags (`photo.renameTokens`), live preview (`photo.renamePreview`), `photo.rename` |
| ui:dialog.labelNames | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | grid "…" ▸ Color Label Names…: the five names and the label sets (apply, save, delete) |
| ui:dialog.captureTime | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | Rename's menu ▸ Edit Capture Time…: adjust to a time (the others move by as much), set each, shift by time zone (`photo.setCaptureTime`) |
| ui:photo.tagFromTracklog | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | select bar "…" ▸ Auto-Tag from Tracklog…: a .gpx from Files (`photo.autoTagTracklog`) |
| ui:app.exportPrevious | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift`, `crates/apphost/src/export.rs` | select bar "…" ▸ Export with Previous: the last export's options (remembered by `apphost::export::remember`, shared with egui) for the chosen photos, then the share sheet (`host.exportPrevious`); checked on the simulator |
| ui:app.saveToPhotos | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | Share ▸ Save to Photos |
| ui:app.language.english | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ English), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.simplifiedChinese | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ 简体中文), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.traditionalChinese | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ 繁體中文), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.japanese | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ 日本語), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.portuguese | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ Português (Brasil)), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.french | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ Français), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.spanish | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ Español), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.german | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ Deutsch), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.russian | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ Русский), opened from Settings ▸ Interface ▸ Language |
| ui:app.language.ukrainian | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | iOS per-app language (Settings ▸ LightCraft ▸ Language ▸ Українська), opened from Settings ▸ Interface ▸ Language |

## Dialogs

| Id | Phase | Status | Evidence | Notes |
|---|---|---|---|---|
| dialog:ContactSheet | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | Contact Sheet sheet |
| dialog:NewAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | name, Create |
| dialog:RenameAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | an album's menu ▸ Rename… |
| dialog:TextPrompt | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | each one-field prompt is a native alert or sheet where it is used (album and keyword names, mask and part names, export preset names) |
| dialog:CaptureTime | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | Edit Capture Time sheet |
| dialog:Import | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportReview.swift` | the phone review |
| dialog:PresetImport | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PresetImportSheet.swift`, `crates/engine/src/preset_review.rs` | the presets by group with what they set and what isn't carried over, LUT profiles, unreadable files; then how it went |
| dialog:LabelNames | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | Color Label Names sheet |
| dialog:Rename | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | Rename Photos sheet |
| dialog:RenameKeyword | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | Albums ▸ Keywords ▸ a keyword's menu ▸ Rename… (`keyword.rename`) |
| dialog:KeywordTag | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | Albums ▸ Keywords ▸ + or a keyword's menu: Create / Edit Keyword Tag (synonyms, export options, person, add to the selected photos) |
| dialog:KeywordSet | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | grid "…" ▸ Keyword Sets…: the nine keywords to toggle on the chosen photos, the sets, editing and new sets |
| dialog:MoveKeyword | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | Keywords ▸ a keyword's menu ▸ Move To… (a keyword or the top level) |
| dialog:DeleteKeyword | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | Albums ▸ Keywords ▸ a keyword's menu ▸ Delete… (asks, with its photo count) |
| dialog:MergeKeywords | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/KeywordListView.swift` | Keywords ▸ a keyword's menu ▸ Merge Into… |
| dialog:AutoStack | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | Auto-Stack sheet |
| dialog:NewSmartAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | as `ui:dialog.newSmartAlbum` |
| dialog:WhatsNew | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | as `ui:app.whatsNew` |
| dialog:Cull | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryDialogs.swift` | the Assisted Culling sheet |
| dialog:DenoiseModel | S1 | 🚫 |  | iOS builds have no AI Denoise or SAM 3: the engine's `denoise` and `sam` features are off for iOS (the egui iOS app has only `enhance` too) |
| dialog:FaceModel | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/FacesSettings.swift` | the terms sheet for a model file or a download (nothing fetched or installed before they are accepted) |
| dialog:SystemInfo | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | as `ui:app.systemInfo` |
| dialog:AllMetadata | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/Panels.swift` | as `ui:dialog.allMetadata` |
| dialog:SmartRules | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SmartAlbumSheet.swift` | as `ui:dialog.smartAlbum`; Albums ▸ + ▸ Smart Album… for a new one |
| dialog:CreatePreset | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | as `ui:dialog.createPreset` |
| dialog:CopySettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | the settings groups |
| dialog:PasteSettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | the settings groups |
| dialog:Export | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | as `ui:dialog.export` |
| dialog:Merge | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/MergeSheet.swift`, `crates/apphost/src/host/tests.rs` | the merge sheet; checked on the simulator (an HDR preview of two imported photos) |
| dialog:Settings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/FacesSettings.swift` | General, Import, Performance, Interface, Sync, Faces (recognition, models, scan progress), AI Models |
| dialog:SamModel | S1 | 🚫 |  | iOS builds have no AI Denoise or SAM 3: the engine's `denoise` and `sam` features are off for iOS (the egui iOS app has only `enhance` too) |
| dialog:SuperRes | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/SuperResSheet.swift` | as `ui:dialog.superRes` |
| dialog:LightroomCatalog | S2 | ⬜ |  |  |
| dialog:ConfirmDelete | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | an alert before Recently Deleted |
| dialog:SyncChoice | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | add to the server's library, use it, or don't sync |
| dialog:RemoveFolder | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/DateFolderLists.swift` | Albums ▸ Folders ▸ a folder's menu ▸ Remove Folder from Library… (asks; `library.removeFolder`) |
| dialog:SynchronizeFolder | S2 | 🚫 |  | no watched folders in the iOS sandbox |
| dialog:About | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` |  |
| dialog:Shortcuts | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/MenuBar.swift` | the Keyboard Shortcuts sheet; checked on the iPad simulator |
| dialog:ShareLink | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | how long, originals, uploads, password |
| dialog:EditIn | S2 | 🚫 |  | opens another desktop app on the file; iOS apps are sandboxed (the share sheet covers it) |
