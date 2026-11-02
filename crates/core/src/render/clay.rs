// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Draws the clay mesh, and other meshes shaded like it (scanned
//! references). The pipeline is shared; each mesh owns its buffers, and
//! each [`Slot`] its camera.

use egui::Color32;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::{DEPTH_FORMAT, SAMPLES, Slot, VIEW_FORMAT};
use crate::camera::OrbitCamera;
use crate::sculpt::{Mesh, Vertex};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    base: [f32; 4],
    light: [f32; 4],
    /// x: flat, y: opacity, z: use vertex colours, w: unused.
    params: [f32; 4],
}

/// How a mesh is shaded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The base colour, multiplied by each vertex's colour if those are on.
    pub colour: Color32,
    /// Filled flat with the colour, without light: the silhouette.
    pub flat: bool,
    /// 0..1. Below 1 the mesh blends over what was drawn before it. Every
    /// surface blends, back faces too (there is no culling); a see-through
    /// mesh meant to read as one layer needs a depth pre-pass first.
    pub opacity: f32,
    /// Use the vertices' own colours, as scans have.
    pub vertex_colours: bool,
}

impl Look {
    /// Lit, opaque, in one colour.
    pub const fn lit(colour: Color32) -> Self {
        Self {
            colour,
            flat: false,
            opacity: 1.0,
            vertex_colours: false,
        }
    }
}

/// The key light, in view space: above, to the left and in front, like a
/// north window over the modelling stand.
const LIGHT: Vec3 = Vec3::new(-0.45, 0.65, 0.6);

pub struct ClayPass {
    pipeline: wgpu::RenderPipeline,
    /// Uniforms and their bind group, one per slot.
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
}

/// A mesh's GPU buffers.
pub struct GpuMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
}

impl GpuMesh {
    pub fn new(device: &wgpu::Device, mesh: &Mesh) -> Self {
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("clay vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices()),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("clay indices"),
            contents: bytemuck::cast_slice(mesh.indices()),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            vertices,
            indices,
            index_count: mesh.indices().len() as u32,
        }
    }

    /// Uploads the vertices in `range` after sculpting changed them.
    pub fn write(&self, queue: &wgpu::Queue, mesh: &Mesh, range: std::ops::Range<usize>) {
        let data: Vec<Vertex> = range.clone().map(|i| mesh.vertex(i)).collect();
        let offset = (range.start * size_of::<Vertex>()) as u64;
        queue.write_buffer(&self.vertices, offset, bytemuck::cast_slice(&data));
    }
}

impl ClayPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("clay.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clay"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let slots = Slot::ALL
            .map(|_| {
                let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("clay uniforms"),
                    size: size_of::<Uniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("clay"),
                    layout: &layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    }],
                });
                (uniforms, bind_group)
            })
            .into();
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("clay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("clay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Unorm8x4,
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: VIEW_FORMAT,
                    // The shader writes premultiplied colour: opaque draws
                    // replace, see-through ones blend.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // No culling: thin forms and the inside of open shapes stay visible.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, slots }
    }

    /// Sets the camera and look for the slot's next draws. Each slot keeps
    /// its own, so every slot can be prepared before one submit.
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        slot: Slot,
        camera: &OrbitCamera,
        aspect: f32,
        look: &Look,
    ) {
        let view: Mat4 = camera.view();
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        let uniforms = Uniforms {
            view_proj: (camera.projection(aspect) * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            base: super::linear(look.colour),
            light: LIGHT.normalize().extend(0.0).to_array(),
            params: [
                flag(look.flat),
                look.opacity.clamp(0.0, 1.0),
                flag(look.vertex_colours),
                0.0,
            ],
        };
        let (buffer, _) = &self.slots[slot.index()];
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot, mesh: &GpuMesh) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.slots[slot.index()].1, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}
