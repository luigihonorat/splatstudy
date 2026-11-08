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
    /// x: flat, y: opacity, z: use vertex colours, w: orthographic.
    params: [f32; 4],
    /// The strengths of the key light, the sky fill, the rim and the
    /// highlight.
    rig: [f32; 4],
    /// x: added to the wireframe's clip depth (see [`PULL`]).
    pull: [f32; 4],
    /// The wireframe's colour, linear RGB, and its opacity.
    wire: [f32; 4],
    /// The chalk's colour, linear RGB, and how much light fully masked
    /// clay keeps.
    layers: [f32; 4],
    /// x: faceted; y: the matcap; z: the matcap's curvature scale
    /// (see [`cavity_scale`]); w: the mesh's typical edge (see
    /// [`Look::edge`]).
    shading: [f32; 4],
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
    /// Flat shading: each triangle lit as one plane, so the facets show,
    /// rather than by its corners' normals smoothed across it.
    pub faceted: bool,
    /// The matcap: a soft studio clay ball's light, read from the normal
    /// as the eye sees it, with hollows a little darker and ridges a
    /// little lighter, so small forms read; off, the plainer light of
    /// [`Lighting`] alone.
    pub matcap: bool,
    /// The mesh's typical edge length (see [`typical_edge`]), for the
    /// matcap: each vertex's curvature is stored against it (see
    /// [`GpuMesh::write`]). 0 leaves out the hollows and ridges.
    pub edge: f32,
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
            faceted: false,
            matcap: false,
            edge: 0.0,
        }
    }

    /// The same look, lit by `lighting`.
    pub const fn with_lighting(self, lighting: Lighting) -> Self {
        Self { lighting, ..self }
    }

    /// The same look, faceted or smooth.
    pub const fn with_facets(self, faceted: bool) -> Self {
        Self { faceted, ..self }
    }

    /// The same look, with the matcap or without, for a mesh whose
    /// typical edge is `edge` long.
    pub const fn with_matcap(self, matcap: bool, edge: f32) -> Self {
        Self {
            matcap,
            edge,
            ..self
        }
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
            contents: bytemuck::cast_slice(&clay_vertices(mesh, 0..mesh.positions.len())),
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

    /// A scanned mesh to draw lit, from its triangles: each vertex's
    /// normal is the area-weighted mean of its triangles', and its colour
    /// the one in `colors`, white without.
    pub fn from_triangles(
        device: &wgpu::Device,
        positions: &[Vec3],
        triangles: &[[u32; 3]],
        colors: Option<&[[u8; 4]]>,
    ) -> Self {
        let mut normals = vec![Vec3::ZERO; positions.len()];
        for t in triangles {
            let [a, b, c] = t.map(|i| positions[i as usize]);
            // Area-weighted: the cross product's length is twice the area.
            let n = (b - a).cross(c - a);
            for &i in t {
                normals[i as usize] += n;
            }
        }
        let vertices: Vec<Vertex> = positions
            .iter()
            .zip(&normals)
            .enumerate()
            .map(|(i, (p, n))| Vertex {
                position: p.to_array(),
                normal: n.normalize_or(Vec3::Y).to_array(),
                color: colors.and_then(|c| c.get(i)).copied().unwrap_or([255; 4]),
                layers: [0; 4],
            })
            .collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reference vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reference indices"),
            contents: bytemuck::cast_slice(triangles),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            vertices,
            indices,
            index_count: (triangles.len() * 3) as u32,
            wire: OnceLock::new(),
        }
    }

    /// Uploads the vertices in `range` after sculpting changed them. A
    /// vertex's curvature depends on where its neighbours are, as its
    /// normal does, so the vertices whose normals a stroke changed are
    /// the ones to upload for both.
    pub fn write(&self, queue: &wgpu::Queue, mesh: &Mesh, range: std::ops::Range<usize>) {
        let data = clay_vertices(mesh, range.clone());
        let offset = (range.start * size_of::<Vertex>()) as u64;
        queue.write_buffer(&self.vertices, offset, bytemuck::cast_slice(&data));
    }
}

