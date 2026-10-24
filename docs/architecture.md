# Architecture

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-24 · **Change:** minor (`cargo xtask ci`: one integration-test binary per crate, doctests apart, one WASM check; the engine host is documented for any native frontend, [engine-host.md](engine-host.md); `cargo xtask ci` reports the machine and skipped tests; `testkit`; the `apphost` layer) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

How LightCraft is built today. The longer design document (`plan/architecture.md`) is local-only
(`plan/` is gitignored); this page is the committed, current summary. Sizes are Rust lines counted
on 2026-10-10 (≈ 202,000 tracked lines in 505 files: 24 library crates, 3 apps and `xtask`).

## Layers

`cargo xtask layers` (part of `cargo xtask ci`, `xtask/src/layers.rs`) enforces that nothing below
L5 depends on egui, eframe, winit or rfd, and that each crate only uses lower layers.

| Layer | Crates (lines) | Role |
|---|---|---|
| L0 | `geom` (1.4k), `color` (0.9k), `raster` (0.8k), `tiff` (2.0k), `fetch` (1.4k), `sysmem` (0.1k) | Matrices and warps, colour spaces and transfer functions, float image buffers, TIFF/IFD read and write, pinned HTTPS model downloads, the `unsafe` FFI calls (`malloc_zone_pressure_relief`, `fsync`) |
| L1 | `raw` (22.3k), `codecs` (7.3k), `meta` (4.3k), `develop` (2.5k), `denoise-core` (2.2k), `denoise` (2.7k), `faces` (5.3k), `scenes` (1.0k), `heif` (2.1k, optional) | Raw decoders and demosaic, DNG colour model; standard codecs and encoders; EXIF/XMP/IPTC; the non-destructive edit model and control specs; AI denoise; face detection and recognition; procedural demo photos; HEIC |
| L2 | `pipeline` (9.8k) | The develop pipeline: scene-referred float, resolution independent; the CPU reference (oracle) for everything else |
| L3 | `catalog` (11.0k), `gpu` (5.1k), `preview` (1.3k), `merge` (3.8k), `segment` (2.3k) | Library model and persistence; wgpu compute twin of the pipeline; thumbnail/preview cache and job pool; HDR and panorama merge; SAM 3 segmentation |
| L4 | `engine` (53.2k), then `apphost` | Session, command registry (everything is a command), history, render and export jobs, import, Lightroom catalog and preset import, faces index, view-models; `apphost` is the frontend runtime every UI shares (render scheduling, background pumps, and the engine host a native frontend talks to, [engine-host.md](engine-host.md)) |
| L5 | `ui-egui` (46.9k), `mcp` (2.3k) | The Lightroom-style egui interface (swappable); the MCP server |
| L6 | `apps/lightcraft` (2.8k), `apps/lightcraft-cli` (1.5k), `apps/lightcraft-web` (3.4k), `apps/lightcraft-ios`, `xtask` (3.4k), `testkit` (0.2k, dev-dependency only) | Desktop app (eframe + wgpu, native menus, control server), CLI (render, commands, calibrate, snapshot, MCP), WASM build, the iOS app on egui, tooling; test helpers (skips the CI report lists) |

**Frontends.** egui (desktop, web, the iOS app) drives the engine directly; a native (non-Rust) frontend drives the same
engine through the engine host ([engine-host.md](engine-host.md)). Logic a second UI needs moves below the UIs rather than being written twice: geometry and edit
math into `geom` / `develop` / `color`, session behaviour into engine commands, and threads, scheduling and
post-command policy into `apphost` (the extraction rule in `CLAUDE.md`).

## Data model

- **Photo** records in `catalog` hold the original's path, content hash, metadata, ratings, flags,
  labels, keywords, faces and the current `develop::Settings`. Albums, smart albums (rule trees),
  stacks, virtual copies, versions and folder records live alongside.
- **Persistence** (`crates/catalog/src/journal.rs`): an append-only journal of human-readable
  operations plus periodic snapshots, compacted in the background; torn appends and crashes are
  tested (`tests_torn_append.rs`, `tests_journal.rs`). A lock file guards against two writers.
  **Opening a big library** (`snapload.rs`): a snapshot over 16 MB (tens of thousands of photos) is read
  straight from the file by byte ranges on all cores: the photos map is cut at entry boundaries, each piece
  is parsed on a worker, and develop settings that repeat inside a piece are compared as text and shared
  rather than parsed again. Anything unexpected falls back to the plain `serde_json` read, which gives the
  same catalog and the errors. A 194,000-photo library (708 MB snapshot) opened in 3.5 to 5.3 s on one
  thread and peaked at 1.8 GB; it opens in about 0.9 s and peaks at 0.9 GB. The grid's list is built with a
  fixed-size sort key per photo, sorted on all cores (`Sort::sort_photos`, `par.rs`), stack lookups use the catalog's own index, and
  `cargo run --release -p lightcraft-apphost --example open_bench -- <library copy>` times the open and the
  first screen's requests the way a native frontend asks for them.
