// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! View-dependent colour: the spherical harmonics of degree 1 to 3 that
//! 3DGS captures carry beyond each splat's base colour, so a splat can
//! catch the light from one side and not from the other.
//!
//! A degree-3 capture has 45 numbers a splat (15 coefficients for each of
//! red, green and blue): 180 bytes as floats, more than the rest of the
//! splat. They are kept packed instead, ready for the GPU as they are:
//! each number in one signed byte, scaled by the splat's largest one,
//! which is kept as a half float. First come three bytes for what the base
//! colour lost when it was rounded and clamped to a byte (a bright splat
//! whose base is above white still dims to the right grey from the side
//! its higher terms darken), then the coefficients in order, red, green
//! and blue together. The scale is the last two bytes of the splat's last
//! word:
//!
//! | degree | coefficients | bytes used | words a splat |
//! |--------|--------------|------------|---------------|
//! | 1      | 3            | 12 + 2     | 4             |
//! | 2      | 8            | 27 + 2     | 8             |
//! | 3      | 15           | 48 + 2     | 13            |
//!
//! A byte keeps about 1/254 of the splat's largest coefficient: finer
//! than `.spz`, which keeps 5 bits for degree 1 and 4 for the rest.
//!
//! `splat.wgsl` reads them the same way; [`Harmonics::colour`] is the same
//! sum on the CPU, for tests.

use glam::Vec3;

/// The base colour's coefficient: colour = 0.5 + C0 · f_dc.
pub const SH_C0: f32 = 0.282_094_8;
const SH_C1: f32 = 0.488_602_52;
const SH_C2: [f32; 5] = [
    1.092_548_4,
    -1.092_548_4,
    0.315_391_57,
    -1.092_548_4,
    0.546_274_2,
];
const SH_C3: [f32; 7] = [
    -0.590_043_6,
    2.890_611_4,
    -0.457_045_8,
    0.373_176_33,
    -0.457_045_8,
    1.445_305_7,
    -0.590_043_6,
];

/// Each splat's view-dependent colour, packed (see the module's notes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Harmonics {
    /// 1 to 3; 0 when the file has none, and then there are no words.
    pub degree: u8,
    /// [`Harmonics::words_per_splat`] words for each splat in turn.
    pub words: Vec<u32>,
}

impl Harmonics {
    /// Ready for splats with harmonics up to `degree` (0 to 3).
    pub fn new(degree: u8) -> Self {
        Self {
            degree: degree.min(3),
            words: Vec::new(),
        }
    }

    /// Coefficients for each colour channel beyond the base colour: 3, 8
    /// or 15.
    pub fn coefficients(degree: u8) -> usize {
        let d = usize::from(degree.min(3));
        (d + 1) * (d + 1) - 1
    }

    /// How a degree is detected from a 3DGS PLY: the number of `f_rest_*`
    /// properties, all three channels together.
    pub fn degree_of_rest(count: usize) -> Option<u8> {
        (1..=3).find(|&d| 3 * Self::coefficients(d) == count)
    }

    /// Words of 32 bits a splat takes at `degree`; 0 at degree 0.
    pub fn words_per_splat(degree: u8) -> usize {
        match degree {
            0 => 0,
            // The residual and the coefficients, three bytes each, and two
            // bytes of scale, rounded up to whole words.
            d => (3 * (Self::coefficients(d) + 1) + 2).div_ceil(4),
        }
    }

