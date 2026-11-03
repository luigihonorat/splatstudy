// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Draws the clay mesh, and other meshes shaded like it (scanned
//! references). The pipelines are shared; each mesh owns its buffers, and
//! each [`Slot`] its camera.
//!
//! A slot has two sets of uniforms: one for ordinary draws and one for the
//! ghost, a see-through mesh laid over what the slot already shows (the
//! reference over the sculpt). Both can be prepared for one submit.
//!
//! The wireframe is a second pass of the same triangles over the clay,
//! without vertex buffers: its vertex shader reads each triangle's corners
//! from the mesh's own vertex and index buffers (bound as storage too), so
//! a corner knows which of the three it is, and the fragment shader draws
//! the edges from the barycentric coordinates, about a pixel wide whatever
//! line width the GPU allows (WebGPU has only 1, and no line polygon
//! mode). A code per triangle hides the diagonal of each quad. Nothing is
//! copied: strokes update the vertices the wireframe reads, and its one
//! buffer of its own, two bits per triangle, is made the first time a mesh
//! is drawn with it.

use std::sync::OnceLock;

use egui::Color32;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::{DEPTH_FORMAT, SAMPLES, Slot, VIEW_FORMAT};
use crate::camera::OrbitCamera;
use crate::sculpt::mesh::NO_DIAGONAL;
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
    /// x: added to the wireframe's clip depth (see [`PULL`]).
    pull: [f32; 4],
    /// The wireframe's colour, linear RGB, and its opacity.
    wire: [f32; 4],
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

/// A pipeline in two halves: back faces, then front faces (see
/// [`ClayPass::draw`]).
type Sides = [wgpu::RenderPipeline; 2];

pub struct ClayPass {
    pipeline: Sides,
    /// The ghost's three steps: push the depth back to the far plane where
    /// the ghost covers, keep its nearest surface's depth, then blend that
    /// surface alone over the colour already drawn.
    ghost_clear: Sides,
    ghost_depth: Sides,
    ghost_colour: Sides,
    /// The wireframe over the clay, and its layout for a mesh's buffers.
    wire: wgpu::RenderPipeline,
    wire_layout: wgpu::BindGroupLayout,
    /// Uniforms and their bind group, per slot and layer.
    slots: Vec<[(wgpu::Buffer, wgpu::BindGroup); 2]>,
}

/// A mesh's GPU buffers.
pub struct GpuMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
    /// What the wireframe reads, made when it is first drawn: the quads'
    /// diagonal codes, and the bindings of them and both buffers above.
    pub wire: OnceLock<(wgpu::Buffer, wgpu::BindGroup)>,
}

/// Vertices, and indices, per write when uploading a whole mesh: a few
/// megabytes at a time.
const UPLOAD_CHUNK: usize = 1 << 17;

impl GpuMesh {
    /// The mesh's buffers, filled in one go as they are made (mapped at
    /// creation). Fine for small meshes and offscreen work; the app's clay
    /// uses [`GpuMesh::upload`].
    pub fn new(device: &wgpu::Device, mesh: &Mesh) -> Self {
        // Storage too, for the wireframe: a vertex is a whole number of
        // words, which its shader reads as one array.
        let storage = wgpu::BufferUsages::STORAGE;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("clay vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices()),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST | storage,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("clay indices"),
            contents: bytemuck::cast_slice(mesh.indices()),
            usage: wgpu::BufferUsages::INDEX | storage,
        });
        Self {
            vertices,
            indices,
            index_count: mesh.indices().len() as u32,
            wire: OnceLock::new(),
        }
    }

    /// The mesh's buffers, filled through the queue a few megabytes at a
    /// time. In the browser, a buffer mapped at creation needs one block
    /// of shared memory for all of it, and making one throws when the
    /// browser can't find that much (a fine level is 80 MB); queued
    /// writes go in pieces, and the vertices are never all copied at once.
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Self {
        let indices = mesh.indices();
        let buffer = |label, size: usize, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // Storage too, for the wireframe, as in `new`.
        let storage = wgpu::BufferUsages::STORAGE;
        let gpu = Self {
            vertices: buffer(
                "clay vertices",
                mesh.positions.len() * size_of::<Vertex>(),
                wgpu::BufferUsages::VERTEX | storage,
            ),
            indices: buffer(
                "clay indices",
                size_of_val(indices),
                wgpu::BufferUsages::INDEX | storage,
            ),
            index_count: indices.len() as u32,
            wire: OnceLock::new(),
        };
        for (i, chunk) in indices.chunks(UPLOAD_CHUNK).enumerate() {
            let offset = (i * UPLOAD_CHUNK * size_of::<u32>()) as u64;
            queue.write_buffer(&gpu.indices, offset, bytemuck::cast_slice(chunk));
        }
        let n = mesh.positions.len();
        for start in (0..n).step_by(UPLOAD_CHUNK) {
            gpu.write(queue, mesh, start..(start + UPLOAD_CHUNK).min(n));
        }
        gpu
    }

    /// Uploads the vertices in `range` after sculpting changed them.
    pub fn write(&self, queue: &wgpu::Queue, mesh: &Mesh, range: std::ops::Range<usize>) {
        let data: Vec<Vertex> = range.clone().map(|i| mesh.vertex(i)).collect();
        let offset = (range.start * size_of::<Vertex>()) as u64;
        queue.write_buffer(&self.vertices, offset, bytemuck::cast_slice(&data));
    }
}

