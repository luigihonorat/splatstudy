// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Offscreen renders of the study aids: the silhouette, the ghost, the
//! silhouette match and the lighting.
//! Skipped (with a message) on machines without any GPU adapter.

use egui::Color32;
use glam::Vec3;
use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Lighting, Look, Overlap};
use splatstudy_core::render::{Slot, ViewTarget, readback::Readback};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::{CLAY, SILHOUETTE_INK, VIEW_BG};

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
    for p in data.as_chunks::<4>().0 {
        // The colour goes to linear and back: allow one step of rounding.
        if p.iter().zip(ink).all(|(a, b)| a.abs_diff(b) <= 1) {
            inked += 1;
        } else if *p != bg {
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

/// Renders two meshes flat, each into its own target of `size` from the
/// front camera, in one submit, as the silhouette match does, and reads
/// them back.
fn flat_renders(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    meshes: [&Mesh; 2],
) -> [Vec<u8>; 2] {
    let clay = ClayPass::new(device);
    let targets = [0, 1].map(|_| ViewTarget::new(device, size));
    let gpu = meshes.map(|m| GpuMesh::new(device, m));
    let slots = [Slot::SculptView, Slot::ReferenceView];
    let mut encoder = device.create_command_encoder(&Default::default());
    for ((target, mesh), slot) in targets.iter().zip(&gpu).zip(slots) {
        let ink = Look::flat(SILHOUETTE_INK);
        clay.prepare(queue, slot, &camera(), target.aspect(), &ink);
        let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
        clay.draw(&mut pass, slot, mesh);
    }
    let readbacks = targets
        .each_ref()
        .map(|t| Readback::copy(device, &mut encoder, t));
    queue.submit([encoder.finish()]);
    readbacks.map(|r| {
        r.map();
        r.wait(device).expect("read back")
    })
}

/// A unit sphere scaled by `s` about the origin.
fn scaled_sphere(s: f32) -> Mesh {
    let mut mesh = Mesh::icosphere(4);
    for p in &mut mesh.positions {
        *p *= s;
    }
    mesh
}

#[test]
fn the_silhouette_match_is_the_overlap_of_the_outlines() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let size = [256, 256];
    let unit = scaled_sphere(1.0);
    let [a, b] = flat_renders(&device, &queue, size, [&unit, &unit]);
    let same = Overlap::of(&a, &b);
    assert_eq!(same.clay_only + same.reference_only, 0);
    assert!(same.iou().unwrap() > 0.999);

    // A sphere at 80 %: its outline is a smaller disc inside the other.
    let small = scaled_sphere(0.8);
    let [clay, reference] = flat_renders(&device, &queue, size, [&small, &unit]);
    let overlap = Overlap::of(&clay, &reference);
    assert_eq!(overlap.clay_only, 0, "the smaller one lies inside");
    // Seen in perspective, a sphere of radius r at distance d fills a disc
    // of radius tan(asin(r / d)) on the image plane.
    let d = camera().distance;
    let disc = |r: f32| (r / d).asin().tan();
    let expected = (disc(0.8) / disc(1.0)).powi(2);
    let iou = overlap.iou().unwrap();
    assert!((iou - expected).abs() < 0.015, "{iou} vs {expected}");

    // Neither in view: no score.
    let away = sphere(Vec3::new(50.0, 0.0, 0.0));
    let [c, r] = flat_renders(&device, &queue, size, [&away, &away]);
    assert_eq!(Overlap::of(&c, &r).iou(), None);
}

/// The mean brightness of the sphere's pixels in each half of the view:
/// left, right, top and bottom.
fn halves(data: &[u8]) -> [f32; 4] {
    let mut sums = [0.0f32; 4];
    let mut counts = [0u32; 4];
    for (i, p) in data.as_chunks::<4>().0.iter().enumerate() {
        if *p == VIEW_BG.to_array() {
            continue;
        }
        let (x, y) = (i as u32 % SIZE, i as u32 / SIZE);
        let half = SIZE / 2;
        for (k, inside) in [x < half, x >= half, y < half, y >= half]
            .into_iter()
            .enumerate()
        {
            if inside {
                sums[k] += f32::from(p[0]);
                counts[k] += 1;
            }
        }
    }
    [0, 1, 2, 3].map(|k| sums[k] / counts[k].max(1) as f32)
}

#[test]
fn each_lighting_lights_the_clay_from_its_side() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let lit = |lighting: Lighting| {
        let look = Look::lit(CLAY).with_lighting(lighting);
        halves(&render(&device, &queue, &look, None))
    };
    let [left, right, top, bottom] = lit(Lighting::Studio);
    assert!(
        left > right && top > bottom,
        "studio: from above on the left"
    );
    let studio_side = left / right;

    let [left, right, top, bottom] = lit(Lighting::Raking);
    assert!(
        left / right > studio_side * 1.5,
        "raking: hard from the side, {left} / {right}"
    );
    assert!(
        (top / bottom - 1.0).abs() < 0.25,
        "raking: level, {top} / {bottom}"
    );

    let [left, right, top, bottom] = lit(Lighting::Top);
    assert!(top > bottom * 1.8, "top: {top} over {bottom}");
    assert!((left / right - 1.0).abs() < 0.05, "top: centred");
}
