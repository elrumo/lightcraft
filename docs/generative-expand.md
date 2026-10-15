# Generative Expand

**Crop ▸ Expand** lets the crop frame reach past the photo's edges, and **Generate** paints the new
area on this device, as Lightroom's Generative Expand does in the cloud. Nothing leaves the
computer, phone or browser: the painting is done by a model you download once (a small fast one,
or a larger diffusion model that draws new detail), or by LightCraft's own texture synthesis with
no download at all, and the result is stored with the photo's settings, so renders and exports
never run a model.

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
   SwiftUI iPhone and iPad app, the Fill With button beside Generate), or leave it on **Auto**, the
   best model for this device; then press **Generate**. It runs in the background with a row in the
   activity stack; ✕ there (or Cancel in the panel, or a tap on the phone's Generate button) stops
   it. The result lands as one undo step, "Generative Expand" (in the SwiftUI app the engine host
   stores it, `apphost` polling the job, and says so).
4. Every edit after that applies to the new area as to the photo: white balance, tone, colour,
   masks, Remove spots, grain. Moving the crop past what was painted shows the checkerboard again;
   Generate paints the whole crop afresh.

Turning Expand off brings the crop back inside the photo and drops the painted area (undo brings
both back). Reset Crop does the same.

## What paints the new area

Each device offers the models it can run, decided by its profile (computer, phone or browser; its
memory: the RAM of a computer, what iOS lets the app use on a phone; whether it has a GPU for the
models, Metal on a Mac, iPhone or iPad). **Settings → AI Models** chooses the model at the top
(Automatic, or any model this device runs), the same choice as the crop tool's Fill With, and lists
every model with its size, licence and folder.

| | Download | Licence | Offered on | Good at |
|---|---|---|---|---|
| **MI-GAN** | ~30 MB (the AI Remove model) | MIT | every device, the browser too | skies, water, foliage, walls; fast |
| **LaMa** | 208 MB | Apache-2.0 | computers; phones that may use 2 GB | long structures across a wide strip (horizons, shorelines, rows of trees) |
| **Stable Diffusion 1.5** | 2.1 GB | CreativeML OpenRAIL-M ¹ | computers with 8 GB; phones with a GPU that may use 4 GB (recent iPhones and iPads) | draws new detail at 512 pixels: clouds, waves, grass, more of the scene |
| **Kandinsky 2.2** | 9.0 GB | Apache-2.0 | computers with 16 GB | draws new detail at 768 pixels, guided by the photo itself (its image embedding) |
| **SDXL** | 6.9 GB | CreativeML Open RAIL++-M ¹ | computers with a GPU and 16 GB, or 32 GB | the most detail, at 1024 pixels |
| **Classic** | none | (LightCraft's own) | everywhere | plain areas: the photo's edge colours carried outward with its texture; it can't invent shapes |

¹ Not an open-source licence: use, also commercial, is allowed, but the licence lists uses that
are not (its Attachment A), which whoever downloads the model accepts. The app says so before the
download. Kandinsky 2.2 is the fully open (Apache-2.0) diffusion model.

**Automatic** picks, by the profile: on a phone, Stable Diffusion 1.5 when it is offered, else
LaMa, else MI-GAN; on a computer with a GPU, SDXL with 32 GB or more, else Stable Diffusion 1.5; on
a computer without one, Stable Diffusion 1.5 with 12 or more threads and 16 GB, else LaMa; in the
browser, MI-GAN. A model it picks that isn't downloaded yet is offered (Generate then waits for
it); `crop.generate` without a model uses the chosen one when it is here, else the most capable one
that is, else Classic.

How long one Generate takes (the whole command, loading the model included), measured on a 4-core
x86-64 Linux VM without a GPU, on a 2560 × 1707 demo photo cropped to 1 × 1:

| | CPU |
|---|---|
| MI-GAN | 6 s |
| LaMa | 20 s |
| Stable Diffusion 1.5 (512, 15 steps) | 8 min |
| Kandinsky 2.2 (768, 15 steps) | 17 min |
| SDXL (1024) | not run: its 32-bit UNet alone is 10 GB, too much for this 16 GB VM (it is offered on a CPU from 32 GB) |
| Classic | 4 s |

On an Apple-silicon Mac LaMa's pass took 7–11 s on the CPU. The diffusion models' GPU (Metal)
path is not timed yet; on a CPU they take minutes, and the app says so before their download.

LaMa could run on a phone through Core ML on the GPU (spike 2026-10-15, [ai-models.md](ai-models.md) → Core ML spike): about
0.15 s on a Mac GPU in a 99 MB package; not built, not measured on an iPhone.

Nothing downloads until you press **Download**, which names the size, the licence and the authors
first. Settings → AI Models lists the models, where they are stored, and lets you turn them off or
delete them. In the browser MI-GAN is kept in the site's storage (`models/migan/migan.onnx`, the
same copy AI Remove uses); the larger models aren't offered there.