    /// How many splats are packed.
    pub fn len(&self) -> usize {
        match Self::words_per_splat(self.degree) {
            0 => 0,
            w => self.words.len() / w,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Packs one more splat. `base` is its base colour before it was
    /// rounded and clamped (0.5 + C0 · f_dc); `stored` the byte the base
    /// colour keeps; `rest(j, c)` coefficient `j` (0-based, past the base
    /// colour) of channel `c`. Numbers that aren't finite count as 0.
    pub fn push(&mut self, base: [f32; 3], stored: [u8; 3], rest: impl Fn(usize, usize) -> f32) {
        let m = Self::coefficients(self.degree);
        let words = Self::words_per_splat(self.degree);
        if words == 0 {
            return;
        }
        let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
        let mut values = [0.0f32; 48];
        for c in 0..3 {
            values[c] = finite(base[c] - f32::from(stored[c]) / 255.0);
            for j in 0..m {
                values[3 * (j + 1) + c] = finite(rest(j, c));
            }
        }
        let used = 3 * (m + 1);
        let largest = values[..used].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let scale = f16_bits(largest);
        let unit = f16_value(scale);
        let mut bytes = [0u8; 52];
        if unit > 0.0 {
            for (b, v) in bytes.iter_mut().zip(&values[..used]) {
                *b = (v / unit * 127.0).round().clamp(-127.0, 127.0) as i8 as u8;
            }
        }
        let end = 4 * words;
        bytes[end - 2..end].copy_from_slice(&scale.to_le_bytes());
        self.words.extend(
            bytes[..end]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w)),
        );
    }

    /// Splat `i`'s words.
    pub fn splat(&self, i: usize) -> &[u32] {
        let w = Self::words_per_splat(self.degree);
        &self.words[i * w..(i + 1) * w]
    }

    /// Splat `i`'s words as they would be at a lower `degree` (or the
    /// same), appended to `out`: the higher terms dropped, the rest kept.
    pub fn push_splat_at(&self, i: usize, degree: u8, out: &mut Vec<u32>) {
        let degree = degree.min(self.degree);
        if degree == self.degree {
            out.extend_from_slice(self.splat(i));
            return;
        }
        let words = Self::words_per_splat(degree);
        if words == 0 {
            return;
        }
        let from: Vec<u8> = self.splat(i).iter().flat_map(|w| w.to_le_bytes()).collect();
        let used = 3 * (Self::coefficients(degree) + 1);
        let mut bytes = [0u8; 52];
        bytes[..used].copy_from_slice(&from[..used]);
        let end = 4 * words;
        bytes[end - 2..end].copy_from_slice(&from[from.len() - 2..]);
        out.extend(
            bytes[..end]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w)),
        );
    }

    /// Splat `i` unpacked: the base colour's residual, then each
    /// coefficient's red, green and blue.
    pub fn unpack(&self, i: usize) -> ([f32; 3], Vec<[f32; 3]>) {
        let bytes: Vec<u8> = self.splat(i).iter().flat_map(|w| w.to_le_bytes()).collect();
        let n = bytes.len();
        let unit = f16_value(u16::from_le_bytes([bytes[n - 2], bytes[n - 1]]));
        // As WGSL's unpack4x8snorm reads them.
        let value = |k: usize| (f32::from(bytes[k] as i8) / 127.0).max(-1.0) * unit;
        let triplet = |t: usize| [value(3 * t), value(3 * t + 1), value(3 * t + 2)];
        let m = Self::coefficients(self.degree);
        (triplet(0), (1..=m).map(triplet).collect())
    }

    /// Splat `i`'s colour as seen along `dir` (from the eye towards the
    /// splat, in the file's own coordinates), given the base colour kept
    /// in its byte: what `splat.wgsl` draws, with the 3DGS constants and
    /// the colour clamped at 0.
    pub fn colour(&self, i: usize, stored: [u8; 3], dir: Vec3) -> [f32; 3] {
        let (residual, coefficients) = self.unpack(i);
        let higher = view_dependent(self.degree, &coefficients, dir);
        std::array::from_fn(|c| (f32::from(stored[c]) / 255.0 + residual[c] + higher[c]).max(0.0))
    }

    /// Keeps the splats whose entry in `keep` is true.
    pub fn retain(&mut self, keep: &[bool]) {
        let w = Self::words_per_splat(self.degree);
        if w == 0 {
            return;
        }
        let mut k = keep.iter();
        let mut kept = Vec::with_capacity(self.words.len());
        for splat in self.words.chunks_exact(w) {
            if *k.next().unwrap_or(&false) {
                kept.extend_from_slice(splat);
            }
        }
        self.words = kept;
    }
}

