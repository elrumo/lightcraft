# Apple RAW 9 (Core Image) against the engine, and as a fallback for raws we can't decode

> **Last updated:** 2026-10-25 · **Status:** measured; the linear stand-in is built for a native Apple frontend (below) · **Tool:** [`tools/apple_raw_compare.sh`](../tools/apple_raw_compare.sh)

Two questions: how does Apple's RAW decoder (`CIRAWFilter`, decoder versions 6 to 9) compare with ours on the raws we
both open, and can it stand in for a decoder we don't have (compressed ORF, Nikon HE, CRW, old ARW) so that the photo
edits like any other raw, with the user noticing nothing.

## Method

`tools/apple_raw_compare.sh FILES|FOLDERS…` (macOS 27 SDK, builds `tools/apple_raw_compare.swift`) renders each file
twice at full size, sRGB and upright: `lightcraft-cli render` (engine defaults) and `CIRAWFilter` (Apple's defaults, newest
decoder the file has). It then measures, per file:

| Column | Meaning |
|---|---|
| `dE00`, `dE00_exp_matched` | CIEDE2000 on a 96-cell grid of linear-light means; the second after the single exposure that best maps Apple's luminance onto ours. **This is the distance between two default looks, not an error against truth.** |
| `noise_ratio`, `detail_ratio` | Apple over engine, in the same 16 x 16 blocks (flattest blocks for noise, edge blocks for detail), per brightness band, divided by the local contrast gain between the two pictures so a stronger tone curve doesn't read as more noise. 1.0 = equal, below 1 = less. Detail also drops when the picture is merely blurrier. |
| `apple_s`, `engine_s` | Wall time. Engine time includes process start, import and PNG encode, so only `apple_s` is a decode time. |
| note | Which side could not open the file, and why (the engine's `preview_only` reason). |

`--coverage` skips pixel metrics and only records who can open each file (engine verdict from `photo.inspect`,
Apple's decoder and size, a 512 px render to prove it makes a picture). `--version newest|9|8|default`, `--nr-off`,
`--lum-nr`, `--col-nr`, `--set CONTROL=VALUE` (engine slider), `--crop N` (centre crops to look at), `--list-models`.

Corpora: a 144-file set of CC0 samples from raw.pixls.us (one per model, a native frontend's test folder) and a 772-file,
14 GB superset of the same archive kept in a scratch folder (neither is in this repo; `corpus/raw` holds 40 of the files).
Engine build: the `lightcraft-cli` release binary of 2026-10-25.

## Findings

**1. Apple's default is not the newest decoder.** An EOS R6 CR3 opens with decoder 8 unless the app sets 9
(`supportedDecoderVersions` = 7, 8, 9). A frontend that wants 9 has to set it.

**2. Decoder 9 covers few cameras.** `supportedCameraModels(with: .version9)` lists 69 of Apple's 941 models (recent
Canon R and EOS, Sony A6xxx / A7 / ZV, Panasonic S5 II and compacts, Fuji X-T5 / X-T50, Nikon Z50 / Z fc / Z6 II / D750 /
D850, Sigma fp). None of the Nikon Z8 / Z9 / Z6 III, OM System or Olympus bodies, and no phone. Everything else uses 7 or 8.

**3. On the 105 files both sides open, Apple's picture is close to ours but quieter.** Medians: `dE00` 4.4 (IQR 3.2 to 6.8),
3.8 after an exposure match; noise 0.61, detail 0.98. By decoder, noise / detail: v7 0.42 / 0.86, v8 0.66 / 1.08,
v8 DNG 0.79 / 0.89, v6 0.85 / 0.86. The two bodies where our default look is fitted to Lightroom exports are the
closest: EOS R6 `dE00` 2.5, EOS R 1.7. 100% crops line up to the pixel; the colour is slightly punchier, the
out-of-focus areas smoother. Several Apple outputs differ in size from ours (a few pixels, or a different aspect:
Sony APS-C and 16:9 frames, a Panasonic multi-aspect body): Apple keeps the active area, we apply the maker's default crop.

**4. RAW 9 did not beat RAW 8 on this corpus.** Same 47 files (58 v9 models in the corpus), medians: v9 noise 0.72,
detail 1.00, `dE00` 4.62, 1.6 s per decode; v8 noise 0.64, detail 1.05, `dE00` 3.94, **0.3 s**. Two high-ISO bodies
(FX3 ISO 20000, A7C ISO 2000) with luminance noise reduction forced to 0: v9 noise / detail 0.68 / 0.63 and 0.72 / 0.63,
v8 0.55 / 0.50 and 0.62 / 0.55; detail per unit of noise is the same. v9 has no colour noise reduction control (it is
reported unsupported) and picks its own luminance amount (0.29 at ISO 20000, where v8 picks 0.73). The corpus is almost
all low ISO and the metric can't see what a machine-learned demosaic does to texture, so this says "not shown", not
"no better": a blind visual comparison on high-ISO files is the missing evidence.

**5. As a stand-in decoder it covers most of what we refuse.** Coverage over 772 files:

| Engine | Files | Apple opens full size |
|---|---|---|
| decodes | 601 | 554 |
| **preview-only** (embedded JPEG) | 117 | **81 (69%)** |
| import fails | 54 | 4 |

The 81: Olympus / OM compressed ORF and ORI 50 of 50, Canon CRW 14 of 17, old Sony ARW (v1 packed, no CFA IFD) 9 of 9,
Nikon HE / HE★ NEF 6 of 6 plus 1 compressed NEF without a linearisation table, Sony SRF 1. It does not open Samsung SRW
(10), Phase One IIQ (8), Sigma X3F (5), Kodak KDC (5), Mamiya Leaf MOS (2), 2 RAF, 3 CRW. A contact sheet of 15 of the 81
(every format above) shows correct, well-exposed pictures. Nothing here checks colour against the camera.

**6. The embedded JPEG is often full size, so resolution is not the gain; latitude is.** The Z8 HE NEF's camera JPEG is
8256 x 5504, which is what the engine edits today. Decoded linearly (`boostAmount` 0, gamut mapping off, EDR 2) the same
file has 8.7% of sampled values above 1.0 (max 2.6): highlights the JPEG clips. Exposure -1.5 EV on the three
candidate sources (Z8, same photo):

- camera JPEG (today): sky is flat grey;
- Apple 16-bit display-referred TIFF: **the same flat grey**, only 16-bit;
- Apple linear half-float TIFF: blue sky with gradation.

So a display-referred Apple render buys 16-bit and Apple's look, nothing in editing range. Only a linear source does.
(The engine reads a float TIFF as an image and shows it unmapped, which is why the linear column is darker at 0 EV: a
linear source has to enter through the raw path, with a look, not as an image.)

**7. Cost.** Apple full-size decode: 0.1 to 0.5 s at 512 px, 0.9 s for 45 MP at v8 to half-float, about 1.7 s for 20 MP
at v9, 2.4 to 3.7 s for 80 MP ORI and Z9 HE; 450 to 540 MB resident at 20 MP. Persisting it: a 16-bit TIFF is 346 MB
for 45 MP and the engine takes 14 s to read it back, so unusable; 16-bit JPEG XL from the engine's encoder is 18.7 MB
(quality 95), 6.2 MB (85) or 39.7 MB (lossless) and takes 4 to 6 s to write; reading a 45 MP JXL back for a 2500 px render
takes 9 to 11 s against 0.8 s for the JPEG fallback. First open of a big file therefore needs the proxy path, not the
full-size source.

**8. A display-only fallback jumps.** A frontend that shows the system decoder's picture in its loupe without giving it to
the engine loses it at the first edit: the engine renders from the camera JPEG, with a different look and a clipped sky.
It is a preview, not an editor.

## Is it viable as an editor fallback?

Yes for the 69% above, as long as the source is **linear** and enters the engine as a raw. A display-referred
source isn't worth building (finding 6). It cannot be made fully invisible without three pieces the engine doesn't have
yet.

**What a seamless version looks like**

1. *Trigger*, already there: `photo.inspect` says `preview_only` and the original is a local file. No setting, no dialog.
   CIRAWFilter exists since macOS 12 / iOS 15, so don't gate it on 27; pin `decoderVersion` per photo (the older
   versions stay available, so the picture doesn't change when the OS does).
2. *Build in the background* under a `TaskGuard` (docs/background-tasks.md): camera JPEG shows at once, then the
   linear development replaces it with no visible step (same framing, the look matched to the JPEG's tone so the swap
   isn't a jump). Loupe-scale proxies first (a 512 to 2560 px decode is cheap), full size for export and 1:1.
3. *Hand it to the engine as a raw*: the frontend only supplies pixels (half-float linear, a named colour space,
   as-shot temperature and tint from `neutralTemperature` / `neutralTint`, orientation, the decoder version); the Rust
   engine writes a LinearRaw DNG with its own `dngwrite`, which it already reads back (Photo Merge output). Edits,
   Kelvin sliders, export and undo then behave as for any raw. The extraction rule applies: the command and the file
   format live in `engine` / `raw`, not in `ui-egui` or a native frontend.
4. *Cache and sync*: the derived file is a new asset kind next to the original (`Derived/<hash>`, JXL or lossy-DNG
   sized, 6 to 19 MB per 45 MP). Without it another device can't edit the photo as a raw.

**What stays hard**

- **Edit semantics change when the source does.** A photo already edited on the JPEG has display-referred settings; the
  same numbers on a linear source look different. Attach only to photos with no edits, or on an explicit "Develop with
  Apple" that converts or discards the old edits. Never silently.
- **Other devices.** Windows, Linux, web and the server can't run `CIRAWFilter`. They need the derived blob, or they keep
  the camera JPEG and show raw-semantics edits on it wrongly. A device that has neither should say so instead of rendering
  something different (`Smart preview` style state in `sync.original`).
- **Look.** The result carries Apple's look, not the camera's JPEG look (the Camera Look profile work does that for
  CR3) and not Lightroom's. Per-camera colour fitting stays with the engine.
- **Memory on iPhone** (a 45 MP half-float buffer is 346 MB): tile it (CIImageProcessor) or write in strips, and check
  `os_proc_available_memory` first.
- **Coverage ends at 69%**: SRW, IIQ, X3F, KDC and MOS stay preview-only; the badge must say which path a photo is on.
- **Provenance**: Apple's output is pixels from an OS API, used at runtime. It is not a reason to reopen the CRW or Nikon
  HE decoder questions (refused for provenance, ORF paused for the maintainer's choice). Using it as an oracle to build our
  own decoders is the same "output only" question as CR3's; that call is the maintainer's.

**Rough size**: engine command + DNG-from-buffer + catalog field + sync blob + tests is a feature of its own (several
days, mostly the sync and edit-semantics work); the frontend's decode side is small.

## Recommendation

1. Don't ship a display-referred Apple source.
2. If the Olympus / OM, Nikon HE and old Canon / Sony users matter, build the linear path above for photos without edits first;
   expect the first version to be Mac and iPhone only for the derivation, with the blob synced to the rest.
3. Until then, make the existing display-only fallback stop jumping: keep Apple's picture as the loupe's source for a photo
   whose engine source is the camera JPEG only if the first edit doesn't swap it back, or drop it.
4. Decoder 8 would do display-only decodes in a fifth of the time at the same measured quality; the frontend takes RAW 9
   wherever a file has it anyway (the maintainer's call), and 8 or older for everything else, since only 69 models have 9.

## Built (2026-10-25): the linear stand-in

Scope as decided: only a native frontend on Apple platforms develops with Core Image; other platforms (and the server)
are not a concern for it, so the file never has to be re-derived elsewhere. Sync of the derived file is therefore not built.

- `lightcraft_raw::write_linear_dng` (`crates/raw/src/dngwrite.rs`): DNG from a linear RGB buffer in a named space.
  `LinearDngLook` chooses `headroom` (samples above 1 are stored under the next power of two with `BaselineExposure`
  raised by those stops, so nothing clips at `WhiteLevel`), `identity_tone` (off: the reader gives its default look),
  `compact` (16-bit integers under lossless JPEG, or half floats under deflate) and a JPEG `preview`, written in a
  sub-IFD where `embedded_preview` finds it. `write_rendered_dng` is the same call with the old settings.
- `photo.attachDecoded {id, path, previewPath?, baselineExposure?, compact?, force?}` (`crates/engine/src/cmd/stand_in.rs`):
  reads the host's linear float TIFF, writes the DNG under `<library>/Derived/<hash>/`, checks it reads back, then relinks the
  photo and its virtual copies like Convert to DNG does and lets `heal_preview_only` bring size, hash, raw defaults and
  cleared renders. Not an undo step. Refused (and left untouched) for a photo with edits unless `force`, for anything
  that isn't a preview-only raw, and for a missing, empty or hostile file (tests: `cmd::stand_in::tests`,
  `rendered_dng`).
- The host also sends its default rendering as `previewPath`. With it the engine's existing DNG tone fit
  (`fit_dng_tone`, "DNGTONE accepted") gives the raw Apple's starting look; without it the raw renders flat and dark
  (mean luma 108 against 135 on an OM-1 file).

Measured on real files, through the frontend's own development code and this command:

| File | Develop (host) | `attachDecoded` | DNG | Starting look, mean luma (Apple / engine) |
|---|---|---|---|---|
| OM-1 ORI, 20 MP | 5 s | 3 to 8 s | 78 MB (compact), 85 MB (half float) | 135.4 / 133.5 |
| Olympus E-M1 II ORF, 20 MP | 6 s | 3.4 s | 68 MB | not measured |
| Nikon Z8 HE NEF, 45 MP | 22 s (mostly writing the 346 MB TIFF) | 15 s | 155 MB | 99.6 / 98.6 |

At exposure -1.5 EV the Z8's sky keeps its blue where the camera JPEG goes flat grey (finding 6): the point of the
exercise. Open: the DNG is 3 to 4 bytes a pixel (a lossy JPEG XL tile DNG, which the reader already decodes, would be
about a tenth of it, but the raw crate has no JPEG XL encoder); the white balance sliders start at D65 / 0 tint because
the picture is already balanced (the camera's own temperature is not carried); a photo with edits stays on the camera JPEG.

## Not covered

No comparison against Lightroom or the camera JPEG; no colour check of Apple's output on the 81 rescued files; no
high-ISO corpus beyond two files; decode timings were taken while another job ran for some passes; the linear source's latitude was shown with a
half-float TIFF through the engine's image path, not a real LinearRaw DNG built from Apple's pixels, so how well the
raw pipeline (white balance, look, highlight reconstruction) treats it is unmeasured.
