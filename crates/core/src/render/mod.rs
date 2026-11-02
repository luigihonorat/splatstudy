// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! GPU side. Each 3D view renders into its own texture, which the interface
//! shows as an image. That keeps the panels simple, lets tests read pixels
//! back, and leaves room for rendering more views (stereo, later).
//!
//! Every format is non-sRGB: colours are written as they should appear, and
//! shaders do any conversion explicitly.

pub mod clay;
pub mod readback;

pub const VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

/// Where a draw goes. Each slot has its own camera uniforms, so draws seen
/// from different cameras can share one submit: the two views, and the two
/// halves of an exported frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    SculptView,
    ReferenceView,
    ExportSculpt,
    ExportReference,
}

impl Slot {
    pub const COUNT: usize = 4;
    pub const ALL: [Slot; Slot::COUNT] = [
        Slot::SculptView,
        Slot::ReferenceView,
        Slot::ExportSculpt,
        Slot::ExportReference,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// The render targets of one 3D view: multisampled colour and depth, and the
/// resolved texture that egui shows.
pub struct ViewTarget {
    pub resolved: wgpu::Texture,
    pub resolved_view: wgpu::TextureView,
    pub msaa_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub size: [u32; 2],
    pub egui_id: Option<egui::TextureId>,
}

impl ViewTarget {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let resolved = texture(
            device,
            size,
            1,
            VIEW_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = texture(
            device,
            size,
            SAMPLES,
            VIEW_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let depth = texture(
            device,
            size,
            SAMPLES,
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        Self {
            resolved_view: resolved.create_view(&Default::default()),
            resolved,
            msaa_view: msaa.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            size,
            egui_id: None,
        }
    }

    /// Shows this target in egui, registering it the first time.
    pub fn register(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
    ) -> egui::TextureId {
        let filter = wgpu::FilterMode::Nearest;
        match self.egui_id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &self.resolved_view,
                    filter,
                    id,
                );
                id
            }
            None => {
                let id = renderer.register_native_texture(device, &self.resolved_view, filter);
                self.egui_id = Some(id);
                id
            }
        }
    }

    /// Recreates the textures when the panel changed size. Returns true if it did.
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
        let id = self.egui_id;
        *self = ViewTarget::new(device, size);
        self.egui_id = id;
        self.register(device, renderer);
        true
    }

    pub fn aspect(&self) -> f32 {
        self.size[0] as f32 / self.size[1] as f32
    }

    /// A render pass that clears to `background` and resolves into the shown texture.
    pub fn begin_pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        background: egui::Color32,
    ) -> wgpu::RenderPass<'e> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("view"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.msaa_view,
                depth_slice: None,
                resolve_target: Some(&self.resolved_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color(background)),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        })
    }
}

fn texture(
    device: &wgpu::Device,
    size: [u32; 2],
    samples: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("view target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
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

/// An sRGB-encoded colour as linear RGBA for shader uniforms.
pub fn linear(c: egui::Color32) -> [f32; 4] {
    let f = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c.r()), f(c.g()), f(c.b()), f32::from(c.a()) / 255.0]
}
