// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Draws Gaussian splats into a view, in the same multisampled pass as the
//! clay so the clay hides what is behind it.
//!
//! The splats live in storage buffers (centre, 3D covariance, colour, and
//! the view-dependent colour as [`Harmonics`] packs it); each is drawn as
//! an instanced 4-vertex quad that the vertex shader shapes by EWA
//! projection and colours for the side it is seen from (see `splat.wgsl`). Blending needs them back to front,
//! so they are sorted by view depth with a 16-bit counting sort: in
//! compute shaders (`splat_sort.wgsl`), or on the CPU where those don't
//! run; and only again once the view has turned past a small angle.
//!
//! The ghost ([`SplatLook::over`]) draws the splats whole into a target of
//! its own, then lays that over the pass at the ghost's opacity: one
//! see-through layer, like the mesh ghost, instead of splats that each
//! fade and build up where they overlap.
//!
//! Like [`ClayPass`](super::clay::ClayPass), each [`Slot`] keeps its own
//! camera, look and draw order, so the reference view, a ghost over the
//! sculpt and an export can all draw the same splats in one submit.
//!
//! Use: [`SplatPass::set_placement`] when the alignment or crop changes,
//! then per frame and slot [`SplatPass::prepare`] with a [`SplatLook`]
//! before the submit and [`SplatPass::draw`] inside the pass.

use std::cell::{Cell, RefCell};

use glam::{Mat3, Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::{DEPTH_FORMAT, SAMPLES, Slot, VIEW_FORMAT};
use crate::camera::OrbitCamera;
use crate::reference::fit::percentile_box;
use crate::reference::{Aabb, Harmonics, Splats};
use crate::ui::theme::SILHOUETTE_INK;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    viewport: [f32; 4],
    crop_min: [f32; 4],
    crop_max: [f32; 4],
    flat_colour: [f32; 4],
    params: [f32; 4],
    /// Where the view comes from in file coordinates (the eye, or the
    /// direction an orthographic view looks along), and the degree of
    /// view-dependent colour to add.
    sh: [f32; 4],
}

/// What the GPU sort needs, as `Params` in `splat_sort.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SortParams {
    row: [f32; 4],
    lo: f32,
    span: f32,
    count: u32,
    stride: u32,
}

/// Re-sort once the view direction has turned by more than this (cosine of
/// about one degree): a stale order shows only as faint popping.
const RESORT_COS: f32 = 0.999_85;

/// Invocations per workgroup in `splat_sort.wgsl`.
const SORT_GROUP: u32 = 256;
/// Depth steps of the sort, as in `splat_sort.wgsl`.
const BUCKETS: u64 = 1 << 16;
/// Keys kept at each end for depths outside the depth box, as in
/// `splat_sort.wgsl`: the outer sixteenth of the keys on either side.
const BAND: f32 = 4096.0;
/// Keys per doubling of the distance outside the box, measured in box
/// depths, as in `splat_sort.wgsl`: 16 doublings fill a band.
const PER_DOUBLING: f32 = 256.0;

/// The sort key of view depth `z` (more negative is farther), for a depth
/// box from `lo` (its farthest) to `lo + span`: farther sorts first.
/// Depths inside the box share out the keys between the two bands
/// evenly. Those beyond it (an unbounded capture's background, floaters)
/// take a band of keys at that end, coarser the farther out, so they keep
/// their order instead of piling up in one key. `splat_sort.wgsl` keys
/// the same way.
pub fn depth_key(z: f32, lo: f32, span: f32) -> u16 {
    let inner = 65_535.0 - 2.0 * BAND;
    let outside = |d: f32| (1.0 + (d / span).min(1e9)).log2() * PER_DOUBLING;
    let hi = lo + span;
    let key = if z < lo {
        (BAND - outside(lo - z)).floor().max(0.0)
    } else if z > hi {
        (65_536.0 - BAND + outside(z - hi).floor()).min(65_535.0)
    } else {
        BAND + ((z - lo) / span * inner).floor()
    };
    key as u16
}

