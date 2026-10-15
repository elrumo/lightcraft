# Generative Expand

**Crop ▸ Expand** lets the crop frame reach past the photo's edges, and **Generate** paints the new
area on this device, as Lightroom's Generative Expand does in the cloud. Nothing leaves the
computer, phone or browser: the painting is done by a small open model you download once (or by
LightCraft's own texture synthesis, with no download at all), and the result is stored with the
photo's settings, so renders and exports never run a model.

## Using it

1. Open the crop tool (`R`) and turn on **Expand** (desktop: the Generative Expand section of the
   Crop panel; phone: the first tile of the Aspect tab, a switch as in Lightroom's mobile app).
   The view zooms out to leave room around the photo.
2. Drag a corner or an edge past the photo, straighten it (the corners it uncovers are filled
   too), or pick an aspect ratio: with Expand on, a ratio adds canvas around the *whole* photo (a
   3 × 2 landscape asked for 1 × 1 gains a strip above and below; Instagram's 4 × 5 or TikTok's
   9 × 16 work the same way). The crop can reach up to the photo's own size past each edge. Area
   with nothing in it yet shows a checkerboard.
3. Choose what paints the new area (**Fill With**; on a phone, tap the line in the bottom bar; in the
   SwiftUI iPhone and iPad app, the Fill With button beside Generate), then press **Generate**. It
   runs in the background with a row in the activity stack; ✕ there (or Cancel in the panel, or a
   tap on the phone's Generate button) stops it. The result lands as one undo step, "Generative
   Expand" (in the SwiftUI app the engine host stores it, `apphost` polling the job, and says so).
4. Every edit after that applies to the new area as to the photo: white balance, tone, colour,
   masks, Remove spots, grain. Moving the crop past what was painted shows the checkerboard again;
   Generate paints the whole crop afresh.

Turning Expand off brings the crop back inside the photo and drops the painted area (undo brings
both back). Reset Crop does the same.

## What paints the new area

The choice follows the device: each offers what it can run, and what is chosen is remembered.

| | Download | Licence | Devices | One Generate (measured) | Good at |
|---|---|---|---|---|---|
| **MI-GAN** | ~30 MB (the AI Remove model) | MIT | computers, iPhone / iPad, the browser | 6 s | skies, water, foliage, walls: the default |
| **LaMa** | 208 MB | Apache-2.0 | computers only | 20 s | long structures across a wide strip (horizons, shorelines, rows of trees) |
| **Classic** | none | (LightCraft's own) | everywhere | 4 s | plain areas: the photo's edge colours carried outward with its texture; it can't invent shapes |

LaMa could run on a phone through Core ML on the GPU (spike 2026-10-15, [ai-models.md](ai-models.md) → Core ML spike): about
0.15 s on a Mac GPU in a 99 MB package; not built, not measured on an iPhone.

(Measured 2026-10-13 on a 4-core x86-64 Linux VM, CPU only, a 2560 × 1707 demo photo cropped to
1 × 1: the whole command, with loading the model and the texture synthesis, which is most of
MI-GAN's time. Phones and the browser are not measured yet.)

Nothing downloads until you press **Download**, which names the size, the licence and the authors
first. Settings → AI Models lists both models, where they are stored, and lets you turn them off or
delete them. In the browser MI-GAN is kept in the site's storage (`models/migan/migan.onnx`, the
same copy AI Remove uses); LaMa isn't offered there, nor on a phone, where it is too large and slow.

Agents and scripts: `crop.expand {on?}`, `crop.generate {model?: "migan" | "lama" | "classic",
wait?}` (by default it runs to the end and answers `{model, passes, width, height, storedBytes}`;
with `wait: false` it runs in the background, `activity.cancel` stops it), and `crop.expandModels`
(what this device offers, what is ready, what Generate picks without a `model`: the best model on
the device, else Classic). The models download with `models.download {"id": "migan" | "lama",
"acknowledged": true}`, only after the user agreed. `crop.drag`, `crop.set`, `crop.straighten` and
`crop.aspect` all honour Expand.

## Apple Image Playground (SwiftUI iPhone / iPad app)

Where the device has Apple Intelligence on (iOS or macOS 27), **Fill With** also offers **Apple Image Playground**. It is
the one exception to "nothing leaves the device": Apple's sheet makes the new area in Apple's cloud (Private Cloud
Compute). So it is never picked by itself (Auto never chooses it), and every Generate asks "Send this photo to
Apple?" first, naming what goes: the part of the photo inside the crop, as the canvas picture below.

How it works: the engine gives the app the canvas as a picture (`crop.expandCanvas`: the crop's box with a little
context at up to 1024 px a side, the photo where it is, the Classic fill as a placeholder over the new area, and a
`key` naming the crop). The app opens Apple's `imagePlaygroundSheet` on it (`creationStrategy: .editExisting`,
personalization off, the canvas's size, a prompt asking to continue the scene and keep the photo as it is); the user
works in the sheet and taps Done; the picture's file goes back (`crop.applyExpandPicture {key, path | png, model:
"appleCloud"}`). Apple's API takes no mask, so the sheet may re-render the whole picture: the engine uses only what it
needs of it:
- the picture is resized to the canvas (refused when its proportions are more than 8 % off the canvas's);
- its tones are matched to the photo's (a mean and a spread per channel over the strip of photo along the new
  area), and it is faded in from the photo's own edge colours over 4 % of the canvas, so the join does not show;
- the photo's pixels never change, and the photo's grain is added over the new area, as for the other painters;
- the result is stored like any other fill (`ExpandFill.model` is `appleCloud`), one undo step, so renders and exports
  never contact Apple. Turning Expand off, or moving the crop while the sheet is open, refuses the answer.

Limits: the sheet needs the user's taps and a device with Apple Intelligence (not the simulator; not scriptable;
`LIGHTCRAFT_FAKE_PLAYGROUND=1` stands in with a tinted copy of the canvas, for tours and tests, and
`ui.set {playgroundExpand: true}` runs it); Apple manages its usage limits; the web and desktop apps do not have it.
**Not yet seen on a device**: how well the sheet's result continues a photo is unknown until it is tried.

## How it works

- **What the model sees.** The crop's bounding box (and a little more past the photo where the crop
  already is) is resampled from the photo at up to 1024 pixels a side (512 in the browser), at the
  stage the fill goes back in: the resampled photo's light, before white balance. It is
  display-encoded as AI Remove does (its bright end near white) and given to the network whole, in
  one 512 × 512 pass: what goes on past one edge agrees with the others (a horizon continues at the
  same height on both sides). A canvas longer than 5 × 4 is squeezed at most 1.25× and the rest
  letterboxed, so shapes keep their proportions; a 12-pixel band around it takes the network's
  darkened border.
- **Detail.** The network's answer has 512 pixels of detail over the canvas. The photo's grain and
  fine texture are added on top from LightCraft's content-aware fill (the Remove tool's patch
  synthesis): `blur(answer) + synthesis − blur(synthesis)`, as AI Remove does for its spots.
  Painting the canvas again in 512-pixel windows at full scale was tried and dropped: windows far
  from the photo saw too little of it and the networks painted streaks there.
- **What is stored.** The answer over the new area plus three pixels of the photo inside its edge
  (black elsewhere, which compresses to almost nothing), at most 1024 pixels a side, 8-bit,
  deflated, in the photo's settings (`expand_fill`, in normalized coordinates of the transformed
  photo, past 0..1 on the sides it extends). Copy Settings, presets and sync never carry it: it is a
  picture of this photo.
- **Where it goes back.** At the resample (`lightcraft_pipeline::expand`): output pixels whose
  source is past the photo take the stored fill, fading in over one fill pixel at the edge. The
  GPU renderer resamples such frames on the CPU. Area past the photo with no fill renders as an
  empty canvas's light neutral and is transparent in the preview (the checkerboard).
- **Upright, lens corrections, Transform** move the photo under a fill made for the old geometry:
  Generate again after changing them.

## Limits (be aware)

- These are small local models, not Lightroom's cloud diffusion model: one result, no variations,
  no prompt. They continue what is there (sky, sea, sand, grass, walls, soft backgrounds); they don't
  invent objects, and a person or a building cut by the frame is continued as a blur at best.
- The new area has at most 1024 pixels of detail over the whole canvas; on a 24 MP export it is
  softer than the photo (the synthesized grain hides some of it). The further past the photo, the
  less the model knows: up to a quarter of the photo's size per side looks best.
- An expanded crop exports at the photo's own resolution over the whole canvas: at the limit (the
  photo's size past every edge) that is nine times the photo's pixels, most of them soft.
- The settings of an expanded photo grow by the stored picture (90–200 KB for the measurements
  above; at most 1024 × 1024 pixels' worth).
- The fill is not written to Lightroom's XMP fields; LightCraft keeps it in its own settings (and
  its XMP block). A Lightroom catalog's generative expansions are not imported.

## Licence and credit

**MI-GAN** (Andranik Sargsyan, Shant Navasardyan, Xingqian Xu, Humphrey Shi, Picsart AI Research;
code and weights MIT): see [ai-remove.md](ai-remove.md). **LaMa** (Roman Suvorov, Elizaveta
Logacheva, Anton Mashikhin, Anastasia Remizova, Arsenii Ashukha, Aleksei Silvestrov, Naejin Kong,
Harshith Goka, Kiwoong Park and Victor Lempitsky, Samsung AI Center: "Resolution-robust Large Mask
Inpainting with Fourier Convolutions", WACV 2022; code and weights Apache-2.0,
<https://github.com/advimman/lama>). The file is `lama_fp32.onnx`, the fixed-size fp32 export of
big-lama published by Carve (<https://huggingface.co/Carve/LaMa-ONNX>, Apache-2.0), downloaded at a
fixed revision and checked against its pinned size and SHA-256 (`crates/erase/src/lama.rs`). Its
answers on rten match onnxruntime's to 2 × 10⁻⁶ (checked 2026-10-13). The training images (Places2)
have terms of their own, not checked here.

Both run with LightCraft's own code on [rten](https://github.com/robertknight/rten), a pure-Rust
ONNX runtime (`crates/erase`: `Painter`, `expand::outpaint`, `lama`). No code from other outpainting
or inpainting apps is used.
