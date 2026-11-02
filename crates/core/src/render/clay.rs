// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Draws the clay mesh, and other meshes shaded like it (scanned
//! references). The pipelines are shared; each mesh owns its buffers, and
//! each [`Slot`] its camera.
//!
//! A slot has two sets of uniforms: one for ordinary draws and one for the
//! ghost, a see-through mesh laid over what the slot already shows (the
//! reference over the sculpt). Both can be prepared for one submit.

use egui::Color32;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::{DEPTH_FORMAT, SAMPLES, Slot, VIEW_FORMAT};
use crate::camera::OrbitCamera;
use crate::sculpt::{Mesh, Vertex};
use crate::ui::theme;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    base: [f32; 4],
    /// xyz: towards the key light, view space; w: how far the key light
    /// wraps past the terminator.
    light: [f32; 4],
    /// x: flat, y: opacity, z: use vertex colours, w: unused.
    params: [f32; 4],
    /// The strengths of the key light, the sky fill, the rim and the
    /// highlight.
    rig: [f32; 4],
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
    /// mesh meant to read as one layer is drawn with
    /// [`ClayPass::draw_ghost`], which keeps only its nearest surface.
    pub opacity: f32,
    /// Use the vertices' own colours, as scans have.
    pub vertex_colours: bool,
    /// Where the light comes from. Flat looks ignore it.
    pub lighting: Lighting,
}

impl Look {
    /// Lit, opaque, in one colour.
    pub const fn lit(colour: Color32) -> Self {
        Self {
            colour,
            flat: false,
            opacity: 1.0,
            vertex_colours: false,
            lighting: Lighting::Studio,
        }
    }

    /// The same look, lit by `lighting`.
    pub const fn with_lighting(self, lighting: Lighting) -> Self {
        Self { lighting, ..self }
    }

    /// Filled flat with one colour, opaque: a silhouette.
    pub const fn flat(colour: Color32) -> Self {
        Self {
            flat: true,
            ..Self::lit(colour)
        }
    }
}

/// Where the light comes from, for the clay and for mesh references alike,
/// so both views read the same. It stays put in view space while the
/// sculpt turns. Each draw's [`Look`] carries its own, so an export keeps
/// the one it started with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lighting {
    /// A soft key light above, to the left and in front, like a north
    /// window over the modelling stand.
    #[default]
    Studio,
    /// Low from the side, grazing the surface: small bumps and hollows
    /// cast their light and shade.
    Raking,
    /// From above, like a skylight: the top planes light up and the
    /// sides fall away, so the turns between planes read clearly.
    Top,
}

/// One lighting's numbers.
struct Rig {
    /// Towards the key light, view space.
    direction: Vec3,
    /// How far the key light wraps past the terminator: 0 is a hard edge.
    wrap: f32,
    key: f32,
    sky: f32,
    rim: f32,
    highlight: f32,
}

impl Lighting {
    pub const ALL: [Lighting; 3] = [Lighting::Studio, Lighting::Raking, Lighting::Top];

    /// The next one, round again after the last.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&l| l == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    fn rig(self) -> Rig {
        match self {
            Lighting::Studio => Rig {
                direction: Vec3::new(-0.45, 0.65, 0.6),
                wrap: 0.35,
                key: 0.78,
                sky: 0.30,
                rim: 0.10,
                highlight: 0.05,
            },
            Lighting::Raking => Rig {
                direction: Vec3::new(-0.95, 0.12, 0.28),
                wrap: 0.0,
                key: 1.05,
                sky: 0.12,
                rim: 0.04,
                highlight: 0.04,
            },
            Lighting::Top => Rig {
                direction: Vec3::new(0.0, 1.0, 0.2),
                wrap: 0.15,
                key: 0.95,
                sky: 0.12,
                rim: 0.04,
                highlight: 0.05,
            },
        }
    }
}

/// How two silhouettes overlap: flat renders of the clay and of the
/// reference, the same size and from the same camera, as RGBA8 pixels
/// filled with [`theme::SILHOUETTE_INK`] on [`theme::VIEW_BG`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overlap {
    /// Pixels inside both outlines.
    pub both: u32,
    /// Inside the clay's alone: where the clay is too big.
    pub clay_only: u32,
    /// Inside the reference's alone: where the clay is too small.
    pub reference_only: u32,
}

/// A pixel of a flat render is inside the outline: at least half inked,
/// which splits the multisampled edge down the middle.
pub fn inked(pixel: &[u8]) -> bool {
    pixel[0] >= theme::SILHOUETTE_INK.r() / 2
}

impl Overlap {
    pub fn of(clay: &[u8], reference: &[u8]) -> Self {
        let mut overlap = Self::default();
        for (c, r) in clay
            .as_chunks::<4>()
            .0
            .iter()
            .zip(reference.as_chunks::<4>().0)
        {
            match (inked(c), inked(r)) {
                (true, true) => overlap.both += 1,
                (true, false) => overlap.clay_only += 1,
                (false, true) => overlap.reference_only += 1,
                (false, false) => {}
            }
        }
        overlap
    }

