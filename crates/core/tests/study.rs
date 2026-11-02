// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Offscreen renders of the study aids: the silhouette and the ghost.
//! Skipped (with a message) on machines without any GPU adapter.

use egui::Color32;
use glam::Vec3;
use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Look};
use splatstudy_core::render::{Slot, ViewTarget, readback::Readback};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::{SILHOUETTE_INK, VIEW_BG};

const SIZE: u32 = 96;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// A unit sphere moved to `centre`.
fn sphere(centre: Vec3) -> Mesh {
    let mut mesh = Mesh::icosphere(3);
    for p in &mut mesh.positions {
        *p += centre;
    }
    mesh
}

/// The camera every render here uses: straight from the front, so a
/// point's pixel is easy to tell.
fn camera() -> OrbitCamera {
    OrbitCamera {
        pitch: 0.0,
        ..OrbitCamera::default()
    }
}

/// The pixel a world point lands on.
fn pixel_of(point: Vec3) -> (u32, u32) {
    let cam = camera();
    let ndc = (cam.projection(1.0) * cam.view()).project_point3(point);
    let x = (ndc.x + 1.0) / 2.0 * SIZE as f32;
    let y = (1.0 - ndc.y) / 2.0 * SIZE as f32;
    (x as u32, y as u32)
}

/// Draws the clay sphere at the origin with `clay`, then `ghost` (a mesh
/// and its look) over it, all in one slot and one submit, and reads the
/// pixels back.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    clay_look: &Look,
    ghost: Option<(&Mesh, &Look)>,
) -> Vec<u8> {
    let target = ViewTarget::new(device, [SIZE, SIZE]);
    let clay = ClayPass::new(device);
    let mesh = GpuMesh::new(device, &sphere(Vec3::ZERO));
    let slot = Slot::SculptView;
    clay.prepare(queue, slot, &camera(), target.aspect(), clay_look);
    let ghost = ghost.map(|(mesh, look)| {
        clay.prepare_ghost(queue, slot, &camera(), target.aspect(), look);
        GpuMesh::new(device, mesh)
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
        clay.draw(&mut pass, slot, &mesh);
        if let Some(ghost) = &ghost {
            clay.draw_ghost(&mut pass, slot, ghost);
        }
    }
    let readback = Readback::copy(device, &mut encoder, &target);
    queue.submit([encoder.finish()]);
    readback.map();
    readback.wait(device).expect("read back")
}

fn pixel(data: &[u8], (x, y): (u32, u32)) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

#[test]
fn the_silhouette_has_two_colours_only() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let data = render(&device, &queue, &Look::flat(SILHOUETTE_INK), None);
    let ink = SILHOUETTE_INK.to_array();
    let bg = VIEW_BG.to_array();
    let (mut inked, mut edges) = (0, 0);
    for p in data.chunks_exact(4) {
        // The colour goes to linear and back: allow one step of rounding.
        if p.iter().zip(ink).all(|(a, b)| a.abs_diff(b) <= 1) {
            inked += 1;
        } else if p != bg {
            // Multisampled edges mix the two: still grey, between them.
            assert!(p[0] == p[1] && p[1] == p[2] && p[0] < ink[0], "{p:?}");
            edges += 1;
        }
    }
    // The sphere fills about 9 % of the view; only its rim is in between.
    let disc = inked + edges;
    assert!(disc > 500, "{disc} pixels of sphere");
    assert!(edges * 6 < disc, "{edges} edge pixels of {disc}");
}

#[test]
fn the_ghost_mixes_with_the_clay_as_one_layer() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let grey = Color32::from_gray(128);
    // The stand-in reference: a sphere to the right and behind the clay,
    // so the clay is in front of it where they overlap.
    let reference = sphere(Vec3::new(1.0, 0.0, -0.6));
    let ghost_look = Look {
        opacity: 0.5,
        ..Look::flat(Color32::WHITE)
    };
    let data = render(
        &device,
        &queue,
        &Look::flat(grey),
        Some((&reference, &ghost_look)),
    );
    let red = |point: Vec3| pixel(&data, pixel_of(point))[0];

    // Clay alone: untouched.
    let alone = red(Vec3::new(-0.6, 0.0, 1.0));
    assert!((127..=129).contains(&alone), "clay alone {alone}");
    // Both: half white over the clay, though the clay is in front.
    let both = red(Vec3::new(0.5, 0.0, 1.0));
    assert!((188..=195).contains(&both), "ghost over clay {both}");
    // The ghost alone: half white over black, once. Its back surface
    // would make it 191.
    let alone = red(Vec3::new(1.6, 0.0, 0.0));
    assert!((124..=131).contains(&alone), "ghost alone {alone}");
    // Outside both: the background.
    assert_eq!(pixel(&data, (2, 2)), VIEW_BG.to_array());
}

#[test]
fn the_clay_keeps_its_own_look_beside_the_ghost() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // The ghost far off to the side: the clay's uniforms are not the
    // ghost's, though both are in one slot.
    let reference = sphere(Vec3::new(30.0, 0.0, 0.0));
    let ghost_look = Look {
        opacity: 0.5,
        ..Look::flat(Color32::WHITE)
    };
    let data = render(
        &device,
        &queue,
        &Look::flat(SILHOUETTE_INK),
        Some((&reference, &ghost_look)),
    );
    let centre = pixel(&data, (SIZE / 2, SIZE / 2));
    let ink = SILHOUETTE_INK.to_array();
    assert!(
        centre.iter().zip(ink).all(|(a, b)| a.abs_diff(b) <= 1),
        "{centre:?}"
    );
}
