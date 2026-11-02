// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Mesh files in and out: the clay as OBJ or STL for other programs, and
//! meshes from them to sculpt on.
//!
//! The readers are deliberately small: positions and triangles only, which
//! is all the clay keeps. Reading returns an error for anything malformed
//! and never panics.

pub mod obj;
pub mod stl;

use std::collections::HashMap;

use glam::Vec3;

pub use crate::io::ReadError;

/// Positions and counter-clockwise triangles, as read from a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Soup {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

/// Mesh formats the clay can be imported from (lowercase, no dot).
pub const EXTENSIONS: &[&str] = &["obj", "stl"];

/// Reads a mesh file, going by its name's extension.
pub fn read(name: &str, bytes: &[u8]) -> Result<Soup, ReadError> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "obj" => obj::read(bytes),
        "stl" => stl::read(bytes),
        _ => Err(ReadError::Invalid {
            offset: 0,
            what: "not a mesh format the clay reads",
        }),
    }
}

impl Soup {
    /// Joins vertices that sit at the same place (STL repeats every corner;
    /// OBJ files split them along texture seams), drops the triangles that
    /// collapse, and the vertices no triangle uses.
    pub fn weld(&self) -> Soup {
        let (min, max) = bounds(&self.positions);
        // A millionth of the size: closer than that is the same point.
        let cell = ((max - min).max_element() * 1e-6).max(f32::MIN_POSITIVE);
        let mut index: HashMap<[i64; 3], u32> = HashMap::new();
        let mut positions = Vec::new();
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut triangles = Vec::with_capacity(self.triangles.len());
        for t in &self.triangles {
            let t = t.map(|v| {
                let r = &mut remap[v as usize];
                if *r == u32::MAX {
                    let p = self.positions[v as usize];
                    let key = ((p - min) / cell).round().as_i64vec3().to_array();
                    *r = *index.entry(key).or_insert_with(|| {
                        positions.push(p);
                        positions.len() as u32 - 1
                    });
                }
                *r
            });
            if t[0] != t[1] && t[1] != t[2] && t[2] != t[0] {
                triangles.push(t);
            }
        }
        Soup {
            positions,
            triangles,
        }
    }

    /// Centres the mesh on the origin and scales it to fit the unit
    /// sphere, where the clay lives.
    pub fn normalise(&mut self) {
        let (min, max) = bounds(&self.positions);
        let centre = (min + max) / 2.0;
        let radius = self
            .positions
            .iter()
            .map(|p| p.distance(centre))
            .fold(0.0, f32::max);
        let scale = if radius > 0.0 { 1.0 / radius } else { 1.0 };
        for p in &mut self.positions {
            *p = (*p - centre) * scale;
        }
    }

    /// Closed and manifold: every edge joins exactly two triangles that
    /// run it in opposite directions.
    pub fn is_closed(&self) -> bool {
        is_closed(&self.triangles)
    }
}

/// See [`Soup::is_closed`].
pub fn is_closed(triangles: &[[u32; 3]]) -> bool {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::with_capacity(triangles.len() * 3);
    for &[a, b, c] in triangles {
        for e in [(a, b), (b, c), (c, a)] {
            *edges.entry(e).or_insert(0) += 1;
        }
    }
    !triangles.is_empty()
        && edges
            .iter()
            .all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
}

fn bounds(positions: &[Vec3]) -> (Vec3, Vec3) {
    if positions.is_empty() {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    positions
        .iter()
        .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
            (lo.min(p), hi.max(p))
        })
}

/// Parses a decimal number from ASCII, for the text formats.
fn number<T: std::str::FromStr>(word: &[u8], offset: usize) -> Result<T, ReadError> {
    std::str::from_utf8(word)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(ReadError::Invalid {
            offset,
            what: "not a number",
        })
}

/// Finite numbers only: a NaN or infinite coordinate would poison the clay.
fn finite(p: Vec3, offset: usize) -> Result<Vec3, ReadError> {
    if p.is_finite() {
        Ok(p)
    } else {
        Err(ReadError::Invalid {
            offset,
            what: "a coordinate that isn't a finite number",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weld_joins_repeated_corners_and_drops_collapsed_triangles() {
        let soup = Soup {
            positions: vec![
                Vec3::ZERO,
                Vec3::X,
                Vec3::Y,
                Vec3::X,
                Vec3::Y,
                Vec3::ONE,
                Vec3::ONE,
            ],
            triangles: vec![[0, 1, 2], [3, 5, 4], [5, 6, 0]],
        };
        let welded = soup.weld();
        assert_eq!(welded.positions.len(), 4);
        assert_eq!(welded.triangles, vec![[0, 1, 2], [1, 3, 2]]);
    }

    #[test]
    fn normalise_fits_the_unit_sphere() {
        let mut soup = Soup {
            positions: vec![Vec3::new(10.0, 0.0, 0.0), Vec3::new(14.0, 3.0, 0.0)],
            triangles: vec![],
        };
        soup.normalise();
        let far = soup
            .positions
            .iter()
            .map(|p| p.length())
            .fold(0.0, f32::max);
        assert!((far - 1.0).abs() < 1e-6);
        assert!((soup.positions[0] + soup.positions[1]).length() < 1e-6);
    }

    #[test]
    fn unknown_extensions_are_refused() {
        assert!(read("a.fbx", b"anything").is_err());
        assert!(read("noext", b"").is_err());
    }
}
