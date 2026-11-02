// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Offscreen splat rendering: draw splats (and clay) into a view target and
//! read the pixels back. Skipped (with a message) without a GPU adapter.

use glam::{Mat4, Quat, Vec3};
use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::reference::{Aabb, Splats};
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Look};
use splatstudy_core::render::splat::{SortOn, SplatLook, SplatPass};
use splatstudy_core::render::{Slot, ViewTarget, readback::Readback};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::{CLAY, GHOST_OPACITY, SILHOUETTE_INK, VIEW_BG};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn splats(list: &[(Vec3, f32, [u8; 4])]) -> Splats {
    Splats {
        positions: list.iter().map(|s| s.0).collect(),
        scales: list.iter().map(|s| Vec3::splat(s.1)).collect(),
        rotations: vec![Quat::IDENTITY; list.len()],
        colors: list.iter().map(|s| s.2).collect(),
    }
}

struct Scene<'a> {
    splats: &'a Splats,
    model: Mat4,
    crop: Option<Aabb>,
    /// A clay sphere of this radius at this point, drawn first.
    clay: Option<(Vec3, f32)>,
    opacity: f32,
    flat: bool,
    over: bool,
    sort: SortOn,
}

impl<'a> Scene<'a> {
    fn new(splats: &'a Splats) -> Self {
        Self {
            splats,
            model: Mat4::IDENTITY,
            crop: None,
            clay: None,
            opacity: 1.0,
            flat: false,
            over: false,
            sort: SortOn::Gpu,
        }
    }
}

const SIZE: u32 = 96;

fn render(device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene) -> Vec<u8> {
    let target = ViewTarget::new(device, [SIZE, SIZE]);
    let pass = SplatPass::sorting_on(device, scene.splats, scene.sort).expect("splats");
    let camera = OrbitCamera {
        pitch: 0.0,
        ..OrbitCamera::default()
    };
    pass.set_placement(scene.model, scene.crop);
    pass.prepare(
        queue,
        Slot::ReferenceView,
        &camera,
        target.size,
        &SplatLook {
            opacity: scene.opacity,
            flat: scene.flat,
            over: scene.over,
        },
    );
    let clay = ClayPass::new(device);
    let mesh = scene.clay.map(|(centre, radius)| {
        let mut sphere = Mesh::icosphere(3);
        for p in &mut sphere.positions {
            *p = centre + *p * radius;
        }
        GpuMesh::new(device, &sphere)
    });
    clay.prepare(
        queue,
        Slot::ReferenceView,
        &camera,
        target.aspect(),
        &Look::lit(CLAY),
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut rpass = target.begin_pass(&mut encoder, VIEW_BG);
        if let Some(mesh) = &mesh {
            clay.draw(&mut rpass, Slot::ReferenceView, mesh);
        }
        pass.draw(&mut rpass, Slot::ReferenceView);
    }
    let readback = Readback::copy(device, &mut encoder, &target);
    queue.submit([encoder.finish()]);
    readback.map();
    readback.wait(device).expect("read back")
}

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

fn close(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    a.iter()
        .zip(b)
        .take(3)
        .all(|(x, y)| x.abs_diff(y) <= tolerance)
}

const RED: [u8; 4] = [230, 40, 20, 255];
const BLUE: [u8; 4] = [20, 60, 240, 255];

#[test]
fn a_single_splat_shows_its_colour_at_its_centre_and_fades_out() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::ZERO, 0.4, RED)]);
    let data = render(&device, &queue, &Scene::new(&s));
    let c = SIZE / 2;
    assert!(
        close(pixel(&data, c, c), RED, 3),
        "{:?}",
        pixel(&data, c, c)
    );
    // Gaussian falloff: dimmer away from the centre, gone at the corner.
    let side = pixel(&data, c + 12, c);
    assert!(side[0] < RED[0] - 20 && side[0] > 20, "{side:?}");
    assert_eq!(pixel(&data, 1, 1), VIEW_BG.to_array());
}

#[test]
fn nearer_splats_cover_farther_ones_whatever_the_order_given() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // The camera is on +Z: z = 1 is nearer.
    for list in [
        [
            (Vec3::new(0.0, 0.0, -1.0), 0.4, RED),
            (Vec3::new(0.0, 0.0, 1.0), 0.4, BLUE),
        ],
        [
            (Vec3::new(0.0, 0.0, 1.0), 0.4, BLUE),
            (Vec3::new(0.0, 0.0, -1.0), 0.4, RED),
        ],
    ] {
        let s = splats(&list);
        let data = render(&device, &queue, &Scene::new(&s));
        let centre = pixel(&data, SIZE / 2, SIZE / 2);
        assert!(close(centre, BLUE, 3), "{centre:?}");
    }
}

