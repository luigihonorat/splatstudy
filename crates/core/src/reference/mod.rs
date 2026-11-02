// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The reference: what the user sculpts from, as loaded from a file, with
//! how it is placed and who made it.
//!
//! Placing: a file comes in its own units and orientation. [`Alignment`]
//! turns it so up is +Y, centres it and scales it so the part inside the
//! crop box fits a sphere of radius 1 at the origin, the space the clay
//! lives in. The two views can then share one camera, the ghost lines up
//! with the clay, and clay made from the reference comes out the right size.

pub mod fit;
pub mod load;

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};

/// A loaded reference.
pub struct Reference {
    pub source: Source,
    pub content: Content,
    pub align: Alignment,
    /// The part kept, in the file's own coordinates; `None` keeps it all.
    pub crop: Option<Aabb>,
    pub credit: Credit,
}

/// The file a reference came from, kept whole so a scene can save it.
#[derive(Clone)]
pub struct Source {
    pub name: String,
    pub bytes: Arc<[u8]>,
}

pub enum Content {
    Splats(Splats),
    Mesh(RefMesh),
    Image(RefImage),
}

/// Gaussian splats (and point clouds, as small round ones), one entry per
/// splat in every list. Only the base colour is kept (no view-dependent
/// colour).
#[derive(Clone, Default)]
pub struct Splats {
    pub positions: Vec<Vec3>,
    /// Standard deviation along each local axis, in file units.
    pub scales: Vec<Vec3>,
    pub rotations: Vec<Quat>,
    /// sRGB colour, opacity in alpha.
    pub colors: Vec<[u8; 4]>,
}

/// A scanned or modelled mesh.
#[derive(Clone, Default)]
pub struct RefMesh {
    pub positions: Vec<Vec3>,
    /// Counter-clockwise seen from outside, where the file says so.
    pub triangles: Vec<[u32; 3]>,
    /// sRGB per vertex, if the file has colours.
    pub colors: Option<Vec<[u8; 4]>>,
}

/// A picture, RGBA8, rows top first.
#[derive(Clone, Default)]
pub struct RefImage {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}

/// Which way is up in the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
    /// Upside down, as many splat captures come.
    NegY,
}

/// How the file's coordinates map into the clay's space:
/// scale · turn about Y · up axis · move `centre` to the origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    pub up: UpAxis,
    /// Turn about the vertical, in radians.
    pub yaw: f32,
    /// The point that goes to the origin, in file coordinates.
    pub centre: Vec3,
    pub scale: f32,
}

impl Default for Alignment {
    fn default() -> Self {
        Self {
            up: UpAxis::Y,
            yaw: 0.0,
            centre: Vec3::ZERO,
            scale: 1.0,
        }
    }
}

impl Alignment {
    /// File coordinates to the clay's space.
    pub fn matrix(&self) -> Mat4 {
        use std::f32::consts::{FRAC_PI_2, PI};
        let up = match self.up {
            UpAxis::Y => Mat4::IDENTITY,
            UpAxis::Z => Mat4::from_rotation_x(-FRAC_PI_2),
            UpAxis::NegY => Mat4::from_rotation_x(PI),
        };
        Mat4::from_scale(Vec3::splat(self.scale))
            * Mat4::from_rotation_y(self.yaw)
            * up
            * Mat4::from_translation(-self.centre)
    }
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
}

/// Who made the reference and under which licence. Shown under the
/// reference view and in every export that shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credit {
    pub title: String,
    pub author: String,
    /// Such as "CC BY 4.0".
    pub licence: String,
    pub url: String,
}

/// A soft round lump of material in the clay's space, for turning splats
/// into clay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blob {
    pub centre: Vec3,
    pub radius: f32,
    /// 0..1.
    pub opacity: f32,
}

impl Reference {
    /// A reference as loaded: not yet placed, cropped or credited.
    pub fn new(source: Source, content: Content) -> Self {
        Self {
            source,
            content,
            align: Alignment::default(),
            crop: None,
            credit: Credit::default(),
        }
    }

    fn kept(&self, p: Vec3) -> bool {
        self.crop.is_none_or(|crop| crop.contains(p))
    }

    /// A mesh reference's triangles inside the crop box, in the clay's
    /// space; `None` for splats and images.
    pub fn world_triangles(&self) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
        let Content::Mesh(mesh) = &self.content else {
            return None;
        };
        let m = self.align.matrix();
        let positions = mesh
            .positions
            .iter()
            .map(|&p| m.transform_point3(p))
            .collect();
        let triangles = mesh
            .triangles
            .iter()
            .filter(|t| t.iter().all(|&i| self.kept(mesh.positions[i as usize])))
            .copied()
            .collect();
        Some((positions, triangles))
    }

    /// The splats inside the crop box as blobs in the clay's space, each
    /// as large as its longest axis; empty for meshes and images.
    pub fn world_blobs(&self) -> Vec<Blob> {
        let Content::Splats(splats) = &self.content else {
            return Vec::new();
        };
        let m = self.align.matrix();
        splats
            .positions
            .iter()
            .zip(&splats.scales)
            .zip(&splats.colors)
            .filter(|((p, _), _)| self.kept(**p))
            .map(|((&p, &scale), &color)| Blob {
                centre: m.transform_point3(p),
                radius: scale.max_element() * self.align.scale,
                opacity: f32::from(color[3]) / 255.0,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source {
            name: "test".into(),
            bytes: Arc::from([]),
        }
    }

    #[test]
    fn alignment_turns_z_up_to_y_up_then_centres_and_scales() {
        let align = Alignment {
            up: UpAxis::Z,
            centre: Vec3::new(10.0, 0.0, 0.0),
            scale: 0.5,
            ..Default::default()
        };
        let m = align.matrix();
        assert!(m.transform_point3(Vec3::new(10.0, 0.0, 0.0)).length() < 1e-6);
        let up = m.transform_point3(Vec3::new(10.0, 0.0, 2.0));
        assert!(up.distance(Vec3::new(0.0, 1.0, 0.0)) < 1e-5, "{up}");
        let flipped = Alignment {
            up: UpAxis::NegY,
            ..Default::default()
        };
        let up = flipped.matrix().transform_point3(Vec3::NEG_Y);
        assert!(up.distance(Vec3::Y) < 1e-6, "{up}");
    }

    #[test]
    fn crop_keeps_only_what_is_inside_the_box() {
        let splats = Splats {
            positions: vec![Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0)],
            scales: vec![Vec3::new(0.1, 0.2, 0.1); 2],
            rotations: vec![Quat::IDENTITY; 2],
            colors: vec![[255, 255, 255, 255], [255, 255, 255, 51]],
        };
        let mut reference = Reference::new(source(), Content::Splats(splats));
        reference.align.scale = 2.0;
        assert_eq!(reference.world_blobs().len(), 2);
        reference.crop = Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        });
        let blobs = reference.world_blobs();
        assert_eq!(blobs.len(), 1);
        assert!((blobs[0].radius - 0.4).abs() < 1e-6);
        assert!(reference.world_triangles().is_none());

        let mesh = RefMesh {
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::new(5.0, 0.0, 0.0)],
            triangles: vec![[0, 1, 2], [1, 3, 2]],
            colors: None,
        };
        let mut reference = Reference::new(source(), Content::Mesh(mesh));
        reference.crop = Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        });
        let (_, triangles) = reference.world_triangles().unwrap();
        assert_eq!(triangles, vec![[0, 1, 2]]);
    }
}
