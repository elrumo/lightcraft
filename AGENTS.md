<!-- craft-house-rules:start core source=dde09ea31e2e457f5b4653a6e76f7cdf55944bdf sha256=540e6cea75d12d0710c3dca89658328e81e508ecef102446ca5bb95b7c0b92bb -->
## House rules

These apply in every Craft repository. A later rule may be stricter, not weaker. The source is `standards/house-rules-core.md` in `storytold/craft-contrib`. Propose changes there, not in a copy.

1. **A person is accountable.** Whoever runs an agent answers for what it opens and for its review comments.
2. **Check, then claim.** Before writing code, read the issue comments, search pull requests for `#N` and the keywords, and check recent commits on `main`. Comment on the issue first. Release a claim you will not finish.
3. **Verify, don't guess.** Open a definition before using it. Confirm the cause in the code, even when someone names a location.
4. **Keep fixes narrow and tested.** Fix what the issue describes, where the bug is. Add a test that fails without the fix.
5. **Review the diff before opening a pull request.** Identifiers exist as used, the test fails without the fix, nothing unrelated changed, and the description is true.
6. **Look for a better pull request.** If another fixes the same thing, close yours when it is better or equal, and say why.
7. **Report only what ran.** Say what you tested and what you could not run. Without a local build or test, say so and rely on CI before marking ready.
8. **`main` changes through a pull request.** Do not push to it. Branch protection enforces this. There is no exception.
9. **Stop and ask only about** licensing outside the allowlist, publishing, releases, a new remote, secrets, spending money, destructive git, and anything else hard to undo. Otherwise keep working. `plan/` is a gitignored notebook, not rules. Without it, do not ask for it and do not stop.
10. **Keep secrets and personal details out** of the repository, logs and requests.
11. **CI stays shared.** Workflows call `storytold/craft-common-actions`. Do not change CI in a feature or fix pull request. If CI cannot cover a change, open an issue there.
12. **Guards say why.** A check that looks removable says what it prevents. Read that before changing it.
13. **Instructions must be readable by everyone.** No notes about one machine, and no links to files a reader cannot open. `craftrules` is retired. Do not link to it.
14. **Keep pull requests moving.** Concurrent pull requests are fine; unanswered actionable feedback is not. Respond in 7 days, a reminder at 14, closure eligible at 30, from when an actionable request waits on the author. Suggestions and unconfirmed bot findings do not start the clock. A fix, explanation or agreed plan resets it; unrelated activity does not. Waiting on a reviewer or maintainer pauses it and does not count against the author. A maintainer decides closure; it is never automatic. Reuse search results, do not poll, and stop at a rate limit.
15. **No agent attribution.** A pull request does not say an agent wrote it. A repository rule may not require that.
<!-- craft-house-rules:end -->
<!-- craft-house-rules:start rust source=dde09ea31e2e457f5b4653a6e76f7cdf55944bdf sha256=387e0ccb4b49e567f60951d7d6fff7305054c89f43ad812b9993dc7c38528013 -->
## Rust Craft repositories

These apply to native Craft apps and to shared Rust libraries. Select them by repository type. A repository that merely contains some Rust does not take this block. Rules later in a repository's file may be stricter, but not weaker. The source is `standards/house-rules-rust.md` in [storytold/craft-contrib](https://github.com/storytold/craft-contrib).

