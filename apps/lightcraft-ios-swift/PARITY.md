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
| S0 | 8 | 7 | 0 | 0 | 8/15 |
| S1 | 0 | 0 | 167 | 0 | 0/167 |
| S2 | 0 | 0 | 81 | 21 | 21/102 |
| **Total** | 8 | 7 | 248 | 21 | 29/284 |
<!-- /ios-parity:summary -->

## Screens

| Id | Phase | Status | Evidence | Notes |
|---|---|---|---|---|
| screen:compact.grid | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/LibraryView.swift` | square grid; no pinch, dates, scrubber or select mode yet (SW1.20, SW1.5) |
| screen:compact.loupe | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/LoupeView.swift` | fit, pinch, pan, double tap; region tiles, carousel, review mode are SW1.8 |
| screen:compact.edit.light | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/EditSheet.swift` | drafts while dragging, one undo step per drag |
| screen:allCommands | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/AllCommandsView.swift`, `apps/lightcraft-ios-swift/LightCraft/Tests/LightCraftCoreTests/EngineTests.swift` | every engine command, grouped, searchable, runnable; why a disabled one is |
| screen:activity | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/RootView.swift` | running tasks with Cancel; no activity stack page yet |
| screen:automation | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/ScriptHost.swift`, `crates/apphost/src/host/script.rs` | `LIGHTCRAFT_SCRIPT`: engine requests, `ui.screenshot`, `ui.inspect`, `ui.set` |
| screen:undoRedo | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/LoupeView.swift` | toolbar buttons; the undo menu (A2.11) is SW1.17 |
| screen:compact.albums | S1 | ⬜ |  |  |
| screen:compact.filterSheet | S1 | ⬜ |  |  |
| screen:compact.select | S1 | ⬜ |  |  |
| screen:compact.reviewMode | S1 | ⬜ |  |  |
| screen:compact.carousel | S1 | ⬜ |  |  |
| screen:compact.beforeHold | S1 | ⬜ |  |  |
| screen:compact.pullToDismiss | S1 | ⬜ |  |  |
| screen:compact.gridPinch | S1 | ⬜ |  |  |
| screen:compact.gridDates | S1 | ⬜ |  |  |
| screen:compact.scrubber | S1 | ⬜ |  |  |
| screen:compact.tool.presets | S1 | ⬜ |  |  |
| screen:compact.tool.crop | S1 | ⬜ |  |  |
| screen:compact.tool.masking | S1 | ⬜ |  |  |
| screen:compact.tool.remove | S1 | ⬜ |  |  |
| screen:compact.tool.info | S1 | ⬜ |  |  |
| screen:compact.edit.profile | S1 | ⬜ |  |  |
| screen:compact.edit.color | S1 | ⬜ |  |  |
| screen:compact.edit.effects | S1 | ⬜ |  |  |
| screen:compact.edit.detail | S1 | ⬜ |  |  |
| screen:compact.edit.optics | S1 | ⬜ |  |  |
| screen:compact.edit.calibration | S1 | ⬜ |  |  |
| screen:compact.edit.curve | S1 | ⬜ |  |  |
| screen:compact.edit.mixer | S1 | ⬜ |  |  |
| screen:compact.edit.grading | S1 | ⬜ |  |  |
| screen:compact.edit.pointColor | S1 | ⬜ |  |  |
| screen:compact.undoMenu | S1 | ⬜ |  |  |
| screen:compact.export | S1 | ⬜ |  |  |
| screen:compact.importReview | S1 | ⬜ |  |  |
| screen:compact.settings | S1 | ⬜ |  |  |
| screen:compact.sync | S1 | ⬜ |  |  |
| screen:compact.people | S1 | ⬜ |  |  |
| screen:compact.map | S1 | ⬜ |  |  |
| screen:compact.alerts | S1 | ⬜ |  |  |
| screen:compact.haptics | S1 | ⬜ |  |  |
| screen:compact.appearance | S1 | ⬜ |  |  |
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
| ui:app.appearance.system | S1 | ⬜ |  |  |
| ui:app.appearance.light | S1 | ⬜ |  |  |
| ui:app.appearance.dark | S1 | ⬜ |  |  |
| ui:ui.dock | S2 | 🚫 |  | no dockable panels on iOS: tools are sheets (phone) and an inspector (iPad) |
| ui:ui.dock.reset | S2 | 🚫 |  | no dockable panels on iOS |
| ui:modelSetup.cancel | S1 | ⬜ |  |  |
| ui:view.photoGrid | S2 | ⬜ |  |  |
| ui:view.squareGrid | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/LibraryView.swift` | the grid is square tiles |
| ui:view.gridToggle | S2 | ⬜ |  |  |
| ui:tool.guidedUpright | S2 | ⬜ |  |  |
| ui:view.detail | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/LoupeView.swift` | tap a photo in the grid |
| ui:view.compare | S2 | ⬜ |  |  |
| ui:view.survey | S2 | ⬜ |  |  |
| ui:view.people | S1 | ⬜ |  |  |
| ui:view.map | S1 | ⬜ |  |  |
| ui:view.person | S1 | ⬜ |  |  |
| ui:view.faceBoxes | S1 | ⬜ |  |  |
| ui:view.reference | S2 | ⬜ |  |  |
| ui:photo.setReference | S2 | ⬜ |  |  |
| ui:compare.swap | S2 | ⬜ |  |  |
| ui:compare.set | S2 | ⬜ |  |  |
| ui:compare.makeSelect | S2 | ⬜ |  |  |
| ui:view.autoAdvance | S2 | ⬜ |  |  |
| ui:view.filmstrip | S2 | ⬜ |  |  |
| ui:view.leftPanel | S2 | ⬜ |  |  |
| ui:view.beforeAfter | S1 | ⬜ |  |  |
| ui:view.beforeAfterSplit | S2 | ⬜ |  |  |
| ui:view.beforeAfterTopBottom | S2 | ⬜ |  |  |
| ui:view.beforeAfterSplitTopBottom | S2 | ⬜ |  |  |
| ui:view.showOriginal | S1 | ⬜ |  |  |
| ui:view.zoomFit | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/LoupeView.swift` | double tap / pinch out to fit; no command yet |
| ui:view.zoom100 | S1 | ⬜ |  |  |
| ui:view.zoomToggle | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/LoupeView.swift` | double tap: 2× and back |
| ui:view.clickZoom | S1 | ⬜ |  |  |
| ui:view.navigate | S1 | ⬜ |  |  |
| ui:view.zoomIn | S1 | ⬜ |  |  |
| ui:view.zoomOut | S1 | ⬜ |  |  |
| ui:view.clipping | S1 | ⬜ |  |  |
| ui:view.visualizeHdr | S1 | ⬜ |  |  |
| ui:view.softProof | S2 | ⬜ |  |  |
| ui:view.histogram | S1 | ⬜ |  |  |
| ui:view.maskOverlay | S1 | ⬜ |  |  |
| ui:view.maskOverlayMode | S1 | ⬜ |  |  |
| ui:view.maskOverlayColor | S1 | ⬜ |  |  |
| ui:view.maskPins | S1 | ⬜ |  |  |
| ui:view.visualizeSpots | S1 | ⬜ |  |  |
| ui:view.cropOverlay | S1 | ⬜ |  |  |
| ui:view.cropOverlayOrientation | S1 | ⬜ |  |  |
| ui:view.back | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/RootView.swift` | the navigation stack |
| ui:tool.done | S1 | ⬜ |  |  |
| ui:view.filterBar | S2 | ⬜ |  |  |
| ui:local.addRoot | S2 | 🚫 |  | no Local folders in the iOS sandbox (photos are copied into the library) |
| ui:local.hide | S2 | 🚫 |  | no Local folders in the iOS sandbox |
| ui:local.restoreHidden | S2 | 🚫 |  | no Local folders in the iOS sandbox |
| ui:view.fullScreenPreview | S1 | ⬜ |  |  |
| ui:view.lightsOut | S2 | ⬜ |  |  |
| ui:view.enterFullScreen | S2 | 🚫 |  | iOS apps are always full screen |
| ui:view.infoOverlay | S1 | ⬜ |  |  |
| ui:view.navigator | S2 | ⬜ |  |  |
| ui:panel.edit | S0 | 🟡 | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/EditSheet.swift` | the Light group only (SW1.9) |
| ui:panel.profiles | S1 | ⬜ |  |  |
| ui:panel.crop | S1 | ⬜ |  |  |
| ui:panel.remove | S1 | ⬜ |  |  |
| ui:panel.masking | S1 | ⬜ |  |  |
| ui:panel.redeye | S1 | ⬜ |  |  |
| ui:panel.presets | S1 | ⬜ |  |  |
| ui:panel.info | S1 | ⬜ |  |  |
| ui:panel.keywords | S1 | ⬜ |  |  |
| ui:panel.versions | S1 | ⬜ |  |  |
| ui:panel.activity | S1 | ⬜ |  |  |
| ui:view.syncStatus | S1 | ⬜ |  |  |
| ui:dialog.syncChoice | S1 | ⬜ |  |  |
| ui:panel.close | S1 | ⬜ |  |  |
| ui:section.light | S0 | ✅ | `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/EditSheet.swift` | sliders from `develop.controls` |
| ui:section.color | S1 | ⬜ |  |  |
| ui:section.effects | S1 | ⬜ |  |  |
| ui:section.detail | S1 | ⬜ |  |  |
| ui:section.optics | S1 | ⬜ |  |  |
| ui:tool.brush | S1 | ⬜ |  |  |
| ui:tool.linear | S1 | ⬜ |  |  |
| ui:tool.radial | S1 | ⬜ |  |  |
| ui:tool.wbPicker | S1 | ⬜ |  |  |
| ui:tool.none | S1 | ⬜ |  |  |
| ui:brush.smaller | S1 | ⬜ |  |  |
| ui:brush.larger | S1 | ⬜ |  |  |
| ui:brush.featherLess | S1 | ⬜ |  |  |
| ui:brush.featherMore | S1 | ⬜ |  |  |
| ui:dialog.newAlbum | S1 | ⬜ |  |  |
| ui:dialog.newFolder | S1 | ⬜ |  |  |
| ui:dialog.smartAlbum | S1 | ⬜ |  |  |
| ui:view.photoCounts | S2 | ⬜ |  |  |
| ui:view.sidebarSection | S2 | ⬜ |  |  |
| ui:view.sidebarMoveSection | S2 | ⬜ |  |  |
| ui:view.sidebarReset | S2 | ⬜ |  |  |
| ui:view.slideshow | S2 | ⬜ |  |  |
| ui:view.secondWindow | S2 | ⬜ |  |  |
| ui:tool.keywordPainter | S2 | ⬜ |  |  |
| ui:view.gridInfo | S2 | ⬜ |  |  |
| ui:view.appearance | S1 | ⬜ |  |  |
| ui:view.theme | S2 | 🚫 |  | the system appearance (`app.appearance.*`) is the theme on iOS |
| ui:dialog.allMetadata | S1 | ⬜ |  |  |
| ui:dialog.faceModel | S1 | ⬜ |  |  |
| ui:dialog.newSmartAlbum | S1 | ⬜ |  |  |
| ui:dialog.createPreset | S1 | ⬜ |  |  |
| ui:dialog.autoStack | S2 | ⬜ |  |  |
| ui:dialog.copySettings | S1 | ⬜ |  |  |
| ui:dialog.pasteSettings | S1 | ⬜ |  |  |
| ui:view.focusSearch | S2 | ⬜ |  |  |
| ui:dialog.export | S1 | ⬜ |  |  |
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
| ui:dialog.shareLink | S1 | ⬜ |  |  |
| ui:superRes.cancel | S1 | ⬜ |  |  |
| ui:view.selectMode | S1 | ⬜ |  |  |
| ui:view.reviewMode | S1 | ⬜ |  |  |
| ui:view.filterSheet | S1 | ⬜ |  |  |
| ui:view.scrollToTop | S1 | ⬜ |  |  |
| ui:view.tagFace | S1 | ⬜ |  |  |
| ui:view.gridColumns | S1 | ⬜ |  |  |
| ui:view.gridSpan | S1 | ⬜ |  |  |
| ui:file.addPhotos | S1 | ⬜ |  |  |
| ui:file.importFromPhotos | S1 | ⬜ |  |  |
| ui:file.importFromFiles | S1 | ⬜ |  |  |
| ui:file.importFolderFromFiles | S1 | ⬜ |  |  |
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
| ui:app.settings | S1 | ⬜ |  |  |
| ui:app.newLibrary | S2 | 🚫 |  | one library in the app sandbox (`docs/ios-gaps.md` B5.2) |
| ui:app.openLibrary | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.recentLibraries | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.clearRecentLibraries | S2 | 🚫 |  | one library in the app sandbox |
| ui:app.displayProfile | S2 | 🚫 |  | iOS colour-manages the display itself |
| ui:app.about | S1 | ⬜ |  |  |
| ui:app.systemInfo | S1 | ⬜ |  |  |
| ui:app.openLogFolder | S2 | 🚫 |  | no folders to open on iOS; sharing the log is `docs/ios-gaps.md` B5.3 |
| ui:app.whatsNew | S1 | ⬜ |  |  |
| ui:dialog.cull | S2 | ⬜ |  |  |
| ui:app.help | S1 | ⬜ |  |  |
| ui:app.discord | S1 | ⬜ |  |  |
| ui:app.feedback | S1 | ⬜ |  |  |
| ui:app.website | S1 | ⬜ |  |  |
| ui:app.github | S1 | ⬜ |  |  |
| ui:app.artcraft | S1 | ⬜ |  |  |
| ui:app.shortcuts | S2 | ⬜ |  |  |
| ui:app.setShortcut | S2 | 🚫 |  | iPadOS has no per-app shortcut editor; shortcuts come from the command specs |
| ui:app.resetShortcuts | S2 | 🚫 |  | no shortcut editor on iPadOS |
| ui:app.export | S0 | 🟡 | `crates/apphost/src/export.rs`, `apps/lightcraft-ios-swift/LightCraft/Sources/LightCraftApp/AllCommandsView.swift` | runs from All Commands; the export page is SW1.4 |
| ui:app.showInFinder | S2 | 🚫 |  | no Finder on iOS |
| ui:dialog.rename | S2 | ⬜ |  |  |
| ui:dialog.labelNames | S2 | ⬜ |  |  |
| ui:dialog.captureTime | S2 | ⬜ |  |  |
| ui:photo.tagFromTracklog | S2 | ⬜ |  |  |
| ui:app.exportPrevious | S2 | ⬜ |  |  |
| ui:app.saveToPhotos | S1 | ⬜ |  |  |
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
| dialog:NewAlbum | S1 | ⬜ |  |  |
| dialog:RenameAlbum | S1 | ⬜ |  |  |
| dialog:TextPrompt | S1 | ⬜ |  |  |
| dialog:CaptureTime | S2 | ⬜ |  |  |
| dialog:Import | S1 | ⬜ |  |  |
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
| dialog:CopySettings | S1 | ⬜ |  |  |
| dialog:PasteSettings | S1 | ⬜ |  |  |
| dialog:Export | S1 | ⬜ |  |  |
| dialog:Merge | S2 | ⬜ |  |  |
| dialog:Settings | S1 | ⬜ |  |  |
| dialog:SamModel | S1 | ⬜ |  |  |
| dialog:SuperRes | S1 | ⬜ |  |  |
| dialog:ShareLink | S1 | ⬜ |  |  |
| dialog:EditIn | S2 | ⬜ |  |  |
| dialog:LightroomCatalog | S2 | ⬜ |  |  |
| dialog:ConfirmDelete | S1 | ⬜ |  |  |
| dialog:SyncChoice | S1 | ⬜ |  |  |
| dialog:RemoveFolder | S2 | ⬜ |  |  |
| dialog:SynchronizeFolder | S2 | 🚫 |  | no watched folders in the iOS sandbox |
| dialog:About | S1 | ⬜ |  |  |
| dialog:Shortcuts | S2 | ⬜ |  |  |
