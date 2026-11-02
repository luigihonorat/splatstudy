// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Wavefront OBJ: text, one vertex or face per line.

use std::fmt::Write as _;

use glam::Vec3;

use super::{ReadError, Soup, finite, number};
use crate::sculpt::Mesh;

/// The clay as OBJ: positions, smooth normals and triangles.
pub fn write(mesh: &Mesh) -> Vec<u8> {
    let mut out = String::with_capacity(mesh.positions.len() * 80 + mesh.triangles.len() * 40);
    out.push_str("# made with splatstudy\n");
    // `{}` prints the shortest text that reads back as the same f32.
    for p in &mesh.positions {
        let _ = writeln!(out, "v {} {} {}", p.x, p.y, p.z);
    }
    for n in &mesh.normals {
        let _ = writeln!(out, "vn {} {} {}", n.x, n.y, n.z);
    }
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| i + 1);
        let _ = writeln!(out, "f {a}//{a} {b}//{b} {c}//{c}");
    }
    out.into_bytes()
}

/// Reads `v` and `f` lines; everything else (normals, texture coordinates,
/// groups, materials) is skipped. Faces may use `v/vt/vn` and negative
/// (relative) indices; polygons become fans of triangles.
pub fn read(bytes: &[u8]) -> Result<Soup, ReadError> {
    let mut soup = Soup::default();
    let mut corners: Vec<u32> = Vec::new();
    let mut offset = 0;
    for line in bytes.split(|&b| b == b'\n') {
        let start = offset;
        offset += line.len() + 1;
        let line = line.split(|&b| b == b'#').next().unwrap_or_default();
        let mut words = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|w| !w.is_empty());
        match words.next() {
            Some(b"v") => {
                let mut xyz = [0.0f32; 3];
                for x in &mut xyz {
                    let word = words.next().ok_or(ReadError::Invalid {
                        offset: start,
                        what: "a vertex with fewer than three coordinates",
                    })?;
                    *x = number(word, start)?;
                }
                soup.positions.push(finite(Vec3::from(xyz), start)?);
            }
            Some(b"f") => {
                corners.clear();
                for word in words {
                    let index = word.split(|&b| b == b'/').next().unwrap_or_default();
                    let i: i64 = number(index, start)?;
                    let count = soup.positions.len() as i64;
                    // 1-based; negative counts back from the last vertex so far.
                    let i = if i < 0 { count + i } else { i - 1 };
                    if !(0..count).contains(&i) {
                        return Err(ReadError::Invalid {
                            offset: start,
                            what: "a face corner that isn't a vertex",
                        });
                    }
                    corners.push(i as u32);
                }
                if corners.len() < 3 {
                    return Err(ReadError::Invalid {
                        offset: start,
                        what: "a face with fewer than three corners",
                    });
                }
                for k in 1..corners.len() - 1 {
                    soup.triangles
                        .push([corners[0], corners[k], corners[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if soup.triangles.is_empty() {
        return Err(ReadError::Invalid {
            offset: 0,
            what: "no faces",
        });
    }
    Ok(soup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clay_round_trips_exactly() {
        let mut mesh = Mesh::icosphere(2);
        for (i, p) in mesh.positions.iter_mut().enumerate() {
            *p *= 1.0 + (i as f32 * 0.37).sin() * 0.1;
        }
        let soup = read(&write(&mesh)).unwrap();
        assert_eq!(soup.positions, mesh.positions);
        assert_eq!(soup.triangles, mesh.triangles);
    }

    #[test]
    fn polygons_slashes_and_relative_indices() {
        let text = b"# a square and a triangle\r\n\
            v 0 0 0\r\nv 1 0 0\nv 1 1 0\nv 0 1 0 1.0\n\
            vt 0 0\nvn 0 0 1\n\
            f 1/1/1 2/1/1 3/1/1 4/1/1\n\
            g side\nf -3//1 -2//1 -1//1\n";
        let soup = read(text).unwrap();
        assert_eq!(soup.positions.len(), 4);
        assert_eq!(soup.triangles, vec![[0, 1, 2], [0, 2, 3], [1, 2, 3]]);
    }

    #[test]
    fn bad_files_are_errors() {
        for bad in [
            &b""[..],
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\n",
            b"v 0 0\nf 1 1 1\n",
            b"v 0 0 0\nv 1 0 0\nf 1 2\n",
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n",
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n",
            b"v 0 0 x\nf 1 1 1\n",
            b"v nan 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
            b"v 0 0 0\nf -5 1 1\n",
            b"\xff\xfe\x00garbage",
        ] {
            assert!(read(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