1. **Never crash.** Non-test code does not panic and has no `unsafe`, except where the repository file names an exception. Return errors, treat input as hostile, and bound recursion and allocations. New production crate roots copy the `deny` attribute from an existing crate: `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::unimplemented`, `clippy::todo`, `clippy::unreachable`. Every crash fix comes with a test that panicked before the fix.
2. **Clean room.** Learn from observed behaviour and public specifications. Do not copy another product's code, assets, strings or artwork. Check a dependency's licence before adding it.
3. **Assets and fonts.** Every asset is original or suitably licensed, and is recorded in `ATTRIBUTION.md` in the same change. Font files live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), not in the app repository.
4. **Rust only for native Craft apps.** No handwritten JavaScript or TypeScript, and no Tauri, Electron or webview shell, in those apps. Shared tooling follows its documented technical scope and maintainer-approved exceptions. Craft PR Review's existing Tauri and TypeScript UI is one such exception. It does not relax this rule for native apps. Preserve any existing web build.
5. **Time goes through `chrono`.** Do not hand-roll calendar conversion. For dates beyond what `chrono` can represent, follow [standards/time.md](https://github.com/storytold/craft-contrib/blob/main/standards/time.md).
6. **Contributor credits.** Add or edit only your own person's entry in `contributors/people.toml`, and only after asking them. Never guess a name or hand-edit `contributors.json`.

## Licence allowlist

Dependencies and assets use SPDX identifiers. An expression is allowed when every identifier is in this list. A repository may name a shorter list. It may not add an identifier here by editing its own copy of these rules.

- `ISC`
- `MIT`
- `BSD-2-Clause`
- `BSD-3-Clause`
- `Apache-2.0`
- `OFL-1.1`
- `Ubuntu-font-1.0`
- `Bitstream-Vera`
- `CC0-1.0`
- `CC-BY-4.0`
- `CC-BY-SA-4.0`
- `LicenseRef-Public-Domain`

`LicenseRef-Storyteller-Trademark` is not in this list. It covers Storyteller brand marks only, where a repository already says so.
<!-- craft-house-rules:end -->

# LightCraft — instructions for agents

LightCraft is a clean-room, open-source, pure-Rust photo library + non-destructive raw developer targeting Adobe Lightroom parity (and beyond). Native on macOS, Windows, Linux; web via WASM. Sibling of `../pdfcraft` (Acrobat), `../photocraft` (Photoshop), `../vectorcraft` (Illustrator) and `../filmcraft` (Premiere), with the same conventions.

**UI target default:** when the user asks for a UI change without naming the app or frontend, make it in the native LightBites SwiftUI app at `apps/lightbites-swift/`. Use LightCraft's Rust/egui UI only when they explicitly ask for LightCraft or the Rust frontend.

## Start every session here
1. Read `plan/STATUS.md` (current milestone, next unchecked task, blockers).
2. Read the task in `plan/execution-plan.md` §3, the relevant section of `plan/architecture.md`, and the README/docs of the crate you touch. Behaviour/visual reference: `plan/lightroom/` (incl. `10-observed-ui.md` + screenshots).
3. **Know where we stand:** read [`ROADMAP.md`](ROADMAP.md) (stage, headline numbers, every dimension) and, for
   method and evidence, [`docs/target-app-parity.md`](docs/target-app-parity.md). The checklist counts features that
   *exist*; the real gaps are quality and coverage: camera colour calibration, CR3 / compressed raws, per-model
   verification, render fidelity against Lightroom, AI models, HDR / video / Classic modules (raw detail:
   [`docs/raw-parity.md`](docs/raw-parity.md)). A ✅ row is not proof of parity — if you find a ✅ feature that is
   wrong or incomplete, downgrade it to 🟡 with a note.
4. **Pick work** from [`docs/gaps.md`](docs/gaps.md) (ranked by user impact; `B` marks beta blockers). The row-by-row
   tracker is [`docs/parity-checklist.md`](docs/parity-checklist.md) (one row per feature, menu item and shortcut).
   When you land a feature, update its row(s) and the gap entry in the same commit, and the numbers in `ROADMAP.md` /
   `docs/target-app-parity.md` when a listed gap closes (add a row to each doc's revision history; the header line keeps only `Last reviewed`, `Target` and the `Changes` link, never a `Last updated` date or a change note, which every PR would edit and conflict on); `cargo xtask parity` (part of `ci`) checks that every `cmd:`/`ctl:` id and
   path the tracker cites still exists and that `docs/gaps.md` lists each gap once, and `cargo xtask parity --write`
   refreshes its summary.
5. Follow the autonomous operation protocol (`plan/execution-plan.md` §7). Don't stop to ask unless §7 lists the decision as the user's.

`plan/` is gitignored (local-only). In a fresh clone without it, don't ask for it and don't stop.

**Merging agent branches:** merge one branch, run `cargo xtask ci`, fix, commit — then the next. Two individually green
branches can still break each other (e.g. a new struct field vs. a new constructor).

**Resuming after a crash / another session:** check `git status` on main, `git worktree list`, and each worktree's
`git log main..HEAD` + `git status` for unmerged commits or uncommitted work before starting anything new. Commit small
and often so a crash loses minutes, not hours.

## HelpBites planning follows the work (LightBites app)
The LightBites app's tasks live on the HelpBites project's Planning board (project `tb4A2EAxRx`; columns Backlog, Now,
Next, Later, Testing, Done, Canceled, Claude, and more may be added: this list is not complete, so never rely on it).
Keep it in step with what you do, **through the HelpBites MCP connector** (endpoint
`https://helpbites.app/mcp`; its tools cover the board and the rest of HelpBites). The API reference is
https://helpbites.app/_docs/scalar (spec: `https://helpbites.app/_docs/openapi.json`); check it, or the connector's own
tool list, for the current parameters instead of trusting this file. Use the admin API
(`/v1/admin/board-items`, same `.env` and keys as the `apps/lightbites-web/scripts/helpbites-*.mjs` scripts; see
`apps/lightbites-web/README.md` → Roadmap) or `helpbites-roadmap.mjs` only when the connector is down.
- **Not sure how to do something in HelpBites? Look it up before guessing.** This covers anything the user asks you to do
  in HelpBites (the board, comments and attachments, feedback, the changelog, email, errors, pricing, settings, anything),
  not only the steps below. Read the live OpenAPI spec first (`https://helpbites.app/_docs/openapi.json`; the browsable
  reference is `https://helpbites.app/_docs/scalar`): it lists every route with its parameters and what it does. Then go
  through **all** the tools the HelpBites MCP connector offers (its tool list, or `tools/list` on
  `https://helpbites.app/mcp`), not only the board ones, and any HelpBites skill your client lists: a tool for the job
  often exists under a name you would not have guessed. Prefer a connector tool; call the admin API directly only when
  no tool covers it or the connector is down. Don't work from memory of this file or of an earlier session: HelpBites
  gains routes and tools often, and the spec and the tool list are always current.
- **When asked to do a task** that concerns the LightBites app: first check the board through the connector for a card
  that is that task (by number if the user gave one, else by title). **Search every column:** call `list_board_items` with no `status`
  and page with `offset` until you have seen `total` items; never filter by the columns named above (cards also sit in
  columns like `claude` and `testing`, and new columns appear). Only when the whole unfiltered board has no such number
  or title is there no card; if the user names a column, filter by that column's status key instead.
  - **A card exists:** edit it if the user's words changed the task (title, description), and move it to Now.
  - **No card:** ask the user whether to create one (and comment on it when the work is done); create it only on a yes,
    in Now. Don't create cards unasked, and don't skip the question.
  - New work you find on the way (a follow-up, a bug): say so in one line at the end and ask whether to file it
    (Backlog); don't file it unasked.
- **When the work is done** (and always before a commit that completes it), on each card the work covers:
  1. **Comment** on the card with what was fixed or built (and the commit, once there is one), and **what the user should
     test**: concrete steps, or "nothing to test" when you verified it all yourself.
  2. **Screenshots** of the finished result, attached to that comment (below).
  3. **Move it to the column that matches**: Done when the work is finished and verified; still Now, with the comment
     saying what is left, when it is not finished, blocked, or needs the user's test to decide; Canceled for work that was
     dropped. Say in your reply which cards moved and where.
- **Name a card as a link.** Whenever you mention a card in a reply, write its number as a Markdown link to the card,
  `[#97](https://helpbites.app/projects/tb4A2EAxRx/board?item=<item id>)`, never a bare `#97`. The `<item id>` is the
  card's `id` (like `jEFaBNN0QD`), not its number; you have it from the connector's `list_board_items` or
  `update_board_item`. (The host is the API's, `HELPBITES_API_URL`; the path is the board page's own `?item=` deep link.)
- **Screenshots of the result go on the card.** When you take screenshots of the end result of a card's work (the running
  app, the changed screen), upload them with the card's comment, a line saying what each shows. The connector and the API
  take up to 10 files per comment: a file is sent as `{ filename, contentBase64 }` to `POST /v1/feedback/attachments`
  (`?project=tb4A2EAxRx`; PNG, JPEG, GIF or WebP up to 8 MB), which returns `{ url, name, mime, size }`, and those go in
  the comment's `attachments`. Only screenshots of the finished result, not every intermediate look, and never one that
  shows the user's real library (run on `LIGHTCRAFT_DEMO=1` or a library of your own, see the app's CLAUDE.md). If the
  upload fails, say so in one line and give the user the screenshot paths instead.