Agents and scripts: `crop.expand {on?}`, `crop.generate {model?: "migan" | "lama" |
"sd15-inpaint" | "kandinsky22-inpaint" | "sdxl-inpaint" | "classic", wait?}` (by default it runs to
the end and answers `{model, passes, width, height, storedBytes}`; with `wait: false` it runs in the
background, `activity.cancel` stops it), `crop.expandModel {model: "auto" | id}` (the choice
Settings and the crop tool make), and `crop.expandModels` (each model with `available`, `ready`,
`restricted`, `recommended`, `chosen` and `default`; whether the choice is `automatic`; the
`device` profile). The models download with `models.download {"id": …, "acknowledged": true}`,
only after the user agreed. `crop.drag`, `crop.set`, `crop.straighten` and `crop.aspect` all honour
Expand.

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
  display-encoded as AI Remove does (its bright end near white) and given to the network whole:
  what goes on past one edge agrees with the others (a horizon continues at the same height on both
  sides). MI-GAN and LaMa paint one 512 × 512 square: a canvas longer than 5 × 4 is squeezed at most
  1.25× and the rest letterboxed, so shapes keep their proportions, and a 12-pixel band around it
  takes the network's darkened border. A diffusion model paints a canvas in the picture's own
  proportions (up to 2 : 1, each side a multiple of 64) of about its native size: 512 × 512 pixels'
  worth for Stable Diffusion 1.5, 768 for Kandinsky, 1024 for SDXL.
- **Detail.** MI-GAN's and LaMa's answer has 512 pixels of detail over the canvas. The photo's grain and
  fine texture are added on top from LightCraft's content-aware fill (the Remove tool's patch
  synthesis): `blur(answer) + synthesis − blur(synthesis)`, as AI Remove does for its spots.
  Painting the canvas again in 512-pixel windows at full scale was tried and dropped: windows far
  from the photo saw too little of it and the networks painted streaks there.
