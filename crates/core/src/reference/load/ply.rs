// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! PLY, in ASCII and in binary of either byte order. One format, three
//! kinds of content:
//!
//! - Gaussian splats as 3DGS writes them: `f_dc_*` colour, `opacity`
//!   before the sigmoid, `scale_*` as logarithms, `rot_*` as w, x, y, z,
//!   and the view-dependent colour's `f_rest_*`, 9, 24 or 45 of them for
//!   degree 1, 2 or 3: all of red's coefficients, then green's, then
//!   blue's.
//! - A mesh, when there are faces, with vertex colours if present.
//! - Otherwise a point cloud, shown as small round splats.
//!
//! Only the properties shown are kept, and the view-dependent colour is
//! packed as it is read (see [`Harmonics`]).

use glam::Vec3;

use super::{
    LoadError, finite_vertices, point_cloud, sh0, sh0_to_u8, sigmoid, unit_quat, unit_to_u8,
};
use crate::io::Reader;
use crate::reference::{Content, Harmonics, RefMesh, Splats};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Option<Scalar> {
        Some(match name {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Scalar::F32 | Scalar::F64)
    }

    /// A value from exactly `size()` bytes.
    fn decode(self, b: &[u8], big: bool) -> f64 {
        macro_rules! num {
            ($t:ty) => {{
                let a = b.try_into().unwrap_or_default();
                f64::from(if big {
                    <$t>::from_be_bytes(a)
                } else {
                    <$t>::from_le_bytes(a)
                })
            }};
        }
        match self {
            Scalar::I8 => f64::from(b[0] as i8),
            Scalar::U8 => f64::from(b[0]),
            Scalar::I16 => num!(i16),
            Scalar::U16 => num!(u16),
            Scalar::I32 => num!(i32),
            Scalar::U32 => num!(u32),
            Scalar::F32 => num!(f32),
            Scalar::F64 => num!(f64),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Scalar(Scalar),
    List { count: Scalar, item: Scalar },
}

#[derive(Debug)]
struct Property {
    name: String,
    kind: Kind,
}

#[derive(Debug)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Ascii,
    Little,
    Big,
}

/// The vertex properties kept, in this order.
const KEPT: [&str; 18] = [
    "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
    "rot_0", "rot_1", "rot_2", "rot_3", "red", "green", "blue", "alpha",
];
const X: usize = 0;
const DC: usize = 3;
const OPACITY: usize = 6;
const SCALE: usize = 7;
const ROT: usize = 10;
const RED: usize = 14;
const ALPHA: usize = 17;

/// Reads the header up to `end_header`; returns the elements and where the
/// data starts.
fn header(bytes: &[u8]) -> Result<(Format, Vec<Element>, usize), LoadError> {
    const MAX_HEADER: usize = 1 << 16;
    let mut lines = Vec::new();
    let mut start = 0;
    let data = loop {
        let Some(len) = bytes[start..].iter().position(|&b| b == b'\n') else {
            return Err(LoadError::Invalid("PLY header has no end"));
        };
        let line = String::from_utf8_lossy(&bytes[start..start + len]);
        let line = line.trim().to_owned();
        start += len + 1;
        if line == "end_header" {
            break start;
        }
        lines.push(line);
        if start > MAX_HEADER {
            return Err(LoadError::Invalid("PLY header too long"));
        }
    };
    let mut lines = lines.into_iter();
    if lines.next().as_deref() != Some("ply") {
        return Err(LoadError::Invalid("not a PLY file"));
    }
    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in lines {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        match words.as_slice() {
            ["format", f, _version] => {
                format = Some(match *f {
                    "ascii" => Format::Ascii,
                    "binary_little_endian" => Format::Little,
                    "binary_big_endian" => Format::Big,
                    _ => return Err(LoadError::Unsupported("unknown PLY format")),
                })
            }
            ["element", name, count] => elements.push(Element {
                name: (*name).to_owned(),
                count: count
                    .parse()
                    .map_err(|_| LoadError::Invalid("bad PLY element count"))?,
                properties: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let (Some(count), Some(item)) = (Scalar::parse(count), Scalar::parse(item)) else {
                    return Err(LoadError::Invalid("unknown PLY property type"));
                };
                elements
                    .last_mut()
                    .ok_or(LoadError::Invalid("PLY property before any element"))?
                    .properties
                    .push(Property {
                        name: (*name).to_owned(),
                        kind: Kind::List { count, item },
                    });
            }
            ["property", ty, name] => {
                let ty =
                    Scalar::parse(ty).ok_or(LoadError::Invalid("unknown PLY property type"))?;
                elements
                    .last_mut()
                    .ok_or(LoadError::Invalid("PLY property before any element"))?
                    .properties
                    .push(Property {
                        name: (*name).to_owned(),
                        kind: Kind::Scalar(ty),
                    });
            }
            ["comment", ..] | ["obj_info", ..] | [] => {}
            _ => return Err(LoadError::Invalid("unknown PLY header line")),
        }
    }
    let format = format.ok_or(LoadError::Invalid("PLY header has no format"))?;
    Ok((format, elements, data))
}

/// Where values come from: whitespace-separated text or binary.
enum Body<'a> {
    Ascii(std::str::SplitAsciiWhitespace<'a>),
    Binary { reader: Reader<'a>, big: bool },
}

impl Body<'_> {
    fn value(&mut self, s: Scalar) -> Result<f64, LoadError> {
        match self {
            Body::Ascii(words) => words
                .next()
                .ok_or(LoadError::Invalid("PLY data ends early"))?
                .parse()
                .map_err(|_| LoadError::Invalid("bad number in PLY data")),
            Body::Binary { reader, big } => Ok(s.decode(reader.take(s.size())?, *big)),
        }
    }

    /// Bytes or words left, as a bound on how many rows can follow.
    fn left(&self) -> usize {
        match self {
            Body::Ascii(words) => words.clone().size_hint().1.unwrap_or(usize::MAX),
            Body::Binary { reader, .. } => reader.remaining(),
        }
    }
}

/// A list's length, checked to be a small whole number.
fn list_len(v: f64) -> Result<usize, LoadError> {
    if v.fract() == 0.0 && (0.0..=65_536.0).contains(&v) {
        Ok(v as usize)
    } else {
        Err(LoadError::Invalid("bad PLY list length"))
    }
}

/// What the vertex element held, column by column.
#[derive(Default)]
struct Vertices {
    columns: [Vec<f32>; KEPT.len()],
    /// The colour columns are floats in 0..1, not bytes.
    float_colour: bool,
    count: usize,
    /// The view-dependent colour, packed row by row.
    harmonics: Harmonics,
}

impl Vertices {
    fn has(&self, column: usize) -> bool {
        self.columns[column].len() == self.count && self.count > 0
    }

    fn get(&self, column: usize, i: usize) -> f32 {
        self.columns[column][i]
    }
}

pub fn load_ply(bytes: &[u8]) -> Result<Content, LoadError> {
    let (format, elements, start) = header(bytes)?;
    let mut body = match format {
        Format::Ascii => Body::Ascii(
            std::str::from_utf8(&bytes[start..])
                .map_err(|_| LoadError::Invalid("PLY text isn't UTF-8"))?
                .split_ascii_whitespace(),
        ),
        Format::Little | Format::Big => Body::Binary {
            reader: Reader::new(&bytes[start..]),
            big: format == Format::Big,
        },
    };
    // Faces are checked against the vertices read so far: a second vertex
    // element would change what they point at.
    if elements.iter().filter(|e| e.name == "vertex").count() > 1 {
        return Err(LoadError::Invalid("PLY with two vertex elements"));
    }
    let mut vertices = Vertices::default();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for element in &elements {
        let row_min = match &body {
            Body::Ascii(_) => element.properties.len(),
            Body::Binary { .. } => element
                .properties
                .iter()
                .map(|p| match p.kind {
                    Kind::Scalar(s) => s.size(),
                    Kind::List { count, .. } => count.size(),
                })
                .sum(),
        };
        if element.count.saturating_mul(row_min.max(1)) > body.left() {
            return Err(LoadError::Invalid("PLY data shorter than its header says"));
        }
        if element.name == "vertex" {
            read_vertices(&mut body, element, &mut vertices)?;
        } else {
            let faces = element.name == "face";
            for _ in 0..element.count {
                for p in &element.properties {
                    match p.kind {
                        Kind::Scalar(s) => {
                            body.value(s)?;
                        }
                        Kind::List { count, item } => {
                            let n = list_len(body.value(count)?)?;
                            let indices =
                                faces && (p.name == "vertex_indices" || p.name == "vertex_index");
                            let mut polygon = Vec::with_capacity(if indices { n } else { 0 });
                            for _ in 0..n {
                                let v = body.value(item)?;
                                if indices {
                                    if v.fract() != 0.0 || v < 0.0 || v >= vertices.count as f64 {
                                        return Err(LoadError::Invalid(
                                            "PLY face refers to a missing vertex",
                                        ));
                                    }
                                    polygon.push(v as u32);
                                }
                            }
                            for k in 1..polygon.len().saturating_sub(1) {
                                triangles.push([polygon[0], polygon[k], polygon[k + 1]]);
                            }
                        }
                    }
                }
            }
        }
    }
    build(vertices, triangles)
}

/// The degree of the view-dependent colour, and which property holds each
/// `f_rest_*` in turn, when the vertices have a whole set of them and the
/// base colour they add to.
fn rest_properties(element: &Element) -> Option<(u8, Vec<usize>)> {
    let mut rest: Vec<(usize, usize)> = Vec::new();
    for (at, p) in element.properties.iter().enumerate() {
        if let Some(k) = p.name.strip_prefix("f_rest_") {
            if !matches!(p.kind, Kind::Scalar(_)) {
                return None;
            }
            rest.push((k.parse().ok()?, at));
        }
    }
    let degree = Harmonics::degree_of_rest(rest.len())?;
    rest.sort_unstable();
    if rest.iter().enumerate().any(|(i, &(k, _))| i != k) {
        return None;
    }
    let dc = (0..3).all(|c| {
        element
            .properties
            .iter()
            .any(|p| p.name == KEPT[DC + c] && matches!(p.kind, Kind::Scalar(_)))
    });
    dc.then(|| (degree, rest.into_iter().map(|(_, at)| at).collect()))
}

/// Packs one splat's view-dependent colour, its base colour already read:
/// `value(k)` is `f_rest_k`, with all of red's first.
fn push_harmonics(out: &mut Vertices, i: usize, value: impl Fn(usize) -> f32) {
    let m = Harmonics::coefficients(out.harmonics.degree);
    let base: [f32; 3] = std::array::from_fn(|c| sh0(out.columns[DC + c][i]));
    out.harmonics
        .push(base, base.map(unit_to_u8), |j, c| value(c * m + j));
}

fn read_vertices(body: &mut Body, element: &Element, out: &mut Vertices) -> Result<(), LoadError> {
    let n = element.count;
    out.count = n;
    let slots: Vec<Option<usize>> = element
        .properties
        .iter()
        .map(|p| KEPT.iter().position(|&k| k == p.name))
        .collect();
    let rest = rest_properties(element);
    if let Some((degree, _)) = &rest {
        out.harmonics = Harmonics::new(*degree);
        out.harmonics
            .words
            .reserve_exact(n.saturating_mul(Harmonics::words_per_splat(*degree)));
    }
    for (p, slot) in element.properties.iter().zip(&slots) {
        if let (Some(slot), Kind::Scalar(s)) = (slot, p.kind) {
            out.columns[*slot].reserve_exact(n);
            if (RED..=ALPHA).contains(slot) && s.is_float() {
                out.float_colour = true;
            }
        }
    }
    let fixed: Option<Vec<Scalar>> = element
        .properties
        .iter()
        .map(|p| match p.kind {
            Kind::Scalar(s) => Some(s),
            Kind::List { .. } => None,
        })
        .collect();
    // Binary rows of fixed size: read each kept field straight from the row.
    if let (Body::Binary { reader, big }, Some(types)) = (&mut *body, &fixed) {
        let stride: usize = types.iter().map(|s| s.size()).sum();
        let block = reader.take(n.checked_mul(stride).ok_or(LoadError::Empty)?)?;
        let mut offset = 0;
        for (&s, slot) in types.iter().zip(&slots) {
            if let Some(slot) = slot {
                let column = &mut out.columns[*slot];
                column.extend(
                    block
                        .chunks_exact(stride.max(1))
                        .map(|row| s.decode(&row[offset..offset + s.size()], *big) as f32),
                );
            }
            offset += s.size();
        }
        if let Some((_, at)) = &rest {
            let offsets: Vec<usize> = types
                .iter()
                .scan(0, |o, s| {
                    let here = *o;
                    *o += s.size();
                    Some(here)
                })
                .collect();
            let fields: Vec<(usize, Scalar)> = at.iter().map(|&a| (offsets[a], types[a])).collect();
            for (i, row) in block.chunks_exact(stride.max(1)).enumerate() {
                push_harmonics(out, i, |k| {
                    let (o, s) = fields[k];
                    s.decode(&row[o..o + s.size()], *big) as f32
                });
            }
        }
        return Ok(());
    }
    // Where each property's value goes among the row's f_rest_*.
    let rest_of: Vec<Option<usize>> = (0..element.properties.len())
        .map(|a| {
            rest.as_ref()
                .and_then(|(_, at)| at.iter().position(|&x| x == a))
        })
        .collect();
    let mut row_rest = vec![0.0f32; rest.as_ref().map_or(0, |(_, at)| at.len())];
    for i in 0..n {
        for ((p, slot), rest_slot) in element.properties.iter().zip(&slots).zip(&rest_of) {
            match p.kind {
                Kind::Scalar(s) => {
                    let v = body.value(s)?;
                    if let Some(slot) = slot {
                        out.columns[*slot].push(v as f32);
                    }
                    if let Some(k) = rest_slot {
                        row_rest[*k] = v as f32;
                    }
                }
                Kind::List { count, item } => {
                    for _ in 0..list_len(body.value(count)?)? {
                        body.value(item)?;
                    }
                }
            }
        }
        if rest.is_some() {
            push_harmonics(out, i, |k| row_rest[k]);
        }
    }
    Ok(())
}

fn build(v: Vertices, triangles: Vec<[u32; 3]>) -> Result<Content, LoadError> {
    if !(v.has(X) && v.has(X + 1) && v.has(X + 2)) {
        return Err(if v.count == 0 {
            LoadError::Empty
        } else {
            LoadError::Invalid("PLY vertices have no x, y, z")
        });
    }
    let n = v.count;
    let positions: Vec<Vec3> = (0..n)
        .map(|i| Vec3::new(v.get(X, i), v.get(X + 1, i), v.get(X + 2, i)))
        .collect();
    let colour = |i: usize, c: usize, default: u8| -> u8 {
        if !v.has(c) {
            default
        } else if v.float_colour {
            unit_to_u8(v.get(c, i))
        } else {
            v.get(c, i).clamp(0.0, 255.0) as u8
        }
    };
    let has_rgb = (RED..RED + 3).all(|c| v.has(c));
    let rgba = |i: usize| -> [u8; 4] {
        if has_rgb {
            [
                colour(i, RED, 255),
                colour(i, RED + 1, 255),
                colour(i, RED + 2, 255),
                colour(i, ALPHA, 255),
            ]
        } else {
            [220, 220, 220, 255]
        }
    };

    if (DC..DC + 3).all(|c| v.has(c)) {
        let scales = (0..n)
            .map(|i| {
                if (SCALE..SCALE + 3).all(|c| v.has(c)) {
                    Vec3::new(
                        v.get(SCALE, i).exp(),
                        v.get(SCALE + 1, i).exp(),
                        v.get(SCALE + 2, i).exp(),
                    )
                } else {
                    Vec3::splat(0.01)
                }
            })
            .collect();
        let has_rot = (ROT..ROT + 4).all(|c| v.has(c));
        let rotations = (0..n)
            .map(|i| {
                if has_rot {
                    unit_quat(
                        v.get(ROT, i),
                        v.get(ROT + 1, i),
                        v.get(ROT + 2, i),
                        v.get(ROT + 3, i),
                    )
                } else {
                    glam::Quat::IDENTITY
                }
            })
            .collect();
        let colors = (0..n)
            .map(|i| {
                let alpha = if v.has(OPACITY) {
                    unit_to_u8(sigmoid(v.get(OPACITY, i)))
                } else {
                    255
                };
                [
                    sh0_to_u8(v.get(DC, i)),
                    sh0_to_u8(v.get(DC + 1, i)),
                    sh0_to_u8(v.get(DC + 2, i)),
                    alpha,
                ]
            })
            .collect();
        return Ok(Content::Splats(
            Splats {
                positions,
                scales,
                rotations,
                colors,
                harmonics: v.harmonics,
            }
            .keep_finite(),
        ));
    }
    if !triangles.is_empty() {
        if triangles.iter().flatten().any(|&i| i as usize >= n) {
            return Err(LoadError::Invalid("PLY face refers to a missing vertex"));
        }
        finite_vertices(&positions, "PLY has numbers that aren't finite")?;
        return Ok(Content::Mesh(RefMesh {
            positions,
            triangles,
            colors: has_rgb.then(|| (0..n).map(rgba).collect()),
        }));
    }
    let colors = (0..n).map(rgba).collect();
    point_cloud(positions, colors).map(Content::Splats)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Position, f_dc, opacity, log scale and rotation (w, x, y, z).
    pub(crate) type Gaussian = ([f32; 3], [f32; 3], f32, [f32; 3], [f32; 4]);

    /// A binary 3DGS PLY, with one view-dependent term to skip.
    pub(crate) fn gaussian_ply(splats: &[Gaussian], big: bool) -> Vec<u8> {
        let mut out = format!(
            "ply\nformat {} 1.0\ncomment made by a test\nelement vertex {}\n",
            if big {
                "binary_big_endian"
            } else {
                "binary_little_endian"
            },
            splats.len()
        )
        .into_bytes();
        for name in [
            "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "f_rest_0", "opacity",
            "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
        ] {
            out.extend_from_slice(format!("property float {name}\n").as_bytes());
        }
        out.extend_from_slice(b"end_header\n");
        for (p, dc, opacity, scale, rot) in splats {
            let row = [
                p[0], p[1], p[2], 0.0, 0.0, 0.0, dc[0], dc[1], dc[2], 9.0, *opacity, scale[0],
                scale[1], scale[2], rot[0], rot[1], rot[2], rot[3],
            ];
            for v in row {
                out.extend_from_slice(&if big {
                    v.to_be_bytes()
                } else {
                    v.to_le_bytes()
                });
            }
        }
        out
    }

    fn three_splats(big: bool) -> Vec<u8> {
        gaussian_ply(
            &[
                (
                    [0.0, 0.0, 0.0],
                    [0.0, 0.0, 0.0],
                    0.0,
                    [0.0; 3],
                    [1.0, 0.0, 0.0, 0.0],
                ),
                (
                    [1.0, 2.0, 3.0],
                    [1.0, -1.0, 0.5],
                    10.0,
                    [-1.0, -2.0, -3.0],
                    [0.0, 2.0, 0.0, 0.0],
                ),
                ([-1.0, 0.5, 0.0], [5.0, 5.0, 5.0], -10.0, [0.0; 3], [0.0; 4]),
            ],
            big,
        )
    }

    #[test]
    fn gaussian_splats_in_both_byte_orders() {
        for big in [false, true] {
            let Content::Splats(s) = load_ply(&three_splats(big)).unwrap() else {
                panic!("splats expected");
            };
            assert_eq!(s.len(), 3);
            assert_eq!(s.positions[1], Vec3::new(1.0, 2.0, 3.0));
            // 0.5 + 0.2820948 * dc, then sigmoid(opacity).
            assert_eq!(s.colors[0], [128, 128, 128, 128]);
            assert_eq!(s.colors[1][0], unit_to_u8(0.5 + 0.282_094_8));
            assert_eq!(s.colors[1][3], 255);
            assert_eq!(s.colors[2], [255, 255, 255, 0]);
            assert!(
                (s.scales[1] - Vec3::new((-1f32).exp(), (-2f32).exp(), (-3f32).exp())).length()
                    < 1e-6
            );
            // Rotations come normalised; w comes first in the file.
            assert!((s.rotations[1] - glam::Quat::from_xyzw(1.0, 0.0, 0.0, 0.0)).length() < 1e-6);
            assert_eq!(s.rotations[2], glam::Quat::IDENTITY);
        }
    }

    #[test]
    fn ascii_mesh_with_colours_and_a_quad() {
        let text = "ply\r\nformat ascii 1.0\r\nelement vertex 4\r\nproperty float x\r\n\
            property float y\r\nproperty float z\r\nproperty uchar red\r\nproperty uchar green\r\n\
            property uchar blue\r\nelement face 1\r\nproperty list uchar int vertex_indices\r\n\
            end_header\r\n0 0 0 255 0 0\r\n1 0 0 0 255 0\r\n1 1 0 0 0 255\r\n0 1 0 9 9 9\r\n\
            4 0 1 2 3\r\n";
        let Content::Mesh(m) = load_ply(text.as_bytes()).unwrap() else {
            panic!("mesh expected");
        };
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        assert_eq!(m.colors.unwrap()[1], [0, 255, 0, 255]);
    }

    #[test]
    fn points_without_faces_become_round_splats() {
        let mut data = b"ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
            property double x\nproperty double y\nproperty double z\n\
            property float red\nproperty float green\nproperty float blue\nend_header\n"
            .to_vec();
        for (p, c) in [
            ([0.0f64, 0.0, 0.0], 1.0f32),
            ([1.0, 0.0, 0.0], 0.5),
            ([0.0, 1.0, 0.0], 0.0),
        ] {
            for v in p {
                data.extend_from_slice(&v.to_le_bytes());
            }
            for _ in 0..3 {
                data.extend_from_slice(&c.to_le_bytes());
            }
        }
        let Content::Splats(s) = load_ply(&data).unwrap() else {
            panic!("splats expected");
        };
        assert_eq!(s.len(), 3);
        assert_eq!(s.colors[1], [128, 128, 128, 255]);
        assert!(s.is_point_cloud());
    }

    #[test]
    fn truncated_or_broken_files_are_errors() {
        let good = three_splats(false);
        for end in 0..good.len() {
            assert!(load_ply(&good[..end]).is_err(), "cut at {end}");
        }
        let text = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\n\
            property float y\nproperty float z\nelement face 1\n\
            property list uchar int vertex_indices\nend_header\n0 0 0\n3 0 1 2\n";
        assert!(
            load_ply(text.as_bytes()).is_err(),
            "face index out of range"
        );
        let huge = "ply\nformat binary_little_endian 1.0\nelement vertex 4000000000\n\
            property float x\nproperty float y\nproperty float z\nend_header\n";
        assert!(
            load_ply(huge.as_bytes()).is_err(),
            "count longer than the data"
        );
        assert!(
            load_ply(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty quux x\nend_header\n")
                .is_err()
        );
        assert!(load_ply(b"not a ply\nend_header\n").is_err());
    }

    #[test]
    fn faces_cannot_point_past_the_vertices() {
        // Faces checked against 100 dummy vertices, then only 3 real ones.
        let mut file = String::from(
            "ply\nformat ascii 1.0\nelement vertex 100\nproperty float q\n\
             element face 1\nproperty list uchar int vertex_indices\n\
             element vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
             end_header\n",
        );
        file += &"0\n".repeat(100);
        file += "3 0 50 99\n0 0 0\n1 0 0\n0 1 0\n";
        assert!(load_ply(file.as_bytes()).is_err());
    }

    #[test]
    fn meshes_with_numbers_that_are_not_finite_are_errors() {
        let file = "ply\nformat ascii 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                    0 0 0\nnan 0 0\n0 1 0\n3 0 1 2\n";
        assert!(load_ply(file.as_bytes()).is_err());
        assert!(load_ply(file.replace("nan", "1").as_bytes()).is_ok());
    }
}
