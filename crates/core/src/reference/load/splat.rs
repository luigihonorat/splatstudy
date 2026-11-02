// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! `.splat`: 32-byte records with no header, as web viewers write them:
//! position (3 × f32), scale (3 × f32, not logarithms), colour and opacity
//! (4 × u8), rotation w, x, y, z (4 × u8, mapped from -1..1 to 0..255).

use super::{LoadError, unit_quat};
use crate::io::Reader;
use crate::reference::Splats;

const RECORD: usize = 32;

pub fn load_splat(bytes: &[u8]) -> Result<Splats, LoadError> {
    if bytes.is_empty() {
        return Err(LoadError::Empty);
    }
    if !bytes.len().is_multiple_of(RECORD) {
        return Err(LoadError::Invalid(
            "not a .splat file: size isn't whole records",
        ));
    }
    let n = bytes.len() / RECORD;
    let mut s = Splats {
        positions: Vec::with_capacity(n),
        scales: Vec::with_capacity(n),
        rotations: Vec::with_capacity(n),
        colors: Vec::with_capacity(n),
    };
    let mut r = Reader::new(bytes);
    let byte_to_unit = |b: u8| (f32::from(b) - 128.0) / 128.0;
    for _ in 0..n {
        s.positions.push(r.vec3()?);
        s.scales.push(r.vec3()?.abs());
        let c = r.take(4)?;
        s.colors.push([c[0], c[1], c[2], c[3]]);
        let q = r.take(4)?;
        s.rotations.push(unit_quat(
            byte_to_unit(q[0]),
            byte_to_unit(q[1]),
            byte_to_unit(q[2]),
            byte_to_unit(q[3]),
        ));
    }
    Ok(s.keep_finite())
}

#[cfg(test)]
pub(crate) mod tests {
    use glam::Vec3;

    use super::*;
    use crate::io::Writer;

    pub(crate) fn record(
        w: &mut Writer,
        p: [f32; 3],
        scale: [f32; 3],
        rgba: [u8; 4],
        rot: [u8; 4],
    ) {
        w.vec3(Vec3::from(p))
            .vec3(Vec3::from(scale))
            .raw(&rgba)
            .raw(&rot);
    }

    #[test]
    fn three_records() {
        let mut w = Writer::new();
        record(
            &mut w,
            [0.0; 3],
            [0.1; 3],
            [255, 0, 0, 255],
            [255, 128, 128, 128],
        );
        record(
            &mut w,
            [1.0, 2.0, 3.0],
            [0.1, 0.2, 0.3],
            [0, 255, 0, 128],
            [128, 255, 128, 128],
        );
        record(&mut w, [-1.0; 3], [0.5; 3], [1, 2, 3, 4], [128; 4]);
        let s = load_splat(&w.finish()).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s.positions[1], Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(s.scales[1], Vec3::new(0.1, 0.2, 0.3));
        assert_eq!(s.colors[1], [0, 255, 0, 128]);
        assert!((s.rotations[0].w - 1.0).abs() < 1e-6);
        assert!((s.rotations[1].x - 1.0).abs() < 1e-6);
        // All 128s is a zero quaternion: no turn.
        assert_eq!(s.rotations[2], glam::Quat::IDENTITY);
    }

    #[test]
    fn partial_records_are_errors() {
        assert!(load_splat(&[0; 31]).is_err());
        assert!(load_splat(&[0; 33]).is_err());
        assert!(load_splat(&[]).is_err());
    }
}