- **Edits** are a `develop::Settings` value (every slider is a control spec with id, range and
  default), normalized to image coordinates so previews and exports match. There is one rendering:
  a retune changes how existing edits look (`docs/whats-new.md` says so).
- **Sidecars**: XMP read and write with Adobe `crs:` interop (`docs/xmp-interop.md`); Lightroom
  Classic `.lrcat` import through a pure-Rust SQLite reader (`docs/lightroom-catalog-import.md`).

## Rendering

1. **Decode** (`raw`, `codecs`): raw files to linear camera RGB (demosaic, black/white levels,
   opcodes, highlight handling) and a camera-to-XYZ colour model; standard images to linear light
   through their ICC profile.
2. **Develop** (`pipeline`): white balance, profile, exposure and tone (local Laplacian
   highlights/shadows), curves, HSL / colour mixer, point colour, colour grading, texture, clarity,
   dehaze, detail (sharpening, NR, AI denoise result), optics (lens profile, CA, defringe,
   vignette), geometry (crop, Upright, transforms), masks with local adjustments, heal/clone, grain,
   effects. Stage caching and a source pyramid make slider drags cheap.
3. **GPU** (`gpu`): the same stages in WGSL on Metal / DX12 / Vulkan, checked against the CPU path
   (within 1/255); GPU errors fall back to the CPU (`docs/gpu-pipeline.md`).
4. **Display and export**: display-profile conversion (`docs/display-profiles.md`); export through
   one encoder for app, CLI, MCP and web (`lightcraft_engine::export`).

## Agent control

Every action is an engine command (id, label, menu path, shortcut, params, enabled, run). The UI,
the CLI, the JSON-lines control channel (`docs/control-protocol.md`) and MCP (`docs/mcp.md`) all
dispatch by id, and headless snapshots render the UI without a window.

## Quality gates

`cargo xtask ci`: fmt, clippy `-D warnings` with the never-crash lints, the HEIF build, tests,
`parity` (every id and path cited in [parity-checklist.md](parity-checklist.md) exists), `layers`,
`assets` (every asset attributed, no Adobe formats) and the WASM build. Other workflows build and
test FreeBSD and Windows ARM64, lint packaging, and build releases.

The tests run under `cargo test --workspace --lib --bins --tests`, or `cargo nextest run` with the
same targets when `LIGHTCRAFT_NEXTEST=1` is set and cargo-nextest is installed; doctests run after
them with `cargo test --workspace --doc`. `cargo test` is the default because the demo library's
generated scenes are cached per process, and nextest's process per test regenerates them.
Examples are left out of the test build (clippy `--all-targets` still
checks them). Each crate links its integration tests into one binary, `tests/it/main.rs` with one
module per file (`cargo test -p <crate> --test it <module>::`); a test that changes state for its
whole process (a global allocator, closing the GPU) keeps a binary of its own in `tests/`. The WASM
build is one `cargo check --target wasm32-unknown-unknown --keep-going` over the wasm-safe crates.

`ci` also probes the machine: OS, CPUs, RAM, the GPU adapters of each backend (from the ignored
`gpu_probe` test of `lightcraft-gpu`, run after the workspace tests so it reuses their build; xtask
has no wgpu), the corpus and `CRAFT_FONTS_DIR`. Tests
that need what is missing call `lightcraft_testkit::skip!(need, reason)`, which appends a JSON line to
the file `LIGHTCRAFT_SKIP_LOG` names. The run ends with the machine and the skips by need and reason,
and writes `target/ci/report.json` (commit, dirty flag, machine, capabilities, each step's result and
time, skips) and `target/ci/summary.md` (Markdown for a PR; `--summary-md` prints it). The report holds
no host name, user name or absolute path, and nothing is uploaded. `--hardware` adds a step that runs
the `lightcraft-gpu` tests once per backend with a hardware adapter (`LIGHTCRAFT_GPU_BACKEND`).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-23 | minor | Quality gates: one integration-test binary per crate, doctests apart, nextest on request, one WASM check |
| 2026-10-19 | minor | The engine host is documented for any native frontend ([engine-host.md](engine-host.md)) |
| 2026-10-13 | minor | `apphost` (L4, after `engine`); the extraction rule |
| 2026-10-11 | minor | Quality gates: the CI machine probe, visible skips (`testkit`) and `target/ci/report.json` |
| 2026-10-10 | major | Created from the code (crate sizes, layers, data model, rendering, control, gates) |
