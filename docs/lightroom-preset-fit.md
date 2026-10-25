# Matching a Lightroom preset: the Phoenix 200 "Orange You Glad" test

> **Last reviewed:** 2026-10-25 · **Last updated:** 2026-10-25 · **Change:** new (first preset-level measurement against Lightroom)

One preset, one raw, one Lightroom export. It measures where LightCraft's render of a whole look departs from
Lightroom's, and what was changed because of it. It is **one scene** (a Sony ILCE-6000, E PZ 16-50mm at 16 mm, sunset
over the sea, grass, white camper vans), so the constants below are fitted to it with a penalty for leaving the old
values; they are not a calibration of Lightroom's sliders in general (gap 3, `LR-BEHAV-RENDER-FIDELITY`, stays open).

Inputs (private, in the LightBites app's `test-raws/Harman Phoenix 200/`): the raw, the XMP preset
`1c. Orange You Glad Lab Scan Emulation.xmp` (white balance 6900 K / +3, Highlights +49, Shadows −30, Blacks −68,
Dehaze −16, Vibrance +33, a parametric and point curve with red / green / blue curves, HSL blue, colour grading shadows
45° / 87 and midtones 26° / 26, calibration, grain 54, one Point Color sample), and Lightroom's 2000 px JPEG export.

## Method

`lightcraft-cli render RAW -o OUT.jpg --settings PRESET.xmp --size 500` (the preset applied as the app applies it), then
the mean CIE ΔE76 between OUT and the export, both box-filtered to 100 px wide (grain and sharpening drop out), over the
frame inside 0.8 of the half diagonal. Single settings were removed one at a time to see which moves the picture
(ablations), then the constants below were fitted by coordinate descent with a penalty on the log of their change.

| Step | ΔE vs Lightroom |
|---|---:|
| As the app rendered it before | 16.4 |
| Point Color imported, A6000 distortion applied | 16.0 |
| A6000 on its own Kelvin scale (white balance, below) | ≈ 10.7 |
| Dehaze, Highlights and grading responses refitted (below), the build as shipped | 9.2 |
| Lightroom's Kelvin and tint read on this body's scale (below) | 8.0 |

What is left is mostly colour in the greens (Lightroom's are more saturated: a 3 × 3 matrix fitted afterwards on the
output takes ΔE from 7.6 to 4.7, with a stronger green and negative red/blue cross terms) and the tone of the
shadows. That points at the camera's own starting look (Adobe Standard against the file's camera-JPEG fit), which
needs a Lightroom export of this raw with nothing applied to separate it from the preset (gap 1).

## What changed, and the evidence

- **Point Color import** (`crs::point_colors`). `crs:PointColors` was left out of every XMP import. A slot is 19 numbers:
  the sample (hue in radians, saturation, linear luminance), its hue / saturation / luminance shifts (−1..1), the range
  (0..1) and three four-point range curves. Reading the sample's luminance as linear is checked by the preset itself:
  the luminance range curve is centred on 0.4519 = sRGB-encoded 0.1722. Shifts are taken ×100 as our sliders, the range
  ×100, the three range sliders left at 50. Unverified: that our Point Color shifts act like Lightroom's at the same
  numbers.
- **Sony ILCE-6000 distortion** (`arw_lens::distortion`). The camera writes an 11-sample table (`11 65 -7 -129 …
  -1912`, padded with zeros to the 17 values of the 16-sample layout). Read like the validated ILCE-7M3 / 7RM4A
  tables (`1 + sample / 16384` at `i / 10` of the half diagonal, then zoomed to keep the frame inside) it lines up the
  frame with Lightroom's lens-corrected export: the correlation of the two frames' gradients below the horizon goes from
  0.62 to 0.81 (0.82 at the optimum of the unit, which is flat between 13000 and 16384). It also removes the black
  corners (the raw's hard lens vignette sits beyond 0.88 of the half diagonal and the correction pushes it out of the
  frame). Shading is not applied: the camera JPEG and our render agree to 1 % out to 0.85 of the radius, so the table's
  gain function stays undecided (`docs/sony-lens-corrections.md`).
- **ILCE-6000 on its own Kelvin scale** (`kelvin_presets::MODELS`). The body's raw carries Sony's white-balance presets
  from 2500 to 8500 K, so it edits in Kelvin like the other listed Sony bodies. Before, a Lightroom preset's absolute 6900 K
  was read on the relative scale around 6500 K (a warm shift of 9 mired against the 63 mired Lightroom makes from its
  as-shot 4800 K); on the Kelvin scale it is 37 mired. Fitting the white balance freely gave 56 mired, so Sony's dial
  still reads this white about 13 % above Lightroom's (5500 K against 4800 K, as the as-shot values show): a
  Lightroom-to-dial factor is the next step; see the Lightroom scale below.
