# Flash

The look of a photo taken with a xenon flash on the camera, made from the photo's **depth**: the same depth Lens Blur, Relight and Depth Map
use (the photo's own depth map, or the Depth Anything 3 estimate stored in the settings, `lens_blur.depth`; turning Flash on estimates it when
the photo has none, as Lens Blur's Apply does, and keeps it where Lens Blur keeps it). The code is `crates/pipeline/src/flash.rs`; a native
(non-Rust) frontend draws the same formulas on the GPU, so every constant here is public.

Relight cannot be tuned into this. A light placed at the lens shades by the surface and marches its shadows through the depth, so the subject
goes dark with a black halo around it, and its Brightness is a gain on what the photo already has: no exposure for the subject, no hot centre, no
colour split, no glare. Flash is a small tool of its own that reuses Relight's depth, its guided filter and its depth range.

## What a flash photo is, and what draws it

1. **Inverse-square falloff.** The depth is a nearness `n` (1 nearest, 0 farthest) that is linear in inverse distance, so a pixel's distance in
   units of the subject's is `inv_subject / inv` with `inv = 1/R + n (1 - 1/R)`; `R` (Falloff, 1.3 to 14) is how deep the scene is in flash
   distances. `E = min((inv / inv_subject)^2, 6)`.
2. **Frontal, flat light.** The flash is at the lens, so a surface turned away from it is not lit: `lambert = 0.65 nz + 0.35`, `nz` from the
   slope of the depth. A depth jump is the edge of an object, not a surface turned away, so a slope above 2 is dropped (`exp(-(slope/2)^4)`);
   shaded, it drew a dark ring around every subject. Surface sets how strongly the slope turns it. Far from the lens (`E` small) the surface
   hardly matters and its slope is noise: `nz' = 1 - (1 - nz) clamp(E, 0, 1)`.
3. **A hot centre.** `hot = exp(-Hotspot 2.2 r^2)`, `r` the distance from the middle of the frame over the half diagonal.
4. **A drop shadow** behind the subject: a ray marched toward the flash through the depth as a height field (28 steps, 14 for a draft).
   Flash Angle puts the flash around the lens, Shadow Offset how far from it (the ray rises a whole depth range in `0.12 Offset/100` long edges).
   Two things keep it honest. An occluder has to stand well clear of what it shades (`0.12` of nearness, all of it from `0.37`): a nose does not
   shadow a cheek, so a well lit face has no shadow on it. And what it falls on has to be lit by the flash at all (`E` from 0.25 to 0.7): a wall far
   behind the subject gets little of the flash, so its shadow is not seen (and the dog's grass is too far for one). That is decided at the depth's size, with the receiver's
   own nearness, so the soft edge of the subject's depth is not a rim of shadow around it.
5. **Glare.** `Gloss`: a glint is small. It shows where a surface faces the lens most (`nz'` from 0.965, squared), only near the lens (`E` over 0.55), on
   bright pixels, and mostly where the skin already shows a highlight: a pixel brighter than its surround (the log luminance against its local mean,
   2 % of the long edge, two box passes; 2 per stop, a tenth of the glint shows without one). A broad glint on every flat facing surface washed a
   face out to one white; this one follows the texture of the skin, as oil on it does. It is the flash's reflection, so it scales with Power (none at
   Power 0), and at Gloss 100 and Power 50 it adds 0.3 of light at most (it added 0.9 and blew a face out at high Gloss, review 2026-11-26).
6. **Two lights, two colours.** The flash has its temperature against the photo's white balance (Flash Temperature), the part it does not reach
   the room's: warmer (toward tungsten) or cooler (Ambient Tint).
7. **Lamps and windows light themselves.** A photo clips them, so a nearly clipped pixel keeps more of its light when the ambient is turned down
   (Keep Lights).
8. **Fill Shadows.** A flash is a fill: what the photo's own light left in shade on what the flash lights (a cheek under the hair, a neck) is lifted toward
   what is around it: the log luminance against its local mean (5 % of the long edge, two box passes), up to 1.4 stops, 0.8 of them at Fill 100, on pixels with `E`
   over 0.5. It is what makes a well lit person with no shadows on the face; it only ever lifts, and leaves the lit part alone.
9. **Contrast and noise.** A shoulder from soft to a hard clip (Blown Highlights), an S curve in display encoding (Punch), and noise where the
   flash does not reach (Noise): the dark rest of a flash photo is an underexposed frame. The noise has a luminance part and a colour part of
   its own per channel (a sensor's noise in the dark does), from a hash of the grain cell so it is the same at every size.

```text
flash = Power * E * lambert * hot * (1 - Shadow * occlusion) * flash colour
out   = colour * lift * (ambient * tint + flash) / (1 + ambient)      ambient = 2^(-6 (1 - Ambient/100)), more where the pixel is a lamp; lift: Fill Shadows (times min(Power, 1): none at Power 0)
out  += Gloss * 0.3 * Power * glint * flash colour                      glint = facing^2 * (0.1 + 0.9 * peak) * ...
```

Everything is scene-linear Rec. 2020, after Relight and before Lens Blur, so every tone and colour tool edits a flashed photo as any photo.
What the depth range (Near, Far, as Lens Blur's and Relight's) leaves out keeps the photo's own light.

### The depth, prepared

Prepared once per depth ([`Prepared`](../crates/pipeline/src/flash.rs)): smoothed as Relight smooths it, then the near part **eroded** 3 px (at a depth
960 px long: the estimate's edges are soft, and a halo of flash on the background is the one artefact people see), the **far field flattened**
(nearness under 0.35, a normalised box blur of radius 42: a sky's relief is noise and the inverse square swings the flash by a factor of two over it;
only far pixels feed the blur, or the subject's nearness smeared into a glow around it), and the slope per long edge. The planes (nearness, the light the
shadow leaves, the facing, the depth range's share) are made at the depth map's own size, sampled through the frame like the picture and **snapped to
the photo's edges** by the guided filter Lens Blur snaps its depth with.

The **subject** is the nearness the flash is exposed for: by default the 70th percentile of the middle of the frame (right for a portrait or a pet,
wrong for a scene without a subject), or a depth the person picks (`flash.subject`).

## Controls

`flash.power` (50, the subject lit as the photo was; 100 twice that), `flash.falloff` (55), `flash.ambient` (47: 6 stops under the lit part at 0, none
at 100), `flash.fill` (70), `flash.tint` (-100 to 100, 20), `flash.temp` (3200 to 10000 K, 5600), `flash.hotspot` (35), `flash.surface` (50), `flash.shadow` (50),
`flash.offset` (35), `flash.angle` (0 to 360 degrees, 35: 0 to the right, 90 above), `flash.gloss` (10), `flash.clip` (30), `flash.contrast` (25),
`flash.emit` (20), `flash.grain` (20), `flash.near` and `flash.far` (0 and 100, the depths it reaches). The defaults are a well lit subject: not blown, no shadow on
the face, none on a far background. They are develop controls, so dragging the
flash on the photo (angle and offset) is a slider drag: drafts follow it and the drag is one undo step.

## Commands

`flash.info`, `flash.apply {on?}`, `flash.preset {preset: fill|direct|night}` (every slider at once; the depth range and the subject stay),
`flash.subject {x, y}` (the depth at a normalized image point) or `{auto: true}`, `flash.reset` (the sliders back, still on, the depth kept).
The copyable group is `flash` (the picked subject is left behind: a depth of the photo it was picked on).

## Tests

`crates/pipeline/src/flash/tests.rs` (off changes nothing; the near is lit and the far dark; ambient and power; the depth range keeps the photo's own
light; lamps keep theirs; the noise has zero mean and unit variance; the far field's relief is flattened and the subject found; the direction follows
the orientation; the shadow falls away from the flash; hostile numbers), `crates/engine/src/tests_flash.rs` (no depth or model, a portrait's own depth,
a stored estimate, a preset and the sliders as undo steps, the subject pick, copies, the JSON), the slider sweeps of every new control
(`tests_sliders.rs`, prepared by `controls::prepare_to_show`).
