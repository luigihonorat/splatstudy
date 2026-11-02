// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The recorded session in a scene file: a count, then each entry as its
//! time (`f64`), a kind byte and the kind's fields.
//!
//! The values are stored exactly (no rounding), because the timelapse
//! replays them on the saved clay and must land on the same vertices.
//!
//! The scene module reads and writes logs only through [`write_entries`]
//! and [`read_entries`], so the encoding can move next to [`Event`] later
//! without touching the file format code.

use crate::io::{ReadError, Reader, Writer};
use crate::sculpt::{Brush, Dab, Entry, Event};

const BEGIN: u8 = 0;
const DAB: u8 = 1;
const GRAB: u8 = 2;
const DRAG: u8 = 3;
const END: u8 = 4;
const UNDO: u8 = 5;
const REDO: u8 = 6;
const REFRESH: u8 = 7;

/// Smallest encoded entry: a time and a kind.
const MIN_ENTRY: usize = 9;

/// Each brush's number in files; never reorder.
fn brush_code(brush: Brush) -> u8 {
    match brush {
        Brush::Move => 0,
        Brush::Clay => 1,
        Brush::Flatten => 2,
        Brush::Smooth => 3,
        Brush::Inflate => 4,
        Brush::Crease => 5,
        Brush::Pinch => 6,
    }
}

fn brush_from(code: u8) -> Option<Brush> {
    Some(match code {
        0 => Brush::Move,
        1 => Brush::Clay,
        2 => Brush::Flatten,
        3 => Brush::Smooth,
        4 => Brush::Inflate,
        5 => Brush::Crease,
        6 => Brush::Pinch,
        _ => return None,
    })
}

pub fn write_entries(w: &mut Writer, entries: &[Entry]) {
    w.len(entries.len());
    for entry in entries {
        write_entry(w, entry);
    }
}

pub fn write_entry(w: &mut Writer, entry: &Entry) {
    w.f64(entry.time);
    match entry.event {
        Event::Begin => {
            w.u8(BEGIN);
        }
        Event::Dab(dab) => {
            let flags = u8::from(dab.invert) | u8::from(dab.symmetry) << 1;
            w.u8(DAB)
                .u8(brush_code(dab.brush))
                .vec3(dab.centre)
                .f32(dab.radius)
                .f32(dab.strength)
                .u8(flags)
                .vec3(dab.direction);
        }
        Event::Grab {
            centre,
            radius,
            symmetry,
        } => {
            w.u8(GRAB).vec3(centre).f32(radius).u8(u8::from(symmetry));
        }
        Event::Drag(offset) => {
            w.u8(DRAG).vec3(offset);
        }
        Event::End => {
            w.u8(END);
        }
        Event::Undo => {
            w.u8(UNDO);
        }
        Event::Redo => {
            w.u8(REDO);
        }
        Event::Refresh => {
            w.u8(REFRESH);
        }
    }
}

pub fn read_entries(r: &mut Reader) -> Result<Vec<Entry>, ReadError> {
    let n = r.len(MIN_ENTRY)?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        entries.push(read_entry(r)?);
    }
    Ok(entries)
}

pub fn read_entry(r: &mut Reader) -> Result<Entry, ReadError> {
    let time = r.f64()?;
    if !time.is_finite() {
        return Err(r.invalid("a time that isn't a number"));
    }
    let event = match r.u8()? {
        BEGIN => Event::Begin,
        DAB => {
            let brush = brush_from(r.u8()?).ok_or_else(|| r.invalid("an unknown brush"))?;
            let centre = r.vec3()?;
            let radius = r.f32()?;
            let strength = r.f32()?;
            let flags = r.u8()?;
            let direction = r.vec3()?;
            Event::Dab(Dab {
                brush,
                centre,
                radius,
                strength,
                invert: flags & 1 != 0,
                symmetry: flags & 2 != 0,
                direction,
            })
        }
        GRAB => Event::Grab {
            centre: r.vec3()?,
            radius: r.f32()?,
            symmetry: r.u8()? != 0,
        },
        DRAG => Event::Drag(r.vec3()?),
        END => Event::End,
        UNDO => Event::Undo,
        REDO => Event::Redo,
        REFRESH => Event::Refresh,
        _ => return Err(r.invalid("an unknown kind of log entry")),
    };
    Ok(Entry { time, event })
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    fn every_kind() -> Vec<Entry> {
        let mut events = vec![Event::Begin];
        for (i, brush) in Brush::ALL.into_iter().enumerate() {
            events.push(Event::Dab(Dab {
                brush,
                centre: Vec3::new(0.1, -0.2, 0.3 + i as f32),
                radius: 0.25,
                strength: 1.0 / 3.0,
                invert: i % 2 == 0,
                symmetry: i % 3 == 0,
                direction: Vec3::new(0.0, 0.6, 0.8),
            }));
        }
        events.extend([
            Event::Grab {
                centre: Vec3::ONE,
                radius: 0.5,
                symmetry: true,
            },
            Event::Drag(Vec3::new(1e-7, -0.0, f32::MAX)),
            Event::End,
            Event::Undo,
            Event::Redo,
            Event::Refresh,
        ]);
        events
            .into_iter()
            .enumerate()
            .map(|(i, event)| Entry {
                time: 0.1 * i as f64,
                event,
            })
            .collect()
    }

    fn encode(entries: &[Entry]) -> Vec<u8> {
        let mut w = Writer::new();
        write_entries(&mut w, entries);
        w.finish()
    }

    #[test]
    fn every_kind_of_entry_round_trips_exactly() {
        let entries = every_kind();
        let data = encode(&entries);
        let back = read_entries(&mut Reader::new(&data)).expect("reads");
        assert_eq!(back.len(), entries.len());
        // Debug prints every field at full precision.
        assert_eq!(format!("{back:?}"), format!("{entries:?}"));
        assert_eq!(encode(&back), data);
    }

    #[test]
    fn truncated_or_unknown_entries_are_errors() {
        let data = encode(&every_kind());
        for end in 0..data.len() {
            assert!(read_entries(&mut Reader::new(&data[..end])).is_err());
        }
        let mut w = Writer::new();
        w.len(1).f64(0.0).u8(200);
        assert!(read_entries(&mut Reader::new(&w.finish())).is_err());
        let mut w = Writer::new();
        w.len(1).f64(0.0).u8(DAB).u8(99);
        assert!(read_entries(&mut Reader::new(&w.finish())).is_err());
        let mut w = Writer::new();
        w.len(1).f64(f64::NAN).u8(BEGIN);
        assert!(read_entries(&mut Reader::new(&w.finish())).is_err());
    }
}
