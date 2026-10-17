# rten-simd, patched for the generic CPU path

This is [rten-simd](https://github.com/robertknight/rten) 0.24.0 (MIT OR Apache-2.0, © Robert Knight) with one
change, marked `LightCraft patch` in `src/arch/generic.rs`. LightCraft uses it in place of the crates.io release
(root `Cargo.toml`, `[patch.crates-io]`); it is not a workspace member.

The bug: `GenericIsa`'s `narrow_saturate` (two vectors of i32 into one of i16, two of i16 into one of u8) took
the split point as half the *input* length and read the second half from `hi.0[i]`, so it panicked with "index out of
bounds: the len is 4 but the index is 4" on its first call. rten uses `GenericIsa` on CPUs without AVX2 (x86) or NEON,
such as the Celeron in many NASes, and the first int8 model that quantizes activations (`DynamicQuantizeLinear`) hit it:
the sensitive-content model failed to load on the Synology server. Test: `cargo test --manifest-path
third_party/rten-simd/Cargo.toml --lib narrowing` (this directory isn't a workspace member, so `ci` doesn't run it).

To update: copy the new release over `src/`, `Cargo.toml` and `README.md` and reapply the change (one line; `diff -u`
against the release shows it). Drop this directory when an rten-simd release has it fixed (0.27.0 is the latest as
of 2026-10-18; not checked whether it is fixed there).