- **Lightroom's Kelvin on this body's scale** (`kelvin_presets::lightroom_scale`, `crs::lightroom_white_to_dial`). An XMP's
  absolute Kelvin is marked in the settings JSON (`wb.lightroom`, never stored in a photo), and a preset applied
  to, or a sidecar read for, an ILCE-6000 converts it: Kelvin x 1.02, tint - 18. Lightroom reads this file's as-shot white as
  4800 K / +15 and Sony's dial as 5472 K / -1 (ratio 1.14, offset 16), but a sweep of Kelvin and tint against the export
  preferred 7000 K / -16 for the preset's 6900 K / +3, so the constants follow the sweep. Taking +3 as written costs about
  1 ΔE and leaves the greens too red. Presets saved in the app on the same body are in dial units and carry no marker, so
  they are left alone. One body measured; others read Lightroom's values as written.

- **Negative Dehaze** (`finish::add_haze`). The haze was a blend towards the airlight in linear light, which lifted the
  darks by 12 L* (the 1st percentile of lightness went from 8.6 to 20.7) where Lightroom's −16 leaves its blacks at 6.7
  and lifts the mids. It now blends in `c^(1/1.4)`, with strength 0.8 (was 0.7).
- **Highlights above 0** (`finish::HIGHLIGHTS_UP`, 2.5 against the 1.7 of Shadows and negative Highlights). Lightroom's
  +49 brightens bright areas about 1.5 times as much as ours did; the fit kept raising it (2.6 to 3.3), 2.5 is the
  conservative end. Negative values are unchanged: nothing here measures them.
- **Colour grading** (`colorops::GRADING_*`). The split between shadows and highlights at Balance 0 moves from OkLab
  0.50 to 0.62 (sRGB mid-grey 0.5 is 0.60), the transition's width per unit of Blending from 0.50 to 0.42, the wheel's
  chroma from 0.09 to 0.08. In the export the shadow wheel's yellow reaches well into the mids (the grass).

Each of these is mirrored in the GPU kernel (the constants reach WGSL from the CPU values, `gpu/src/ctx.rs`) and
`crates/gpu/tests/it/equivalence.rs` renders them on both.

## What is left (ΔE about 8 here)

Measured on the render of the shipped build against the export, by lightness and by hue:

- Mid-tone chroma: ours is 20 to 30 % below Lightroom's from L* 35 to 80 (greens 36 against 44 at L* 40 to 45, the sea
  and sky 12 against 18), and the greens sit about 10 degrees too yellow-orange in hue. A per-lightness chroma scale
  fitted afterwards would take ΔE from 9.4 to 7.0; one global scale does almost nothing (9.4 to 9.0).
- Tone: Lightroom's blacks are 6 L* deeper and its upper mids 6 to 9 L* brighter (sky and sea at L* 60 to 80).
- Highlights: Lightroom's whites are neutral (b* -0.8), ours are warm (+3.9). Pulling the chroma of the brightest
  17 % of pixels to neutral gains 0.7 ΔE (and costs 0.08 on default EOS R6 renders), so it was not added.
- Mapping the tone per channel instead of on luminance (`BETA` in the scratch fit) takes ΔE from 8.05 to 6.83 and
  accounts for the chroma and the neutral highlights at once, but it raises the error of default EOS R6 renders
  against Lightroom's own default exports from 5.15 to 7.94 (those were tuned to a luminance map), and applying only
  the slider tone per channel made this preset worse (8.05 to 8.91). So the gap is the camera's starting look, as with
  the R6's hue and saturation table: a default Lightroom export of this raw (nothing applied) is what `lightcraft-cli
  calibrate --references` needs to fit it.

## Tried and dropped

- Applying the curves before colour grading (Lightroom's order, as the dark blue channel suggested): ΔE 8.27 against
  7.69 with them after.
- Clipping instead of gamut mapping into sRGB: 7.91 against 7.69.
- A per-primary Calibration strength, and a stronger Shadows Tint: no gain once the white balance and Point Color
  were right (they were absorbing the green saturation the base look lacks).
- Zeroing the colour mixer's Blue band: it only looked better before Point Color was imported.
- +0.125 EV of exposure improved ΔE by 0.6; it is a statement about the camera's starting brightness, which one
  preset cannot separate, so it was not applied.

## Reproducing

The fit's scratch tooling (env-var tunables in the pipeline, a coordinate-descent driver) is not kept: the final
constants are the ones named above. To re-measure, render with the CLI as in *Method* and compare the two JPEGs at
100 px.
