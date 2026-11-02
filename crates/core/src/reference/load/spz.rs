// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! `.spz`: compressed Gaussian splats. A gzip stream holding a 16-byte
//! header, then each attribute for all splats in turn:
//!
//! - positions: 3 × 24-bit signed fixed point, `fractional_bits` after the point
//! - opacities: 1 byte, after the sigmoid
//! - colours: 3 bytes, the first spherical harmonic scaled by 0.15 around 0.5
//! - scales: 3 bytes, logarithms as `byte / 16 - 10`
//! - rotations: version 2 keeps x, y, z as bytes and w ≥ 0; version 3
//!   keeps the three smallest components in 32 bits
//! - higher spherical harmonics, which are skipped
//!
//! Coordinates are already Y up. The gzip wrapper is read here; the deflate
//! data goes to miniz_oxide.

use glam::{Quat, Vec3};

use super::{LoadError, MAX_SPLATS, sh0_to_u8, unit_quat};
use crate::io::Reader;
use crate::reference::Splats;

const MAGIC: u32 = 0x5053_474E; // "NGSP"
/// The bytes before the splats.
const HEADER: usize = 16;

/// The deflate data inside a gzip member, after its header.
fn deflated(bytes: &[u8]) -> Result<&[u8], LoadError> {
    const FHCRC: u8 = 1 << 1;
    const FEXTRA: u8 = 1 << 2;
    const FNAME: u8 = 1 << 3;
    const FCOMMENT: u8 = 1 << 4;
    let mut r = Reader::new(bytes);
    if r.take(2)? != [0x1F, 0x8B] {
        return Err(LoadError::Invalid("not a .spz file: not gzip"));
    }
    if r.u8()? != 8 {
        return Err(LoadError::Unsupported("gzip that isn't deflate"));
    }
    let flags = r.u8()?;
    r.skip(6)?; // time, extra flags, system
    if flags & FEXTRA != 0 {
        let n = usize::from(r.u16()?);
        r.skip(n)?;
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            while r.u8()? != 0 {}
        }
    }
    if flags & FHCRC != 0 {
        r.skip(2)?;
    }
    Ok(&bytes[r.position()..])
}

/// Up to the first `limit` bytes of the inflated data: never more, so a
/// small file can't make a huge buffer.
fn inflate(deflated: &[u8], limit: usize) -> Result<Vec<u8>, LoadError> {
    use miniz_oxide::inflate::{TINFLStatus, decompress_to_vec_with_limit};
    match decompress_to_vec_with_limit(deflated, limit) {
        Ok(data) => Ok(data),
        Err(e) if e.status == TINFLStatus::HasMoreOutput => Ok(e.output),
        Err(_) => Err(LoadError::Invalid("broken compressed data")),
    }
}

pub fn load_spz(bytes: &[u8]) -> Result<Splats, LoadError> {
    let deflated = deflated(bytes)?;
    // The header first, which says how much to inflate.
    let header = inflate(deflated, HEADER)?;
    let mut r = Reader::new(&header);
    if r.u32()? != MAGIC {
        return Err(LoadError::Invalid("not a .spz file"));
    }
    let version = r.u32()?;
    if !(2..=3).contains(&version) {
        return Err(LoadError::Unsupported("this .spz version"));
    }
    let n = r.u32()? as usize;
    let sh_degree = r.u8()?;
    let fractional_bits = r.u8()?;
    let _flags = r.u8()?;
    let _reserved = r.u8()?;
    if sh_degree > 3 || fractional_bits > 24 {
        return Err(LoadError::Invalid("bad .spz header"));
    }
    if n == 0 {
        return Err(LoadError::Empty);
    }
    if n > MAX_SPLATS {
        return Err(LoadError::Unsupported(
            "more splats than SplatStudy can show",
        ));
    }
    let rotation_size = if version == 2 { 3 } else { 4 };
    let per_splat = 9 + 1 + 3 + 3 + rotation_size;
    // Only the splats' own fields; the higher harmonics after them stay
    // compressed.
    let data = inflate(deflated, HEADER + n * per_splat)?;
    let mut r = Reader::new(&data[HEADER.min(data.len())..]);
    if n * per_splat > r.remaining() {
        return Err(LoadError::Invalid(".spz data shorter than its header says"));
    }
    let positions = r.take(n * 9)?;
    let alphas = r.take(n)?;
    let colours = r.take(n * 3)?;
    let scales = r.take(n * 3)?;
    let rotations = r.take(n * rotation_size)?;
    // Spherical harmonics beyond the base colour follow; not used.

    let unit = 1.0 / (1u32 << fractional_bits) as f32;
    let fixed = |b: &[u8]| {
        let v = i32::from(b[0]) | i32::from(b[1]) << 8 | i32::from(b[2]) << 16;
        // Sign-extend from 24 bits.
        ((v << 8) >> 8) as f32 * unit
    };
    let colour = |b: u8| sh0_to_u8((f32::from(b) / 255.0 - 0.5) / 0.15);
    let scale = |b: u8| (f32::from(b) / 16.0 - 10.0).exp();
    Ok(Splats {
        positions: positions
            .as_chunks::<9>()
            .0
            .iter()
            .map(|p| Vec3::new(fixed(&p[0..3]), fixed(&p[3..6]), fixed(&p[6..9])))
            .collect(),
        scales: scales
            .as_chunks::<3>()
            .0
            .iter()
            .map(|s| Vec3::new(scale(s[0]), scale(s[1]), scale(s[2])))
            .collect(),
        rotations: rotations
            .chunks_exact(rotation_size)
            .map(|q| {
                if version == 2 {
                    rotation_v2(q)
                } else {
                    rotation_v3(q)
                }
            })
            .collect(),
        colors: colours
            .as_chunks::<3>()
            .0
            .iter()
            .zip(alphas)
            .map(|(c, &a)| [colour(c[0]), colour(c[1]), colour(c[2]), a])
            .collect(),
    }
    .keep_finite())
}

