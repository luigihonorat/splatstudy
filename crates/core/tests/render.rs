// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Offscreen rendering: draw into a view target and read the pixels back.
//! Skipped (with a message) on machines without any GPU adapter.

use egui::Color32;
use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Look};
use splatstudy_core::render::{Slot, ViewTarget, readback::Readback};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::{CLAY, SILHOUETTE_INK, VIEW_BG};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// Renders a sphere with `look` into a `size` target and reads it back.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, size: [u32; 2], look: &Look) -> Vec<u8> {
    let target = ViewTarget::new(device, size);
    let clay = ClayPass::new(device);
    let mesh = GpuMesh::new(device, &Mesh::icosphere(3));
    clay.prepare(
        queue,
        Slot::SculptView,
        &OrbitCamera::default(),
        target.aspect(),
        look,
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
        clay.draw(&mut pass, Slot::SculptView, &mesh);
    }
    let readback = Readback::copy(device, &mut encoder, &target);
    queue.submit([encoder.finish()]);
    readback.map();
    let pixels = readback.wait(device).expect("read back");
    assert_eq!(pixels.len(), (size[0] * size[1] * 4) as usize);
    pixels
}

fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

#[test]
fn default_sphere_renders_lit_clay_on_the_background() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    const SIZE: u32 = 128;
    let data = render(&device, &queue, [SIZE, SIZE], &Look::lit(CLAY));

    let bg = VIEW_BG.to_array();
    assert_eq!(pixel(&data, SIZE, 2, 2), bg, "corner shows the background");
    let centre = pixel(&data, SIZE, SIZE / 2, SIZE / 2);
    assert_ne!(centre, bg, "the sphere covers the centre");
    // The key light comes from the upper left: that side is brighter.
    let lit = pixel(&data, SIZE, SIZE / 2 - 14, SIZE / 2 - 14)[0];
    let shaded = pixel(&data, SIZE, SIZE / 2 + 14, SIZE / 2 + 14)[0];
    assert!(
        lit > shaded,
        "upper left {lit} should be brighter than lower right {shaded}"
    );
}

#[test]
fn rows_that_are_not_aligned_read_back_in_place() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // 101 * 4 bytes per row is not a multiple of 256.
    let size = [101, 57];
    let data = render(&device, &queue, size, &Look::lit(CLAY));
    let bg = VIEW_BG.to_array();
    assert_eq!(pixel(&data, size[0], 0, 0), bg);
    assert_eq!(pixel(&data, size[0], size[0] - 1, size[1] - 1), bg);
    assert_ne!(pixel(&data, size[0], size[0] / 2, size[1] / 2), bg);
}

#[test]
fn flat_look_fills_the_silhouette_with_one_colour() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    const SIZE: u32 = 64;
    let look = Look {
        flat: true,
        ..Look::lit(SILHOUETTE_INK)
    };
    let data = render(&device, &queue, [SIZE, SIZE], &look);
    let ink = SILHOUETTE_INK.to_array();
    let bg = VIEW_BG.to_array();
    // Inside the sphere every pixel is the ink, whatever the light would do.
    for (x, y) in [(32, 32), (26, 26), (38, 38)] {
        let p = pixel(&data, SIZE, x, y);
        assert!(
            p.iter().zip(ink).all(|(a, b)| a.abs_diff(b) <= 1),
            "{p:?} at {x},{y}"
        );
    }
    // Outside, only the background: two colours in all (edges aside).
    assert_eq!(pixel(&data, SIZE, 1, 1), bg);
}

