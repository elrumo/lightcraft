# Light sliders fitted to Lightroom's single-slider exports

> **Last reviewed:** 2026-11-06 · **Last updated:** 2026-11-06 · **Change:** colour by tone of fitted looks; Camera Matching keeps the camera's own

How Contrast, Highlights, Shadows, Whites and Blacks move a photo was tuned by eye. Here they are fitted to Lightroom's own
response on one raw: a Sony ILCE-6000 with the E PZ 16-50mm at 16 mm (sunset over the sea, grass, white camper vans; the
same scene as [lightroom-preset-fit.md](lightroom-preset-fit.md)). Lightroom 7.5 exported it unedited and with each slider
alone at −100, −54, +54 and +100 (Camera Standard, lens profile on, full-size sRGB JPEG). The exports are private, in the
LightBites app's `test-raws/light/`. Only rendered output was read: no Adobe file.

## Method

For each export, the display luminance of every pixel before and after the slider gives Lightroom's response as a function
of tone: the median per band of `p = Y^(1/2.2)` (bands 0.04 wide). That needs no alignment and no colour model, since it
compares Lightroom with itself.

- **Contrast, Whites, Blacks** act on display luminance after the base tone ([`ToneMap::display`], also the adjustment on
  top of a camera curve). Each is now a table of the shift of `p`: for an amount `a` = |slider| / 100 it is
  `A(p)·a + B(p)·a²` (21 knots, a Catmull-Rom spline), for each sign. The quadratic is exact at 54 and 100. `A` is held
  to the direction of the full slider, so a small amount never moves the wrong way. That clamp matters for Whites + and
  Blacks −, which Lightroom strengthens much faster than linearly (+100 moves three to four times as much as +54). A
  running maximum keeps the summed curve monotone.
- **Highlights, Shadows** act on the scene before the tone map, through the edge-aware base
  (`finish.rs`, local tone). Their windows and gains were fitted by emulation: each band's display tone goes back to scene
  EV through the photo's camera curve, takes the slider's shift and comes forward again. Shadows now weighs EV −2.15 to
  +1.05 over grey with gain 1.07 (was −4.8 to +0.3, 1.7). Highlights weighs −3.3 to +4.5 EV with gain 0.55 up and 0.6 down
  (was −1 to +2.8, 2.5 and 1.7). The free fit put the top of the Highlights window at 8.6 EV. This photo's brightest sky
  is only +1.6 EV, so capping it at 4.5 costs nothing here and keeps more recovery for blown areas. Above +1.6 EV, where the
  exports have no tones, a negative Highlights' gain grows back towards the old 1.7 by +3 EV (`finish::highlights_down`):
  at 0.6 alone a fully clipped light no longer came down below white at −100 (`clipped_highlights_stay_neutral_when_darkened`).
  The ramp reads the larger of the base and the pixel: on the base alone, bright pixels beside a dark edge got a halo; on
  the pixel alone, dark twigs in a bright sky lost their contrast.

Positive Highlights at 2.5 came from the Phoenix 200 preset fit, where Highlights +49 sat with Blacks −68, Shadows −30 and
curves, and other gaps could hide in it. On its own, Lightroom's Highlights moves bright tones less than half as much
as that, so the single-slider value replaces it.

## Result

The Apple render of the raw (`tools/apple_parity`, `LRFIT_*`), with each side's unedited render mapped onto the other's
so only the slider's response is compared: mean |ΔL\*| against Lightroom.

| Slider | −100 | −54 | +54 | +100 |
|---|---|---|---|---|
| Contrast | 2.89 → 1.64 | 2.08 → 1.58 | 2.40 → 1.95 | 3.60 → 2.35 |
| Highlights | 4.04 → 2.26 | 2.76 → 1.86 | 3.00 → 1.81 | 3.71 → 2.00 |
| Shadows | 2.98 → 2.06 | 2.13 → 1.77 | 2.46 → 2.01 | 3.83 → 2.49 |
| Whites | 3.25 → 1.91 | 2.80 → 1.79 | 4.02 → 2.03 | 11.36 → 2.94 |
| Blacks | 11.24 → 2.42 | 3.01 → 2.04 | 2.95 → 1.66 | 4.73 → 1.68 |

Over all 22 exports (with Exposure ±1, unchanged) the mean goes from 3.78 to 2.01. What is left is mostly the unedited
render's own spread after the mapping.

## Colour

Lightroom changes colourfulness with these sliders as if it applied the curve to each RGB channel: a channel spread grows
with the curve's slope in log-log terms. Ours applied the curve to luminance and kept the channels' ratios, so Blacks −100
left its mid tones at 0.66 of their chroma where Lightroom's reach 1.48, and Contrast went the other way from Lightroom.

The tone map now carries a chroma scale for every tone-table entry ([`ToneMap::chroma_at`], `edit_chroma`): the camera
look's at the unedited curve's output, times (edited slope / unedited slope)^0.85 over ±0.5 EV, within 0.5..2. Only
Contrast, Whites that lift and Blacks that crush take part (`colour_edit`): where Blacks lift or Whites lower the curve,
Lightroom's chroma moves by under 5 %. The constants come from a fit to the median chroma change per lightness band of
the Contrast, Whites and Blacks exports (error 0.090 with no slope term, 0.009 with it).

Median C\* after / before by base L\*, Lightroom / ours:

| Slider | L\* 30–45 | 45–60 | 60–75 |
|---|---|---|---|
| Contrast +100 | 1.12 / 0.76 → 1.20 | 1.47 / 0.86 → 1.35 | 1.49 / 0.99 → 1.40 |
| Contrast −100 | 0.88 / 1.21 → 0.81 | 0.64 / 1.11 → 0.69 | 0.60 / 1.00 → 0.68 |
| Blacks −100 | 0.74 / 0.46 → 0.74 | 1.48 / 0.66 → 1.35 | 1.41 / 0.91 → 1.35 |
| Whites +100 | 1.38 / 1.26 → 1.42 | 1.35 / 1.25 → 1.33 | 1.00 / 0.84 → 0.93 |

Exposure, Highlights and Shadows move a pixel along the curve rather than change it, so this leaves them alone. Exposure
−1.09 still gives the brightest band 0.95 of its chroma where Lightroom gives 1.36, and Shadows +100 the darkest band
1.41 where Lightroom gives 1.74.

## Colour by tone

Exposure, Highlights and Shadows move a pixel to another tone, so its colour there is the base look's. Against
Lightroom's unedited exports, ours was too grey below mid grey and too colourful in the brightest tones (the ILCE-6000:
0.63 to 0.80 of Lightroom's chroma from L\* 0 to 75, 1.43 above 90). Lightroom's camera-matching profiles were then
measured against each file's own camera JPEG, which our looks are fitted to, on four bodies (ILCE-6000 and ILCE-6700
Camera Standard, D70 Camera Standard, X-A5 Provia/Standard). On all four, Lightroom is more colourful than the camera
JPEG in the dark tones and less in the highlights, with the same median shape:

| Display luminance | 0.005 | 0.016 | 0.035 | 0.067 | 0.116 | 0.194 | 0.316 | 0.49 | 0.71 | 0.92 |
|---|---|---|---|---|---|---|---|---|---|---|
| Lightroom / camera JPEG | 1.10 | 1.15 | 1.13 | 1.10 | 1.06 | 1.03 | 0.99 | 0.94 | 0.93 | 0.94 |

So a raw whose look is fitted to its camera JPEG now starts from that look with these scales on its chroma curve
(`hue_sat::LIGHTROOM_CHROMA`: 1.12, 1.04, 0.99, 0.95, 0.93, 0.93, 0.93, 0.93 at display luminance 0, 1/7 … 1): the
default profile looks as Lightroom shows the photo. The fit itself is kept for the **Camera Matching** profile, which
looks as the camera does (`photo.cameraMatch`; until now such raws had none, and Camera Matching fell back to the
default). Neutral looks, the Canon CR3 tone fitted to Lightroom, and DNG profile tones are unchanged. Camera-look fit
version 21: smart previews rebuild.

Ours / Lightroom chroma per L\* band, Apple renderer, before → after:

| Body | 15–30 | 30–45 | 45–60 | 60–75 |
|---|---|---|---|---|
| ILCE-6000 | 0.73 → 0.81 | 0.80 → 0.87 | 0.69 → 0.70 | 0.78 → 0.76 |
| ILCE-6700 | 0.83 → 0.93 | 0.80 → 0.87 | 0.85 → 0.88 | 0.99 → 0.94 |
| D70 | 0.65 → 0.73 | 0.67 → 0.73 | 0.87 → 0.91 | 0.99 → 1.00 |
| X-A5 | 0.56 → 0.61 | 0.66 → 0.70 | 0.70 → 0.72 | 0.84 → 0.81 |

What is left is not Lightroom's: our fit falls short of the camera JPEG itself on some bodies (the X-A5's darks reach
0.5 to 0.6 of its JPEG's chroma, in the core's render too), and the Apple renderer keeps about 0.85 of the core's
chroma in the ILCE-6000's mid tones. Bodies on Adobe Color in Lightroom (no camera-matching profile: E-1, KP, GR here)
are Adobe's measured profile per model, which this does not try to match.

## Limits

- **One photo.** Lightroom adapts its sliders to the photo's range (gap 3 in [gaps.md](gaps.md)). These are fixed curves
  that match it on this scene. A photo with blown highlights, a dark interior or a backlit face may differ. How much
  Highlights −100 recovers above +1.6 EV is a guess that keeps blown areas recoverable, not a measurement.
- **Raws without a camera curve** keep their own base tone ([`ToneMap::hdr`]), whose Contrast, Whites and Blacks were not
  refitted.
- **Combined sliders** add their shifts. Only single sliders were measured.

The core (`crates/pipeline/src/tone.rs`, `finish.rs`, `crates/gpu/src/wgsl/finish.wgsl`) and the native frontend's Metal
port share the same constants; the port's goldens come from the core.

## Revision history

| Date | Change |
|---|---|
| 2026-11-06 | New: Contrast, Whites and Blacks as fitted shift tables, Highlights and Shadows windows and gains refitted. |
| 2026-11-06 | Colour: Contrast, lifting Whites and crushing Blacks scale chroma by their curve's slope, as Lightroom's do. |
| 2026-11-06 | Colour by tone: camera-JPEG fitted looks start with Lightroom's camera-profile chroma; Camera Matching keeps the fit. |

[`ToneMap::display`]: ../crates/pipeline/src/tone.rs
[`ToneMap::hdr`]: ../crates/pipeline/src/tone.rs
[`ToneMap::chroma_at`]: ../crates/pipeline/src/tone.rs
