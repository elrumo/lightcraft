# Super Resolution (AI)

**Photo ▸ Enhance ▸ Super Resolution…** enlarges the selected photo to twice its width and height (four times the
pixels) with a small neural network, **Nomos Uni SPAN 2×**, and adds the result next to the original.

**The model is optional.** It isn't part of LightCraft, nothing requires it, and everything else works without it.

## Using it

1. Select a photo and choose Photo ▸ Enhance ▸ Super Resolution…
2. The first time, the dialog offers the model: a one-time download of about 4.5 MB, its licence (CC-BY-4.0) and
   its author. Nothing downloads until you press **Download**; the progress shows in the dialog, **Cancel Download**
   stops it (it resumes next time), and you can close the dialog while it runs.
3. With the model in place the dialog shows the photo's size before and after. **Enlarge** runs in the background
   (progress in the toast, **Photo ▸ Enhance ▸ Cancel Super Resolution** stops it); the window stays usable.
4. The result is added to the library: `<name>-SR.tif` in the original's folder (never replacing a file: `-SR-2.tif`
   and so on), stacked on the original, and selected. The original is not touched.

What you get: the photo **rendered with its edits and crop**, in sRGB, enlarged, as a 16-bit TIFF with your metadata.
It is added with neutral settings (its pixels carry the edits already), so a preset or Auto doesn't apply twice.

## Limits (be aware)

- It enlarges the *rendered* picture, not the raw data. You can edit the TIFF further, but not as a raw file (white
  balance and highlight recovery were applied before the enlargement). Lightroom's Enhance writes a linear DNG; this
  doesn't yet.
- The model enlarges by exactly 2×, in sRGB. Wide-gamut colours are brought into sRGB first.
- The enlarged photo may have at most **100 megapixels** (a 25 MP photo): crop first, or raise it with
  `enhance.superRes {maxMegapixels}`.
- Speed: on an Apple-silicon Mac (GPU through Metal) about 0.15 megapixels of input per second: a 4 MP photo takes
  about half a minute, and a 17 MP photo took about 2 minutes (measured while the computer was busy with other work);
  on the CPU (Windows, Linux for now) it is slower.
- Memory (measured with a 17 MP photo, a 69 MP result): about 1.7 GB while it works, and up to about 2.9 GB for a
  moment when the large result is added to the library. Plan for roughly 40 bytes per pixel of the *enlarged* photo.
- The result is a large file: that 69 MP TIFF is 325 MB.
- It sharpens and adds plausible detail; it can't recover detail that was never captured, and very smooth photos
  gain little. Judge a result at 100 %.
- Tones are kept: the enlargement's broad tones are anchored to the original's (the network alone drifts about 1.5 %
  darker).

## Managing the model

Settings → AI Models shows the model, where it is stored and how much space it takes, and lets you turn it off, or
delete it to free the space (you can download it again). See [ai-models.md](ai-models.md).

## On iPhone and iPad

Super Resolution is in the photo's "…" menu (and in All Commands); the dialog is the same, as a full-screen page.

- The model is stored in the app's `Library/Caches`, which iOS never backs up and may empty when storage runs low
  (the app then offers the download again). It is about 4.5 MB.
- The work is sized to half of the memory iOS gives the app, from the figures above, so the photo has to be smaller
  than on a computer: roughly 9 MP on a recent iPhone (a typical 12 MP photo needs cropping first), less on older
  ones; the dialog says when a photo is too large. The enlarged photo is kept as 16-bit samples and the network
  works on small tiles to stay within that.
- Keep the app open while it works: iOS stops GPU work for apps in the background, so a job interrupted that way
  ends with an error and can be started again.
- **Status:** the iOS build compiles and type-checks (`cargo xtask ios`), and the engine and dialog are tested on
  the computer. It has **not been run on a device yet**, so the speed and memory figures above are estimates.

## Licence and credit

The weights are **Nomos Uni SPAN** by Philip Hofmann ("Phips"), licensed **CC-BY-4.0**
(<https://creativecommons.org/licenses/by/4.0/>): <https://huggingface.co/Phips/2xNomosUni_span_multijpg>. The
network is **SPAN** (Wan et al., CVPR Workshops 2024, Apache-2.0). LightCraft credits the author in the dialog;
please keep the credit if you share enlarged photos made with it where attribution is expected.

## Getting the model

### In the app

The dialog downloads `2xNomosUni_span_multijpg.safetensors` straight from the author's Hugging Face repository
(public; no account). LightCraft hosts nothing. The file is checked against its pinned size and SHA-256 before use, a
file that doesn't match is deleted, a partial download resumes, and no download ever blocks the app.

Agents and scripts: `enhance.model.status`, `enhance.model.download {"acknowledged": true}` (pass it only after the
user agreed to the download and the licence), `enhance.model.cancel`, then `enhance.superRes {id?, maxMegapixels?}`.

### Other locations, by hand

The model folder is `models/nomos-span-2x/` in LightCraft's settings folder (`~/Library/Application Support/LightCraft/`
on macOS, `%APPDATA%\LightCraft\` on Windows, `~/.config/lightcraft/` on Linux). Put the `.safetensors` file there
yourself to skip the download. To download from your own mirror instead (a base URL where
`2xNomosUni_span_multijpg.safetensors` lives): set `LIGHTCRAFT_NOMOS_SPAN_MIRRORS`, or list the URLs, one per line,
in `models/nomos-span-2x-mirrors.txt`; they are tried before the built-in location.

## For developers

- `crates/enhance` runs the network on candle (Metal on macOS, CPU elsewhere): `Span::load`, `Span::upscale`
  (tiled with a halo wider than the receptive field, so tiles equal a whole-image pass). `cargo run --release -p
  lightcraft-enhance --example upscale -- <model> <image> <out.png> [--check] [--crop N]` tries a model; `--check`
  shrinks the picture, enlarges it, and reports PSNR against bicubic and how well the result shrinks back.
- `crates/models` describes the model (`registry::NOMOS_SPAN_2X`: pinned size and SHA-256, licence, credit) and does
  the download.
- `crates/engine/src/enhance` is the job: `Session::super_res_job` (plan), `SuperResJob::run` (slow, any thread,
  progress and cancel), `Session::finish_super_res` (import, neutral settings, stack).
- Porting notes (SPAN's reference code is easy to misread): the file's stored `eval_conv` tensors are the random
  *initial* values, never trained, so the 3×3 weights are fused from the training branches (`sk`, `conv.0..2`); the
  reference applies SiLU in place, so the tensor the last block returns for the final concatenation is the *activated*
  one; input normalisation is `(x − mean) × 255` with the DIV2K mean and no inverse on the output.