/// x, y, z as bytes over -1..1; w is what is left, never negative.
fn rotation_v2(b: &[u8]) -> Quat {
    let c = |v: u8| f32::from(v) / 127.5 - 1.0;
    let (x, y, z) = (c(b[0]), c(b[1]), c(b[2]));
    let w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
    unit_quat(w, x, y, z)
}

/// "Smallest three": the top 2 bits say which of x, y, z, w is largest;
/// the other three follow from w down to x, 10 bits each (a sign bit over
/// 9 bits of magnitude, which is at most √½).
fn rotation_v3(b: &[u8]) -> Quat {
    let mut bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let largest = (bits >> 30) as usize;
    const MASK: u32 = (1 << 9) - 1;
    let mut q = [0.0f32; 4];
    let mut sum = 0.0;
    for i in (0..4).rev() {
        if i == largest {
            continue;
        }
        let magnitude = (bits & MASK) as f32 / MASK as f32 * std::f32::consts::FRAC_1_SQRT_2;
        let negative = (bits >> 9) & 1 == 1;
        bits >>= 10;
        q[i] = if negative { -magnitude } else { magnitude };
        sum += q[i] * q[i];
    }
    q[largest] = (1.0 - sum).max(0.0).sqrt();
    unit_quat(q[3], q[0], q[1], q[2])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::reference::Content;

    /// Wraps data in a gzip member, with a file name in the header.
    pub(crate) fn gzip(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1F, 0x8B, 8, 1 << 3, 0, 0, 0, 0, 0, 255];
        out.extend_from_slice(b"splats.raw\0");
        out.extend(miniz_oxide::deflate::compress_to_vec(data, 6));
        // CRC and length; not checked.
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out
    }

    fn encode_v3(q: Quat) -> [u8; 4] {
        let c = q.normalize().to_array(); // x, y, z, w
        let largest = (0..4)
            .max_by(|&a, &b| c[a].abs().total_cmp(&c[b].abs()))
            .unwrap();
        let sign = if c[largest] < 0.0 { -1.0 } else { 1.0 };
        let mut bits = largest as u32;
        for (i, &v) in c.iter().enumerate() {
            if i == largest {
                continue;
            }
            let v = v * sign;
            let m = ((v.abs() / std::f32::consts::FRAC_1_SQRT_2) * 511.0).round() as u32;
            bits = bits << 10 | u32::from(v < 0.0) << 9 | m.min(511);
        }
        bits.to_le_bytes()
    }

    /// Three splats: at the origin, at (1, -2, 0.5) and at (-3, 0, 0).
    pub(crate) fn three_splats(version: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&MAGIC.to_le_bytes());
        d.extend_from_slice(&version.to_le_bytes());
        d.extend_from_slice(&3u32.to_le_bytes());
        d.extend_from_slice(&[0, 12, 0, 0]);
        for p in [[0.0f32, 0.0, 0.0], [1.0, -2.0, 0.5], [-3.0, 0.0, 0.0]] {
            for v in p {
                let fixed = (v * 4096.0).round() as i32;
                d.extend_from_slice(&fixed.to_le_bytes()[..3]);
            }
        }
        d.extend_from_slice(&[255, 128, 0]);
        // 0.5 + 0.15 * dc: dc 0 → 127.5, which rounds to the middle grey.
        d.extend_from_slice(&[128, 128, 128, 255, 0, 128, 0, 0, 0]);
        // exp(160 / 16 - 10) = 1.
        d.extend_from_slice(&[160, 160, 160, 144, 144, 144, 0, 0, 0]);
        let turn = Quat::from_rotation_y(0.7);
        if version == 2 {
            d.extend_from_slice(&[128, 128, 128]);
            for c in [turn.x, turn.y, turn.z] {
                d.push(((c + 1.0) * 127.5).round() as u8);
            }
            d.extend_from_slice(&[128, 128, 128]);
        } else {
            d.extend(encode_v3(Quat::IDENTITY));
            d.extend(encode_v3(turn));
            d.extend(encode_v3(-Quat::from_rotation_x(2.5)));
        }
        gzip(&d)
    }

    #[test]
    fn versions_2_and_3() {
        for version in [2, 3] {
            let s = load_spz(&three_splats(version)).unwrap();
            assert_eq!(s.len(), 3);
            assert_eq!(s.positions[1], Vec3::new(1.0, -2.0, 0.5));
            assert_eq!(s.positions[2], Vec3::new(-3.0, 0.0, 0.0));
            assert_eq!(s.colors[0], [128, 128, 128, 255]);
            assert_eq!(s.colors[1][0], 255);
            assert_eq!(s.colors[1][3], 128);
            assert!((s.scales[0] - Vec3::ONE).length() < 1e-6);
            assert!((s.scales[1].x - (-1f32).exp()).abs() < 1e-5);
            assert!(s.rotations[0].angle_between(Quat::IDENTITY) < 0.02);
            let turn = Quat::from_rotation_y(0.7);
            assert!(s.rotations[1].angle_between(turn) < 0.02, "v{version}");
            if version == 3 {
                let q = Quat::from_rotation_x(2.5);
                assert!(s.rotations[2].angle_between(q) < 0.02);
            }
        }
    }

    #[test]
    fn broken_files_are_errors() {
        let good = three_splats(3);
        for end in 0..good.len() - 8 {
            assert!(load_spz(&good[..end]).is_err(), "cut at {end}");
        }
        assert!(load_spz(&gzip(b"NGSP")).is_err());
        assert!(load_spz(b"\x1F\x8B\x08\x00garbage garbage").is_err());
        let mut wrong_version = three_splats(3);
        wrong_version[0] = 0;
        assert!(load_spz(&wrong_version).is_err());
    }

    /// CRC-32 (the gzip one), bit by bit.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A splat as a capture holds it, before packing.
    struct Gaussian {
        position: Vec3,
        /// Natural logarithms of the standard deviations.
        log_scale: Vec3,
        rotation: Quat,
        /// Before the sigmoid.
        opacity: f32,
        /// The first spherical harmonic's coefficients.
        dc: [f32; 3],
    }

    /// Packs `splats` the way the published `.spz` description lays a file
    /// out, from the floats a capture holds rather than from the bytes this
    /// loader reads: a gzip member (no file name, with its CRC and size)
    /// around the 16-byte header, then positions, alphas, colours, scales,
    /// rotations and `sh_degree`'s higher harmonics, each for all splats.
    fn canonical(splats: &[Gaussian], version: u32, sh_degree: u8) -> Vec<u8> {
        let byte = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        let mut d = Vec::new();
        d.extend_from_slice(&0x5053_474E_u32.to_le_bytes());
        d.extend_from_slice(&version.to_le_bytes());
        d.extend_from_slice(&(splats.len() as u32).to_le_bytes());
        d.extend_from_slice(&[sh_degree, 12, 0, 0]);
        for s in splats {
            for v in s.position.to_array() {
                let fixed = (v * 4096.0).round() as i32;
                d.extend_from_slice(&fixed.to_le_bytes()[..3]);
            }
        }
        for s in splats {
            d.push(byte(255.0 / (1.0 + (-s.opacity).exp())));
        }
        for s in splats {
            d.extend(s.dc.map(|c| byte(c * 0.15 * 255.0 + 127.5)));
        }
        for s in splats {
            d.extend(s.log_scale.to_array().map(|l| byte((l + 10.0) * 16.0)));
        }
        for s in splats {
            let q = s.rotation.normalize();
            if version == 2 {
                let q = if q.w < 0.0 { -q } else { q };
                d.extend([q.x, q.y, q.z].map(|c| byte(c * 127.5 + 127.5)));
            } else {
                d.extend(encode_v3(q));
            }
        }
        let higher = match sh_degree {
            0 => 0,
            1 => 3,
            2 => 8,
            _ => 15,
        };
        // Arbitrary higher harmonics, which must be skipped whole.
        d.extend((0..splats.len() * higher * 3).map(|i| (i * 37 % 251) as u8));

        let mut gz = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
        gz.extend(miniz_oxide::deflate::compress_to_vec(&d, 9));
        gz.extend_from_slice(&crc32(&d).to_le_bytes());
        gz.extend_from_slice(&(d.len() as u32).to_le_bytes());
        gz
    }

    #[test]
    fn a_file_packed_as_published_reads_back() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let splats = [
            Gaussian {
                position: Vec3::new(0.25, -1.5, 3.0),
                log_scale: Vec3::new(-4.0, -5.5, -3.25),
                rotation: Quat::from_axis_angle(Vec3::new(1.0, 2.0, -0.5).normalize(), 1.1),
                opacity: 2.0,
                dc: [1.2, -0.4, 0.0],
            },
            Gaussian {
                position: Vec3::new(-1000.125, 0.0, 512.5),
                log_scale: Vec3::new(0.0, -9.0, 2.5),
                rotation: Quat::from_rotation_y(-2.8),
                opacity: -3.0,
                dc: [-1.7, 1.6, 0.3],
            },
            Gaussian {
                position: Vec3::new(1e-3, -2e-3, 7.0),
                log_scale: Vec3::splat(-6.0),
                rotation: Quat::from_xyzw(0.1, -0.7, 0.2, -0.6).normalize(),
                opacity: 0.0,
                dc: [0.5, 0.5, -0.5],
            },
        ];
        for (version, sh_degree) in [(2, 0), (2, 1), (3, 2), (3, 3)] {
            let file = canonical(&splats, version, sh_degree);
            let Ok(Content::Splats(s)) = super::super::load("capture.spz", &file) else {
                panic!("v{version} with degree {sh_degree} doesn't load");
            };
            assert_eq!(s.len(), splats.len());
            for (i, g) in splats.iter().enumerate() {
                let at = format!("v{version}, degree {sh_degree}, splat {i}");
                assert!(s.positions[i].distance(g.position) < 1e-3, "{at}");
                let scale = g.log_scale.exp();
                assert!(
                    ((s.scales[i] - scale) / scale).abs().max_element() < 0.04,
                    "{at}: {} vs {scale}",
                    s.scales[i]
                );
                let turned = s.rotations[i].angle_between(g.rotation.normalize());
                assert!(turned < 0.03, "{at}: {turned}");
                let [r, gr, b, a] = s.colors[i];
                for (got, dc) in [r, gr, b].into_iter().zip(g.dc) {
                    let want = sh0_to_u8(dc);
                    assert!(got.abs_diff(want) <= 2, "{at}: {got} vs {want}");
                }
                let alpha = 255.0 / (1.0 + (-g.opacity).exp());
                assert!((f32::from(a) - alpha).abs() <= 1.0, "{at}: {a} vs {alpha}");
            }
        }
        // As the format says, already Y up.
        let file = canonical(&splats, 3, 0);
        let content = super::super::load("capture.spz", &file).unwrap();
        assert_eq!(
            super::super::usual_up("capture.spz", &content),
            crate::reference::UpAxis::Y
        );
    }

    #[test]
    fn only_what_the_header_declares_is_inflated() {
        // Too many splats: refused from the header alone.
        let mut d = Vec::new();
        d.extend_from_slice(&MAGIC.to_le_bytes());
        d.extend_from_slice(&3u32.to_le_bytes());
        d.extend_from_slice(&(MAX_SPLATS as u32 + 1).to_le_bytes());
        d.extend_from_slice(&[0, 12, 0, 0]);
        assert_eq!(
            load_spz(&gzip(&d)).err(),
            Some(LoadError::Unsupported(
                "more splats than SplatStudy can show"
            ))
        );
        // A small file that inflates to 64 MiB of zeros after three splats
        // reads just the splats.
        let mut data = miniz_oxide::inflate::decompress_to_vec(
            &three_splats(3)[21..three_splats(3).len() - 8],
        )
        .unwrap();
        data.resize(64 << 20, 0);
        let bomb = gzip(&data);
        assert!(bomb.len() < 1 << 20);
        assert_eq!(load_spz(&bomb).unwrap().len(), 3);
    }

    #[test]
    fn small_values_round_trip_in_v3() {
        for q in [
            Quat::from_rotation_z(-1.0),
            Quat::from_axis_angle(Vec3::ONE.normalize(), 2.0),
        ] {
            let back = rotation_v3(&encode_v3(q));
            assert!(back.angle_between(q) < 0.02, "{q} → {back}");
        }
    }
}