/// The depth box's span along a view, never zero, so keys never divide by
/// zero: a flat box keys everything off it by distance in file units.
fn safe_span(lo: f32, hi: f32) -> f32 {
    if hi > lo { hi - lo } else { 1.0 }
}

/// Where splats are put in depth order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOn {
    /// A counting sort in compute shaders, leaving the CPU free, so
    /// millions of splats turn smoothly. The default where compute shaders
    /// run.
    Gpu,
    /// The same counting sort on the CPU, then uploaded.
    Cpu,
}

/// The ghost's own target: the splats at full opacity, laid over the slot
/// in one go.
struct Layer {
    size: [u32; 2],
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    /// What the target holds: the uniforms (camera, placement, crop, look,
    /// size) and the sort it was drawn with. While a stroke goes on these
    /// stay the same, and the splats aren't drawn into it again.
    drawn_for: Option<(Uniforms, u64)>,
}

/// One slot's camera, draw order and the direction it was sorted for.
struct SlotState {
    uniforms: wgpu::Buffer,
    order: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The GPU sort's parameters and bindings, when the GPU sorts.
    sort: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    layer: Option<Layer>,
    sorted_for: Option<Vec3>,
    /// Counts this slot's sorts, so the ghost's target knows when the
    /// order it was drawn with changed.
    sorts: u64,
    over: bool,
}

/// How splats are drawn into a slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplatLook {
    /// 0..1. Alone, it fades every splat; with `over`, the splats fade
    /// together as one layer.
    pub opacity: f32,
    /// Filled with the silhouette colour, [`SILHOUETTE_INK`].
    pub flat: bool,
    /// Drawn over everything already in the pass instead of being hidden by
    /// the clay in front: the ghost over the sculpt. The splats are drawn
    /// whole into a target of their own when the slot is prepared, and the
    /// pass gets that target at `opacity`, so where splats overlap the
    /// ghost is no stronger than where one splat covers.
    pub over: bool,
    /// Colours change with the side they are seen from, when the file has
    /// view-dependent colour; otherwise every splat shows its base colour.
    /// Ignored when `flat`.
    pub view_dependent: bool,
}

impl Default for SplatLook {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            flat: false,
            over: false,
            view_dependent: true,
        }
    }
}

/// The compute pipelines and scratch buffers of the GPU sort.
struct GpuSort {
    layout: wgpu::BindGroupLayout,
    depth_keys: wgpu::ComputePipeline,
    prefix: wgpu::ComputePipeline,
    place: wgpu::ComputePipeline,
    /// Each splat's depth key. The slots share it: each sort goes in a
    /// submit of its own, one after the other.
    keys: wgpu::Buffer,
    /// Splats per depth step, then where each step starts in the order.
    counts: wgpu::Buffer,
    /// The dispatch: workgroups across, rows, and invocations per row.
    groups: [u32; 3],
}

/// Gaussian splats on the GPU, ready to draw from any slot.
pub struct SplatPass {
    device: wgpu::Device,
    /// Hidden by the clay in front.
    pipeline: wgpu::RenderPipeline,
    /// The ghost's splats, alone into its target at full opacity.
    layer_splats: wgpu::RenderPipeline,
    /// The ghost's target laid over the pass.
    layer_over: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    layer_layout: wgpu::BindGroupLayout,
    positions: wgpu::Buffer,
    covariances: wgpu::Buffer,
    colours: wgpu::Buffer,
    /// The view-dependent colour, packed; one word when there is none.
    harmonics: wgpu::Buffer,
    /// The view-dependent colour's degree on the GPU: the file's, or lower
    /// when its buffer would be larger than the GPU allows.
    sh_degree: u8,
    /// Centres in file coordinates, for sorting.
    centres: Vec<Vec3>,
    /// Where nearly all centres lie, in file coordinates. The sort's
    /// depth steps span it evenly; the few beyond (floaters, a background)
    /// get coarser steps at either end, see [`depth_key`].
    depth_box: Aabb,
    /// File coordinates to the clay's space, and the crop box.
    placement: Cell<(Mat4, Option<Aabb>)>,
    /// Made the first time a slot is prepared.
    slots: [RefCell<Option<SlotState>>; Slot::COUNT],
    gpu_sort: Option<GpuSort>,
    scratch: RefCell<Sorter>,
    /// How many times splats were drawn into a ghost's target, for tests.
    layer_draws: Cell<u32>,
}

