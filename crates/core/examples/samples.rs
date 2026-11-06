// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Makes the bundled samples and their gallery thumbnails, with the app's
//! own loaders and renderer, so both can be made again the same way.
//! Run through xtask:
//!
//!   cargo xtask samples convert <folder>   the sources in <folder> → web/samples/*.ply
//!   cargo xtask samples thumbs             web/samples/*.ply → web/samples/thumbs/<id>.png
//!
//! `convert` reads the files as their museums publish them (see
//! [`SOURCES`]; download them into one folder first), welds the corners
//! the formats repeat, stands them Y up and writes binary PLY: positions
//! and triangles, nothing else changed. `thumbs` draws each sample as the
//! reference view does (clay colour, studio light, on the views' black)
//! from a three-quarter view, and writes a small grey PNG.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use glam::{Mat4, Quat, Vec3};
use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::meshio::Soup;
use splatstudy_core::reference::load::{self, load_obj, load_stl};
use splatstudy_core::reference::samples::SAMPLES;
use splatstudy_core::reference::{Content, Reference, Source as RefSource};
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Look};
use splatstudy_core::render::readback::Readback;
use splatstudy_core::render::{Slot, ViewTarget};
use splatstudy_core::ui::theme::{CLAY, VIEW_BG};

/// Where a sample comes from.
struct Source {
    /// The file under `web/samples/`.
    file: &'static str,
    /// The downloaded files, joined into one mesh.
    parts: &'static [Part],
    /// The sources stand Z up (SMK's STLs) and are turned to Y up.
    z_up: bool,
}

/// One downloaded file, and where its museum's own viewer places it.
struct Part {
    file: &'static str,
    /// A turn (x, y, z, w), then a move, applied to its vertices.
    rotation: [f32; 4],
    translation: [f32; 3],
}

/// A file used as it is.
const fn part(file: &'static str) -> Part {
    Part {
        file,
        rotation: [0.0, 0.0, 0.0, 1.0],
        translation: [0.0; 3],
    }
}

const SMK: &str = "https://api.smk.dk/api/v1/download-3d/";
const LINCOLN: &str = "https://3d-api.si.edu/content/document/3d_package:d8c642d6-4ebc-11ea-b77f-2e728ce88125/resources/lincoln_hands-150k-4096-obj.zip";

/// SMK's `_small` STLs (from each record's `files_3D` in the SMK API,
/// `api.smk.dk/api/v1/art/?object_number=<KAS no.>`), and the Smithsonian's
/// 150k OBJs of the Lincoln hands, unzipped (the textures are left out).
const SOURCES: &[Source] = &[
    Source {
        file: "brutus.ply",
        parts: &[part("df65vd436_KAS105_small.stl")],
        z_up: true,
    },
    Source {
        file: "venus-de-milo.ply",
        parts: &[part("h128nk52f_KAS434_small.stl")],
        z_up: true,
    },
    Source {
        file: "head-of-david.ply",
        parts: &[part("nz806401j_KAS2232_small.stl")],
        z_up: true,
    },
    Source {
        file: "homer.ply",
        parts: &[part("ms35tf42j_KAS210_small.stl")],
        z_up: true,
    },
    Source {
        file: "socrates.ply",
        parts: &[part("8g84ms153_KAS635_small.stl")],
        z_up: true,
    },
    Source {
        file: "julius-caesar.ply",
        parts: &[part("5999n821n_KAS297_small.stl")],
        z_up: true,
    },
    Source {
        file: "farnese-hercules-head.ply",
        parts: &[part("nk322j800_KAS701_small.stl")],
        z_up: true,
    },
    Source {
        file: "athlete-right-foot.ply",
        parts: &[part("05741x193_KAS27_small.stl")],
        z_up: true,
    },
    Source {
        file: "lincoln-hands.ply",
        // As the Smithsonian's own viewer shows them (the model nodes of
        // scene.svx.json, in the same package): resting side by side.
        parts: &[
            Part {
                file: "npg_71_6_left-hires_unwrapped-150k-unwrapped.obj",
                rotation: [-0.500_147_5, -0.499_161_4, -0.122_185_9, 0.696_965_5],
                translation: [-416.047_9, 655.028, 34.887_69],
            },
            Part {
                file: "npg_71_6_right-hires_unwrapped-150k-unwrapped.obj",
                rotation: [-0.457_809_6, -0.488_704_7, -0.028_220_5, 0.742_146_7],
                translation: [-726.126, 405.564, 178.872_73],
            },
        ],
        z_up: false,
    },
];

