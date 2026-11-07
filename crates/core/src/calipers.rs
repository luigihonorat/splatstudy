// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Calipers, as a sculptor uses them: set between two landmarks of the
//! model, then held against the sculpt to see whether its span is the same.
//!
//! A caliper's two ends are points of the clay's space, the space the
//! reference is placed in (see [`crate::reference::Alignment`]) and both
//! views look into. So a caliper set on the reference shows, unchanged, at
//! the same place over the sculpt, and its length is the same in both
//! views: where the clay doesn't reach its ends, the clay is too small
//! there. Moving the reference afterwards (its turn, its fit) leaves the
//! calipers where they are, as real calipers stay set when the cast is
//! turned.
//!
//! Calipers are not part of the sculpt: the timelapse never records them.
//! Scenes keep them in a chunk of their own (see [`crate::scene`]).

use glam::Vec3;

use crate::io::{ReadError, Reader, Writer};

/// The most calipers a scene may hold, so a damaged file can't ask for
/// more. Far more than a study needs.
pub const MAX_CALIPERS: usize = 256;

/// One caliper: its two ends, in the clay's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caliper {
    /// Its name in the list and over the views: 1, 2, 3…
    pub number: u32,
    pub ends: [Vec3; 2],
    /// Drawn in the views; hidden ones stay in the list.
    pub visible: bool,
}

impl Caliper {
    /// The span between its ends, in the clay's units.
    pub fn length(&self) -> f32 {
        self.ends[0].distance(self.ends[1])
    }
}

/// All the calipers, and whether each view shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Calipers {
    pub list: Vec<Caliper>,
    /// Indexed by view: the sculpt view's, then the reference view's.
    pub shown: [bool; 2],
}

impl Default for Calipers {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            shown: [true; 2],
        }
    }
}

impl Calipers {
    /// Adds a caliper between `ends`, numbered after the highest number in
    /// the list, and returns its index.
    pub fn add(&mut self, ends: [Vec3; 2]) -> usize {
        let number = self.list.iter().map(|c| c.number).max().unwrap_or(0) + 1;
        self.list.push(Caliper {
            number,
            ends,
            visible: true,
        });
        self.list.len() - 1
    }

    /// Removes caliper `index`, if there is one.
    pub fn remove(&mut self, index: usize) {
        if index < self.list.len() {
            self.list.remove(index);
        }
    }

    /// Removes them all; the next one is number 1 again.
    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// Caliper `index`'s length as a share of the first caliper's, the
    /// one the others are compared with; `None` for the first itself or
    /// when the first has no length.
    pub fn ratio(&self, index: usize) -> Option<f32> {
        let first = self.list.first()?.length();
        if index == 0 || first < 1e-6 {
            return None;
        }
        Some(self.list.get(index)?.length() / first)
    }

    /// Nothing to keep: no caliper, and both views show them.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// As a scene's `CALI` chunk keeps them: the views that show them,
    /// then each caliper. Anything added later goes after the list, where
    /// readers from before it stop.
    pub fn write(&self, w: &mut Writer) {
        w.u8(u8::from(self.shown[0]) | u8::from(self.shown[1]) << 1)
            .len(self.list.len());
        for c in &self.list {
            w.u32(c.number)
                .u8(u8::from(c.visible))
                .vec3(c.ends[0])
                .vec3(c.ends[1]);
        }
    }

    /// Reads what [`Calipers::write`] wrote. Calipers with ends that
    /// aren't numbers are dropped.
    pub fn read(r: &mut Reader) -> Result<Calipers, ReadError> {
        let flags = r.u8()?;
        let count = r.len(29)?;
        if count > MAX_CALIPERS {
            return Err(r.invalid("too many calipers"));
        }
        let mut list = Vec::with_capacity(count);
        for _ in 0..count {
            let number = r.u32()?;
            let visible = r.u8()? & 1 != 0;
            let ends = [r.vec3()?, r.vec3()?];
            if ends.iter().all(|e| e.is_finite()) {
                list.push(Caliper {
                    number,
                    ends,
                    visible,
                });
            }
        }
        Ok(Calipers {
            list,
            shown: [flags & 1 != 0, flags & 2 != 0],
        })
    }
}

/// A length as the calipers show it: three significant digits or so,
/// in the clay's units.
pub fn length_text(length: f32) -> String {
    if length >= 10.0 {
        format!("{length:.1}")
    } else if length >= 1.0 {
        format!("{length:.2}")
    } else {
        format!("{length:.3}")
    }
}

/// A ratio to the first caliper, as the panel shows it: `×1.32`.
pub fn ratio_text(ratio: f32) -> String {
    format!("×{ratio:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Calipers {
        let mut c = Calipers::default();
        c.add([Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0)]);
        c.add([Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 3.0)]);
        c
    }

    #[test]
    fn calipers_are_numbered_measured_and_compared_with_the_first() {
        let mut c = two();
        assert_eq!(c.list.iter().map(|c| c.number).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(c.list[0].length(), 2.0);
        assert_eq!(c.list[1].length(), 3.0);
        assert_eq!(c.ratio(0), None, "the first is the yardstick");
        assert_eq!(c.ratio(1), Some(1.5));
        assert_eq!(c.ratio(2), None);

        // A new one comes after the highest number left.
        c.remove(0);
        assert_eq!(c.list[0].number, 2);
        assert_eq!(c.ratio(0), None);
        let k = c.add([Vec3::ZERO, Vec3::X]);
        assert_eq!(c.list[k].number, 3);
        assert_eq!(c.ratio(k), Some(1.0 / 3.0));
        c.remove(9);
        assert_eq!(c.list.len(), 2, "no such caliper");

        c.list[0].visible = false;
        c.shown[1] = false;
        assert!(!c.is_default());
        c.clear();
        assert!(c.list.is_empty());
        c.shown[1] = true;
        assert!(c.is_default());
        let k = c.add([Vec3::ZERO; 2]);
        assert_eq!(c.list[k].number, 1);
    }

    #[test]
    fn calipers_round_trip_and_damage_is_caught() {
        let mut c = two();
        c.list[1].visible = false;
        c.shown = [false, true];
        let mut w = Writer::new();
        c.write(&mut w);
        let bytes = w.finish();
        assert_eq!(Calipers::read(&mut Reader::new(&bytes)).unwrap(), c);
        // Cut short anywhere: an error, never a panic.
        for end in 0..bytes.len() {
            assert!(Calipers::read(&mut Reader::new(&bytes[..end])).is_err());
        }
        // An end that isn't a number drops its caliper only.
        let mut c = two();
        c.list[0].ends[1] = Vec3::new(f32::NAN, 0.0, 0.0);
        let mut w = Writer::new();
        c.write(&mut w);
        let read = Calipers::read(&mut Reader::new(&w.finish())).unwrap();
        assert_eq!(read.list.len(), 1);
        assert_eq!(read.list[0].number, 2);
    }

    #[test]
    fn lengths_show_about_three_digits() {
        assert_eq!(length_text(0.08412), "0.084");
        assert_eq!(length_text(1.2345), "1.23");
        assert_eq!(length_text(12.345), "12.3");
        assert_eq!(ratio_text(1.3249), "×1.32");
    }
}
