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
| S0 | 9 | 6 | 0 | 0 | 9/15 |
| S1 | 92 | 15 | 60 | 0 | 92/167 |
| S2 | 1 | 0 | 79 | 22 | 23/102 |
| **Total** | 102 | 21 | 139 | 22 | 124/284 |
<!-- /ios-parity:summary -->

## Screens

| Id | Phase | Status | Evidence | Notes |
|---|---|---|---|---|
| screen:compact.grid | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | square tiles paged from the engine, month/year headers, pinch 1–14 columns, scrubber, badges, title → albums, sort, filter, + menu; no drag-choose or filter chips yet |
| screen:compact.loupe | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | fit, pinch, pan, double tap; region tiles, carousel, review mode are SW1.8 |
| screen:compact.edit.light | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | drafts while dragging, one undo step per drag |
| screen:allCommands | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/AllCommandsView.swift`, `apps/lightcraft-ios-swift/LightCraft/Tests/LightCraftCoreTests/EngineTests.swift` | every engine command, grouped, searchable, runnable; why a disabled one is |
| screen:activity | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | running tasks with Cancel; no activity stack page yet |
| screen:automation | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/ScriptHost.swift`, `crates/apphost/src/host/script.rs` | `LIGHTCRAFT_SCRIPT`: engine requests, `ui.screenshot`, `ui.inspect`, `ui.set` |
| screen:undoRedo | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | toolbar buttons; the undo menu (A2.11) is SW1.17 |
| screen:compact.albums | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | All Photos, Recently Added, People, Picks, Recently Deleted, the album tree with covers and counts, album menus (offline, share link, export, move, rename, delete), Create New; no lift-and-carry, By Date, Folders or Keywords sections yet |
| screen:compact.filterSheet | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/FilterSheet.swift`, `crates/engine/src/tests_libops.rs` | stars with ≥ = ≤, flags, labels; Type, Edited, Camera, People, Location, Keyword lists with counts (`library.facets`) |
| screen:compact.select | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/SelectionBar.swift` | long press or Select; action bar: rating, flag, label, add to album, export, more (copy, paste, paste selected), delete (album choice + confirm); the album + ▸ LightCraft flow; no drag-choose yet |
| screen:compact.reviewMode | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a tap hides the bars; up/down on the left half sets stars (44 pt a star), on the right the flag (56 pt a step); HUD; Auto Advance |
| screen:compact.carousel | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | a paging scroll view over the grid; follows Auto Advance |
| screen:compact.beforeHold | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | hold 0.4 s: the Before (`render.view {before}`), medium haptic |
| screen:compact.pullToDismiss | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the zoom navigation transition (the photo grows from and flies back into its tile, pull down to close); not yet checked on a device |
| screen:compact.gridPinch | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | pinch through 1, 2, 3, 4, 5, 7, 10, 14 columns, selection haptic per step |
| screen:compact.gridDates | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/DateBlocksView.swift`, `crates/engine/src/tests_libops.rs` | Years / Months / All capsule; Months and Years from `library.blocks` (moved from egui) |
| screen:compact.scrubber | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | over 200 photos: a handle down the right edge with the month in a bubble |
| screen:compact.tool.presets | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/Panels.swift`, `crates/apphost/src/host/tests.rs` | groups, favourites, user presets with live previews of the photo (`render.variant`), Amount 0–200 %, create, update, delete |
| screen:compact.tool.crop | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CropOverlay.swift` | frame with handles over the whole photo (`crop.drag`, rotate outside, double tap resets), dial, auto, lock, rotate, flips, aspect tiles and ratios, Geometry tab (Upright, sliders, Constrain Crop), ✕ undoes to where it opened (`edit.undoTo`); no guided upright or straighten line yet |
| screen:compact.tool.masking | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskingScreen.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskCanvas.swift` | the masks' column over the photo (create, select, a mask's menu, its parts with their menus: invert, duplicate, add / subtract / intersect, rename, delete; ± to combine), the groups (Mask: amount, invert, a part's own controls via `mask.shapeControls`, refine edges, overlay options; Light, Color, Effects, Detail, Optics), the overlay, pins, radial and linear handles, the brush, colour range sampling; Select Object and Describe need SAM, which iOS builds don't have |
| screen:compact.tool.remove | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/RemoveCanvas.swift` | Remove / Heal / Clone, size, feather, opacity, paint on the photo, pins (mode, new source, delete, move), Find Dust, Delete All; no Visualize Spots yet |
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
| screen:compact.export | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | the photos, Share (system share sheet), Save to Photos (add-only), file type, size, quality, watermark, metadata, location; exports run in the background with progress and Cancel; no export presets or More Options page yet |
| screen:compact.importReview | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportReview.swift`, `crates/apphost/src/host/tests.rs` | Photos / Files / folder picks scanned (`import.scan`), three across with checks and thumbnails, duplicates, in library and Recently Deleted marked; album (or a new one), keywords, preset, what to do with deleted ones; imported in the background (`import.start`, one undo step) |
| screen:compact.settings | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | the library, General, Import, Performance, Interface, Sync, AI Models; no Faces section yet |
| screen:compact.sync | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift`, `crates/apphost/src/sync.rs` | sign in, account, Sync Now, pause, On This Device options, shared links, sign out, the cloud button and status page, the conflict choice; the host drives the sync (shared with egui); no storage section yet |
| screen:compact.people | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift`, `crates/apphost/src/faces.rs` | people and unnamed groups, name a group, a person's photos; the host paces the face scan; face crops aren't rendered yet (the photo is shown) |
| screen:compact.map | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift` | MapKit with clustered photo markers (`map.points`), place search (`map.place`), a tap shows them in the grid |
| screen:compact.alerts | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | Move to Recently Deleted asks first; messages as glass capsules |
| screen:compact.haptics | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ControlSlider.swift` | selection on pinch steps, groups, tools, sliders at the default and the ends; medium on Before; not yet every place egui has |
| screen:compact.appearance | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | System / Light / Dark |
| screen:compact.landscape | S1 | ⬜ |  |  |
| screen:compact.ipadPortrait | S1 | ⬜ |  |  |
| screen:compact.accessibility | S1 | ⬜ |  |  |
| screen:compact.localization | S1 | ⬜ |  |  |
| screen:ipad.splitView | S2 | ⬜ |  |  |
| screen:ipad.inspector | S2 | ⬜ |  |  |
| screen:ipad.menuBar | S2 | ⬜ |  |  |
| screen:ipad.keyboard | S2 | ⬜ |  |  |
| screen:ipad.pointer | S2 | ⬜ |  |  |
| screen:ipad.compare | S2 | ⬜ |  |  |
| screen:ipad.survey | S2 | ⬜ |  |  |
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
| ui:modelSetup.cancel | S1 | ⬜ |  |  |
| ui:view.photoGrid | S2 | ⬜ |  |  |
| ui:view.squareGrid | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/GridView.swift` | the grid is square tiles |
| ui:view.gridToggle | S2 | ⬜ |  |  |
| ui:tool.guidedUpright | S2 | ⬜ |  |  |
| ui:view.detail | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | tap a photo in the grid |
| ui:view.compare | S2 | ⬜ |  |  |
| ui:view.survey | S2 | ⬜ |  |  |
| ui:view.people | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift` | albums ▸ People, grid "…" ▸ People |
| ui:view.map | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift` | grid "…" ▸ Map, Info ▸ the place |
| ui:view.person | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/PeopleView.swift` | a named person opens their page |
| ui:view.faceBoxes | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/FacesOverlay.swift` | the photo's "…" ▸ Faces: boxes from `photo.faces`, names or `faces.suggest` guesses under them, a tap names one |
| ui:view.reference | S2 | ⬜ |  |  |
| ui:photo.setReference | S2 | ⬜ |  |  |
| ui:compare.swap | S2 | ⬜ |  |  |
| ui:compare.set | S2 | ⬜ |  |  |
| ui:compare.makeSelect | S2 | ⬜ |  |  |
| ui:view.autoAdvance | S2 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ General ▸ Auto Advance |
| ui:view.filmstrip | S2 | ⬜ |  |  |
| ui:view.leftPanel | S2 | ⬜ |  |  |
| ui:view.beforeAfter | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the photo's "…" ▸ Before / After: side by side on a wide screen, stacked on a tall one |
| ui:view.beforeAfterSplit | S2 | ⬜ |  |  |
| ui:view.beforeAfterTopBottom | S2 | ⬜ |  |  |
| ui:view.beforeAfterSplitTopBottom | S2 | ⬜ |  |  |
| ui:view.showOriginal | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | hold the photo |
| ui:view.zoomFit | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | double tap / pinch out to fit; no command yet |
| ui:view.zoom100 | S1 | ⬜ |  |  |
| ui:view.zoomToggle | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | double tap: 2× and back |
| ui:view.clickZoom | S1 | ⬜ |  |  |
| ui:view.navigate | S1 | ⬜ |  |  |
| ui:view.zoomIn | S1 | ⬜ |  |  |
| ui:view.zoomOut | S1 | ⬜ |  |  |
| ui:view.clipping | S1 | ⬜ |  |  |
| ui:view.visualizeHdr | S1 | ⬜ |  |  |
| ui:view.softProof | S2 | ⬜ |  |  |
| ui:view.histogram | S1 | ⬜ |  |  |
| ui:view.maskOverlay | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Masking ▸ eye button (`render.view {overlay}`) |
| ui:view.maskOverlayMode | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Masking ▸ ⋯ ▸ Overlay Options ▸ Show As |
| ui:view.maskOverlayColor | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | Masking ▸ ⋯ ▸ Overlay Options ▸ Color, Opacity |
| ui:view.maskPins | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/MaskCanvas.swift` | Masking ▸ ⋯ ▸ Show Pins |
| ui:view.visualizeSpots | S1 | ⬜ |  |  |
| ui:view.cropOverlay | S1 | ⬜ |  |  |
| ui:view.cropOverlayOrientation | S1 | ⬜ |  |  |
| ui:view.back | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the navigation stack |
| ui:tool.done | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/ToolScreens.swift` | a tool screen's ✓ |
| ui:view.filterBar | S2 | ⬜ |  |  |
| ui:local.addRoot | S2 | 🚫 |  | no Local folders in the iOS sandbox (photos are copied into the library) |
| ui:local.hide | S2 | 🚫 |  | no Local folders in the iOS sandbox |
| ui:local.restoreHidden | S2 | 🚫 |  | no Local folders in the iOS sandbox |
| ui:view.fullScreenPreview | S1 | ⬜ |  |  |
| ui:view.lightsOut | S2 | ⬜ |  |  |
| ui:view.enterFullScreen | S2 | 🚫 |  | iOS apps are always full screen |
| ui:view.infoOverlay | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Info Overlay |
| ui:view.navigator | S2 | ⬜ |  |  |
| ui:panel.edit | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.profiles | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/EditGroupPanel.swift` | Edit ▸ Profile |
| ui:panel.crop | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.remove | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.masking | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.redeye | S1 | ⬜ |  |  |
| ui:panel.presets | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.info | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/PhotoScreen.swift` | the tool bar |
| ui:panel.keywords | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/HistorySheet.swift` | photo "…" ▸ Keywords |
| ui:panel.versions | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/HistorySheet.swift` | undo menu or "…" ▸ Versions |
| ui:panel.activity | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/HistorySheet.swift` | undo menu or "…" ▸ History |
| ui:view.syncStatus | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | the grid's cloud button |
| ui:dialog.syncChoice | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | the host asks (`ask {dialog: syncChoice}`); Settings ▸ Sync |
| ui:panel.close | S1 | ⬜ |  |  |
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
| ui:dialog.smartAlbum | S1 | ⬜ |  |  |
| ui:view.photoCounts | S2 | ⬜ |  |  |
| ui:view.sidebarSection | S2 | ⬜ |  |  |
| ui:view.sidebarMoveSection | S2 | ⬜ |  |  |
| ui:view.sidebarReset | S2 | ⬜ |  |  |
| ui:view.slideshow | S2 | ⬜ |  |  |
| ui:view.secondWindow | S2 | ⬜ |  |  |
| ui:tool.keywordPainter | S2 | ⬜ |  |  |
| ui:view.gridInfo | S2 | ⬜ |  |  |
| ui:view.appearance | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | Settings ▸ Interface ▸ Appearance |
| ui:view.theme | S2 | 🚫 |  | the system appearance (`app.appearance.*`) is the theme on iOS |
| ui:dialog.allMetadata | S1 | ⬜ |  |  |
| ui:dialog.faceModel | S1 | ⬜ |  |  |
| ui:dialog.newSmartAlbum | S1 | ⬜ |  |  |
| ui:dialog.createPreset | S1 | ⬜ |  |  |
| ui:dialog.autoStack | S2 | ⬜ |  |  |
| ui:dialog.copySettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | photo "…" ▸ Copy Settings… |
| ui:dialog.pasteSettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | select ▸ More ▸ Paste Selected Settings… |
| ui:dialog.shareLink | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | an album's menu ▸ Share Link… (signed in) |
| ui:view.focusSearch | S2 | ⬜ |  |  |
| ui:dialog.export | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | Share page; no presets or progress card yet |
| ui:dialog.contactSheet | S2 | ⬜ |  |  |
| ui:app.contactSheet | S2 | ⬜ |  |  |
| ui:photo.editInPhotoCraft | S2 | 🚫 |  | no external editors on iOS (the egui iOS app does not offer it either) |
| ui:photo.editInExternal | S2 | 🚫 |  | no external editors on iOS |
| ui:dialog.mergeHdr | S2 | ⬜ |  |  |
| ui:dialog.mergePanorama | S2 | ⬜ |  |  |
| ui:dialog.mergeHdrPanorama | S2 | ⬜ |  |  |
| ui:merge.hdrLast | S2 | ⬜ |  |  |
| ui:merge.panoramaLast | S2 | ⬜ |  |  |
| ui:merge.hdrPanoramaLast | S2 | ⬜ |  |  |
| ui:dialog.superRes | S1 | ⬜ |  |  |
| ui:superRes.cancel | S1 | ⬜ |  |  |
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
| ui:file.findMissing | S2 | ⬜ |  |  |
| ui:file.backupLibrary | S2 | ⬜ |  |  |
| ui:file.restoreLibrary | S2 | ⬜ |  |  |
| ui:photo.locate | S2 | ⬜ |  |  |
| ui:dialog.saveMetadataPreset | S2 | ⬜ |  |  |
| ui:app.quit | S2 | 🚫 |  | iOS apps are not quit by the app |
| ui:file.importPresets | S1 | ⬜ |  |  |
| ui:file.exportPresets | S1 | ⬜ |  |  |
| ui:file.importKeywords | S2 | ⬜ |  |  |
| ui:file.exportKeywords | S2 | ⬜ |  |  |
| ui:file.importCurvePresets | S2 | ⬜ |  |  |
| ui:file.exportCurvePresets | S2 | ⬜ |  |  |
| ui:app.settings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | grid "…" ▸ Settings |
| ui:app.newLibrary | S2 | 🚫 |  | one library in the app sandbox (`docs/ios-gaps.md` B5.2) |
| ui:app.openLibrary | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.recentLibraries | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.clearRecentLibraries | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.displayProfile | S2 | 🚫 |  | iOS colour-manages the display itself |
| ui:app.about | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | grid "…" ▸ About LightCraft |
| ui:app.systemInfo | S1 | ⬜ |  |  |
| ui:app.openLogFolder | S2 | 🚫 |  | no folders to open on iOS; sharing the log is `docs/ios-gaps.md` B5.3 |
| ui:app.whatsNew | S1 | ⬜ |  |  |
| ui:dialog.cull | S2 | ⬜ |  |  |
| ui:app.help | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/LibraryScreen.swift` | grid "…" ▸ Help |
| ui:app.discord | S1 | ⬜ |  |  |
| ui:app.feedback | S1 | ⬜ |  |  |
| ui:app.website | S1 | ⬜ |  |  |
| ui:app.github | S1 | ⬜ |  |  |
| ui:app.artcraft | S1 | ⬜ |  |  |
| ui:app.shortcuts | S2 | ⬜ |  |  |
| ui:app.setShortcut | S2 | 🚫 |  | iPadOS has no per-app shortcut editor; shortcuts come from the command specs |
| ui:app.resetShortcuts | S2 | 🚫 |  | no shortcut editor on iPadOS |
| ui:app.export | S0 | 🟡 | `crates/apphost/src/export.rs`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/AllCommandsView.swift` | runs from All Commands; the export page is SW1.4 |
| ui:app.showInFinder | S2 | 🚫 |  | no Finder on iOS |
| ui:dialog.rename | S2 | ⬜ |  |  |
| ui:dialog.labelNames | S2 | ⬜ |  |  |
| ui:dialog.captureTime | S2 | ⬜ |  |  |
| ui:photo.tagFromTracklog | S2 | ⬜ |  |  |
| ui:app.exportPrevious | S2 | ⬜ |  |  |
| ui:app.saveToPhotos | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | Share ▸ Save to Photos |
| ui:app.language.english | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.simplifiedChinese | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.traditionalChinese | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.japanese | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.portuguese | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.french | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.spanish | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.german | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.russian | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |
| ui:app.language.ukrainian | S1 | ⬜ |  | iOS per-app language (Settings ▸ LightCraft ▸ Language) or an in-app override: open question (`docs/ios-swift.md`) |

## Dialogs

| Id | Phase | Status | Evidence | Notes |
|---|---|---|---|---|
| dialog:ContactSheet | S2 | ⬜ |  |  |
| dialog:NewAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | name, Create |
| dialog:RenameAlbum | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Library/AlbumsSheet.swift` | an album's menu ▸ Rename… |
| dialog:TextPrompt | S1 | ⬜ |  |  |
| dialog:CaptureTime | S2 | ⬜ |  |  |
| dialog:Import | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ImportReview.swift` | the phone review |
| dialog:PresetImport | S1 | ⬜ |  |  |
| dialog:LabelNames | S2 | ⬜ |  |  |
| dialog:Rename | S2 | ⬜ |  |  |
| dialog:RenameKeyword | S1 | ⬜ |  |  |
| dialog:KeywordTag | S1 | ⬜ |  |  |
| dialog:KeywordSet | S2 | ⬜ |  |  |
| dialog:MoveKeyword | S2 | ⬜ |  |  |
| dialog:DeleteKeyword | S1 | ⬜ |  |  |
| dialog:MergeKeywords | S2 | ⬜ |  |  |
| dialog:AutoStack | S2 | ⬜ |  |  |
| dialog:NewSmartAlbum | S1 | ⬜ |  |  |
| dialog:WhatsNew | S1 | ⬜ |  |  |
| dialog:Cull | S2 | ⬜ |  |  |
| dialog:DenoiseModel | S1 | ⬜ |  |  |
| dialog:FaceModel | S1 | ⬜ |  |  |
| dialog:SystemInfo | S1 | ⬜ |  |  |
| dialog:AllMetadata | S1 | ⬜ |  |  |
| dialog:SmartRules | S1 | ⬜ |  |  |
| dialog:CreatePreset | S1 | ⬜ |  |  |
| dialog:CopySettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | the settings groups |
| dialog:PasteSettings | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Photo/CopySettingsSheet.swift` | the settings groups |
| dialog:Export | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Share/ExportSheet.swift` | Share page with options |
| dialog:Merge | S2 | ⬜ |  |  |
| dialog:Settings | S1 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | no Faces section yet |
| dialog:SamModel | S1 | ⬜ |  |  |
| dialog:SuperRes | S1 | ⬜ |  |  |
| dialog:LightroomCatalog | S2 | ⬜ |  |  |
| dialog:ConfirmDelete | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/App/RootView.swift` | an alert before Recently Deleted |
| dialog:SyncChoice | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | add to the server's library, use it, or don't sync |
| dialog:RemoveFolder | S2 | ⬜ |  |  |
| dialog:SynchronizeFolder | S2 | 🚫 |  | no watched folders in the iOS sandbox |
| dialog:About | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` |  |
| dialog:Shortcuts | S2 | ⬜ |  |  |
| dialog:ShareLink | S1 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/Settings/SettingsView.swift` | how long, originals, uploads, password |
| dialog:EditIn | S2 | 🚫 |  | opens another desktop app on the file; iOS apps are sandboxed (the share sheet covers it) |
