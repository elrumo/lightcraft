# Video clips

> **Last updated:** 2026-10-21 · **Change:** minor (import, poster frames, `video.info` / `video.frame`; first revision)

LightCraft lists video clips next to photos: MP4 / M4V / MOV files import as photos of kind **Video** with their length,
size, recording date and place, and a **poster frame** stands for them in the grid, the filmstrip, the loupe and in the
previews a sync server builds. There is no player, trim, edit or export yet (see [gaps.md](gaps.md) gap 18 and
[parity-checklist.md](parity-checklist.md) `LR-VID-*`).

## What is read

| | Supported | Not yet |
|---|---|---|
| Containers | MP4, M4V, MOV (also QuickTime files that start with `moov`) | MKV, WebM, AVI, MPEG-TS |
| Video | H.264 (Baseline, Main, High: 8-bit 4:2:0, progressive), HEVC (Main, Main 10: 4:2:0) | AV1, VP9, ProRes, 4:2:2 / 4:4:4, 10-bit H.264, interlaced; these clips are catalogued with their length and size but have no picture (`video.info` says why in `unsupported`) |
| Colour | BT.601 / 709 / 2020 matrices, limited and full range, sRGB-like SDR curves; PQ and HLG get a plain SDR mapping with a soft shoulder | exact HDR tone mapping, the `clap` clean aperture |
| Audio | none | decode, playback, export |

Everything is decoded by [FilmCraft](https://github.com/storytold/filmcraft)'s clean-room pure-Rust demuxer and decoders
(Apache-2.0) behind the `lightcraft-video` crate (L1). Only that crate may depend on `filmcraft-*` (`cargo xtask layers`
enforces it, and refuses `filmcraft-codecs` and `symphonia` (MPL-2.0)). The H.264 and HEVC decoders are cargo features
(`h264`, `hevc`, both on) so a distributor can leave out the patent-encumbered codecs.

## How a clip is handled

- **Header only.** A clip's length, size, rotation, date and GPS come from the `moov` box through a `ByteSource`
  (`lightcraft_video::probe`); the file is never read whole. Importing hashes it in 1 MiB chunks
  (`files::probe_video_path`), so a multi-gigabyte clip costs one pass over the file and almost no memory. The hash equals
  `hash_bytes` of the whole file, which the copy check and sync dedupe rely on.
- **Poster frame.** `Clip::poster`: a key frame about a tenth of the way in (then a quarter, a half, the start), skipping
  black ones, decoded on its own, so a poster costs one picture however long the clip is. `Clip::frame_at(t)` seeks to the
  key frame before `t` and decodes forward (B pictures follow presentation order).
- **One hook for every preview.** `files::fs_hooks`' loader returns the poster as the source image, so the grid
  thumbnails, the loupe, smart and mini previews and the server's proxies all use it; `cloud_sync::build_proxies` branches
  on video before reading the original, which serves the server scanner, device uploads and the admin's "Build missing
  previews".
- **Server libraries.** A library folder of phone clips is scanned like photos (`import::EXTENSIONS` now lists `mp4`,
  `m4v`, `mov`): `folders.rs` describes a clip from its header in place, builds its smart and mini previews from the poster,
  and serves the original with Range.
- **Commands.** `video.info {id?}` (what the file says; `unsupported` names what this version can't decode) and
  `video.frame {id?, time?, maxEdge?, path?}` (the picture at `time`, else the poster; PNG base64, or written to `path`).
  Both read the file in place, so they need the file on this computer. Develop settings are not applied to them yet.
- **Never crash.** Every call into FilmCraft runs under `catch_unwind`; sizes read from the file are capped before they
  allocate (picture size, sample size, how far a seek may decode); damaged clips are errors the import reports like any
  unreadable file. Tests cut files at every eighth and overwrite their middle.

## FilmCraft pin

`Cargo.toml` pins every `filmcraft-*` crate to one `rev` (the comment above them says why). It moves by hand for now:
change the three/four `rev` values together, `cargo update -p filmcraft-isobmff`, run `cargo test -p lightcraft-video
-p lightcraft-engine tests_video` and `cargo xtask ci`. FilmCraft needs Rust 1.95; this workspace declares 1.90.

## Tests

Synthetic clips only: `lightcraft_video::testdata` (feature `testdata`) encodes flat-grey H.264 pictures with FilmCraft's
encoder and muxes them as MP4, with or without B pictures. Real HEVC iPhone clips would go in the gitignored `corpus/`
(`skip!`); none is wired in yet.
