// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! STL: a list of triangles, each with its own three corners. Binary is an
//! 80-byte header, a count and 50 bytes per triangle; the text form spells
//! the same out as `facet … vertex x y z … endfacet`.

use glam::Vec3;

use super::{ReadError, Soup, finite, number};
use crate::io::{Reader, Writer};
use crate::sculpt::Mesh;

const HEADER: usize = 80;
const RECORD: usize = 50;

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

/// Reads binary or text STL. Every triangle comes with its own corners:
/// [`Soup::weld`] joins them.
pub fn read(bytes: &[u8]) -> Result<Soup, ReadError> {
    // A binary file may also begin with "solid", so its size decides.
    let binary = bytes.len() >= HEADER + 4 && {
        let count = u32::from_le_bytes(bytes[HEADER..HEADER + 4].try_into().expect("4 bytes"));
        (count as usize).checked_mul(RECORD) == Some(bytes.len() - HEADER - 4)
    };
    let soup = if binary {
        read_binary(bytes)?
    } else if bytes.trim_ascii_start().starts_with(b"solid") {
        read_text(bytes)?
    } else {
        return Err(ReadError::Invalid {
            offset: 0,
            what: "neither binary nor text STL",
        });
    };
    if soup.triangles.is_empty() {
        return Err(ReadError::Invalid {
            offset: 0,
            what: "no triangles",
        });
    }
    Ok(soup)
}

fn read_binary(bytes: &[u8]) -> Result<Soup, ReadError> {
    let mut r = Reader::new(bytes);
    r.skip(HEADER)?;
    let count = r.u32()? as usize;
    let mut soup = Soup {
        positions: Vec::with_capacity(count * 3),
        triangles: Vec::with_capacity(count),
    };
    for _ in 0..count {
        let at = r.position();
        r.skip(12)?; // the facet normal: recomputed from the corners
        for _ in 0..3 {
            soup.positions.push(finite(r.vec3()?, at)?);
        }
        r.skip(2)?;
        let v = soup.positions.len() as u32;
        soup.triangles.push([v - 3, v - 2, v - 1]);
    }
    Ok(soup)
}

fn read_text(bytes: &[u8]) -> Result<Soup, ReadError> {
    let mut soup = Soup::default();
    let mut offset = 0;
    for line in bytes.split(|&b| b == b'\n') {
        let start = offset;
        offset += line.len() + 1;
        let mut words = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|w| !w.is_empty());
        if words.next() != Some(b"vertex") {
            continue;
        }
        let mut xyz = [0.0f32; 3];
        for x in &mut xyz {
            let word = words.next().ok_or(ReadError::Invalid {
                offset: start,
                what: "a vertex with fewer than three coordinates",
            })?;
            *x = number(word, start)?;
        }
        soup.positions.push(finite(Vec3::from(xyz), start)?);
        if soup.positions.len() % 3 == 0 {
            let v = soup.positions.len() as u32;
            soup.triangles.push([v - 3, v - 2, v - 1]);
        }
    }
    if soup.positions.len() % 3 != 0 {
        return Err(ReadError::Invalid {
            offset,
            what: "a facet without three vertices",
        });
    }
    Ok(soup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clay_round_trips_through_binary_stl() {
        let mesh = Mesh::icosphere(2);
        let bytes = write(&mesh);
        assert_eq!(bytes.len(), 84 + 50 * mesh.triangles.len());
        let soup = read(&bytes).unwrap().weld();
        assert_eq!(soup.triangles.len(), mesh.triangles.len());
        assert_eq!(soup.positions.len(), mesh.positions.len());
        // Same triangles, corner for corner, though the numbering differs.
        for (t, u) in soup.triangles.iter().zip(&mesh.triangles) {
            for k in 0..3 {
                assert_eq!(soup.positions[t[k] as usize], mesh.positions[u[k] as usize]);
            }
        }
        assert!(soup.is_closed());
    }

    #[test]
    fn a_binary_header_may_start_with_solid() {
        let mut bytes = write(&Mesh::icosphere(0));
        bytes[..5].copy_from_slice(b"solid");
        assert_eq!(read(&bytes).unwrap().triangles.len(), 20);
    }

    #[test]
    fn text_stl() {
        let text = b"solid tri\n facet normal 0 0 1\n  outer loop\n\
            vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\n\
            facet normal 0 0 1\n outer loop\n vertex 1 0 0\n vertex 1 1 0\n vertex 0 1 0\n\
            endloop\n endfacet\nendsolid tri\n";
        let soup = read(text).unwrap().weld();
        assert_eq!(soup.positions.len(), 4);
        assert_eq!(soup.triangles, vec![[0, 1, 2], [1, 3, 2]]);
    }

    #[test]
    fn bad_files_are_errors() {
        let good = write(&Mesh::icosphere(0));
        // Every cut of a binary file is either refused or shorter text.
        for end in 0..good.len() {
            assert!(read(&good[..end]).is_err(), "cut at {end}");
        }
        for bad in [
            &b""[..],
            b"solid empty\nendsolid\n",
            b"solid x\nvertex 0 0 0\nvertex 1 0 0\n",
            b"solid x\nvertex 0 0 zero\n",
            b"garbage that is not stl",
        ] {
            assert!(read(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
