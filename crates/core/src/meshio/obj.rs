// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Wavefront OBJ: text, one vertex or face per line.

use std::fmt::Write as _;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::load::load_obj;

    #[test]
    fn the_clay_round_trips_exactly() {
        let mut mesh = Mesh::icosphere(2);
        for (i, p) in mesh.positions.iter_mut().enumerate() {
            *p *= 1.0 + (i as f32 * 0.37).sin() * 0.1;
        }
        let back = load_obj(&write(&mesh)).unwrap();
        assert_eq!(back.positions, mesh.positions);
        assert_eq!(back.triangles, mesh.triangles);
    }
}