- **Every new card is private**: create it with `public: false`, so it does not show on the public roadmap or the site.
  Only make a card public when the user says so. (`helpbites-roadmap.mjs` files `shared/roadmap.json` entries as public;
  that file is the published roadmap, so don't add working tasks to it.)
- **Page the board, don't pull it whole.** `list_board_items` takes `status`, `offset` and `limit` (`limit` defaults to 50,
  max 200) and returns `total`; `GET /v1/admin/board-items` takes the same three and, without `limit`, returns every item.
  (Before 2026-10-27 the whole board came back in one 60k-character reply that was cut off, and cards past the cut
  could not be reached.)
- If HelpBites can't be reached (no keys, offline), say so in one line and carry on; never block the task or the commit.

## Never crash (outranks feature work)
People trust LightCraft with their photo libraries and edits; a crash loses their work. A malformed raw/JPEG/XMP, a bad
command, control or MCP argument, a corrupt catalog or settings file, or a full disk must produce an error the user (or
agent) can act on, never a panic. Don't ship a feature by adding a panic path, and fix a crash before building on top
of it.
- **Non-test code never panics:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, and
  no `unsafe`. Return the crate's error type through `Result` and `?`; use `ok_or(..)?`, `let … else`, `if let`, or a
  fallback (`unwrap_or…`) only where it can't silently corrupt a document. An unfinished feature returns an
  "unsupported" error or is disabled. Sole exception: a provably infallible literal, as `#[allow(clippy::expect_used)]`
  + `.expect("why it can't fail")`.
