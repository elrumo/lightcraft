# DNG black-level normalization

DNG and DNG-layout CFA TIFFs use the maximum computed black level of each sample plane when
scaling black-subtracted samples. This follows the public [DNG 1.7.1 specification, p. 99](https://helpx.adobe.com/content/dam/help/en/camera-raw/digital-negative/jcr_content/root/content/flex/items/position/position-par/download_section_733958301/download-1/DNG_Spec_1_7_1_0.pdf).
The computed black includes the repeat-pattern value and the horizontal and vertical deltas,
anchored at the active area. Each linear RGB sample plane has its own black and white levels.

Previously, the scale used the mean of all repeat-pattern values and ignored the deltas. For a
linear DNG with black levels 64 / 256 / 512 and white levels 1024 / 2048 / 4096, sensor white in
the red plane became 1.286 instead of 1.0. For a CFA DNG with black levels 64 / 128 / 192 / 256
and white 1024, a normalized sample of 0.995 became 0.884, below the engine's 0.99 clipping
threshold. These are synthetic regressions, not measurements of Lightroom fidelity or of a
particular camera.

`RawImage::normalization_scales` is shared by full-resolution and binned development. It finds
the maximum over the entire active area, before the default crop, in each sample plane. Row and
column delta maxima are grouped by repeat-pattern phase: simply adding three global maxima
can produce a black level that never occurs. Work is proportional to the active area's width
and height plus the repeat-pattern size, rather than its pixel count; scaling remains a
precomputed multiplication per sample. Missing delta entries contribute zero. Negative deltas
are kept. Values outside the active area do not contribute.

The source API still preserves negative shadow noise and values above white. Vendor RAW formats
keep their existing scale. This is a DNG source-decoding correction, not a change to the develop
process or to tone-slider response (`docs/process-versions.md`). Cached thumbnails are invalidated.
Existing smart previews contain already developed pixels and must be removed and rebuilt from
their originals to receive the correction; offline proxies keep their stored rendering.

Regression coverage in `crates/raw/tests/dng.rs` checks a decoded linear DNG, repeated black with
phase-dependent deltas, cropped binned previews through both the direct and opcode paths,
clipping masks, negative noise and headroom. A property test checks the optimized maximum
against exhaustive computation for varying active sizes, repeat patterns, sample-plane counts
and signed delta arrays. A vendor-format regression preserves the previous scale.

This change was implemented from the public format specification and LightCraft's source model;
it does not incorporate code or data from other RAW developers.

A non-finite or non-positive range, an overflowing reciprocal, or an empty active area falls back to a unit scale. Empty active areas still fail validation when normalizing pixels. The unit regression `normalization_scales_fall_back_for_empty_or_non_finite_ranges` covers these cases.

The render-cache version is bumped so cached thumbnails and renders from the previous normalization are not reused. Existing offline smart-preview pixels cannot be rebuilt without their original raw.
