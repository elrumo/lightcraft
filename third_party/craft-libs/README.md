# third_party/craft-libs

Four crates from [storytold/craft-libs](https://github.com/storytold/craft-libs) at `c7a872e2f24b2c0d9aa78f25c83f342be26fcad3` (MIT OR Apache-2.0; see
`LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`), merged with LightCraft fork changes:
`craft-raw`, `craft-tiff`, `craft-heif`, `craft-metadata`.

Upstream LightCraft moved its raw decoders, TIFF, HEIF and metadata code into these crates (2026-11). This fork had
its own work in that code (Olympus ORF, Sony lens corrections, HEIC gain maps, the GPX writer, binned linear-raw
decode, partial big-TIFF reads and more), so the crates live here with that work preserved, and the root
`Cargo.toml` replaces the git dependencies with them through `[patch."https://github.com/storytold/craft-libs"]`.

This is its own Cargo workspace (excluded from LightCraft's), so it keeps craft-libs' lints and conventions.

When upstream LightCraft moves its craft-libs pin: diff craft-libs between the old and new pin for these four
crates and apply that diff here (`git diff <old> <new> -- crates/raw crates/tiff crates/heif crates/metadata`).