- **`unsafe` lives only in `crates/sysmem`, `crates/ios-host`, `crates/capture`, `crates/wincolor`.** `sysmem`: libSystem calls,
  `malloc_zone_pressure_relief` (returns freed allocator pages to macOS / iOS after raw decodes),
  `os_proc_available_memory` (the iOS app's memory limit), `fsync` (volumes that refuse std's `F_FULLFSYNC`, such as an SMB share), `setpriority` / `pthread_set_qos_class_self_np` (`lower_thread_priority`: the sync server's video encoder, and a device's work for its server below the High usage, run at the lowest priority; `utility_thread_priority`: a phone's background indexing runs between that and the default), `qos_class_self` / `pthread_set_qos_class_self_np` (`Qos`: a thread std starts runs at the default class, so a thread the engine or a frontend waits on takes its starter's, against priority inversions), and `mmap` / `munmap` for `ReturningAlloc`, every
  app's global allocator (large blocks get pages of their own that go back when freed, but for a few kept for the next
  block of the same size until `release_free_memory`: macOS / iOS 27's allocator keeps freed large blocks dirty in the
  footprint), and `CountingAlloc` (`ReturningAlloc` that counts the bytes in use and refuses requests past a limit:
  the global allocator of the corrupt-file sweep, `cargo xtask corrupt-sweep`, never of an app). `ios-host`: the iOS app's Objective-C calls through `objc2`
  (Photos and Files pickers, share sheet, pasteboard, lifecycle notifications, ImageIO decoding, status-bar taps), compiled for iOS only. (The iOS
  app's C entry point, `lightcraft_ios_main`, is the one `#[unsafe(no_mangle)]` there.) `capture`: ImageCaptureCore through
  `objc2` (cameras, phones and card readers: listing, thumbnails, copying files off them), compiled for iOS and macOS
  only and used by `ui-egui`'s `camera_import::system_cameras`. `wincolor`: Win32 calls that read the colour profile
  Windows assigns to the monitor showing the window, and whether Windows colour-manages that monitor itself. Every other production crate root has
  `#![forbid(unsafe_code)]`. A new unsafe need goes in an isolated, well-tested helper crate like them: `// SAFETY:` on
  every block, a safe API, a safe fallback where possible, and a line here naming it.
- **Input-derived numbers are hostile:** `get()` instead of `[i]`/`[a..b]` for offsets from files, users, agents or
  arithmetic on them; checked/saturating math for lengths, offsets and counts; no division by zero, NaN/inf or negative
  casts to `usize`; cap allocations sized by input; slice strings only at char boundaries.
- **Bound recursion** (depth limits or seen-sets: IFD chains, nested metadata, collections). **Don't cascade:**
  `lock().unwrap_or_else(PoisonError::into_inner)` or an error; worker-thread joins are `Result`s.
- **Last-resort guard:** a panic hook plus `catch_unwind` around command dispatch and import/export turns an escaped
  panic into an error dialog and keeps the document. It's a safety net, not a licence; keep `panic = "unwind"` on native.
- **Prove it:** every crash fix lands with a small synthetic regression test that panicked before the fix.
- **Before every beta: `cargo xtask corrupt-sweep`** (`crates/engine/examples/corrupt_sweep.rs`): fixed-seed damaged
  copies of every file in `corpus/` and `apps/lightbites-swift/test-raws` through the import's decoders, reporting
  panics, hangs, crashes and oversized allocations, each with the command that reruns it alone.
- Enforced by clippy: root `clippy.toml` keeps unwrap/expect/panic allowances scoped to tests; indexing is not denied. The workspace
  denies `cognitive_complexity`, `too_many_arguments` and `wildcard_imports`; members inherit them with
  `workspace = true` under a `[lints]` table, and `sysmem` repeats those denies in its local lint table. Every production crate root
  (`lib.rs`, each binary's `main.rs`) carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic,
  clippy::unimplemented, clippy::todo, clippy::unreachable)]`. Every new workspace crate must include
  `workspace = true` under a `[lints]` table in `Cargo.toml` to inherit `[workspace.lints]`, and new production crate roots include
  the attribute before modules or other crate items; crate-level `//!` documentation may precede it. `cargo xtask roots`
  (part of `ci` and `ci --quick`) machine-checks that every root, including binary-only crates and the `cdylib`/`rlib`
  web build, carries both this and the `#![forbid(unsafe_code)]` above (the crates allowed `unsafe` are the forbid exceptions).

