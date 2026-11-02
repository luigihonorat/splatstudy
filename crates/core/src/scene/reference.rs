// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The reference in a scene file: the original file, whole, then how it is
//! placed and who made it. The splats or mesh are not stored: opening the
//! scene loads the original file again, which keeps scenes as small as the
//! reference allows and lets better loaders improve old scenes.
//!
//! Fields: name, bytes, up axis (`u8`), yaw, centre, scale, crop (`u8`
//! present, then min and max), then the credit's title, author, licence and
//! link.
//!
//! An autosave keeps the file apart, by its content (see [`super::blob`]):
//! its `REFH` chunk has the same fields with a [`BlobKey`] in place of the
//! bytes.

use std::sync::Arc;

use super::blob::BlobKey;
use crate::io::{ReadError, Reader, Writer};
use crate::reference::{Aabb, Alignment, Content, Credit, Reference, Source, UpAxis};

/// A reference as a scene stores it: everything but the loaded content.
#[derive(Clone)]
pub struct SavedReference {
    pub source: Source,
    pub align: Alignment,
    pub crop: Option<Aabb>,
    pub credit: Credit,
}

impl SavedReference {
    /// The reference again, once `source` has been loaded into `content`;
    /// the saved placement and credit replace whatever loading guessed.
    pub fn into_reference(self, content: Content) -> Reference {
        Reference {
            source: self.source,
            content,
            align: self.align,
            crop: self.crop,
            credit: self.credit,
        }
    }
}

fn up_code(up: UpAxis) -> u8 {
    match up {
        UpAxis::Y => 0,
        UpAxis::Z => 1,
        UpAxis::NegY => 2,
    }
}

pub fn write_reference(w: &mut Writer, reference: &Reference) {
    w.str(&reference.source.name).bytes(&reference.source.bytes);
    write_placement(w, reference);
}

/// The reference with its file named by `key` instead of included.
pub fn write_reference_by_key(w: &mut Writer, reference: &Reference, key: BlobKey) {
    w.str(&reference.source.name);
    key.write(w);
    write_placement(w, reference);
}

/// What follows the file: placement, crop and credit.
fn write_placement(w: &mut Writer, reference: &Reference) {
    let Reference {
        align,
        crop,
        credit,
        source: _,
        content: _,
    } = reference;
    w.u8(up_code(align.up))
        .f32(align.yaw)
        .vec3(align.centre)
        .f32(align.scale);
    match crop {
        Some(crop) => w.u8(1).vec3(crop.min).vec3(crop.max),
        None => w.u8(0),
    };
    w.str(&credit.title)
        .str(&credit.author)
        .str(&credit.licence)
        .str(&credit.url);
}

/// The REF_ chunk's payload for `reference`.
pub fn encode_reference(reference: &Reference) -> Vec<u8> {
    let mut w = Writer::new();
    write_reference(&mut w, reference);
    w.finish()
}

pub fn read_reference(r: &mut Reader) -> Result<SavedReference, ReadError> {
    let name = r.str()?;
    let bytes: Arc<[u8]> = Arc::from(r.bytes()?);
    read_placement(r, Source { name, bytes })
}

/// Reads a `REFH` payload; `find` gives the file stored under its key.
/// `Ok(None)` when there is none, or one of another length: the scene
/// still opens, without its reference.
pub fn read_reference_by_key(
    r: &mut Reader,
    find: impl FnOnce(BlobKey) -> Option<Arc<[u8]>>,
) -> Result<Option<SavedReference>, ReadError> {
    let name = r.str()?;
    let key = BlobKey::read(r)?;
    // The rest is read before looking, so a damaged chunk is an error
    // whether or not the file is there.
    let empty = Source {
        name,
        bytes: Arc::from(&[][..]),
    };
    let saved = read_placement(r, empty)?;
    Ok(find(key)
        .filter(|bytes| bytes.len() as u64 == key.len)
        .map(|bytes| SavedReference {
            source: Source {
                bytes,
                ..saved.source
            },
            ..saved
        }))
}

/// The key of the file a `REFH` payload names.
pub fn key_in(payload: &[u8]) -> Result<BlobKey, ReadError> {
    let mut r = Reader::new(payload);
    r.str()?;
    BlobKey::read(&mut r)
}

fn read_placement(r: &mut Reader, source: Source) -> Result<SavedReference, ReadError> {
    let up = match r.u8()? {
        0 => UpAxis::Y,
        1 => UpAxis::Z,
        2 => UpAxis::NegY,
        _ => return Err(r.invalid("an unknown up axis")),
    };
    let yaw = r.f32()?;
    let centre = r.vec3()?;
    let scale = r.f32()?;
    if !(yaw.is_finite() && centre.is_finite() && scale.is_finite() && scale > 0.0) {
        return Err(r.invalid("a placement that can't be"));
    }
    let crop = match r.u8()? {
        0 => None,
        1 => {
            let (min, max) = (r.vec3()?, r.vec3()?);
            if !(min.is_finite() && max.is_finite()) {
                return Err(r.invalid("a crop box that can't be"));
            }
            Some(Aabb { min, max })
        }
        _ => return Err(r.invalid("a crop box that can't be")),
    };
    let credit = Credit {
        title: r.str()?,
        author: r.str()?,
        licence: r.str()?,
        url: r.str()?,
    };
    Ok(SavedReference {
        source,
        align: Alignment {
            up,
            yaw,
            centre,
            scale,
        },
        crop,
        credit,
    })
}

