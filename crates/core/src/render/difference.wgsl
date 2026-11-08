// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
// The silhouette difference: from the clay and the reference rendered flat,
// pixel for pixel, a tone where the clay alone is inked (too big) and
// another where the reference alone is (too small). Written premultiplied,
// as egui shows textures.

struct Tones {
    // Premultiplied and gamma-encoded, as egui's colours are.
    over: vec4<f32>,
    under: vec4<f32>,
    // The flat renders' ink, in the red channel; their background is black.
    ink: f32,
}

@group(0) @binding(0) var<uniform> tones: Tones;
@group(0) @binding(1) var clay_mask: texture_2d<f32>;
@group(0) @binding(2) var reference_mask: texture_2d<f32>;

// One triangle that covers the target.
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// How much of a pixel is inked: the renders are multisampled, so an edge
// pixel holds part of the ink. Half of it is the edge, as for the score;
// the ramp around it keeps the edge smooth and about a pixel wide.
fn inked(value: f32) -> f32 {
    return smoothstep(0.1, 0.9, value / tones.ink);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(position.xy);
    let clay = inked(textureLoad(clay_mask, at, 0).r);
    let reference = inked(textureLoad(reference_mask, at, 0).r);
    return tones.over * (clay * (1.0 - reference)) + tones.under * (reference * (1.0 - clay));
}