/// Whether `splat_sort.wgsl` can run: 256 invocations per workgroup, 1 KiB
/// of workgroup memory and four storage buffers.
fn can_sort_on_gpu(limits: &wgpu::Limits) -> bool {
    limits.max_compute_workgroup_size_x >= SORT_GROUP
        && limits.max_compute_invocations_per_workgroup >= SORT_GROUP
        && limits.max_compute_workgroup_storage_size >= 4 * SORT_GROUP
        && limits.max_storage_buffers_per_shader_stage >= 4
        && limits.max_compute_workgroups_per_dimension >= 1
}

impl SplatPass {
    /// Uploads `splats`, to be sorted on the GPU where it can. More splats
    /// than the GPU's buffers hold are thinned evenly; [`SplatPass::len`]
    /// says how many were kept. `None` if there are none.
    pub fn new(device: &wgpu::Device, splats: &Splats) -> Option<Self> {
        Self::sorting_on(device, splats, SortOn::Gpu)
    }

    /// [`SplatPass::new`], sorting where asked. [`SortOn::Gpu`] falls back
    /// to the CPU without compute shaders.
    pub fn sorting_on(device: &wgpu::Device, splats: &Splats, sort: SortOn) -> Option<Self> {
        let n = splats.positions.len();
        if n == 0 {
            return None;
        }
        let limits = device.limits();
        let largest = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let fits = (largest / 24).max(1) as usize;
        let step = n.div_ceil(fits);
        let kept = n.div_ceil(step);
        // The view-dependent colour's higher degrees go first when its
        // buffer would be too large: degree 1 always fits.
        let mut sh_degree = if splats.harmonics.len() == n {
            splats.harmonics.degree
        } else {
            0
        };
        while sh_degree > 1 && (kept * Harmonics::words_per_splat(sh_degree) * 4) as u64 > largest {
            sh_degree -= 1;
        }
        if sh_degree < splats.harmonics.degree {
            log::warn!(
                "view-dependent colour of degree {} kept to degree {sh_degree}",
                splats.harmonics.degree
            );
        }

        let mut positions = Vec::with_capacity(kept * 3);
        let mut covariances = Vec::with_capacity(kept * 6);
        let mut colours = Vec::with_capacity(kept);
        let mut centres = Vec::with_capacity(kept);
        let mut harmonics = Vec::with_capacity(kept * Harmonics::words_per_splat(sh_degree));
        for i in (0..n).step_by(step) {
            let p = splats.positions[i];
            centres.push(p);
            positions.extend_from_slice(&p.to_array());
            covariances.extend_from_slice(&covariance(splats.scales[i], splats.rotations[i]));
            colours.push(u32::from_le_bytes(splats.colors[i]));
            if sh_degree > 0 {
                splats.harmonics.push_splat_at(i, sh_degree, &mut harmonics);
            }
        }
        if harmonics.is_empty() {
            // Bound but never read.
            harmonics.push(0);
        }
        let storage = |label, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let positions = storage("splat positions", bytemuck::cast_slice(&positions));
        let covariances = storage("splat covariances", bytemuck::cast_slice(&covariances));
        let colours = storage("splat colours", bytemuck::cast_slice(&colours));
        let harmonics = storage("splat harmonics", bytemuck::cast_slice(&harmonics));
        let depth_box = percentile_box(&centres, 0.01, 0.99)
            .map(|b| b.grown(0.5))
            .unwrap_or(Aabb {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            });

        let shader = device.create_shader_module(wgpu::include_wgsl!("splat.wgsl"));
        let buffer_entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = wgpu::BufferBindingType::Uniform;
        let read_only = wgpu::BufferBindingType::Storage { read_only: true };
        let vertex = wgpu::ShaderStages::VERTEX;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splats"),
            entries: &[
                buffer_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, uniform),
                buffer_entry(1, vertex, read_only),
                buffer_entry(2, vertex, read_only),
                buffer_entry(3, vertex, read_only),
                buffer_entry(4, vertex, read_only),
                buffer_entry(6, vertex, read_only),
            ],
        });
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat layer"),
            entries: &[
                buffer_entry(0, wgpu::ShaderStages::FRAGMENT, uniform),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = |layout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("splats"),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            })
        };
        let splat_layout = pipeline_layout(&layout);
        let over_layout = pipeline_layout(&layer_layout);
        // Into a view's pass, tested against its depth with `compare`; or,
        // with `None`, alone into the ghost's target.
        let pipeline = |layout, vs, fs, strip, compare: Option<wgpu::CompareFunction>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("splats"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: VIEW_FORMAT,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: if strip {
                        wgpu::PrimitiveTopology::TriangleStrip
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: compare.map(|compare| wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: if compare.is_some() { SAMPLES } else { 1 },
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let gpu_sort = (sort == SortOn::Gpu && can_sort_on_gpu(&limits))
            .then(|| GpuSort::new(device, centres.len(), &limits));
        Some(Self {
            device: device.clone(),
            pipeline: pipeline(
                &splat_layout,
                "vs",
                "fs",
                true,
                Some(wgpu::CompareFunction::Less),
            ),
            layer_splats: pipeline(&splat_layout, "vs", "fs", true, None),
            layer_over: pipeline(
                &over_layout,
                "vs_layer",
                "fs_layer",
                false,
                Some(wgpu::CompareFunction::Always),
            ),
            layout,
            layer_layout,
            positions,
            covariances,
            colours,
            harmonics,
            sh_degree,
            centres,
            depth_box,
            placement: Cell::new((Mat4::IDENTITY, None)),
            slots: Default::default(),
            gpu_sort,
            scratch: RefCell::new(Sorter::default()),
            layer_draws: Cell::new(0),
        })
    }

    /// How many splats are drawn.
    pub fn len(&self) -> usize {
        self.centres.len()
    }

    pub fn is_empty(&self) -> bool {
        self.centres.is_empty()
    }

    /// The degree of view-dependent colour drawn: 0 when there is none.
    pub fn view_dependent_degree(&self) -> u8 {
        self.sh_degree
    }

    /// The bytes the splats take on the GPU, besides each slot's order.
    pub fn gpu_bytes(&self) -> u64 {
        [
            &self.positions,
            &self.covariances,
            &self.colours,
            &self.harmonics,
        ]
        .iter()
        .map(|b| b.size())
        .sum()
    }

    /// The slot was prepared at least once.
    #[cfg(test)]
    pub(crate) fn was_prepared(&self, slot: Slot) -> bool {
        self.slots[slot.index()].borrow().is_some()
    }

    /// Where the splats are sorted.
    pub fn sorts_on(&self) -> SortOn {
        if self.gpu_sort.is_some() {
            SortOn::Gpu
        } else {
            SortOn::Cpu
        }
    }

    /// Where the splats sit: `model` maps file coordinates into the clay's
    /// space (the reference's alignment); splats outside `crop` (file
    /// coordinates) are not drawn.
    pub fn set_placement(&self, model: Mat4, crop: Option<Aabb>) {
        if self.placement.get() != (model, crop) {
            self.placement.set((model, crop));
            for slot in &self.slots {
                if let Some(state) = slot.borrow_mut().as_mut() {
                    state.sorted_for = None;
                }
            }
        }
    }

    /// Sets the camera and look for the slot's next draw, and re-sorts the
    /// slot's splats if the view has turned. `size` is the target's size
    /// in pixels.
    ///
    /// The GPU sort and the ghost's own target ([`SplatLook::over`]) are
    /// drawn here, in a submit of their own, so call it before submitting
    /// the pass that [`draw`](Self::draw)s the slot.
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        slot: Slot,
        camera: &OrbitCamera,
        size: [u32; 2],
        look: &SplatLook,
    ) {
        let (model, crop) = self.placement.get();
        let pixels = [size[0].max(1), size[1].max(1)];
        let size = pixels.map(|v| v as f32);
        let view = camera.view();
        let proj = camera.projection(size[0] / size[1]);
        let focal = [proj.x_axis.x * size[0] / 2.0, proj.y_axis.y * size[1] / 2.0];
        let (crop_min, crop_max, crop_on) = match crop {
            Some(b) => (b.min, b.max, 1.0),
            None => (Vec3::ZERO, Vec3::ZERO, 0.0),
        };
        let [r, g, b, _] = SILHOUETTE_INK.to_array().map(|c| f32::from(c) / 255.0);
        let opacity = look.opacity.clamp(0.0, 1.0);
        // A ghost's splats go whole into its target, and the target fades.
        let (splat_opacity, layer_opacity) = if look.over {
            (1.0, opacity)
        } else {
            (opacity, 1.0)
        };
        // The view direction in the file's own coordinates, where the
        // harmonics were fitted: the alignment (up axis, turn) undone.
        let to_file = model.inverse();
        let from = if camera.ortho {
            to_file
                .transform_vector3(camera.target - camera.eye())
                .normalize_or_zero()
        } else {
            to_file.transform_point3(camera.eye())
        };
        let sh_degree = if look.view_dependent && !look.flat && from.is_finite() {
            self.sh_degree
        } else {
            0
        };
        let uniforms = Uniforms {
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            model: model.to_cols_array_2d(),
            viewport: [size[0], size[1], focal[0], focal[1]],
            crop_min: crop_min.extend(crop_on).to_array(),
            crop_max: crop_max.extend(0.0).to_array(),
            flat_colour: [r, g, b, 1.0],
            params: [
                splat_opacity,
                if look.flat { 1.0 } else { 0.0 },
                layer_opacity,
                if camera.ortho { 1.0 } else { 0.0 },
            ],
            sh: from.extend(f32::from(sh_degree)).to_array(),
        };

        let mut cell = self.slots[slot.index()].borrow_mut();
        let state = cell.get_or_insert_with(|| self.slot_state());
        queue.write_buffer(&state.uniforms, 0, bytemuck::bytes_of(&uniforms));
        state.over = look.over;
        let mut encoder = None;

        // View depth is one row of view · model; only its direction
        // changes the order.
        let row = (view * model).row(2);
        let direction = row.truncate().normalize_or_zero();
        let fresh = state
            .sorted_for
            .is_some_and(|last| last.dot(direction) > RESORT_COS);
        if !fresh {
            let (lo, hi) = depth_range(self.depth_box, row.truncate());
            match (&self.gpu_sort, &state.sort) {
                (Some(sort), Some((params, bind_group))) => {
                    let now = SortParams {
                        row: row.to_array(),
                        lo,
                        span: safe_span(lo, hi),
                        count: self.centres.len() as u32,
                        stride: sort.groups[2],
                    };
                    queue.write_buffer(params, 0, bytemuck::bytes_of(&now));
                    let encoder = encoder.get_or_insert_with(|| {
                        self.device.create_command_encoder(&Default::default())
                    });
                    sort.encode(encoder, bind_group);
                }
                _ => {
                    let mut sorter = self.scratch.borrow_mut();
                    let order = sorter.sort(&self.centres, row.truncate(), (lo, hi));
                    queue.write_buffer(&state.order, 0, bytemuck::cast_slice(order));
                }
            }
            state.sorted_for = Some(direction);
            state.sorts += 1;
        }

        if look.over {
            if state.layer.as_ref().is_none_or(|l| l.size != pixels) {
                state.layer = Some(self.layer(&state.uniforms, pixels));
            }
            let drawn_for = Some((uniforms, state.sorts));
            if let Some(layer) = state
                .layer
                .as_mut()
                .filter(|l| !same(&l.drawn_for, &drawn_for))
            {
                layer.drawn_for = drawn_for;
                self.layer_draws.set(self.layer_draws.get() + 1);
                let encoder = encoder
                    .get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("splat layer"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &layer.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&self.layer_splats);
                pass.set_bind_group(0, &state.bind_group, &[]);
                pass.draw(0..4, 0..self.centres.len() as u32);
            }
        }
        if let Some(encoder) = encoder {
            queue.submit([encoder.finish()]);
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot) {
        let cell = self.slots[slot.index()].borrow();
        let Some(state) = cell.as_ref() else {
            return;
        };
        if !state.over {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &state.bind_group, &[]);
            pass.draw(0..4, 0..self.centres.len() as u32);
        } else if let Some(layer) = &state.layer {
            pass.set_pipeline(&self.layer_over);
            pass.set_bind_group(0, &layer.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// A ghost target of `size` pixels, laid over with `uniforms`.
    fn layer(&self, uniforms: &wgpu::Buffer, size: [u32; 2]) -> Layer {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("splat layer"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: VIEW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat layer"),
            layout: &self.layer_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Layer {
            size,
            view,
            bind_group,
            drawn_for: None,
        }
    }

    fn slot_state(&self) -> SlotState {
        let uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let order = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat order"),
            size: (self.centres.len() * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splats"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.positions.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.covariances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.colours.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: order.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.harmonics.as_entire_binding(),
                },
            ],
        });
        let sort = self
            .gpu_sort
            .as_ref()
            .map(|sort| sort.bindings(&self.device, &self.positions, &order));
        SlotState {
            uniforms,
            order,
            bind_group,
            sort,
            layer: None,
            sorted_for: None,
            sorts: 0,
            over: false,
        }
    }
}

/// The same uniforms bit for bit, and the same sort.
fn same(a: &Option<(Uniforms, u64)>, b: &Option<(Uniforms, u64)>) -> bool {
    match (a, b) {
        (Some((a, i)), Some((b, j))) => i == j && bytemuck::bytes_of(a) == bytemuck::bytes_of(b),
        _ => false,
    }
}

/// The least and greatest depth `dot(row, p)` over the corners of `b`.
fn depth_range(b: Aabb, row: Vec3) -> (f32, f32) {
    (0..8).fold((f32::MAX, f32::MIN), |(lo, hi), i| {
        let corner = Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        let z = row.dot(corner);
        (lo.min(z), hi.max(z))
    })
}

impl GpuSort {
    fn new(device: &wgpu::Device, n: usize, limits: &wgpu::Limits) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("splat_sort.wgsl"));
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |read_only| wgpu::BufferBindingType::Storage { read_only };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat sort"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, storage(true)),
                entry(2, storage(false)),
                entry(3, storage(false)),
                entry(4, storage(false)),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splat sort"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // Rows of workgroups no longer than the device allows.
        let needed = (n as u32).div_ceil(SORT_GROUP).max(1);
        let across = needed.min(limits.max_compute_workgroups_per_dimension);
        let rows = needed.div_ceil(across);
        Self {
            depth_keys: pipeline("depth_keys"),
            prefix: pipeline("prefix"),
            place: pipeline("place"),
            layout,
            keys: buffer("splat keys", (n.max(1) * 4) as u64),
            counts: buffer("splat counts", BUCKETS * 4),
            groups: [across, rows, across * SORT_GROUP],
        }
    }

    /// A slot's sort parameters, and its bindings, sorting into `order`.
    fn bindings(
        &self,
        device: &wgpu::Device,
        positions: &wgpu::Buffer,
        order: &wgpu::Buffer,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat sort"),
            size: size_of::<SortParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let resources = [
            params.as_entire_binding(),
            positions.as_entire_binding(),
            self.keys.as_entire_binding(),
            self.counts.as_entire_binding(),
            order.as_entire_binding(),
        ];
        let entries: Vec<wgpu::BindGroupEntry> = resources
            .into_iter()
            .enumerate()
            .map(|(binding, resource)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource,
            })
            .collect();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat sort"),
            layout: &self.layout,
            entries: &entries,
        });
        (params, bind_group)
    }

    /// Sorts into the order `bind_group` holds, with its parameters already
    /// written.
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, bind_group: &wgpu::BindGroup) {
        encoder.clear_buffer(&self.counts, 0, None);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("splat sort"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, bind_group, &[]);
        let [across, rows, _] = self.groups;
        pass.set_pipeline(&self.depth_keys);
        pass.dispatch_workgroups(across, rows, 1);
        pass.set_pipeline(&self.prefix);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&self.place);
        pass.dispatch_workgroups(across, rows, 1);
    }
}

