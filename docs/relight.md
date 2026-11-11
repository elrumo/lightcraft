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
- **Near**, **Far** (`relight.near`, `relight.far`, depths 0 the nearest to 100 the farthest, as Lens Blur's): the depths
  the light reaches. Past each end it fades out over 6 depths (`RANGE_FEATHER`), and what lies outside keeps the photo's
  own light (Ambient applies only where the light reaches). Left out of the JSON while they span the whole scene.

The depth, smoothed by a Gaussian of 1.5 depth pixels (the stored depth is 8-bit: its steps would show as terraces),
is a height field over the stored photo in units of its long edge, the scene 0.5 long edges deep. At every pixel of the
depth map: the normal (below), a wrapped Lambert term, an inverse-square falloff, the clearance of a ray
marched toward the light through the height field (32 steps, 16 for a draft; held two depth steps above the ground; soft by
the clearance over the distance, divided by what the same ray clears over an open surface with the pixel's own slope, so a
light low over a wall or a floor does not half-shadow it: `open_clearance`, at least `OPEN_MIN`), and the highlight. The two planes (light, highlight) are sampled through the frame like the picture and **snapped to the
photo's edges** by the guided filter Lens Blur snaps its depth with (`lensblur::nearness_out`, about two depth pixels wide,
over the log luminance): a light that stops at a face stops at its outline, not a few depth pixels off it. Every output
pixel then becomes `colour × (ambient + brightness × light colour × light) + shine × highlight`, in linear Rec. 2020.

### The surface

The normal comes from the depth's slope (a wider Gaussian of the depth, `normal_sigma`, taken after the depth is
**levelled**: averaged over 2 % of its long side with only the depths within about 1.5 codes of each pixel's own, `levelled`,
`LEVEL_SCALE`, `LEVEL_RANGE`, so the flat steps of an 8-bit slope melt into it instead of banding the light, card #345,
while real edges and a face's shape stay) unless the settings keep the
**surface model's** normals (`relight.normals`, a `NormalMap`): MoGe-2 ViT-S with its normal head (Microsoft Research,
MIT licence, 35M parameters, an optional 141 MB download, `lightcraft_depth::surface`, run on rten like Depth Anything).
A relative depth has steps and no perspective, so its slope turns a face into terraces and a floor seen from above into a
wall facing the camera; the network's normals show the shape. They are estimated once (`relight.surface`, or at once by
`relight.apply` when the model is here) from the same picture the depth is estimated from, at about 1200 of the network's
14-pixel patches (the bottom of its authors' range: against 2000 the normals turn by a median 1.5° to 2° once shrunk to the
size kept, which a relit photo doesn't show, in 1.5 s instead of 2.7 s on a 4-core CPU; the estimate shows in the activity
stack), and stored over the uncropped, EXIF-oriented photo at 384 pixels on the long
side: x and y as signed bytes (z is what makes the vector one long), delta coded, deflated and base64 encoded, about 100 KB.
Where the network sees no surface (its mask: sky), the normal faces the camera. Surface tilts them as it tilts the slope's
(0 flat, 50 as estimated, 100 twice that). Shadows are still cast through the depth. Like the depth estimate, the surface is a
picture of this photo: copies, presets and Auto Sync leave it behind, and Reset Relight keeps it. The last few decoded
surfaces and smoothed depths are kept between renders, so a drag of the light redraws without decoding or smoothing again.

Removing the photo's own light before the new one lands (a light from the right filling the shadows a light from the left
cast) needs the photo's albedo apart from its shading. A fit of the photo's light as low-order spherical harmonics over the
normals was tried and left out: on real photos it fits the dark clothes and the white shirt rather than the light. That
needs an intrinsic-decomposition model.

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

## Depth in masks

A mask's **Depth Range** part (`MaskShape::DepthRange`, `masks.rs`) selects what lies between two depths (0 the nearest,
1 the farthest) with a feathered falloff, from the same depth, snapped to the photo's edges as Lens Blur's is. A mask may hold
several (added: what lies in any of them), and any part may be intersected with one (a brush or a gradient only on the
background). `mask.pickDepth {id?, component?, x, y, x2?, y2?}` sets a part's ends from the photo: a band of 0.1 around the
depth at a point, or the 5th to 95th percentile of the depths in a box (`lensblur::picked_range`). `depth.histogram {bins?}`
counts the framed photo's depths, near to far, for drawing under a range's ends (`DepthView::histogram`). A photo without
depth gets the estimate from `lensBlur.estimate`.

## Commands

`relight.info`, `relight.apply {on?}`, `relight.surface {on?}`, `relight.color {color: [r,g,b] | "#rrggbb"}`, `relight.reset`;
`depthMap.info` (with every palette's stops for the pickers), `depthMap.apply {on?}`, `depthMap.palette {palette}`,
`depthMap.colors {near?, far?}`, `depthMap.invert {on?}`, `depthMap.reset`, `depthMap.export` (above). The sliders are `relight.*` and
`depthMap.*` through `develop.set`; Copy Settings and presets have a Relight and a Depth Map group.

## Tests

`crates/pipeline/src/relight/tests.rs` (the light brightens what faces it and follows the orientation, a block casts a
shadow away from it, Brightness and Ambient, colour, Shine, the palettes, the range, Invert, Curve, Contours, Amount,
a windowed render matching the whole one, settings that are not numbers, the surface model's normals shading a flat
depth, the light stopping at the photo's own edge, an 8-bit wall lit without bands or a shadow on itself, the depth range), `crates/depth/src/surface.rs` (the network's input size, its
camera-space answer turned to face the camera, the stored size; with `LIGHTCRAFT_SURFACE_MODEL` the real model sees a ball
round), the slider sweep of every new control
(`tests_sliders.rs`, prepared by `controls::prepare_to_show`), and `crates/engine/src/tests_relight.rs` (no depth or
model, a Portrait's own depth, the estimate shared with Lens Blur, undo steps, the light's drag, copies, the export's
formats, framing and inversion, the surface model offered, kept through Reset and taken away, a Depth Range picked by a
click and by a box, the depth histogram, Auto Sync leaving the depth and the surface behind).