/// The terms of degree 1 to `degree` of the spherical harmonics with
/// `coefficients` (red, green and blue each), along unit direction `dir`
/// from the eye to the splat, as 3DGS evaluates them: without the base
/// colour and its 0.5.
pub fn view_dependent(degree: u8, coefficients: &[[f32; 3]], dir: Vec3) -> [f32; 3] {
    let dir = dir.normalize_or_zero();
    let (x, y, z) = (dir.x, dir.y, dir.z);
    let mut basis = Vec::with_capacity(15);
    if degree >= 1 {
        basis.extend([-SH_C1 * y, SH_C1 * z, -SH_C1 * x]);
    }
    if degree >= 2 {
        let (xx, yy, zz) = (x * x, y * y, z * z);
        basis.extend([
            SH_C2[0] * x * y,
            SH_C2[1] * y * z,
            SH_C2[2] * (2.0 * zz - xx - yy),
            SH_C2[3] * x * z,
            SH_C2[4] * (xx - yy),
        ]);
    }
    if degree >= 3 {
        let (xx, yy, zz) = (x * x, y * y, z * z);
        basis.extend([
            SH_C3[0] * y * (3.0 * xx - yy),
            SH_C3[1] * x * y * z,
            SH_C3[2] * y * (4.0 * zz - xx - yy),
            SH_C3[3] * z * (2.0 * zz - 3.0 * xx - 3.0 * yy),
            SH_C3[4] * x * (4.0 * zz - xx - yy),
            SH_C3[5] * z * (xx - yy),
            SH_C3[6] * x * (xx - 3.0 * yy),
        ]);
    }
    let mut sum = [0.0; 3];
    for (b, k) in basis.iter().zip(coefficients) {
        for c in 0..3 {
            sum[c] += b * k[c];
        }
    }
    sum
}

/// A non-negative float as the nearest half float's bits; NaN and tiny
/// values as 0, huge ones as the largest half.
fn f16_bits(v: f32) -> u16 {
    const LARGEST: u16 = 0x7BFF; // 65504
    if v.is_nan() || v <= 0.0 {
        return 0;
    }
    if v >= 65_504.0 {
        return LARGEST;
    }
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    if exp < -14 {
        // Subnormal halves: steps of 2⁻²⁴.
        return (v * 16_777_216.0).round() as u16;
    }
    let mantissa = bits & 0x7F_FFFF;
    let half = (((exp + 15) as u32) << 10) | (mantissa >> 13);
    // Round to nearest; a carry moves into the exponent, as it should.
    let half = half + ((mantissa >> 12) & 1);
    half.min(u32::from(LARGEST)) as u16
}