#[test]
fn clay_in_front_hides_a_splat_and_clay_behind_does_not() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::ZERO, 0.4, RED)]);
    let c = SIZE / 2;
    let front = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, 1.5), 0.3)),
            ..Scene::new(&s)
        },
    );
    let p = pixel(&front, c, c);
    assert!(p[0].abs_diff(p[2]) < 10, "clay grey, not red: {p:?}");
    assert!(p[0] > 60, "lit clay: {p:?}");
    let behind = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, -1.5), 0.3)),
            ..Scene::new(&s)
        },
    );
    assert!(
        close(pixel(&behind, c, c), RED, 3),
        "{:?}",
        pixel(&behind, c, c)
    );
}

#[test]
fn placement_crop_ghost_and_silhouette() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::new(10.0, 0.0, 0.0), 4.0, RED)]);
    let c = SIZE / 2;
    // Centred and scaled down into the clay's space, it shows in the middle.
    let model =
        Mat4::from_scale(Vec3::splat(0.1)) * Mat4::from_translation(Vec3::new(-10.0, 0.0, 0.0));
    let placed = Scene {
        model,
        ..Scene::new(&s)
    };
    let data = render(&device, &queue, &placed);
    assert!(close(pixel(&data, c, c), RED, 3));

    // Cropped away.
    let cropped = Scene {
        crop: Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        }),
        ..placed
    };
    let data = render(&device, &queue, &cropped);
    assert_eq!(pixel(&data, c, c), VIEW_BG.to_array());

    // Half see-through.
    let ghost = Scene {
        opacity: 0.5,
        model,
        ..Scene::new(&s)
    };
    let data = render(&device, &queue, &ghost);
    let p = pixel(&data, c, c);
    assert!(p[0].abs_diff(RED[0] / 2) <= 3, "{p:?}");

    // Silhouette.
    let flat = Scene {
        flat: true,
        model,
        ..Scene::new(&s)
    };
    let data = render(&device, &queue, &flat);
    assert!(close(pixel(&data, c, c), SILHOUETTE_INK.to_array(), 3));
}

#[test]
fn a_long_splat_is_drawn_along_its_axis() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // Long in x, thin in y and z.
    let s = Splats {
        positions: vec![Vec3::ZERO],
        scales: vec![Vec3::new(0.6, 0.03, 0.03)],
        rotations: vec![Quat::IDENTITY],
        colors: vec![RED],
    };
    let c = SIZE / 2;
    let data = render(&device, &queue, &Scene::new(&s));
    assert!(pixel(&data, c + 15, c)[0] > 100, "along the axis");
    assert_eq!(pixel(&data, c, c + 15), VIEW_BG.to_array(), "across it");
    // Turned a quarter about z, it stands up.
    let turned = Splats {
        rotations: vec![Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)],
        ..s
    };
    let data = render(&device, &queue, &Scene::new(&turned));
    assert!(pixel(&data, c, c + 15)[0] > 100);
    assert_eq!(pixel(&data, c + 15, c), VIEW_BG.to_array());
}

#[test]
fn a_ghost_shows_over_clay_in_front() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::ZERO, 0.4, RED)]);
    let c = SIZE / 2;
    let clay_only = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, 1.5), 0.3)),
            opacity: 0.0,
            ..Scene::new(&s)
        },
    );
    let ghost = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, 1.5), 0.3)),
            opacity: 0.5,
            over: true,
            ..Scene::new(&s)
        },
    );
    let (clay, mixed) = (pixel(&clay_only, c, c), pixel(&ghost, c, c));
    // Half red over the grey clay: redder, and less blue.
    assert!(
        mixed[0] > clay[0] && mixed[2] < clay[2],
        "{clay:?} → {mixed:?}"
    );
}

#[test]
fn a_ghost_of_overlapping_splats_is_one_layer() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let ghost = |list: &[(Vec3, f32, [u8; 4])]| {
        let s = splats(list);
        render(
            &device,
            &queue,
            &Scene {
                opacity: GHOST_OPACITY,
                over: true,
                ..Scene::new(&s)
            },
        )
    };
    let one = ghost(&[(Vec3::ZERO, 0.4, RED)]);
    // Five splats, one behind the other.
    let stack: Vec<_> = (0..5)
        .map(|i| (Vec3::new(0.0, 0.0, i as f32 * -0.1), 0.4, RED))
        .collect();
    let five = ghost(&stack);
    let c = SIZE / 2;
    let (p, q) = (pixel(&one, c, c), pixel(&five, c, c));
    assert!(close(p, q, 3), "one {p:?}, five {q:?}");
    // And that one layer is the reference faded to the ghost's opacity.
    let bg = VIEW_BG.to_array();
    for k in 0..3 {
        let (from, to) = (f32::from(bg[k]), f32::from(RED[k]));
        let want = from + (to - from) * GHOST_OPACITY;
        assert!((f32::from(p[k]) - want).abs() <= 3.0, "{p:?}, channel {k}");
    }
}

