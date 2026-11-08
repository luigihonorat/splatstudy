// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The silhouette difference tint, drawn on the GPU at the sculpt view's
//! own size: the clay and the reference are rendered flat as large as the
//! view, and a pass compares them pixel for pixel. Its edges are as sharp
//! as the clay's, where the small renders read back for the score, shown
//! stretched, were blocky.

use wgpu::util::DeviceExt;

use super::{VIEW_FORMAT, ViewTarget, texture};
use crate::ui::theme;

/// The comparing pass.
pub struct Difference {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    tones: wgpu::Buffer,
}

/// The tint, premultiplied, which egui shows over the sculpt view: kept
/// while the tint is on.
pub struct Canvas {
    pub tint: wgpu::Texture,
    pub tint_view: wgpu::TextureView,
}

impl Canvas {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let tint = texture(
            device,
            [size[0].max(1), size[1].max(1)],
            1,
            VIEW_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        Self {
            tint_view: tint.create_view(&Default::default()),
            tint,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        [self.tint.width(), self.tint.height()]
    }
}

/// Frees the GPU memory now in the browser, as [`ViewTarget`] does.
impl Drop for Canvas {
    fn drop(&mut self) {
        if cfg!(target_arch = "wasm32") {
            self.tint.destroy();
        }
    }
}

/// The flat renders the tint is drawn from, made for one comparison and
/// let go after its submit: multisampled, they weigh several times the
/// tint.
pub struct Masks {
    /// Each flat render in turn: the clay's, then the reference's.
    pub mask: ViewTarget,
    /// The clay's flat render, copied out of `mask`.
    clay: wgpu::Texture,
}

impl Masks {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let mask = ViewTarget::new(device, size);
        let clay = texture(
            device,
            mask.size,
            1,
            VIEW_FORMAT,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        Self { mask, clay }
    }

    /// Copies the clay's flat render, now in `mask`, aside. Record it
    /// between the clay's pass and the reference's.
    pub fn keep_clay(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_texture(
            self.mask.resolved.as_image_copy(),
            self.clay.as_image_copy(),
            self.clay.size(),
        );
    }
}

impl Drop for Masks {
    fn drop(&mut self) {
        if cfg!(target_arch = "wasm32") {
            self.clay.destroy();
        }
    }
}

/// The tones as the shader takes them.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Tones {
    over: [f32; 4],
    under: [f32; 4],
    ink: f32,
    _pad: [f32; 3],
}

fn unit(c: egui::Color32) -> [f32; 4] {
    c.to_array().map(|v| f32::from(v) / 255.0)
}

impl Difference {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("difference.wgsl"));
        let tones = Tones {
            over: unit(theme::CLAY_OVER.gamma_multiply(0.6)),
            under: unit(theme::CLAY_UNDER.gamma_multiply(0.6)),
            ink: f32::from(theme::SILHOUETTE_INK.r()) / 255.0,
            _pad: [0.0; 3],
        };
        let tones = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("difference tones"),
            contents: bytemuck::bytes_of(&tones),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mask = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("difference"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                mask(1),
                mask(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("difference"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("difference"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: VIEW_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            tones,
        }
    }

    /// Draws the tint from the clay's flat render (kept aside) and the
    /// reference's (in the mask). Record it after the reference's pass.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        masks: &Masks,
        canvas: &Canvas,
    ) {
        let clay = masks.clay.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("difference"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.tones.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&clay),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&masks.mask.resolved_view),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("difference"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &canvas.tint_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
