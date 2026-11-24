# Matching a Lightroom preset: the Phoenix 200 "Orange You Glad" test

> **Last reviewed:** 2026-10-25 · **Changes:** [revision history](#revision-history)

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
  to, or a sidecar read for, an ILCE-6000 converts it: Kelvin x 1.06, tint - 11 (until 2026-11-22 x 1.02 and - 18, from the sweep below; refitted on five white-balance exports of that raw, see the section of that date). Lightroom reads this file's as-shot white as
  4800 K / +15 and Sony's dial as 5472 K / -1 (ratio 1.14, offset 16), but a sweep of Kelvin and tint against the export
  preferred 7000 K / -16 for the preset's 6900 K / +3, so the constants follow the sweep. Taking +3 as written costs about
  1 ΔE and leaves the greens too red. Presets saved in the app on the same body are in dial units and carry no marker, so
  they are left alone. One body measured; others read Lightroom's values as written.

- **Negative Dehaze** (`finish::add_haze`). The haze was a blend towards the airlight in linear light, which lifted the
  darks by 12 L* (the 1st percentile of lightness went from 8.6 to 20.7) where Lightroom's −16 leaves its blacks at 6.7
  and lifts the mids. It now blends in `c^(1/1.4)`, with strength 0.8 (was 0.7).
- **Highlights above 0** (`finish::HIGHLIGHTS_UP`, 2.5 against the 1.7 of Shadows and negative Highlights). Lightroom's
  +49 brightens bright areas about 1.5 times as much as ours did; the fit kept raising it (2.6 to 3.3), 2.5 is the
  conservative end. Negative values are unchanged: nothing here measures them. **Superseded 2026-11-06:** Lightroom's
  single-slider exports of this raw put it at 0.55 ([lightroom-light-sliders.md](lightroom-light-sliders.md)); the
  preset's other gaps had leaked into this fit.
- **Colour grading** (`colorops::GRADING_*`). The split between shadows and highlights at Balance 0 moves from OkLab
  0.50 to 0.62 (sRGB mid-grey 0.5 is 0.60), the transition's width per unit of Blending from 0.50 to 0.42, the wheel's
  chroma from 0.09 to 0.08. In the export the shadow wheel's yellow reaches well into the mids (the grass).

Each of these is mirrored in the GPU kernel (the constants reach WGSL from the CPU values, `gpu/src/ctx.rs`) and
`crates/gpu/tests/it/equivalence.rs` renders them on both.

## The tone curves, across six cameras (2026-11-11)

To make sure the curve work is about Lightroom and not one photo, Lightroom exported raws of six bodies (Sony
ILCE-6000 and ILCE-6700, Canon EOS RP, Fujifilm X-A5, Nikon D70, Olympus E-1; private clones in the LightBites app's
`test-raws/light/`, each a one-change XMP sidecar next to a copy of the raw) with Adobe Standard and nothing else, then
with only red / green / blue curves, only a master point curve, only Dehaze, only the light sliders, only a parametric
curve or only a point curve; and two of them (ILCE-6000, D70) with one parametric region at ±50 / ±100, with moved
splits, and with three point-curve shapes. Each change is measured as its *effect* (the export with it minus the export
without it, against the same difference in our renders), so each camera's starting colour drops out.

- **Curve space** (`finish::curve_space`). Lightroom's channel curves change chroma two to three times as much as ours
  did on sRGB's own primaries (+7 to +12 against +1 to +5). Applying all the curves on ProPhoto's primaries under the
  sRGB curve, before the gamut map, matches on every camera (ProPhoto with gamma 2.2 fits as well). SDR only: an HDR
  render keeps its curves on the encoded output.
- **Point curves** (`MonotoneCurve::natural`). Lightroom's point curve is the natural cubic spline through the points
  (within about 1/255); the monotone spline we used is flatter between points. The curve is clipped to 0..1.
- **Parametric regions** (`curve::REGION_SHAPES`). Lightroom's regions reach well past their own windows and are
  stronger: Lights +50 lifts +36/255 at 0.70 from 0.2 to 1, Darks lifts from black (+10 at 0.02), and moving the splits
  widens Lights' reach and strength (+51 at 0.65 with splits 15 / 35 / 60). Each region is now a smoothstep bump placed on
  the split grid with its own gains for raising and lowering, fitted to both raws together (2 to 4/255 rms).

| Effect error, mean ΔE (ours against Lightroom) | Before | After |
|---|---:|---:|
| Red / green / blue curves, six cameras | 9.0 | 2.6 |
| Master point curve, six cameras | 4.0 | 2.3 |
| Parametric probes (S / D / L / H, splits, point shapes), two raws | 4.1 | 2.2 |
| Light sliders, parametric, point curve, six cameras | 2.5 | 1.4 |
| The Phoenix 200 preset, whole render | 11.5 | 10.1 |

Dehaze was untouched here; it was refitted on 2026-11-14 (below).
The D70's Highlights probes moved from 0.6 to 0.8 and its S-curve point probe from 2.9 to 3.8; nothing else got worse.

## Negative Dehaze refitted on six cameras (2026-11-14, card #387)

Lightroom's Dehaze -16 on the six cameras' raws (ILCE-6000, ILCE-6700, EOS RP, X-A5, D70, E-1) lifts the darks, the mids
and, a little less, the highlights by about the same L* (+2 to +5.5) and takes out 1 to 3 of chroma. Ours (a blend in
`c^(1/1.4)` towards 0.9 times the airlight) lifted the darks two to three times as much as the mids and the highlights
hardly at all, and on the D70 took 5 of chroma out. `finish::add_haze` keeps its shape (a blend towards a multiple of the
airlight, weighted by the pixel's dark channel) with five constants fitted on the Apple renderer's display and scene
paths together (Nelder-Mead over the six cameras, rendered output only): gamma 8.4 (close to a blend in log light),
strength 0.9, a floor of 0.37 for pixels with no haze of their own, a target of 1.52 times the airlight, and a
desaturation towards the grey of the new luminance by `(1 - k)^0.56`. A first fit took colour out with a term that, at
-100, pushed colours past grey into their complement; the desaturation factor cannot cross grey (a test holds that).

| Dehaze -16, effect ΔE against Lightroom | Before | After |
|---|---:|---:|
| Apple renderer, display (mean of six; worst) | 2.89 (ILCE-6700 4.39) | 1.89 (ILCE-6700 3.09) |
| Apple renderer, scene (mean of six; worst) | 3.20 (D70 4.58) | 1.81 (ILCE-6700 3.29) |
| Rust renderer (mean of six; worst) | 3.29 (D70 5.09) | 1.80 (ILCE-6700 3.22) |
| D70, Rust (darks +L* ours against Lightroom's +5.5; chroma against -1.5) | 5.09 (+12.3; -5.1) | 1.36 (+5.7; -2.0) |

The ILCE-6700 stays the worst: Lightroom takes 3.1 of chroma out of that photo and lifts it least (+2.0 L*). Positive
Dehaze and the stronger negative values are in the next section.

## Positive Dehaze, and negative beyond -16 (2026-11-14, card #439)

Lightroom's exports of the same six raws at Dehaze +16, +50, +100, -50 and -100 (`test-raws/light/xcam5-lr`, made from
APFS clones with one-line XMP sidecars in its Local tab). Effect ΔE against Lightroom, mean of the six, Apple renderer:

| Dehaze | display before | display after | scene before | scene after |
|---|---:|---:|---:|---:|
| +16 | 2.07 | 1.56 | 1.89 | 1.43 |
| +50 | 5.58 | 4.46 | 4.82 | 4.14 |
| +100 | 10.84 | 11.40 | 9.83 | 10.22 |
| -50 (not refitted) | 5.86 | | 4.98 | |
| -100 (not refitted) | 12.06 | | 9.72 | |

Positive: `finish::dehaze_v4_pixel` keeps its veil model with constants refitted on +16 / +50 / +100: the strength acts in
proportion to the slider (the ease-in of #1237 made +16 act at a third of Lightroom's), the transmission floor is 0.12
(was 0.15) and the veil cap stays at 0.9 of the darkest channel (0.95 fitted a hair better but took the #2504 test's dark
field to black). +100 does not improve: Lightroom's positive Dehaze darkens and deepens even a haze-free photo (the
EOS RP still life loses 6 to 15 L* in its mids at +100, ours under 4) while white stays white, and it turns a hazy sky
an even deep blue where ours blows out near the sun (dividing by a small transmission brightens what is above the
airlight). Tried and dropped: a haze-free share of the effect growing with the slider, a blend space and a brighter
airlight with a saturation term (eight constants did not converge below the plain refit). A new model is card #439.

Negative beyond -16: the shape holds, but Lightroom's lift grows about linearly with the slider (+3.5, +11, +21 L* at
-16, -50, -100) while ours falls about a quarter short at -50 and -100, most in the darks (EOS RP at -100: +35 L*
against +19). Refitting #387's constants on all three values, or adding a haze-free share that grows with the slider,
moved the error by under 3 %, so -16's fit stays.

## Colour Grading, HSL and Calibration (2026-11-14, card #388)

The Lightroom references there are for these tools: Calibration, each slider alone at ±50 on one EOS R6 raw
(`test-raws/light/r6-xmp2/`, white balance 5050 K / +27, Camera Standard, cropped); the colour mixer, Orange, Blue and
Green at Hue +50, Saturation −50 and Luminance −50 on two EOS R6 CR3s and an iPhone ProRAW (`temp-tests/`, Lightroom's base
the per-pixel median of its exports); Colour Grading only inside the Phoenix preset above. None of the six cameras has
an export of these tools yet. Effects measured as for the curves (each export minus its base, against the same difference
of our renders, mean ΔE76 at 150 px inside 0.8 of the half diagonal). The Apple renderer's display path is the app's default
for raws; its scene path draws from the core's camera curve.

- **Shadows Tint** (`colorops::SHADOW_TINT`, `SHADOW_TINT_WINDOW`). Lightroom's ±50 moves a* by 6 to 11 in every tone of
  the (dark) R6 photo up to L\* 70, the same amount both ways, and turns its hue by about 25°. Ours scaled the green channel
  by `1 − 0.3 · t` below half a stop under mid grey and did a quarter of that (+50 also went further than −50). It is now
  `exp(−0.45 · t · w)` with `w` falling from two stops below mid grey to two above. The window reaches higher than the
  scene-path fit alone wants (−3 to +1 stops, 2.95) because the Apple display path applies it to Core Image's
  display-referred light, where the same tones sit higher.
- **The primaries** (Red / Green / Blue Hue and Saturation) keep their 2026-11-08 fit: a search over the primaries' basis
  (Rec. 2020, sRGB, ProPhoto) and their strengths found nothing better than the current constants (mean 1.49 over the
  twelve exports). What is left: Lightroom's Red and Green Hue also turn yellows and purples by 6 to 14° (ours by 1 to
  2°), and its Saturation +50 adds about 3.7 of chroma where ours adds 2.2.
- **The colour mixer** keeps its 2026-10-20 fit. On the scene path and in the core it answers alike (Orange Hue +50
  turns skin 8 to 11°, Lightroom 17 to 18°: loosening the chroma weighting that damps hue shifts of weak colours matches
  that angle but raises the error through chroma and lightness, 0.67 → 0.71). The Apple display path's Orange answers
  a third as much on the R6 photos because Core Image's own look draws R6 skin 13 to 16° yellower than Lightroom and
  the core (CIELAB hue 78 to 84° against 65 to 71°), so it falls half into the Yellow band: a base-look gap, not the mixer.
