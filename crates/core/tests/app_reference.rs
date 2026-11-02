// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The whole app, offscreen, with a reference loaded from a file event:
//! splats, a mesh and a picture each show in the reference view. Skipped
//! without a GPU adapter. Set `SPLATSTUDY_SHOTS` to a folder to also save
//! each frame as a PNG to look at.

use std::sync::Arc;

use glam::{Quat, Vec3};
use splatstudy_core::{App, FilePurpose, InputEvent, Lang};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Rows of 1280 pixels need no padding to copy.
const SIZE: [u32; 2] = [1280, 720];

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// A binary 3DGS PLY of a ring of flat splats, coloured around the ring
/// and stored upside down, as captures come.
fn ring_ply() -> Vec<u8> {
    let (major, minor) = (2.0f32, 0.7f32);
    let mut rows: Vec<[f32; 14]> = Vec::new();
    let (nu, nv) = (180, 48);
    for i in 0..nu {
        for j in 0..nv {
            let u = i as f32 / nu as f32 * std::f32::consts::TAU;
            let v = j as f32 / nv as f32 * std::f32::consts::TAU;
            let normal = Vec3::new(u.cos() * v.cos(), v.sin(), u.sin() * v.cos());
            let centre = Vec3::new(u.cos() * major, 0.0, u.sin() * major);
            // Stood up (the ring's axis along z), then upside down.
            let p = centre + normal * minor;
            let p = Vec3::new(p.x, -p.z, -p.y) + Vec3::new(5.0, 1.0, -3.0);
            let n = Vec3::new(normal.x, -normal.z, -normal.y);
            let q = Quat::from_rotation_arc(Vec3::Z, n);
            // f_dc from a colour: (c - 0.5) / 0.2821.
            let colour = [
                0.5 + 0.45 * u.cos(),
                0.5 + 0.45 * (u + 2.1).cos(),
                0.5 + 0.45 * (u + 4.2).cos(),
            ];
            let dc = colour.map(|c| (c - 0.5) / 0.282_094_8);
            let tangent = (0.035f32).ln();
            rows.push([
                p.x,
                p.y,
                p.z,
                dc[0],
                dc[1],
                dc[2],
                3.0,
                tangent,
                tangent,
                (0.004f32).ln(),
                q.w,
                q.x,
                q.y,
                q.z,
            ]);
        }
    }
    let mut out = format!(
        "ply\nformat binary_little_endian 1.0\nelement vertex {}\n",
        rows.len()
    )
    .into_bytes();
    for name in [
        "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
        "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        out.extend_from_slice(format!("property float {name}\n").as_bytes());
    }
    out.extend_from_slice(b"end_header\n");
    for row in rows {
        for v in row {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// An OBJ octahedron with vertex colours.
const OCTAHEDRON: &str = "v 1 0 0 1 0.2 0.2\nv -1 0 0 0.2 1 0.2\nv 0 1 0 0.2 0.2 1\n\
    v 0 -1 0 1 1 0.2\nv 0 0 1 1 0.2 1\nv 0 0 -1 0.2 1 1\n\
    f 1 3 5\nf 3 2 5\nf 2 4 5\nf 4 1 5\nf 3 1 6\nf 2 3 6\nf 4 2 6\nf 1 4 6\n";

/// A 64 × 48 picture: orange on the left half, blue on the right.
fn picture() -> Vec<u8> {
    let mut rgb = Vec::new();
    for _ in 0..48 {
        for x in 0..64 {
            rgb.extend_from_slice(if x < 32 {
                &[240, 140, 20]
            } else {
                &[20, 80, 230]
            });
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 64, 48);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&rgb).unwrap();
    writer.finish().unwrap();
    out
}

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    app: App,
    target: wgpu::Texture,
    time: f64,
}

impl Harness {
    fn new() -> Option<Self> {
        let (device, queue) = device()?;
        let app = App::new(device.clone(), queue.clone(), FORMAT, Lang::En);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Some(Self {
            device,
            queue,
            app,
            target,
            time: 0.0,
        })
    }

    fn open(&mut self, name: &str, bytes: Vec<u8>) {
        self.app.push_event(InputEvent::File {
            purpose: FilePurpose::Any,
            name: name.into(),
            bytes: Arc::from(bytes),
        });
    }

    /// Draws a few frames and reads the last one back.
    fn frame(&mut self, shot: &str) -> Vec<u8> {
        let view = self.target.create_view(&Default::default());
        for _ in 0..3 {
            self.time += 0.1;
            self.app.frame(self.time, &view, SIZE, 1.0);
        }
        let row = SIZE[0] * 4;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * SIZE[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let pixels = buffer.slice(..).get_mapped_range().unwrap().to_vec();
        if let Ok(folder) = std::env::var("SPLATSTUDY_SHOTS") {
            let file = std::fs::File::create(format!("{folder}/{shot}.png")).unwrap();
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), SIZE[0], SIZE[1]);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        }
        pixels
    }
}

/// The share of pixels in the reference view (the right half of the
/// middle, between the panels) for which `test` holds.
fn share_in_reference_view(pixels: &[u8], test: impl Fn([u8; 4]) -> bool) -> f32 {
    let (x0, x1) = (SIZE[0] / 2 + 20, SIZE[0] - 220);
    let (y0, y1) = (60, SIZE[1] - 60);
    let mut hits = 0;
    let mut total = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * SIZE[0] + x) * 4) as usize;
            total += 1;
            if test([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]) {
                hits += 1;
            }
        }
    }
    hits as f32 / total as f32
}

fn colourful(p: [u8; 4]) -> bool {
    let max = p[..3].iter().max().copied().unwrap_or(0);
    let min = p[..3].iter().min().copied().unwrap_or(0);
    max > 80 && max - min > 60
}

#[test]
fn splats_meshes_and_pictures_show_in_the_reference_view() {
    let Some(mut h) = Harness::new() else {
        eprintln!("no GPU adapter; skipping app test");
        return;
    };
    let empty = h.frame("empty");
    assert!(share_in_reference_view(&empty, colourful) < 0.001);

    h.open("ring.ply", ring_ply());
    let splats = h.frame("splats");
    let share = share_in_reference_view(&splats, colourful);
    assert!(share > 0.05, "the coloured ring shows: {share}");

    h.open("octahedron.obj", OCTAHEDRON.as_bytes().to_vec());
    let mesh = h.frame("mesh");
    let share = share_in_reference_view(&mesh, colourful);
    assert!(share > 0.03, "the coloured mesh shows: {share}");

    h.open("picture.png", picture());
    let image = h.frame("picture");
    let orange = share_in_reference_view(&image, |p| p[0] > 200 && p[1] > 100 && p[2] < 60);
    let blue = share_in_reference_view(&image, |p| p[2] > 200 && p[0] < 60);
    assert!(orange > 0.1 && blue > 0.1, "{orange} {blue}");

    // A broken file leaves the picture in place.
    h.open(
        "broken.ply",
        b"ply\nformat ascii 1.0\nend_header\n".to_vec(),
    );
    let after = h.frame("broken");
    let orange = share_in_reference_view(&after, |p| p[0] > 200 && p[1] > 100 && p[2] < 60);
    assert!(orange > 0.1);
}
