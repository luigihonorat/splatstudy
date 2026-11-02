// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Offscreen rendering: draw into a view target and read the pixels back.
//! Skipped (with a message) on machines without any GPU adapter.

use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::render::{ViewTarget, clay::ClayPass, clay::GpuMesh};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::LIGHT;

const SIZE: u32 = 128; // 128 * 4 bytes per row is a multiple of 256, as copies require

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, target: &ViewTarget) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.resolved.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped")
        .to_vec()
}

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

#[test]
fn default_sphere_renders_lit_clay_on_the_background() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let target = ViewTarget::new(&device, [SIZE, SIZE]);
    let clay = ClayPass::new(&device);
    let mesh = GpuMesh::new(&device, &Mesh::icosphere(3));
    let camera = OrbitCamera::default();

    clay.prepare(&queue, &camera, target.aspect(), LIGHT.vp_geo);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = target.begin_pass(&mut encoder, LIGHT.vp_bg);
        clay.draw(&mut pass, &mesh);
    }
    queue.submit([encoder.finish()]);
    let data = read_back(&device, &queue, &target);

    let bg = LIGHT.vp_bg.to_array();
    assert_eq!(pixel(&data, 2, 2), bg, "corner shows the background");
    let centre = pixel(&data, SIZE / 2, SIZE / 2);
    assert_ne!(centre, bg, "the sphere covers the centre");
    // The key light comes from the upper left: that side is brighter.
    let lit = pixel(&data, SIZE / 2 - 14, SIZE / 2 - 14)[0];
    let shaded = pixel(&data, SIZE / 2 + 14, SIZE / 2 + 14)[0];
    assert!(
        lit > shaded,
        "upper left {lit} should be brighter than lower right {shaded}"
    );
}