/// A splat's 3D covariance R S Sᵀ Rᵀ, as xx, xy, xz, yy, yz, zz.
pub fn covariance(scale: Vec3, rotation: glam::Quat) -> [f32; 6] {
    let m = Mat3::from_quat(rotation.normalize()) * Mat3::from_diagonal(scale);
    let s = m * m.transpose();
    [
        s.x_axis.x, s.y_axis.x, s.z_axis.x, s.y_axis.y, s.z_axis.y, s.z_axis.z,
    ]
}

/// Sorting by depth, reusing its buffers between sorts.
#[derive(Default)]
pub struct Sorter {
    keys: Vec<u16>,
    counts: Vec<u32>,
    order: Vec<u32>,
}

impl Sorter {
    /// Indices of `points` from the farthest to the nearest along a view
    /// whose depth is `dot(depth_row, p)` (view-space z: more negative is
    /// farther), with the nearest and the farthest point as the depth box.
    pub fn back_to_front(&mut self, points: &[Vec3], depth_row: Vec3) -> &[u32] {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in points {
            let z = depth_row.dot(*p);
            if z.is_finite() {
                lo = lo.min(z);
                hi = hi.max(z);
            }
        }
        self.sort(points, depth_row, (lo, hi))
    }

    /// [`back_to_front`](Self::back_to_front) with the depth box from
    /// `range.0` (farthest) to `range.1`, keyed by [`depth_key`] as the GPU
    /// sort keys them. Points with the same key keep their own order.
    pub fn sort(&mut self, points: &[Vec3], depth_row: Vec3, range: (f32, f32)) -> &[u32] {
        let n = points.len();
        let (lo, span) = (range.0, safe_span(range.0, range.1));
        self.keys.clear();
        self.keys.reserve(n);
        self.keys.extend(
            points
                .iter()
                .map(|p| depth_key(depth_row.dot(*p), lo, span)),
        );
        self.counts.clear();
        self.counts.resize(1 << 16, 0);
        for &k in &self.keys {
            self.counts[usize::from(k)] += 1;
        }
        let mut start = 0;
        for c in &mut self.counts {
            let count = *c;
            *c = start;
            start += count;
        }
        self.order.clear();
        self.order.resize(n, 0);
        for (i, &k) in self.keys.iter().enumerate() {
            let slot = &mut self.counts[usize::from(k)];
            self.order[*slot as usize] = i as u32;
            *slot += 1;
        }
        &self.order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorting_goes_from_far_to_near() {
        let points: Vec<Vec3> = [3.0, -5.0, 0.0, -1.0, 2.0]
            .iter()
            .map(|&z| Vec3::new(1.0, 2.0, z))
            .collect();
        let mut sorter = Sorter::default();
        // Looking down -Z from far away: depth is z itself.
        assert_eq!(sorter.back_to_front(&points, Vec3::Z), &[1, 3, 2, 4, 0]);
        // Turned around.
        assert_eq!(sorter.back_to_front(&points, -Vec3::Z), &[0, 4, 2, 3, 1]);
        assert!(sorter.back_to_front(&[], Vec3::Z).is_empty());
    }

    #[test]
    fn the_ghost_is_drawn_again_only_when_its_inputs_change() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some((device, queue)) =
            pollster::block_on(instance.request_adapter(&Default::default()))
                .ok()
                .and_then(|a| pollster::block_on(a.request_device(&Default::default())).ok())
        else {
            eprintln!("no GPU adapter; skipping render test");
            return;
        };
        let splats = Splats {
            positions: vec![Vec3::ZERO, Vec3::new(0.3, 0.1, -0.2)],
            scales: vec![Vec3::splat(0.2); 2],
            rotations: vec![glam::Quat::IDENTITY; 2],
            colors: vec![[200, 100, 50, 255]; 2],
            ..Default::default()
        };
        let ghost = SplatLook {
            opacity: 0.5,
            over: true,
            ..Default::default()
        };
        for sort in [SortOn::Gpu, SortOn::Cpu] {
            let pass = SplatPass::sorting_on(&device, &splats, sort).expect("splats");
            let mut camera = OrbitCamera::default();
            let size = [64, 48];
            let prepare = |camera: &OrbitCamera, size, look: &SplatLook| {
                pass.prepare(&queue, Slot::SculptView, camera, size, look);
                pass.layer_draws.get()
            };
            assert_eq!(prepare(&camera, size, &ghost), 1);
            assert_eq!(prepare(&camera, size, &ghost), 1, "nothing changed");
            camera.distance *= 1.1;
            assert_eq!(prepare(&camera, size, &ghost), 2, "the camera moved");
            assert_eq!(prepare(&camera, size, &ghost), 2);
            camera.yaw += 0.3;
            assert_eq!(prepare(&camera, size, &ghost), 3, "turned and re-sorted");
            assert_eq!(prepare(&camera, [80, 48], &ghost), 4, "resized");
            let flat = SplatLook {
                flat: true,
                ..ghost
            };
            assert_eq!(prepare(&camera, [80, 48], &flat), 5, "another look");
            pass.set_placement(Mat4::from_translation(Vec3::X), None);
            assert_eq!(prepare(&camera, [80, 48], &flat), 6, "moved");
            let plain = SplatLook::default();
            assert_eq!(prepare(&camera, [80, 48], &plain), 6, "not a ghost");
            // The target was left alone and still holds that ghost.
            assert_eq!(prepare(&camera, [80, 48], &flat), 6, "the same ghost again");
        }
    }

