# Relight and Depth Map

Two tools that read the photo's **depth** as a surface: the same depth Lens Blur uses ([lens-blur.md](lens-blur.md)),
the photo's own depth map (iPhone Portrait HEIC / JPEG, DNG 1.5) or the Depth Anything 3 estimate, stored once in the
settings (`lens_blur.depth`). Turning either on estimates the depth when the photo has none, exactly as Lens Blur's
Apply does, and keeps it where Lens Blur keeps it: the three tools share it, and copies leave it behind.

Both are drawn by `crates/pipeline/src/relight.rs` on the white-balanced, retouched and denoised scene-linear picture,
after noise reduction: Depth Map first, then Relight, then Lens Blur. The tone, colour and effects tools come after them,
so they edit a relit photo or a depth map as they edit any photo. The native Apple renderer of the LightBites app draws
the same formulas (its `RelightKernel` and `DepthLookKernel`); every constant is public in `relight.rs` for it.

## Relight

A point light added to the scene (`relight`, a settings object left out of the JSON while it is the default).

- **Place**: `relight.x`, `relight.y` in normalized image coordinates, as masks and spots use (they may run past the
  frame, -0.5 to 1.5, for a light off to one side). They are develop controls, so dragging the light on the photo is a
  slider drag: drafts follow it and the drag is one undo step.
- **Distance** (0–100): the light's height over the back of the scene, in scene depths. 0 is the back of the scene, 50
  its nearest part, then out in front of it, quadratically, to five scene depths at 100 (`light_height`).
- **Brightness** (gain up to ×4), **Spread** (the falloff's reach, 0.15 to 3 long edges, exponential), **Softness**
  (wraps the light past a surface's edge and widens the shadows' penumbra), **Cast Shadows** (how dark the cast shadows
  are), **Shine** (a Blinn-Phong highlight, exponent 32), **Surface** (how strongly the depth's slope turns the
  surface: 0 flat, 50 as the depth says, 100 twice that), **Ambient** (how much of the photo's own light is kept), and
  the light's **colour** (`relight.color`, display sRGB; normalized by its largest channel, so a coloured gel takes
  light away).

The depth, smoothed by a Gaussian of 1.5 depth pixels (the stored depth is 8-bit: its steps would show as terraces),
is a height field over the stored photo in units of its long edge, the scene 0.5 long edges deep. At every pixel of the
depth map: the normal from the depth's slope, a wrapped Lambert term, an inverse-square falloff, the clearance of a ray
marched toward the light through the height field (32 steps, 16 for a draft; soft by the clearance over the distance),
and the highlight. The two planes (light, highlight) are sampled through the frame like the picture, and every output
pixel becomes `colour × (ambient + brightness × light colour × light) + shine × highlight`, in linear Rec. 2020.

## Depth Map

The photo drawn as its depth (`depth_map`, also left out while it is the default): each output pixel's depth (0 the
nearest, 100 the farthest) placed between **Near** and **Far**, inverted if asked, bent by **Curve** (an exponent of
2^(−curve / 50)), looked up in a **palette** (Gray, Thermal, Ocean, Spectrum, Sunset: original colour stops, or Custom
with two colours of one's own), darkened by **Contours** (thin lines along equal depths, their width a fraction of the
long edge found from the depth's slope, none on a flat), and mixed over the photo by **Amount**. A palette name from a
newer build reads as Gray rather than making the settings unreadable.

## Export as data

`depthMap.export {path, id?, format?: png | tiff, invert?, long?, crop?}` writes the photo's depth for other programs (3D,
compositing, displacement, a parallax effect): relative inverse depth (nearness, 0 the farthest part of the photo, 1 the
nearest; white near unless `invert`), smoothed as Depth Map draws it, one value per pixel of the photo framed as an export of
it is (its crop, straighten and flips unless `crop` is false; its orientation and geometry always), at the original's size or
`long` pixels along the long side, so it lines up with an export of the photo. PNG holds it in 16 bits, TIFF as 32-bit
floats. It needs no look turned on, only depth (`relight::depth_out`).

## Commands

`relight.info`, `relight.apply {on?}`, `relight.color {color: [r,g,b] | "#rrggbb"}`, `relight.reset`;
`depthMap.info` (with every palette's stops for the pickers), `depthMap.apply {on?}`, `depthMap.palette {palette}`,
`depthMap.colors {near?, far?}`, `depthMap.invert {on?}`, `depthMap.reset`, `depthMap.export` (above). The sliders are `relight.*` and
`depthMap.*` through `develop.set`; Copy Settings and presets have a Relight and a Depth Map group.

## Tests

`crates/pipeline/src/relight/tests.rs` (the light brightens what faces it and follows the orientation, a block casts a
shadow away from it, Brightness and Ambient, colour, Shine, the palettes, the range, Invert, Curve, Contours, Amount,
a windowed render matching the whole one, settings that are not numbers), the slider sweep of every new control
(`tests_sliders.rs`, prepared by `controls::prepare_to_show`), and `crates/engine/src/tests_relight.rs` (no depth or
model, a Portrait's own depth, the estimate shared with Lens Blur, undo steps, the light's drag, copies, the export's
formats, framing and inversion).