/// The thumbnails' size in pixels, drawn at twice that and halved.
const THUMB: u32 = 256;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = match args.first().map(String::as_str) {
        Some("convert") => match args.get(1) {
            Some(folder) => convert(Path::new(folder), &root.join("web/samples"), &args[2..]),
            None => Err("usage: convert <folder with the sources> [file…]".into()),
        },
        Some("thumbs") => thumbs(&root.join("web/samples"), &args[1..]),
        _ => Err("usage: samples convert <folder> [file…] | thumbs [id…]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("samples: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Converts every source found in `folder` (or only the `only` files).
fn convert(folder: &Path, out: &Path, only: &[String]) -> Result<(), String> {
    println!("sources: {SMK}<file> and {LINCOLN}");
    for source in SOURCES {
        if !only.is_empty() && !only.iter().any(|f| f == source.file) {
            continue;
        }
        let paths: Vec<PathBuf> = source.parts.iter().map(|p| folder.join(p.file)).collect();
        if let Some(missing) = paths.iter().find(|p| !p.exists()) {
            println!("{}: skipped, no {}", source.file, missing.display());
            continue;
        }
        let mut soup = Soup::default();
        for (path, part) in paths.iter().zip(source.parts) {
            let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name = path.to_string_lossy();
            let mesh = if name.ends_with(".stl") {
                load_stl(&bytes)
            } else {
                load_obj(&bytes)
            }
            .map_err(|e| format!("{name}: {e}"))?;
            let base = soup.positions.len() as u32;
            let place = Mat4::from_rotation_translation(
                Quat::from_array(part.rotation).normalize(),
                Vec3::from_array(part.translation),
            );
            soup.positions
                .extend(mesh.positions.iter().map(|&p| place.transform_point3(p)));
            soup.triangles
                .extend(mesh.triangles.iter().map(|t| t.map(|i| i + base)));
        }
        let mut soup = soup.weld();
        if source.z_up {
            // As the app's "Z up": a quarter turn about X.
            for p in &mut soup.positions {
                *p = Vec3::new(p.x, p.z, -p.y);
            }
        }
        let bytes = ply(&soup);
        let to = out.join(source.file);
        fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        println!(
            "{}: {} vertices, {} triangles, {:.2} MiB",
            source.file,
            soup.positions.len(),
            soup.triangles.len(),
            bytes.len() as f64 / (1024.0 * 1024.0)
        );
    }
    Ok(())
}

/// A binary PLY of positions and triangles.
fn ply(soup: &Soup) -> Vec<u8> {
    let mut out = format!(
        "ply\nformat binary_little_endian 1.0\nelement vertex {}\n\
         property float x\nproperty float y\nproperty float z\n\
         element face {}\nproperty list uchar int vertex_indices\nend_header\n",
        soup.positions.len(),
        soup.triangles.len()
    )
    .into_bytes();
    out.reserve(soup.positions.len() * 12 + soup.triangles.len() * 13);
    for p in &soup.positions {
        for v in p.to_array() {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    for t in &soup.triangles {
        out.push(3);
        for i in t {
            out.extend_from_slice(&(*i as i32).to_le_bytes());
        }
    }
    out
}

/// Draws every sample (or only the `only` ids) into `samples/thumbs/`.
fn thumbs(samples: &Path, only: &[String]) -> Result<(), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
        .map_err(|e| format!("no device: {e}"))?;
    let folder = samples.join("thumbs");
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let clay = ClayPass::new(&device);
    for sample in SAMPLES {
        if !only.is_empty() && !only.iter().any(|id| id == sample.id) {
            continue;
        }
        let path = samples.join(sample.file);
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let content =
            load::load(sample.file, &bytes).map_err(|e| format!("{}: {e}", sample.file))?;
        let mut reference = Reference::new(
            RefSource {
                name: sample.file.into(),
                bytes: bytes.into(),
            },
            content,
        );
        // Placed as the app places a sample it loads.
        reference.align.up = load::usual_up(sample.file, &reference.content);
        reference.fit();
        let Some((positions, triangles)) = reference.world_triangles() else {
            return Err(format!("{}: not a mesh", sample.file));
        };
        let colors = match &reference.content {
            Content::Mesh(m) => m.colors.as_deref(),
            _ => None,
        };
        let mesh = GpuMesh::from_triangles(&device, &positions, &triangles, colors);
        let camera = three_quarter(&positions);

        let size = THUMB * 2;
        let target = ViewTarget::new(&device, [size, size]);
        clay.prepare(&queue, Slot::ReferenceView, &camera, 1.0, &Look::lit(CLAY));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = target.begin_pass(&mut encoder, VIEW_BG);
            clay.draw(&mut pass, Slot::ReferenceView, &mesh);
        }
        let readback = Readback::copy(&device, &mut encoder, &target);
        queue.submit([encoder.finish()]);
        readback.map();
        let pixels = readback
            .wait(&device)
            .map_err(|_| "could not read the frame back")?;
        let grey = halve_to_grey(&pixels, size);
        let to = folder.join(format!("{}.png", sample.id));
        write_png(&to, &grey, THUMB)?;
        println!(
            "{}: {:.1} KiB",
            to.display(),
            fs::metadata(&to).map_or(0, |m| m.len()) as f64 / 1024.0
        );
    }
    Ok(())
}

/// A camera from the front left and a little above, framing `points` (in
/// the clay's space) with a small margin.
fn three_quarter(points: &[Vec3]) -> OrbitCamera {
    let mut camera = OrbitCamera {
        yaw: 0.6,
        pitch: 0.15,
        ..Default::default()
    };
    let (lo, hi) = points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
        (lo.min(p), hi.max(p))
    });
    camera.frame((lo + hi) / 2.0, (hi - lo).length() / 2.0);
    // Then centred and sized by what the camera sees: a few passes, as
    // perspective changes a little with each.
    for _ in 0..4 {
        let m: Mat4 = camera.projection(1.0) * camera.view();
        let (mut min, mut max) = (glam::Vec2::MAX, glam::Vec2::MIN);
        for &p in points.iter().step_by(7) {
            let ndc = m.project_point3(p).truncate();
            min = min.min(ndc);
            max = max.max(ndc);
        }
        let (right, up) = camera.right_up();
        let centre = (min + max) / 2.0;
        camera.target += (right * centre.x + up * centre.y) * camera.half_height();
        let extent = (max - min).max_element() / 2.0;
        camera.distance *= extent / 0.86;
    }
    camera
}

/// Every 2 × 2 pixels as one grey: the clay and the studio light are
/// grey, so nothing is lost but bytes.
fn halve_to_grey(rgba: &[u8], size: u32) -> Vec<u8> {
    let half = size / 2;
    let mut out = Vec::with_capacity((half * half) as usize);
    for y in 0..half {
        for x in 0..half {
            let mut sum = 0u32;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (((y * 2 + dy) * size + x * 2 + dx) * 4) as usize;
                // Rec. 709 luma of the display values.
                sum += (u32::from(rgba[i]) * 2126
                    + u32::from(rgba[i + 1]) * 7152
                    + u32::from(rgba[i + 2]) * 722)
                    / 10_000;
            }
            out.push(((sum + 2) / 4) as u8);
        }
    }
    out
}

fn write_png(path: &Path, grey: &[u8], size: u32) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size, size);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(grey))
        .map_err(|e| format!("{}: {e}", path.display()))
}
