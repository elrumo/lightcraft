# candle-core, patched for the Metal random seed buffer

This is [candle-core](https://github.com/huggingface/candle) 0.9.2 (MIT OR Apache-2.0, © Hugging Face) with one
change, marked `LightCraft patch` in `src/metal_backend/mod.rs`. LightCraft uses it in place of the crates.io release
(root `Cargo.toml`, `[patch.crates-io]`); it is not a workspace member.

The bug: `MetalDevice::new` allocates the RNG seed buffer with 4 bytes, but the `rand_uniform` / `rand_normal`
kernels bind it as a `seed_buffer` of two `atomic_uint`s (8 bytes). With Metal API Validation on (Xcode's default for
a Run) the first `randn_like` aborts: "argument sb[0] ... has space for 4 bytes, but argument has a length(8)". It
killed Generative Expand (`DiagonalGaussianDistribution::sample` in the Stable Diffusion VAE encode). Without
validation the GPU reads and writes 4 bytes past the buffer, inside its page, so it went unnoticed.

Test: `MTL_DEBUG_LAYER=1 cargo test -p lightcraft-diffuse --test metal_randn` (aborts without the patch).

To update: copy the new release over this directory and reapply the change (one line; `diff -u` against the release
shows it). Drop this directory when a candle-core release has it fixed (not checked beyond 0.9.2).
