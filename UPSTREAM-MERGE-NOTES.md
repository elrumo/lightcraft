# UPSTREAM MERGE IN PROGRESS (delete this file when the merge is finished)

State at commit time (2026-10-27): `git merge upstream/main` (storytold/lightcraft v0.14.0, 201 commits) committed
mid-resolution so the work can continue in the cloud. THIS COMMIT DOES NOT COMPILE.

## Left to do
1. Conflict markers (`<<<<<<<`) remain in: crates/ui-egui/src/lib.rs, crates/ui-egui/src/panels/library_problem.rs,
   xtask/src/layers.rs, xtask/src/main.rs, xtask/src/parity.rs. Find all: `grep -rnE '^(<<<<<<<|>>>>>>>) ' . --exclude-dir=.git --exclude-dir=target`.
2. Some other files were resolved by stopped agents and are UNREVIEWED (they may be half done): apps/lightcraft/src/{main,launch,prefs,session,tests}.rs,
   crates/ui-egui/src/panels/{grid,left,masking,presets,unnamed,notices,export}.rs, crates/ui-egui/src/headless.rs, crates/engine/src/files/fs.rs,
   crates/apphost/src/render.rs, and all the other ex-conflicted ui-egui / engine / raw files. Check each against both sides
   (`git show 38f8d2508:F` base, `git show main:F` ours, `git show upstream/main:F` theirs). One agent was mid-way adding tests to apphost render.rs
   ("a_render_becomes_what_its_slot_shows") and replacing `BIN` with `binary()` in apps/lightcraft-cli/tests.
3. crates/ui-egui/src/import.rs, panels/settings.rs, text_field.rs were deleted upstream (moved to dialogs/import.rs, dialogs/settings.rs, state/settings.rs, and a new
   TextField home); they were removed here. OUR changes to them (diff `git diff 38f8d2508 main -- <old path>`) must be confirmed ported into the new locations.
4. Then: `cargo check --workspace --all-targets`, fix egui 0.36 -> 0.37 breakage in OUR code (apphost, apps/lightcraft-web, lightcraft-ios, lightcraft-ios-swift,
   third_party/egui-wgpu), `cargo test --workspace --no-fail-fast`, `cargo xtask parity --write`, `cargo xtask layers`, `cargo xtask assets`, wasm and ios checks.
   Re-bless the Swift protocol fixture (`LIGHTCRAFT_BLESS=1 cargo test -p lightcraft-ios-swift fixtures`), `cargo xtask swift-strings`, 18 locale catalogs
   (3-way merge; -formats.json of cs/it/ka/pl list our extra strings in English), PARITY.md rows for new UI commands, bump Swift CORE_REV.
5. ROADMAP.md / docs/target-app-parity.md still say "up to v0.13.0"; bump when numbers are recounted. docs/gaps.md cites crates/ui-egui/src/menubar.rs: verify the path.
6. Do NOT push to origin main; this is branch upstream-merge-2026-10-27. When green, merge into main with a fast-forward only after the user agrees.

## Done
- Cargo.toml (version 0.14.0, egui 0.37 fork patches), Cargo.lock (upstream's + ours repaired by cargo), NOTICE, assets/ATTRIBUTION.md, contributors/people.toml, xtask/Cargo.toml.
- third_party/egui-wgpu rebuilt from the storytold/egui fork rev 6d518fc (0.37.0) with our three HDR patches reapplied; it is patched through
  `[patch."https://github.com/storytold/egui.git"]` because eframe reaches egui-wgpu via the git source. Not yet compiled.
- docs/*.md conflicts (parity-checklist, ui-parity, gaps, control-protocol, file-format-parity): done by an agent; gaps: upstream's gap 30 is our 41.

## Policies (from earlier merges; keep consistent)
- Engine `sync` = upstream's Synchronize Folder; our self-hosted cloud sync is `cloud_sync` (`engine/src/cloud_sync.rs`, `cmd/cloud_sync.rs`, `LightcraftApp.cloud_sync`).
- Catalog format: ours is 12 (upstream 9; ours include `SetServerPath`). Keep our numbering; add upstream's new ops only if upstream added a format bump.
- Upstream codecs win where both built one (CR3/CRX, JXL DNG, HEIC, lightcraft-fetch); our binned linear-raw decode (`dng::decode_with` `bin`) and JXL tile cap stay.
- Presets: upstream's `PresetList` + our `preset_group_order`, content-hash import ids, `presets_gen`.
- "Added" (`Photo.imported`) = date taken in this fork (see `dated` in import commit).
- Our Export dialog lives in `ui-egui/src/panels/export.rs`; upstream edits to the export dialog go there by hand. Our text boxes use `widgets::text_box`;
  upstream wraps fields in `widgets::named`.
- The render scheduler lives in `crates/apphost/src/render.rs` (`Scheduler<V>`); ui-egui `render.rs` `Renderer` wraps it. Upstream edits to ui-egui `render.rs` must
  be ported onto our wrapper (and to apphost where they are scheduling logic). Job pool keeps both panic fixes.
- More ui-egui code lives in `crates/apphost` (`lightroom.rs`, `region.rs` constants, `ui_commands.rs` = UI_COMMANDS, `merge.rs`, `keys.rs::FIXED_KEYS` replaced shortcuts.rs's ALIASES and
  the rating/label digit loops): upstream additions to those in ui-egui must go to apphost.
- New egui UI commands/dialogs need a row in `apps/lightcraft-ios-swift/PARITY.md` (leave to the lead).
- People: upstream's faces stack is base; our vision clustering is the "Found in Your Photos" section.
- Hide moved to Cmd+Alt+L (upstream gave Cmd+L to Enable Filters). Upstream drop review is default (`import_dropped_directly` false; fork's tiles stay when on).
- `video_export` is cfg-gated off wasm. Our wasm clippy bans `std::time::Instant::now` (use `web_time`).
- The fork's sections/folders/panels animate; upstream UI tests assume instant layout: add `h.settle(..)` before clicking where needed.
- Never-crash rule (CLAUDE.md): no unwrap/expect/panic/indexing in non-test code; keep it in code you write.
- No em dashes in any text you write (docs, comments, strings).
- Docs rows changed on both sides are merged by hand (keep both sides' statuses and notes; if both changed the same row, keep the more specific/correct text and our fork-only rows).

## How to finish
Make every assigned file free of conflict markers, `git add` it when done, and report a SHORT list: files done, decisions you made that the lead must know,
anything you could not finish or are unsure of (e.g. code you ported by guess, new files you created, items for the compile phase).
