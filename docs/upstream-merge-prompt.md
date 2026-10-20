# Prompt: bring upstream LightCraft changes into this fork

Copy everything below the line into an agent session opened in this repository.

---

Bring the latest changes from the upstream repository https://github.com/storytold/lightcraft into my local `main`
(this fork's remote `upstream`; `origin` is my own fork). Cover the Rust code, and finish with a written summary.

## Ground rules

- **Read first:** `CLAUDE.md`, `plan/STATUS.md`, `ROADMAP.md`, and my memory notes `upstream-merge.md`,
  `upstream-batches-2026-10-17.md`, `shared-main-checkout.md`, `one-rendering-no-process-versions.md`,
  `disk-space-builds.md`. They record merge policies and traps from earlier merges.
- **Parallel sessions share this checkout.** Other sessions edit `main` and sweep-commit. Before starting, run
  `git status`, `git worktree list`, and check for `.git/MERGE_HEAD`. Do not stage or commit files you did not change
  (use `git add <paths>`, never `git add -A` after the first sweep). Re-check `git status` right before each commit; a
  sweep commit by another session may have already closed your merge, in which case add your fixes as a follow-up commit.
- **Disk:** check `df -h .` first. `target/` can be 100+ GB. Build with `CARGO_INCREMENTAL=0`. If space is short, delete
  `target/*/incremental` (regenerable), nothing else.
- **Shell is fish:** unquoted globs such as `--include=*.rs` fail. Quote them. Use `/bin/mv -f` and `/bin/cp -f`.
- **No em dashes** in replies, docs, comments or commit messages.
- Clean-room and asset rules in `CLAUDE.md` apply to everything arriving from upstream. After the merge run
  `cargo xtask assets`; a new asset without an `assets/ATTRIBUTION.md` entry, or anything Adobe-derived, GPL/LGPL/AGPL
  code, a C/C++ dependency or a new `unsafe` outside the allowed crates, must be reported to me, not silently kept.

## Steps

1. **Commit my work first.** If the tree is dirty, commit it (one local commit, honest message) so the merge starts from a
   clean state. Note the commit id.
2. **Fetch and measure.** `git fetch upstream`. Report `git rev-list --left-right --count main...upstream/main`, the merge
   base, and `git log --no-merges <base>..upstream/main`. Read the new commit list before merging; group it
   (security, fixes, features, docs, release/CI, localisation).
3. **Merge** `upstream/main` into `main` (`git merge upstream/main`, a merge commit, no rebase). List conflicts with
   `git diff --name-only --diff-filter=U` and the hunk count per file.
4. **Resolve with these policies:**
   - Where I **refactored or moved code** (apphost runners, `crates/catalog/src/search.rs`, the Lightroom runner, merge
     runner, "one rendering" with no process versions, catalog format, presets, export panel), keep my structure and
     **port upstream's behaviour change onto it**. Do not take upstream's side wholesale and do not drop its fix. For each
     file, diff `<base>..upstream/main` for that file to see exactly what upstream changed, then re-apply that intent.
   - Docs (`README*`, `docs/gaps.md`, `docs/parity-checklist.md`, `docs/raw-parity.md`, `docs/whats-new.md`): keep my rows and
     wording, fold in upstream's additions (a word-level diff of the base and upstream row shows the delta), keep both
     changelog tables, bump the *Last updated* lines and revision history as `CLAUDE.md` asks.
   - `Cargo.lock`: resolve by hand to the union, then let cargo normalise it. New git dependencies need network access.
   - Locale JSON: no duplicate keys, valid JSON after resolving (check with a script).
   - Never leave a conflict marker anywhere (`grep -rn '^<<<<<<<' .` excluding `target` and `.git`).
5. **Verify the Rust side.**
   - `cargo check --workspace --all-targets` must be clean. Fix test-only compile errors caused by signature differences.
   - Run targeted tests for every crate whose files conflicted or changed (catalog, codecs, raw, engine, apphost, ui-egui
     for the areas touched). Run `cargo xtask ci` if disk and time allow, and say plainly if you did not.
   - A failing test is reported with its output. Do not delete or weaken tests to get green.
6. **Commit.** One merge commit for the merge itself; fixes after it as follow-up commits with a task-style message and the
   attribution line the environment requires. Only commit green states you actually checked.
7. **Update the memory note** `upstream-merge.md`: last merged upstream commit, the date, new policies you had to decide,
   and anything left unverified.

## The summary (final reply, and save it to the scratchpad)

Write it for me, in plain words, with these sections:

1. **What was merged:** upstream range (base and tip ids), number of commits and files, version.
2. **What upstream brought in:** grouped list (security, fixes, features, raw/camera support, docs, release).
3. **What I had to decide:** each conflicted file, what you kept, what you ported, and why.
4. **Verification:** commands run, pass/fail counts, what you did not run and why, anything flaky.
5. **State:** commit ids, branch, untouched work from other sessions, confirmation that nothing was pushed.
6. **Open items:** anything for me to decide.

Be honest about gaps: if something was not run, not built or not seen on a device, say so in that section.
