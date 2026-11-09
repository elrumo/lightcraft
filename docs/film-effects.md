# Film effects

> **Last updated:** 2026-11-09 · **Status:** built (core settings, commands and renderer; `ctl:film.border`, `filmLayer.<i>.*`, `cmd:film.add`); the editing UI is a native frontend's, not egui's

Film effects put a photo on film and mark it as film gets marked: a **frame** the photo sits in (a 35mm strip with its
sprocket holes, a filed-out negative carrier's black edge, an instant print's frame) and a **stack of marks** drawn over
the finished picture, bottom to top: light leaks, light streaks, a burnt-out edge, dust with hairs, scratches. All of it
is procedural (no texture files): numbers, a seed and a hash. Nothing here is fitted to or taken from another
application.

## Settings

`film` in the develop settings (`lightcraft_develop::Film`), left out of the JSON while it is the default, so settings
without film effects keep their JSON and `hash64`. The Film eye (section `film`) switches it all off; Reset Section
`film` clears it; Copy Settings, presets and sync carry it as the **Film Effects** group (`SettingsGroup::Film`), and a
copy of a photo without film effects clears them on the photo it is pasted onto.

| Setting | Values | Default | What it does |
|---|---|---|---|
| `film.frame` | `none`, `strip`, `filed`, `instant` | `none` | the frame; the photo shrinks into its window, the output keeps its size |
| `film.frame_tone` | `dark`, `light` | `dark` | strip: black slide base or amber negative base; instant: black or white frame |
| `film.border` (`ctl:film.border`) | 0..100 | 30 | filed edge and instant frame: how wide the border is |
| `film.markings` | bool | true | strip: a frame number and an arrow along the edge |
| `film.vary` | bool | true | each photo draws its own marks; off, the same marks on every photo ("Same on Every Photo") |
| `film.layers[]` | at most 8 | none | the marks, bottom to top (below) |

A layer: `{id, kind, visible, seed, amount, size, angle, offset, color, texture, tone, fibres}`. The numbers are
generic; each kind reads its own, and their sliders are indexed controls `filmLayer.<i>.<field>` (`develop.set`).
Angles are degrees counter-clockwise: 0 the right edge (a horizontal line), 90 the top, −90 the bottom.

| Kind | Fields (label) | Starting values |
|---|---|---|
| `leak` Light Leak | amount, size (Reach), angle (Direction), color, texture | 60, 45, 90, 0, 50 |
| `streak` Light Streak | amount, size (Width), angle, offset, color, texture | 50, 25, 0, 0, 0, 50 |
| `burn` Edge Burn | amount, size (Depth), angle (Edge), color, texture (Roughness) | 80, 12, −90, 20, 60 |
| `dust` Dust | amount, size, fibres (Hairs), tone (Dark ↔ Light), texture (Softness) | 40, 40, 25, 100, 30 |
| `scratches` Scratches | amount, size (Width), angle, tone, texture (Breaks) | 30, 20, 0, 100, 50 |

Commands (`crates/engine/src/cmd/film.rs`), each one undo step: `film.add {kind, seed?}` (a random seed, the kind's
starting values, on top), `film.delete {id}`, `film.visible {id, visible?}`, `film.duplicate {id}` (above the original,
its own seed), `film.move {id, to | delta}` (0 = bottom), `film.shuffle {id, seed?}`, `film.list`. The frame and the
switches are plain settings: `develop.merge {settings: {film: {frame, frame_tone, border, markings, vary}}}`.

### Per-photo variation

A layer's seed as drawn is `hash(seed, salt, 0x68e31da4)` while `vary` is on, else `seed`. `salt` is FNV-1a (32-bit)
of the photo's id as a decimal string (the id sync uses, so every device draws the same marks); a render of no
particular photo (the CLI) uses 0. So a preset or a paste that puts the same layers on many photos gives each its own
dust and leaks with no seed rewritten, and Shuffle changes `seed`.

## Method (`crates/pipeline/src/film.rs`)

The effects are drawn over the finished picture (after grain and the LUT, the last step of a render), in **linear light
of the output's primaries**, SDR white 1. Colours are linear sRGB, taken to the output's primaries once. Every position
is in units of the whole output's **long edge** from its top-left, from the pixel's centre in the whole output: `u =
(x + 0.5) / long`, so a zoom window, a preview, a thumbnail and an export draw the same marks. `pu = 1 / long` is a
pixel's size; shapes are antialiased over `0.75 pu` and a shape thinner or smaller than a pixel fades by its area, so a
small render looks like a large one scaled down.

