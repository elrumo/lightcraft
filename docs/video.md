# Video clips

> **Last updated:** 2026-10-22 · **Change:** minor (Hardware video: VideoToolbox decode and encode measured against the pure-Rust ones on a Mac, with a go / no-go; the FilmCraft pin moves with `cargo xtask filmcraft`; library stats; Live Photos, `live.*`; import, poster frames, `video.info` / `video.frame`; first revision)

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
  `video.source {id?}` says where a system player can play the clip from (`path`, or `remote` for a synced clip whose
  original `sync.downloadOriginals` has yet to bring), and `catalog.query` photos carry `duration`.
- **Never crash.** Every call into FilmCraft runs under `catch_unwind`; sizes read from the file are capped before they
  allocate (picture size, sample size, how far a seek may decode); damaged clips are errors the import reports like any
  unreadable file. Tests cut files at every eighth and overwrite their middle.

## Editing a clip

A clip's own edits (the part kept, the poster frame) are in its develop settings (`DevelopSettings.video`: `trim_start`,
`trim_end`, `cover`, seconds into the clip; catalog format 12), so they are undoable, kept in the history, synced, and left
out of Copy Settings and presets. The develop settings themselves (exposure, colour, tone, effects, crop…) are the clip's
too: they show on its poster frame like on a photo and are applied to every picture when it is exported.

- `video.setTrim {id?, start?, end?}`: keep the part between; at least 0.1 s stays; `null` puts that end back. One undo step.
- `video.setCover {id?, time}`: the poster frame, the picture the grid, the loupe and the previews show. The source path the
  loader reads carries the time (`files::at_time` / `split_time`, a NUL between the file and the seconds) and the render
  caches' content key includes it (`media::content_key`), so a new frame is a new picture everywhere.
- `video.frame {time?, edits?}`: one picture; with `edits` as an export shows it (develop settings and crop applied), without
  as recorded. No `time` is the poster frame the clip was given, else the library's pick.
- `video.export {id?, path, maxEdge?, quality?, edits?, trim?, wait?}`: the clip written again as an H.264 MP4 in the
  background (a row in the activity stack; cancelling leaves no file; written as `.part` and renamed; never over an
  original). Each picture is decoded once, in order (`Clip::for_each_frame`), goes through the same pipeline as a still
  (`media::develop`, on the GPU when there is one), is made even and encoded by FilmCraft's pure-Rust encoder
  (`Mp4Encoder`: constant quality 28 / 23 / 19 for smaller / balanced / better, two B pictures, key frames every two seconds).
  The kept part's sound is copied sample for sample, not decoded. `video.exportStatus` reads the progress.

Not built: playing an edited clip in real time (a native frontend renders a small preview file with `video.export` and plays
that), the hardware encoder (see *Hardware video*), HDR output, joining clips, copying the video stream of an untouched trim
without re-encoding, and following a moving subject with a mask (masks, spots and lens blur apply at the same place in every
picture).

## Live Photos

A Live Photo is a still (HEIC or JPEG) and the few seconds of clip a phone records around it. The still is the photo;
its clip, the **motion**, is kept with it (`crates/engine/src/live_photo.rs`):

- **Where.** In the library folder's `Motion/` (named by the photo's id), listed in `live-photos.json` with the still's
  content hash, the clip's length and, when the frontend knows them, the moment the still was taken and Apple's content
  identifier (what pairs the two in a system photo library). No paths: the app's folder moves with every iOS update.
- **How it gets there.** A frontend hands it over: `photoLibrary.open {asset, path, motion}` (a photo-library picture
  opened), `library.import` / `import.start {motions: {still path: clip path}}` (the clips are not imported as videos of
  their own; a still the library already has gains its motion), or `live.attach {id, path}`. A clip from the app's
  staging folder is moved, anything else copied. The clip is checked first: a MOV or MP4 whose header reads, up to 60 s
  and 512 MB.
- **What a frontend sees.** `live: true` in `catalog.query`'s photos and `catalog.describe`; `live.info {id}` says where
  the clip is, to play it as recorded (develop settings are not applied to it) or to save the two to a photo library as
  a Live Photo again. `live.detach` takes the motion away.
