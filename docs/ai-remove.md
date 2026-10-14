# AI Remove

**Remove ▸ Generative AI** fills a Remove stroke with what the photo most likely shows behind it, painted by a small neural
network, **MI-GAN**. Without it, Remove copies texture from around the stroke (patch synthesis), which works well on
grass, sand and water but can't continue an edge or a shape it can't find nearby.

**The model is optional.** It isn't part of LightCraft, nothing requires it, and everything else works without it.

## Using it

1. Open Remove (`H`), keep the **Remove** tool and turn on **Generative AI** (desktop: the Remove panel; phone: the switch
   under Size). Turn it off to go back to filling from around the stroke; the switch stays as you leave it.
2. The first time, the switch offers the model: a one-time download of about 30 MB, its licence (MIT) and its authors.
   Nothing downloads until you press **Download**.
3. Paint over what should go. "Removing with AI…" shows while it works (about half a second on a computer, a few
   seconds in a browser), then the spot is added as one undo step.
4. Drag the spot's pin to move it and it is painted again at its new place. Copied spots (Copy / Paste settings,
   presets) leave their fill behind, so on another photo they fill from around them.

Agents and scripts: `spot.add {points, size?, feather?, opacity?, "ai": true}`, `spot.erase {index?}` (paint an
existing Remove spot again). The model: `models.download {"id": "migan", "acknowledged": true}` (only after the user
agreed), `models.list`, `models.cancel`, `models.delete`. Without the model both commands fail with "The AI Remove model
(MI-GAN) is not downloaded…" and add nothing.

## How it works

- The model sees 512 × 512 pixels. LightCraft gives it the stroke's area and about four times as much around it, at
  the retouching stage (white balanced, earlier spots applied), and keeps what it paints **with the spot** (an 8-bit
  picture of at most 512 pixels a side over the spot, compressed: 30–70 KB for strokes the size of a person in a 2 MP
  photo, up to about 400 KB for the largest, in the photo's settings). Renders
  and exports never run the model, and look the same at every size.
- When the spot renders, the painted picture gives the broad structure, and the fine grain and texture come from the
  patch synthesis that classic Remove uses, so the fill doesn't look smoother than the photo around it.
- The fill is matched to the photo's tones from the stroke's surroundings, as Heal does: a later white-balance or
  exposure change carries over.

## Limits (be aware)

- It is a small local model, not Lightroom's cloud generative fill: one result, no variations, no prompt.
- A large area on a large photo has at most 512 × 512 pixels of painted detail; the patch synthesis adds texture, not
  shapes.
- In busy scenes (a crowd, a market) it can paint a person-like shape into the hole. Paint a little wider, or use
  classic Remove, Heal or Clone there.
- Changing Upright, lens corrections or Transform after painting moves the photo under a fill made for the old geometry;
  paint the spot again (`spot.erase`, or move its pin) after such changes.
- In the browser the model runs on the page's thread, so the page pauses for a few seconds per stroke (the notice is
  shown first). Running it in a Web Worker is the next step.
- The settings of a photo with many AI spots grow by the size of their fills.
- The fill is not written to Lightroom's XMP fields; LightCraft keeps it in its own settings (and its XMP block).

## Managing the model

Settings → AI Models shows it (where it is stored, its size), and lets you turn it off or delete it. The same file
paints the crop tool's Generative Expand ([generative-expand.md](generative-expand.md)): one download serves both. In the browser
it is kept in the site's storage (`models/migan/migan.onnx`): Back Up Library leaves it out, and `?reset` removes it.
On iPhone it is stored in the app's caches, which iOS may empty when storage runs low (the switch then offers it again).

## Licence and credit

**MI-GAN** (Andranik Sargsyan, Shant Navasardyan, Xingqian Xu, Humphrey Shi: "MI-GAN: A Simple Baseline for Image
Inpainting on Mobile Devices", ICCV 2023), Picsart AI Research. Code and weights are **MIT**:
<https://github.com/Picsart-AI-Research/MI-GAN> (`LICENSE`, `LICENSE-WEIGHTS`). The file is `migan.onnx` from the
authors' Hugging Face repository (<https://huggingface.co/andraniksargsyan/migan>, MIT), downloaded at a fixed revision
and checked against its pinned size and SHA-256 (`crates/erase/src/lib.rs`). The training images (Places2) have terms of
their own, not checked here.

LightCraft runs it with its own code on [rten](https://github.com/robertknight/rten), a pure-Rust ONNX runtime; the
ONNX file's `uint8` casts are made `Identity` in memory (`lightcraft_vision::onnxfix`), never on disk. No code from
other inpainting apps (for example inpaint-web, which is GPL-3.0) is used.
