// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Wavefront OBJ: `v` positions (with an optional r g b after them, as
//! some scanners write) and `f` faces. Face corners may carry texture and
//! normal indices after slashes, which are ignored; negative indices count
//! back from the last vertex; polygons become fans of triangles. Everything
//! else (materials, groups, curves) is skipped.

use glam::Vec3;

use super::{LoadError, unit_to_u8};
use crate::reference::RefMesh;

/// Reads an OBJ mesh. Public so importing a sculpt can use it too.
pub fn load_obj(bytes: &[u8]) -> Result<RefMesh, LoadError> {
    let mut positions: Vec<Vec3> = Vec::new();
    let mut colors: Vec<[u8; 4]> = Vec::new();
    let mut any_colour = false;
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut polygon: Vec<u32> = Vec::new();
    for line in bytes.split(|&b| b == b'\n') {
        // Non-UTF-8 bytes can only be in names and comments, which are skipped.
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let mut words = line.split_ascii_whitespace();
        match words.next() {
            Some("v") => {
                let mut values = [0.0f32; 7];
                let mut n = 0;
                for word in words.take(7) {
                    values[n] = word
                        .parse()
                        .map_err(|_| LoadError::Invalid("bad number in OBJ vertex"))?;
                    n += 1;
                }
                if n < 3 {
                    return Err(LoadError::Invalid("OBJ vertex with fewer than 3 numbers"));
                }
                positions.push(Vec3::new(values[0], values[1], values[2]));
                // x y z r g b, or x y z w r g b.
                let rgb = match n {
                    6 => Some(&values[3..6]),
                    7 => Some(&values[4..7]),
                    _ => None,
                };
                any_colour |= rgb.is_some();
                colors.push(rgb.map_or([255; 4], |c| {
                    [unit_to_u8(c[0]), unit_to_u8(c[1]), unit_to_u8(c[2]), 255]
                }));
            }
            Some("f") => {
                polygon.clear();
                for word in words {
                    let first = word.split('/').next().unwrap_or("");
                    let i: i64 = first
                        .parse()
                        .map_err(|_| LoadError::Invalid("bad OBJ face index"))?;
                    let n = positions.len() as i64;
                    let index = if i < 0 { n + i } else { i - 1 };
                    if !(0..n).contains(&index) {
                        return Err(LoadError::Invalid("OBJ face refers to a missing vertex"));
                    }
                    polygon.push(index as u32);
                }
                for k in 1..polygon.len().saturating_sub(1) {
                    triangles.push([polygon[0], polygon[k], polygon[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if triangles.is_empty() {
        return Err(if positions.is_empty() && !bytes.is_ascii() {
            LoadError::Invalid("not an OBJ file")
        } else {
            LoadError::Empty
        });
    }
    Ok(RefMesh {
        positions,
        triangles,
        colors: any_colour.then_some(colors),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: &str = "# a unit cube\n\
        mtllib cube.mtl\n\
        o cube\n\
        v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
        v 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
        vt 0 0\nvn 0 0 1\n\
        usemtl clay\n\
        f 1/1/1 4/1/1 3/1/1 2/1/1\n\
        f 5//1 6//1 7//1 8//1\n\
        f -8 -7 -3 -4\n\
        f 2 3 7 6\n\
        f 3 4 8 7\r\n\
        f 4 1 5 8\n";

    #[test]
    fn cube_with_slashes_negative_indices_and_quads() {
        let m = load_obj(CUBE.as_bytes()).unwrap();
        assert_eq!(m.positions.len(), 8);
        assert_eq!(m.triangles.len(), 12);
        assert_eq!(m.triangles[0], [0, 3, 2]);
        assert_eq!(m.triangles[1], [0, 2, 1]);
        // -8 is the first vertex once all eight are read.
        assert_eq!(m.triangles[4], [0, 1, 5]);
        assert!(m.colors.is_none());
    }

    #[test]
    fn vertex_colours() {
        let m = load_obj(b"v 0 0 0 1 0 0\nv 1 0 0 0 1 0\nv 0 1 0 0 0 1\nf 1 2 3\n").unwrap();
        assert_eq!(m.colors.unwrap()[1], [0, 255, 0, 255]);
    }

    #[test]
    fn broken_files_are_errors() {
        assert!(
            load_obj(b"v 0 0 0\nv 1 0 0\nf 1 2 3\n").is_err(),
            "missing vertex"
        );
        assert!(load_obj(b"v 0 0 0\nf 0 0 0\n").is_err(), "index 0");
        assert!(load_obj(b"v 0 zero 0\n").is_err());
        assert!(load_obj(b"v 0 0 0\n").is_err(), "no faces");
        assert!(load_obj(&[0xFF, 0x00, 0x13]).is_err());
        for end in 0..CUBE.len() {
            // Cuts either fail or give part of the cube; never a panic.
            let _ = load_obj(&CUBE.as_bytes()[..end]);
        }
    }
}