- **This device's own.** The motion is not synced yet: other devices and the sync server have the still alone.
- **Removed late.** A photo deleted for good keeps its clip until the library next opens (Undo can bring it back until
  then); a photo-library picture left unchanged takes its clip with it when it is forgotten; a sign-out tidies the
  account's clips away; files in `Motion/` that no entry names go when the library opens.
- **Not yet.** A folder import pairing `IMG_0001.HEIC` with `IMG_0001.MOV` by itself, syncing the motion, developing it.

## FilmCraft pin

`Cargo.toml` pins every `filmcraft-*` crate to one `rev` (the comment above them says why); the measurement crate in
`crates/video/hw-bench` pins the same one. Dependabot does not move git dependencies pinned by `rev`, so
`cargo xtask filmcraft` does:

```bash
cargo xtask filmcraft --check [--strict]        # main's commits the pin lacks, the files they changed in the crates we use, rust-version
cargo xtask filmcraft --update [REV] [--no-test] # every rev, `cargo update -p filmcraft-*`, then the video tests
```

`--check` exits non-zero only with `--strict`. `--update` takes `main`'s head by default, refuses a rev whose
`rust-version` is above ours *and* the old pin's (bump `rust-version` in `Cargo.toml` deliberately first; FilmCraft needs
1.95 and this workspace declares 1.90, so today only a bump that raises it further is refused), and without `--no-test`
runs `cargo test -p lightcraft-video --features testdata` and `-p lightcraft-engine tests_video`. Then run
`cargo xtask ci` (it includes wasm).

`.github/workflows/filmcraft.yml` does it every Monday: `--update --no-test` on the branch `chore/filmcraft-pin`, and one
pull request "chore(deps): FilmCraft <rev>" that CI builds and tests. Nothing merges by itself; the golden-picture tests in
`crates/video` are what tells a person that a bump changed decoded pictures. (The default `GITHUB_TOKEN` cannot start
CI on a pull request it opens: set the repository secret `FILMCRAFT_PIN_TOKEN`. A fork can run the workflow on its own
CI, or run the command by hand.) At an upstream merge take whichever rev is newer and run `--check`.

`cargo xtask layers` keeps the pin contained: only `lightcraft-video` may depend on `filmcraft-*`, `filmcraft-codecs` is
refused, and so is any `symphonia` package in `Cargo.lock`.

Developing FilmCraft and LightCraft together: add this to your own `.cargo/config.toml` (not a commit) to build against a
local checkout of `../filmcraft`:

```toml
[patch."https://github.com/storytold/filmcraft"]
filmcraft-isobmff = { path = "../filmcraft/crates/isobmff" }
filmcraft-h264 = { path = "../filmcraft/crates/h264" }
filmcraft-hevc = { path = "../filmcraft/crates/hevc" }
filmcraft-h264enc = { path = "../filmcraft/crates/h264enc" }
```

## Hardware video

*Measured 2026-10-22, M1 Max, macOS 27.2, FilmCraft rev d643a886. Whether handing decode and encode to the Mac's media engine
pays, for the Mac that would work through a sync server's videos while idle: lighter copies of every video, and poster
frames.*

### Verdict

| | Verdict | Why |
|---|---|---|
| **Encoding lighter copies** | **Go**: on a Mac, in hardware | VideoToolbox H.264 / HEVC encode took 0.4–0.6 CPU-ms per 720p / 1080p picture; FilmCraft's pure-Rust encoder took 80–155. About **140–310x less CPU time and 33x the speed** (two runs; the tables below are the second). A whole 2160p to 1080p copy costs 9 CPU-ms per picture instead of 177: one hour of 2160p30 video is **16 CPU-minutes instead of 5.3 CPU-hours**. A server in software did about 5 pictures a second at 1080p with every core busy (on a busy Mac), so "lighter copies of all videos" is only practical on a Mac; keep the server's software fallback opt-in and low priority. |
| **Decoding for that copy** | **Go**, with the encode | Hardware decode alone buys little (path B: 1.3–1.5x less CPU, the encode dominates), but once the encode is in hardware the software decode is what is left (path D uses 5x and 8x the CPU of path C, at 1080p and 2160p). So the helper needs both. |
| **Poster frames** | **No-go as a speed-up**; go only to fill gaps | A poster costs the server 0.09–0.27 CPU-seconds in software, whatever the codec. Hardware decode of the key frame is 25x cheaper (113 to 4.5 CPU-ms for 4K HEVC, where decoding is most of the poster; a fraction of it for H.264), but 10,000 clips are 25–45 CPU-minutes, once. Keep posters on the server. The helper's `videoPosters` part is worth building for clips the server cannot decode at all (4:2:2, 10-bit H.264, interlaced), which VideoToolbox takes. |
| **Scaling on the CPU** | The next cost | In path C the planar scale is 2.7 of 3.9 CPU-ms (1080p to 720p) and 5.9 of 9.1 (2160p to 1080p). The bench's scaler is a naive single-thread bilinear; make the product's parallel or SIMD before reaching for `VTPixelTransferSession`. |