- **The diffusion models** (`crates/diffuse`, on candle: Metal on macOS and iOS in 16-bit, the CPU
  elsewhere in 32-bit). An inpainting UNet reads the noisy latents, the hole at the latents' size
  and the latents of the canvas with the hole made grey; Generate starts from noise with a fixed
  seed (the same canvas gives the same answer) and takes 20 DPM-Solver++ steps on a GPU, 15 on a
  CPU, with classifier-free guidance 4. No prompt is asked of you, as in Lightroom: Stable
  Diffusion and SDXL are given an empty text and a fixed negative text ("frame, border, text,
  watermark, blurry, …", the usual failures of an outpainted edge), encoded once per Generate;
  Kandinsky, which reads an image embedding instead of text, is given the photo's own (OpenCLIP
  ViT-bigG/14, from Kandinsky's prior) and an empty picture's as the negative, and after every
  step the photo's latents (noised to the step) replace the network's over the photo, as
  Kandinsky's pipeline does. A diffusion model repaints the photo too and its colours drift from
  the photo's: the difference over the photo (blurred, so only colour and light count) is carried
  into the new area by membrane interpolation, so the new area meets the photo at its edge without
  a line. Its answer has detail of its own, so no texture is synthesized on it. The weights are
  read a tensor at a time and the text and image encoders a layer at a time, so loading never
  holds a file twice; a model is loaded for each Generate and dropped after it (gigabytes).
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

- These are local models, not Lightroom's cloud model: one result, no variations, no prompt.
  MI-GAN and LaMa continue what is there (sky, sea, sand, grass, walls, soft backgrounds); they
  don't invent objects, and a person or a building cut by the frame is continued as a blur at best.
  The diffusion models draw new detail, clouds, waves, more of the scene, but can draw the wrong
  thing (a different season, an object that doesn't belong): Generate again after moving the crop,
  or choose another model.
- The diffusion models are slow without a GPU: minutes per Generate on a CPU (Windows and Linux
  run them on the CPU; candle's GPU backends there need C++ toolchains LightCraft doesn't use). On
  a Mac, iPhone or iPad they run on the GPU.
- MI-GAN and LaMa give the new area at most 1024 pixels of detail over the whole canvas, Stable
  Diffusion 1.5 512, Kandinsky 768, SDXL 1024; on a 24 MP export it is softer than the photo. The
  further past the photo, the less the model knows: up to a quarter of the photo's size per side
  looks best.
- An expanded crop exports at the photo's own resolution over the whole canvas: at the limit (the
  photo's size past every edge) that is nine times the photo's pixels, most of them soft.
- The settings of an expanded photo grow by the stored picture (90–200 KB for the measurements
  above; at most 1024 × 1024 pixels' worth).
- The fill is not written to Lightroom's XMP fields; LightCraft keeps it in its own settings (and
  its XMP block). A Lightroom catalog's generative expansions are not imported.
- Not yet run on a phone or a Mac GPU here: the GPU path is type-checked for iOS and macOS and its
  maths checked on the CPU (see below).

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

**Stable Diffusion 1.5 Inpainting** (Robin Rombach and Patrick Esser; Runway, CompVis LMU Munich,
Stability AI; Rombach et al., "High-Resolution Image Synthesis with Latent Diffusion Models", CVPR
2022): the 16-bit diffusers files of `stable-diffusion-v1-5/stable-diffusion-inpainting` at a fixed
revision, under the CreativeML OpenRAIL-M licence
(<https://huggingface.co/spaces/CompVis/stable-diffusion-license>). **SDXL 1.0 Inpainting 0.1** (the
Hugging Face Diffusers team, from Stability AI's Stable Diffusion XL 1.0; Podell et al., "SDXL",
2023): `diffusers/stable-diffusion-xl-1.0-inpainting-0.1`, under the CreativeML Open RAIL++-M
licence (<https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/blob/main/LICENSE.md>).
**Kandinsky 2.2** (Arseniy Shakhmatov, Anton Razzhigaev, Aleksandr Nikolich, Vladimir Arkhipkin,
Igor Pavlov, Andrey Kuznetsov and Denis Dimitrov; Sber AI, AIRI): the decoder-inpaint UNet and MoVQ
from `kandinsky-community/kandinsky-2-2-decoder-inpaint` and the image encoder from
`kandinsky-community/kandinsky-2-2-prior`, Apache-2.0. Each file is pinned by size and SHA-256
(`crates/diffuse/src/models.rs`); the training images (LAION subsets) have terms of their own, not
checked here.

MI-GAN and LaMa run with LightCraft's own code on [rten](https://github.com/robertknight/rten), a
pure-Rust ONNX runtime (`crates/erase`: `Painter`, `expand::outpaint`, `lama`); the diffusion models
on [candle](https://github.com/huggingface/candle) (`crates/diffuse`): Stable Diffusion's UNet is put
together from candle-transformers' blocks with SDXL's added conditioning, and Kandinsky's UNet,
MoVQ and image encoder are re-written following diffusers (Apache-2.0; see NOTICE). No code from
other outpainting or inpainting apps is used.

## How it was checked

Against diffusers 0.31 / transformers 4.46 on the CPU in 32-bit, with the downloaded files
(2026-10-14; the reference outputs are made by scripts kept outside the repository and the tests
run when `$LIGHTCRAFT_SD15`, `$LIGHTCRAFT_KANDINSKY`, `$LIGHTCRAFT_SDXL` and their `_REF` files are
given):

- Stable Diffusion 1.5: the tokenizer's ids are identical; the text encoder, one UNet step and the
  VAE agree within 2 × 10⁻³ (`sd::tests::matches_diffusers`).
- SDXL: both tokenizers' ids are identical; the two text encoders agree within 5 × 10⁻⁴, the
  pooled text embedding within 2 × 10⁻⁵ and one UNet step, with the added text and time
  conditioning, within 3 × 10⁻⁴ (`sd::tests::sdxl_matches_diffusers`). Its VAE is Stable Diffusion
  1.5's network with other weights, checked there.
- Kandinsky 2.2: the image encoder within 2 × 10⁻⁵, one UNet step within 5 × 10⁻⁶, MoVQ within
  5 × 10⁻⁵ (`kandinsky::tests::matches_diffusers`). (This found that candle 0.9.2's `Linear` answers
  wrongly for a transposed three-dimensional input; the code makes such inputs contiguous.)
- DPM-Solver++ follows diffusers' trajectories to 10⁻⁶ (`schedule::tests`).
