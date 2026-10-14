# egui-wgpu, patched for HDR output

This is [egui-wgpu](https://github.com/emilk/egui/tree/main/crates/egui-wgpu) 0.36.2 (MIT OR
Apache-2.0, © Emil Ernerfeldt and the egui contributors) with three small changes, each marked
`LightCraft patch` in the source. LightCraft uses it in place of the crates.io release (root
`Cargo.toml`, `[patch.crates-io]`); it is not a workspace member and is not built by `cargo xtask ci`
on its own.

Why: eframe 0.36 picks the window surface's format itself, 8-bit only (`preferred_framebuffer_format`),
so no photo can be brighter than SDR white. With a 16-bit float surface in the extended Display P3
colour space, macOS draws values above 1.0 as brighter than white (EDR) on a display that has headroom.

| Change | Where |
|--------|-------|
| `SurfaceConfig::hdr_surface`: use `Rgba16Float` when the surface offers it (read once, at start) | `lib.rs` |
| `set_hdr_active(bool)`: present it as extended Display P3 (HDR) or plain sRGB, at runtime, from any thread (a process-wide flag: eframe gives the app a *copy* of `RenderState`, so a `SurfaceConfig` field edited through `Frame::set_wgpu_surface_config` never reaches the painter) | `lib.rs`, `winit.rs` |
| no dithering on a float surface | `renderer.rs` |

egui's shader writes gamma-encoded colours, which is what extended Display P3 is (same curve), so the UI
looks the same (the window was always drawn in the screen's own values) and an HDR picture is a Display
P3 `Rgba16Float` texture holding the same encoding (SDR white = 1.0, highlights above it): egui's textures
aren't sRGB-aware, so what it samples is what it writes. See `docs/hdr-display.md`.

To update: copy the new release over `src/` and `Cargo.toml`, reapply the three changes (`git log --
third_party/egui-wgpu` has the original diff), and bump the version in the root `[patch.crates-io]` if
eframe moved on. Drop this directory when egui-wgpu can configure its surface's format and colour space.
