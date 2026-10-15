# Roadmap: milestones, current focus and what's next

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** minor (current focus re-ranked after merging upstream up to 0da998f2: full-resolution zoom closed, 26 CR3 bodies and 545 models verified, Slideshow and Print started; Next actions added) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

Forward-looking plan. The summary with the headline numbers is [ROADMAP.md](../ROADMAP.md); the
ranked shortfalls are [gaps.md](gaps.md); the numbers come from [target-app-parity.md](target-app-parity.md).
LightCraft has no `cargo xtask scorecard`; the measured checklist is
[parity-checklist.md](parity-checklist.md) (`cargo xtask parity`).

## Current focus

Toward **beta** (~340–600 Opus 5.5 agent hours; ranked by photographers unlocked or users unblocked per hour,
each a gap in [gaps.md](gaps.md); the step-by-step list is [Next actions](#next-actions)):

1. **Compressed ORF** (gap 6, 8–15 h): +2.6 points of photographers, the best ratio of any raw gap.
2. **Per-body decode fixes and verification** (gap 10, 25–45 h): 545 of 1,446 models verified; the cheap
   fixes (#769, #535, #702) are left.
3. **Tests in CI on macOS, Windows and Linux** (gap 9, 6–12 h).
4. **Render-fidelity suite** (gap 3, 30–60 h): score our output against Lightroom's on the same CC0
   raws (references local in `plan/`), then tune; a retune ships as a new process version.
5. **Camera colour of our own** (gap 1, 50–90 h): chart-based calibration for the most-used models,
   building on the per-file JPEG fits and pooled profiles (`lightcraft-cli calibrate`); never Adobe
   data.
6. **AI masks** (gap 5, 40–70 h): Subject / Sky / People on licensable weights; configure the SAM 3
   mirror. Needs the maintainer's model decision.
7. **Lens profiles of our own** (gap 8, 40–80 h).
8. **CR3 variants no public file uses and HE NEF** (gaps 2 and 7, 30–65 h): not beta blockers any more;
   the CRX variants wait for user files, HE NEF needs a JPEG XS decoder.

Done since the last review: full-resolution zoom (gap 4, 2026-10-12), CR3 on 26 verified bodies (every CC0
CR3 on raw.pixls.us decodes), the per-model sweeps of Nikon, Fujifilm, Canon CR2, Pentax, Sony and Leica
bodies, Slideshow settings, playback and PDF / JPEG / AV1 export, the Print dialog.

## Next actions

Concrete, in the order to take them. Hours are the gap's estimate from [gaps.md](gaps.md).

**A. Keep the docs true (1 h, after each upstream merge)**
- A1. Run `cargo xtask parity --write`, then copy the new row counts into [ROADMAP.md](../ROADMAP.md) and
  [target-app-parity.md](target-app-parity.md); add a progress-log line and bump each doc's *Last updated*.
- A2. Re-read this file's Current focus and Alpha gate against [gaps.md](gaps.md): strike what closed.

**B. Beta track**
1. Compressed ORF (gap 6): decode from the two Olympus patents cited in [raw-parity.md](raw-parity.md), in
   `crates/raw/src/orf.rs`. Done when the 66 preview-only Olympus / OM models decode and one pinned CC0
   file per body passes the camera-JPEG check.
2. Cheap per-body fixes (gap 10): land PR #671 (Z 6II packed 14-bit, #769), Sony A200 to A390 / A850 12-bit
   ARW (#535), the Sony A100 / F828 regression (#702) and pre-X FinePix RAF, each with a pinned sample test.
3. CI on macOS, Windows and Linux (gap 9): a matrix job running `cargo test --workspace` with the
   craft-fonts input; keep FreeBSD. Fix the load-sensitive tests first so the new jobs don't flake.
4. Render-fidelity suite (gap 3): an xtask that renders CC0 raws and scores them against local-only
   Lightroom references in `plan/` (never committed); then tune per control and ship process version V2.
5. Camera colour (gap 1): extend `lightcraft-cli calibrate` to chart-based fits, starting with the ten most
   used bodies in `docs/raw/usage-weights.tsv`; this needs chart shots from people who own them.
6. AI masks (gap 5): the maintainer picks permissively licensed segmentation weights; inference runs on the
   rten stack the fork already uses for MI-GAN and Depth Anything. Apple Vision selection (iOS, Mac) and Lens
   Blur depth can feed Subject and Depth Range masks meanwhile.
7. Lens profiles (gap 8): generalise the in-file corrections already read (Sony ILCE-7RM4A / 7RM6, Nikon Z,
   DNG opcodes, RW2) into one profile format; never Adobe LCPs.
8. Core-path user issues: triage the crash and data-loss ones first (never-crash rule); 40 to 80 h.

**C. Fork track** (outside the beta numbers; see *Fork additions* in [ROADMAP.md](../ROADMAP.md))
- C1. Verify on real hardware: the sync server with a physical iPhone over real TLS, a real NAS and a
  long-lived library; the iOS host on a device; Map, Search / People and AI Remove on a device.
- C2. Triage the branches `git branch --no-merged main` lists: check each with `git cherry main <branch>`
  (some landed through sweep commits), then merge or delete.
- C3. Depth: land Lens Blur and wire Depth Range into the masking parity rows.
- C4. SwiftUI app: the Mac app's SM3 / SM4 and a PARITY.md row for every new egui command.
- C5. Sharing between people: comments, likes and accounts for other people on the sync server.

## Alpha gate

The target's core workflows: what a typical Lightroom user does every day. Alpha requires each to
work end to end on the main platform (macOS) and the work to save and reopen. Checked on 2026-10-10
against the code, tests, issues and [gaps.md](gaps.md).

| Workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---:|
| 1. Import a shoot (raw + JPEG) from a card or folder into the library | yes | Add / Copy / Move import with templates, duplicates, devices with DCIM, background import with cancel (`crates/engine/src/import.rs`, `tests_import.rs`, `tests_import_move.rs`); checklist A 83% | 0 |
| 2. Cull: rate, flag, label, compare / survey, filter, check focus | yes | ratings / flags / labels with advance, Compare, Survey ≤ 48, filter bar, smart albums (checklist B–D 88–92%); 100% zoom shows original pixels in Detail, Compare, Reference, Before / After, mask overlays and soft proofing (gap 4, closed 2026-10-12) | 0 |
| 3. Develop raws non-destructively (global sliders, crop, masks, heal) | yes for most cameras (partial: compressed ORF and HE NEF open as embedded previews, and so do the rarer CR3 variants) | Sony, Nikon (except HE), Fujifilm, Panasonic, Pentax, CR2, CR3 (26 verified bodies), DNG decode from sensor data (raw-parity.md); every Edit slider, crop / Upright, brush / gradient / range masks, heal / clone (checklist F, H, K); colour estimated per file rather than measured, gaps 1 and 3 | 0 (colour and coverage gaps are beta blockers 1, 3, 6) |
| 4. Save, quit and reopen the work | yes | crash-safe journal + snapshots, torn-append and crash tests (`crates/catalog/src/journal.rs`, `tests_journal.rs`, `tests_torn_append.rs`); XMP sidecars read / write; failed saves reported | 0 |
| 5. Batch-apply a look: presets, copy / paste / sync settings | yes | presets (incl. `.xmp` / `.lrtemplate` import), copy / paste / sync, auto sync (checklist L 86%, N 100%) | 0 |
| 6. Export deliverables (JPEG / TIFF, size, sharpening, metadata, watermark, naming) | yes | one encoder for app, CLI and MCP (`crates/engine/src/export.rs`, `tests_export.rs`); checklist S 89% | 0 |

**Result: passes.** Every core workflow runs end to end on macOS and the work survives quit and
reopen; the partial rows degrade fidelity for some cameras but don't stop the
workflow, so LightCraft stays **alpha** (ready for real work ~61%, above the ~40% bar).

## After beta

Enhance (Raw Details, Super Resolution, X-Trans denoise), content-aware remove, HDR display, the
Classic output modules (Map and the Slideshow module view, Book, publish services; Print and Slideshow export exist), tethering, video,
plug-in ecosystem, localization to `full` and new scripts (RTL, shaping). Estimates in
[gaps.md](gaps.md) entries 11–28; ~500–1,300 h after beta.

## Milestones

Original wall-clock estimates (made before work started, 4–6 parallel agents) are kept for
calibration: M0–M13 took ≈ 25 active hours against an estimate of ≈ 110–170 h.

**Status legend:** ✅ done · 🚧 in progress · ⬜ not started

| # | Milestone | Scope (summary) | Estimate (h) | Status |
|---|---|---|---|---|
| M0 | Skeleton + visual shell | workspace, xtask CI + layering, geom/color/raster, develop model, pipeline v0, catalog v0, engine commands, Lightroom-look UI (grid, loupe, filmstrip, Edit panel), control channel, MCP, web build | 3–5 | ✅ |
| M1 | Library core | import (JPEG/PNG/TIFF/WebP), EXIF/XMP, persistent catalog (op log + snapshots), albums, ratings/flags/labels, filter/search/sort, thumbnail cache, 100k-photo grid | 6–10 | ✅ (100k-photo grid scale test pending) |
| M2 | Pipeline v1 (quality) | WB temp/tint, profiles, local tone mapping (highlights/shadows), curves, HSL, point colour, colour grading, texture/clarity/dehaze, vignette, grain, B&W, auto tone/WB, histogram, before/after | 10–15 | ✅ (look tuning vs our references ongoing) |
| M3 | RAW I | TIFF/DNG (LJ92, deflate, tiles, opcodes), demosaic (AHD/PPG/bilinear), highlight recovery, DNG colour model, CR2, NEF, ARW, embedded previews | 10–15 | ✅ (DNG, CR2 incl. sRAW, ARW, NEF uncompressed / lossless / lossy / lossy after split, embedded previews) |
| M4 | Crop, geometry, optics | crop tool + overlays, straighten, Upright (auto/level/vertical/full/guided), manual transforms, CA, defringe, manual lens corrections | 6–10 | ✅ |
| M5 | Performance | source pyramids, wgpu compute pipeline (CPU oracle), draft/full renders, prefetch, budgets (16 ms slider updates on 24 MP) | 10–15 | 🚧 (stage cache, source pyramid, wgpu pipeline, prefetch, memory budget ✅; colour NR at half resolution, GPU histogram ⬜) |
| M6 | Masking | brush, linear/radial gradients, colour/luminance/depth range, add/subtract/intersect/invert, all local adjustments, masks panel | 8–12 | 🚧 (brush/linear/radial/colour/luminance range, add/subtract/intersect, masks panel ✅; depth range, AI masks ⬜) |
| M7 | Detail | sharpening + masking preview, luminance/colour NR, Denoise, Raw Details, Super Resolution | 6–10 | 🚧 (sharpening, luminance/colour NR ✅; AI Denoise 🟡 (Bayer, optional weights; policy/fidelity open); Super Resolution 🟡 (fork: Nomos Uni SPAN 2×, optional download); Raw Details ⬜) |
| M8 | Heal / Remove | content-aware remove (PatchMatch), heal, clone, brush spots, visualize spots, red/pet eye | 6–10 | 🚧 (heal, clone, auto source, visualize spots, red/pet eye ✅; content-aware remove 🟡 (patch synthesis; MI-GAN generative fill, fork)) |
| M9 | Presets, profiles, versions, sync | preset browser + amount, create/import presets, profile browser, versions, history, copy/paste/sync settings | 5–8 | ✅ |
| M10 | Export & share | export dialog (JPEG/PNG/TIFF/DNG/AVIF/JXL/original), sizing, sharpening, metadata, watermark, naming, batch jobs, XMP sidecars, HDR export | 6–10 | 🚧 (all formats incl. DNG/original, sizing, presets, background jobs, HDR gain map JPEG / PQ AVIF ✅; JXL encode ⬜) |
| M11 | RAW II | CR3, RAF (X-Trans), ORF, RW2, PEF, SRW, 3FR, IIQ + long tail; camera calibration DB; HEIC/AVIF/JXL import | 20–35 | 🚧 (RAF all, RW2 every format, PEF, ORF uncompressed and 12.8 bpp, SRW uncompressed, HEIC ✅; CR3 🟡 (26 bodies verified; rarer CRX variants open as previews); **camera colour calibration** 🚧 (per-file JPEG fits, 5 bundled profiles, 52 spectral matrices; measured database missing); compressed ORF / SRW, HE NEF, IIQ, X3F ⬜) |
| M12 | AI & smart features | subject/sky/background/people/object masks, semantic search, faces/People (permissively licensed models, pure-Rust inference) | 20–40 | 🚧 (faces: detection, recognition, People view ✅; Object / Describe masks via SAM 3 🟡; semantic search 🟡 (fork: SigLIP 2, text in photos, opt-in downloads); Subject / Sky / People masks ⬜) |
| M13 | Merge | HDR merge (deghost), panorama (projections, boundary warp, fill edges), HDR panorama | 10–15 | ✅ |
| M14 | Video | import/playback/trim via FilmCraft crates, global edits + presets on video, video export | 6–10 | ⬜ |
| M15 | Classic modules | Map, Book, Slideshow, Print, Web; smart collections, stacks, virtual copies, publish services, tethering | 25–40 | 🚧 (smart albums, stacks, virtual copies, compare/survey ✅; Print dialog, layouts and PDF ✅/🟡; Slideshow settings, playback and PDF / JPEG / AV1 export 🟡 (no module view yet); Map view 🟡 (fork); Book/Web/publish ⬜) |
| M16 | 1.0 polish | preferences, shortcut editor, accessibility, localization, packaging (dmg/msi/AppImage/web), hardening | 10–20 | 🚧 (settings, keyboard shortcuts sheet, packaging basics ✅; English/Chinese (Simplified, Traditional)/Japanese/Brazilian Portuguese/Spanish/German/Russian/Ukrainian/Georgian/Italian/Czech/Polish localisation 🟡; accessibility and further locales ⬜) |

## Risks that coding hours alone don't retire

- **AI features** (subject / sky / people masks, generative remove) need model weights with
  licences we can ship; classical fallbacks first. No permissively licensed sky-segmentation or
  raw-denoise model was found; we may need to train our own.
- **Camera colour and lens data** is a data problem: we never use Adobe's matrices, DCPs or LCPs.
  DNG-embedded data first, then our own calibration; long-tail coverage grows over time and needs
  people with cameras, lenses and charts.
- **Raw-format sources:** decoders are written from prose format descriptions (even ones published
  alongside GPL code); decoder source is never read (decided 2026-10-05). Still open:
  freedom-to-operate review for local Laplacian filters, PatchMatch and HEVC (HEIC).
- **Look parity** with Adobe's default rendering is tuned by eye until the fidelity suite exists.
  Retuning changes existing edits, so it ships as a new process version
  ([process-versions.md](process-versions.md)).

## Design notes kept from the old ROADMAP

- **Folder records:** library folders have records of their own in the catalog
  (`Catalog.folder_records`, keyed by folder path; colour labels first). Photos still store full
  paths, so moving a whole disk relinks each photo. If that becomes a problem (an "Update Folder
  Location" for a renamed drive, journal growth on large moves), the next step is the Classic model:
  photos point to a folder id (root + relative path).
- **Zoom:** native pinch zoom and two-finger pan work in Detail, Compare, Reference and Before / After; at
  1:1 and above they render a window of original pixels (gap 4, closed 2026-10-12).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-15 | minor | Merged upstream up to 0da998f2: Current focus re-ranked (full-resolution zoom and CR3 plan struck, compressed ORF first), Next actions added, alpha-gate rows 2 and 3 and milestones M7, M8, M11, M12, M15, M16 refreshed; numbers unchanged |
| 2026-10-10 | minor | Beta hours 340–600; full ready ~59% in the gate note |
| 2026-10-10 | minor | Added the Alpha gate table (core-workflow gate from the revised standard): passes, stays alpha |
| 2026-10-10 | major | Created: milestones moved from ROADMAP.md (statuses refreshed: M3 ✅, M11 / M12 / M15 detail), current focus re-ranked toward beta, risks and design notes kept |