## Non-negotiables
- **Clean-room.** Never read/disassemble anything inside Adobe app bundles (names/listings only). Never copy Adobe icons, presets, profiles (DCP), lens profiles (LCP), camera matrices, fonts. Observation of the installed Lightroom is read-only (it syncs the user's personal library: never import/edit/rate/delete there). Never copy GPL/LGPL/AGPL code (darktable, RawTherapee, ART, LibRaw, rawspeed, rawloader, rawler, lensfun, dcraw-derived GPL code…). **One exception (maintainer decision, 2026-10-17):** the EOS R6's neutral-look hue/saturation table
  (`assets/camera-profiles/Canon_EOS_R6.neutral.json`) is fitted to Lightroom's default exports of the maintainer's
  own photos (rendered output only, no Adobe file read; `docs/camera-preview-colour.md`). Fitting anything else to
  Adobe output needs the maintainer's go-ahead first.
- **Pure Rust** in the product. No C/C++ dependencies.
- **craft-libs, patched here:** `craft-raw`, `craft-tiff`, `craft-heif` and `craft-metadata` (upstream's raw decoders,
  TIFF, HEIF and metadata) come from `third_party/craft-libs`, a copy with this fork's own decoder and metadata work on
  top, patched in through the root `Cargo.toml`'s `[patch."https://github.com/storytold/craft-libs"]`. Change those
  crates there, and follow `third_party/craft-libs/README.md` when upstream moves its craft-libs pin.
- **Shared frontend logic lives below the UIs** (extraction rule, [`docs/engine-host.md`](docs/engine-host.md)): when
  another frontend first needs logic that sits in `ui-egui`, move it down in the same commit and switch egui to it;
  never write it twice. Geometry and edit math go to
  `geom` / `develop` / `color`; session behaviour becomes an engine command; threads, render scheduling, background
  pumps and post-command policy go to `crates/apphost`.
- **Layering** (enforced by `cargo xtask layers`; `plan/architecture.md` §3 is background): nothing below L5 depends on egui/eframe/winit/rfd.
- **Everything is a command** (`crates/engine`): id, label, menu path, shortcut, params, enabled(), run(). UI, CLI, control channel and MCP all dispatch by id. Every slider is a `develop` control spec.
- **Long-running work shows in the activity stack** ([`docs/background-tasks.md`](docs/background-tasks.md)): anything
  that runs off the UI thread and can take more than a moment holds a `TaskGuard` from `session.activity.start(kind,
  label, cancel)` for its whole run. It adopts the cancel flag the job already checks (`Cancel::Flag`), or uses
  `Cancel::No` when the job can't stop. No progress window, panel or progress toast of its own; its tests assert the row.
- **Resolution independence:** settings use normalized image coordinates and relative radii; previews and exports must match.
- **Time goes through `chrono`** (workspace dependency). Never hand-roll UTC/calendar conversion (days-to-date algorithms, leap-year math); a crate owns only its output format string. The one exception is range extension: past the years chrono can represent, shift the date by whole 400-year Gregorian cycles (146,097 days, after which the calendar repeats), let chrono compute the month and day, and add the cycles back to the year (splitting seconds into hours, minutes and seconds is plain arithmetic).
- **Quality gates** before every commit: `cargo xtask ci --quick` (fmt, clippy -D warnings, the changed packages'
  tests, layers, assets, roots, and heif / parity / wasm / ios when touched). The `ios` step is a type-check for `aarch64-apple-ios` (`rustup target add aarch64-apple-ios`); see `docs/ios.md`. GitHub runs the full `cargo xtask ci` on every PR
  (`.github/workflows/ci.yml`); run the full one locally before merging branches (below) or when `--quick` can't see
  the risk. One `ci` runs at a time per machine; others wait their turn (`xtask/src/ci_lock.rs`).
- **Commits:** one task id per commit (`M2.3: local Laplacian highlights/shadows`). Only green states. End messages with the attribution line required by the environment.

## Assets: icons, images, fonts (ABSOLUTE RULE — never violate)
- **Never use any iconography, image, artwork, font, sound or other asset from Adobe products** (no Lightroom/Creative Cloud icons, no screenshots, no presets/profiles/LUTs, no UI bitmaps — not even as a temporary placeholder or "reference copy"). Observing Adobe's UI to imitate *layout and behaviour* is allowed; copying or tracing its assets is not.
- **This includes Adobe's open-licensed assets**: no Source Sans/Serif/Code or Source Han fonts, no Adobe Fonts, no
  Adobe-published icon sets, sample photos, colour profiles or LUTs — even when OFL/MIT. The Rust UIs' font is Inter (OFL);
  Japanese and Chinese fonts come from craft-fonts (below). **One exception (maintainer decision, 2026-10-07): Noto
  Sans/Serif CJK** (Google-branded, OFL, co-developed with Adobe as Source Han) is allowed via craft-fonts for Chinese
  text, because nearly every OFL Chinese face derives from it. Use it unmodified under its OFL; this does not open the
  door to Source Han under Adobe's name or to any other Adobe asset.
- Every asset in the repository must be one of: **our own original work** (e.g. icons drawn in code as vectors, procedurally generated demo photos), **public domain / CC0**, **Creative Commons** (CC-BY / CC-BY-SA with attribution honoured), **OFL** (fonts), or **permissive open-source** (MIT/Apache-2.0/BSD/ISC) — or contributed by a person who created the asset and licenses it openly.
- **Exception: `docs/brand/`.** The ArtCraft name, wordmark and logos there are ArtCraft Team trademarks, not open source and not covered by LightCraft's MIT OR Apache-2.0 licence (`LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`); their terms are in `docs/brand/LICENSE-brand.txt`. Use them only unmodified and never redraw, recolour or derive from them.
- **Every asset must have an entry in `assets/ATTRIBUTION.md`** (path, title, author/creator, source URL or "original work", licence, date added, modifications) and its licence text when required (e.g. `assets/fonts/OFL-*.txt`). Add the entry in the same commit as the asset. Assets without an attribution entry must not be committed.
- **Fonts live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), never in this repo.** Don't commit
  font files here (Inter, already in `assets/fonts/`, is the one exception); add new fonts to craft-fonts. LightCraft
  uses it as the optional build input `CRAFT_FONTS_DIR`: `git clone https://github.com/storytold/craft-fonts ../craft-fonts`
  then `CRAFT_FONTS_DIR=../craft-fonts cargo run -p lightcraft` (or any cargo/xtask command). `crates/engine/build.rs`
  embeds the manifest's fonts as `lightcraft_engine::CRAFT_FONTS` (wasm32: BIZ UDPGothic Regular only); `cargo xtask web`
  ships Noto Sans CJK SC Regular as a separate asset loaded by the UI before startup. The UI
  (`theme::font_definitions`) and the export watermark renderer use CJK faces (picked by script) as fallbacks after Inter; the UI also falls back on the faces of other scripts (Thai, Arabic, …). Unset,
  `CRAFT_FONTS` is empty: everything builds, tests and runs, but Chinese and Japanese text lack glyphs. Releases always build
  with it (`release.yml`, `CRAFT_FONTS_REQUIRED=1`) and ship the fonts' OFL licences. Tests that need these fonts skip
  without it; the FreeBSD CI job runs them with it.
