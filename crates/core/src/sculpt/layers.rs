// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Values painted on the clay, one per vertex, 0 to 1: the mask, which
//! holds the clay still under every brush, and the chalk, a tint that
//! sculpting wipes off, like the blue of a carpenter's chalk line, so what
//! was worked and what is still untouched show apart.
//!
//! Both are 0 by default and take no memory until something is painted.
//! They travel with the mesh: through undo, checkpoints and scene files,
//! up and down the subdivision levels, and onto a remesh.

use crate::io::{ReadError, Reader, Writer};

use super::Sculpt;
use super::mesh::{Mesh, Stamp};

/// A value per vertex, 0 to 1; empty, taking no memory, while every value
/// is 0. Two layers are equal when their values are, however they are
/// held.
#[derive(Clone, Debug, Default)]
pub struct Layer(Vec<f32>);

impl PartialEq for Layer {
    fn eq(&self, other: &Self) -> bool {
        let n = self.0.len().max(other.0.len());
        (0..n).all(|v| self.get(v) == other.get(v))
    }
}

impl Layer {
    /// Every one of `n` vertices at `value`.
    pub fn filled(n: usize, value: f32) -> Layer {
        if value == 0.0 {
            Layer::default()
        } else {
            Layer(vec![value; n])
        }
    }

    /// The value at vertex `v`.
    pub fn get(&self, v: usize) -> f32 {
        self.0.get(v).copied().unwrap_or(0.0)
    }

    /// Sets vertex `v` of a layer over `n` vertices.
    pub fn set(&mut self, v: usize, value: f32, n: usize) {
        if self.0.is_empty() {
            if value == 0.0 {
                return;
            }
            self.0 = vec![0.0; n];
        }
        self.0[v] = value;
    }

    /// True while every value is 0 and none was ever set otherwise.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The values, one per vertex; empty while all are 0.
    pub fn values(&self) -> &[f32] {
        &self.0
    }

    /// 1 minus each value, over `n` vertices.
    pub fn inverted(&self, n: usize) -> Layer {
        if self.0.is_empty() {
            return Layer::filled(n, 1.0);
        }
        Layer(self.0.iter().map(|&x| 1.0 - x).collect())
    }

    /// Roughly the memory it holds.
    pub fn bytes(&self) -> usize {
        self.0.len() * 4
    }

    /// The layer on the level the clay steps to, `to`, from this layer on
    /// the clay's `from` vertices: a level up keeps the vertices below
    /// first and adds one per edge, which takes the average of the edge's
    /// two ends (its neighbours among the vertices below); a level down
    /// keeps the first vertices only.
    fn carried(&self, from: usize, to: &Mesh) -> Layer {
        if self.0.is_empty() {
            return Layer::default();
        }
        let n = to.positions.len();
        if n <= from {
            return Layer(self.0[..n].to_vec());
        }
        let mut values = self.0.clone();
        values.extend((from..n).map(|v| {
            let (sum, count) = to
                .neighbours
                .of(v as u32)
                .iter()
                .filter(|&&u| (u as usize) < from)
                .fold((0.0, 0u32), |(s, c), &u| (s + self.0[u as usize], c + 1));
            if count == 0 { 0.0 } else { sum / count as f32 }
        }));
        Layer(values)
    }

    /// Writes the values of a layer over `n` vertices.
    fn write(&self, w: &mut Writer, n: usize) {
        if self.0.len() == n {
            w.u32s(bytemuck::cast_slice(&self.0));
        } else {
            (0..n).for_each(|v| {
                w.f32(self.get(v));
            });
        }
    }

    /// Reads what [`Layer::write`] wrote for `n` vertices.
    fn read(r: &mut Reader, n: usize) -> Result<Layer, ReadError> {
        if r.remaining() / 4 < n {
            return Err(r.invalid("a layer cut short"));
        }
        let values = (0..n)
            .map(|_| match r.f32()? {
                x if (0.0..=1.0).contains(&x) => Ok(x),
                _ => Err(r.invalid("a layer value that isn't between 0 and 1")),
            })
            .collect::<Result<Vec<f32>, _>>()?;
        Ok(Layer(values))
    }
}

/// Which of the clay's layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mask,
    Chalk,
}