    /// Intersection over union, 0..1: 1 when the outlines match exactly.
    /// `None` when both are empty.
    pub fn iou(&self) -> Option<f32> {
        let union = self.both + self.clay_only + self.reference_only;
        (union > 0).then(|| self.both as f32 / union as f32)
    }
}

/// Which set of a slot's uniforms a draw reads.
#[derive(Clone, Copy)]
enum Layer {
    Main,
    Ghost,
}

pub struct ClayPass {
    pipeline: wgpu::RenderPipeline,
    /// The ghost's three steps: push the depth back to the far plane where
    /// the ghost covers, keep its nearest surface's depth, then blend that
    /// surface alone over the colour already drawn.
    ghost_clear: wgpu::RenderPipeline,
    ghost_depth: wgpu::RenderPipeline,
    ghost_colour: wgpu::RenderPipeline,
    /// Uniforms and their bind group, per slot and layer.
    slots: Vec<[(wgpu::Buffer, wgpu::BindGroup); 2]>,
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

/// What differs between the clay pipelines.
struct Variant {
    label: &'static str,
    fragment: &'static str,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    /// Writes and blends colour; otherwise only depth.
    colour: bool,
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
        let uniforms = || {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
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
                    resource: buffer.as_entire_binding(),
                }],
            });
            (buffer, bind_group)
        };
        let slots = Slot::ALL.map(|_| [uniforms(), uniforms()]).into();
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("clay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |v: Variant| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(v.label),
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
                    entry_point: Some(v.fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: VIEW_FORMAT,
                        // The shader writes premultiplied colour: opaque
                        // draws replace, see-through ones blend.
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: if v.colour {
                            wgpu::ColorWrites::ALL
                        } else {
                            wgpu::ColorWrites::empty()
                        },
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
                    depth_write_enabled: Some(v.depth_write),
                    depth_compare: Some(v.depth_compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLES,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction::{Always, Less, LessEqual};
        Self {
            pipeline: pipeline(Variant {
                label: "clay",
                fragment: "fs",
                depth_write: true,
                depth_compare: Less,
                colour: true,
            }),
            ghost_clear: pipeline(Variant {
                label: "ghost clear",
                fragment: "fs_far",
                depth_write: true,
                depth_compare: Always,
                colour: false,
            }),
            ghost_depth: pipeline(Variant {
                label: "ghost depth",
                fragment: "fs_blank",
                depth_write: true,
                depth_compare: Less,
                colour: false,
            }),
            ghost_colour: pipeline(Variant {
                label: "ghost",
                fragment: "fs",
                depth_write: false,
                depth_compare: LessEqual,
                colour: true,
            }),
            slots,
        }
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
        self.write(queue, slot, Layer::Main, camera, aspect, look);
    }

    /// Like [`prepare`](Self::prepare), for the slot's ghost draws. Kept
    /// apart, so the slot's own draws and its ghost can share one submit.
    pub fn prepare_ghost(
        &self,
        queue: &wgpu::Queue,
        slot: Slot,
        camera: &OrbitCamera,
        aspect: f32,
        look: &Look,
    ) {
        self.write(queue, slot, Layer::Ghost, camera, aspect, look);
    }

    fn write(
        &self,
        queue: &wgpu::Queue,
        slot: Slot,
        layer: Layer,
        camera: &OrbitCamera,
        aspect: f32,
        look: &Look,
    ) {
        let view: Mat4 = camera.view();
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        let rig = look.lighting.rig();
        let uniforms = Uniforms {
            view_proj: (camera.projection(aspect) * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            base: super::linear(look.colour),
            light: rig.direction.normalize().extend(rig.wrap).to_array(),
            params: [
                flag(look.flat),
                look.opacity.clamp(0.0, 1.0),
                flag(look.vertex_colours),
                0.0,
            ],
            rig: [rig.key, rig.sky, rig.rim, rig.highlight],
        };
        let (buffer, _) = &self.slots[slot.index()][layer as usize];
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot, mesh: &GpuMesh) {
        pass.set_pipeline(&self.pipeline);
        self.draw_mesh(pass, slot, Layer::Main, mesh);
    }

    /// Lays `mesh` over everything the pass has drawn so far, as one
    /// see-through layer with the look given to
    /// [`prepare_ghost`](Self::prepare_ghost): only its nearest surface
    /// shows, and nothing drawn before hides it. It leaves the ghost's depth
    /// in the depth buffer, so draw it last.
    pub fn draw_ghost(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot, mesh: &GpuMesh) {
        for pipeline in [&self.ghost_clear, &self.ghost_depth, &self.ghost_colour] {
            pass.set_pipeline(pipeline);
            self.draw_mesh(pass, slot, Layer::Ghost, mesh);
        }
    }

    fn draw_mesh(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot, layer: Layer, mesh: &GpuMesh) {
        pass.set_bind_group(0, &self.slots[slot.index()][layer as usize].1, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}
