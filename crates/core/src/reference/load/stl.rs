// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! STL, binary and ASCII. Every triangle carries its own three corners, so
//! corners at the same place are welded into shared vertices.
//!
//! A binary file may also begin with "solid", so the size decides: a
//! binary file is exactly 84 bytes plus 50 per triangle.

use std::collections::HashMap;

use glam::Vec3;

use super::LoadError;
use crate::io::Reader;
use crate::reference::RefMesh;

/// Reads an STL mesh and welds it. Public so importing a sculpt can use it.
pub fn load_stl(bytes: &[u8]) -> Result<RefMesh, LoadError> {
    let corners = if is_binary(bytes) {
        binary(bytes)?
    } else if bytes.trim_ascii_start().starts_with(b"solid") {
        ascii(bytes)?
    } else {
        return Err(LoadError::Invalid("not an STL file"));
    };
    if corners.is_empty() {
        return Err(LoadError::Empty);
    }
    if corners.iter().any(|p| !p.is_finite()) {
        return Err(LoadError::Invalid("STL has numbers that aren't finite"));
    }
    let (positions, triangles) = weld(&corners);
    if triangles.is_empty() {
        return Err(LoadError::Empty);
    }
    Ok(RefMesh {
        positions,
        triangles,
        colors: None,
    })
}

fn is_binary(bytes: &[u8]) -> bool {
    let Some(count) = bytes.get(80..84) else {
        return false;
    };
    let count = u32::from_le_bytes([count[0], count[1], count[2], count[3]]) as u64;
    bytes.len() as u64 == 84 + 50 * count
}

fn binary(bytes: &[u8]) -> Result<Vec<Vec3>, LoadError> {
    let mut r = Reader::new(bytes);
    r.skip(80)?;
    let n = r.u32()? as usize;
    let mut corners = Vec::with_capacity(n * 3);
    for _ in 0..n {
        r.skip(12)?; // the facet normal; recomputed from the corners
        for _ in 0..3 {
            corners.push(r.vec3()?);
        }
        r.skip(2)?;
    }
    Ok(corners)
}

fn ascii(bytes: &[u8]) -> Result<Vec<Vec3>, LoadError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| LoadError::Invalid("STL text isn't UTF-8"))?;
    let mut corners = Vec::new();
    let mut words = text.split_ascii_whitespace();
    let mut in_facet = 0;
    while let Some(word) = words.next() {
        match word {
            "vertex" => {
                let mut v = [0.0f32; 3];
                for c in &mut v {
                    *c = words
                        .next()
                        .and_then(|w| w.parse().ok())
                        .ok_or(LoadError::Invalid("bad STL vertex"))?;
                }
                corners.push(Vec3::from(v));
                in_facet += 1;
            }
            "endfacet" => {
                if in_facet != 3 {
                    return Err(LoadError::Unsupported("STL facet that isn't a triangle"));
                }
                in_facet = 0;
            }
            _ => {}
        }
    }
    if in_facet != 0 {
        return Err(LoadError::Invalid("STL ends inside a facet"));
    }
    Ok(corners)
}

/// Joins corners closer than a millionth of the model's size into one
/// vertex. `corners` holds three per triangle; triangles that collapse
/// (two corners welded together) are dropped.
pub fn weld(corners: &[Vec3]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (min, max) = corners.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
        (lo.min(p), hi.max(p))
    });
    let step = ((max - min).max_element() * 1e-6).max(f32::MIN_POSITIVE);
    let mut index: HashMap<[i64; 3], u32> = HashMap::with_capacity(corners.len() / 4);
    let mut positions = Vec::new();
    let mut ids = Vec::with_capacity(corners.len());
    for &p in corners {
        let q = ((p - min) / step).round();
        let key = [q.x as i64, q.y as i64, q.z as i64];
        let id = *index.entry(key).or_insert_with(|| {
            positions.push(p);
            (positions.len() - 1) as u32
        });
        ids.push(id);
    }
    let triangles = ids
        .as_chunks::<3>()
        .0
        .iter()
        .copied()
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[2] != t[0])
        .collect();
    (positions, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Writer;

    /// The twelve triangles of a unit cube, outward facing.
    fn cube() -> Vec<[Vec3; 3]> {
        let c = |i: u32| Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32);
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        quads
            .iter()
            .flat_map(|q| [[c(q[0]), c(q[1]), c(q[2])], [c(q[0]), c(q[2]), c(q[3])]])
            .collect()
    }

    fn binary_cube(header: &[u8; 80]) -> Vec<u8> {
        let mut w = Writer::new();
        w.raw(header).u32(12);
        for t in cube() {
            w.vec3(Vec3::ZERO);
            for p in t {
                w.vec3(p);
            }
            w.u16(0);
        }
        w.finish()
    }

    fn ascii_cube() -> String {
        let mut s = String::from("solid cube\n");
        for t in cube() {
            s += "  facet normal 0 0 0\n    outer loop\n";
            for p in t {
                s += &format!("      vertex {} {} {}\n", p.x, p.y, p.z);
            }
            s += "    endloop\n  endfacet\n";
        }
        s + "endsolid cube\n"
    }

    #[test]
    fn binary_and_ascii_cubes_weld_to_eight_vertices() {
        let mut header = [b' '; 80];
        header[..5].copy_from_slice(b"solid"); // binary files may start like ASCII ones
        for bytes in [
            binary_cube(&header),
            binary_cube(&[0; 80]),
            ascii_cube().into_bytes(),
        ] {
            let m = load_stl(&bytes).unwrap();
            assert_eq!(m.positions.len(), 8);
            assert_eq!(m.triangles.len(), 12);
        }
    }

    #[test]
    fn broken_files_are_errors() {
        let good = binary_cube(&[0; 80]);
        for end in 0..good.len() {
            assert!(load_stl(&good[..end]).is_err(), "binary cut at {end}");
        }
        let text = ascii_cube();
        for end in [10, 40, 100, text.len() - 30] {
            assert!(
                load_stl(&text.as_bytes()[..end]).is_err(),
                "ascii cut at {end}"
            );
        }
        assert!(load_stl(b"solid x\nfacet\nvertex 1 two 3\n").is_err());
        assert!(load_stl(b"nothing").is_err());
    }
}