/// The reference stored in a REF_ chunk's payload, given a way to load its
/// file: `load(name, bytes)`. `Ok(None)` when the file no longer loads.
pub fn decode_reference(
    payload: &[u8],
    load: impl FnOnce(&str, &[u8]) -> Option<Content>,
) -> Result<Option<Reference>, ReadError> {
    let saved = read_reference(&mut Reader::new(payload))?;
    Ok(load(&saved.source.name, &saved.source.bytes).map(|content| saved.into_reference(content)))
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::reference::RefImage;

    fn reference() -> Reference {
        let mut reference = Reference::new(
            Source {
                name: "石膏像 bust.ply".into(),
                bytes: Arc::from(&b"ply\nformat binary_little_endian 1.0\n\0\x01\x02"[..]),
            },
            Content::Image(RefImage::default()),
        );
        reference.align = Alignment {
            up: UpAxis::Z,
            yaw: 0.75,
            centre: Vec3::new(1.0, -2.0, 0.5),
            scale: 0.125,
        };
        reference.crop = Some(Aabb {
            min: Vec3::splat(-3.0),
            max: Vec3::new(3.0, 4.0, 5.0),
        });
        reference.credit = Credit {
            title: "Bust of a woman".into(),
            author: "A. Museum".into(),
            licence: "CC BY 4.0".into(),
            url: "https://example.org/bust".into(),
        };
        reference
    }

    #[test]
    fn a_reference_round_trips_with_its_file_placement_and_credit() {
        let original = reference();
        let payload = encode_reference(&original);
        let back = decode_reference(&payload, |name, bytes| {
            assert_eq!(name, original.source.name);
            assert_eq!(bytes, &original.source.bytes[..]);
            Some(Content::Image(RefImage {
                size: [1, 1],
                rgba: vec![9; 4],
            }))
        })
        .expect("reads")
        .expect("loads");
        assert_eq!(back.align, original.align);
        assert_eq!(back.crop, original.crop);
        assert_eq!(back.credit, original.credit);
        assert!(matches!(back.content, Content::Image(ref i) if i.size == [1, 1]));
        assert_eq!(encode_reference(&back), payload);

        // Without a crop box too.
        let mut uncropped = reference();
        uncropped.crop = None;
        let saved = read_reference(&mut Reader::new(&encode_reference(&uncropped))).unwrap();
        assert_eq!(saved.crop, None);
    }

    #[test]
    fn a_reference_by_key_finds_its_file_or_goes_without() {
        let original = reference();
        let key = BlobKey::of(&original.source.bytes);
        let mut w = Writer::new();
        write_reference_by_key(&mut w, &original, key);
        let payload = w.finish();
        // The file isn't in it, only its key.
        assert!(payload.len() < encode_reference(&original).len());
        assert_eq!(key_in(&payload), Ok(key));

        let found = read_reference_by_key(&mut Reader::new(&payload), |k| {
            assert_eq!(k, key);
            Some(original.source.bytes.clone())
        })
        .expect("reads")
        .expect("found");
        assert_eq!(found.source.name, original.source.name);
        assert_eq!(found.source.bytes, original.source.bytes);
        assert_eq!(found.align, original.align);
        assert_eq!(found.crop, original.crop);
        assert_eq!(found.credit, original.credit);

        // Missing, or another file under the key: no reference, no error.
        let read = |find: Option<Arc<[u8]>>| {
            read_reference_by_key(&mut Reader::new(&payload), |_| find)
                .expect("reads")
                .is_none()
        };
        assert!(read(None));
        assert!(read(Some(Arc::from(&b"short"[..]))));
        // Damage is an error even when the file would be found.
        for end in 0..payload.len() {
            let mut r = Reader::new(&payload[..end]);
            assert!(
                read_reference_by_key(&mut r, |_| Some(original.source.bytes.clone())).is_err()
            );
        }
    }

    #[test]
    fn a_file_that_no_longer_loads_gives_no_reference() {
        let payload = encode_reference(&reference());
        assert!(matches!(decode_reference(&payload, |_, _| None), Ok(None)));
    }

    #[test]
    fn truncated_or_bad_references_are_errors() {
        let payload = encode_reference(&reference());
        for end in 0..payload.len() {
            assert!(read_reference(&mut Reader::new(&payload[..end])).is_err());
        }
        // An up axis that doesn't exist: right after the name and bytes.
        let r = reference();
        let at = 8 + r.source.name.len() + 8 + r.source.bytes.len();
        let mut bad = payload.clone();
        bad[at] = 7;
        assert!(read_reference(&mut Reader::new(&bad)).is_err());
    }
}
