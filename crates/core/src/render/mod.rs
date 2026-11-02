// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! GPU side. Each 3D view renders into its own texture, which the interface
//! shows as an image. That keeps the panels simple, lets tests read pixels
//! back, and leaves room for rendering more views (stereo, later).
//!
//! Every format is non-sRGB: colours are written as they should appear, and
//! shaders do any conversion explicitly.

pub const VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// An offscreen colour target for one 3D view, registered with egui.
pub struct ViewTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: [u32; 2],
    pub egui_id: egui::TextureId,
}

impl ViewTarget {
    pub fn new(device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer, size: [u32; 2]) -> Self {
        let (texture, view) = create(device, size);
        let egui_id = renderer.register_native_texture(device, &view, wgpu::FilterMode::Nearest);
        Self {
            texture,
            view,
            size,
            egui_id,
        }
    }

    /// Recreates the texture when the panel changed size. Returns true if it did.
    pub fn fit(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        size: [u32; 2],
    ) -> bool {
        let size = [size[0].max(1), size[1].max(1)];
        if size == self.size {
            return false;
        }
        let (texture, view) = create(device, size);
        renderer.update_egui_texture_from_wgpu_texture(
            device,
            &view,
            wgpu::FilterMode::Nearest,
            self.egui_id,
        );
        self.texture = texture;
        self.view = view;
        self.size = size;
        true
    }
}

fn create(device: &wgpu::Device, size: [u32; 2]) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("view target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: VIEW_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// A colour as a wgpu clear value, without any sRGB conversion.
pub fn clear_color(c: egui::Color32) -> wgpu::Color {
    let [r, g, b, a] = c.to_array();
    wgpu::Color {
        r: f64::from(r) / 255.0,
        g: f64::from(g) / 255.0,
        b: f64::from(b) / 255.0,
        a: f64::from(a) / 255.0,
    }
}
