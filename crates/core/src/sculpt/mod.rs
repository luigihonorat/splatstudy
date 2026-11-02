// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the clay mesh and, next, the brushes that shape it.

pub mod mesh;

pub use mesh::{Mesh, Vertex};

/// Subdivision level of the default sphere: 163,842 vertices, 327,680
/// triangles; enough for a head study, light enough for the browser.
pub const DEFAULT_LEVEL: u32 = 7;
