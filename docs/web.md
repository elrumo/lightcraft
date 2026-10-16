# LightCraft in the browser

`apps/lightcraft-web` runs the same egui UI as the desktop app (`crates/ui-egui`) in the browser,
compiled to WebAssembly and drawn with WebGL2 (eframe's `glow` backend).

## Build and run locally

One-time setup:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock> --locked
```

`cargo xtask web` checks the CLI against `Cargo.lock` and prints the exact install command if
it's missing or a different version.

Build, then serve:

```sh
cargo xtask web            # → <target>/web-site/{index.html, worker.js, lightcraft_web.js, lightcraft_web_bg.wasm}
cargo xtask web --serve    # build, then serve on http://127.0.0.1:8080/ (or `--serve 9000`)
cargo xtask web --dev      # unoptimized build with debug info (faster to compile, slow to run)
```

`<target>` is `target/` unless `CARGO_TARGET_DIR` is set. Any static HTTP server works, as long as
it serves `.wasm` as `application/wasm`. The bundle can't be opened from `file://` (module
workers and OPFS need an HTTP(S) origin; `localhost`/`127.0.0.1` count as secure). If `wasm-opt`
(binaryen) is on `PATH`, the release build also runs it over the module.

The release build uses the `web` Cargo profile (see *Bundle size* below) and writes gzip (`-9`)
and brotli (`-11`) precompressed copies next to each file (`*.gz`, `*.br`, pure Rust, printed
as a size table). `--serve` sends them with `Content-Encoding` when the browser accepts them;
configure a production server the same way (e.g. nginx `gzip_static`/`brotli_static`).

## Bundle size

Measured on the full bundle (every codec and raw decoder is in the module), without `wasm-opt`:

| `web` profile                                   | `.wasm` bytes | gzip -9 | brotli -11 | slider job* |
|-------------------------------------------------|--------------:|--------:|-----------:|------------:|
| before: release, thin LTO                       |    15 742 091 | 5 251 664 | 3 473 726 | 3.8 ms |
| fat LTO, 1 CGU, strip, panic=abort, opt 3       |    12 924 849 | 4 779 686 | 3 210 246 | 3.9 ms |
| same, opt-level "s"                             |    13 056 715 | 4 362 536 | 2 959 126 | 6.1 ms |
| same, opt-level "z"                             |    12 648 958 | 4 152 661 | 2 860 173 | 9.0 ms |
| **current:** "s", per-pixel crates at opt 3     |    13 533 126 | 4 583 465 | 3 093 780 | 4.0 ms |

\* median Exposure draft render of the `?bench` loupe in a worker (stage-cached), headless
Chrome. Size-optimizing the pipeline crates costs 50–130 % render time, so `Cargo.toml` keeps
them (pipeline, raster, color, develop, geom, raw, codecs, scenes, preview and the JPEG/PNG
codecs) at opt-level 3 and the rest (egui, eframe, serde, glue) at "s". What goes over the wire
is the brotli column: 3.1 MB, 11 % less than the old build's brotli size and 41 % less than its
gzip size (5.25 MB). `wasm-opt -Oz`, when installed, shrinks it further.

