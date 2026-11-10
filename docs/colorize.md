# Colorize

> **Last updated:** 2026-11-10 · **Change:** new

Colorize gives a black-and-white photo colours made by a model, as an edit: the colours are stored with the photo's
settings, so rendering, exporting, the sync server and other devices never run the model, and every other control still
acts on the result.

## The model

DDColor (Kang et al., ICCV 2023, [github.com/piddnad/DDColor](https://github.com/piddnad/DDColor); code and weights
Apache-2.0, trained on ImageNet) takes a 512 × 512 grey picture (the photo's CIE Lab lightness with a = b = 0, back in
sRGB) and answers Lab a and b for it. Two sizes: Tiny (ConvNeXt-T, `ddcolor_paper_tiny`) for phones and Large
(ConvNeXt-L, `ddcolor_modelscope`, the authors' default) for computers and large tablets.

The engine does not run it. A native frontend on Apple's systems runs Core ML conversions of it (fp16, CPU and GPU: the
Neural Engine gives wrong colours, CIEDE2000 median 6 to 9 against PyTorch, where CPU and GPU give 0.0), and names
the two models with its own mirror in the host's open options (`hostModels`); the engine lists, downloads and deletes
them with the other models (`models.*`, `HOST_RUN` in `crates/enhance/src/models.rs`) and offers them nowhere else.
Measured on an M1 Max (2026-11-10): Tiny 121 MB, 44 ms a picture; Large 467 MB, 99 ms.

## Commands

- `colorize.input {photo?, maxEdge?: 512}` → `{photo, width, height, png}`: the photo as the model sees it, uncropped
  and as its file is oriented, exposed to mid-grey (Lens Blur's model input).
- `colorize.apply {photo?, map: {width, height, data}, model?, on?}`: stores the model's answer and turns Colorize on,
  one undo step. `data` is base64 of `width × height` signed byte pairs (a, then b, in Lab units), row by row, over the
  picture `colorize.input` gave, at any size of its shape up to 1024. Without `map` it turns Colorize on or off.
- `colorize.clear {photo?}`: Remove Colors. `colorize.info {photo?}`: what the panel shows.
- `colorize.amount` (0–100, default 100) is a develop control.

## Storing and rendering

`DevelopSettings::colorize` holds `enabled`, `amount`, `model` and `map`, a `ChromaMap` (`crates/develop/src/segmask.rs`:
two signed bytes a pixel, delta-coded per row, deflated, base64, as Relight's normals). No copyable group carries it
(Copy Settings, presets): the colours are of this photo.

The stage (`crates/pipeline/src/colorize.rs`) runs in the scene-linear stage right after white balance. Each pixel keeps
its luminance Y and is multiplied by the colour Lab (L of Y, a, b) is in the working space, over that colour's
luminance: a grey pixel becomes the model's colour, while White Balance, a print's tone and every colour control after
it still act on the photo. Amount mixes the factors toward 1. The map is read through the photo's framing (orientation,
geometry, crop) bilinear. The GPU renderer runs this stage on the CPU (`lin_needs_cpu`).

## Not done

- Paste and presets do not make the colours again on another photo (no `ai_work` entry).
- Old-photo repair beyond colour: scratches (a detector whose mask feeds AI Remove) and faces (no model passes the
  weights policy yet: GFPGAN and CodeFormer carry non-commercial terms).
