// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The surface as a graph of samples, and coarser copies of it.
//!
//! The fields are smoothed on the coarsest copy first, where a few passes
//! reach across the whole shape, then handed down level by level: each
//! finer level only has to settle the details.

use glam::Vec3;

use crate::sculpt::Mesh;

/// Samples of the surface joined by edges: a position, a normal and the
/// area each sample stands for.
pub struct Graph {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub areas: Vec<f32>,
    offsets: Vec<u32>,
    links: Vec<u32>,
}

impl Graph {
    /// The vertices of `mesh` joined along its edges.
    pub fn of_mesh(mesh: &Mesh) -> Graph {
        let n = mesh.positions.len();
        let mut areas = vec![0.0; n];
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            let third = (b - a).cross(c - a).length() / 6.0;
            for &i in t {
                areas[i as usize] += third;
            }
        }
        let mut offsets = Vec::with_capacity(n + 1);
        let mut links = Vec::with_capacity(mesh.neighbours.len());
        offsets.push(0);
        for i in 0..n as u32 {
            links.extend_from_slice(mesh.neighbours.of(i));
            offsets.push(links.len() as u32);
        }
        Graph {
            positions: mesh.positions.clone(),
            normals: mesh.normals.clone(),
            areas,
            offsets,
            links,
        }
    }

    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// The samples joined to `i`.
    pub fn of(&self, i: usize) -> &[u32] {
        &self.links[self.links_of(i)]
    }

    /// Where the samples joined to `i` are among all the links.
    pub fn links_of(&self, i: usize) -> std::ops::Range<usize> {
        self.offsets[i] as usize..self.offsets[i + 1] as usize
    }

    /// All the links, `i`'s at [`Graph::links_of`].
    pub fn links(&self) -> &[u32] {
        &self.links
    }

    /// A graph of about half as many samples, each made of two joined
    /// ones (or one left alone), and which coarse sample each of these
    /// went into. `None` when it would hardly shrink.
    fn coarsen(&self) -> Option<(Graph, Vec<u32>)> {
        let n = self.len();
        // Join first the pairs facing the same way and covering little
        // area, so the coarse samples stay flat and of similar size.
        let mut pairs: Vec<(f32, u32, u32)> = Vec::with_capacity(self.links.len() / 2);
        for i in 0..n {
            for &j in self.of(i) {
                if (i as u32) < j {
                    let j = j as usize;
                    let facing = self.normals[i].dot(self.normals[j]);
                    let area = (self.areas[i] + self.areas[j]).max(f32::MIN_POSITIVE);
                    pairs.push((facing / area, i as u32, j as u32));
                }
            }
        }
        pairs.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then((a.1, a.2).cmp(&(b.1, b.2))));
        const NONE: u32 = u32::MAX;
        let mut parent = vec![NONE; n];
        let mut count = 0u32;
        for (_, i, j) in pairs {
            let (i, j) = (i as usize, j as usize);
            if parent[i] == NONE && parent[j] == NONE {
                parent[i] = count;
                parent[j] = count;
                count += 1;
            }
        }
        for p in parent.iter_mut().filter(|p| **p == NONE) {
            *p = count;
            count += 1;
        }
        let m = count as usize;
        if m as f32 > n as f32 * 0.9 {
            return None;
        }
        let mut positions = vec![Vec3::ZERO; m];
        let mut normals = vec![Vec3::ZERO; m];
        let mut areas = vec![0.0f32; m];
        let mut weights = vec![0.0f32; m];
        for i in 0..n {
            let c = parent[i] as usize;
            // Weighted by area, but never by nothing, so a sample of
            // zero area still has a place.
            let w = self.areas[i] + 1e-12;
            positions[c] += self.positions[i] * w;
            normals[c] += self.normals[i] * w;
            areas[c] += self.areas[i];
            weights[c] += w;
        }
        for c in 0..m {
            positions[c] /= weights[c];
            normals[c] = normals[c].normalize_or(Vec3::Y);
        }
        let mut edges: Vec<(u32, u32)> = Vec::with_capacity(self.links.len() / 2);
        for i in 0..n {
            for &j in self.of(i) {
                let (a, b) = (parent[i], parent[j as usize]);
                if a != b {
                    edges.push((a, b));
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();
        let mut offsets = vec![0u32; m + 1];
        for &(a, _) in &edges {
            offsets[a as usize + 1] += 1;
        }
        for c in 0..m {
            offsets[c + 1] += offsets[c];
        }
        let links = edges.into_iter().map(|(_, b)| b).collect();
        let coarse = Graph {
            positions,
            normals,
            areas,
            offsets,
            links,
        };
        Some((coarse, parent))
    }
}

/// The graph and its coarser copies, finest first.
pub struct Hierarchy {
    pub levels: Vec<Graph>,
    /// For each level but the last, the sample of the next level that
    /// each of its samples went into.
    pub parents: Vec<Vec<u32>>,
}

/// Stop coarsening below this many samples.
const COARSEST: usize = 64;
/// And after this many levels in any case.
const MAX_LEVELS: usize = 24;

impl Hierarchy {
    pub fn build(graph: Graph) -> Hierarchy {
        let mut levels = vec![graph];
        let mut parents = Vec::new();
        while levels.len() < MAX_LEVELS {
            let Some(last) = levels.last() else { break };
            if last.len() <= COARSEST {
                break;
            }
            let Some((coarse, parent)) = last.coarsen() else {
                break;
            };
            levels.push(coarse);
            parents.push(parent);
        }
        Hierarchy { levels, parents }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_shrink_and_keep_the_area_and_the_links_both_ways() {
        let mesh = Mesh::icosphere(4);
        let hierarchy = Hierarchy::build(Graph::of_mesh(&mesh));
        let area: f32 = hierarchy.levels[0].areas.iter().sum();
        assert!(hierarchy.levels.len() > 4);
        for (k, level) in hierarchy.levels.iter().enumerate() {
            let a: f32 = level.areas.iter().sum();
            assert!((a - area).abs() < area * 1e-4, "level {k}");
            for i in 0..level.len() {
                for &j in level.of(i) {
                    assert!(level.of(j as usize).contains(&(i as u32)));
                }
            }
        }
        for (k, parent) in hierarchy.parents.iter().enumerate() {
            assert_eq!(parent.len(), hierarchy.levels[k].len());
            let next = hierarchy.levels[k + 1].len() as u32;
            assert!(parent.iter().all(|&p| p < next));
        }
    }
}