/// Frees the GPU memory now in the browser, where dropping a buffer only
/// lets the garbage collector free it some time later, and it doesn't
/// hurry for GPU memory: each level change or remesh of a fine clay left
/// tens of megabytes behind. Work already submitted still finishes with
/// them. Desktop backends free a dropped buffer once the GPU is done with
/// it anyway (and there, destroying it early slowed later readbacks).
impl Drop for GpuMesh {
    fn drop(&mut self) {
        if cfg!(target_arch = "wasm32") {
            self.vertices.destroy();
            self.indices.destroy();
            if let Some((codes, _)) = self.wire.get() {
                codes.destroy();
            }
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
/// `c·(1 - s)/s` to the clip z and keeping x, y and w. An orthographic
/// view has no eye to scale towards and an even depth: the point moves
/// [`PULL`] of the `distance` to the target nearer, `a` per unit.
fn pull(projection: &Mat4, distance: f32) -> f32 {
    if is_ortho(projection) {
        return projection.z_axis.z * PULL * distance;
    }
    let s = 1.0 - PULL;
    projection.w_axis.z * (1.0 - s) / s
}

/// The length the matcap scales curvature by into its hollows and
/// ridges: a share of the camera's distance, so detail reads the same way
/// at every zoom. A bump a few hundredths of the distance across is
/// caught at half strength; broad forms, hardly at all.
fn cavity_scale(distance: f32) -> f32 {
    CAVITY_SHARE * distance
}

const CAVITY_SHARE: f32 = 0.01;

/// The vertices in `range` as the clay's shader reads them: each with its
/// curvature in its third layer byte (see [`curvature_byte`]), measured
/// against the mesh's [`typical_edge`], which the matcap is given too.
fn clay_vertices(mesh: &Mesh, range: std::ops::Range<usize>) -> Vec<Vertex> {
    let edge = typical_edge(mesh);
    range
        .map(|i| {
            let mut v = mesh.vertex(i);
            v.layers[2] = curvature_byte(curvature(mesh, i) * edge);
            v
        })
        .collect()
}

/// How much the clay bends at vertex `i`, per unit of length, the two
/// principal curvatures added: positive where it bulges (a ball of radius
/// r gives 2 / r), negative in a hollow. From the vertex's neighbours: how
/// far their mean lies below it along its normal, over the square of their
/// mean distance (the umbrella). It depends on the neighbours' positions
/// alone, so it changes exactly where the normals do. Per vertex and
/// interpolated across the triangles, it has no steps along their edges,
/// as a curvature read per pixel from the screen derivatives does.
pub fn curvature(mesh: &Mesh, i: usize) -> f32 {
    // A mesh made only to be picked on has no neighbours or normals.
    if mesh.neighbours.is_empty() || i >= mesh.normals.len() {
        return 0.0;
    }
    let around = mesh.neighbours.of(i as u32);
    if around.is_empty() {
        return 0.0;
    }
    let p = mesh.positions[i];
    let (mut sum, mut squares) = (Vec3::ZERO, 0.0);
    for &j in around {
        let d = mesh.positions[j as usize] - p;
        sum += d;
        squares += d.length_squared();
    }
    if squares <= 0.0 {
        return 0.0;
    }
    // The neighbours a distance h away on a ball of radius r sit
    // h² / 2r below the vertex: four times that over h² is 2 / r.
    -4.0 * sum.dot(mesh.normals[i]) / squares
}

/// A curvature times the typical edge (a few hundredths on a smooth ball,
/// up to about 1 on a crease) as a signed byte, finer near 0: the shader
/// undoes it with `bend_of`. 0 is flat, so meshes without (references)
/// read as flat.
fn curvature_byte(k: f32) -> u8 {
    let e = k / (CURVATURE_KNEE + k.abs());
    (e * 127.0).round() as i8 as u8
}

/// Where [`curvature_byte`] is half its range; also in clay.wgsl.
const CURVATURE_KNEE: f32 = 0.25;

/// The mean length of the edges of up to a few hundred of `mesh`'s
/// triangles, spread through it: cheap enough for every frame.
pub fn typical_edge(mesh: &Mesh) -> f32 {
    let n = mesh.triangles.len();
    if n == 0 {
        return 0.0;
    }
    let step = n.div_ceil(256);
    let (mut sum, mut count) = (0.0, 0);
    for t in mesh.triangles.iter().step_by(step) {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        sum += a.distance(b) + b.distance(c) + c.distance(a);
        count += 3;
    }
    sum / count as f32
}

/// An orthographic projection keeps w at 1.
pub fn is_ortho(projection: &Mat4) -> bool {
    projection.z_axis.w == 0.0
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
                            3 => Unorm8x4,
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
                flag(camera.ortho),
            ],
            rig: [rig.key, rig.sky, rig.rim, rig.highlight],
            pull: [pull(&projection, camera.distance), 0.0, 0.0, 0.0],
            wire: {
                let [r, g, b, _] = super::linear(theme::WIRE);
                [r, g, b, theme::WIRE_OPACITY]
            },
            layers: {
                let [r, g, b, _] = super::linear(theme::CHALK);
                [r, g, b, theme::MASK_SHADE]
            },
            shading: [
                flag(look.faceted),
                flag(look.matcap),
                cavity_scale(camera.distance),
                look.edge,
            ],
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
            clip.z += pull(&projection, camera.distance);
            let moved = to_clip * (eye + (p - eye) * (1.0 - PULL)).extend(1.0);
            let (a, b) = (clip.truncate() / clip.w, moved.truncate() / moved.w);
            assert!((a - b).abs().max_element() < 1e-6, "{a} {b}");
            assert!(a.z < (to_clip * p.extend(1.0)).z / clip.w);
        }

        // Orthographic: the point moves nearer by the share of the
        // distance to the target, along the view.
        let camera = OrbitCamera {
            ortho: true,
            ..camera
        };
        let projection = camera.projection(1.5);
        assert!(is_ortho(&projection));
        assert!(!is_ortho(&OrbitCamera::default().projection(1.5)));
        let to_clip = projection * camera.view();
        let towards = (camera.eye() - camera.target).normalize() * PULL * camera.distance;
        for p in [Vec3::new(0.3, -0.2, 0.9), Vec3::new(-1.0, 0.5, -0.4)] {
            let mut clip = to_clip * p.extend(1.0);
            clip.z += pull(&projection, camera.distance);
            let moved = to_clip * (p + towards).extend(1.0);
            assert!((clip - moved).abs().max_element() < 1e-7, "{clip} {moved}");
            assert!(clip.z < (to_clip * p.extend(1.0)).z);
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

    /// The start clay sculpted a little: a broad mound, a ridge over it,
    /// a groove down one side, a row of small bumps and fine ripples, the
    /// sizes of a sculpt's forms and details.
    fn sculpted(level: usize) -> Mesh {
        let (mut mesh, _) = crate::sculpt::cube_sphere(level);
        let bump =
            |p: Vec3, at: Vec3, size: f32| (-(p - at).length_squared() / (size * size)).exp();
        for p in &mut mesh.positions {
            let n = p.normalize();
            let front = n.z.max(0.0);
            let mut h = 0.12 * bump(n, Vec3::new(0.0, -0.1, 1.0), 0.3);
            h += 0.05 * (-((n.y - 0.35) / 0.06).powi(2)).exp() * front;
            h -= 0.03 * (-((n.x - 0.3) / 0.025).powi(2)).exp() * front * (n.y < 0.2) as u8 as f32;
            for k in 0..5 {
                let at = Vec3::new(-0.6 + 0.09 * k as f32, -0.45, 0.66).normalize();
                h += 0.02 * bump(n, at, 0.03);
            }
            if n.x < -0.25 && n.y > 0.0 {
                h += 0.006 * (n.y * 90.0).sin() * front;
            }
            *p = n * (1.0 + h);
        }
        mesh.recompute_normals();
        mesh
    }

    /// The start clay a level up with a few clay strokes across its front, as a hand
    /// would lay them: dabs every 0.15 radius, each centred where a ray
    /// from the front meets the clay now, one stroke dug in.
    fn stroked() -> Mesh {
        use crate::sculpt::{Brush, Dab, Sculpt};
        let mut s = Sculpt::new(crate::sculpt::cube_sphere(crate::sculpt::START_LEVEL + 1).0);
        let strokes = [
            (glam::vec2(-0.6, 0.35), glam::vec2(0.55, 0.45), 0.12, false),
            (glam::vec2(-0.5, 0.05), glam::vec2(0.6, -0.05), 0.15, false),
            (glam::vec2(-0.55, -0.3), glam::vec2(0.5, -0.2), 0.1, true),
            (glam::vec2(-0.2, -0.55), glam::vec2(0.3, 0.6), 0.09, false),
        ];
        for (from, to, radius, invert) in strokes {
            let step = 0.15 * radius;
            let count = (from.distance(to) / step) as usize;
            let direction = (to - from).normalize().extend(0.0);
            s.begin_stroke();
            for i in 0..=count {
                let at = from.lerp(to, i as f32 / count as f32);
                let hit = s.raycast(at.extend(3.0), -Vec3::Z).unwrap();
                s.dab(Dab {
                    brush: Brush::Clay,
                    centre: hit.point,
                    radius,
                    strength: 0.5,
                    invert,
                    symmetry: false,
                    direction: if i == 0 { Vec3::ZERO } else { direction },
                    buildup: Default::default(),
                    fill: false,
                    wipes: false,
                });
                s.refresh();
            }
            s.end_stroke();
        }
        s.mesh
    }

    /// The renders the matcap was judged by, the classic light beside it:
    /// set `SPLATSTUDY_MATCAP_PNGS` to a folder to have them written
    /// there (`cargo test -p splatstudy-core matcap -- --nocapture`).
    /// Checks that hollows read darker with the matcap than without, and
    /// that the chalk, the mask and the silhouette still show through it.
    #[test]
    fn the_matcap_darkens_hollows_and_keeps_the_chalk_and_mask() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some((device, queue)) =
            pollster::block_on(instance.request_adapter(&Default::default()))
                .ok()
                .and_then(|a| pollster::block_on(a.request_device(&Default::default())).ok())
        else {
            eprintln!("no GPU adapter; skipping render test");
            return;
        };
        let out = std::env::var_os("SPLATSTUDY_MATCAP_PNGS").map(std::path::PathBuf::from);
        let side = if out.is_some() { 640 } else { 160 };
        let size = [side, side];
        let target = ViewTarget::new(&device, size);
        let clay = ClayPass::new(&device);
        let camera = OrbitCamera {
            pitch: 0.0,
            distance: 4.2,
            ..OrbitCamera::default()
        };
        let render = |gpu: &GpuMesh, look: &Look, background: Color32, camera: &OrbitCamera| {
            clay.prepare(&queue, Slot::SculptView, camera, 1.0, look);
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = target.begin_pass(&mut encoder, background);
                clay.draw(&mut pass, Slot::SculptView, gpu);
            }
            let readback = Readback::copy(&device, &mut encoder, &target);
            queue.submit([encoder.finish()]);
            readback.map();
            readback.wait(&device).expect("pixels")
        };
        let mut mesh = sculpted(if out.is_some() { 8 } else { 6 });
        // Chalk on the upper left, the mask on the upper right.
        let n = mesh.positions.len();
        for i in 0..n {
            let p = mesh.positions[i];
            if p.y > 0.55 && p.x < -0.2 {
                mesh.layers.chalk.set(i, 1.0, n);
            }
            if p.y > 0.55 && p.x > 0.2 {
                mesh.layers.mask.set(i, 1.0, n);
            }
        }
        let gpu = GpuMesh::new(&device, &mesh);
        let plain = Look::lit(theme::CLAY);
        let matcap = plain.with_matcap(true, typical_edge(&mesh));
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            let start_mesh = crate::sculpt::cube_sphere(7).0;
            let start_gpu = GpuMesh::new(&device, &start_mesh);
            let start = (&start_gpu, typical_edge(&start_mesh));
            let turned = OrbitCamera {
                yaw: 0.9,
                pitch: 0.25,
                ..camera
            };
            let light = Color32::from_gray(0xE8);
            // Coarse and close: each triangle many pixels wide.
            let coarse_mesh = sculpted(5);
            let coarse_gpu = GpuMesh::new(&device, &coarse_mesh);
            let coarse = (&coarse_gpu, typical_edge(&coarse_mesh));
            let fine = (&gpu, matcap.edge);
            let close = OrbitCamera {
                distance: 2.2,
                target: Vec3::new(0.2, 0.0, 0.0),
                ..camera
            };
            let strokes_mesh = stroked();
            let strokes_gpu = GpuMesh::new(&device, &strokes_mesh);
            let strokes = (&strokes_gpu, typical_edge(&strokes_mesh));
            let strokes_close = OrbitCamera {
                distance: 1.8,
                target: Vec3::new(0.0, 0.1, 0.9),
                ..camera
            };
            for (name, gpu, camera, background) in [
                ("start", &start, &camera, theme::VIEW_BG),
                ("strokes", &strokes, &camera, theme::VIEW_BG),
                ("strokes-close", &strokes, &strokes_close, theme::VIEW_BG),
                ("strokes-light-bg", &strokes, &turned, light),
                ("coarse", &coarse, &close, theme::VIEW_BG),
                ("sculpted", &fine, &camera, theme::VIEW_BG),
                ("turned", &fine, &turned, theme::VIEW_BG),
                ("light-bg", &fine, &turned, light),
            ] {
                let (gpu, edge) = *gpu;
                let matcap = plain.with_matcap(true, edge);
                for lighting in Lighting::ALL {
                    for (shader, look) in [("classic", plain), ("matcap", matcap)] {
                        let look = look.with_lighting(lighting);
                        let pixels = render(gpu, &look, background, camera);
                        let file = format!("{name}-{lighting:?}-{shader}.png").to_lowercase();
                        let png = crate::export::png(&pixels, size).unwrap();
                        std::fs::write(dir.join(file), png).unwrap();
                    }
                }
                let faceted = matcap.with_facets(true);
                let png = crate::export::png(&render(gpu, &faceted, background, camera), size);
                std::fs::write(dir.join(format!("{name}-faceted-matcap.png")), png.unwrap())
                    .unwrap();
            }
        }

