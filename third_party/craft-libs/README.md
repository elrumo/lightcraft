# third_party/craft-libs

Four crates of [storytold/craft-libs](https://github.com/storytold/craft-libs) (MIT OR Apache-2.0, see
`LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`), copied at commit `f0f4758c01581d79577b9bf5046c06f4e77b1f18`:
`craft-raw`, `craft-tiff`, `craft-heif`, `craft-metadata`.

Upstream LightCraft moved its raw decoders, TIFF, HEIF and metadata code into these crates (2026-11). This fork had
its own work in that code (Olympus ORF, Sony lens corrections, HEIC gain maps, the GPX writer, binned linear-raw
decode, partial big-TIFF reads and more), so the crates live here with that work ported on top, and the root
`Cargo.toml` replaces the git dependencies with them through `[patch."https://github.com/storytold/craft-libs"]`.

This is its own Cargo workspace (excluded from LightCraft's), so it keeps craft-libs' lints and conventions.

When upstream LightCraft moves its craft-libs pin: diff craft-libs between the old and new pin for these four
crates and apply that diff here (`git diff <old> <new> -- crates/raw crates/tiff crates/heif crates/metadata`).
