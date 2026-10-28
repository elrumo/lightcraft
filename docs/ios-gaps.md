# iOS: what is missing

What [`ios.md`](ios.md) does not do yet, ranked by how much a person with an iPhone would miss it. `B` marks what
blocks a first beta. Nothing here is a regression on the desktop: these are touch-layout and host gaps.

## A. Gaps in what exists

| # | Gap | Notes |
|---|---|---|
| A1 `B` | **Run it.** This tree has been type-checked (`cargo xtask ios`) and tested headless, but not launched on a simulator or a device. | The pieces ran in an earlier tree; expect small fixes in the host (the swizzles in `SceneDelegate.m`, the keyboard, the pickers) |
| A2 `B` | **Settings on a phone** is the desktop dialog (a sidebar of tabs and a section) inside a page. | Wants iOS's pattern: a list of sections, each pushed as a page with a back button; choices as pull-down menus, on / off as switches |
| A3 `B` | **Export on a phone** is the desktop dialog's body inside a page; rows such as Format and Color space run past a 390 pt screen. | Wants a flow of its own: the share sheet first, the options on a page, More Options below it |
| A4 | **People** has no phone layout: the People view is the desktop one, and a face on the photo can't be tapped to name it. | Round faces three across, a naming page, a person's page. Needs the engine's naming commands (`faces.*`) to cover a face region added by hand |
| A5 | **The photo** has no carousel (neighbours alongside while a finger carries it), no pull-down-to-the-grid with the grid behind, and no hold-for-Before. | A swipe changes photo and a pull down goes back, but without the interactive animation |
| A6 | **Filter page** has no Location list. | The catalog filter has no place criterion to list |
| A7 | **Camera and card reader import** (ImageCaptureCore, a cameras list in the import review). | The grid's + offers Photos, Files and a folder only. Written separately (`crates/capture`); not part of this branch |
| A8 | **iPad**: held sideways it gets the desktop layout; multi-window, Stage Manager, pointer hover and keyboard shortcuts are untested. | |
| A9 | **Accessibility**: the phone widgets announce themselves to AccessKit like the desktop's, but VoiceOver on iOS has not been tried. | |
| A10 | **Localisation** of the phone's own strings (the catalogs fall back to English). | |

## B. Lightroom mobile features LightCraft does not have

Not iOS-specific unless noted; see [`gaps.md`](gaps.md) and [`parity-checklist.md`](parity-checklist.md) for the whole
list.

- **Capture**: an in-app camera (HDR, raw, ProRAW), and Live Photos as a photo with a motion part.
- **Cloud**: Lightroom's cloud, Adobe-style collaboration, and iCloud as a place for the library. The self-hosted
  server ([sync.md](sync.md)) is the way to share a library between devices.
- **Video**: trimming and editing clips; the library shows photos only.
- **AI**: Generative Remove / Expand, AI keywording and search by description. Select Subject / Sky / Object masks
  work through the same model the desktop uses (downloaded when the user agrees).
- **Library**: hidden photos.

## Suggested order

A1 first (nothing else can be judged until it runs on a phone), then A2 and A3 (the two dialogs every user opens),
then A5 and A4.
