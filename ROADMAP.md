# LightCraft roadmap

**Stage: alpha** · next: beta, ~14 points and ~340–600 h away

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-24 · **Change:** minor (GoPro GPR decodes (upstream #2787); pre-X FinePix and SuperCCD RAFs decode (upstream #2316); upstream merged again, up to v0.13.0 (38f8d2508): Print gains Japanese photo and postcard paper sizes (L判, 2L判, KG判, はがき, 往復はがき, 六切, 四切, 名刺); Minolta MRW decoded, 6 bodies verified; German catalog 91% → 97%; Finnish and Indonesian interfaces; earlier: checklist recount 84.3% of 554 rows after the colour mixer's fit to Lightroom exports and Calibration ✅ → 🟡; a native app moved to its own repository, so its rows and mentions are removed; Lightroom albums import in *Fork additions*; browser export no longer freezes the page: new row LR-BEHAV-WEB-EXPORT 🟡, checklist 550 rows; RAW 83% decode / 38% verified, CR3 on 26 bodies, Slideshow and Print started, full-resolution zoom closed; Next actions added in docs/roadmap.md; Sign Out and Generative Expand diffusion models in *Fork additions*) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

One-page summary of how close LightCraft is to Adobe Lightroom. Agents: read this, then pick work
from [docs/gaps.md](docs/gaps.md). The full assessment, method and evidence are in
[docs/target-app-parity.md](docs/target-app-parity.md).

## Headline numbers

| | Value | Kind |
|---|---:|---|
| **Feature breadth** (checklist) | **84.3%** of 554 rows (P0 98.0%, P1 93.9%, P2 61.2%) | measured: [parity-checklist.md](docs/parity-checklist.md) recount |
| **Feature breadth** (weighted by use) | **~77%** | estimated |
| **Ready for real work** (full target) | **~61%** (56–66%) | estimated: weighted sum over the dimensions |
| **Mainstream practitioner** | **~51%** (46–56%) | estimated |
| **Essentials user** | **~65%** (60–70%) | estimated |
| Remaining to **beta** | **~340–600 h** (Opus 5.5 agent hours, ~60% parallel) | estimated |
| Remaining to **full parity** | **~1,000–1,800 h** (~70% parallel) | estimated |

Each audience with its own hours (Opus 5.5 agent wall-clock hours to ~95%):

| Audience | Ready | Opus 5.5 agent wall-clock hours to ~95% | Work that dominates |
|---|---:|---:|---|
| Full target (ready for real work) | **~61%** | **1,000–1,800 h** (~70% parallelizes) | raw colour, lens and camera data; AI models; Classic output modules; localization; plug-in ecosystem |
| Mainstream practitioner | **~51%** | **450–840 h** (~60% parallelizes) | camera colour and render fidelity; CR3 / ORF / HE NEF; lens database; subject / sky masks; Lightroom XMP exchange; stability |
| Essentials user | **~65%** | **160–300 h** (~50% parallelizes) | default raw look for popular cameras; launch stability; basic-slider feel; opening CR3 / HEIC files people send |

The gap between breadth and readiness is quality and coverage, not missing buttons: camera colour is
estimated rather than measured, CR3 decodes on 26 verified Canon bodies (rarer CRX variants still open as previews), compressed ORF and HE NEF
open as previews, rendering is tuned by eye, AI masks are heuristics, there is no lens database, and
`main` does not yet pass the tests CI now runs on macOS, Windows and Linux (`ci.yml`). **Why alpha:** core workflows run end to end, but readiness is below
the ~75% beta bar (it is above the ~40% alpha bar) and Lightroom's main inputs, raw files, still have blocking gaps (compressed ORF and HE NEF open as previews; Canon,
~30% of raw users, decodes CR3 on 26 verified bodies). It passes the alpha gate: all six core workflows
(import, cull, develop, save and reopen, batch looks, export) work end to end on macOS
([docs/roadmap.md](docs/roadmap.md#alpha-gate)). The three readiness numbers and their weights and discounts are in
[target-app-parity.md](docs/target-app-parity.md#ready-for-real-work-mainstream-practitioner-and-essentials-user).
Hours are calibrated on this repo's PR history; see
[target-app-parity.md](docs/target-app-parity.md#how-hours-are-calibrated).

## By dimension

| Dimension | Ready | Breadth | Remaining | Doc |
|---|---:|---:|---:|---|
| Features | ~61% | ~77% (checklist 84.3%) | 600–1,100 h | [target-app-parity](docs/target-app-parity.md#features) |
| ↳ RAW cameras, colour, lens profiles | ~60% | 86% of 1,446 Adobe models decode, 40% verified | 255–480 h | [raw-parity](docs/raw-parity.md) |
| UI/UX fidelity | ~70% | ~85% | 32–54 h | [ui-parity](docs/ui-parity.md) |
| File formats (non-raw) | ~70% | ~85% | 30–55 h | [file-format-parity](docs/file-format-parity.md) |
| Hardware | ~40% | ~50% | 40–80 h | [hardware-parity](docs/hardware-parity.md) |
| Localization | ~64% of the 12 key languages | 8 of 12 present | 130–210 h (+135–239 h new scripts) | [localization-parity](docs/localization-parity.md) |
| Performance | ~70% | ~80% | 30–50 h | [target-app-parity](docs/target-app-parity.md#by-dimension) |
| Stability | ~60% | — | 30–60 h | [target-app-parity](docs/target-app-parity.md#by-dimension) |
| Platforms | ~60% | macOS, Windows, Linux, FreeBSD, web | 25–45 h | [target-app-parity](docs/target-app-parity.md#by-dimension) |
| Ecosystem / plug-ins | ~15% | ~25% | 40–80 h | [target-app-parity](docs/target-app-parity.md#by-dimension) |
| AI features | ~20% | ~30% | 120–220 h (overlaps features) | [target-app-parity](docs/target-app-parity.md#ai) |

## Features

| Area | Ready | Breadth | Remaining |
|---|---:|---:|---:|
| Import and library | 80% | 90% | 25–45 h |
| RAW decoding and camera coverage | 77% | 82% | 120–220 h |
| Colour, camera profiles, render fidelity | 35% | 65% | 80–150 h |
| Global develop adjustments | 70% | 93% | 30–60 h |
| Optics, lens profiles, crop and geometry | 50% | 80% | 40–80 h |
| Masking | 45% | 67% | 60–110 h |
| Remove, heal, Enhance | 30% | 45% | 70–130 h |
| Presets, profiles, versions, sync | 85% | 95% | 8–15 h |
| Export | 75% | 89% | 15–30 h |
| Merge and HDR | 60% | 85% | 20–35 h |
| Views, compare, zoom, culling | 70% | 90% | 15–30 h |
| Classic output modules (Map, Book, Slideshow, Print, publish, tethering) | 10% | 15% | 90–150 h |
| Video | 0% | 0% | 25–45 h |

Weights and evidence: [target-app-parity.md](docs/target-app-parity.md#features).

## Languages

Measured from `crates/ui-egui/locales` (share of 2,304 catalog messages translated; German counts 3,644 of 3,754, because its What's New lines add keys); none is `full`
while lower-layer errors stay English. Detail: [localization-parity.md](docs/localization-parity.md).

| Language | Code | Translated | Status |
|---|---|---:|---|
| English | en | 100% | full (source) |
| Simplified Chinese | zh-hans | 87% | partial |
| Spanish | es | 86% | partial |
| Hindi | hi | 0% | none (no Devanagari shaping) |
| Arabic | ar | 0% | none (no RTL / shaping) |
| French | fr | 88% | partial |
| Portuguese (Brazil) | pt-br | 85% | partial |
| Indonesian | id | 100% (What's New in English) | partial |
| Japanese | ja | 87% | partial |
| German | de | 97% (3,644/3,754) | partial |
| Korean | ko | 0% | none (no Hangul face) |
| Vietnamese | vi | 0% | none |

Also shipped: Traditional Chinese (87%), Russian (74%), Ukrainian (99%), Italian (92%), Georgian (100%), Finnish (79%).

## Upcoming

Ranked toward beta; detail in [docs/roadmap.md](docs/roadmap.md), gaps in [docs/gaps.md](docs/gaps.md).

1. Compressed ORF (8–15 h; +2.6 points of photographers, the best ratio of any raw gap)
2. Per-body decode fixes and per-model raw verification (25–45 h)
3. Tests in CI on macOS, Windows and Linux (6–12 h)
4. Render-fidelity suite against Lightroom, then tuning (30–60 h)
5. Camera colour of our own: measured calibration for the popular models (50–90 h)
6. AI masks: Subject / Sky / People (40–70 h + model decision)
7. Lens profile database of our own (40–80 h)
8. CR3 variants no public file uses, and HE NEF (30–65 h; not beta blockers any more)

Done: ~~full-resolution zoom~~ (2026-10-12), ~~CR3 for the popular Canon bodies~~ (26 verified). The step-by-step
list is *Next actions* in [docs/roadmap.md](docs/roadmap.md#next-actions).

## Fork additions

This fork adds features upstream doesn't have; they are outside the numbers above, and their rows are in
[parity-checklist.md](docs/parity-checklist.md).

| Feature | State | Missing |
|---|---|---|
| 2026-10-25 | minor | Pre-X FinePix and SuperCCD RAFs decode (upstream #2316 for #2063, raw-parity.md): 11 more Fujifilm models verified (562, 39%), 85% of Adobe's models decode; use-weighted share unchanged at ~94% (86% on a verified body); headline numbers unchanged |
| **Optional self-hosted sync** (`lightcraft-server`, [sync.md](docs/sync.md); local-first stays the default) | One library shared by a user's devices (desktop, CLI, web build, iOS app): previews and originals by content hash, field-level merges, photo folders already on the server read in place; presets, sets and LUT profiles follow the user; a library that already has photos can merge into the server's; album links (a timeline by day, a page per photo, a slideshow, Download All, an optional password, optionally a form for visitors to add photos to the album); per-user storage limits; opt-in writable folders (XMP sidecars written back, uploads filed into an imports folder); a size budget for downloaded originals, resumable uploads, small previews downloaded as the grid shows them; Sign Out waits for what is left, signs the device out and empties the library, so another account starts clean | Verification on real hardware (a physical iPhone over real TLS, a real NAS, a long-lived library); comments and likes on shared albums, accounts for other people |
| **iPhone / iPad app** (`apps/lightcraft-ios`, `crates/ios-host`; [ios.md](docs/ios.md)) | The egui app with a compact touch layout: phone grid, touch sliders, crop, Remove and Masking screens, an iOS 26 glass look, export with Save to Photos; native host (Photos / Files pickers, share sheet, HEIC through ImageIO, lifecycle, memory budget). Checked on the simulator, partly on an iPhone | Most of the host on a device; Lightroom-mobile features ([ios-gaps.md](docs/ios-gaps.md)) |
| **Map view and place search** (`crates/geo`, [map.md](docs/map.md)) | Desktop, web and iOS: clustered thumbnail markers, an offline world map plus OpenStreetMap tiles, place search (`madrid`, `photos in lisbon in june 2024`) and the place in Info, offline from GeoNames | Not yet run in a browser or on a device; drag-to-geotag, saved locations, a satellite layer |
| **Hidden album and possibly sensitive photos** ([hidden.md](docs/hidden.md)) | Hide / Unhide like Apple Photos: hidden photos leave every view until Hidden is unlocked (a synced password), Show Hidden mixes them back in, syncs through the server; an opt-in NSFW classifier (desktop and server) lists photos that might be sensitive, with Hide All and a grid blur | The classifier's cut-off and accuracy unmeasured; no "not sensitive" mark |
| **Search by description, text in photos, People** (`crates/vision`, [search-people.md](docs/search-people.md)) | SigLIP 2, PP-OCRv6 and YuNet + SFace, each an opt-in consented download, on the desktop and on the sync server | Measured on a handful of images only (face grouping accuracy, OCR at about a second a photo, no relevance cut-off); a device-side model on iOS |
| **Super Resolution** (`crates/enhance`, [super-resolution.md](docs/super-resolution.md)) | Nomos Uni SPAN 2× (optional download) on desktop and iPhone; every optional model listed, downloaded and deleted in Settings → AI Models ([ai-models.md](docs/ai-models.md)) | Enlarges the rendered photo, not the raw data; no preview in the dialog |
| **Content-aware Remove by patch synthesis** (M8.4) and **panorama Fill Edges** (M13) | Remove fills from synthesized texture seeded by the spot's source; a stitched panorama's empty corners are filled with continued texture | Rows or tiles in strong perspective can come out at the wrong angle |
| **AI Remove** (`crates/erase`, [ai-remove.md](docs/ai-remove.md)) | Remove ▸ Generative AI: MI-GAN (MIT, optional download, pinned SHA-256) on rten fills a stroke on desktop, iPhone and the web; the fill is stored with the spot, so renders and exports never run the model | Not run on an iPhone yet |
| **Lens Blur and depth** (`crates/depth`, `crates/pipeline/src/lensblur.rs`, [lens-blur.md](docs/lens-blur.md)) | Edit ▸ Lens Blur and Depth Range masks from the photo's own depth (iPhone Portrait HEIC / JPEG, DNG 1.5) or a Depth Anything V2 Small estimate (Apache-2.0, optional 27 MB download, stored with the settings); bokeh shapes, Boost, Focus / Blur / Erase refine brushes; desktop, phone and browser | Portrait depth checked on Apple-written synthetic files only; not run on a device or in a browser yet |
| **Generative Expand on the device** (`crates/erase`, `crates/diffuse`, [generative-expand.md](docs/generative-expand.md)) | Crop ▸ Expand: the frame reaches past the photo and Generate paints the new area with MI-GAN (every device), LaMa (Apache-2.0, 208 MB), Stable Diffusion 1.5 Inpainting (2.1 GB, also capable phones), Kandinsky 2.2 Inpaint (Apache-2.0, 9 GB) or SDXL Inpainting (6.9 GB), offered by the device's profile with an Automatic default (Settings → AI Models), or Classic (no download), in the background; stored with the settings | The diffusion models take minutes on a CPU, not timed on a GPU yet; no variations or prompt; not run on a phone yet |
| **Lightroom albums import** (`crates/engine/src/lightroom_albums.rs`, `lightroom_cloud.rs`, [lightroom-catalog-import.md](docs/lightroom-catalog-import.md#lightroom-albums-albums-only)) | A Lightroom (cloud) library's albums and folders (read from the desktop app's local `Managed Catalog.mcat`), or a Classic catalog's collections, become albums holding the photos the library already has, matched by file identity; desktop, CLI; synced to the library's other devices | Phones without a computer can't reach Lightroom mobile's data; photos only in Lightroom are counted, not imported |

Next for the fork: sync and the iOS app verified on real hardware (Lens Blur on an iPhone's Portrait photos among
it); then sharing between people with accounts (comments, likes, shared libraries).

## Progress log

- 2026-10-24: Upstream merged again (up to v0.13.0, 38f8d2508; 675 commits). From upstream: Print gains the Japanese photo and
  postcard papers, Minolta MRW decodes, German catalog 97%, Finnish and Indonesian interfaces, a shared CI gate workflow,
  `cargo xtask ci --quick`. This fork keeps its sync server, Generative Expand and Sign Out ahead of it.
- 2026-10-23: GoPro GPR (VC-5) decodes from sensor data (`crates/raw/src/vc5.rs`, ported from GoPro's
  Apache-2.0 / MIT GPR SDK); FUSION, HERO5, HERO6, HERO7 and HERO10 Black verified on CC0 files. 556 models
  verified, 28 unsupported (was 37); 84% of Adobe's models decode (was 83%); use-weighted shares unchanged.
- 2026-10-22: Print page setup and PDF output gain the Japanese photo and postcard papers — L判
  (89 × 127 mm), 2L判 (127 × 178), KG判 (102 × 152), はがき (100 × 148), 往復はがき (148 × 200),
  六切 (203 × 254), 四切 (254 × 305) and 名刺 (55 × 91) — at their exact metric sizes. The Print dialog
  and `print.set` take their customary names, and names are matched case-, space- and
  punctuation-insensitively (`L判`, `l`, `L-size` are one paper).
- 2026-10-15: Upstream merged again (up to 0da998f2, v0.8.0; 699 commits). From upstream: 545 verified raw
  models (~85% of photographers on a verified body), CR3 on 26 verified Canon bodies, full-resolution zoom closed,
  the Slideshow settings / playback / PDF, JPEG and AV1 export, the Print dialog, a user manual under `docs/user/`, Italian, Georgian, Czech and
  Polish interfaces. This fork keeps its sync server, Generative Expand and Sign Out ahead of it. The roadmap's
  current focus is re-ranked and a *Next actions* list is added ([docs/roadmap.md](docs/roadmap.md#next-actions)).
- 2026-10-13: **Generative Expand on the device** ([generative-expand.md](docs/generative-expand.md)): Crop ▸ Expand
  lets the frame reach past the photo (an aspect ratio adds canvas around the whole photo), and Generate paints the
  new area with MI-GAN (MIT, every device), LaMa (Apache-2.0, 208 MB, computers only) or Classic (no download), in
  the background with a row in the activity stack; the fill is stored with the settings so renders and exports never
  run a model. One whole-canvas network pass gives the structure and patch synthesis the photo's grain; the phone's
  crop screen has Lightroom mobile's Expand switch, Generate button and model line. LR-CROP-GENEXPAND OOS → P2 🟡.
- 2026-10-13: The sync server's album links grow into shared albums, as photo services have them: Share Link… asks
  when the link ends, whether people may download the originals or **add their own photos** (they become the owner's,
  in the album, on every device), and for an optional **password**; the page goes by day, opens each photo large
  (← →), plays a slideshow and offers Download All (a streamed ZIP). Admins can give each user a **storage limit**
  ([sync.md](docs/sync.md#share-an-album)).
- 2026-10-14: Camera-coverage sweep verifies 30 more models on 40 CC0 files, with file and sensor
  hashes pinned (`docs/raw/coverage.py`: 515 → 545 verified). Eleven candidates excluded after rechecking
  identity, provenance and camera-image agreement. Phone and drone DNG tone differences remain tracked in #1277.
- 2026-10-18: Minolta / Konica Minolta MRW decodes from sensor data (12-bit packed and 16-bit words,
  RGGB and GBRG, as-shot WB from `WBG`); DiMAGE 7, 7Hi, A1, A2, A200 and Dynax / Maxxum 7D verified on
  CC0 files. 551 models verified, 201 preview-only; headline numbers unchanged.
- 2026-10-14: 97 Canon CR2, Pentax and Ricoh models verified, one CC0 raw.pixls.us file each pinned and
  checked against the camera JPEG (`crates/raw/tests/it/canon_pentax_corpus.rs`). 334 models verified, ~50% of
  photographers on a verified body; headline numbers unchanged.
- 2026-10-12: Mamiya MEF (ZD) decodes from sensor data and is imported; verified on a CC0 file. 241 models
  verified, 35 unsupported; headline numbers unchanged.
- 2026-10-12: Epson ERF (R-D1, R-D1s, R-D1x) decodes from sensor data, white balance measured from each
  file's preview; 3 bodies verified on CC0 files. 240 models verified; headline numbers unchanged.
- 2026-10-11: Leica Q3 verified by a recorded check of 38 private DNGs (all decode and develop at full
  size; mean ΔE to the camera JPEG 6.3–13.9). 236 models verified; headline numbers unchanged.
- 2026-10-11 (CI): `ci.yml` runs `cargo xtask ci` on Linux and clippy and the tests on macOS and Windows, and the CPU ↔ GPU equivalence tests on lavapipe, Metal and WARP (`LIGHTCRAFT_GPU_ALLOW_SOFTWARE`, tests only), within the documented bounds on all three (max 1 LSB on lavapipe and WARP, 2 in one case on the runner's Metal; the bound is 3). `main` fails there until the #989 fixes and #1052 (Windows) land (#1050). Headline numbers unchanged.
- 2026-10-12: 22 more Canon CR3 bodies verified (EOS R, RP, R3, R5, R6, R6 Mark II / III, R7, R10, R50,
  R50 V, 90D, 250D, 850D, M200, M50 Mark II, M6 Mark II, 1D X Mark III, PowerShot G5 X II, G7 X III, SX70,
  V1): one CC0 file each pinned and checked against the camera JPEG; all 110 CC0 CR3s on raw.pixls.us
  decode. The EOS R6 Mark III gets its as-shot white balance (`ColorData` 66). 259 models verified; RAW
  decoding 77% → 83%; mainstream ~49% → ~51%, essentials ~64% → ~65%, full ~61%.
- 2026-10-12: 117 Nikon and Fujifilm bodies verified, one CC0 raw.pixls.us file each pinned and checked
  against the camera JPEG (`crates/raw/tests/it/body_corpus.rs`); the sweep found COOLPIX 5700 / 8400 / 8800
  decoding wrongly. 354 models verified, ~58% of photographers on a verified body; headline numbers unchanged.
- 2026-10-11: Canon EOS R5 Mark II C-RAW decodes from sensor data (57% of its files opened as a
  preview): the CRX QP map's first-symbol literal and limited-length escape; four CC0 R5 Mark II files
  pinned, the QP maps of 33,665 private C-RAW files decode exactly ([cr3.md](docs/cr3.md)). 235 models
  verified; RAW decoding 76% → 77%; headline numbers unchanged.
- **2026-10-10 (UI docking):** Library, Tools, and Presets panels can be regrouped, split, floated within the app window, closed, and reopened. Tools keeps the current Edit/Crop/Masking mode; the photo view and filmstrip stay together. Panel moves and reset are available as UI commands, and saved UI preferences retain arrangements. Widget state styling uses the shared `craft-ui` theme helper, retaining the dark palette, typography, spacing and per-state borders. No readiness percentage change.
- 2026-10-10 (RAW): RAW measured model by model against Adobe's 1,446-model list (234 verified, 943
  unverified, 233 preview-only, 36 unsupported; ~84% of photographers' cameras decode, ~40% on a
  verified body). Full ~61%, mainstream ~49%, essentials ~64%; beta ~14 points away.
- 2026-10-10 (final): full ready is a weighted sum over the dimensions (~59%; method aligned with
  the standard); mainstream practitioner ~51% (450–840 h), essentials user ~63% (160–300 h);
  alpha gate passes.
- 2026-10-10: full re-measure for the progress-docs standard: stage alpha, ready ~55%, checklist
  81.1%; new docs (target-app-parity, gaps, roadmap, architecture, raw / file-format / hardware / UI
  / localization parity); `docs/parity.md` renamed `docs/parity-checklist.md`. Landed today: Nikon
  lossy-after-split NEF (#633), ORF 12.8 bpp (#651), HDR gain map JPEG / PQ AVIF / float TIFF export
  (#646), Lightroom-like Auto (#628), activity stack.
- 2026-10-11: Sony ILCA-99M2 (α99 II) camera colour: bundled profile from 144 photos; on 30 withheld photos
  mean ΔE vs the camera JPEG 5.11 → 4.87, 25 of them closer.
- 2026-10-09: Sony ILCE-7CR camera colour: bundled profile from 125 photos; on 26 withheld photos mean
  ΔE vs the camera JPEG 4.07 → 3.29. HEIC in release builds (#609), Samsung uncompressed SRW,
  contact-sheet PDF export, Ukrainian and French UI, release v0.5.0.
- 2026-10-08: independent Rust CR3 lossless and version 0x100/0x200 C-RAW decoding, six exact
  full-sensor regressions on M50 / R100 / R8; guarded CR3 camera-JPEG colour fitting
  ([cr3.md](docs/cr3.md)).
- 2026-10-08: Fujifilm lossless / lossy compressed RAF: pure-Rust striped decoder, full sensor arrays
  verified on 17 CC0 and 7 supplied files (13 bodies). Fujifilm camera colour: bundled X-H2S / X-T4
  profiles from 201 / 545 photos; on 150 withheld files mean ΔE X-H2S 1.70 → 1.62, X-T4 2.43 → 2.34.
  AI RAW denoise (#384), Lightroom catalog import, German, Spanish and Russian UI.
- 2026-10-07: Panasonic RW2 / Leica RWL / Panasonic RAW decode in every raw format, established on
  178 CC0 files from 118 bodies; RW2 file-local camera look (140 of 174 files). Faces: recognition in
  pure Rust and People view.
- 2026-10-03 – 10-05: community PRs (#42–#51, #60) and 25+ issues: compressed NEF, Move import,
  folder templates, Local roots and cleanup, 85k-photo grid and catalog performance, background
  compaction, failed-save errors, GPU export hardening. Added the honest *Where we stand* assessment
  and tracker rows for camera colour, camera coverage and render fidelity.
- 2026-10-02: parity estimate added (`cargo xtask parity` prints weighted completion); milestones
  refreshed.
- 2026-10-01: RAW II formats: RAF (uncompressed Bayer + X-Trans), RW2 (packed), PEF (incl. Huffman),
  ORF (uncompressed); embedded previews for every container incl. CR3; raw corpus test with 37 CC0
  samples.
- 2026-09-30 (later): app running with the full Lightroom-style UI; pipeline v0; DNG / CR2 / ARW;
  README showcase. ≈ 8 h elapsed.
- 2026-09-30: roadmap created; M0 in progress; research docs (Lightroom reference, Rust imaging
  ecosystem) complete.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-25 | minor | Camera support from community pull requests integrated (X-Trans III Kelvin #2514, Blackmagic DNG #1305, two-field Coolpix #672, pre-X FinePix #2316, GoPro GPR #2787) and the Olympus family work (ORI files, five more uncompressed bodies verified, ORF preview details, aspect crop); Coolpix 5700, 8400 and 8800 move from unsupported to verified; RAW: 86% of Adobe's models decode, 40% verified (575), use-weighted share unchanged at ~94% (86% on a verified body); Canon CRW (#2512) and Nikon HE (#896) held back on clean-room provenance |
| 2026-10-25 | minor | Upstream #2787: GoPro GPR (VC-5) decoded: RAW 83% → 84% of Adobe's models decode, 38% verified; readiness unchanged |
| 2026-10-24 | minor | Upstream merged again (up to v0.13.0); headline numbers are recounted after the merge |
| 2026-10-22 | minor | Print: Japanese photo and postcard paper sizes (L判, 2L判, KG判, はがき, 往復はがき, 六切, 四切, 名刺) in the page setup, Print dialog and PDF output; headline numbers unchanged |
| 2026-10-19 | minor | Indonesian interface (`id`): every plain and formats message, plus 69 command labels, locale, menu command and tests; What's New left in English; Localization ~55% → ~64% (mean of the 12 key languages), 8 of 12 present |
| 2026-10-20 | minor | Checklist recount: 84.3% of 554 rows (P0 98.0%, P1 93.9%, P2 61.2%); Calibration ✅ → 🟡 (unlike Lightroom's), the colour mixer fitted to Lightroom single-slider exports |
| 2026-10-19 | minor | Removed the mentions of a native app that moved to its own repository; headline numbers unchanged |
| 2026-10-16 | minor | Browser build: Export no longer runs on the page's thread (each rendered photo is made on a Web Worker, ✕ stops it at once): new checklist row LR-BEHAV-WEB-EXPORT 🟡, 549 → 550 rows, P1 94.8% → 94.5%; other numbers unchanged |
| 2026-10-16 | minor | Fork additions: Lightroom albums import (a Lightroom cloud library's or a Classic catalog's albums, holding the photos already in the library) |
| 2026-10-15 | minor | Upstream merged again (up to 0da998f2); Upcoming re-ranked, *Next actions* added in docs/roadmap.md; numbers as upstream's |
| 2026-10-15 | minor | Sync: Sign Out waits until everything has reached the server, then removes the account's library from the device (as Lightroom's does), so two accounts' libraries never meet on a device; *Fork additions* row updated; numbers unchanged |
| 2026-10-14 | minor | Generative Expand: Stable Diffusion 1.5, Kandinsky 2.2 and SDXL inpainting, offered by the device's profile; *Fork additions* row updated; numbers unchanged |
| 2026-10-14 | minor | Localization ~52% → ~55% (mean of the 12 key languages): de, es, fr, ja and uk catalogs filled |
| 2026-10-16 | minor | German 91% → 97% (217 interface strings and 862 What's New lines added); headline numbers unchanged |
| 2026-10-14 | minor | 21 Sony and Leica models verified (258 models, 18%); headline numbers unchanged |
| 2026-10-14 | minor | 17 uncompressed Olympus and Samsung models verified on one CC0 file each (254 models verified, 18%); headline numbers unchanged |
| 2026-10-14 | minor | Full-resolution zoom (gap 4) closed: soft proofing gets zoom windows; headline numbers unchanged |
| 2026-10-14 | minor | Camera-coverage sweep (#1327): 30 models verified on 40 CC0 files (coverage.py 515 → 545 verified); sensor verification does not establish colour parity (#1277); headline numbers unchanged |
| 2026-10-14 | minor | 97 Canon CR2, Pentax and Ricoh models verified (334 models, 23%); headline numbers unchanged |
| 2026-10-14 | minor | RAW: 82% of Adobe's models decode, 36% verified (integration batch 36's CC0 per-model checks) |
| 2026-10-14 | minor | Lens Blur and depth added to *Fork additions* (LR-EDIT-LENSBLUR ⬜ → 🟡, LR-MASK-DEPTHRANGE ⬜ → ✅); *Next* no longer lists depth; numbers unchanged |
| 2026-10-13 | minor | Generative Expand on the device (MI-GAN / LaMa / Classic) added to *Fork additions*; LR-CROP-GENEXPAND OOS → P2 🟡; numbers unchanged |
| 2026-10-13 | minor | Upstream merged into this fork again (up to b3d392b4: Leica Q3 verified, 236 models; accessibility audit); numbers as upstream's |
| 2026-10-13 | minor | Sync server: album links with passwords, guest uploads, Download All, photo pages and a slideshow; per-user storage limits; *Fork additions* row and *Next* updated; numbers unchanged |
| 2026-10-13 | minor | Sony ILCA-99M2 camera colour profile in the log; numbers unchanged |
| 2026-10-12 | minor | Upstream merged into this fork; *Fork additions* lists the fork-only features (sync, iOS app, map, search / People, Super Resolution, patch-synthesis Remove); numbers unchanged |
| 2026-10-12 | minor | Italian interface (`it`, #1192): 2,201 of 2,398 messages, locale, menu command and tests; headline numbers unchanged |
| 2026-10-12 | minor | UI/UX remaining 34–58 h → 32–54 h (canvas tools by keyboard); headline numbers unchanged |
| 2026-10-12 | minor | Mamiya MEF decoded and imported: ZD verified (241); headline numbers unchanged |
| 2026-10-12 | minor | Epson ERF decoded: 3 more verified models (240); headline numbers unchanged |
| 2026-10-12 | minor | Georgian (`ka`) interface language added (every catalog message) |
| 2026-10-17 | minor | Finnish (`fi`) interface language added: 2,610 of 3,308 messages (79%), locale, menu command and tests; headline numbers unchanged |
| 2026-10-12 | minor | UI/UX remaining 40–70 h → 34–58 h (accessibility audit landed); headline numbers unchanged |
| 2026-10-11 | minor | Tests in CI on macOS, Windows and Linux (`ci.yml`, #1050), GPU equivalence tests on software adapters; not green on `main` yet, gap 9 6–12 h → 2–4 h; headline numbers unchanged |
| 2026-10-18 | minor | Minolta MRW decodes from sensor data; 6 Konica Minolta bodies verified; headline numbers unchanged |
| 2026-10-12 | minor | 22 more CR3 bodies verified, EOS R6 Mark III white balance: RAW decoding 77% → 83%, RAW ~56% → ~60%; mainstream 49% → 51%, essentials 64% → 65%; full unchanged at ~61% |
| 2026-10-12 | minor | 117 Nikon and Fujifilm bodies verified (354 models, 24%); headline numbers unchanged |
| 2026-10-11 | minor | EOS R5 Mark II CR3 verified: CR3 on four verified bodies, RAW decoding 76% → 77%; headline numbers unchanged |
| 2026-10-10 | minor | RAW measured model by model; RAW decode and colour as explicit rows: full 59% → 61%, mainstream 51% → 49%, essentials 63% → 64%; beta ~14 points |
| 2026-10-10 | minor | Full ready restored to the additive weighted sum (~59%; method aligned with the standard, no new evidence); hours per audience; beta ~16 points away |
| 2026-10-10 | minor | Mainstream practitioner and essentials user numbers; full ready 55% → 46% (method written down); beta 340–600 h |
| 2026-10-10 | minor | Alpha gate added (core-workflow check): passes, stays alpha |
| 2026-10-10 | major | Restructured to the progress-docs standard; re-measured; *Where we stand*, *Parity estimate* and *Totals* moved to docs/target-app-parity.md, milestones and risks to docs/roadmap.md, raw coverage to docs/raw-parity.md, catalog migration to docs/file-format-parity.md, CJK notes to docs/localization-parity.md |
| 2026-10-08 | minor | *Where we stand* by dimension and by kind of user |
| 2026-10-02 | major | Parity estimate and effort table |
| 2026-09-30 | major | Created |
