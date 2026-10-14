//! HDR pictures as textures of the window's own wgpu device: what the loupe draws when the
//! display can show the range above SDR white (LR-VIEW-HDR-DISPLAY, `lightcraft_ui_egui::hdr`).
//!
//! A picture is a Display P3 `Rgba16Float` texture holding values encoded with the sRGB curve (SDR
//! white at 1.0), registered with the window's egui renderer like any native texture. egui's
//! textures aren't sRGB-aware: its shader writes what it samples, so values above 1 reach a surface
//! that is in HDR output (`egui_wgpu::set_hdr_active`, `third_party/egui-wgpu/LIGHTCRAFT.md`) and
//! show brighter than white.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use lightcraft_ui_egui::hdr::{HdrPicture, HdrSink};

pub struct WgpuSink {
    state: RenderState,
    /// The textures made, by the id egui draws them with (a registered view keeps its texture alive,
    /// but only this map lets [`HdrSink::upload`] write into one again).
    textures: Mutex<HashMap<egui::TextureId, wgpu::Texture>>,
}

impl WgpuSink {
    pub fn new(state: RenderState) -> Self {
        WgpuSink { state, textures: Mutex::new(HashMap::new()) }
    }

    fn write(&self, texture: &wgpu::Texture, picture: &HdrPicture, size: wgpu::Extent3d) {
        self.state.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&picture.rgba16f),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width.saturating_mul(8)), rows_per_image: Some(size.height) },
            size,
        );
    }
}

impl HdrSink for WgpuSink {
    fn upload(&self, picture: &HdrPicture, reuse: Option<egui::TextureId>) -> Option<egui::TextureId> {
        let (width, height) = (u32::try_from(picture.width).ok()?, u32::try_from(picture.height).ok()?);
        let limit = self.state.device.limits().max_texture_dimension_2d;
        // (a wgpu validation error ends the process: never hand it a size or a buffer it would refuse)
        if width == 0
            || height == 0
            || width > limit
            || height > limit
            || picture.rgba16f.len() != picture.width.checked_mul(picture.height)?.checked_mul(4)?
        {
            return None;
        }
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let mut textures = self.textures.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(id) = reuse
            && let Some(texture) = textures.get(&id).filter(|t| t.width() == width && t.height() == height)
        {
            self.write(texture, picture, size);
            return Some(id);
        }
        let texture = self.state.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lightcraft-hdr-picture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.write(&texture, picture, size);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let id = self.state.renderer.write().register_native_texture(&self.state.device, &view, wgpu::FilterMode::Linear);
        textures.insert(id, texture);
        Some(id)
    }

    fn free(&self, id: egui::TextureId) {
        self.state.renderer.write().free_texture(&id);
        self.textures.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}