- Icons drawn in code (e.g. `crates/ui-egui/src/icons.rs`) are original work and are recorded in `assets/ATTRIBUTION.md` as such; do not trace them from Adobe icons.
- Demo/test images: generated procedurally by `lightcraft-scenes`, or CC0 downloads kept in the gitignored `corpus/` with their source recorded. Screenshots of Adobe apps live only in the gitignored `plan/` and are never committed or published.
- **Enforced:** `cargo xtask assets` (in `ci`) fails when an image/icon/font/sound/video/raw/ICC/XMP file is not matched
  by a path pattern in the first column of `assets/ATTRIBUTION.md`, when a referenced licence file is missing, when a
  file name suggests Adobe material, or when an Adobe profile/template format (`.dcp`, `.lcp`, `.lrtemplate`, …) appears.
  Never weaken this check to make a commit pass — fix the attribution or remove the asset.
- When in doubt about an asset's licence: don't use it.

## Running and looking at the app
- `cargo run --release -p lightcraft -- --control 7980` opens the desktop app with the JSON-lines control server (see `docs/control-protocol.md`).
- **Never send OS-level synthetic input** (osascript/System Events keystrokes or clicks, `cliclick`, accessibility
  automation): it goes to whatever window is frontmost — the user's terminal or other apps. Drive LightCraft only
  through its control channel (`ui.key`, `ui.pointer`, `ui.clickWidget`, `ui.menu.invoke`) or headless snapshots.
- For UI work, **look at the result**: drive via the control channel and take `ui.screenshot`. Compare with
  `plan/lightroom/screenshots/` when available. Without that optional notebook, verify the result against the
  task's requirements, committed documentation and regression tests; do not request the notebook or block work.