/// What a whole-layer edit does (see `Sculpt::edit_layer`). The order is
/// the saved code: never reorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerOp {
    /// Every vertex free again.
    ClearMask,
    /// The masked clay free and the free clay masked.
    InvertMask,
    /// Chalk over all the clay.
    AddChalk,
    /// No chalk anywhere.
    RemoveChalk,
}

impl LayerOp {
    pub const ALL: [LayerOp; 4] = [
        LayerOp::ClearMask,
        LayerOp::InvertMask,
        LayerOp::AddChalk,
        LayerOp::RemoveChalk,
    ];

    /// The layer it changes.
    pub fn kind(self) -> Kind {
        match self {
            LayerOp::ClearMask | LayerOp::InvertMask => Kind::Mask,
            LayerOp::AddChalk | LayerOp::RemoveChalk => Kind::Chalk,
        }
    }

    /// `layer` after this edit, over `n` vertices.
    pub fn apply(self, layer: &Layer, n: usize) -> Layer {
        match self {
            LayerOp::ClearMask | LayerOp::RemoveChalk => Layer::default(),
            LayerOp::InvertMask => layer.inverted(n),
            LayerOp::AddChalk => Layer::filled(n, 1.0),
        }
    }
}

/// The clay's layers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layers {
    /// How still each vertex is held: 1 is frozen, 0 free.
    pub mask: Layer,
    /// How much chalk is left on each vertex.
    pub chalk: Layer,
}

impl Layers {
    /// True while neither layer holds anything.
    pub fn is_empty(&self) -> bool {
        self.mask.is_empty() && self.chalk.is_empty()
    }

    pub fn get(&self, kind: Kind) -> &Layer {
        match kind {
            Kind::Mask => &self.mask,
            Kind::Chalk => &self.chalk,
        }
    }

    pub fn get_mut(&mut self, kind: Kind) -> &mut Layer {
        match kind {
            Kind::Mask => &mut self.mask,
            Kind::Chalk => &mut self.chalk,
        }
    }

    /// Roughly the memory they hold.
    pub fn bytes(&self) -> usize {
        self.mask.bytes() + self.chalk.bytes()
    }

    /// The layers on another level of the clay (see [`Layer::carried`]).
    pub fn carried(&self, from: usize, to: &Mesh) -> Layers {
        Layers {
            mask: self.mask.carried(from, to),
            chalk: self.chalk.carried(from, to),
        }
    }

    /// True if each layer is empty or has one value per vertex of `n`.
    pub fn fits(&self, n: usize) -> bool {
        [&self.mask, &self.chalk]
            .iter()
            .all(|l| l.is_empty() || l.0.len() == n)
    }

    /// Writes the layers of clay with `n` vertices: a byte saying which
    /// hold anything (bit 0 the mask, bit 1 the chalk), then the values
    /// of each of those, as `f32`s.
    pub fn write(&self, w: &mut Writer, n: usize) {
        let mask = !self.mask.is_empty();
        let chalk = !self.chalk.is_empty();
        w.u8(u8::from(mask) | u8::from(chalk) << 1);
        if mask {
            self.mask.write(w, n);
        }
        if chalk {
            self.chalk.write(w, n);
        }
    }

    /// Reads what [`Layers::write`] wrote for `n` vertices.
    pub fn read(r: &mut Reader, n: usize) -> Result<Layers, ReadError> {
        let which = r.u8()?;
        if which > 3 {
            return Err(r.invalid("unknown layers"));
        }
        let mut layers = Layers::default();
        if which & 1 != 0 {
            layers.mask = Layer::read(r, n)?;
        }
        if which & 2 != 0 {
            layers.chalk = Layer::read(r, n)?;
        }
        Ok(layers)
    }
}

