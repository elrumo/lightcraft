# Video clips

> **Last updated:** 2026-10-22 · **Change:** minor (Lighter copies: a sync server keeps a smaller copy of every video beside the original, set from the admin page, made by a Mac helper later or by the server's own pure-Rust encoder at the lowest priority; the helper and replacing originals remain; poster frames cost 3 to 6 times less CPU: profiled, the decoder was fed pictures past the key frame and every pixel paid three `powf`; Hardware video: VideoToolbox decode and encode measured against the pure-Rust ones on a Mac, with a go / no-go; the FilmCraft pin moves with `cargo xtask filmcraft`; library stats; Live Photos, `live.*`; import, poster frames, `video.info` / `video.frame`; first revision)

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
  key frame before `t` and decodes forward (B pictures follow presentation order). `LIGHTCRAFT_PROFILE=1` prints the
  stages of a poster (reading the sample, feeding the decoder, the flush, the colour conversion) to stderr. The picture
  comes back at most about twice the edge asked for on its long side (`yuv::to_working_within`), so a thumbnail's resize
  starts from fewer pixels.
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

## Lighter copies

A sync server can keep a **lighter copy** of every video beside the original, so a library of phone clips streams and
downloads at a fraction of the size. The original is never touched. Admins turn it on from the admin page, **Processing
▸ Videos** ([sync.md](sync.md#the-admin-page)); it is off by default.

- **Settings** (`<data>/video.json`, server-wide, `GET` / `PUT /api/admin/video`): the format (`h264`, or `hevc` for a Mac
  helper), the biggest size (`original`, 1080, 720, 480; a clip is never made bigger than it is), the quality
  (`smaller` / `balanced` / `better`), `skipIfSmaller`, `serverFallback` and `helper`. The bitrate is worked out from
  the picture's height and rate: Balanced is 5000 kbit/s at 1080p30 and 2500 at 720p30, interpolated between 240p and
  2160p, scaled by the frame rate (0.5x to 2x of 30), by 0.6 / 1 / 1.5 for the quality and by 0.7 for HEVC. A damaged
  file means the defaults; a field that is not one, or a value that is not one of its choices, is refused naming it.
- **The copy** is the blob kind `video` (`blobs/video/<xx>/<hash of the original>`, an MP4; same duration, the pictures
  turned as displayed, BT.709 tags, the sound copied as it is). Beside it, outside `blobs`, a record
  (`users/<name>/video/<hash>.json`) holds the codec, height, bitrate, the sizes, whether a hardware encoder made it, the
  settings it was made with, the copy's digest, or the mark "already efficient", or why the clip failed. `skipIfSmaller`
  keeps no copy that is not at least 15% smaller than the original (the clip is marked and not tried again). The copy
  counts in the user's storage limit and in `Usage.video.encoded`, and `gc` removes it, and its record, a day after its
  video has left the library. A copy is uploaded with `PUT /api/blobs/video/<hash>` and its digest in
  `X-LightCraft-Digest` ([sync.md](sync.md#protocol)).
- **The queue** is every video in a user's library (not in Recently Deleted, not a virtual copy), existing and new,
  with no copy and no mark. The admin page shows how many are done of how many, how many are left, the time that takes
  at the pace so far, what is failing and why (`unsupported`, `unreadable`, `encoder`), and can pause, resume, try the
  failed ones again, and make the copies again that were made with other settings (**Encode existing videos now**).
  Devices see it in `GET /api/activity` (`video`). A video whose original the server does not have yet (a device has not
  uploaded it) waits and is not counted as left.
- **Who encodes.** A Mac helper claims videos (`video::Videos::next_job`, `fail`, `finish`: the Rust surface; the claim
  routes, leases that renew and the helper itself are not built). When nothing can encode, the panel says **waiting for a
  helper**. With `serverFallback` on the server encodes by itself with FilmCraft's pure-Rust H.264 encoder
  (`lightcraft_video::lighter_copy`, behind the `VideoEncoder` trait): one video at a time, with a panic in it a failed
  clip, not a crash, and stopped at once by a pause or a change of setting. It runs at the lowest priority
  (`lightcraft_sysmem::lower_thread_priority`: nice 19 on Linux, background quality of service on macOS) on a rayon pool of
  one thread of its own (`video::encode_pool`). The encoder and the decoders are set to one thread, but the encoder's
  look-ahead, the scaler and the colour conversion call rayon, which would have run them on the global pool at normal
  priority; installed on that pool, they stay on the one lowered thread (a unit test and the queue test check it). A
  thread at the lowest priority gets almost no time on a machine other work has saturated (a test run with 60 busy loops
  added to a machine at load 1000: the worker did not reach its first clip in a minute, where the same tests at normal
  priority passed in 51 s), which is the intent for a server; `Config::video_low_priority` (default on) turns it off, and
  the tests do, unless `LC_TEST_VIDEO_LOW_PRIORITY=1`. It makes H.264 only, and refuses HDR clips (the copy would be squeezed into SDR). It is slow (see
  *Hardware video*: about 5 pictures a second at 1080p with every core; on one background thread a 6 s 720p clip took
  about 165 s on an M1 Max, a bit over one picture a second), which is why it is opt-in. Checked on a scratch server
  with ffmpeg-made clips: a 720p H.264 clip with AAC sound came back as a 1.9 MB copy of 3.6 MB (2.49 Mbit/s for the
  2.5 asked, 6.000 s, BT.709 tags, sound kept, and ffmpeg decodes it cleanly), a 4:2:2 clip is listed as failed with its
  reason, and an 87 KB clip whose copy would have been 352 KB is marked already efficient.
- **Not built yet:** the helper (claim routes, a Mac encoding with VideoToolbox when idle), replacing uploaded originals
  with their copies, devices choosing the copy to play or download ("Prefer lighter videos"), serving the copy for a
  device that asks for a video the server holds no original of, and urgent copies for a clip a device is trying to play.

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
| **Poster frames** | **No-go as a speed-up**; go only to fill gaps | A poster cost the server 0.09–0.27 CPU-seconds in software, whatever the codec, and 0.016–0.114 after the profile below found what was wasted. Hardware decode of the key frame is 25x cheaper (113 to 4.5 CPU-ms for 4K HEVC, where decoding is nearly all of the poster now; a fraction of it for H.264), but 10,000 clips are 3–19 CPU-minutes (15–45 before the fix), once. Keep posters on the server. The helper's `videoPosters` part is worth building for clips the server cannot decode at all (4:2:2, 10-bit H.264, interlaced), which VideoToolbox takes. |
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

| clip | software decode | VideoToolbox, new session | VideoToolbox, session open | `Clip::poster(512)` before | after |
|---|---|---|---|---|---|
| H.264 1080p | 10.9 · 20 | 2.4 · 15 | 1.9 · 12 | 129 · 550 | **16.0 · 10.6** |
| H.264 2160p | 37.4 · 158 | 5.1 · 40 | 5.0 · 29 | 265 · 2233 | **45.7 · 35.5** |
| HEVC 1080p | 22.6 · 32 | 2.2 · 11 | 1.8 · 11 | 91 · 170 | **28.4 · 22.6** |
| HEVC 2160p | 113.5 · 695 | 4.5 · 33 | 4.4 · 28 | 154 · 765 | **114.0 · 104.2** |

The last two columns are the product's whole poster (open the file, decode, convert to linear Rec.2020 `f32`, resize). The
"before" column is the first run (a rerun the same day, same machine, gave 126 · 122, 258 · 443, 90 · 57 and 154 · 248); "after" is
a run with the load average at 890, so its wall-clock times mean little and its CPU times are what to read (a quiet-machine
rerun is still to do). The poster is now within 0.5–8 CPU-ms of decoding its key frame.

#### Where a poster's time went

`LIGHTCRAFT_PROFILE=1 RAYON_NUM_THREADS=1 cargo run --release --example poster_profile -- CLIP.mp4` in `crates/video/hw-bench`
(milliseconds of one thread, on the busy machine, so only the ratios mean something; the decode itself did not change, the
spread in its figures is the load):

| | key sample fed | fed after it | colour conversion | open the file |
|---|---|---|---|---|
| H.264 1080p before → after | 10 → 9 | **64–82** → 0 | **103–108** (1920 × 1080) → 6 (960 × 540) | 0.1 |
| H.264 2160p | 52–81 → 35 | **463–479** → 0 | 32–38 → 10 | 0.1 |
| HEVC 1080p | 23–25 → 22 | 0 → 0 | **91–95** → 7 | 0.1–0.3 |
| HEVC 2160p | 183–196 → 106 | 0 → 0 | 32 → 10 | 0.1–0.2 |

- **Pictures fed past the key frame (H.264 with B pictures).** The decoder holds the key frame back until the next picture
  is fed, and `Clip::decode` kept feeding samples until one came out: the second sample alone took 64–82 ms against 10 for the
  key frame at 1080p (463–479 against 52–81 at 2160p). Every picture the caller wants is among the samples already fed;
  `flush()` releases what the decoder holds at once (0.3 ms), so `decode` stops at its last sample and flushes (`frame_at`
  gains the same).
- **The conversion.** `to_working` reduces by the largest whole factor that keeps the picture at least twice the edge asked
  for: 1920 / 1024 rounds down to 1, so a 1080p clip was converted at full size for a 512 px ask (2 million pixels, three
  `powf` each, about 50 ns a pixel), against the doc's "at most about twice the edge". A poster now rounds up (960 × 540
  for 1080p and 2160p alike: 4 times fewer pixels than before at 1080p, 1.8 times at 2160p, where it was 1280 × 720), the
  transfer curve is a 4096-step table (largest difference from the formula 3e-7 for the SDR curves, 2e-4 for PQ), and the
  block sum reads whole rows. The pictures are the same: `yuv::tests::the_fast_conversion_is_the_old_one` holds it to the old per-pixel code, and
  `decode::tests` compares a poster of clips with and without B pictures, turned 0 to 3 quarters, against the old feed-on
  decode and the old conversion (differences under 2e-6; the size differs only where the old rounding-down made the
  picture larger than the edge allows).
- **Opening the file** is 0.1–0.3 ms (the header only).

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

1. **Ask FilmCraft to move the video-decoder half of `filmcraft-codecs` into a crate of its own (preferred; chosen
   2026-10-22).** What `filmcraft-platform`'s decode half takes from `filmcraft-codecs` is the registry
   (`register_video_decoder`, `software_video_decoder`), `VideoDecoder`, `DecodedFrame`, `CodecError` / `Result` and the
   `hw` module: `lib.rs`, `video.rs`, `hw.rs`, `hw_frame.rs` and `stream_color.rs`, none of which uses symphonia.
   Symphonia is in `audio.rs` (its `CodecType` is in signatures that mp4, mkv, mpeg, avi and ogg call, so a plain `audio`
   feature on `filmcraft-codecs` is not a small change). With the video half in its own crate, which `filmcraft-codecs`
   re-exports so nothing breaks, and the `filmcraft-export` half of `filmcraft-platform` (`hardware_encode.rs`, the encoder
   registration) behind a default-on `export` feature, `filmcraft-platform = { default-features = false }` has **no
   symphonia and no export stack: a scratch `cargo tree` of the video half (16 FilmCraft crates: the decoders, parsers and
   `filmcraft-media`; plus the `objc2-*` bindings) is 76 crates instead of 132**, all clean-room Apache-2.0 pure Rust or already in
   our tree. FilmCraft owns the `unsafe`; the product adds one dependency and a named `unsafe` exception. The draft
   request is in the shared notes folder (`filmcraft-split-request.md`).
2. **Vendor the two VideoToolbox files** into a small crate (`unsafe` allowed, a line in `CLAUDE.md`, a `NOTICE` entry,
   a sync script), the decoder rewired to our own planar `Frame`. The encoder alone is self-contained, but the helper needs
   both (path D vs C). About 1,250 lines to keep in step with upstream.
3. **Write a thin wrapper over `objc2-video-toolbox` ourselves**: the same size, no licence or sync question, all the FFI
   ours to maintain.
4. **Accept MPL-2.0 symphonia** (used unmodified, a `NOTICE` entry): the least work, but it brings the whole 132-crate tree,
   13 of them MPL-2.0, so it is the maintainer's call.

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
once. `cargo run --release --example poster_profile -- CLIP.mp4...` (the clips it made, in `lightcraft-hw-bench/`) times only
the product's poster, opening the file apart from making the poster, and with `LIGHTCRAFT_PROFILE=1` prints the stages. On other systems it says it skipped. `--dir` moves the clips; `--frames` takes `height=pictures` pairs.

### Follow-ups this found

- ~~`Clip::poster` costs 90–265 CPU-ms where the decode is 11–113~~: done, 16–114 CPU-ms (see *Where a poster's time went*).
  Left: `frame_at` and `for_each_frame` still reduce by the rounded-down factor, so a chosen cover frame (`frame_at`) and a
  video export at 512 px from 1080p still convert 2 million pixels; moving them to the rounded-up factor changes the
  sharpness and size of what the export develops, so it wants a look at the export first.
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
`clip_textured` has a moving ramp and chroma that changes across and down the picture instead, for tests that must tell a
turned or mirrored picture, a wrong chroma sample or a wrong picture of a B-picture group from a right one: the poster and
`frame_at` tests (`decode/tests.rs`) compare against the decoder fed the long way and the conversion as it was
(`yuv/tests.rs` keeps the old per-pixel code as its reference), and one counts the bytes a poster reads from the file.

Real clips are opt-in: `LIGHTCRAFT_VIDEO_CLIPS=<folder of .mp4 / .mov / .m4v> cargo test -p lightcraft-video --test clips`
probes each, makes its poster, reads its middle and its first second in order (`skip!` without the variable). On
2026-10-22 it passed on clips made with ffmpeg: x264 1280 x 720 with AAC sound, a clip with a 90 degree display matrix (read
as 720 x 1280), and VideoToolbox's HEVC at 960 x 540 (8-bit) and 1920 x 1080 (Main 10). iPhone clips (Dolby Vision HLG, Live
Photo movies) have not been tried.