    #[test]
    fn depths_beyond_the_box_keep_their_order() {
        let (lo, span) = (-2.0, 4.0);
        assert_eq!(depth_key(lo, lo, span), 4096);
        assert_eq!(depth_key(lo + span, lo, span), 61_439);
        assert_eq!(depth_key(0.0, lo, span), 32_767);
        let out = [0.001, 0.5, 1.0, 3.0, 10.0, 100.0, 1e4, 1e30];
        // Beyond the far side: each farther depth (in box spans) sorts
        // before the nearer one, down to the end of the band.
        let far: Vec<u16> = out
            .iter()
            .map(|d| depth_key(lo - d * span, lo, span))
            .collect();
        assert!(far.windows(2).all(|w| w[0] > w[1]), "{far:?}");
        assert_eq!((far[0], far[far.len() - 1]), (4095, 0), "{far:?}");
        // And the near side, mirrored.
        let near: Vec<u16> = out
            .iter()
            .map(|d| depth_key(lo + span + d * span, lo, span))
            .collect();
        assert!(near.windows(2).all(|w| w[0] < w[1]), "{near:?}");
        assert_eq!((near[0], near[near.len() - 1]), (61_440, 65_535));
        // One doubling of the distance out is 256 keys.
        assert_eq!(depth_key(lo - span, lo, span), 4096 - 256);
        assert_eq!(depth_key(lo - 3.0 * span, lo, span), 4096 - 512);
    }

    #[test]
    fn far_points_sort_by_depth_on_the_cpu() {
        // A box from -1 to 1, and points far behind it given nearest first.
        let points: Vec<Vec3> = [0.5, -0.5, -10.0, -20.0, -40.0, 30.0]
            .iter()
            .map(|&z| Vec3::new(0.0, 0.0, z))
            .collect();
        let mut sorter = Sorter::default();
        assert_eq!(
            sorter.sort(&points, Vec3::Z, (-1.0, 1.0)),
            &[4, 3, 2, 1, 0, 5]
        );
    }

    #[test]
    fn covariance_of_a_turned_splat() {
        let c = covariance(
            Vec3::new(2.0, 1.0, 1.0),
            glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        );
        // The long axis now lies along y.
        assert!((c[0] - 1.0).abs() < 1e-5 && (c[3] - 4.0).abs() < 1e-5);
        assert!(c[1].abs() < 1e-5 && (c[5] - 1.0).abs() < 1e-5);
    }
}
