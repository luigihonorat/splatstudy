// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Wavefront OBJ: text, one vertex or face per line.

use std::fmt::Write as _;

use crate::sculpt::Mesh;

/// The clay as OBJ: positions, smooth normals and faces: quads where the
/// mesh came from a quad remesh, triangles otherwise.
pub fn write(mesh: &Mesh) -> Vec<u8> {
    let mut out = String::with_capacity(mesh.positions.len() * 80 + mesh.triangles.len() * 40);
    out.push_str("# made with SplatStudy\n");
    // `{}` prints the shortest text that reads back as the same f32.
    for p in &mesh.positions {
        let _ = writeln!(out, "v {} {} {}", p.x, p.y, p.z);
    }
    for n in &mesh.normals {
        let _ = writeln!(out, "vn {} {} {}", n.x, n.y, n.z);
    }
    let mut in_quad = vec![false; mesh.triangles.len()];
    let mut quads = Vec::with_capacity(mesh.quads.len());
    for &[s, t] in &mesh.quads {
        let (Some(&first), Some(&second)) = (
            mesh.triangles.get(s as usize),
            mesh.triangles.get(t as usize),
        ) else {
            continue;
        };
        if in_quad[s as usize] || in_quad[t as usize] || s == t {
            continue;
        }
        if let Some(quad) = join(first, second) {
            in_quad[s as usize] = true;
            in_quad[t as usize] = true;
            quads.push((s.min(t), quad));
        }
    }
    // Each quad where its first triangle was, the rest in order.
    quads.sort_unstable_by_key(|q| q.0);
    let mut next = quads.iter().peekable();
    for (i, t) in mesh.triangles.iter().enumerate() {
        if let Some((_, quad)) = next.next_if(|q| q.0 as usize == i) {
            let [a, b, c, d] = quad.map(|i| i + 1);
            let _ = writeln!(out, "f {a}//{a} {b}//{b} {c}//{c} {d}//{d}");
        } else if !in_quad[i] {
            let [a, b, c] = t.map(|i| i + 1);
            let _ = writeln!(out, "f {a}//{a} {b}//{b} {c}//{c}");
        }
    }
    out.into_bytes()
}

/// The quad made of two triangles sharing an edge (wound opposite ways),
/// wound as they are.
fn join(first: [u32; 3], second: [u32; 3]) -> Option<[u32; 4]> {
    for k in 0..3 {
        let [a, b, c] = [first[k], first[(k + 1) % 3], first[(k + 2) % 3]];
        // The shared edge is c → a in the first, a → c in the second.
        let shares = (0..3).any(|j| second[j] == a && second[(j + 1) % 3] == c);
        if shares {
            let d = *second.iter().find(|&&v| v != a && v != c)?;
            return (d != b).then_some([a, b, c, d]);
        }
    }
    None
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

    #[test]
    fn quads_are_written_as_quads() {
        // A square of two triangles sharing the diagonal 0–2, and one more
        // triangle on its side.
        let positions = vec![
            glam::Vec3::ZERO,
            glam::Vec3::X,
            glam::Vec3::new(1.0, 1.0, 0.0),
            glam::Vec3::Y,
            glam::Vec3::new(2.0, 0.0, 0.0),
        ];
        let mut mesh = Mesh::from_ordered(positions, vec![[0, 1, 2], [1, 4, 2], [0, 2, 3]]);
        mesh.quads = vec![[0, 2]];
        let text = String::from_utf8(write(&mesh)).unwrap();
        let faces: Vec<&str> = text.lines().filter(|l| l.starts_with("f ")).collect();
        assert_eq!(faces, ["f 1//1 2//2 3//3 4//4", "f 2//2 5//5 3//3"]);
        // Read back, the quad is the same two triangles.
        let back = load_obj(text.as_bytes()).unwrap();
        assert_eq!(back.triangles.len(), 3);
    }
}
