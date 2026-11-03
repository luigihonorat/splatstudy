// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The clay's GPU buffers: uploaded in pieces through the queue, they draw
//! what buffers filled at creation draw, and making new ones in quick
//! succession (fast level changes) while the old ones are destroyed under
//! submitted work is valid. Skipped (with a message) without a GPU adapter.

use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Look};
use splatstudy_core::render::{Slot, ViewTarget, readback::Readback};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::{CLAY, VIEW_BG};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// A lumpy sphere, so that vertices in the wrong place would show.
fn lumpy(level: u32) -> Mesh {
    let sphere = Mesh::icosphere(level);
    let positions = sphere
        .positions
        .iter()
        .map(|&p| p * (1.0 + 0.15 * (7.0 * p.x).sin() * (5.0 * p.y).cos() + 0.1 * p.z))
        .collect();
    Mesh::from_ordered(positions, sphere.triangles.clone())
}

fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    clay: &ClayPass,
    target: &ViewTarget,
    mesh: &GpuMesh,
) -> Vec<u8> {
    let mut camera = OrbitCamera::default();
    camera.orbit(30.0, -20.0);
    clay.prepare(
        queue,
        Slot::SculptView,
        &camera,
        target.aspect(),
        &Look::lit(CLAY),
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
        clay.draw(&mut pass, Slot::SculptView, mesh);
    }
    let readback = Readback::copy(device, &mut encoder, target);
    queue.submit([encoder.finish()]);
    readback.map();
    readback.wait(device).expect("read back")
}

#[test]
fn a_mesh_uploaded_in_pieces_draws_as_one_filled_at_creation() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping upload test");
        return;
    };
    // More vertices and indices than one piece holds.
    let mesh = lumpy(7);
    assert!(mesh.positions.len() > 1 << 17);
    let target = ViewTarget::new(&device, [96, 80]);
    let clay = ClayPass::new(&device);
    let filled = draw(
        &device,
        &queue,
        &clay,
        &target,
        &GpuMesh::new(&device, &mesh),
    );
    let uploaded = draw(
        &device,
        &queue,
        &clay,
        &target,
        &GpuMesh::upload(&device, &queue, &mesh),
    );
    assert!(filled.chunks(4).any(|p| p != VIEW_BG.to_array()));
    assert_eq!(filled, uploaded);
}

#[test]
fn meshes_replaced_quickly_under_submitted_work_stay_valid() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping upload test");
        return;
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let target = ViewTarget::new(&device, [64, 64]);
    let clay = ClayPass::new(&device);
    let meshes: Vec<Mesh> = (2..6).map(lumpy).collect();
    let mut gpu = GpuMesh::upload(&device, &queue, &meshes[0]);
    // Up and down the "levels", drawing each and destroying it (by
    // dropping it) right after its frame is submitted, before the GPU is
    // done with it.
    for step in 0..16 {
        let mesh = &meshes[[0, 1, 2, 3, 2, 1][step % 6]];
        gpu = GpuMesh::upload(&device, &queue, mesh);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
            clay.draw(&mut pass, Slot::SculptView, &gpu);
        }
        queue.submit([encoder.finish()]);
    }
    let last = draw(&device, &queue, &clay, &target, &gpu);
    drop(gpu);
    assert!(last.chunks(4).any(|p| p != VIEW_BG.to_array()));
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{error:?}");
}

/// What drawing the clay costs the GPU at each size of the start clay's
/// levels, back faces then front faces as `ClayPass::draw` does.
/// `cargo test --release -p splatstudy-core --test upload draw_timing -- --ignored --nocapture`
#[test]
#[ignore]
fn draw_timing() {
    use splatstudy_core::sculpt::{START_LEVEL, cube_sphere};
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping upload test");
        return;
    };
    let target = ViewTarget::new(&device, [1000, 800]);
    let clay = ClayPass::new(&device);
    let (mesh, _) = cube_sphere(START_LEVEL);
    let mut sizes = vec![mesh];
    for _ in 0..2 {
        let mut s = splatstudy_core::sculpt::Sculpt::new(sizes.last().unwrap().clone());
        s.subdivide();
        sizes.push(s.mesh.clone());
    }
    for mesh in &sizes {
        let gpu = GpuMesh::upload(&device, &queue, mesh);
        draw(&device, &queue, &clay, &target, &gpu);
        let start = std::time::Instant::now();
        const FRAMES: u32 = 40;
        for _ in 0..FRAMES {
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
                clay.draw(&mut pass, Slot::SculptView, &gpu);
            }
            queue.submit([encoder.finish()]);
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        println!(
            "{} vertices: {:.2} ms a frame",
            mesh.positions.len(),
            start.elapsed().as_secs_f64() * 1e3 / f64::from(FRAMES)
        );
    }
}