        let grey = |pixels: &[u8], p: Vec3| {
            let q = (camera.projection(1.0) * camera.view()).project_point3(p);
            let x = ((q.x * 0.5 + 0.5) * size[0] as f32) as usize;
            let y = ((0.5 - q.y * 0.5) * size[1] as f32) as usize;
            let i = (y * size[0] as usize + x) * 4;
            [0, 1, 2].map(|c| i32::from(pixels[i + c]))
        };
        let on_clay = |n: Vec3| {
            let n = n.normalize();
            mesh.positions
                .iter()
                .copied()
                .max_by(|a, b| a.normalize().dot(n).total_cmp(&b.normalize().dot(n)))
                .unwrap()
        };
        let (before, after) = (
            render(&gpu, &plain, theme::VIEW_BG, &camera),
            render(&gpu, &matcap, theme::VIEW_BG, &camera),
        );
        // The groove, against the clay just beside it.
        let groove = on_clay(Vec3::new(0.3, -0.2, 0.93));
        let beside = on_clay(Vec3::new(0.38, -0.2, 0.9));
        let depth = |pixels: &[u8]| grey(pixels, beside)[1] - grey(pixels, groove)[1];
        // A little darker: the cavity is a quiet aid.
        assert!(
            depth(&after) > depth(&before) + 2,
            "{} {}",
            depth(&after),
            depth(&before)
        );
        // Chalk is blue and the mask dark, as without the matcap.
        let [r, _, b] = grey(&after, on_clay(Vec3::new(-0.45, 0.8, 0.4)));
        assert!(b > r + 10, "chalk {r} {b}");
        let masked = grey(&after, on_clay(Vec3::new(0.45, 0.8, 0.4)))[1];
        let unmasked = grey(&after, on_clay(Vec3::new(0.0, 0.8, 0.6)))[1];
        assert!(masked + 20 < unmasked, "mask {masked} {unmasked}");
        // A silhouette is flat whichever light.
        let ink = Look::flat(theme::SILHOUETTE_INK);
        assert_eq!(
            render(&gpu, &ink, theme::VIEW_BG, &camera),
            render(&gpu, &ink.with_matcap(true, 0.0), theme::VIEW_BG, &camera)
        );
    }

    #[test]
    fn curvature_is_two_over_the_radius_and_survives_its_byte() {
        let (ball, _) = crate::sculpt::cube_sphere(5);
        let mut big = ball.clone();
        big.positions.iter_mut().for_each(|p| *p *= 2.0);
        big.recompute_normals();
        for (mesh, expected) in [(&ball, 2.0), (&big, 1.0)] {
            let mean = (0..mesh.positions.len())
                .map(|i| curvature(mesh, i))
                .sum::<f32>()
                / mesh.positions.len() as f32;
            assert!((mean - expected).abs() < 0.05 * expected, "{mean}");
        }
        // Signed, 0 for flat, and finer near 0 than far out.
        assert_eq!(curvature_byte(0.0), 0);
        let signed = |k: f32| curvature_byte(k) as i8;
        assert!(signed(0.02) > 0 && signed(-0.02) < 0);
        assert_eq!(signed(0.3), -signed(-0.3));
        assert!(signed(0.1) - signed(0.0) > signed(2.0) - signed(1.9));
    }

    /// The matcap's curvature is found per vertex and blended across the
    /// triangles: along a stroke's dabs and over the triangles' edges it
    /// changes smoothly, without the steps a curvature read per triangle
    /// from screen derivatives makes. Measured as the mean jump in the
    /// change of grey from pixel to pixel over the clay, which steps
    /// raise and which the plain light, smooth everywhere, sets the
    /// level of. (The derivative curvature measured 1.5 times the plain
    /// light's here; per vertex, 1.2.)
    #[test]
    fn the_matcap_has_no_steps_along_strokes() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some((device, queue)) =
            pollster::block_on(instance.request_adapter(&Default::default()))
                .ok()
                .and_then(|a| pollster::block_on(a.request_device(&Default::default())).ok())
        else {
            eprintln!("no GPU adapter; skipping render test");
            return;
        };
        let size = [400, 400];
        let target = ViewTarget::new(&device, size);
        let clay = ClayPass::new(&device);
        let mesh = stroked();
        let gpu = GpuMesh::new(&device, &mesh);
        let camera = OrbitCamera {
            pitch: 0.0,
            distance: 1.8,
            target: Vec3::new(0.0, 0.1, 0.9),
            ..OrbitCamera::default()
        };
        let render = |look: &Look| {
            clay.prepare(&queue, Slot::SculptView, &camera, 1.0, look);
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = target.begin_pass(&mut encoder, theme::VIEW_BG);
                clay.draw(&mut pass, Slot::SculptView, &gpu);
            }
            let readback = Readback::copy(&device, &mut encoder, &target);
            queue.submit([encoder.finish()]);
            readback.map();
            readback.wait(&device).expect("pixels")
        };
        let [w, h] = size.map(|s| s as usize);
        // The mean of |a - 2b + c| over runs of three clay pixels, across
        // and down.
        let roughness = |pixels: &[u8]| {
            let at = |x: usize, y: usize| pixels[(y * w + x) * 4 + 1];
            let bg = theme::VIEW_BG.g();
            let (mut sum, mut count) = (0.0, 0);
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    for [a, b, c] in [
                        [at(x - 1, y), at(x, y), at(x + 1, y)],
                        [at(x, y - 1), at(x, y), at(x, y + 1)],
                    ] {
                        if [a, b, c].contains(&bg) {
                            continue;
                        }
                        sum += (f32::from(a) - 2.0 * f32::from(b) + f32::from(c)).abs();
                        count += 1;
                    }
                }
            }
            sum / count as f32
        };
        let plain = Look::lit(theme::CLAY);
        let matcap = plain.with_matcap(true, typical_edge(&mesh));
        let (smooth, shaded) = (roughness(&render(&plain)), roughness(&render(&matcap)));
        assert!(shaded < 1.3 * smooth, "{shaded} against {smooth}");
    }

    #[test]
    fn faceted_clay_lights_each_triangle_as_its_plane_facing_the_eye() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some((device, queue)) =
            pollster::block_on(instance.request_adapter(&Default::default()))
                .ok()
                .and_then(|a| pollster::block_on(a.request_device(&Default::default())).ok())
        else {
            eprintln!("no GPU adapter; skipping render test");
            return;
        };
        // A tent facing the camera: two slopes meeting at a ridge along y,
        // so the smoothed normals turn across each slope.
        let positions = [(-1.0, 0.0), (0.0, 0.5), (1.0, 0.0)]
            .iter()
            .flat_map(|&(x, z)| [Vec3::new(x, -1.0, z), Vec3::new(x, 1.0, z)])
            .collect();
        let triangles = vec![[0, 2, 3], [0, 3, 1], [2, 4, 5], [2, 5, 3]];
        let tent = Mesh::from_ordered(positions, triangles);
        let size = [96, 96];
        let target = ViewTarget::new(&device, size);
        let clay = ClayPass::new(&device);
        let gpu = GpuMesh::new(&device, &tent);
        let render = |camera: &OrbitCamera, faceted: bool| {
            let look = Look::lit(theme::CLAY).with_facets(faceted);
            clay.prepare(&queue, Slot::SculptView, camera, 1.0, &look);
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = target.begin_pass(&mut encoder, theme::VIEW_BG);
                clay.draw(&mut pass, Slot::SculptView, &gpu);
            }
            let readback = Readback::copy(&device, &mut encoder, &target);
            queue.submit([encoder.finish()]);
            readback.map();
            readback.wait(&device).expect("pixels")
        };
        let red = |pixels: &[u8], camera: &OrbitCamera, x: f32| {
            let to_clip = camera.projection(1.0) * camera.view();
            let z = 0.5 * (1.0 - x.abs());
            let p = to_clip.project_point3(Vec3::new(x, 0.2, z));
            let px = ((p.x * 0.5 + 0.5) * size[0] as f32) as usize;
            let py = ((0.5 - p.y * 0.5) * size[1] as f32) as usize;
            i32::from(pixels[(py * size[0] as usize + px) * 4])
        };
        for yaw in [0.0, 0.6, std::f32::consts::PI] {
            // Head on, turned, and from behind, where the back faces show.
            let camera = OrbitCamera {
                yaw,
                pitch: 0.0,
                ..OrbitCamera::default()
            };
            let (smooth, faceted) = (render(&camera, false), render(&camera, true));
            for (from, to) in [(-0.85, -0.15), (0.15, 0.85)] {
                let across = |pixels: &[u8]| red(pixels, &camera, from) - red(pixels, &camera, to);
                // Smooth, the light changes across a slope (head on, where
                // the light falls on both alike); faceted, it is even over
                // it from every side.
                if yaw == 0.0 {
                    assert!(across(&smooth).abs() > 4, "{}", across(&smooth));
                }
                assert!(
                    across(&faceted).abs() <= 2,
                    "yaw {yaw}: {}",
                    across(&faceted)
                );
                // Where a slope's own corners are, its smoothed normal is
                // its plane's: both agree, so the plane faces the eye as
                // the smooth normal does, front or back.
                let edge = |pixels: &[u8]| red(pixels, &camera, from.signum() * 0.97);
                let gap = edge(&smooth) - edge(&faceted);
                assert!(gap.abs() <= 3, "yaw {yaw}: {gap}");
            }
        }
    }
}
