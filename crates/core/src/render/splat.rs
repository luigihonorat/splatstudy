// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Draws Gaussian splats into a view, in the same multisampled pass as the
//! clay so the clay hides what is behind it.
//!
//! The splats live in storage buffers (centre, 3D covariance, colour); each
//! is drawn as an instanced 4-vertex quad that the vertex shader shapes by
//! EWA projection (see `splat.wgsl`). Blending needs them back to front,
//! so the CPU sorts them by view depth with a 16-bit counting sort, and
//! only again once the view has turned past a small angle.
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
use crate::reference::{Aabb, Splats};
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
}

/// Re-sort once the view direction has turned by more than this (cosine of
/// about one degree): a stale order shows only as faint popping.
const RESORT_COS: f32 = 0.999_85;

/// One slot's camera, draw order and the direction it was sorted for.
struct SlotState {
    uniforms: wgpu::Buffer,
    order: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    sorted_for: Option<Vec3>,
    over: bool,
}

/// How splats are drawn into a slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplatLook {
    /// 0..1, fading every splat (a ghost).
    pub opacity: f32,
    /// Filled with the silhouette colour, [`SILHOUETTE_INK`].
    pub flat: bool,
    /// Drawn over everything already in the pass instead of being hidden by
    /// the clay in front: the ghost over the sculpt.
    pub over: bool,
}

impl Default for SplatLook {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            flat: false,
            over: false,
        }
    }
}

/// Gaussian splats on the GPU, ready to draw from any slot.
pub struct SplatPass {
    device: wgpu::Device,
    /// Hidden by the clay in front.
    pipeline: wgpu::RenderPipeline,
    /// Over everything drawn before: the ghost.
    over: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    positions: wgpu::Buffer,
    covariances: wgpu::Buffer,
    colours: wgpu::Buffer,
    /// Centres in file coordinates, for sorting.
    centres: Vec<Vec3>,
    /// File coordinates to the clay's space, and the crop box.
    placement: Cell<(Mat4, Option<Aabb>)>,
    /// Made the first time a slot is prepared.
    slots: [RefCell<Option<SlotState>>; Slot::COUNT],
    scratch: RefCell<Sorter>,
}

impl SplatPass {
    /// Uploads `splats`. More splats than the GPU's buffers hold are thinned
    /// evenly; [`SplatPass::len`] says how many were kept. `None` if there
    /// are none.
    pub fn new(device: &wgpu::Device, splats: &Splats) -> Option<Self> {
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
        let kept: Vec<usize> = (0..n).step_by(step).collect();

        let mut positions = Vec::with_capacity(kept.len() * 3);
        let mut covariances = Vec::with_capacity(kept.len() * 6);
        let mut colours = Vec::with_capacity(kept.len());
        let mut centres = Vec::with_capacity(kept.len());
        for &i in &kept {
            let p = splats.positions[i];
            centres.push(p);
            positions.extend_from_slice(&p.to_array());
            covariances.extend_from_slice(&covariance(splats.scales[i], splats.rotations[i]));
            colours.push(u32::from_le_bytes(splats.colors[i]));
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

        let shader = device.create_shader_module(wgpu::include_wgsl!("splat.wgsl"));
        let storage_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splats"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(1),
                storage_entry(2),
                storage_entry(3),
                storage_entry(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splats"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |compare| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("splats"),
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
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
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
        Some(Self {
            device: device.clone(),
            pipeline: pipeline(wgpu::CompareFunction::Less),
            over: pipeline(wgpu::CompareFunction::Always),
            layout,
            positions,
            covariances,
            colours,
            centres,
            placement: Cell::new((Mat4::IDENTITY, None)),
            slots: Default::default(),
            scratch: RefCell::new(Sorter::default()),
        })
    }

    /// How many splats are drawn.
    pub fn len(&self) -> usize {
        self.centres.len()
    }

    pub fn is_empty(&self) -> bool {
        self.centres.is_empty()
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
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        slot: Slot,
        camera: &OrbitCamera,
        size: [u32; 2],
        look: &SplatLook,
    ) {
        let (model, crop) = self.placement.get();
        let size = [size[0].max(1) as f32, size[1].max(1) as f32];
        let view = camera.view();
        let proj = camera.projection(size[0] / size[1]);
        let focal = [proj.x_axis.x * size[0] / 2.0, proj.y_axis.y * size[1] / 2.0];
        let (crop_min, crop_max, crop_on) = match crop {
            Some(b) => (b.min, b.max, 1.0),
            None => (Vec3::ZERO, Vec3::ZERO, 0.0),
        };
        let [r, g, b, _] = SILHOUETTE_INK.to_array().map(|c| f32::from(c) / 255.0);
        let uniforms = Uniforms {
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            model: model.to_cols_array_2d(),
            viewport: [size[0], size[1], focal[0], focal[1]],
            crop_min: crop_min.extend(crop_on).to_array(),
            crop_max: crop_max.extend(0.0).to_array(),
            flat_colour: [r, g, b, 1.0],
            params: [
                look.opacity.clamp(0.0, 1.0),
                if look.flat { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        };

        let mut cell = self.slots[slot.index()].borrow_mut();
        let state = cell.get_or_insert_with(|| self.slot_state());
        queue.write_buffer(&state.uniforms, 0, bytemuck::bytes_of(&uniforms));
        state.over = look.over;

        // View depth is one row of view · model; only its direction
        // changes the order.
        let row = (view * model).row(2);
        let direction = row.truncate().normalize_or_zero();
        let fresh = state
            .sorted_for
            .is_some_and(|last| last.dot(direction) > RESORT_COS);
        if !fresh {
            let mut sorter = self.scratch.borrow_mut();
            let order = sorter.back_to_front(&self.centres, row.truncate());
            queue.write_buffer(&state.order, 0, bytemuck::cast_slice(order));
            state.sorted_for = Some(direction);
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot) {
        let cell = self.slots[slot.index()].borrow();
        let Some(state) = cell.as_ref() else {
            return;
        };
        pass.set_pipeline(if state.over {
            &self.over
        } else {
            &self.pipeline
        });
        pass.set_bind_group(0, &state.bind_group, &[]);
        pass.draw(0..4, 0..self.centres.len() as u32);
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
            ],
        });
        SlotState {
            uniforms,
            order,
            bind_group,
            sorted_for: None,
            over: false,
        }
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
    /// farther). Depths are bucketed into 65 536 steps between the nearest
    /// and the farthest point.
    pub fn back_to_front(&mut self, points: &[Vec3], depth_row: Vec3) -> &[u32] {
        let n = points.len();
        self.keys.clear();
        self.keys.reserve(n);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in points {
            let z = depth_row.dot(*p);
            if z.is_finite() {
                lo = lo.min(z);
                hi = hi.max(z);
            }
        }
        let scale = if hi > lo { 65_535.0 / (hi - lo) } else { 0.0 };
        self.keys.extend(points.iter().map(|p| {
            let z = depth_row.dot(*p);
            // Farthest (lowest z) gets key 0 and draws first.
            ((z - lo) * scale).clamp(0.0, 65_535.0) as u16
        }));
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