### Noise

- `hash(x, y, seed)`: `h = seed ^ x·0x8da6b343 ^ y·0xd8163841` (wrapping, 32-bit), then `h ^= h >> 16; h *= 0x7feb352d;
  h ^= h >> 15; h *= 0x846ca68b; h ^= h >> 16`. `rand = (hash >> 8) / 2^24`; `rand_n(x, y, seed, n) = rand(x, y, seed +
  n·0x9e3779b9)`.
- `value_noise`: `rand` on the integer lattice joined by the quintic `t³(t(6t − 15) + 10)`.
- `fbm(x, y, seed, octaves, footprint)`: octave `o` at frequency `2^o`, weight `0.5^o`, seed `seed + o·0x632be5ab`, each
  faded to 0.5 by `1 − smoothstep(0.25, 1, footprint · 2^o)`, normalised by the weights.

### Light: leaks, streaks, burns

An intensity `I` becomes leaked light through film's response, `L = 1 − e^(−I·k)` per channel, with `k = (1, 0.42,
0.15)` turned by Color toward `(1, 0.18, 0.30)` (magenta red, −100) or `(1, 0.85, 0.22)` (yellow, 100): weak light reads
deep red, then orange and yellow, white where it burns. It is added as `c + L·(1 − clamp(c, 0, 1))`: a screen in SDR,
nothing added over white in an HDR render.

With `dir = (cos θ, −sin θ)`, `q` the position from the frame's centre, `ext = |dir.x|·w/2 + |dir.y|·h/2`: the depth from
the edge the light comes from is `t = ext − q·dir` and the place along that edge `s = −q.x·dir.y + q.y·dir.x`.

- **Leak**: reach `R = 0.05 + 0.6·size`; a patch centred at `(rand_n(0,0,seed,1) − 0.5)·0.9·edge length`, width `(R·(0.6
  + 0.8·rand_n(0,0,seed,2)) + 0.08)·(1 + 0.8·t/R)`, Gaussian across it; falloff `0.65·e^(−t/0.25R) + 0.35·e^(−2(t/R)²)`;
  bands `n = fbm((s + 0.6R·warp)·1.8/R, t·0.5/R, seed, 2)`, `warp = fbm(s·1.5/R, t·0.8/R, seed ^ 0x1111, 2) − 0.5`;
  `I = amount·3·falloff·patch·(1 − texture + texture·1.6·smoothstep(0.2, 0.8, n))`.
- **Streak**: normal `n = (−dir.y, dir.x)`; distance `d = q·n − offset·ext_n`; width `W = 0.01 + 0.3·size^1.5`; band
  `e^(−2(d/W)²) + 0.6·e^(−2(d/0.25W)²)`; `I = amount·2.4·band·mod(fbm(along/3W, d/W, seed, 4))`.
- **Burn**: depth `D = 0.006 + 0.22·size^1.6`; edge `D·max(1 + rough·1.6·(fbm(s/(1.5D + 0.01), 0.5, seed, 3) − 0.5), 0.15)`;
  a grainy boundary `(fbm(u/0.0015, v/0.0015, seed ^ 0x2222, 2) − 0.5)·rough·0.6D` added to `t`; inside =
  `smoothstep(edge + soft, edge − soft, t')`, `soft = max(0.75pu, 0.12D)`; glow `e^(−max(t − edge, 0)/(0.8D + 0.015))`;
  `I = amount·(5·inside + 1.2·glow·(1 − inside))`.

### Marks: dust, hairs, scratches

Marks mix the picture toward light `(0.92, 0.92, 0.90)` or dark `(0.006, 0.006, 0.006)` by their coverage; each speck,
hair or scratch is light when its own random number is under `(tone + 1)/2`.

- **Dust**: three octaves of cells — `(cell, chance at Amount 100, radius range in cells)` = `(1/10, 0.06, 0.03–0.10)`,
  `(1/28, 0.10, 0.03–0.10)`, `(1/75, 0.12, 0.04–0.12)` — each cell of the 3 × 3 around the pixel holding a speck when
  `rand_n(cell, seed_o, 0) < chance·amount`. A speck sits at `(cell + 0.2 + 0.6·r1)·size`, radius `(rmin + (rmax −
  rmin)·r3³)·cell·(0.45 + 1.4·size)`, turned by `r4·2π`, squashed by `1 + 2·r5²`, its outline `0.65 + 0.7·value_noise`
  read round a circle; softened by `texture·radius·0.8`, opacity `0.55 + 0.45·r8`.
- **Hairs**: cells of 1/6 holding a hair when `rand < 0.6·amount·fibres`: a cubic Bézier of length `(0.35 + 0.6 r3)·cell`
  with two bends, width `(0.00025 + 0.0006 r6)·(0.45 + 1.4·size)`, thinner toward its ends, distance along 16 pieces.
- **Scratches**: slots of 0.025 across the line's normal, each holding a scratch when `rand_n(j, 0, seed, 0) < 0.5·amount`;
  a slow wobble (`0.006·(value_noise(along·2.5, 7.3 j) − 0.5) + 0.0015·(value_noise(along·14, 1.9 j) − 0.5)`), width
  `(0.0002 + 0.0022·size^1.5)·(0.5 + r2)`, a faint halo, broken where `value_noise(along·3 + 17 j, 0.5)` is under
  `0.75·texture`, running a random part of the frame.

### Frames

The frame's window is a rectangle in whole output pixels (rounded half up); the picture is shrunk into it covering it,
centred, with an area-weighted resample in linear light, and the frame is drawn round it per pixel. The output keeps its
size. A windowed render with a frame works on the whole output and keeps its window.

- **35mm strip** (the public 35mm dimensions, millimetres): film 35 wide, frames 36 × 24 at a 38 mm pitch, perforations
  1.98 along × 2.79 across with 0.5 corners, their outer edge 2.01 from the film's edge, one every 4.75. The strip runs
  along the output's long edge; the film's width fills the short side unless 38 mm would not fit along the long one
  (then the scanner's light shows past the film's edges). Dark: slide base `(0.006, 0.006, 0.007)`, holes and the light
  past the film `(0.95, 0.94, 0.90)` with a faint glow into the base; light: amber negative base `(0.70, 0.27, 0.09)`.
  The base's density wanders ±6 %. Edges soften over at least 0.06 mm (a scan is never sharp). Markings: an arrow and
  the frame number (1–36, from the seeds), and the number with an A half a frame on, as 5 × 7 dots (pitch 0.17 mm) in
  the margin outside the holes on one side, orange on a slide, dark brown on a negative. The window has 0.3 mm corners.
- **Filed edge**: a black border `0.012 + 0.10·border` wide, its inner edge filed by hand (a slow fBm wander and fine
  teeth reaching into the photo by up to about a third of the border).
- **Instant**: sides and top `0.02 + 0.07·border`, the bottom 3.5 times that (the window keeps 30 % of the height),
  white `(0.80, 0.79, 0.76)` or black paper with a faint texture, and a soft shadow on the picture along its top and left.

## Renderers

- **CPU** (`render_impl`): after `finish` (8-bit, before the histogram) and `finish_deep` (16-bit and float, before the
  window is cut).
- **GPU** (`lightcraft-gpu`): drawn on the CPU over the read-back 8-bit picture. A deep render with film effects goes to
  the CPU renderer (`gpu_renders`).
- **Without the frame**: an uncropped view (the Crop tool, `apply_crop: false`) and a loupe asked with `render.view {filmFrame:
  false}` (`RenderJob::with_film_frame`: a screen that maps taps onto the photo, as Masking, Remove and Red Eye do) leave the
  frame out, so a tap lands where it shows; the marks stay.
- **A native (non-Rust) frontend** draws the same with its own kernel; the numbers above are its specification, and
  `film::tests` its reference.

## Tests

`film::tests` (each kind marks where it should and leaves the rest; dust and scratches follow their tone; hairs come with
fibres; the same marks at any size; a window is cut from the whole; seeds, salts and Same on Every Photo; the strip's
window and hole pitch, the negative's amber; filed and instant borders; hostile numbers and one-pixel pictures; 8-bit and
deep alike), `lightcraft_develop` (`default_film_is_not_serialized_so_existing_hashes_stay`,
`amount_scales_groups_the_photo_leaves_out`), and the engine's `tests/film.rs` (the commands as undo steps, Copy and
Paste carrying or clearing the group).

## Open

- No egui panel (the core renders the settings any frontend makes).
- A zoomed engine render with a frame renders the whole output; a banded export with a frame is not banded.
- The marks are static on a video clip.