**Fonts.** The browser has no system fonts to fall back on, so CJK text comes from
[craft-fonts](https://github.com/storytold/craft-fonts), the optional `CRAFT_FONTS_DIR` build
input (`CRAFT_FONTS_DIR=../craft-fonts cargo xtask web`; release builds always set it). On wasm32
`crates/engine/build.rs` embeds BIZ UDPGothic Regular (UI and watermark fallback) and Noto Sans
Thai Regular (~214 KiB, for Thai file and folder names in every UI language). The web
bundle also ships Noto Sans CJK SC Regular as `lightcraft_zh_hans.otf` and loads it before the UI
starts. Keeping the 16 MB Chinese face separate leaves the module under Cloudflare's 25 MiB
per-file limit (the older Japanese-only module measured 21.8 MB). Without `CRAFT_FONTS_DIR`, the
web build still works, but Chinese and Japanese text lack glyphs.

**AI Remove** (rten, the MI-GAN runner) adds 2.2 MB to the module (+0.42 MB brotli): measured 2026-10-12 without
craft-fonts or wasm-opt, 21.8 MB → 24.0 MB. rten stays at opt-level 3: at "s" it is no smaller and a fill takes ten
times longer. With craft-fonts the module now needs `wasm-opt` to stay under Cloudflare's 25 MiB per-file limit;
check the size table `cargo xtask web` prints before deploying.

## Deploying: headers

`cargo xtask web --serve` sends these on every response. A production server may send them too;
today they are optional:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
Cross-Origin-Resource-Policy: same-origin
```

COOP + COEP make the page *cross-origin isolated* (`crossOriginIsolated === true`). That will be
**required** for `SharedArrayBuffer`, i.e. for a future wasm-threads build (`+atomics`, nightly
`build-std`), and gives full-resolution `performance.now()`. The current render workers (below)
don't share memory, so the app runs without them; with COEP on, every subresource must be
same-origin or send CORP/CORS headers (the bundle has no third-party resources). The file names
are the same in every version, so serve them with `Cache-Control: no-cache` (not `immutable`);
`packaging/web/README.md` (shipped as `HOSTING.md`) has the hosting details.

## What works

- **A persistent library.** The library lives in the browser's storage for the page's origin:
  [OPFS](https://developer.mozilla.org/docs/Web/API/File_System_API/Origin_private_file_system)
  when the main thread can write it (`FileSystemFileHandle.createWritable`: Chrome, Edge,
  Firefox, recent Safari), otherwise IndexedDB. Both hold the same layout:
  - `library/catalog.snap`, `library/catalog.log`: the same crash-safe journal as the desktop app
    (`lightcraft-catalog`), plus `presets.json`, `view.json`, `prefs.json` and `ui.json` (panel
    layout). The catalog `Store` is a memory mirror loaded at start-up; every change is flushed in
    the background within a frame or two, each file replaced atomically, in modification order
    (`apps/lightcraft-web/src/files.rs`). View state and UI prefs are saved every second when they
    change (a tab can close without notice).
  - Known limitation: the browser storage has no file locks, so two tabs of the same origin open
    the same library and the last one to write a snapshot wins (the desktop app and the CLI lock
    a library folder: `catalog.lock`, issue #99). Use one tab at a time; a guard through the Web
    Locks API (`navigator.locks`) is still to do.
  - `originals/<content hash>`: the bytes of every imported photo. The catalog refers to them as
    `web/<hash>/<file name>`; the main thread keeps recently used originals in memory (≤ 768 MB)
    and loads the rest on demand (the active photo is prefetched).
  - `thumbs/<key>.jpg` + `thumbs/index.json`: the rendered-thumbnail cache, with the desktop's
    budget (2 GB, least recently used pruned to 80 %).

  A new library starts with the procedural demo photos. URL options: `?store=idb` forces
  IndexedDB, `?store=memory` keeps nothing, `?reset` deletes the stored library first (after a
  confirmation: it deletes every imported photo too).
- **Keeping the library safe** (experimental: the library lives only in this browser):
  - The app asks for persistent storage (`navigator.storage.persist()`); when the browser doesn't
    grant it, a notice says the library may be evicted under storage pressure.
  - **File ▸ Back Up Library…** downloads a zip of the library files and every stored original
    (`originals/<hash>/<file name>`, so an unzipped backup is browsable; entries are stored, not
    compressed; at most 4 GB). **File ▸ Restore Library from Backup…** reads such a zip into a
    new library folder (`library-restored-<time>/`; originals already stored are skipped, every
    entry's checksum is verified, and each restored original must match its content hash) and
    only then switches to it (`active-library` names the folder in use) and reloads: the previous
    library stays in storage. Both are web-only (`file.backupLibrary`, `file.restoreLibrary`; the
    desktop library is a folder).
  - A failed save (quota exceeded, storage cleared) is not silent: the catalog then refuses new
    writes, so commands report `saved in memory but not written`, the top bar shows the unsaved
    warning (as on the desktop, see `docs/control-protocol.md`), and saving is retried every
    2 s until it works (`web.stats` → `saveError`).
  - A picked or dropped photo whose bytes can't be stored is not added (it would be gone after
    a reload); a notice says why.
  - One tab at a time: the page holds a Web Lock (`navigator.locks`, `lightcraft-library`); a
    second tab or window shows "LightCraft is already open in another tab" instead of loading
    its own copy (two copies would overwrite each other's saves). Browsers without Web Locks, or
    that refuse the lock request, aren't protected: the app starts without the guard.
  - If the stored library can't be opened, a notice says the session is temporary and nothing
    is saved; the stored library is left as it was.
  - A panic (wasm is built with `panic = "abort"`) replaces the page with a message saying the
    stored library is kept and to reload, instead of a frozen canvas; the message has a Reload button.
  - **A tab that lost its graphics reloads itself.** Browsers take a tab's WebGL context away (a tab in
    the background under memory pressure, a GPU process restart, the machine sleeping). eframe has no
    answer to that: every texture is gone, the canvas stays empty or stale and the painter panics on
    its next new texture, so the page looked frozen while the app behind it still ran
    (`apps/lightcraft-web/src/lifecycle.rs`). Now the loss is noticed (`webglcontextlost`, cancelled so
    the browser may restore it; also checked when the tab comes back), painting stops, a message covers
    the canvas, and once the tab is in front and the library is in storage the page reloads (after 2 s
    if the browser has not given the graphics back, at once if it has). It does not reload while a
    change is on its way to storage, nor when saving fails (the page in memory is then the only copy:
    the message offers the Reload button instead), nor more than three times a minute
    (`health::reload_budget`, so a driver that fails again at once cannot loop). A tab that comes back
    to the front also repaints at once, as an idle app draws nothing by itself.
- **Importing photos with no filesystem.** *File ▸ Import Photos…* (<kbd>⌘⇧I</kbd>) opens the
  browser's file picker. You can also drop files anywhere on the page. The bytes are written to
  storage, then imported and decoded by the same engine code as the desktop app
  (`lightcraft_engine::files::{probe_bytes, load_bytes}`): JPEG/PNG/TIFF/WebP and the supported
  raw formats. "Copy into library" is the same as "Add" here.
- **Rendering in Web Workers.** Renders don't run on the main thread: up to four dedicated
  workers (`hardwareConcurrency − 1`, `?workers=N` to override, `?workers=0` for the old inline
  path) each run a second instance of the same wasm module. `index.html` compiles the module once
  and posts the compiled `WebAssembly.Module` to `worker.js`. A job crosses as JSON (the photo's
  source, develop settings, request); the worker reads the original or the cached thumbnail from
  storage itself, keeps decoded sources and the loupe's stage cache, renders, and transfers the
  RGBA bytes back. Jobs for a photo go back to the worker that decoded it. This needs no wasm
  threads or `SharedArrayBuffer`. If no worker starts, rendering falls back to the main thread. A worker
  that fails (an out-of-memory trap, an exception) ends its job with an error and is started again in its
  place after a growing delay (0.5 s, 1 s, 2 s … up to six times in a row; a worker that answers resets the
  count). Without that one failure left the page with fewer workers for good, and with none every render
  ran on the page's own thread, freezing it for the length of each one. `web.stats` → `workers.restarted`
  counts them; `lightcraft.command("web.crashWorker", '{"index":0}')` makes a worker throw to see it.
- **Slider drags.** The browser has no GPU pipeline (it is the CPU one, a few hundred nanoseconds per
  output pixel per worker), so a drag cannot re-render the loupe at canvas size on every tick: on a
  Retina window that was 1–7 picture updates a second, and the sharp picture after the release
  took 1–5 s. Two things keep a drag moving:
  - **The draft shrinks to what the browser can render in time.** While a slider drags, the loupe renders a
    draft; `DraftBudget` (`crates/ui-egui/src/render.rs`) learns how long this host takes per pixel from the drafts
    that finish and picks the draft's linear scale (0.2–0.6 of the loupe size, aiming at about 40 ms a tick; a
    GPU host stays at 0.6) once per drag: it only goes down during the drag, as a draft of another size starts
    from a cold stage cache. A zoomed view's windows (always drafted at full size before) shrink in proportion
    once the frame's draft is below 0.6, so a fast host keeps them as they were. The sharp render comes on release.
  - **The workers take turns.** One worker could not keep up with the slider and the others sat idle, so a
    draft of a view goes to another worker that already holds the photo, or to an idle one that loads it (up to
    three per photo, `wire::choose_worker`, `MAX_PIPELINE`), instead of waiting for the one that rendered the
    last tick; results that arrive late are dropped by the loupe as before. An original (the `Full` level) is
    kept to one worker. The first drag of a photo is slower while the others decode their copy.
  - The filmstrip thumbnail of the photo being dragged keeps its picture until the drag ends (a render per
    tick used to take a worker from the loupe).

  Measured 2026-10-13 with `docs/showcase/web-drag-bench.js`, Chrome on an M1 Max that was running at a load
  average of 600+ (so read these as a ratio, not as what a quiet machine does): a 1728 × 907 window at 2×, a 20 MP
  Canon CR3, Exposure / Highlights / Temperature drags. Before: 0.7–6.7 picture updates a second, 1.3–5.7 s to the
  sharp picture after the release. After, on the production build (fat LTO): 40–60 updates a second, the main
  thread's animation frames still at the display's rate, ~1.0–1.3 s to the sharp picture; the first drag of a
  photo ~10 a second. The drafts are soft at the 0.2 floor (about 0.13 MP on that window) — that is the price of
  a CPU pipeline at ~300 ns a pixel here; the per-pixel stage got ~20 % cheaper from an always-inlined array map
  ([gpu-pipeline.md](gpu-pipeline.md#drafts-during-slider-drags)).
- **Every develop control** (sliders, curves, mixer, grading, masking, crop…) works as it does
  on the desktop, since it's the same crate.
- **Export downloads the file.** *Export…* (<kbd>⌘⇧E</kbd>) runs the same `app.export` path as
  the desktop app (`lightcraft_engine::export`: JPEG/PNG/TIFF/WebP, sizing, naming). The host's
  `write` service hands each file to the browser as a download instead of writing it to disk.
- **Text on a phone.** Under 900 pt wide the page uses the phone layout (`docs/ios-gaps.md`). A tap
  on a text field raises the on-screen keyboard, also for a field a tap opens by itself (New
  Album): iOS Safari raises it only for a focus made inside the tap, so `panels::mobile::edit_menu`
  reports the focused field's rect to eframe's hidden `<input>` in the tap's own frame instead of a
  frame later. A second tap on the focused field opens the iOS-style edit menu (Select All / Cut /
  Copy / Paste). Copy and Cut write the clipboard in another pass of the tap's frame (a browser
  allows that only inside the tap); Paste reads `navigator.clipboard.readText()` there too, and
  Safari asks to allow it (`web.rs` `read_clipboard`). Checked on the iOS 26.5 simulator's Safari
  (keyboard, typing, Select All, Copy, Paste); not on a device, and not on Android Chrome.
- **Automation.** `await lightcraft.command("library.info", "{}")` runs any engine or UI command
  by id on the next frame and resolves to the JSON result (`web.stats` reports storage, workers
  and the render queue). This is how the headless-Chrome checks drive the page.

## Sync with your other devices

`lightcraft-server` serves this build at `/` (`--web target/web-site`, or the Docker image), and the page signs in to the
server that serves it: Settings ▸ Sync has its address filled in. Synced photos come down as previews into browser
storage (`proxies/<hash>.lcsp|.lcsm`; the render workers use them when the original isn't stored here, and so do auto
tone, white-balance picks and exports up to 2560 px), originals only when asked (Photo ▸ Download Originals; a larger
export needs one), and photos imported in the browser are uploaded without decoding them on the page: the server builds
their previews (Settings ▸ Sync, *Let the server build the previews*). A build hosted on another site can sign in to a
server that lists that site (`--cors-origin`, [sync.md](sync.md#a-web-build-on-another-site)); the library and
sign-in then belong to that site's browser storage. See [sync.md](sync.md).

**Signing out** ([sync.md](sync.md#signing-out)) waits for what is left to be sent, then empties the library in the page
(catalog, sync state, presets and the other synced documents; the screens reset) and deletes the account's preview files
(`proxies/…`) from browser storage, so the next account starts clean. The originals stay (`originals/<hash>`), as
imports do: they are this browser's only copy of those photos, and a backup includes them. There is no safety copy before
Sign Out Anyway here, since the library isn't a folder; File ▸ Back Up Library… first if in doubt.

## Not yet

- **AI Remove runs on the main thread** (a few seconds a stroke; a "Removing with AI…" notice shows first). Its model
  (MI-GAN, 30 MB, [ai-remove.md](ai-remove.md)) is fetched from Hugging Face only when the user presses Download, and
  kept in the site's storage (`models/migan/`).
- **Generative Expand runs on the main thread too** (Crop ▸ Expand ▸ Generate, [generative-expand.md](generative-expand.md)):
  one MI-GAN pass at 512 pixels and the texture synthesis, a few seconds; the same stored MI-GAN, or Classic without
  a download. LaMa (208 MB) isn't offered in the browser.
- **Exports and auto-adjustments run on the main thread** (they need the original's pixels
  there). Right after a reload, the first such command on a photo whose original isn't in memory
  yet fails with "still loading" and works a moment later.
- **Durability is "a frame later", not fsync-before-return:** a crash or tab kill in the few
  milliseconds between a command and its flush loses that command.
- **Data-safety gaps still open** (issue #107): a panic in a decoder still ends the page (and a
  photo that panics while its thumbnail renders can do so again after a reload; there is no
  `?safe` start that skips rendering yet); removing photos doesn't delete their stored originals
  (`originals/<hash>`; they are kept, and included in backups); restored-over libraries stay in
  storage until the site's data is cleared; backups larger than 4 GB need zip64, which isn't
  written yet.
- **Exporting presets** (File ▸ Export Presets…) and **importing `.cube` LUT profiles** need a file on disk: not in
  the browser yet. Importing presets works (File ▸ Import Profiles & Presets…, Presets panel ▸ ⋯ ▸ Import Presets…):
  the browser's file chooser opens, and the chosen files are read into memory and imported by content.
- **Control channel / MCP.** These are desktop-only, because they need a TCP socket.
- **AVIF export** is untested in the browser; it's the one encoder that may not be wasm-safe.
- **JPEG XL export** is native only: the encoder (`jixel`) starts threads of its own, which wasm32 has not got; the
  web build reports "not available in this build". JPEG XL import works.

## Measuring

Open `http://127.0.0.1:8080/?bench` to run a scripted measurement:

1. Wait for the grid thumbnails.
2. Open the first photo in Detail.
3. Drag Exposure through 8 steps, exactly as a slider drag does (begin interaction → `develop.set`
   ×8 → end).

The page then logs one console line:

```
lightcraft-bench {"first_frame_ms":…,"thumbs_done_ms":…,"slider_draft_ms":[…],"slider_draft_median_ms":…,"slider_job_ms":[…],"release_full_ms":…}
```

- `slider_draft_ms`: time from the `develop.set` command until the new loupe texture is ready.
- `slider_job_ms`: the pipeline's share of that time.
- `release_full_ms`: the full-quality render after the drag ends.

The numbers follow the window: the loupe is rendered at canvas size × `devicePixelRatio`, and the render costs
the same per pixel whatever the photo, so a 900 × 600 headless window (0.1 MP drafts, the table above) and a
1728 × 907 Retina window (3 MP full, 0.4 MP drafts) differ ~10×. Measure at the size you care about, with a real
raw (`?bench` opens the first demo photo, which is procedural) and from the same browser the users have. Not
`?store=memory` with a photo you import: the workers have no storage to read its original from, and every
render fails. `await lightcraft.command("web.stats", "{}")` reports `draft` (`scale` of the latest drag,
`msPerMpx` learned, `lastMainMs`) beside the workers and the render queue.
[`docs/showcase/web-drag-bench.js`](showcase/web-drag-bench.js) does the whole drag measurement from the console:
it drops a raw served beside the page, drags Exposure, Temperature and Highlights once per animation frame, and
prints the picture updates per second, the slowest animation frame and the time the sharp render takes after the
release.
