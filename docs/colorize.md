# Colorize

> **Last updated:** 2026-11-12 · **Change:** minor (Colorize over the photo's colour or its black and white version; Neutral Highlights and Neutral Shadows; a developed photo reaches the model as it is)

Colorize gives a black-and-white photo colours made by a model, as an edit: the colours are stored with the photo's
settings, so rendering, exporting, the sync server and other devices never run the model, and every other control still
acts on the result.

## The model

DDColor (Kang et al., ICCV 2023, [github.com/piddnad/DDColor](https://github.com/piddnad/DDColor); code and weights
Apache-2.0, trained on ImageNet) takes a 512 × 512 grey picture (the photo's CIE Lab lightness with a = b = 0, back in
sRGB) and answers Lab a and b for it. Two sizes: Tiny (ConvNeXt-T, `ddcolor_paper_tiny`) for phones and a large one
(ConvNeXt-L) for computers and large tablets: `ddcolor_artistic` since 2026-11-12, the checkpoint whose colours are the
most like photographs' (below); the authors' default `ddcolor_modelscope`, the first large model, stays listed so a copy
already downloaded can be deleted.

The engine does not run it. A native frontend on Apple's systems runs Core ML conversions of it (fp16, CPU and GPU: the
Neural Engine gives wrong colours, CIEDE2000 median 6 to 9 against PyTorch, where CPU and GPU give 0.0), and names
the two models with its own mirror in the host's open options (`hostModels`); the engine lists, downloads and deletes
them with the other models (`models.*`, `HOST_RUN` in `crates/enhance/src/models.rs`) and offers them nowhere else.
Measured on an M1 Max: Tiny 121 MB, 44 ms a picture; the large ones 467 MB, 99 to 105 ms. The frontend runs the model on the
photo and on its mirror image and averages the two answers, so a colouring is two of those.

## Commands

- `colorize.input {photo?, maxEdge?: 512}` → `{photo, width, height, png}`: the photo as the model sees it, uncropped
  and as its file is oriented. A developed photo (a JPEG, a scan) goes as it is; a raw's scene-linear light is exposed
  to mid-grey first (Lens Blur's model input).
- `colorize.apply {photo?, map: {width, height, data}, model?, on?, base?: "color"|"bw"}`: stores the model's answer and turns Colorize on,
  one undo step. `data` is base64 of `width × height` signed byte pairs (a, then b, in Lab units), row by row, over the
  picture `colorize.input` gave, at any size of its shape up to 1024. Without `map` it turns Colorize on or off; `base` alone only changes what the colours go over (below).
- `colorize.clear {photo?}`: Remove Colors. `colorize.info {photo?}`: what the panel shows.
- `colorize.amount` (0–100, default 100), `colorize.highlights` (Neutral Highlights, 0–100, default 70) and
  `colorize.shadows` (Neutral Shadows, 0–100, default 40) are develop controls.

## Storing and rendering

`DevelopSettings::colorize` holds `enabled`, `amount`, `model`, `base` and `map`, a `ChromaMap` (`crates/develop/src/segmask.rs`:
two signed bytes a pixel, delta-coded per row, deflated, base64, as Relight's normals). No copyable group carries it
(Copy Settings, presets): the colours are of this photo.

The stage (`crates/pipeline/src/colorize.rs`) runs in the scene-linear stage right after white balance. Each pixel keeps
its luminance Y and is multiplied by the colour Lab (L of Y, a, b) is in the working space, over that colour's
luminance: a grey pixel becomes the model's colour, while White Balance, a print's tone and every colour control after
it still act on the photo. Amount mixes the factors toward 1. Before that the map's a and b are scaled by `tone_keep`
of the pixel's lightness: 1 in the mid tones, `1 − Neutral Highlights` at white (a smooth step from L 70 to 100) and
`1 − Neutral Shadows` at black (from L 25 down to 0). The map is read through the photo's framing (orientation,
geometry, crop) bilinear. The GPU renderer runs this stage on the CPU (`lin_needs_cpu`).

`base` (`ColorizeBase`, `color` by default, left out of the JSON then) is what the colours go over. A scan of a print is
seldom purely grey: `color` multiplies the colours onto the photo as it is, so its tint scales with them; `bw` first
takes the tint off, each pixel moving to its luminance by Amount, then lays the colours on. The black-and-white treatment
(and the `lc.mono` / `lc.bw.*` looks) is not applied to a colorized photo in either case: it would take the colours back
off. The `bw` base is the plain luminance: the treatment's own colour mixer does not shape it.

## How true the colours are

Measured 2026-11-12 on 124 colour photos (renders of the CC0 test raws) made grey, coloured again and compared with
their own colours (the script is with the model's conversion tools in the native frontend's repository). The large
model; dE is mean CIEDE2000 (it never reaches 0: no model can know a car's colour), chroma the share of the photo's own
colourfulness, tint the mean chroma laid on bright (L > 85) pixels that were neutral.

| Recipe | dE | chroma | tint on whites |
|---|---|---|---|
| Exposed input (the first version) | 10.81 | 1.35 | 12.8 |
| The photo as it is | 9.98 | 1.28 | 9.3 |
| … averaged with the answer for its mirror image | 9.79 | 1.27 | 9.2 |
| … Neutral Highlights 70, Neutral Shadows 40 | 9.53 | 1.19 | 4.5 |
| … at 0.85 of the model's strength (the frontend's choice) | about 9.2 | about 1.0 | about 3.8 |

So: the model paints more colour than photographs have and tints what should be white or black; the two sliders and the
frontend's strength (0.85 for the large model, 0.9 for Tiny, where it paints 1.09 times a photo's colour) bring it level
with real photos.

The large checkpoints, each as it is fed above with the two sliders at their defaults and at full strength:

| Checkpoint | dE | chroma | tint on whites |
|---|---|---|---|
| `ddcolor_modelscope` (the first large model) | 9.53 | 1.19 | 4.5 |
| `ddcolor_paper` | 9.49 | 1.09 | 4.4 |
| `ddcolor_artistic` (the large model now) | 8.97 | 0.99 | 3.7 |
| `ddcolor_paper_tiny` (Tiny) | 9.51 | 1.09 | 4.3 |

## Not done

- Paste and presets do not make the colours again on another photo (no `ai_work` entry).
- Old-photo repair beyond colour: scratches (a detector whose mask feeds AI Remove) and faces (no model passes the
  weights policy yet: GFPGAN and CodeFormer carry non-commercial terms).