/// `n` overlapping splats of many colours, in no particular order.
fn crowd(n: usize) -> Splats {
    let mut seed = 0x2545_F491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let list: Vec<_> = (0..n)
        .map(|_| {
            let p = Vec3::new(next() - 0.5, next() - 0.5, next() * 2.0 - 1.0);
            let colour = [
                (next() * 255.0) as u8,
                (next() * 255.0) as u8,
                (next() * 255.0) as u8,
                200,
            ];
            (p, 0.05 + next() * 0.1, colour)
        })
        .collect();
    splats(&list)
}

#[test]
fn the_gpu_sorts_like_the_cpu() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = crowd(400);
    if SplatPass::new(&device, &s).map(|p| p.sorts_on()) != Some(SortOn::Gpu) {
        eprintln!("no compute shaders; the GPU sort isn't used here");
        return;
    }
    let gpu = render(&device, &queue, &Scene::new(&s));
    let cpu = render(
        &device,
        &queue,
        &Scene {
            sort: SortOn::Cpu,
            ..Scene::new(&s)
        },
    );
    let (gpu_px, cpu_px) = (gpu.as_chunks::<4>().0, cpu.as_chunks::<4>().0);
    let differ = gpu_px
        .iter()
        .zip(cpu_px)
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 2))
        .count();
    // The same but where two splats share a depth step, if any do.
    assert!(
        differ <= (SIZE * SIZE / 200) as usize,
        "{differ} pixels differ"
    );
    // Not an empty picture: most of it is covered.
    let bg = VIEW_BG.to_array();
    let covered = gpu_px.iter().filter(|p| p[..3] != bg[..3]).count();
    assert!(covered > (SIZE * SIZE / 4) as usize, "{covered}");
}

/// A 3DGS capture's worth of splats: a lumpy shell of radius about 1 with
/// a few floaters, small turned splats, every colour.
fn capture(n: usize) -> Splats {
    let mut seed = 0x9E37_79B9_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let mut s = Splats::default();
    for _ in 0..n {
        let dir = Vec3::new(next() - 0.5, next() - 0.5, next() - 0.5).normalize_or(Vec3::Y);
        // One in a hundred floats away, at random rather than every
        // hundredth, which the fitting's even sampling could alias with.
        let r = if next() < 0.01 {
            3.0 + next() * 20.0
        } else {
            0.9 + next() * 0.2
        };
        s.positions.push(dir * r);
        s.scales
            .push(Vec3::new(next(), next(), next() * 0.2) * 0.01 + Vec3::splat(0.001));
        s.rotations
            .push(Quat::from_xyzw(next() - 0.5, next() - 0.5, next() - 0.5, next()).normalize());
        s.colors.push([
            (next() * 255.0) as u8,
            (next() * 255.0) as u8,
            (next() * 255.0) as u8,
            (next() * 255.0) as u8,
        ]);
    }
    s
}

