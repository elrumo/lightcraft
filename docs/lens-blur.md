# Lens Blur and depth

**Lens Blur** (Edit ▸ Lens Blur; on a phone, the Lens Blur group) blurs what is nearer or farther than
the subject, as a wide aperture would. It works from the photo's **depth**: how far each part of it is
from the camera. The same depth drives the **Depth Range** mask (Masking ▸ Range ▸ Depth).

## Where the depth comes from

1. **The photo's own depth map**, when the file has one. It is read with the photo and never copied
   into the settings:
   - iPhone **Portrait** photos: HEIC (a depth auxiliary image) and JPEG (a second, greyscale JPEG after
     the photo). Apple's metadata says whether the map holds disparity or distance, and over what range.
   - **DNG** files with a DNG 1.5 depth map (`DepthFormat`, `DepthNear`, `DepthFar`).
2. Otherwise an **estimate** by an AI model, **Depth Anything V2 Small**, made once when Lens Blur is
   first applied and stored with the photo's settings (about 30–60 KB). Renders and exports never run the
   model, and look the same at every size.

iPhone **ProRAW** DNGs carry no depth map (only Apple's segmentation mattes: a person or the sky), so
they use the estimate; a ProRAW's person matte, when it has one, is used to focus on the subject.

## Using it

1. Open Lens Blur and press **Apply**. With depth in the file it turns on at once; otherwise
   "Estimating depth…" shows for a second or two (longer in a browser), the first time after the model's
   download is offered (see below).
2. **Blur Amount** sets how strong the blur is (it grows with the difference in distance from the
   focal range, as a lens's does).
3. **Focal Range**: the bar runs from near (left, warm) to far (right, cool). Drag its ends to widen or
   narrow the range kept sharp, its middle to move it, or click the bar to centre it there. The **focus
   picker** (target icon; phone: Focus on a Point) focuses on the depth of the point you then click, and
   **Subject** focuses on the photo's subject. The range also has sliders (`lensBlur.focalDistance`,
   `lensBlur.focalRange`).
4. **Visualize Depth** paints the depth over the photo (warm near, cool far), as the blur uses it.
5. **Bokeh** picks the shape out-of-focus highlights take: Circle, Bubble (bright rims), 5-Blade,
   Ring (a mirror lens) or Cat Eye (clipped toward the frame's corners). **Boost** makes those
   highlights bloom brighter.
6. **Refine** fixes the depth where it is wrong: choose **Focus** and paint what should stay sharp,
   **Blur** and paint what should be blurred, **Erase** to take either back (⌥ paints with Erase on
   a computer). The brushes share the mask brush's Size, Feather, Flow and Auto Mask; Visualize Depth
   shows while you paint. **Clear Brushing** removes the strokes.

Copy, paste, presets and sync carry the Lens Blur settings and strokes but never the estimate: on
another photo, Lens Blur uses that photo's own depth (or needs Apply there to estimate it).

## The model

The model is optional and not part of LightCraft. **Depth Anything V2 Small** (Lihe Yang, Bingyi
Kang, Zilong Huang, Zhen Zhao, Xiaogang Xu, Jiashi Feng and Hengshuang Zhao, NeurIPS 2024; code and
Small weights **Apache-2.0**: <https://github.com/DepthAnything/Depth-Anything-V2>,
<https://huggingface.co/depth-anything/Depth-Anything-V2-Small>). LightCraft downloads the int8
ONNX export `onnx/model_int8.onnx` (27 MB) from onnx-community's repository
(<https://huggingface.co/onnx-community/depth-anything-v2-small>, Apache-2.0) at a fixed revision,
checked against its pinned size and SHA-256 (`crates/depth/src/lib.rs`), only after you agree. The
larger Base / Large / Giant models are non-commercial and not used. The `model_quantized.onnx` export
(uint8 weights) doesn't run on rten; the int8 one gives the fp32 model's depth to about 2 % of its range
(median) on the photos it was checked on.

It runs on [rten](https://github.com/robertknight/rten) (pure Rust) on every platform, at about
690 × 518 pixels. Settings → AI Models lists it (on / off, its folder, Delete); in the browser it is
kept in the site's storage (`models/depth-anything-v2-small/model_int8.onnx`) like AI Remove's model.

## Agents and scripts

- `lensBlur.apply {on?}`: turn on (estimating when needed) or off. Without depth and without the
  model it fails with "The depth model (Depth Anything V2 Small) is not downloaded…"; download it with
  `models.download {"id": "depth-anything-v2-small", "acknowledged": true}` after the user agreed.
- `lensBlur.estimate`: estimate again (also for a photo with depth of its own: the estimate then wins).
- `lensBlur.focus {x, y}` or `{subject: true}`; `lensBlur.bokeh {shape}`;
  `lensBlur.brushStroke {mode: focus|blur|erase, points, size?, feather?, flow?, autoMask?}`,
  `lensBlur.clearBrush {mode?}`; `lensBlur.info`.
- Sliders through `develop.set`: `lensBlur.amount`, `lensBlur.focalDistance`, `lensBlur.focalRange`,
  `lensBlur.boost`. Depth Range masks: `mask.add {kind: "depthRange", lo, hi, feather}` (0 near … 1 far).
- `view.visualizeDepth`; `render.view {overlay: {kind: "depth"}}`.

## How it renders

`crates/pipeline/src/lensblur.rs`: the depth is sampled like the image (orientation, lens corrections,
crop) and its edges are snapped to the photo's with a guided filter, so a subject's outline stays crisp
although the map is coarse. Each pixel's circle of confusion grows linearly with its distance in inverse
depth from the focal range. The blur is gathered: each pixel collects the neighbours whose bokeh shape
reaches it, each spreading its light over its shape; nearer neighbours are laid over it (a blurred
foreground spills over a sharp subject) and farther ones can't spill over a pixel sharper than them.
Big blurs are gathered on a coarser grid, the transitions near the focal range at full size. Zoomed
views render exactly as the same pixels of the whole frame. It runs after noise reduction, on the CPU
(the GPU renderer hands it over). About 0.2–0.7 s for a 1600-pixel preview on a busy Apple-silicon
Mac; drafts while dragging are coarser.

## Limits

- The estimate is relative depth from one picture: thin structures (hair, railings, leaves against the
  sky) and glass can come out wrong. Paint them with Refine.
- Depth read from Portrait HEIC / JPEG was checked on files Apple's own ImageIO wrote, not yet on photos
  taken by an iPhone; DNG depth maps follow the specification alone (no camera-written sample).
- No "Use photo's depth / estimate" choice in the panel yet: a photo with depth of its own uses it
  until `lensBlur.estimate` is run.
- In the browser the model runs on the page's thread: the page pauses while it estimates.
- Lightroom's own Lens Blur XMP fields are not read or written (`docs/xmp-interop.md`).
