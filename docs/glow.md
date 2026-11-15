# Glow

> **Last updated:** 2026-11-15 · **Status:** built (Effects panel, under Grain; `ctl:glow.*`), fitted to Lightroom at
> Glow 100, Range 50, Spread 50 (card #444)

Glow is light spread from a photo's bright parts, as a lens, a diffusion filter or film scatters it. Lightroom added
it to the Effects panel in 2026 (Glow, a Style menu with Bloom, Diffusion and Halation on the desktop and also Motion
and Streak on mobile, and Range, Spread and Warmth). This is our own model of the same controls. Its constants are
fitted to Lightroom mobile's renders of one of the maintainer's own photos (maintainer decision, 2026-11-14, card #444:
rendered output only, screenshots of each style at Glow 100 next to the photo with no edits; no Adobe file was read).

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

The glow is added in scene-linear light (linear Rec. 2020) right after noise reduction, Depth Map, Relight and Lens
Blur, before the local tone and the tone map, so Highlights and Shadows see it. It runs on the un-exposed image (the
scene-linear stage's), with Exposure's gain folded into the bright pass's knee and the screen.

**White.** The fit was made on a JPEG, whose light is the display's. A raw's scene light is not: its camera curve, after
this stage, lifts the highlights (on the corpus EOS R6 raw nothing reached scene 0.57 while its display reached 0.86), so
measured against 1 a raw's brightest parts sat below Bloom's knee and nothing glowed. The glow measures light against the photo's white instead
(`glow::source_white`): the scene light its base tone (the camera curve, or the default scene tone of a raw without one,
at neutral Contrast, Whites and Blacks) shows at display 0.9, over 0.9, found by halving 24 stops; 1 for a
display-referred source. Exposure's gain is divided by it. It is fixed per photo, so dragging a tone slider keeps the
glow (and the picture the native frontend keeps before the local tone).

1. **Bright pass.** Each pixel's light `c` is weighted by `w = smoothstep(k0, k0 + width, log2(Y(c) · 2^Exposure))`,
   `Y` its luminance, `k0 = start · 2^(2 · Range/100 − 1)` stops below SDR white (`start`, the knee at Range 50, and
   `width` by style, below: Range 0 halves the depth, so the white still glows, and Range 100 doubles it). Halation keeps
   the luminance alone (`Y · w` in every channel); the other styles keep the light's colour (`c · w`). Light past
   (exposed) white counts as white, its colour kept (`c / (Y · 2^Exposure)`): the fit was made on a JPEG, whose lamp
   clips at white, and a raw's lights reach past it by as much as its decoder keeps (Apple's development of an iPhone
   ProRAW keeps far more than this engine's, which made the native frontend's glow ten times this one's there).
2. **Spread.** The bright light is blurred, sizes in multiples of a base sigma
   `σ = 0.003 · 2^(5.5 · Spread/100)` of the long edge (0.3 % at Spread 0, 2 % at 50, 13.6 % at 100), so a preview and
   an export look alike. Gaussians are `lightcraft_raster::blur::gaussian` (three boxes); a line style's Gaussians run
   along the angle (the image sheared so the line is a row, each row blurred by the same three boxes with running sums,
   sheared back: linear in the pixels whatever the length; a mostly vertical line is transposed first), each sigma at
   most the long edge. Streak's are centred on each light; Motion's are moved `trail` of their sigmas along the angle,
   so a light's trail runs one way, ahead of it (to the right at 0°, up at 90°, down at −90°). Core Image's motion blur,
   which the native frontend uses for them, is a Gaussian along the line whose sigma is its radius (measured: 0.61 of the
   centre one radius out, 0.32 at one and a half), so the two renderers draw the same kernel.
3. **Colour and screen.** `G = Glow · add · tint · spread`, screened over each channel in exposed light:
   `out = c + min(G, 1) · (1 − c)` (the room below white clamped to 0..1), so a dark wall beside a lamp is lifted
   most, nothing is lifted past white and a light already over white is left alone. A large glow over a dark subject
   ghosts it, as Lightroom's Motion and Streak do. The tint is the Warmth colour scaled to luminance 1, in linear sRGB
   before it is taken to the working space: `(1 + 0.45 w, 1 + 0.05 w, 1 − 0.55 w)` for a filter over the light's own
   colour, or for Halation the halo's own colour, `(1, 0.32, 0.14)` (w = −1) to `(1, 0.06, 0.08)` (w = 1).

### Styles

Sizes are in base sigmas (2 % of the long edge at Spread 50). Fitted at Glow 100, Range 50, Spread 50 by least squares
on the sRGB-encoded pixels of the whole photo (Bloom with the lamp's surroundings weighted four times: see Open).

| Style | Knee start at Range 50 (stops below white) | Knee width | Spread (Gaussians, sigma × σ: weight) | add |
|---|---:|---:|---|---:|
| Bloom | −0.81 | 1 | 0.87: 0.92, 8.4: 0.08 (a halo round the light and a faint wide wash) | 5.85 |
| Diffusion | −1.85 | 2 | 0.084: 0.86, 0.39: 0.14 (a fine haze over the mid-bright tones) | 0.89 |
| Halation | −0.45 | 1 | 0.39: 0.97, 10.9: 0.03, luminance only (a tight red rim) | 6.65 |
| Motion | −2.76 | 4.25 | along the line, 0.42: 0.278, 1.62: 0.43, 4.25: 0.292, each moved 1.68 of its sigmas (trail) | 2.58 |
| Streak | −2.93 | 4 | along the line, 0.72: 0.085, 2.25: 0.234, 5.41: 0.681 | 3.56 |

What the fit showed of Lightroom (the photo less Lightroom's render, as a share of the room below white, along the
lamp's row): Bloom is a halo about 1.7 % of the long edge round the brightest lights (0.77 five pixels from the lamp,
0.2 at twenty, nothing past fifty at 1206 pixels wide); Motion is a comet tail to the right only, falling off over about
15 % of the long edge; Streak the same tail both ways and a lift of the whole frame; Halation a red rim a few pixels
wide; Diffusion a fine haze. Motion and Streak reach far down the tones (most of the room lights up), Bloom and Halation
only the brightest. The first model (2026-11-09, chosen by eye) added the glow instead of screening it and started every
knee 2.5 to 4 stops lower, which turned Bloom and Streak at Glow 100 into a white-out.

## Renderers

- **CPU** (`render_impl`): the glow is applied to the scene-linear image the stage cache keeps without it
  (`Plan::base_key`), so a Glow slider's drag re-runs the glow and what follows, not the white balance and noise
  reduction under it; the planes and masks key on the image with its glow (`Plan::lin_key`, which equals `base_key`
  without a glow).
- **GPU** (`lightcraft-gpu`): the glow runs on the CPU between the scene-linear stage and the planes, as Lens Blur does.
- **Windows** (a zoomed view's region): the window grows by the glow's reach (three widest sigmas, plus Motion's
  trail), at most an eighth of the long edge, so the glow inside it is the whole frame's up to that distance; the
  far tail of a wider blur is cut there.
- **A native (non-Rust) frontend** draws the same steps with its own blurs (the numbers above are its specification).

## Tests

`glow::tests` (every style lights the dark beside a lamp and leaves far corners alone; Range reaches further down the
tones; Exposure brings more of the photo over the knee; Warmth colours the glow and Halation is red; a line glow runs
along its angle, −90 and 90 both vertical; the screen lifts a dark wall more than a light one and nothing past white;
Motion trails one way and Streak both; the glow is the same size at any resolution; hostile numbers and one-pixel
pictures), and the slider sweep (`tests_sliders`: every control changes the picture at both ends, more the further it
goes). The native frontend's parity harness has a case per style at Glow 100 (`glow full ...`).

## Open

- **Local glow in masks.** Lightroom's masks have a Glow slider (the style, Range, Spread and Warmth are shared by the
  photo); ours have none yet.
- **One photo, one setting.** The fit is of one indoor photo (a lamp in a white room) at Range 50 and Spread 50; how
  Range and Spread move the glow away from 50 is still ours (Range scales the knee's depth, Spread the sizes by
  `2^(5.5 · Spread/100)`). Fits at other values and on other photos (a night street, a sunset) would pin them.
- **Bloom's colour and HDR.** The reference was an SDR export of an HDR photo, where the lamp clips to white like the
  ceiling round it. Lightroom's Bloom, drawn from the HDR original, puts more light round the lamp than over the
  ceiling and colours it yellow (the lamp's own colour, 0.72 blue to green 10 to 30 pixels out); the fit weights the
  lamp's surroundings to keep the halo on the light. Bloom keeps the light's own colour; no style tint was added.
- **Motion's way.** Angle runs −90..90°, so Motion trails right, up or down, never left.
- **Streak and large bright areas.** As in Lightroom, Streak and Motion spread every bright area in their range along
  the line, a bright sky too, which streaks over what stands in front of it; lowering Range keeps it to the brightest
  lights.
- **XMP.** Glow is not written to or read from Lightroom's `crs:` XMP (its names there are unknown to us); LightCraft's
  own settings carry it.