- **Unattended (display asleep/locked, CI, nightly runs): prefer headless snapshots** — no window needed:
  `lightcraft-cli snapshot --demo --script tour.jsonl -o out.png --size 1600x1000` (control-protocol requests,
  one per line; see `docs/control-protocol.md` → Headless rendering). In a running app use
  `ui.screenshot {"path": …, "headless": true}`; windowed screenshots fall back to headless after 2 s.
- MCP: `lightcraft-cli mcp` (headless, `--demo` for the procedural library) or `lightcraft-cli mcp --connect` (drives
  the running app). See `docs/mcp.md`. Quick non-UI checks: `lightcraft-cli render in.jpg -o out.jpg --set light.exposure=1`.
- Export goes through `lightcraft_engine::export` (one encoder for app, CLI, MCP and web); UI-only commands live in
  `crates/ui-egui/src/menus.rs`.
- Text boxes use `crates/ui-egui/src/widgets/text_field.rs` (`TextField`), not `egui::TextEdit`: it gives every field the
  Cut / Copy / Paste / Select All menu, Esc restoring the text, and reports how an edit ended. Give each a widget id
  that is unique on screen. Older fields still on `TextEdit` move over as they are touched.
- Shell gotcha: `mv`/`cp` are aliased interactive here — use `/bin/mv -f` / `/bin/cp -f`.
- **Big rebuildable data goes to the Backup drive when it is mounted** (the main drive is small and fills mid-CI):
  `~/bin/ext-storage <subpath> <fallback>` prints `/Volumes/Backup/dev-cache/<subpath>` while the drive is really
  mounted, else the fallback, and never fails. Shells already export `CARGO_TARGET_DIR` that way (`~/.zshenv`,
  fish `conf.d/ext-storage.fish`). Agents that need their own dirs use
  `export CARGO_TARGET_DIR="$(ext-storage cargo-target/agent-<name> target/agent-<name>)"` and
  `-derivedDataPath "$(ext-storage xcode-dd/<name> <scratch>/dd)"` instead of a path on the main drive. Unplugged =
  the default location, so a cold build; never symlink `target/` to the drive.