/// Frees the GPU memory now. In the browser, dropping a buffer only lets
/// the garbage collector free it some time later, and it doesn't hurry for
/// GPU memory: each level change or remesh of a fine clay left tens of
/// megabytes behind. Work already submitted still finishes with them.
impl Drop for GpuMesh {
    fn drop(&mut self) {
        self.vertices.destroy();
        self.indices.destroy();
        if let Some((codes, _)) = self.wire.get() {
            codes.destroy();
        }
    }
}

/// How far the wireframe is pulled towards the camera, as a share of its
/// distance: enough to stay in front of the clay it lies on whatever the
/// rounding, far too little to show through clay in front of it.
const PULL: f32 = 2e-4;

/// What to add to a clip-space z so that a point lands where the point
/// [`PULL`] of the way to the camera would. With a perspective, clip z is
/// `a·z + c` and w is `-z` (view space z); scaling the point by `s`
/// towards the eye gives `s·(a·z) + c` over `s·w`, the same as adding
/// `c·(1 - s)/s` to the clip z and keeping x, y and w.
fn pull(projection: &Mat4) -> f32 {
    let s = 1.0 - PULL;
    projection.w_axis.z * (1.0 - s) / s
}

fn wire_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
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
        let read_only = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let wire_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wireframe"),
            entries: &[read_only(0), read_only(1), read_only(2)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("clay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let side = |v: &Variant, cull: wgpu::Face| {
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
                    // Each half culls the other side; drawn one after the
                    // other, thin forms and the inside of open shapes stay
                    // visible.
                    cull_mode: Some(cull),
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
        let pipeline =
            |v: Variant| [wgpu::Face::Front, wgpu::Face::Back].map(|cull| side(&v, cull));
        use wgpu::CompareFunction::{Always, Less, LessEqual};
        let wire_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wireframe"),
            bind_group_layouts: &[Some(&layout), Some(&wire_layout)],
            immediate_size: 0,
        });
        let wire = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wireframe"),
            layout: Some(&wire_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_wire"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_wire"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: VIEW_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            // Both sides at once: its shader doesn't ask which faces.
            primitive: wgpu::PrimitiveState::default(),
            // The clay's own surface passes the test, as each corner is
            // pulled a little towards the camera; nearer clay hides it.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(LessEqual),
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
            wire,
            wire_layout,
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
        let projection = camera.projection(aspect);
        let uniforms = Uniforms {
            view_proj: (projection * view).to_cols_array_2d(),
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
            pull: [pull(&projection), 0.0, 0.0, 0.0],
            wire: {
                let [r, g, b, _] = super::linear(theme::WIRE);
                [r, g, b, theme::WIRE_OPACITY]
            },
        };
        let (buffer, _) = &self.slots[slot.index()][layer as usize];
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    ///
    /// Back faces are drawn first, then front faces, as two draws. Mixed in
    /// one draw, Direct3D 12 on NVIDIA GPUs (Dawn in the browser, wgpu on
    /// desktop) shaded some front pixels as back faces: dashed lines along
    /// edges of subdivided clay. Back first is also the order see-through
    /// clay blends in.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot, mesh: &GpuMesh) {
        self.draw_sides(pass, slot, Layer::Main, mesh, &self.pipeline);
    }

    /// Lays `mesh` over everything the pass has drawn so far, as one
    /// see-through layer with the look given to
    /// [`prepare_ghost`](Self::prepare_ghost): only its nearest surface
    /// shows, and nothing drawn before hides it. It leaves the ghost's depth
    /// in the depth buffer, so draw it last.
    pub fn draw_ghost(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot, mesh: &GpuMesh) {
        for sides in [&self.ghost_clear, &self.ghost_depth, &self.ghost_colour] {
            self.draw_sides(pass, slot, Layer::Ghost, mesh, sides);
        }
    }

    /// Draws the edges of `mesh`, built from `cpu`, over the clay just
    /// drawn from it in this pass, as the slot's last
    /// [`prepare`](Self::prepare) sees it: quads as their outline, other
    /// triangles whole. Skipped for a mesh too big to bind as storage.
    pub fn draw_wire(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        slot: Slot,
        mesh: &GpuMesh,
        cpu: &Mesh,
    ) {
        let largest = device.limits().max_storage_buffer_binding_size;
        let bindable = |b: &wgpu::Buffer| {
            b.usage().contains(wgpu::BufferUsages::STORAGE) && b.size() <= largest
        };
        if mesh.index_count == 0 || !bindable(&mesh.vertices) || !bindable(&mesh.indices) {
            return;
        }
        let (_, bind_group) = mesh.wire.get_or_init(|| {
            // Four triangles to a byte; without quads, every edge shows.
            let all = NO_DIAGONAL * 0b0101_0101;
            let mut codes = cpu
                .diagonals()
                .unwrap_or_else(|| vec![all; cpu.triangles.len().div_ceil(4)]);
            codes.resize(codes.len().next_multiple_of(4), all);
            let diagonals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wireframe diagonals"),
                contents: &codes,
                usage: wgpu::BufferUsages::STORAGE,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wireframe"),
                layout: &self.wire_layout,
                entries: &[
                    wire_entry(0, &mesh.vertices),
                    wire_entry(1, &mesh.indices),
                    wire_entry(2, &diagonals),
                ],
            });
            (diagonals, bind_group)
        });
        pass.set_pipeline(&self.wire);
        pass.set_bind_group(0, &self.slots[slot.index()][Layer::Main as usize].1, &[]);
        pass.set_bind_group(1, bind_group, &[]);
        pass.draw(0..mesh.index_count, 0..1);
    }

    fn draw_sides(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: Slot,
        layer: Layer,
        mesh: &GpuMesh,
        sides: &Sides,
    ) {
        for pipeline in sides {
            pass.set_pipeline(pipeline);
            self.draw_mesh(pass, slot, layer, mesh);
        }
    }

    fn draw_mesh(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot, layer: Layer, mesh: &GpuMesh) {
        pass.set_bind_group(0, &self.slots[slot.index()][layer as usize].1, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ViewTarget;
    use crate::render::readback::Readback;

    /// A square facing the camera, `half` wide each way at depth `z`, as
    /// two triangles split along the diagonal from bottom left to top
    /// right, starting at vertex `first`.
    fn square(half: f32, z: f32, first: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .map(|(x, y)| Vec3::new(x * half, y * half, z));
        let [a, b, c, d] = [0, 1, 2, 3].map(|i| first + i);
        (corners.to_vec(), vec![[a, b, c], [a, c, d]])
    }

    /// What the wireframe costs on the start clay two levels up (1.57M
    /// vertices), drawn whole into a 1600×1000 view.
    /// `cargo test --release -p splatstudy-core wireframe_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn wireframe_timing() {
        use std::time::Instant;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        println!("{:?}", adapter.get_info());
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let (mesh, _) = crate::sculpt::cube_sphere(9);
        let size = [1600, 1000];
        let target = ViewTarget::new(&device, size);
        let clay = ClayPass::new(&device);
        let gpu = GpuMesh::new(&device, &mesh);
        let camera = OrbitCamera::default();
        let aspect = size[0] as f32 / size[1] as f32;
        clay.prepare(
            &queue,
            Slot::SculptView,
            &camera,
            aspect,
            &Look::lit(theme::CLAY),
        );
        let frames = |wire: bool, n: usize| {
            let start = Instant::now();
            for _ in 0..n {
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = target.begin_pass(&mut encoder, theme::VIEW_BG);
                    clay.draw(&mut pass, Slot::SculptView, &gpu);
                    if wire {
                        clay.draw_wire(&device, &mut pass, Slot::SculptView, &gpu, &mesh);
                    }
                }
                queue.submit([encoder.finish()]);
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            }
            start.elapsed().as_secs_f64() * 1e3 / n as f64
        };
        let start = Instant::now();
        frames(true, 1);
        println!(
            "first wireframe frame (codes made and uploaded): {:.1} ms",
            start.elapsed().as_secs_f64() * 1e3
        );
        for _ in 0..2 {
            let plain = frames(false, 30);
            let wired = frames(true, 30);
            println!(
                "{} vertices: clay {plain:.2} ms a frame, with the wireframe {wired:.2} ms",
                mesh.positions.len()
            );
        }
    }

    #[test]
    fn the_pull_draws_the_point_moved_towards_the_camera() {
        let camera = OrbitCamera::default();
        let projection = camera.projection(1.5);
        let to_clip = projection * camera.view();
        let eye = camera.eye();
        for p in [Vec3::new(0.3, -0.2, 0.9), Vec3::new(-1.0, 0.5, -0.4)] {
            let mut clip = to_clip * p.extend(1.0);
            clip.z += pull(&projection);
            let moved = to_clip * (eye + (p - eye) * (1.0 - PULL)).extend(1.0);
            let (a, b) = (clip.truncate() / clip.w, moved.truncate() / moved.w);
            assert!((a - b).abs().max_element() < 1e-6, "{a} {b}");
            assert!(a.z < (to_clip * p.extend(1.0)).z / clip.w);
        }
    }

    #[test]
    fn the_wireframe_outlines_quads_and_hides_behind_the_clay() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some((device, queue)) =
            pollster::block_on(instance.request_adapter(&Default::default()))
                .ok()
                .and_then(|a| pollster::block_on(a.request_device(&Default::default())).ok())
        else {
            eprintln!("no GPU adapter; skipping render test");
            return;
        };
        // A big square of two triangles, and in front of it a small one
        // that is a quad.
        let (mut positions, mut triangles) = square(1.0, 0.0, 0);
        let (p, t) = square(0.3, 0.02, 4);
        positions.extend(p);
        triangles.extend(t);
        let mut mesh = Mesh::from_ordered(positions, triangles);
        mesh.quads = vec![[2, 3]];
        let camera = OrbitCamera {
            pitch: 0.0,
            ..OrbitCamera::default()
        };
        let size = [96, 96];
        let target = ViewTarget::new(&device, size);
        let clay = ClayPass::new(&device);
        let gpu = GpuMesh::new(&device, &mesh);
        let render = |wire: bool| {
            clay.prepare(
                &queue,
                Slot::SculptView,
                &camera,
                1.0,
                &Look::lit(theme::CLAY),
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = target.begin_pass(&mut encoder, theme::VIEW_BG);
                clay.draw(&mut pass, Slot::SculptView, &gpu);
                if wire {
                    clay.draw_wire(&device, &mut pass, Slot::SculptView, &gpu, &mesh);
                }
            }
            let readback = Readback::copy(&device, &mut encoder, &target);
            queue.submit([encoder.finish()]);
            readback.map();
            readback.wait(&device).expect("pixels")
        };
        let plain = render(false);
        let wired = render(true);
        let to_clip = camera.projection(1.0) * camera.view();
        let red = |pixels: &[u8], x: f32, y: f32| {
            let p = to_clip.project_point3(Vec3::new(x, y, 0.0));
            let px = ((p.x * 0.5 + 0.5) * size[0] as f32) as usize;
            let py = ((0.5 - p.y * 0.5) * size[1] as f32) as usize;
            i32::from(pixels[(py * size[0] as usize + px) * 4])
        };
        let darker = |x, y| red(&plain, x, y) - red(&wired, x, y);
        // The big square's diagonal shows where nothing is in front of it,
        // and so do the small square's sides.
        assert!(darker(0.6, 0.6) > 20, "{}", darker(0.6, 0.6));
        assert!(darker(0.3, -0.1) > 20, "{}", darker(0.3, -0.1));
        // The small square's diagonal is hidden, and the big one's behind
        // it doesn't show through.
        assert!(darker(0.0, 0.0).abs() <= 2, "{}", darker(0.0, 0.0));
        assert!(darker(0.15, 0.15).abs() <= 2, "{}", darker(0.15, 0.15));
        // Away from every edge, the clay is as it was.
        assert!(darker(0.1, -0.15).abs() <= 2, "{}", darker(0.1, -0.15));
        assert!(darker(-0.6, 0.2).abs() <= 2, "{}", darker(-0.6, 0.2));
    }
}
