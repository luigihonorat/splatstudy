// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Reading reference files: splats, meshes and pictures. Every loader takes
//! the whole file and returns a [`LoadError`] for anything it can't read;
//! none of them panics on bad input.

mod image;
mod obj;
mod ply;
mod splat;
mod spz;
mod stl;

use std::fmt;

use glam::{Quat, Vec3};

pub use image::{load_jpeg, load_png};
pub use obj::load_obj;
pub use ply::load_ply;
pub use splat::load_splat;
pub use spz::load_spz;
pub use stl::{load_stl, weld};

use super::{Content, Splats, UpAxis};
use crate::io::ReadError;

/// Why a file couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Short or malformed binary data.
    Read(ReadError),
    /// The file isn't in the format its name says, or is broken.
    Invalid(&'static str),
    /// A valid file of a kind SplatStudy doesn't read.
    Unsupported(&'static str),
    /// Nothing to show: no splats, points or triangles.
    Empty,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) => e.fmt(f),
            Self::Invalid(what) | Self::Unsupported(what) => f.write_str(what),
            Self::Empty => f.write_str("nothing to show"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<ReadError> for LoadError {
    fn from(e: ReadError) -> Self {
        Self::Read(e)
    }
}

/// A file name's extension, lowercase, without the dot.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Reads a reference file, choosing the format by its extension, or by its
/// first bytes when the extension says nothing.
pub fn load(name: &str, bytes: &[u8]) -> Result<Content, LoadError> {
    match extension(name).as_str() {
        "ply" => load_ply(bytes),
        "splat" => load_splat(bytes).map(Content::Splats),
        "spz" => load_spz(bytes).map(Content::Splats),
        "obj" => load_obj(bytes).map(Content::Mesh),
        "stl" => load_stl(bytes).map(Content::Mesh),
        "png" => load_png(bytes).map(Content::Image),
        "jpg" | "jpeg" => load_jpeg(bytes).map(Content::Image),
        _ if bytes.starts_with(b"ply") => load_ply(bytes),
        _ if bytes.starts_with(&[0x1F, 0x8B]) => load_spz(bytes).map(Content::Splats),
        _ if bytes.starts_with(b"\x89PNG") => load_png(bytes).map(Content::Image),
        _ if bytes.starts_with(&[0xFF, 0xD8]) => load_jpeg(bytes).map(Content::Image),
        _ => Err(LoadError::Unsupported("unknown file type")),
    }
}

/// Which way is up in a file, as most files of its kind come: splat
/// captures in camera coordinates (Y down), `.spz` already Y up, STL from
/// CAD and 3D printing Z up.
pub fn usual_up(name: &str, content: &Content) -> UpAxis {
    match (extension(name).as_str(), content) {
        ("ply" | "splat", Content::Splats(s)) if !s.is_point_cloud() => UpAxis::NegY,
        ("stl", _) => UpAxis::Z,
        _ => UpAxis::Y,
    }
}

impl Splats {
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Points loaded as splats are all round and the same size.
    fn is_point_cloud(&self) -> bool {
        let Some(&first) = self.scales.first() else {
            return true;
        };
        first.x == first.y
            && first.y == first.z
            && self.scales.iter().all(|&s| s == first)
            && self.rotations.iter().all(|&q| q == Quat::IDENTITY)
    }

    /// Keeps only splats whose numbers are all finite; NaNs in a file
    /// would break fitting and sorting.
    fn keep_finite(mut self) -> Self {
        let ok = |i: usize| {
            self.positions[i].is_finite()
                && self.scales[i].is_finite()
                && self.rotations[i].is_finite()
        };
        if (0..self.len()).all(ok) {
            return self;
        }
        let keep: Vec<bool> = (0..self.len()).map(ok).collect();
        let mut k = keep.iter();
        self.positions.retain(|_| *k.next().unwrap_or(&false));
        let mut k = keep.iter();
        self.scales.retain(|_| *k.next().unwrap_or(&false));
        let mut k = keep.iter();
        self.rotations.retain(|_| *k.next().unwrap_or(&false));
        let mut k = keep.iter();
        self.colors.retain(|_| *k.next().unwrap_or(&false));
        self
    }
}

/// Points as small round splats, sized so a scanned surface reads as
/// closed: about half the spacing of points spread over the bounding box's
/// faces.
pub(crate) fn point_cloud(positions: Vec<Vec3>, colors: Vec<[u8; 4]>) -> Result<Splats, LoadError> {
    let finite: Vec<Vec3> = positions
        .iter()
        .copied()
        .filter(|p| p.is_finite())
        .collect();
    if finite.is_empty() {
        return Err(LoadError::Empty);
    }
    let (min, max) = finite.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
        (lo.min(p), hi.max(p))
    });
    let size = max - min;
    let area = 2.0 * (size.x * size.y + size.y * size.z + size.z * size.x);
    let spacing = (area / finite.len() as f32).sqrt();
    let sigma = if spacing > 0.0 {
        0.5 * spacing
    } else {
        // All points in one place, or a single point.
        size.max_element().max(1.0) * 0.01
    };
    let n = positions.len();
    Ok(Splats {
        positions,
        scales: vec![Vec3::splat(sigma); n],
        rotations: vec![Quat::IDENTITY; n],
        colors,
    }
    .keep_finite())
}

/// The base colour of a 3DGS splat from its first spherical harmonic
/// coefficient, as stored display values.
pub(crate) fn sh0_to_u8(dc: f32) -> u8 {
    const SH_C0: f32 = 0.282_094_8;
    unit_to_u8(0.5 + SH_C0 * dc)
}

/// 0..1 to 0..255, rounded; out-of-range values are clamped.
pub(crate) fn unit_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(crate) fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// A unit quaternion from components in the file's order; an all-zero or
/// broken one becomes no turn.
pub(crate) fn unit_quat(w: f32, x: f32, y: f32, z: f32) -> Quat {
    let q = Quat::from_xyzw(x, y, z, w);
    let len = q.length();
    if len.is_finite() && len > 1e-12 {
        q / len
    } else {
        Quat::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_garbage_files_are_errors() {
        assert!(load("notes.txt", b"hello").is_err());
        for name in [
            "a.ply", "a.splat", "a.spz", "a.obj", "a.stl", "a.png", "a.jpg",
        ] {
            assert!(load(name, b"").is_err(), "{name} empty");
            assert!(load(name, &[0xFF; 7]).is_err(), "{name} garbage");
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(sh0_to_u8(0.0), 128);
        assert_eq!(unit_to_u8(2.0), 255);
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
        assert_eq!(unit_quat(0.0, 0.0, 0.0, 0.0), Quat::IDENTITY);
        let q = unit_quat(2.0, 0.0, 0.0, 0.0);
        assert!((q.w - 1.0).abs() < 1e-6);
        let cloud = point_cloud(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[255; 4]; 3]).unwrap();
        assert!(cloud.is_point_cloud());
        assert!(cloud.scales[0].x > 0.0);
    }
}
