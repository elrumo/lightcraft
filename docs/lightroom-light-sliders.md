# Light sliders fitted to Lightroom's single-slider exports

> **Last reviewed:** 2026-11-06 · **Last updated:** 2026-11-06 · **Change:** new (Contrast, Highlights, Shadows, Whites and Blacks refitted)

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

[`ToneMap::display`]: ../crates/pipeline/src/tone.rs
[`ToneMap::hdr`]: ../crates/pipeline/src/tone.rs