- Parallel agents: separate git worktrees and `CARGO_TARGET_DIR` (see above; was `target/agent-<name>`); each agent uses its **own control port** (pick one in 18000–19999, never the default 7980) and its own scratch subfolder (`<scratch>/<agent-name>/`) — never `rm -rf` shared paths; delete your target dir when done (disk is shared); keep every `Cargo.toml` valid at all times (the `crates/*` glob means one broken manifest breaks everyone).
- **Queued builds share one warm cache.** When several agents take turns building (one slot at a time, so the
  machine isn't overloaded), they all build the shared checkout with the same dirs instead of their own:
  `CARGO_TARGET_DIR="$(ext-storage cargo-target/agent-chain target/agent-chain)"`, Xcode
  `-derivedDataPath "$(ext-storage xcode-dd/chain-ios <scratch>/dd)"` (`chain-mac` for the Mac app) and one simulator,
  `Chain iPhone` (created once as an iPhone 17 Pro; never erase or delete it, reset only your own app data). Each slot
  then rebuilds only what changed instead of building from cold. Plan every check before building and run them all on
  that one build; group cards in the same part of the app into one agent so one build tests several. Only a build in
  a separate clean copy or worktree uses its own dirs (a different source path invalidates the shared cache).
- Test corpora: `cargo xtask corpus --download` into `corpus/` (gitignored, CC0 only). Never commit media.
- Shared real-file test corpora (Photoshop-authored PSDs, etc.) live in [`storytold/photocraft-corpus`](https://github.com/storytold/photocraft-corpus). Never commit large binary fixtures to this repo; fetch them pinned by commit and sha256-verified, as PhotoCraft does with `cargo xtask corpus`.

## Testing & performance (do this often)
- Unit/property tests next to the code; end-to-end tests drive real binaries (`crates/mcp/tests/e2e.rs`,
  `apps/lightcraft-cli/tests/`). New features need at least one test that would fail without them.
- **Run the app after every user-visible change**: launch with `--control 7980`, drive it with a JSON-lines script
  (`docs/showcase/run.py file.jsonl`), take `ui.screenshot`, and look at it. Check `ui.inspect` → `perf`.
- **Benchmarks:** `cargo xtask bench` (24 MP raw from corpus; CPU and GPU columns) appends to `target/bench/history.jsonl`
  and flags CPU-time regressions > 20 % vs the previous run (`--strict` to fail). Run it before and after perf work.
- **Measure, don't guess**: `LIGHTCRAFT_PROFILE=1` prints per-stage pipeline timings to stderr; time CLI renders
  with `/usr/bin/time`. Record numbers in `plan/STATUS.md` → Metrics. Budgets: slider update ≤ 16 ms (draft) / loupe
  ≤ 60 ms on ~2.5 MP; export ≤ 1 s per 24 MP JPEG.
- Never commit media: test images are procedural (`lightcraft-scenes`) or CC0 downloads in the gitignored `corpus/`.
- **Flaky tests:** a test that fails, then passes on a rerun of the same commit, is flaky. Don't rerun until it
  passes and move on: open an issue with the failing log, and if it failed on two unrelated PRs, quarantine it with
  `#[ignore = "flaky: #<issue>"]` (a doctest: an `ignore` code fence and a comment naming the issue) in a PR of its
  own until the issue is fixed. Never quarantine a test that fails every time; fix the code or the test. A test that
  hangs is a failure too: under `cargo test`, libtest's "has been running for over 60 seconds" line names it; under
  nextest (`LIGHTCRAFT_NEXTEST=1`), its `SLOW` lines do.
- A test that can't run here (no GPU, corpus file, craft-fonts face, model) calls `lightcraft_testkit::skip!(Gpu, "why")`
  before it returns, not a bare `eprintln!`. `cargo xtask ci` then lists the skips and the machine's GPU adapters and
  data, and writes `target/ci/report.json` and `target/ci/summary.md` (Markdown for a PR).

## Map of the code
`geom`, `color`, `raster` (L0) → `raw`, `codecs`, `meta`, `develop` (L1; over the craft-libs crates above) → `pipeline` → `catalog` → `engine`
→ `apphost` (L4: the frontend runtime shared by the UIs) → `ui-egui`, `mcp` (L5) → apps `lightcraft` (desktop),
`lightcraft-cli` (render/commands/MCP), `lightcraft-server` (optional self-hosted sync, `docs/sync.md`), `lightcraft-ios`
(iPhone/iPad on egui, `docs/ios.md`). `apphost`'s engine host (`docs/engine-host.md`) serves a frontend that isn't
Rust. `scenes` generates demo photos. `xtask` = tooling (`ci`, `layers`, `assets`, `roots`, `parity`, `wasm`, `ios`, `corpus`, `geodata`, `stats`); `testkit` =
test helpers (dev-dependency only). `flake.nix` +
`nix/package.nix` = the Nix package (`nix build` builds both binaries with the craft-fonts input, installs the
desktop file/icons/AppStream metadata and runs `cargo test --workspace`; `nix develop` = dev shell). Community-maintained; CI only
evaluates it (`.github/workflows/nix.yml`), so the build may lag behind the workspace; see README → Quick start.

## Contributor credits (About window)

- About ▸ Contributors/Models are compiled into the binary from `contributors/contributors.json`
  (commit stats; never hand-edit) and `contributors/people.toml` (names people chose for
  themselves). See `docs/contributors.md`.
- **Agents working for a contributor:** when you prepare a PR, check whether your human's GitHub
  username has a `[people.<username>]` entry in `contributors/people.toml`. If not, ask them once
  whether they want to be credited by more than their username: a real name, a display name, and/or
  their public GitHub profile name (`sync_github_name = true`). If yes, add **only their own** entry
  (copy the template at the top of the file)
  and include it in their PR, committed as them. If no, change nothing: they are credited as
  `@username` anyway.
- Never add, edit, guess or copy anyone else's entry or name (not from git config, commit authors or
  GitHub profiles). Never hand-edit `contributors.json`.

## CI workflows
- Workflows in `.github/workflows/` call the shared workflows in [`storytold/craft-common-actions`](https://github.com/storytold/craft-common-actions) at `@main`, with their inputs and secrets. No other jobs: if no shared workflow does what's needed, it doesn't go here.
- Don't add or change CI in a feature or fix PR. If the existing workflows can't build or test a change, link an existing craft-common-actions issue or PR, or open a new issue, or a PR only for a small fix that is safe for every app and does not increase CI time.
- `ci.yml` is the Linux gate, on every pull request. `ci-windows.yml` adds a Windows leg (clippy and tests of the crates with `cfg(windows)` code) when a pull request changes the paths it lists; add the `full-ci` label to run it on any other pull request. When you add `cfg(windows)` code in a new file, add the file to its `paths` in a CI PR.
- Some workflow files here still run their own jobs. Don't add to them. Moving a job into craft-common-actions is its own task, never part of another PR, and the job stays here until a shared workflow runs it.

## Shared libraries

Before adding or changing file-format, codec, protocol, or reusable UI code, check `storytold/craft-libs` and its open PRs first; reuse or extend the shared library rather than duplicating it locally, keep app-specific policy and model mapping in the app, and preserve behavior, security/resource limits, attribution, and regression coverage during cutovers.