impl Sculpt {
    /// The clay's mask and chalk moved onto `mesh`, a remesh of it: each
    /// vertex takes the values of the clay's vertex nearest to it, found
    /// with the clay's own hierarchy. Empty if the clay has neither.
    pub fn layers_for(&self, mesh: &Mesh) -> Layers {
        let from = &self.mesh;
        if from.layers.is_empty() {
            return Layers::default();
        }
        // The search starts about one edge out, and widens until it
        // finds a vertex.
        let edges: Vec<f32> = from
            .triangles
            .iter()
            .take(1024)
            .map(|&[a, b, _]| from.positions[a as usize].distance(from.positions[b as usize]))
            .collect();
        let edge = edges.iter().sum::<f32>() / edges.len().max(1) as f32;
        let (lo, hi) = from
            .positions
            .iter()
            .fold((glam::Vec3::MAX, glam::Vec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        let widest = (hi - lo).length().max(edge);
        let mut stamp = Stamp::new(from.positions.len());
        let mut found = Vec::new();
        let n = mesh.positions.len();
        let mut layers = Layers::default();
        for (v, &p) in mesh.positions.iter().enumerate() {
            let mut radius = edge.max(1e-6);
            loop {
                found.clear();
                self.bvh
                    .vertices_in_sphere(from, p, radius, &mut stamp, &mut found);
                if !found.is_empty() || radius > widest {
                    break;
                }
                radius *= 2.0;
            }
            let nearest = found.iter().copied().min_by(|&a, &b| {
                let d = |u: u32| from.positions[u as usize].distance_squared(p);
                d(a).total_cmp(&d(b)).then(a.cmp(&b))
            });
            if let Some(u) = nearest {
                let u = u as usize;
                layers.mask.set(v, from.layers.mask.get(u), n);
                layers.chalk.set(v, from.layers.chalk.get(u), n);
            }
        }
        layers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::subdivide::Split;

    #[test]
    fn an_empty_layer_equals_one_of_zeros_and_reads_as_zero() {
        let mut layer = Layer::default();
        assert!(layer.is_empty());
        layer.set(3, 0.0, 10);
        assert!(layer.is_empty(), "a zero takes no memory");
        assert_eq!(layer, Layer(vec![0.0; 10]));
        layer.set(3, 0.5, 10);
        assert_eq!(layer.get(3), 0.5);
        assert_eq!(layer.values().len(), 10);
        assert_ne!(layer, Layer::default());
        assert_eq!(layer.inverted(10).get(3), 0.5);
        assert_eq!(layer.inverted(10).get(4), 1.0);
        assert_eq!(Layer::default().inverted(4), Layer::filled(4, 1.0));
    }

    #[test]
    fn layers_go_up_a_level_as_averages_and_down_as_the_first_vertices() {
        let coarse = Mesh::icosphere(1);
        let n = coarse.positions.len();
        let split = Split::of(&coarse).unwrap();
        let fine = Mesh::from_ordered(
            split.positions(&coarse, &coarse.positions),
            split.triangles(&coarse),
        );
        let mut layers = Layers::default();
        for v in 0..n {
            layers.mask.set(v, (v % 3) as f32 / 2.0, n);
        }
        let up = layers.carried(n, &fine);
        assert!(up.fits(fine.positions.len()));
        assert!(up.chalk.is_empty());
        for v in 0..n {
            assert_eq!(up.mask.get(v), layers.mask.get(v));
        }
        // Each new vertex sits on an edge of the level below: the average
        // of its ends, which are its only neighbours from there.
        for v in n..fine.positions.len() {
            let ends: Vec<u32> = fine
                .neighbours
                .of(v as u32)
                .iter()
                .copied()
                .filter(|&u| (u as usize) < n)
                .collect();
            assert_eq!(ends.len(), 2);
            let mean =
                (layers.mask.get(ends[0] as usize) + layers.mask.get(ends[1] as usize)) / 2.0;
            assert_eq!(up.mask.get(v), mean);
        }
        assert_eq!(up.carried(fine.positions.len(), &coarse), layers);
    }

    #[test]
    fn layers_round_trip_and_bad_values_are_refused() {
        let n = 5;
        let mut layers = Layers::default();
        layers.chalk.set(2, 0.25, n);
        let mut w = Writer::new();
        layers.write(&mut w, n);
        let bytes = w.finish();
        let read = Layers::read(&mut Reader::new(&bytes), n).unwrap();
        assert_eq!(read, layers);
        assert!(read.mask.is_empty());
        assert!(Layers::read(&mut Reader::new(&bytes[..bytes.len() - 1]), n).is_err());
        let mut w = Writer::new();
        w.u8(1);
        for x in [0.0, 1.0, f32::NAN, 0.0, 0.0] {
            w.f32(x);
        }
        assert!(Layers::read(&mut Reader::new(&w.finish()), n).is_err());
    }
}