- **Colour Grading**: no single-wheel reference. The one-change sidecars for all three tools on the six cameras are
  ready in `test-raws/light/xcam4/` (base, six grading cases, eight mixer cases including the halves never exported
  before, four calibration cases, each camera); their exports go in `xcam4-lr/` as `lr-<camera>-<case>.jpg`.
- **The two renderers on the six cameras** (those sidecars, no Lightroom export needed): the Apple display path's effects
  follow the core's within 0.9 to 1.3 ΔE on average for the grading wheels, the mixer and the calibration primaries (each
  from its own base look; worst Yellow Hue −50 on the EOS RP, 3.3, and Green Saturation −50 on the ILCE-6700, 3.8). Its
  Shadows Tint does about half the core's there (3.3 off an effect of 6.9), for the display-referred light above.

| Calibration on the EOS R6, effect ΔE against Lightroom | Before | After |
|---|---:|---:|
| Shadows Tint +50 / −50, Rust renderer | 6.34 / 7.62 | 3.71 / 2.61 |
| Shadows Tint +50 / −50, Apple display path | 7.37 / 8.65 | 3.40 / 3.74 |
| All fourteen exports, Rust renderer | 2.23 | 1.68 |
| All fourteen exports, Apple display / scene path | 2.54 / – | 1.91 / 1.72 |

## Colour Grading, Calibration and Vibrance on six cameras (2026-11-22, cards #386 / #388)

Lightroom exported the six cameras' raws (ILCE-6000, ILCE-6700, EOS RP, X-A5, D70, E-1) with one change each: 18 in
`test-raws/light/xcam4` (six Colour Grading, eight colour mixer, four Calibration) and 24 more in `xcam6` (Colour
Grading with Blending written out, more wheel hues, Balance and Luminance; Vibrance and Saturation at ±50; a custom
white at four values; the Calibration halves `xcam4` lacks; Point Color), and the Phoenix 200 preset split into its
groups on the ILCE-6000 raw (`phx2`: each group alone, and the whole look without each group). Effects are measured as
before (the export with the change minus the export without it, against the same difference of our renders, mean ΔE76
at 150 px inside 0.8 of the half diagonal). Lightroom's own response was read from its exports first (the change per
lightness band on near-neutral pixels, per hue and chroma band on the others, and a white-preserving matrix fitted
to each Calibration export); the constants were then searched on a numpy stand-in of the colour stage applied to our
own base renders (it follows the real renders within 0.3 to 0.8 ΔE per case) and checked with real renders.