### What the hardware flags said

Not assumed; read back in every row of every run:

- `filmcraft_platform::register()` returned `Available("VideoToolbox")`, and `hw_backend()` says `VideoToolbox`.
- `hardware_decoder_for(stream)` was **true** for all four clips; `hw_stats()` counted exactly as many pictures decoded by
  hardware as there are samples (120, 60), with **0 declined and 0 fallbacks**, in every hardware decode run.
- `VtEncoder::uses_hardware()` (the session property `UsingHardwareAcceleratedVideoEncoder`, read from the live session) was
  **true** for H.264 and HEVC in every encode run. The sessions are created with hardware *required*
  (`kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder`, and the decoder's counterpart), so a Mac without
  one fails instead of falling back silently.
- The process CPU time (user + system, every thread) next to the wall clock: it falls to 0.4–3 CPU-ms per picture, while a
  software run uses a core's worth.

### Numbers

Procedural 1080p and 2160p clips (a drifting pattern with grain, made by FilmCraft's pure-Rust encoder: High profile,
2 B-frames, a key frame a second, 12 and 40 Mb/s), 120 and 60 pictures (4 and 2 s at 30 fps); medians of 3 repetitions for
CPU time, the best of 3 for the wall clock. The software encoder is FilmCraft's `Balanced` preset, Main profile, no
B-frames, all cores; VideoToolbox has no B-frames either. The HEVC clips are the same pictures encoded by VideoToolbox.
The second run (below) repeated the first within a few percent for CPU time; the wall clock moved 2x with the load.

Decode only (CPU-ms per picture, and pictures a second by the wall clock):

| clip | software | VideoToolbox | CPU time |
|---|---|---|---|
| H.264 1080p | 17.2 ms · 59/s | 0.91 ms · 426/s | 19x lower |
| H.264 2160p | 61.1 ms · 16/s | 2.90 ms · 168/s | 21x |
| HEVC 1080p | 22.9 ms · 35/s | 0.90 ms · 626/s | 25x |
| HEVC 2160p | 80.6 ms · 8/s | 2.88 ms · 219/s | 28x |

Encode only (the input is in memory, no decode, no scale):

| target | pure Rust H.264 | VideoToolbox H.264 | VideoToolbox HEVC |
|---|---|---|---|
| 720p at 2500 kbit/s | 80.6 CPU-ms · 7.5/s · made 2226 kbit/s | 0.54 · 252/s · 2496 | 0.42 · 251/s · 2500 |
| 1080p at 5000 kbit/s | 154.8 CPU-ms · 5.0/s · made 4336 kbit/s | 0.57 · 170/s · 5003 | 0.57 · 185/s · 4980 |

(The pure-Rust encoder undershoots its bitrate by 11–13%, so its pictures are a little worse than the hardware's at the
same ask; quality was not compared.)

The lighter copy, a clip decoded, scaled on the CPU and encoded (CPU-ms per picture · pictures a second by the wall clock;
the last column is what one hour of 30 fps video costs in CPU time):

| path | 1080p to 720p | 2160p to 1080p | an hour of 2160p30 |
|---|---|---|---|
| A software decode + software encode | 60.9 · 10/s | 176.8 · 5.2/s | 5.3 CPU-hours |
| B VideoToolbox decode + software encode | 45.6 · 9.9/s | 118.0 · 5.1/s | 3.5 h |
| C VideoToolbox decode + VideoToolbox encode | **3.9 · 52/s** | **9.1 · 26/s** | **16 CPU-min** |
| D software decode + VideoToolbox encode | 19.5 · 45/s | 69.6 · 9.5/s | 2.1 h |

In C the CPU goes to the decode's copy into planes (0.8 CPU-ms at 1080p, 2.7 at 2160p), the scale (2.7 and 5.9) and almost nothing to the encode (0.3 and 0.5).

A poster (one key frame; median of 10; CPU-ms · wall-ms):

| clip | software decode | VideoToolbox, new session | VideoToolbox, session open | `Clip::poster(512)` today |
|---|---|---|---|---|
| H.264 1080p | 10.9 · 20 | 2.4 · 15 | 1.9 · 12 | 129 · 550 |
| H.264 2160p | 37.4 · 158 | 5.1 · 40 | 5.0 · 29 | 265 · 2233 |
| HEVC 1080p | 22.6 · 32 | 2.2 · 11 | 1.8 · 11 | 91 · 170 |
| HEVC 2160p | 113.5 · 695 | 4.5 · 33 | 4.4 · 28 | 154 · 765 |

The last column is the product's whole poster (open the file, decode, convert to linear Rec.2020 `f32`, resize): the
decode is the smaller part of it for H.264, and the rest (the full-size `f32` conversion; pictures fed after the key frame
until the decoder's reorder buffer releases one) was not profiled. See the follow-ups.

### What was not measured

- **The machine was busy**: other builds kept the load average between 300 and 1000 throughout, so the wall clock is 2–5x
  slower than on a quiet Mac and noisy (the two runs differ by up to 2x). CPU time repeated within a few percent, and every
  conclusion above rests on ratios of 15x or more. Re-run on a quiet machine before quoting a wall-clock figure.
- **No `powermetrics`**: it needs `sudo`. To see the media engine work, run
  `sudo powermetrics --samplers gpu_power,cpu_power -i 1000` in one terminal and the bench in another, or record Instruments'
  *Video Toolbox* template; the flags above are the evidence until then.
- **Not inside the Mac app's process**: whether its sandbox and entitlements allow hardware sessions, and its memory gate,
  are untested (`vision.helperOnce` on a dev server is the way to check).
- **No real iPhone clip**: the HEVC is procedural and VideoToolbox-encoded, 8-bit Main only. 10-bit HDR, 4:2:2 and
  interlaced clips, and HEVC from `corpus/`, are not covered. Clips are 2–4 s.
- **Quality** (PSNR / VMAF) of the lighter copies against the originals.
- The bench's scaler is a naive single-thread bilinear, not the product's.

### Adopting VideoToolbox: the options

`filmcraft-platform` (Apache-2.0) is the VideoToolbox code, and **it was not adopted**. At d643a886 its normal dependency tree
on macOS is **132 crates**. The ones that matter:

- `filmcraft-codecs`, which pulls **symphonia, MPL-2.0: 13 crates** (`symphonia`, `-core`, `-metadata`, `-bundle-flac`,
  `-bundle-mp3`, `-codec-aac`, `-codec-alac`, `-codec-pcm`, `-codec-vorbis`, `-format-isomp4`, `-format-ogg`,
  `-format-riff`, `-utils-xiph`). The workspace has none, and `cargo xtask layers` refuses both `filmcraft-codecs` and a
  `symphonia` package in `Cargo.lock`.
- `filmcraft-export` (and through it `filmcraft-render`, `-project`, `-text`, `-audio-dsp`, `-media`, `image`, a font stack)
  and every other FilmCraft codec.
- `unsafe`: `videotoolbox.rs` and `videotoolbox_encode.rs` (68 mentions in 1,250 lines), plus the `objc2-*` bindings
  (`objc2-video-toolbox`, `-core-media`, `-core-video` are not in our lock yet). `CLAUDE.md` allows `unsafe` only in
  `sysmem`, `ios-host` and `capture`.

What each file needs: `videotoolbox_encode.rs` (679 lines) depends on `objc2-*` only, no FilmCraft crate. `videotoolbox.rs`
(571 lines) needs `filmcraft_codecs::{hw::NalStreamInfo, DecodedFrame, VideoDecoder, CodecError}`, `filmcraft_frame` and
`filmcraft_color`; `hardware_encode.rs` needs `filmcraft_export`'s encoder trait.

1. **Ask FilmCraft for a split (preferred).** Two Cargo features: `audio` on `filmcraft-codecs` (default on; it gates
   symphonia and `audio.rs`) and the same for `filmcraft-export` on `filmcraft-platform`. Then
   `filmcraft-platform = { default-features = false }` brings neither symphonia nor the export stack. FilmCraft is the same
   organisation and owns the unsafe; the product only adds one dependency and a named `unsafe` exception.
2. **Vendor the two VideoToolbox files** into a small crate (`unsafe` allowed, a line in `CLAUDE.md`, a `NOTICE` entry,
   a sync script), the decoder rewired to our own planar `Frame`. The encoder alone is self-contained, but the helper needs
   both (path D vs C). About 1,250 lines to keep in step with upstream.
3. **Write a thin wrapper over `objc2-video-toolbox` ourselves**: the same size, no licence or sync question, all the FFI
   ours to maintain.
4. **Accept MPL-2.0 symphonia** (used unmodified, a `NOTICE` entry): the least work, but it widens the tree by 132 crates
   and the licence set, so it is the maintainer's call.

In all four the code lives behind a macOS-only `hw-video` feature of `lightcraft-video` (never wasm), and the engine reaches it
through the `VideoDecoder` / encoder traits.

### Running it again

The bench is `crates/video/hw-bench`, **a crate of its own outside the workspace** (its own `[workspace]` and `Cargo.lock`,
ignored by git), because it depends on `filmcraft-platform`: building it leaves the product's `Cargo.lock` and `cargo xtask
layers` untouched.

```bash
cd crates/video/hw-bench
cargo run --release --example hw_agent_bench -- --frames 1080=120,2160=60 --reps 3 > hw-bench.md
```

It prints Markdown (machine, load average, the flags, every table above) and makes its clips in the system temp folder
once. On other systems it says it skipped. `--dir` moves the clips; `--frames` takes `height=pictures` pairs.

### Follow-ups this found

- `Clip::poster` costs 90–265 CPU-ms where the decode is 11–113: profile the `f32` conversion and the pictures decoded after
  the key frame (probably: the decoder's reorder buffer holds the key frame back until later pictures are fed, where a
  `flush()` after the key sample returns it at once, as the bench does), and convert after the resize instead of before.
- The CPU scale in the lighter-copy path (parallel or SIMD) is now the largest cost of path C.

## Library stats

A sync server's admin page and `GET /api/usage` count a library's **photos and videos apart** (Recently Deleted and
virtual copies left out; counted once per `Catalog::library_revision`), with the size of the videos' own files and, once
the server keeps them, of the lighter copies it made and what they saved (`proto::Usage` `videos`, `video`; the Users list
`videos`, `videoBytes`, `videoCopies`, `videoCopyBytes`, `videoSaved`). The web admin shows them on Overview, Users, a
user's page and Storage; a native frontend reads the same JSON through `host.admin.call` (`users`). The fields are in
[sync.md](sync.md) (the `GET /api/usage` and `GET /api/admin/users` rows). Nothing makes lighter copies yet.

## Tests

Nothing is committed but synthetic clips: `lightcraft_video::testdata` (feature `testdata`) encodes flat-grey H.264
pictures with FilmCraft's encoder and muxes them as MP4, with or without B pictures (and a sound track that is not sound).

Real clips are opt-in: `LIGHTCRAFT_VIDEO_CLIPS=<folder of .mp4 / .mov / .m4v> cargo test -p lightcraft-video --test clips`
probes each, makes its poster, reads its middle and its first second in order (`skip!` without the variable). On
2026-10-22 it passed on clips made with ffmpeg: x264 1280 x 720 with AAC sound, a clip with a 90 degree display matrix (read
as 720 x 1280), and VideoToolbox's HEVC at 960 x 540 (8-bit) and 1920 x 1080 (Main 10). iPhone clips (Dolby Vision HLG, Live
Photo movies) have not been tried.