/// A half float's value, for the non-negative ones [`f16_bits`] makes.
fn f16_value(h: u16) -> f32 {
    let exp = i32::from((h >> 10) & 0x1F);
    let mantissa = f32::from(h & 0x3FF);
    if exp == 0 {
        mantissa * (2f32).powi(-24)
    } else {
        (1.0 + mantissa / 1024.0) * (2f32).powi(exp - 15)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        for v in [1.0f32, 0.5, 0.123, 3.75, 1e-5, 2.0e-7, 1000.0, 65504.0] {
            let back = f16_value(f16_bits(v));
            assert!((back - v).abs() <= v * 1e-3 + 6e-8, "{v} → {back}");
        }
        assert_eq!(f16_bits(1.0), 0x3C00);
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(f32::NAN), 0);
        assert_eq!(f16_bits(1e9), 0x7BFF);
        // Rounding up across a power of two.
        assert_eq!(f16_bits(1.999_9), 0x4000);
    }

    #[test]
    fn sizes() {
        assert_eq!(Harmonics::degree_of_rest(9), Some(1));
        assert_eq!(Harmonics::degree_of_rest(24), Some(2));
        assert_eq!(Harmonics::degree_of_rest(45), Some(3));
        assert_eq!(Harmonics::degree_of_rest(1), None);
        assert_eq!(Harmonics::degree_of_rest(0), None);
        assert_eq!([0, 1, 2, 3].map(Harmonics::words_per_splat), [0, 4, 8, 13]);
    }

    /// Known values of the basis: along each axis only the matching
    /// degree-1 term is left, with 3DGS's signs.
    #[test]
    fn the_basis_matches_3dgs() {
        let one = |j: usize| -> Vec<[f32; 3]> {
            (0..15)
                .map(|k| if k == j { [1.0, 2.0, -1.0] } else { [0.0; 3] })
                .collect()
        };
        // Coefficient 0 goes with -y, 1 with z, 2 with -x.
        let r = view_dependent(1, &one(0), Vec3::Y);
        assert!((r[0] + SH_C1).abs() < 1e-6 && (r[1] + 2.0 * SH_C1).abs() < 1e-6);
        assert!((view_dependent(1, &one(1), Vec3::Z)[0] - SH_C1).abs() < 1e-6);
        assert!((view_dependent(1, &one(2), Vec3::X)[0] + SH_C1).abs() < 1e-6);
        assert!((view_dependent(1, &one(2), -Vec3::X)[0] - SH_C1).abs() < 1e-6);
        // Degree 2: (2z² - x² - y²) along z, xy along the diagonal.
        let r = view_dependent(2, &one(5), Vec3::Z);
        assert!((r[0] - 2.0 * SH_C2[2]).abs() < 1e-6);
        let d = Vec3::new(1.0, 1.0, 0.0);
        let r = view_dependent(2, &one(3), d);
        assert!((r[0] - SH_C2[0] * 0.5).abs() < 1e-6, "{r:?}");
        // Degree 3: z(2z² - 3x² - 3y²) along z; the terms are ignored at a
        // lower degree.
        let r = view_dependent(3, &one(11), Vec3::Z);
        assert!((r[0] - 2.0 * SH_C3[3]).abs() < 1e-6);
        assert_eq!(view_dependent(2, &one(11), Vec3::Z), [0.0; 3]);
        // x(x² - 3y²) along x.
        let r = view_dependent(3, &one(14), Vec3::X);
        assert!((r[0] - SH_C3[6]).abs() < 1e-6);
        // The sum a 3DGS reference implementation gives for a mixed set
        // along (0.36, 0.48, 0.8).
        let coefficients: Vec<[f32; 3]> = (0..15)
            .map(|k| {
                let k = k as f32;
                [0.1 * (k - 7.0), 0.05 * k, -0.02 * k * k + 0.3]
            })
            .collect();
        let r = view_dependent(3, &coefficients, Vec3::new(0.36, 0.48, 0.8));
        let want = reference_3dgs(&coefficients, [0.36, 0.48, 0.8]);
        for c in 0..3 {
            assert!((r[c] - want[c]).abs() < 1e-5, "{r:?} vs {want:?}");
        }
    }

    /// `computeColorFromSH` of the 3DGS rasteriser, in f64, written out
    /// term by term from its CUDA, without the base colour.
    fn reference_3dgs(sh: &[[f32; 3]], dir: [f64; 3]) -> [f32; 3] {
        let [x, y, z] = dir;
        let (xx, yy, zz, xy, yz, xz) = (x * x, y * y, z * z, x * y, y * z, x * z);
        let c1 = 0.488_602_511_902_919_9;
        let c2 = [
            1.092_548_430_592_079_2,
            -1.092_548_430_592_079_2,
            0.315_391_565_252_520_05,
            -1.092_548_430_592_079_2,
            0.546_274_215_296_039_6,
        ];
        let c3 = [
            -0.590_043_589_926_643_5,
            2.890_611_442_640_554,
            -0.457_045_799_464_465_8,
            0.373_176_332_590_115_4,
            -0.457_045_799_464_465_8,
            1.445_305_721_320_277,
            -0.590_043_589_926_643_5,
        ];
        std::array::from_fn(|c| {
            let s = |k: usize| f64::from(sh[k - 1][c]);
            let r = -c1 * y * s(1) + c1 * z * s(2) - c1 * x * s(3)
                + c2[0] * xy * s(4)
                + c2[1] * yz * s(5)
                + c2[2] * (2.0 * zz - xx - yy) * s(6)
                + c2[3] * xz * s(7)
                + c2[4] * (xx - yy) * s(8)
                + c3[0] * y * (3.0 * xx - yy) * s(9)
                + c3[1] * xy * z * s(10)
                + c3[2] * y * (4.0 * zz - xx - yy) * s(11)
                + c3[3] * z * (2.0 * zz - 3.0 * xx - 3.0 * yy) * s(12)
                + c3[4] * x * (4.0 * zz - xx - yy) * s(13)
                + c3[5] * z * (xx - yy) * s(14)
                + c3[6] * x * (xx - 3.0 * yy) * s(15);
            r as f32
        })
    }

    #[test]
    fn packing_keeps_the_coefficients_to_a_byte() {
        for degree in 1..=3 {
            let m = Harmonics::coefficients(degree);
            let mut h = Harmonics::new(degree);
            let rest = |j: usize, c: usize| (j as f32 * 0.37 + c as f32).sin() * 0.8;
            // A base above white, kept as 255.
            h.push([1.3, 0.5, -0.1], [255, 128, 0], rest);
            h.push([0.5; 3], [128; 3], |_, _| f32::NAN);
            assert_eq!(h.len(), 2);
            assert_eq!(h.words.len(), 2 * Harmonics::words_per_splat(degree));
            let (residual, coefficients) = h.unpack(0);
            let largest = 0.8f32;
            let step = largest / 127.0;
            for (got, want) in residual.iter().zip([0.3, 0.5 - 128.0 / 255.0, -0.1]) {
                assert!((got - want).abs() <= step, "{got} vs {want}");
            }
            assert_eq!(coefficients.len(), m);
            for (j, k) in coefficients.iter().enumerate() {
                for (c, v) in k.iter().enumerate() {
                    assert!((v - rest(j, c)).abs() <= step, "degree {degree}, {j}, {c}");
                }
            }
            // Not finite: nothing view-dependent, and no residual.
            let (residual, coefficients) = h.unpack(1);
            assert!(residual.iter().all(|&v| v.abs() <= 1.0 / 255.0));
            assert!(coefficients.iter().flatten().all(|&v| v == 0.0));

            // The colour along a direction: the base before clamping plus
            // the higher terms.
            let dir = Vec3::new(0.3, -0.5, 0.8).normalize();
            let exact = view_dependent(
                degree,
                &(0..m)
                    .map(|j| [rest(j, 0), rest(j, 1), rest(j, 2)])
                    .collect::<Vec<_>>(),
                dir,
            );
            let got = h.colour(0, [255, 128, 0], dir);
            for c in 0..3 {
                let want = ([1.3, 0.5, -0.1][c] + exact[c]).max(0.0);
                assert!((got[c] - want).abs() < 0.02, "{got:?} vs {want}");
            }

            // Lower degrees drop the higher terms and keep the rest.
            for lower in 1..degree {
                let mut out = Vec::new();
                h.push_splat_at(0, lower, &mut out);
                let low = Harmonics {
                    degree: lower,
                    words: out,
                };
                let (r, k) = low.unpack(0);
                assert_eq!(r, h.unpack(0).0);
                assert_eq!(k[..], h.unpack(0).1[..Harmonics::coefficients(lower)]);
            }
        }
    }

    #[test]
    fn retain_keeps_whole_splats() {
        let mut h = Harmonics::new(1);
        for i in 0..3 {
            h.push([0.5; 3], [128; 3], |j, _| (i * 3 + j) as f32 * 0.1 + 0.05);
        }
        let third = h.splat(2).to_vec();
        h.retain(&[false, true, true]);
        assert_eq!(h.len(), 2);
        assert_eq!(h.splat(1), &third[..]);
    }
}
