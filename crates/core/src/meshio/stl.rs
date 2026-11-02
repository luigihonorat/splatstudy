// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Binary STL: an 80-byte header, a count, and 50 bytes per triangle (its
//! normal, its three corners and two unused bytes).

use crate::io::Writer;
use crate::sculpt::Mesh;

const HEADER: usize = 80;

/// The clay as binary STL.
pub fn write(mesh: &Mesh) -> Vec<u8> {
    let mut w = Writer::new();
    let mut header = [b' '; HEADER];
    let title = b"binary stl made with splatstudy";
    header[..title.len()].copy_from_slice(title);
    w.raw(&header).u32(mesh.triangles.len() as u32);
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        w.vec3((b - a).cross(c - a).normalize_or_zero())
            .vec3(a)
            .vec3(b)
            .vec3(c)
            .u16(0);
    }
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshio::is_closed;
    use crate::reference::load::load_stl;

    #[test]
    fn the_clay_round_trips_through_binary_stl() {
        let mesh = Mesh::icosphere(2);
        let bytes = write(&mesh);
        assert_eq!(bytes.len(), 84 + 50 * mesh.triangles.len());
        let back = load_stl(&bytes).unwrap();
        assert_eq!(back.triangles.len(), mesh.triangles.len());
        assert_eq!(back.positions.len(), mesh.positions.len());
        // Same triangles, corner for corner, though the numbering differs.
        for (t, u) in back.triangles.iter().zip(&mesh.triangles) {
            for k in 0..3 {
                assert_eq!(back.positions[t[k] as usize], mesh.positions[u[k] as usize]);
            }
        }
        assert!(is_closed(&back.triangles));
    }
}
