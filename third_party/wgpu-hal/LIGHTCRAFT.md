# wgpu-hal, patched for HDR colour spaces on macOS

This is [wgpu-hal](https://github.com/gfx-rs/wgpu/tree/trunk/wgpu-hal) 30.0.1 (MIT OR Apache-2.0, © the
gfx-rs developers) with one change, marked `LightCraft patch` in `src/metal/surface.rs`. LightCraft uses
it in place of the crates.io release (root `Cargo.toml`, `[patch.crates-io]`); it is not a workspace member.

The bug: on Metal, wgpu resolves CoreGraphics' `kCGColorSpaceExtendedDisplayP3` and the two BT.2100
constants with `dlsym`, which returns the address of the *variable* holding the `CFStringRef`, and used
that address as the string. Asking for `SurfaceColorSpace::ExtendedDisplayP3` (or `Bt2100Pq` / `Hlg`)
therefore crashed inside `CGColorSpaceCreateWithName` with a bus error. (`ExtendedSrgb`,
`ExtendedSrgbLinear` and `DisplayP3` read their constants directly and are fine.)
LightCraft needs `ExtendedDisplayP3` for HDR on macOS: `third_party/egui-wgpu/LIGHTCRAFT.md`,
`docs/hdr-display.md`.

To update: copy the new release over `src/`, `Cargo.toml` and `build.rs` and reapply the change (one
function; `diff -u` against the release shows it). Drop this directory when a wgpu-hal release has it
fixed (30.0.1 is the latest as of 2026-10-14).