#[test]
fn see_through_look_blends_with_the_background() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    const SIZE: u32 = 64;
    let white = Color32::WHITE;
    let solid = render(
        &device,
        &queue,
        [SIZE, SIZE],
        &Look {
            flat: true,
            ..Look::lit(white)
        },
    );
    let ghost = render(
        &device,
        &queue,
        [SIZE, SIZE],
        &Look {
            flat: true,
            opacity: 0.5,
            ..Look::lit(white)
        },
    );
    let solid = pixel(&solid, SIZE, 32, 32)[0];
    let ghost = pixel(&ghost, SIZE, 32, 32)[0];
    assert_eq!(solid, 255);
    // Back and front of the sphere each blend half of white over what is
    // behind (in stored sRGB values): 0.5, then 0.5 + 0.5 * 0.5.
    assert!((185..=197).contains(&ghost), "ghost {ghost}");
}

/// Pixels inside the sphere that stand out from both pairs of neighbours,
/// away from its outline: cracks or seams in the shading.
fn outliers(data: &[u8], [width, height]: [u32; 2]) -> usize {
    let at = |x: u32, y: u32| pixel(data, width, x, y)[1] as i32;
    let bg = VIEW_BG.to_array()[1] as i32;
    let mut count = 0;
    for y in 5..height - 5 {
        for x in 5..width - 5 {
            let c = at(x, y);
            let near_bg = [(5, 0), (0, 5), (1, 0), (0, 1)]
                .iter()
                .any(|&(dx, dy)| at(x - dx, y - dy) == bg || at(x + dx, y + dy) == bg);
            if near_bg || c == bg {
                continue;
            }
            let h = (at(x - 1, y) + at(x + 1, y)) / 2;
            let v = (at(x, y - 1) + at(x, y + 1)) / 2;
            if (c - h).abs() > 6 && (c - v).abs() > 6 {
                count += 1;
            }
        }
    }
    count
}

/// The start clay and other subdivided clay render as smoothly as the old
/// icosphere. Drawn in one call with no culling, Direct3D 12 on NVIDIA
/// shaded some of their front pixels as back faces, in dashed lines (see
/// `ClayPass::draw`), so a DX12 adapter is used where there is one.
#[test]
fn subdivided_clay_renders_without_cracks() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let dx12 = pollster::block_on(instance.request_adapter(&Default::default()))
        .ok()
        .and_then(|a| pollster::block_on(a.request_device(&Default::default())).ok());
    let Some((device, queue)) = dx12.or_else(device) else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // The size of the sculpt view in a 1400 x 850 window.
    let size = [505, 819];
    let (cube, _) = splatstudy_core::sculpt::cube_sphere(splatstudy_core::sculpt::START_LEVEL);
    let mut subdivided = splatstudy_core::sculpt::Sculpt::new(Mesh::icosphere(4));
    subdivided.subdivide();
    subdivided.subdivide();
    for (name, mesh) in [
        ("icosphere", Mesh::icosphere(7)),
        ("cube sphere", cube),
        ("subdivided icosphere", subdivided.mesh.clone()),
        ("cube sphere 3", splatstudy_core::sculpt::cube_sphere(3).0),
        ("cube sphere 4", splatstudy_core::sculpt::cube_sphere(4).0),
        ("cube sphere 5", splatstudy_core::sculpt::cube_sphere(5).0),
        ("cube sphere 6", splatstudy_core::sculpt::cube_sphere(6).0),
    ] {
        let target = ViewTarget::new(&device, size);
        let clay = ClayPass::new(&device);
        let gpu = GpuMesh::new(&device, &mesh);
        let mut camera = OrbitCamera::default();
        for (yaw, pitch) in [(0.0, 0.0), (35.0, 30.0), (-60.0, -20.0)] {
            camera.orbit(yaw, pitch);
            clay.prepare(
                &queue,
                Slot::SculptView,
                &camera,
                target.aspect(),
                &Look::lit(CLAY),
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
                clay.draw(&mut pass, Slot::SculptView, &gpu);
            }
            let readback = Readback::copy(&device, &mut encoder, &target);
            queue.submit([encoder.finish()]);
            readback.map();
            let data = readback.wait(&device).expect("read back");
            let n = outliers(&data, size);
            assert!(n <= 2, "{name}, turned {yaw} {pitch}: {n} cracked pixels");
        }
    }
}