- **Colour Grading** (`colorops::grading_tint`, `grading_weights`, `grading_fade`). What the exports show:
  - A wheel's hue is an HSV hue on **ProPhoto's primaries**. The CIELAB direction of the tint on greys is 211° for
    hue 200, 154° for 120, 334° for 300, 12° for 0, 244° for 220, 83° for 50 and 65° for 40; ProPhoto's hues land at
    211.5, 154, 334, 11.5, 246.5, 85 and 67, sRGB's (what the wheels used) at 236, 144, 325, 20, 271, 102 and 91. The
    strength from hue to hue follows the same model within 2 % for the global wheel (a tint is a small step from grey
    towards the wheel's colour in that space).
  - The tint **fades towards black and white**: the global wheel at saturation 30 adds a chroma of 9.7 in the mid
    tones, 2.6 at L\* 5 and 2.9 at L\* 95. Ours added the same offset at every lightness.
  - **Blending** mixes two fixed windows per range: the shadows wheel (hue 220, saturation 60) at L\* 55 adds 17.5 at
    Blending 100, 5.6 at 50 and 1.4 at 0, and the 50 window is 0.7 of the 0 window plus 0.3 of the 100 one at every
    lightness. **Balance** moves the ranges like a gamma on lightness (+50 halves the shadows' reach at L\* 55).
  - The midtones window is a bump of its own around L\* 55, and the shadows and highlights wheels are 1.4 and 2.1
    times as strong as the global one at the same saturation.
  - A sidecar **without `ColorGradeBlending`** is Blending 100 in Lightroom (the older Split Toning panel): its export
    of `grade-sh` is pixel for pixel its export of the same sidecar with Blending 100. The import read it as our
    default, 50 (`crs.rs`).
  - Not modelled: the tint also shifts lightness a little (the midtones wheel at 40 / 40 lifts its range by 1.8 L\*,
    the global wheel at hue 0 / 60 lowers the mid tones by 2.4), and Lightroom's tint is somewhat weaker on colourful
    pixels than on greys.
- **Calibration, the primaries** (`colorops::calibration_matrix`, `CALIB_SAT`, `CALIB_HUE`). A white-preserving 3 × 3
  matrix fitted to each export is the same on every camera once it is written on linear ProPhoto: the primary's own
  channel is untouched and the two others mix with it. Red Saturation +50 is `G' = G + 0.18 (G − R)`, `B' = B + 0.15
  (B − R)`; Green and Blue Saturation likewise on their channels (blue 0.25); a Hue slider moves the next channel
  towards the primary's and the one after away from it; the EOS R6's ±50 exports show −50 is +50's mirror image. So
  every Saturation slider raises the chroma of all colours (20 to 25 % at +50) and Blue Hue +50 turns yellows by 20
  to 27°. The model of 2026-11-08 (each Rec. 2020 primary's chromaticity turned about the white in CIE xy) is replaced
  by that mix, with one strength for red and green and one for blue per slider kind.
- **Calibration, Shadows Tint** (`colorops::SHADOW_TINT`, `SHADOW_TINT_WINDOW`). Lightroom's strength differs from
  raw to raw: at +50 it moves a\* in the dark tones by 10.4 on the ILCE-6000 raw, 6 to 11 on the EOS R6's, 4.3 on the
  D70's, 3.9 on the ILCE-6700's, 3.6 on the EOS RP's (with b\* −3.4), 1.8 on the X-A5's (b\* −2) and 1.7 on the
  E-1's, the same at −50. Strength and direction follow the raws' white balance multipliers (red 2.75, 2.44, 2.17,
  1.70, 1.64, 1.69 and blue 1.68, 1.71, 1.52, 2.17, 1.99, 1.12 for the six) and their exposure: it acts like an offset
  in the raw's own units. Ours was one strength, fitted to the EOS R6 alone (about 9 to 11 on every camera). It now
  sits in the middle (0.45 → 0.25) and reaches from 5 stops below mid grey to 3 above (Lightroom's is strongest in
  the darkest tones). Following each raw needs its white balance gains in the finish stage: open. Built since with a
  strength per body (2026-11-23, card #534, below).
- **Vibrance** (`colorops::vibrance_scale`). Lightroom's −50 leaves every colour 0.46 to 0.56 of its chroma, weak or
  strong; ours left 0.64 to 0.94. Its +50 adds 30 to 45 %, a little less on strong colours, and 10 to 13 % on reds
  and oranges. Vibrance is now `exp(1.1 v)` below zero and `1 + 0.66 v (1 − c)` above, with 0.7 of it held back around
  the reds and oranges. Not modelled: Lightroom's Vibrance moves lightness with the hue (−50 darkens yellows and
  greens by 3 to 5 L\* and lightens blues by up to 6; +50 darkens blues by 1.4 to 5.6).
- **Kept**: Saturation (Lightroom × 0.47 at −50, × 1.5 to 1.65 at +50 with blues at 1.2 to 1.4; ours × 0.5 and
  × 1.5) and the colour mixer (table below).

Effect ΔE against Lightroom's, mean of the six cameras, before → after (the Apple renderer's display path is the app's default for raws; its scene path draws from the core's camera curve):

| Change | Lightroom's effect | Rust | Apple display | Apple scene |
|---|---:|---:|---:|---:|
| Global 200 / 30 | 7.8 | 6.33 → 1.75 | 6.31 → 1.80 | 6.31 → 1.74 |
| Global 120 / 30 | 13.4 | 6.41 → 2.17 | 6.30 → 2.12 | 6.42 → 2.17 |
| Global 300 / 30 | 13.1 | 5.70 → 2.14 | 5.56 → 2.04 | 5.73 → 2.17 |
| Global 0 / 60 | 22.0 | 11.04 → 3.82 | 11.05 → 3.73 | 11.02 → 3.82 |
| Shadows 220 / 60, Blending 100 | 13.4 | 10.10 → 3.88 | 10.27 → 4.15 | 10.03 → 3.87 |
| Shadows, Blending 50 | 7.3 | 4.62 → 2.38 | 4.70 → 2.46 | 4.60 → 2.37 |
| Shadows, Blending 0 | 5.3 | 4.45 → 2.28 | 4.32 → 2.33 | 4.47 → 2.27 |
| Shadows, Blending 100, Balance +50 | 9.1 | 7.37 → 2.81 | 7.55 → 2.91 | 7.29 → 2.83 |
| Highlights 50 / 60, Blending 100 | 11.3 | 8.23 → 4.24 | 7.91 → 4.66 | 8.25 → 4.26 |
| Highlights, Blending 50 | 5.8 | 4.34 → 2.60 | 4.29 → 2.87 | 4.33 → 2.59 |
| Highlights, Blending 50, Balance -50 | 3.2 | 2.36 → 1.45 | 2.40 → 1.59 | 2.35 → 1.41 |
| Midtones 40 / 40 | 5.3 | 3.01 → 2.45 | 3.22 → 2.54 | 2.98 → 2.42 |
| Shadows Luminance +50 | 2.8 | 2.12 → 2.10 | 2.07 → 2.05 | 2.15 → 2.13 |
| Red Hue +50 | 3.6 | 2.06 → 1.63 | 1.86 → 1.38 | 2.08 → 1.65 |
| Red Saturation +50 | 4.0 | 2.64 → 1.90 | 2.50 → 1.72 | 2.63 → 1.92 |
| Green Hue -50 | 3.5 | 1.96 → 1.50 | 1.73 → 1.31 | 1.93 → 1.47 |
| Green Saturation +50 | 4.2 | 2.76 → 1.65 | 2.60 → 1.48 | 2.73 → 1.65 |
| Blue Hue +50 | 7.5 | 2.45 → 2.09 | 2.33 → 1.88 | 2.62 → 2.28 |
| Blue Saturation +50 | 4.0 | 2.34 → 1.83 | 2.13 → 1.39 | 2.34 → 1.81 |
| Shadows Tint +50 | 3.1 | 4.57 → 1.98 | 2.78 → 2.17 | 4.58 → 1.95 |
| Shadows Tint -50 | 3.2 | 4.42 → 1.94 | 2.70 → 2.21 | 4.41 → 1.89 |
| Vibrance +50 | 5.9 | 3.42 → 2.59 | 3.28 → 2.45 | 3.50 → 2.69 |
| Vibrance -50 | 9.7 | 5.79 → 3.70 | 5.72 → 3.54 | 5.88 → 3.84 |
| Saturation +50 | 9.7 | 4.27 | 3.78 | 4.40 |
| Saturation -50 | 10.0 | 3.02 | 2.91 | 3.15 |
| 4000 K / +10 | 10.0 | 7.66 → 7.64 | 6.43 → 6.44 | 6.46 → 6.45 |
| 5500 K / +10 | 5.5 | 6.59 → 6.45 | 4.49 → 4.35 | 4.73 → 4.58 |
| 7500 K / +10 | 14.8 | 8.06 → 7.97 | 9.53 → 9.45 | 9.35 → 9.27 |
| 5500 K / +40 | 8.7 | 6.66 → 6.58 | 5.91 → 5.86 | 5.97 → 5.89 |
| Point Color hue +0.5 | 1.1 | 0.92 | 0.81 | 1.00 |
| Point Color saturation -0.5 | 0.8 | 0.63 | 0.73 | 0.63 |
| Point Color luminance -0.5 | 1.1 | 0.81 | 0.78 | 0.89 |
| Mixer: Aqua Hue +50 | 0.3 | 0.38 | 0.34 | 0.33 |
| Mixer: Blue Luminance +50 | 2.9 | 1.57 | 1.94 | 1.71 |
| Mixer: Green Saturation -50 | 3.2 | 1.40 | 1.61 | 1.40 |
| Mixer: Magenta Saturation -50 | 0.0 | 0.11 | 0.08 | 0.09 |
| Mixer: Orange Luminance +50 | 0.9 | 0.58 | 0.54 | 0.58 |
| Mixer: Orange Saturation +50 | 1.0 | 0.94 | 0.71 | 0.98 |
| Mixer: Red Hue +50 | 0.4 | 0.83 | 0.96 | 0.81 |
| Mixer: Yellow Hue -50 | 1.4 | 1.25 | 1.14 | 1.18 |

Per tool and camera:
**Rust** (mean effect ΔE of the tool's cases, before → after)

| Tool | ILCE-6000 | ILCE-6700 | EOS RP | X-A5 | D70 | E-1 | six |
|---|---:|---:|---:|---:|---:|---:|---:|
| Colour Grading | 5.29 → 2.19 | 5.79 → 2.86 | 6.38 → 3.69 | 5.53 → 2.11 | 6.73 → 2.82 | 5.39 → 2.05 | 5.85 → 2.62 |
| Calibration primaries | 1.80 → 1.31 | 3.33 → 2.29 | 3.66 → 3.10 | 1.65 → 1.29 | 2.16 → 1.24 | 1.60 → 1.35 | 2.37 → 1.77 |
| Shadows Tint | 3.29 → 4.21 | 4.41 → 1.30 | 5.21 → 2.19 | 5.41 → 1.50 | 3.90 → 1.19 | 4.75 → 1.38 | 4.50 → 1.96 |
| Vibrance | 3.00 → 2.74 | 6.61 → 3.99 | 6.66 → 4.14 | 3.02 → 2.72 | 5.65 → 2.94 | 2.72 → 2.37 | 4.61 → 3.15 |
| Saturation | 2.43 | 4.27 | 7.13 | 2.77 | 2.31 | 2.96 | 3.65 |
| Colour mixer | 0.54 | 0.99 | 1.75 | 0.61 | 0.66 | 0.74 | 0.88 |
| White balance | 3.14 → 2.64 | 10.29 | 7.21 | 10.01 | 8.37 | 4.43 | 7.24 → 7.16 |
| Point Color | 1.72 | 0.50 | 0.23 | 1.54 | 0.11 | 0.64 | 0.79 |

**Apple display** (mean effect ΔE of the tool's cases, before → after)

| Tool | ILCE-6000 | ILCE-6700 | EOS RP | X-A5 | D70 | E-1 | six |
|---|---:|---:|---:|---:|---:|---:|---:|
| Colour Grading | 5.28 → 2.39 | 5.93 → 3.50 | 6.45 → 3.91 | 5.59 → 2.14 | 6.33 → 2.46 | 5.47 → 1.87 | 5.84 → 2.71 |
| Calibration primaries | 1.84 → 1.29 | 3.30 → 2.44 | 3.16 → 2.55 | 1.48 → 1.06 | 2.05 → 1.00 | 1.33 → 0.82 | 2.19 → 1.53 |
| Shadows Tint | 3.87 → 5.30 | 1.75 → 1.80 | 2.62 → 2.67 | 2.34 → 0.92 | 2.30 → 1.40 | 3.58 → 1.05 | 2.74 → 2.19 |
| Vibrance | 2.96 → 2.73 | 7.29 → 4.86 | 6.25 → 3.34 | 2.80 → 2.36 | 5.34 → 2.67 | 2.38 → 1.99 | 4.50 → 2.99 |
| Saturation | 2.45 | 5.55 | 5.74 | 2.93 | 1.75 | 1.65 | 3.35 |
| Colour mixer | 0.49 | 1.54 | 1.74 | 0.67 | 0.56 | 0.49 | 0.91 |
| White balance | 3.75 → 3.37 | 7.82 | 6.96 | 8.76 | 7.46 | 4.78 | 6.59 → 6.53 |
| Point Color | 1.72 | 0.49 | 0.22 | 1.50 | 0.11 | 0.62 | 0.78 |

**Apple scene** (mean effect ΔE of the tool's cases, before → after)

| Tool | ILCE-6000 | ILCE-6700 | EOS RP | X-A5 | D70 | E-1 | six |
|---|---:|---:|---:|---:|---:|---:|---:|
| Colour Grading | 5.30 → 2.23 | 5.78 → 2.99 | 6.37 → 3.67 | 5.51 → 1.98 | 6.72 → 2.82 | 5.38 → 2.02 | 5.84 → 2.62 |
| Calibration primaries | 1.87 → 1.40 | 3.41 → 2.43 | 3.63 → 3.06 | 1.66 → 1.27 | 2.17 → 1.27 | 1.60 → 1.37 | 2.39 → 1.80 |
| Shadows Tint | 3.27 → 4.18 | 4.38 → 1.27 | 5.20 → 2.16 | 5.49 → 1.47 | 3.90 → 1.13 | 4.73 → 1.31 | 4.49 → 1.92 |
| Vibrance | 3.16 → 2.96 | 6.75 → 4.28 | 6.65 → 4.12 | 3.02 → 2.74 | 5.76 → 3.04 | 2.78 → 2.43 | 4.69 → 3.26 |
| Saturation | 2.74 | 4.79 | 7.15 | 2.35 | 2.52 | 3.08 | 3.77 |
| Colour mixer | 0.55 | 0.99 | 1.73 | 0.57 | 0.66 | 0.81 | 0.88 |
| White balance | 3.17 → 2.68 | 7.82 | 7.25 | 8.76 | 8.33 | 4.44 | 6.63 → 6.55 |
| Point Color | 1.74 | 0.50 | 0.22 | 1.82 | 0.11 | 0.65 | 0.84 |

The EOS R6's fourteen Calibration exports (`r6-xmp2`, the raw the earlier constants were fitted to), Rust renderer: the twelve primaries 1.44 → 1.74 and Shadows Tint +50 / −50 3.71 / 2.61 → 5.16 / 6.12: that one raw is worse off, the six are each better (table above).

**Still off, with what would settle it** (measured, not changed):

- **White balance from a Lightroom file.** A custom Kelvin on the six raws: effect ΔE 4 to 10 per camera (table above). The X-A5 and the
  ILCE-6700 have no Kelvin scale of their own, so a preset's Kelvin is read around 6500 K and a sidecar's is dropped
  on import; the EOS RP, D70 and E-1 read Lightroom's numbers on their own dial as written, but Lightroom's As Shot
  for these raws (read from its four exports: 3924 K / +34, 5159 K / −12, 5284 K / +10) is not the dial's (3749 K /
  +22, 6502 K / +10, 5300 K / 0). It needs Lightroom's own Kelvin for a raw's as-shot white (the DNG method, from
  the camera's matrices and neutral), not one more body in `kelvin_presets::LIGHTROOM`.
- **Point Color.** The Phoenix preset's blue sample with one shift at a time: Lightroom's hue shift +0.5 turns the
  X-A5's sky by 24° (ours 7°) and the ILCE-6000's by 15° (ours 0°): our sample selects less and turns less. Fitted since
  (2026-11-23, card #536, below).
- **The colour mixer**, mean effect error per case 0.1 to 1.6; the worst cells are the EOS RP's Red Hue +50 (4.4 on an
  effect of 2.1: ours turns its reds by 34°, Lightroom by 12°; no other raw has reds) and Green Saturation −50 on the
  D70 and ILCE-6700 (3.6 and 2.4 on 8.9 and 7.2: Lightroom darkens the greens it desaturates by 1 to 2 L\*).
- Lightroom's lens profile on the ILCE-6000's kit lens brightens the corners (the Phoenix sky by 3 L\*); ours applies
  the distortion only.

### The Phoenix 200 preset, group by group (card #386)

Lightroom exported the preset on the ILCE-6000 raw split up (`test-raws/light/phx2`): nothing applied, each group alone
(white balance, Vibrance and Saturation, the colour mixer, Colour Grading, Calibration, Point Color), every look group
together (no grain, no lens profile), that without each group in turn, and the preset as it is. A group's effect alone
is its export minus the one with nothing applied; in context it is the whole look minus the look without that group.
Effect ΔE of ours against Lightroom's, before → after this work:

| Group | Lightroom's effect (alone / in context) | Rust: alone | Rust: in context | Apple display: alone | Apple display: in context | Apple scene: alone | Apple scene: in context |
|---|---:|---:|---:|---:|---:|---:|---:|
| White balance | 10.8 / 10.8 | 3.94 → 3.26 | 5.06 → 4.54 | 5.25 → 4.66 | 5.40 → 4.88 | 4.03 → 3.37 | 5.28 → 4.76 |
| Light sliders | – / 8.1 | – | 5.85 → 5.12 | – | 4.98 → 4.35 | – | 6.35 → 5.60 |
| Dehaze | – / 5.5 | – | 3.72 → 3.60 | – | 2.91 → 2.92 | – | 3.85 → 3.78 |
| Vibrance and Saturation | 1.2 / 0.7 | 1.37 → 1.31 | 0.94 → 0.86 | 1.27 → 1.25 | 0.86 → 0.81 | 1.30 → 1.23 | 0.90 → 0.81 |
| Tone curves | – / 10.2 | – | 5.41 → 5.28 | – | 5.37 → 5.26 | – | 5.79 → 5.66 |
| Colour mixer | 8.5 / 1.5 | 4.29 → 4.29 | 0.83 → 0.80 | 4.26 → 4.26 | 0.58 → 0.65 | 4.35 → 4.35 | 0.77 → 0.77 |
| Colour Grading | 12.0 / 8.2 | 5.07 → 4.35 | 5.31 → 5.53 | 4.90 → 3.85 | 5.34 → 4.94 | 5.17 → 4.50 | 5.25 → 5.68 |
| Calibration | 3.7 / 3.9 | 2.11 → 2.29 | 2.62 → 2.61 | 2.03 → 2.82 | 2.97 → 3.28 | 2.16 → 2.19 | 2.69 → 2.54 |
| Point Color | 1.2 / 0.3 | 1.03 → 1.03 | 0.25 → 0.25 | 1.03 → 1.03 | 0.25 → 0.25 | 1.04 → 1.04 | 0.25 → 0.25 |

| Whole render against Lightroom's | Rust: ΔE | Rust: sky ΔE | Rust: sky L\* a\* b\* | Apple display: ΔE | Apple display: sky ΔE | Apple display: sky L\* a\* b\* | Apple scene: ΔE | Apple scene: sky ΔE | Apple scene: sky L\* a\* b\* |
|---|---:|---:|---|---:|---:|---|---:|---:|---|
| Nothing applied | 5.06 → 5.06 | 2.63 → 2.63 | 79.8 -3.7 -14.9 → 79.8 -3.7 -14.9 (Lightroom 79.1 -2.0 -14.6) | 6.23 → 6.23 | 3.64 → 3.64 | 81.0 -3.9 -14.8 → 81.0 -3.9 -14.8 (Lightroom 79.1 -2.0 -14.6) | 6.52 → 6.52 | 3.19 → 3.19 | 79.8 -3.8 -15.2 → 79.8 -3.8 -15.2 (Lightroom 79.1 -2.0 -14.6) |
| Every look group (no grain, no lens profile) | 12.91 → 11.33 | 6.99 → 3.84 | 84.5 -14.0 -7.5 → 84.3 -9.1 -6.4 (Lightroom 85.2 -7.7 -8.9) | 10.78 → 10.05 | 6.92 → 5.18 | 87.2 -11.8 -7.5 → 87.2 -8.6 -6.0 (Lightroom 85.2 -7.7 -8.9) | 14.45 → 13.01 | 7.22 → 4.12 | 84.5 -14.0 -7.6 → 84.4 -9.0 -6.4 (Lightroom 85.2 -7.7 -8.9) |
| The preset as it is | 15.56 → 13.99 | 8.75 → 5.16 | 84.5 -14.0 -7.5 → 84.3 -9.1 -6.4 (Lightroom 88.3 -6.3 -7.5) | 11.11 → 10.44 | 7.63 → 5.31 | 87.2 -11.8 -7.5 → 87.2 -8.6 -6.0 (Lightroom 88.3 -6.3 -7.5) | 16.76 → 15.23 | 8.80 → 5.12 | 84.5 -14.0 -7.6 → 84.4 -9.0 -6.4 (Lightroom 88.3 -6.3 -7.5) |

The sky (the upper part of the frame that Lightroom's unedited render draws blue, a third of the measured pixels) was
the visible difference: with every look group applied Lightroom's is L\* 85.2, a\* −7.7, b\* −8.9 and ours was 84.5 /
−14.0 / −7.5 on the Rust renderer, more cyan-green and more saturated (chroma 16.0 against 11.8). In context that a\*
gap of −6.3 came from white balance (−3.4), Colour Grading (−1.8) and the starting look (−1.7), with the curves and the
light sliders adding −0.6 each and the colour mixer and Dehaze taking +1.2 and +1.5 back. With the wheels on ProPhoto's
hues and the ILCE-6000's Lightroom white balance scale refitted to that raw's five custom-white exports (Kelvin × 1.06,
tint − 11; mean effect ΔE 3.30 → 2.76, `kelvin_presets::LIGHTROOM`), ours is 84.3 / −9.1 / −6.4 (chroma 11.3). In the
preset as it is, Lightroom's sky is another 3 L\* lighter (88.3): its lens profile corrects the kit lens's vignetting,
and ours applies that lens's distortion only.

What is left of the whole render, of which the starting look alone is 5.1: the light sliders, the tone curves and
Colour Grading in context (about 5 each: with everything else applied their errors compound, and Lightroom's lift of
the mid tones by the midtones wheel is part of the grading's), white balance (4.5: the far ends of the Kelvin scale)
and Dehaze (3.6).

## White balance written by Lightroom, on six cameras (2026-11-23, card #535)

A custom Kelvin and tint from a Lightroom preset or sidecar did not give Lightroom's white on most cameras (effect ΔE
2.6 to 10.3 in the section above). Measured again here on the same 24 exports (`xcam6`: 4000, 5500 and 7500 K at tint
+10, 5500 K at +40, per camera), with two things the earlier work did not have.

**Lightroom's own As Shot for each raw.** Its exports carry the white it used (`crs:Temperature` / `crs:Tint` in the
JPEG's XMP, also when the white balance is As Shot), so the six values are read, not estimated (the earlier
interpolation of four exports was up to 180 K and 9 tint units off):

| Raw | Lightroom's As Shot | The raw's own scale read | Through |
|---|---|---|---|
| ILCE-6000 | 5150 K / +11 | 5472 K / −1 | Sony's presets in the file |
| ILCE-6700 | 4850 K / +11 | 4940 K / +3 | Sony's presets in the file (the body had no Kelvin scale before) |
| EOS RP | 4100 K / +25 | 3749 K / +22 | its measured matrices (Canon's own `ColorTempAsShot`: 3800 K) |
| X-A5 | 4850 K / +1 | 4430 K / −5 | Fujifilm's neutral pair in the file (no Kelvin scale before) |
| D70 | 5150 K / −3 | 5076 K / −10 (was 6502 K / +10) | its measured matrices |
| E-1 | 5350 K / +11 | 5300 K / 0 | Olympus's presets in the file |

**A sweep of our own white balance.** Each raw rendered at 105 whites of its own scale (mired 60 to 340 in steps of
20, tint −30 to +60 in steps of 15, one `lightcraft-cli run` per camera), the 150 px Lab pictures interpolated between
them, so the error of any conversion can be scored without another render, and the best white per export found: the
floor a conversion of two numbers can reach on today's renderer.

What the sweep says:

- **The white that matches is the same step from As Shot as Lightroom's.** For every camera the best white on our scale
  is, within the floor, Lightroom's mired and tint difference from *its* As Shot applied to *our* As Shot. So a
  Lightroom white is only as good as our idea of Lightroom's As Shot for that raw: an As Shot read 20 mired off moves
  every custom white by 20 mired (about 4 ΔE here), 10 tint units by about 2.
- **The absolute route through matrices works where the matrices are the camera's own and its white is read.** The
  D70's As Shot through its measured matrices is 74 K and 7 tint units from Lightroom's. It read 6502 K / +10 before
  because the D70's NEF keeps its as-shot multipliers only in the maker note's ColorBalance record (version `0103`),
  which the decoder did not read: the white was unknown and stood for D65, so every Lightroom white was 40 mired off.
- **It does not work through anyone else's matrices.** The X-A5's multipliers through the typical camera's matrices
  read 3655 K (Lightroom 4850 K), the E-1's read a tint of −98, the ILCE-6000's through the typical Sony a tint of
  +37: a worse start than leaving the white as shot. Even a camera's own matrices drift between their two calibration
  lights: the EOS RP reads 3749 K where Lightroom says 4100 K (the EOS R6 has the same ratio, 0.90 to 0.91).
- **A camera's own presets are the better start.** Sony's and Olympus's Kelvin presets put the As Shot within 2 to
  6 % of Lightroom's Kelvin, and always about 10 tint units below it: the presets above tungsten are daylight
  settings, daylight sits about 10 units on the green side of the Planckian locus Lightroom's tint counts from (D65
  is +9.5), and our preset curve is tint 0.

What changed (`crates/raw/src/kelvin_presets.rs`, `third_party/craft-libs/crates/raw/src/nef.rs`):

- The **D70** (and any body writing ColorBalance `0103`) has its as-shot white read. Its Kelvin is used as Lightroom
  writes it.
- The **ILCE-6700** and the **X-A5** edit white balance in Kelvin through the presets their files carry
  (`MODELS`, `FUJI_MODELS`), like the ILCE-6000 and the X-Trans III bodies. A sidecar's custom Kelvin is no longer
  dropped for them, and a preset's Kelvin is no longer read as a shift around 6500 K.
- **A body whose scale is a curve through its own presets** (the listed Sony, Canon, Olympus, OM System and Nikon
  bodies) and that nobody measured reads Lightroom's Kelvin as written and its tint 10 lower (`PRESET_CURVE`).
- **Measured bodies** (`LIGHTROOM`, the scale's As Shot over Lightroom's): the EOS RP (Kelvin × 0.914, tint − 3) and
  the X-A5 (× 0.913, − 5.5) join the ILCE-6000 (× 1.06, − 11, unchanged: it is what its As Shot gives, 1.0625 and 12)
  and the EOS R6.
- A body with no Kelvin scale of its own still keeps As Shot for a sidecar's custom Kelvin and says so: with nothing
  of the camera's own to read its white through, a guessed As Shot is not better than none (above).

Effect ΔE of a custom white against Lightroom's export (mean of the four whites per camera; the floor is the best any Kelvin and tint of our scale reaches on the Rust renderer):

| Camera | Lightroom's effect | Rust | Apple display | Apple scene | Floor |
|---|---:|---:|---:|---:|---:|
| ILCE-6000 | 8.2 | 2.6 | 3.4 | 2.7 | 2.4 |
| ILCE-6700 | 7.8 | 10.3 → 2.9 | 7.8 → 3.1 | 7.8 → 3.0 | 2.8 |
| EOS RP | 14.5 | 7.2 → 5.7 | 7.0 → 5.0 | 7.2 → 5.7 | 5.5 |
| X-A5 | 8.8 | 10.0 → 4.0 | 8.8 → 4.4 | 8.8 → 3.9 | 2.3 |
| D70 | 10.6 | 8.4 → 4.6 | 7.5 → 3.9 | 8.3 → 4.6 | 3.4 |
| E-1 | 8.7 | 4.4 → 3.8 | 4.8 → 4.0 | 4.4 → 3.8 | 2.9 |
| Mean | | 7.2 → 3.9 | 6.5 → 4.0 | 6.5 → 3.9 | 3.2 |

Per white, Rust renderer, before → after:

| Camera | 4000 K / +10 | 5500 K / +10 | 7500 K / +10 | 5500 K / +40 |
|---|---:|---:|---:|---:|
| ILCE-6000 | 4.2 → 4.2 | 1.1 → 1.1 | 3.7 → 3.7 | 1.5 → 1.5 |
| ILCE-6700 | 12.2 → 3.3 | 10.0 → 2.0 | 9.6 → 4.9 | 9.3 → 1.6 |
| EOS RP | 5.0 → 1.6 | 7.5 → 6.5 | 9.2 → 9.1 | 7.0 → 5.4 |
| X-A5 | 12.0 → 5.1 | 9.7 → 2.1 | 8.9 → 6.4 | 9.5 → 2.3 |
| D70 | 5.2 → 5.5 | 8.3 → 1.9 | 11.3 → 6.2 | 8.7 → 4.8 |
| E-1 | 7.1 → 5.6 | 2.1 → 0.9 | 5.1 → 5.9 | 3.4 → 3.0 |

(Before, the ILCE-6700's and X-A5's Apple columns are a sidecar import, which kept As Shot, so the whole effect was the error; their Rust column is a preset applied, read as a shift around 6500 K.)

What is left, and why a conversion cannot remove it:

- **EOS RP, 5.5 at best.** Lightroom takes this raw from 4100 K to 5500 and 7500 K, 62 and 111 mired. Its picture
  gets lighter and more saturated than ours as it warms (at 7500 K: L\* +4.0 and chroma +19.9 against +0.7 and
  +15.4 at the best white of our scale), which no Kelvin or tint reaches, and neither does a lighter picture (Exposure
  raised until the mean lightness matches leaves 6.5 at 5500 K and 9.4 at 7500 K, from 6.4 and 9.1): each colour moves
  differently.
  That is the renderer's white balance (`pipeline::local::wb_matrix_for`) against Lightroom's on a large step, to be
  measured on its own; the small step (4000 K) is 1.6.
- **The developed picture is adapted, not re-developed.** On the other five the tint that matches drifts with the
  temperature step, by about −0.05 (the two Sonys) to −0.3 (E-1, X-A5, D70) tint units per mired, and the step that
  matches is 15 % short (ILCE-6000) to 40 % long (E-1) at the far ends. Lightroom rebalances the raw in the camera's
  own space; we adapt the developed picture from its as-shot white (Bradford). The floors are 2.3 to 3.4; the
  conversion lands 0.1 (ILCE-6700) to 1.7 (X-A5) above them, most on the 4000 and 7500 K whites. A rule for the drift
  from the camera's own data was tried (the colour Lightroom's white makes neutral, through typical matrices): it
  gains 0.4 on the X-A5 and nothing elsewhere, so it is not in.
- **Bodies nobody measured**: the Kelvin of a maker's presets is 2 % below to 6 % above Lightroom's from body to
  body (Fujifilm's neutral pair 9 % below), so their Lightroom whites stay up to about 15 mired off until the body's
  As Shot is read next to Lightroom's (one export of one raw is enough: `crs:Temperature` of an As Shot export).

## White balance: Lightroom's lightness, blown highlights and more bodies (2026-11-24, card #535)

The section above left three things open: the EOS RP's large warm steps, a tint that drifts with the step and
highlights that take the tint, and the bodies nobody had measured. Measured again on the same 24 exports, per pixel
and per brightness band this time.

**Lightroom's white balance changes the lightness, ours did not.** Lightroom's 7500 K is lighter than its As Shot
and its 4000 K darker, on every camera, where ours kept the luminance of a neutral:

| Camera | 4000 K | 5500 K | 7500 K | 5500 K / +40 |
|---|---:|---:|---:|---:|
| ILCE-6000 (As Shot 5150 K) | −1.8 (ours +0.1) | +0.5 (0.0) | +2.1 (−0.1) | −0.1 (0.0) |
| ILCE-6700 (4850 K) | −1.1 (−0.1) | +0.7 (0.0) | +2.0 (+0.1) | +0.3 (−0.1) |
| EOS RP (4100 K) | −0.2 (0.0) | +2.1 (+0.4) | +3.9 (+0.7) | +2.5 (+0.5) |
| X-A5 (4850 K) | −0.7 (+0.1) | +0.5 (0.0) | +1.7 (−0.1) | +0.4 (−0.1) |
| D70 (5150 K) | −1.3 (−0.3) | +0.5 (−0.1) | +1.9 (0.0) | +0.9 (−0.2) |
| E-1 (5350 K) | −0.7 (−0.1) | 0.0 (0.0) | +3.0 (+0.1) | +0.8 (0.0) |

(mean L\* of the white balance's effect, variant minus As Shot, 150 px.) That is what a raw developer does: it
balances a new white by scaling the sensor's channels and holds the one with the smallest gain, green, so nothing
unclipped clips; more red and less blue then adds light, the reverse takes it away. Scaling the channels of the
typical camera (`lightcraft_raw::spectral::generic`) with green held predicts Lightroom's shift on all six raws
(a mid grey at 7500 K: +1.9 L\* on the ILCE-6000, +2.4 on the ILCE-6700, +2.8 on the X-A5, +2.1 on the D70, +4.6 on
the EOS RP).

**Blown highlights stay white in Lightroom.** The D70 raw's overcast sky (18 % of the frame) is clipped in the
file. Lightroom shows it white at every white (a\* +0.4 to +0.6, b\* +0.5 to +1.6 across the four); ours tinted it
with everything else (a\* +6 to +13 at the tint steps, b\* −10 to +11 at the Kelvin steps), which is the pink sky of
the section above. Lightroom balances the sensor's values before it clips them; we balance a picture whose clipped
pixels are already white.

**What was tried**, on the Rust renderer, as mean effect ΔE of the 24 exports (the six bodies' new rows in, so the
D70 and the two Olympus / Sony preset bodies start a little off the section above):

| Variant | ILCE-6000 | ILCE-6700 | EOS RP | X-A5 | D70 | E-1 | Mean |
|---|---:|---:|---:|---:|---:|---:|---:|
| Bradford, a neutral's luminance kept (before) | 2.65 | 2.93 | 5.68 | 3.97 | 5.09 | 3.84 | 4.02 |
| The typical camera's channels scaled, green held | 2.60 | 2.52 | 7.06 | 4.03 | 4.70 | 3.76 | 4.11 |
| The same, a neutral's luminance kept | 2.86 | 3.13 | 6.29 | 4.20 | 5.04 | 3.95 | 4.24 |
| The photo's own developed matrix, its smallest gain held | 2.57 | 3.02 | 7.05 | 4.24 | 4.84 | 3.27 | 4.16 |
| The typical camera, green held, blown pixels white | 2.60 | 2.52 | 7.06 | 4.03 | 3.54 | 3.76 | 3.92 |
| Bradford, the camera's green of a grey kept, blown pixels white (what is in) | 2.37 | 2.56 | 6.42 | 3.74 | 3.83 | 3.71 | 3.77 |

Scaling a camera's channels instead of adapting with Bradford is not closer in colour: with the lightness taken
out it is worse on every body (row 3 against row 1), and whether the camera is the typical one or the photo's own
fit changes which body gains (rows 2 and 4). What helps is the lightness (row 2 against row 3: five bodies gain 0.2
to 0.6) and the blown pixels (the D70, 4.70 to 3.54; no other raw here has a blown area). So the colours stay
Bradford's, and two things are added (last row; the D70's 3.83 becomes 3.21 with its row below).

**What changed** (`crates/pipeline/src/local.rs`: `balance_for`, `camera_green`, `Balance`; the GPU kernel
`wb_k`; the Apple renderer's `CameraWhite` and `WhiteBalanceStage`):

- **A raw's white in Kelvin keeps the camera's green of a grey.** The adaptation was scaled so that a grey of the
  picture kept its luminance; it is now scaled so that the grey keeps its green in the typical camera (the same
  normalisation with the camera's green weights, 0.003 / 0.745 / 0.252 on linear Rec. 2020, in place of the
  luminance weights). Holding the green of the colour the new white makes neutral instead, which looks like the
  same thing, overshot on large steps (EOS RP 7.8, its 7500 K 5 L\* too light). A white on the relative scale (a
  rendered photo, a raw whose camera has no Kelvin scale, settings written before a raw had one) is unchanged:
  there the white balance is a colour correction of a finished picture, and old edits keep their look.
- **Blown highlights stay white.** A pixel whose darkest channel is at the sensor's clip before the white balance
  (0.9 to 1 of it, as a ramp) goes to its brightest balanced channel.

**Before → now** (effect ΔE against Lightroom's 24 exports, mean of the four whites; before is the state of the
section above; the Apple columns are the app's own renderer, `tools/apple_parity`):

| Camera | Rust | Apple display | Apple scene |
|---|---:|---:|---:|
| ILCE-6000 | 2.6 → 2.4 | 3.4 → 3.1 | 2.7 → 2.4 |
| ILCE-6700 | 2.9 → 2.6 | 3.1 → 2.7 | 3.0 → 2.6 |
| EOS RP | 5.7 → 6.4 | 5.0 → 6.7 | 5.7 → 6.4 |
| X-A5 | 4.0 → 3.7 | 4.4 → 4.0 | 3.9 → 3.7 |
| D70 | 4.6 → 3.2 | 3.9 → 3.5 | 4.6 → 3.3 |
| E-1 | 3.8 → 3.7 | 4.0 → 3.9 | 3.8 → 3.7 |
| Mean | 3.9 → 3.7 | 4.0 → 4.0 | 3.9 → 3.7 |

Per white, Rust renderer, before → now:

| Camera | 4000 K / +10 | 5500 K / +10 | 7500 K / +10 | 5500 K / +40 |
|---|---:|---:|---:|---:|
| ILCE-6000 | 4.2 → 4.2 | 1.1 → 1.0 | 3.7 → 2.7 | 1.5 → 1.6 |
| ILCE-6700 | 3.3 → 2.5 | 2.0 → 1.8 | 4.9 → 4.0 | 1.6 → 1.9 |
| EOS RP | 1.6 → 1.7 | 6.5 → 7.5 | 9.1 → 10.3 | 5.4 → 6.1 |
| X-A5 | 5.1 → 4.9 | 2.1 → 2.0 | 6.4 → 5.9 | 2.3 → 2.1 |
| D70 | 5.5 → 4.5 | 1.9 → 1.1 | 6.2 → 4.6 | 4.8 → 2.7 |
| E-1 | 5.6 → 6.1 | 0.9 → 0.9 | 5.9 → 4.9 | 3.0 → 3.0 |

Five bodies gain on every renderer, most on the warm whites (the lightness) and the D70 (its sky, and its row).
The lightness itself is a wash in ΔE on the Apple display path (the mean stays 4.0) because of one photo, the
EOS RP, which loses on all three. Its mean L\* is right now or a little over (Lightroom +3.9 at 7500 K, ours +0.7
before and +5.4 now), and the cooler whites overshoot by about 1 L\* (4000 K: Lightroom −0.7 to −1.8, ours −1.7 to
−2.8): the linear change is the camera's, how much L\* it makes is the tone curve's. The GPU kernel gives the CPU's
picture (mean ΔE 0.001 on the D70 at 7395 K / +40), and the unedited pictures did not move.

**What is left, and what would settle it:**

- **EOS RP: its highlights, not its white balance.** By brightness band, the error of its 7500 K white is 7 in the
  dark tones and 16 in the brightest (11 and 16 with the lightness right). The raw is a bright still life at
  4100 K; warming it by 111 mired pushes red and green past white. Lightroom keeps those areas yellow (b\* +21 on
  the brightest pixels), ours roll off to a pale pink (b\* +4): the tone map of the neutral look these CR3s start
  from desaturates light above white where Lightroom's keeps the lower channel, and the unedited pictures already
  differ there (the apple's lit side holds detail in Lightroom's As Shot, ours is near white; base ΔE 8.1). Adding
  the lightness makes this photo's number worse (5.7 to 6.4 on the Rust renderer, 5.0 to 6.7 on the Apple display path) although its lightness is now right (L\* +3.9 in
  Lightroom, +0.7 before): there is more light for the roll-off to lose. What settles it is the highlight roll-off
  of that tone map against Lightroom's on this raw, which is tone work, not white balance.
- **The tint that drifts with the step** (about −0.05 to −0.3 tint units per mired) is not the operator: neither
  camera space above moves it. Lightroom reads a white through matrices that change with the temperature (two
  calibration lights), so its Kelvin axis bends against ours. A constant per body would be fitted on four exports
  each; it needs a temperature series per body (five whites at tint 0, two bodies with matrices of their own)
  before it is worth a rule.
- **Bodies on the relative scale** (below).

**More bodies measured against Lightroom, from exports that already existed.** Every export Lightroom writes carries the
white it used, so the accuracy sets (`acc-lr`, `acc2-lr`: one default export per body) give Lightroom's As Shot for 40
more bodies without another export. Next to it, what the raw's own scale reads (one raw per body):

| Body | Lightroom's As Shot | The raw's own scale | Kelvin ratio | Tint offset |
|---|---|---|---|---|
| EOS 600D | 4850 K / +12 | 4787 K / +24 (measured matrices) | 0.987 | −12 |
| EOS R5 | 3750 K / +16 | 3576 K / +15 (measured matrices) | 0.954 | +1 |
| EOS R50 | 5150 K / +9 | 4793 K / +6 (measured matrices) | 0.931 | +3 |
| EOS R8 | 4850 K / +12 | 4512 K / +16 (measured matrices) | 0.930 | −4 |
| X-T3 | 5150 K / +40 | 4832 K / +34 (measured matrices) | 0.938 | +6 |
| X100V | 5200 K / +26 | 4591 K / +2 (the X-T3's matrices) | 0.883 | +24 |
| LEICA CL | 5000 K / +5 | 5551 K / +9 (the DNG's own matrices) | 1.110 | −4 |
| LEICA M10 | 5300 K / +9 | 5175 K / +4 (the DNG's own matrices) | 0.976 | +5 |
| D70 | 5150 K / −3 | 5076 K / −10 (measured matrices) | 0.986 | 0 (As Shot says +7) |
| E-1 | 5350 K / +11 | 5300 K / 0 (Olympus's presets) | 0.991 | +11 |
| PENTAX KP | 5200 K / +17 | 4650 K / +2 (the DNG's own matrices) | 0.894 | +15 |
| RICOH GR III | 4950 K / +6 | 4953 K / +10 (the DNG's own matrices) | 1.000 | −4 |
| ILCE-6400 | 5850 K / +17 | 5226 K / +10 (measured matrices) | 0.893 | +7 |
| ILCE-6700 | 4850 K / +11 | 4940 K / +3 (Sony's presets) | 1.019 | +8 |
| ILCE-7M3 | 5050 K / +13 | 4268 K / +7 (measured matrices) | 0.845 | +6 |
| Galaxy S22 (SM-S901U) | 5550 K / +16 | 5127 K / −13 (the DNG's own matrices) | 0.924 | +29 |

Each is a row of `LIGHTROOM` now (and of the Swift copy, `CRSMapping.lightroomScales`). The D70 is the one body of these with exports of custom whites, and they overrule its As Shot tint: with Lightroom's tint as written the four are 3.2 off, with the offset of 7 its As Shot gives 3.8 (the Kelvin ratio helps: 3.3 without it). So an As Shot tint offset of a body read through matrices is the best single number there is for it, not a measured one; the two preset bodies with both (ILCE-6700, E-1) come out the same either way. Three bodies read the same
both ways and need none (DJI FC6310 4506 K / +13 against 4500 K / +13, Pixel 8 Pro 6017 K / −8 against 6000 K / −8,
Mi 9 Lite 4382 K / −9 against 4400 K / −9: Lightroom reads these DNGs through their own matrices, as we do). What
the exports could not settle:

- **Three readings of ours are broken, not offset**, so they get no row: the EOS 5D Mark IV sample reads 1785 K / −189
  (Lightroom 5050 K / +13), the Pixel 6 Pro DNG 3376 K / −95 (5800 K / +11) and the LEICA Q (Typ 116) DNG 8557 K / +16
  (5750 K / +14). A ratio from one such point would be wrong at every other white; the as-shot reading of those
  files is its own bug.
- **A body on the relative scale has nothing to convert to.** Eighteen of the exported bodies have neither measured
  matrices nor presets here (EOS 80D, 90D and R3, X-H2S, D500, D5600, D750, Z 5, Z f, Z6III, E-M1 Mark II, OM-5,
  GH5, S1, GX8, RX100 VII, ILCE-7C, ZV-E10): a custom Kelvin from Lightroom still keeps As Shot on them. Lightroom's
  As Shot for one photo does not give the next photo's, so they need a scale first (the camera's presets where the
  file has them, else measured matrices), then one export each.
- **Lightroom's tint depends on its profile.** The X-T3 raw exports as 5150 K / +40 with the default profile and
  5150 K / +79 with Adobe Standard picked by hand. The rows are for the default profile.

**The D100 and the D2H have their as-shot white read.** Their NEFs keep the as-shot levels only in the maker note's
ColorBalance record, like the D70, in two more layouts: version `0100` (D100) with red, blue, green, green at byte
72, version `0102` (D2H) with red, green, green, blue at byte 10 (ExifTool's tag documentation; checked on
raw.pixls.us's `DSC_2376.NEF`, `744 368 256 256`, and `D2A_0005.NEF`, `466 256 256 463`, both added to
`test-raws/MANIFEST.tsv`). Both bodies are on the relative scale and their look is fitted to the camera's preview,
so the picture hardly moves; what changes is every step that needs the real multipliers (the highlights rebuilt at
load, a DNG written from the raw). The D1, D1H, D1X and D2Hs samples write `0x000c` and were read before.

## Point Color on six cameras (2026-11-23, card #536)

Lightroom exported the six cameras' raws with one Point Color sample each (`test-raws/light/xcam7`, 35 cases per camera:
a sample of saturation 0.5, linear luminance 0.2 and range 0.25 at eight hues with Saturation −100; three hues at range
0.1, 0.5 and 1; two at another saturation and luminance; two with Hue ±100, Luminance ±100 and Saturation +100 at range
0.5). Effects are measured as before. The constants were fitted on a numpy stand-in of `PointK` applied to our own base
renders (Point Color runs after the tone curve, so the stand-in follows the real renders within 0.02 ΔE on the mean).

- **Where a sample is** (`crs::point_sample`). A sample is written as hue, saturation and linear luminance. The pixels
  a sample takes most (those that lose most chroma at Saturation −100) are, read as an HSV colour on **linear
  Rec. 2020**, at hue 30, 61 and 224 for samples at 30, 60 and 220; on sRGB's primaries they are at 22, 60 and 216, on
  ProPhoto's at 36, 67 and 230. The import read the sample as an encoded sRGB colour: the Phoenix preset's blue (219°,
  saturation 0.57, luminance 0.17) came out at OkLCh hue 262 with chroma 0.128, a colour no sky has, and selected
  next to nothing of the ILCE-6000's sky. On linear Rec. 2020 it is hue 241, chroma 0.065: the sky's own colour.
- **The range** (`colorops::POINT_HUE_WIDTH`, `POINT_CHROMA_WIDTH`, `POINT_LUM_WIDTH`, `POINT_INNER`). Lightroom's Range
  hardly widens the hues a sample takes: a selection shaped like the sidecar's own range curves fits best with a hue
  window of 30° at Range 0 and 33° at 1. What Range widens is the saturation and the luminance a sample takes (the
  sidecar writes those two windows itself: full within the sample ± Range / 3, none at 0 and 1). A pure green sample
  (hue 120) therefore leaves foliage almost alone at every range (effect 0.2 to 1.0 on the D70's and the ILCE-6700's
  greens); ours took the foliage whole (error 19 and 24 on the D70 at ranges 0.5 and 1, on an effect of 0.7 and 1.0;
  now 0.5 and 0.9). The box keeps its form (smooth windows on OkLCh hue, chroma and lightness); its half-widths are
  now 0.38 + 0.08 R radians, 0.05 + 0.09 R chroma and 0.155 + 0.35 R lightness (they were 0.8 R, 0.16 R and 0.5 R),
  fully selected inside 0.32 of each (0.5).
- **The shifts** (`POINT_HUE`, `POINT_LUM`, `POINT_SAT`, `PointK::apply`). On the pixels the blue sample takes fully,
  Lightroom's Hue ±100 turns the colour by +59° / −48° (OkLab hue; ours ±29°), Luminance ±100 scales lightness by 1.32
  / 0.76 and chroma by 1.29 / 0.77, so the whole colour gets about 1.2 stops lighter or darker (ours added ±0.2 to
  lightness at every lightness and left chroma), and Saturation +100 gives 2.2 times the chroma, −100 about half
  (ours 2 and none). Luminance now scales lightness and chroma together; the strengths fitted with the ranges are
  smaller than those readings (Hue 45°, Luminance e^±0.2, Saturation × 1.89 and × 0.64), because the box selects
  more evenly than Lightroom does (its selection is strongest right at the sample and thins out quickly).
- **Not modelled.** Lightroom's selection is made on its own hue, saturation and luminance (a selection of that shape
  reaches 0.53 on the 25 selection cases where the box reaches 0.62): the EOS RP's oranges are still taken too little
  (sample at hue 30 on that raw: error 3.2 → 2.7 on an effect of 3.4 at range 0.25, 6.9 → 6.2 on 7.7 at 0.5 and
  8.9 → 10.5 on 13.5 at 1). Saturation +100 also darkens what it saturates (−12 L\* on the blue sample) and Hue −100
  raises chroma by 1.4.

Effect error against Lightroom, mean of the six cameras, before → now (the Apple renderer's display path on twelve of
the cases; a sample of hue 120 or 220 at saturation 0.5, luminance 0.2 and range 0.25 unless the row says otherwise):

| Sample (saturation 0.5, luminance 0.2, range 0.25, Saturation −100 unless said) | Lightroom's effect | Rust | Apple display |
|---|---|---|---|
| hue 0 | 0.32 | 0.32 → 0.31 |  |
| hue 30, range 0.1 | 0.66 | 0.64 → 0.55 |  |
| hue 30, range 0.5 | 1.95 | 2.05 → 1.51 |  |
| hue 30, range 1 | 3.07 | 2.90 → 2.31 |  |
| hue 30 | 1.03 | 1.02 → 0.81 | 0.93 → 0.77 |
| hue 60 | 2.40 | 2.31 → 2.12 | 2.37 → 2.14 |
| hue 120, Hue +100 (range 0.5) | 0.34 | 2.31 → 0.43 | 2.27 → 0.51 |
| hue 120, Hue −100 (range 0.5) | 0.34 | 2.19 → 0.42 |  |
| hue 120, luminance 0.05 | 0.21 | 0.31 → 0.24 |  |
| hue 120, luminance 0.5 | 0.12 | 0.12 → 0.13 |  |
| hue 120, Luminance +100 (range 0.5) | 0.46 | 2.31 → 0.40 |  |
| hue 120, Luminance −100 (range 0.5) | 0.36 | 2.38 → 0.34 |  |
| hue 120, range 0.1 | 0.12 | 0.12 → 0.14 |  |
| hue 120, range 0.5 | 0.34 | 4.06 → 0.31 |  |
| hue 120, range 1 | 0.53 | 8.54 → 0.47 | 9.21 → 0.53 |
| hue 120, saturation 0.25 | 0.16 | 0.27 → 0.19 |  |
| hue 120, saturation 0.75 | 0.13 | 0.16 → 0.14 |  |
| hue 120, Saturation +100 (range 0.5) | 0.59 | 2.36 → 0.48 |  |
| hue 120 | 0.16 | 0.30 → 0.19 | 0.19 → 0.18 |
| hue 180 | 0.15 | 0.16 → 0.15 |  |
| hue 220, Hue +100 (range 0.5) | 2.15 | 1.67 → 0.98 | 1.47 → 0.95 |
| hue 220, Hue −100 (range 0.5) | 2.38 | 1.85 → 1.22 | 1.63 → 1.21 |
| hue 220, luminance 0.05 | 0.42 | 0.42 → 0.41 |  |
| hue 220, luminance 0.5 | 1.22 | 1.22 → 1.05 |  |
| hue 220, Luminance +100 (range 0.5) | 2.85 | 1.96 → 1.37 | 2.03 → 1.40 |
| hue 220, Luminance −100 (range 0.5) | 2.09 | 1.47 → 0.87 | 1.51 → 0.87 |
| hue 220, range 0.1 | 0.48 | 0.48 → 0.40 |  |
| hue 220, range 0.5 | 1.19 | 1.14 → 0.82 |  |
| hue 220, range 1 | 2.37 | 2.58 → 1.59 | 3.10 → 1.51 |
| hue 220, saturation 0.25 | 0.82 | 0.80 → 0.62 |  |
| hue 220, saturation 0.75 | 0.31 | 0.31 → 0.30 |  |
| hue 220, Saturation +100 (range 0.5) | 2.54 | 1.90 → 1.70 | 1.86 → 1.68 |
| hue 220 | 0.69 | 0.68 → 0.53 | 0.65 → 0.51 |
| hue 240 | 0.46 | 0.46 → 0.45 |  |
| hue 300 | 0.08 | 0.08 → 0.08 |  |
| **mean** | 0.96 | 1.48 → 0.69 | 2.27 → 1.02 (12 cases) |

| Camera (35 cases) | a6000 | a6700 | rp | xa5 | d70 | e1 |
|---|---|---|---|---|---|---|
| Lightroom's effect | 1.28 | 0.66 | 1.15 | 1.43 | 0.44 | 0.77 |
| Rust before | 1.22 | 1.33 | 1.01 | 1.48 | 2.99 | 0.85 |
| Rust now | 0.76 | 0.57 | 0.97 | 0.87 | 0.43 | 0.52 |

On the Phoenix preset's own sample (blue, range 0.25) with one shift at a time (`xcam6`, the cases of 2026-11-22: hue
+0.5, saturation −0.5, luminance −0.5), mean effect error per camera, Rust renderer: ILCE-6000 1.72 → 0.97, ILCE-6700
0.50 → 0.36, EOS RP 0.23 → 0.31, X-A5 1.54 → 1.42, D70 0.11 → 0.11, E-1 0.64 → 0.50, all 0.79 → 0.61. The hue shift now
turns the ILCE-6000's sky by 17° (Lightroom 15°, ours was 0°) and the X-A5's by 13° (Lightroom 24°, ours was 7°); the
EOS RP's few blues are not taken at all (Lightroom's effect there is 0.3). The Apple renderer's Point Color is the
same code in Metal (`ColorKernel`), checked against the core's values (`ColourGolden`, worst difference 0.0005).

## Shadows Tint as Lightroom's, with a strength per body (2026-11-23, card #534)

Lightroom exported the six raws with Shadows Tint at +25 and ±100, at +50 with Exposure ±1 and at ±50 with the white
balance at 3500 K and 8000 K (`test-raws/light/xcam7`; the ±50 are `xcam4` / `xcam6`), and six photos of one body
(EOS R6, `xcam8`) at ±50. Effects are measured as before.

- **What it is.** Lightroom's Shadows Tint is an amount of light taken away before Exposure and the tone curve: linear
  in the slider (ILCE-6000, green at L\* 15 to 25: 0.20 / 0.39 / 0.73 log units at +25 / +50 / +100), doubling with
  Exposure +1 and halving with −1 at the same display tone. Plus takes green away, minus red and blue. Ours was a
  gain on green inside a tone window with the luminance kept, the same on every raw.
- **Its strength is a constant of the camera body.** Green at L\* 15 to 25 for +50 moves by 28.5 (× 0.01 log units)
  on the ILCE-6000, 19.3 on the EOS RP, 13.1 on the D70, 10.5 on the X-A5, 10.3 on the ILCE-6700 and 5.1 on the E-1,
  and by 8.9 to 11.6 on six EOS R6 photos of different ISO, exposure, white balance and scene. It follows nothing in
  the file that was tried (white balance gains, bit depth, black and white level, ISO, exposure, the picture's
  levels: in the raw's own units the offset is 1.9 to 7.9 thousandths of the raw range).
- **The form** (`colorops::calibrate`, `SHADOW_TINT_PLUS`, `SHADOW_TINT_MINUS`; `finish.wgsl`; the Apple renderer's
  `CalibrationKernel`). The tint now comes before the primaries and takes light away from the scene-linear colour:
  `c − axis · |t| · gain`, `t` the slider / 100, `gain` the Exposure slider's, `axis` one colour for plus and one for
  minus (linear Rec. 2020: plus takes 0.0063 of green at 100, minus 0.0073 of red and 0.0037 of blue), fitted with
  the strengths below on a numpy stand-in of the finish stage, on the six cameras and the six EOS R6 photos
  together, the EOS R6 as the unit.
- **The strength per body** (`crs::lightroom_shadow_tint_to_slider`, `LIGHTROOM_SHADOW_TINT`). A value read from a
  Lightroom file (a sidecar, a preset: `calibration.lightroom` marks it on the way, as `wb.lightroom` does a white
  balance) is multiplied for the photo's body where it is applied: ILCE-6000 × 2.89, D70 × 1.5, EOS RP × 1.35,
  EOS R6 × 1, ILCE-6700 × 0.86, E-1 × 0.82, X-A5 × 0.77; any other body × 1. The renderers need no fact of the
  photo. The slider keeps its ±100 and the value is stored up to ±300 (2026-11-24: `controls::SHADOWS_TINT_STORED`,
  `ControlSpec::stored`; the renderers clamp there), so an ILCE-6000 value past ±35, a D70's past ±67 and an EOS
  RP's past ±74 keep Lightroom's strength: the slider then stands at its end, and dragging it brings the value back
  inside ±100. The presets of the test set carry −2 to −28.

Effect error against Lightroom per camera, before → now:

Rust renderer:

| Case | Lightroom's effect | a6000 | a6700 | rp | xa5 | d70 | e1 | mean |
|---|---|---|---|---|---|---|---|---|
| Shadows Tint +50 | 3.1 | 4.08 → 2.07 | 1.41 → 1.44 | 2.09 → 2.26 | 1.59 → 1.30 | 1.32 → 0.85 | 1.42 → 0.89 | 1.98 → 1.47 |
| Shadows Tint −50 | 3.2 | 4.34 → 1.92 | 1.19 → 1.55 | 2.30 → 2.46 | 1.41 → 1.20 | 1.06 → 0.82 | 1.34 → 0.88 | 1.94 → 1.47 |
| +25 | 1.6 | 2.20 → 1.16 | 0.91 → 0.90 | 1.21 → 1.28 | 0.95 → 0.81 | 1.01 → 0.75 | 0.91 → 0.71 | 1.20 → 0.93 |
| +100 | 5.9 | 7.27 → 3.90 | 2.38 → 2.73 | 3.79 → 4.20 | 3.01 → 2.35 | 1.98 → 1.27 | 2.62 → 1.43 | 3.51 → 2.65 |
| −100 | 6.3 | 8.28 → 2.85 | 1.93 → 2.52 | 4.20 → 4.69 | 2.59 → 2.04 | 1.33 → 1.30 | 2.39 → 1.28 | 3.45 → 2.45 |
| +50 at Exposure +1 | 4.4 | 6.53 → 2.44 | 2.91 → 1.53 | 3.98 → 3.17 | 1.46 → 1.59 | 3.02 → 1.98 | 1.35 → 0.99 | 3.21 → 1.95 |
| +50 at Exposure −1 | 2.1 | 2.08 → 1.99 | 1.70 → 1.24 | 2.17 → 1.64 | 2.53 → 1.11 | 1.36 → 0.82 | 1.73 → 0.78 | 1.93 → 1.27 |
| +50 at 3500 K | 3.9 | 5.75 → 3.61 | 2.15 → 2.21 | 2.66 → 2.75 | 2.25 → 2.37 | 2.04 → 2.33 | 1.70 → 1.63 | 2.76 → 2.48 |
| −50 at 3500 K | 3.6 | 4.46 → 2.46 | 1.47 → 2.16 | 2.63 → 2.83 | 1.24 → 1.66 | 1.79 → 2.40 | 1.47 → 1.58 | 2.18 → 2.18 |
| +50 at 8000 K | 3.4 | 4.85 → 3.38 | 2.15 → 1.64 | 1.69 → 1.47 | 1.44 → 0.82 | 3.01 → 2.69 | 2.11 → 1.81 | 2.54 → 1.97 |
| −50 at 8000 K | 3.3 | 4.80 → 2.95 | 1.54 → 1.60 | 1.24 → 1.51 | 1.31 → 0.87 | 2.40 → 2.39 | 1.78 → 1.62 | 2.18 → 1.82 |
| **mean** | | 4.97 → 2.61 | 1.79 → 1.78 | 2.54 → 2.57 | 1.80 → 1.47 | 1.85 → 1.60 | 1.71 → 1.24 | 2.44 → 1.88 |

The Apple renderer's display path, on the five cases rendered:

| Case | Lightroom's effect | a6000 | a6700 | rp | xa5 | d70 | e1 | mean |
|---|---|---|---|---|---|---|---|---|
| +25 | 1.6 | 2.69 → 1.88 | 1.00 → 1.01 | 1.38 → 1.39 | 0.68 → 0.77 | 1.03 → 0.86 | 0.74 → 0.60 | 1.26 → 1.08 |
| +100 | 5.9 | 9.57 → 6.33 | 3.02 → 3.42 | 4.79 → 4.79 | 1.88 → 2.47 | 2.22 → 2.04 | 2.04 → 1.35 | 3.92 → 3.40 |
| −100 | 6.3 | 10.58 → 6.76 | 3.53 → 4.06 | 5.30 → 5.17 | 1.28 → 2.20 | 2.06 → 1.80 | 1.76 → 1.31 | 4.08 → 3.55 |
| +50 at Exposure +1 | 4.4 | 7.39 → 4.39 | 3.70 → 2.62 | 4.66 → 3.89 | 1.60 → 1.71 | 3.13 → 1.98 | 1.31 → 1.06 | 3.63 → 2.61 |
| +50 at Exposure −1 | 2.1 | 2.76 → 2.54 | 0.86 → 1.28 | 1.38 → 1.66 | 1.29 → 1.02 | 1.00 → 0.87 | 1.53 → 0.82 | 1.47 → 1.36 |
| **mean** | | 6.60 → 4.38 | 2.43 → 2.48 | 3.50 → 3.38 | 1.35 → 1.63 | 1.89 → 1.51 | 1.48 → 1.03 | 2.87 → 2.40 |

In 14 of the 66 cells (8 of the Apple path's 30) the scaled value lies past the slider's ±100. Until 2026-11-24 it
stopped there, and those cells stayed weak: 4.55 → 3.77 with the cut and 2.79 stored in full on the Rust renderer
(all 66: 2.06 with the cut), 5.58 → 5.64 and 4.23 on the Apple path (all 30: 2.77 with the cut). The numpy stand-in
of the finish stage the constants were fitted on reaches 1.70 on the 66 cells, and 0.8 to 1.5 on the six EOS R6
photos. The "now" renders also carry other cards' later changes to the base rendering (the neutral looks of the
EOS RP, E-1 and D70, white balance); every case is measured against its own base.

- **Left.** Under a custom white balance Lightroom's minus direction changes (red and blue in proportion to the
  white balance's gains) and ours does not: the four cases at 3500 K and 8000 K 2.42 → 2.11. The EOS RP gains
  nothing (2.54 → 2.57, its ±100 3.79 → 4.20 and 4.20 → 4.69), not explained. A body nobody measured gets the EOS
  R6's strength, and the measured ones differ 3.7 times. The Apple renderer's display path gains less (it takes the
  light from display-referred values). A Shadows Tint set before this change renders differently (light taken away
  where it was a green gain), and a value imported from Lightroom before it keeps its unscaled number.

## White balance: per-body maps from a series of whites, more Kelvin scales, and the EOS RP (2026-11-24, card #535)

The section above left five things. Each was taken up the same day.

**A series of Lightroom whites per body.** Four whites per camera cannot tell a tint that drifts from a Kelvin ratio,
so Lightroom exported ten more of each of the six raws (`xcam9`: 3000, 4000, 5000, 6500, 8000 and 10000 K at its As
Shot tint; tints −30, 0, +30 and +60 at its As Shot Kelvin). Each raw's own white balance was swept again (105
whites) and the best white found per export:

- **The two scales are not a ratio apart.** In mired, the dial's white is a line of Lightroom's with a slope that is
  not the inverse of the As Shot ratio: 0.85 (ILCE-6000), 0.92 (ILCE-6700) and 0.86 (X-A5) with offsets of +14 to +48
  mired, 1.09 (D70) and 1.08 (E-1) with offsets of −14.
- **The tint drifts with the temperature**, by −0.06 (ILCE-6000, ILCE-6700), −0.15 (X-A5), −0.18 (D70) and −0.31
  (E-1) tint units per mired of Lightroom's white: its Kelvin axis bends against ours, as the first section here
  guessed from four whites.
- **It holds across the two sets.** A map fitted on the ten new whites reads the four old ones as well as one fitted
  on all fourteen, and the other way round (within 0.1 to 0.4 ΔE).

So `LightroomScale` has two more numbers for a body with such a series (`mired_offset`, `tint_per_mired`; 0 for a body
measured at one white), and five bodies have their map. Effect ΔE, Rust renderer, a ratio and an offset → the map:

| Camera | The 4 whites of `xcam6` | The 10 of `xcam9` | 3000 K alone |
|---|---:|---:|---:|
| ILCE-6000 | 2.4 → 2.1 | 3.0 → 2.8 | 7.6 → 8.3 |
| ILCE-6700 | 2.6 → 2.4 | 3.2 → 3.2 | 6.9 → 7.4 |
| X-A5 | 3.7 → 2.2 | 4.6 → 2.8 | 10.7 → 7.0 |
| D70 | 2.7 → 2.3 | 3.4 → 3.1 | 6.7 → 9.5 |
| E-1 | 3.3 → 2.0 | 4.4 → 3.3 | 13.0 → 12.6 |
| EOS RP (no map) | 6.4 → 6.5 | 6.5 → 6.6 | 10.8 → 11.2 |
| Mean | 3.5 → 2.9 | 4.2 → 3.7 | |

(The "before" column is on the core as it was that morning: the D70 and E-1 had got their own neutral look the night
before, card #447, which alone took them from 3.2 and 3.7 to 2.7 and 3.3.) Five of the six are within 3 ΔE on the
four whites the work started from. 3000 K is the white no map reaches (7 to 13 off on every body; it was left out of
the fits): a daylight raw taken to tungsten is beyond what adapting the developed picture does well.

**More bodies on a Kelvin scale.** Of the 18 exported bodies that had none:

- **Five have one now, from the camera's own data in the file**, and a row from their As Shot: the RX100 VII, ILCE-7C
  and ZV-E10 (Sony's five presets), the X-H2S (Fujifilm's neutral pair), and the EOS 80D, whose sample is an sRAW
  (below). Their readings against Lightroom's: 5292 K / 0 against 5400 K / +15, 5698 K / +27 against 5400 K / +30,
  7045 K / +21 against 6350 K / +32, 5045 K / +7 against 5100 K / +16, 5655 K / −1 against 5700 K / +4.
- **The E-M1 Mark II and OM-5 samples are not developed at all**: compressed ORF is not decoded, the photo shows its
  embedded preview, so its white balance is a rendered picture's. Their gap is the decoder, not the scale.
- **The EOS 90D and R3** (CR3) carry Canon's presets in a `ColorData` version (19, 34) whose layout is not read; the
  CR2 reader knows versions −4 and 13 to 15. A table of the two layouts, checked on a file each, would give them one.
- **The GH5, S1 and GX8** (RW2) carry named presets (Fine Weather, Cloudy, Shade…) without temperatures, and **the
  D500, D5600, D750, Z 5, Z f and Z6III** carry none readable (Nikon's are encrypted): these need measured matrices
  (a spectral measurement of the body) or the maker's stated temperatures per preset (as the Z5II has).

**The three readings that looked broken.**

- **The EOS 5D Mark IV sample is an sRAW.** Its samples are stored white-balanced, so its multipliers are 1 and the
  measured matrices read 1785 K / −189 from them. The file records the white it was balanced for (`2036 1024 1024
  1509`); a Kelvin scale reads that now (`ColorData::recorded_wb`): 4847 K / +9, Lightroom 5050 K / +13. The same fix
  gives the EOS 80D sRAW its presets.
- **The Pixel 6 Pro and LEICA Q DNGs are not misread.** Recomputed from the files' tags by hand, their own matrices
  give exactly what we show (3376 K / −95 and 8557 K / +16); Lightroom develops these files with matrices of its
  own and shows 5800 K / +11 and 5750 K / +14. They get rows, so a Lightroom white lands on the photo's own, exact at
  As Shot; one photo each cannot say how the two scales run apart from there.

**The Apple renderer and raws the core re-develops.** A DNG (an iPhone ProRAW among them) is re-developed by the core
through its own matrices for a new white; the Apple renderer adapts its development instead and could not tell such a
raw from another, so it gave it the new lightness and highlight rule too (two ProRAW files with a cooler white moved 2
to 3 ΔE away from Lightroom). It is told now (`RenderSettings.ownColourModel`, from the photo's format) and keeps the
adaptation it had for them.

**The EOS RP.** Its gap at a strongly changed white was taken for a highlight roll-off. It is not one thing that was
tried:

| What was tried on the EOS RP raw | Effect ΔE, 4 whites | 7500 K |
|---|---:|---:|
| As it is | 6.4 | 10.3 |
| The best white of our scale for each export (no conversion can beat it) | 5.9 | 10.0 |
| A neutral-look table fitted to Lightroom's export of the raw (`calibrate --min 1`) | 6.5 | 10.7 |
| The raw re-developed for the new white through its measured matrices, as a DNG is | 6.2 | 10.5 |
| Saturation +10 to +40 on top (its chroma is 5 short at 7500 K) | | 10.2 to 12.9 |

The table is in all the same (maintainer decision, 2026-11-24): it brings the unedited picture closer to
Lightroom's (7.8 → 5.8, the bright tones 11.1 → 5.9, `docs/camera-preview-colour.md`), which is what it can do. What
the series shows instead is that this raw's error grows with the size of the change in every direction, tint
included (floors: 0.9 at 4000 K, 4.6 at 5000 K, 8.3 at 6500 K, 11.7 at 10000 K, 8.0 at 3000 K; 6.1 at tint −30), and
that matching the mean chroma does not help: colours move differently, not less. It is the one raw of the six with
large saturated areas (fruit, a red and gold tin, a green bag under warm light). What would settle whether that is
the scene or the camera: the same series for a second EOS RP raw of an ordinary scene, and for a saturated scene
from one of the five bodies that are within 3. If it is the scene, the gap is how saturated colours take a white
balance (Bradford on the developed picture against Lightroom's camera-space gains through its own profile), and the
fix is in the renderer's white balance for saturated colours, not in a table.

**Where the six stand** (effect ΔE against the 24 exports of `xcam6`, mean of four whites; the section above → now.
In between, the D70 and E-1 also got their neutral look, card #447, which the Rust and Apple scene columns include):

| Camera | Rust | Apple display | Apple scene |
|---|---:|---:|---:|
| ILCE-6000 | 2.4 → 2.1 | 3.1 → 2.9 | 2.4 → 2.1 |
| ILCE-6700 | 2.6 → 2.4 | 2.7 → 2.4 | 2.6 → 2.4 |
| EOS RP | 6.4 → 6.5 | 6.7 → 6.7 | 6.4 → 6.5 |
| X-A5 | 3.7 → 2.2 | 4.0 → 3.2 | 3.7 → 2.2 |
| D70 | 3.2 → 2.3 | 3.5 → 3.3 | 3.3 → 2.3 |
| E-1 | 3.7 → 2.0 | 3.9 → 3.1 | 3.7 → 1.9 |
| Mean | 3.7 → 2.9 | 4.0 → 3.6 | 3.7 → 2.9 |

The Apple display path (Core Image's own look of the raw) keeps more of a gap on the X-A5, D70 and E-1 than the two
paths that develop on the core's scale; its conversion is the same, so what is left there is that look's.

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

The six-camera comparison is a scratch script (render each sidecar with `lightcraft-cli render`, box-downsample both to
150 px wide, compare the effects inside 0.8 of the half diagonal); the references are the private exports named above.


The fit's scratch tooling (env-var tunables in the pipeline, a coordinate-descent driver) is not kept: the final
constants are the ones named above. To re-measure, render with the CLI as in *Method* and compare the two JPEGs at
100 px.

## Revision history

| Date | Change |
|---|---|
| 2026-11-24 | White balance, the five things left open: per-body maps from a series of 14 Lightroom whites (five bodies within 3 ΔE), five more bodies on a Kelvin scale from their own presets, sRAW's recorded white, rows for two DNGs, the Apple renderer told which raws the core re-develops, and the EOS RP's gap narrowed to its scene or its saturated colours (card #535) |
| 2026-11-24 | White balance: a raw's white in Kelvin keeps the camera's green channel (lighter when warmer, as Lightroom) and blown highlights stay white; 16 more bodies measured against Lightroom from existing exports; the D100's and D2H's as-shot white read (card #535) |
| 2026-11-24 | Shadows Tint: a value scaled for a strong body is stored past the slider's ±100, up to ±300 (card #534): effect error 2.06 → 1.88 on the Rust renderer, 2.77 → 2.40 on the Apple renderer's display path |
| 2026-11-23 | Shadows Tint in Lightroom's form (light taken away before the primaries, following Exposure) with a strength per camera body applied where a Lightroom value is read (card #534) |
| 2026-11-23 | Point Color fitted to Lightroom's exports of one sample on six cameras: the sample read on linear Rec. 2020, the range box and the three shifts refitted, Luminance scales the whole colour (card #536) |
| 2026-11-23 | White balance written by Lightroom measured against its own As Shot on six cameras: the D70's as-shot white read, the ILCE-6700 and X-A5 on a Kelvin scale from their own presets, the EOS RP and X-A5 measured, a tint rule for preset scales (card #535) |
| 2026-11-22 | Colour Grading, the Calibration primaries, Shadows Tint and Vibrance refitted on six cameras; the Phoenix 200 preset measured group by group; white balance, Point Color and Saturation measured (cards #386 / #388) |
| 2026-11-15 | Header in the revision-history form upstream's `cargo xtask parity` asks for (upstream v0.21.1 merged). |
| 2026-11-14 | positive Dehaze refitted, negative Dehaze checked at -50 and -100 (card #439); Calibration's Shadows Tint refitted; Colour Grading, HSL and Calibration checked against the exports there are |