/// A binary 3DGS PLY of `n` splats, with the 45 higher spherical harmonics
/// such files carry (62 floats, 248 bytes a splat) when `higher`.
fn capture_ply(n: usize, higher: bool) -> Vec<u8> {
    let rest = if higher { 45 } else { 0 };
    let mut header = format!("ply\nformat binary_little_endian 1.0\nelement vertex {n}\n");
    let mut names: Vec<String> = ["x", "y", "z", "nx", "ny", "nz"].map(String::from).to_vec();
    names.extend((0..3).map(|i| format!("f_dc_{i}")));
    names.extend((0..rest).map(|i| format!("f_rest_{i}")));
    names.push("opacity".into());
    names.extend((0..3).map(|i| format!("scale_{i}")));
    names.extend((0..4).map(|i| format!("rot_{i}")));
    for name in &names {
        header += &format!("property float {name}\n");
    }
    header += "end_header\n";
    let s = capture(n);
    let mut out = header.into_bytes();
    out.reserve(n * names.len() * 4);
    for i in 0..n {
        let p = s.positions[i];
        let q = s.rotations[i];
        let mut row = vec![p.x, p.y, p.z, 0.0, 0.0, 0.0];
        row.extend(
            s.colors[i][..3]
                .iter()
                .map(|&c| (f32::from(c) / 255.0 - 0.5) / 0.282),
        );
        row.extend(std::iter::repeat_n(0.01, rest));
        row.push(0.5);
        row.extend(s.scales[i].to_array().map(f32::ln));
        row.extend([q.w, q.x, q.y, q.z]);
        for v in row {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Timings for large captures, by hand in release:
/// `cargo test --release -p splatstudy-core --test splats -- --ignored --nocapture`.
#[test]
#[ignore = "timings, not a check; run by hand in release"]
fn timings_of_large_captures() {
    use splatstudy_core::reference::load::load_ply;
    use splatstudy_core::reference::{Content, Reference, Source};
    use splatstudy_core::render::splat::Sorter;
    use std::time::Instant;

    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;

    // Loading a 200 MB file.
    let n = 200 * 1000 * 1000 / 248;
    let bytes = capture_ply(n, true);
    // For timing the browser too: SPLAT_TIMING_PLY=<path> keeps the file,
    // and <path>.2m.ply gets 2M splats without the higher harmonics.
    if let Some(path) = std::env::var_os("SPLAT_TIMING_PLY") {
        std::fs::write(&path, &bytes).expect("writes the file");
        let mut two = path;
        two.push(".2m.ply");
        std::fs::write(two, capture_ply(2_000_000, false)).expect("writes the file");
    }
    let t = Instant::now();
    let content = load_ply(&bytes).expect("loads");
    let parse = ms(t);
    let t = Instant::now();
    let source = Source {
        name: "capture.ply".into(),
        bytes: bytes.into(),
    };
    let mut reference = Reference::new(source, content);
    reference.fit();
    let fit = ms(t);
    println!(
        "load {:.0} MB, {n} splats: parse {parse:.0} ms, fit {fit:.0} ms",
        reference.source.bytes.len() as f64 / 1e6
    );
    let Content::Splats(loaded) = &reference.content else {
        panic!("splats");
    };

    let Some((device, queue)) = device() else {
        println!("no GPU adapter; no GPU timings");
        return;
    };
    let t = Instant::now();
    let pass = SplatPass::new(&device, loaded).expect("splats");
    println!("upload {} splats: {:.0} ms", pass.len(), ms(t));
    drop(pass);

    for n in [1_000_000, 2_000_000] {
        let s = capture(n);
        let mut sorter = Sorter::default();
        let rows: Vec<Vec3> = (0..10)
            .map(|i| Vec3::new((i as f32).sin(), 0.3, (i as f32).cos()).normalize())
            .collect();
        let t = Instant::now();
        for row in &rows {
            std::hint::black_box(sorter.back_to_front(&s.positions, *row));
        }
        println!("{n} splats: CPU sort alone {:.1} ms", ms(t) / 10.0);

        let size = [1280, 800];
        let target = ViewTarget::new(&device, size);
        for sort in [SortOn::Cpu, SortOn::Gpu] {
            let pass = SplatPass::sorting_on(&device, &s, sort).expect("splats");
            let frame = |yaw: f32, draw: bool| {
                let camera = OrbitCamera {
                    yaw,
                    ..OrbitCamera::default()
                };
                let t = Instant::now();
                pass.prepare(
                    &queue,
                    Slot::ReferenceView,
                    &camera,
                    size,
                    &SplatLook::default(),
                );
                let prepare = ms(t);
                let mut encoder = device.create_command_encoder(&Default::default());
                if draw {
                    let mut rpass = target.begin_pass(&mut encoder, VIEW_BG);
                    pass.draw(&mut rpass, Slot::ReferenceView);
                }
                queue.submit([encoder.finish()]);
                device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
                (prepare, ms(t))
            };
            frame(0.0, true); // warm up
            let (mut sort_cpu, mut sort_all, mut frame_cpu, mut frame_all) = (0.0, 0.0, 0.0, 0.0);
            for i in 1..=10 {
                let (a, b) = frame(i as f32 * 0.1, false);
                sort_cpu += a / 10.0;
                sort_all += b / 10.0;
                let (a, b) = frame(i as f32 * 0.1 + 0.05, true);
                frame_cpu += a / 10.0;
                frame_all += b / 10.0;
            }
            println!(
                "{n} splats, {sort:?} sort: sort {sort_all:.1} ms ({sort_cpu:.1} on the CPU); \
                 sort and draw {frame_all:.1} ms ({frame_cpu:.1} on the CPU)"
            );
        }
    }
}
