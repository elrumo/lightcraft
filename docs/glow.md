# Glow

> **Last updated:** 2026-11-09 · **Status:** built (Effects panel, under Grain; `ctl:glow.*`), not fitted to Lightroom

Glow is light spread from a photo's bright parts, as a lens, a diffusion filter or film scatters it. Lightroom added
it to the Effects panel in 2026 (Glow, a Style menu with Bloom, Diffusion and Halation on the desktop and also Motion
and Streak on mobile, and Range, Spread and Warmth). This is our own model of the same controls, built from what the
controls are described to do and from looking at our own renders; no Lightroom file or output was read or fitted.

## Settings

`glow` in the develop settings (`lightcraft_develop::Glow`), left out of the JSON while it is the default so settings
without a glow keep their JSON and `hash64`. It sits in the Effects panel: the Effects eye switches it off, and the
Effects group of Copy Settings, presets and sync carries it.

| Control | Range | Default | What it does |
|---|---|---|---|
| `glow.amount` (Glow) | 0..100 | 0 | how strong the glow is |
| `glow.style` | Bloom, Diffusion, Halation, Motion, Streak | Bloom | how the light spreads (below) |
| `glow.range` (Range) | 0..100 | 50 | which tones glow: only the brightest at 0, down into the midtones and shadows at 100 |
| `glow.spread` (Spread) | 0..100 | 50 | how far the light reaches |
| `glow.warmth` (Warmth) | −100..100 | 0 | the glow's colour, cool to warm; Halation's runs from yellow-orange to red |
| `glow.angle` (Angle) | −90..90° | 0 | the direction of Motion and Streak (0: horizontal, positive turns counter-clockwise); shown for those two only |

Angle is ours: Lightroom's controls for Motion and Streak beyond the four above were not seen.

## Method (`crates/pipeline/src/glow.rs`)

The glow is added in scene-linear light (linear Rec. 2020) right after noise reduction and Lens Blur, before the local
tone and the tone map. The stages after it develop the glow with the rest of the light: it rolls off into the
highlights as scattered light does, Highlights and Shadows see it, and an HDR render keeps it above SDR white. It runs
on the un-exposed image (the scene-linear stage's), with Exposure's gain folded into the bright pass's knee.

1. **Bright pass.** Each pixel's light `c` is weighted by `w = smoothstep(k0, k0 + width, log2(Y(c) · 2^Exposure))`,
   `Y` its luminance, `k0 = start − range · Range/100` stops below SDR white (`start`, `range` and `width` by style, below). Halation keeps the luminance alone (`Y · w`
   in every channel); the other styles keep the light's colour (`c · w`).
2. **Spread.** The bright light is blurred, sizes in multiples of a base sigma
   `σ = 0.003 · 2^(5.5 · Spread/100)` of the long edge (0.3 % at Spread 0, 2 % at 50, 13.6 % at 100), so a preview and
   an export look alike. Gaussians are `lightcraft_raster::blur::gaussian` (three boxes); a line style's boxes run
   along the angle (the image sheared so the line is a row, the rows box-blurred with running sums, sheared back: linear
   in the pixels whatever the length; a mostly vertical line is transposed first), each at most the long edge.
3. **Colour and add.** `out = c + Glow · add · tint · spread − Glow · take · bright`. The tint is the Warmth colour
   scaled to luminance 1, in linear sRGB before it is taken to the working space: `(1 + 0.45 w, 1 + 0.05 w, 1 − 0.55 w)`
   for a filter over the light's own colour, or for Halation the halo's own colour, `(1, 0.5, 0.18)` (w = −1) to
   `(1, 0.2, 0.08)` (w = 1). Diffusion takes part of the bright light from where it was (`take`), so the photo softens
   instead of only brightening.

### Styles

| Style | Knee start (stops below white, Range 0) | Range 100 moves it down (stops) | Knee width | Spread (× σ: weight) | add | take |
|---|---:|---:|---:|---|---:|---:|
| Bloom | −1 | 4 | 1.5 | Gaussians 1: 0.6, 4: 0.4 (a bright core and a long tail) | 1.5 | 0 |
| Diffusion | −2.5 | 4 | 3 | Gaussians 0.5: 0.5, 2: 0.5 | 0.8 | 0.5 |
| Halation | −0.5 | 2.5 | 1 | Gaussian 0.5 (a tight halo), luminance only | 0.8 | 0 |
| Motion | −1 | 4 | 1.5 | a box of half-length 3 along the angle | 1.5 | 0 |
| Streak | −0.5 | 2.5 | 1 | boxes of half-length 8, 2 and 0.5 along the angle, averaged (a peaked kernel: a bright core, a long thin tail) | 2 | 0 |

Halation and Streak reach down fewer stops: they are for the brightest lights. A first Halation (Gaussian 0.35, add 1.5,
reaching 5 stops down like the others) turned the sparkles on a canal's water into an orange blotch at Glow 70, and a
first Streak (add 3) smeared a bright sky across the tree in front of it at Glow 60.

## Renderers

- **CPU** (`render_impl`): the glow is applied to the scene-linear image the stage cache keeps without it
  (`Plan::base_key`), so a Glow slider's drag re-runs the glow and what follows, not the white balance and noise
  reduction under it; the planes and masks key on the image with its glow (`Plan::lin_key`, which equals `base_key`
  without a glow).
- **GPU** (`lightcraft-gpu`): the glow runs on the CPU between the scene-linear stage and the planes, as Lens Blur does.
- **Windows** (a zoomed view's region): the window grows by the glow's reach (three widest sigmas, or a line's
  half-length), at most an eighth of the long edge, so the glow inside it is the whole frame's up to that distance; the
  far tail of a wider blur is cut there.
- **A native (non-Rust) frontend** draws the same steps with its own blurs (the numbers above are its specification).

## Tests

`glow::tests` (every style lights the dark beside a lamp and leaves far corners alone; Range reaches further down the
tones; Exposure brings more of the photo over the knee; Warmth colours the glow and Halation is red; a line glow runs
along its angle, −90 and 90 both vertical; Diffusion softens bright stripes and Bloom does not; the glow is the same size
at any resolution; hostile numbers and one-pixel pictures), and the slider sweep (`tests_sliders`: every control changes
the picture at both ends, more the further it goes).

## Open

- **Local glow in masks.** Lightroom's masks have a Glow slider (the style, Range, Spread and Warmth are shared by the
  photo); ours have none yet.
- **Not fitted to Lightroom.** The strengths, the knee and the sizes are chosen by eye on our renders; a fit to
  Lightroom's exports of the maintainer's own photos would need the maintainer's go-ahead (CLAUDE.md, clean-room).
- **Streak and large bright areas.** Streak spreads every bright area in its range along the line, a bright sky
  too, which then streaks over what stands in front of it (a tree against the sky at Range 50); lowering Range keeps
  it to the brightest lights. Keeping it to small lights (the bright pass less its own local mean) is not done.
- **XMP.** Glow is not written to or read from Lightroom's `crs:` XMP (its names there are unknown to us); LightCraft's
  own settings carry it.
