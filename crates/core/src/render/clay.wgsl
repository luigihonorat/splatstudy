// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
// Clay shading: a key light that wraps around the form, a sky fill from
// above and a faint rim, so planes and edges read the way they do on a
// modelling stand. The lighting (studio window, raking or skylight) sets the
// key light's direction and wrap and how strong each part is. Lighting is in
// view space so the light stays put while the sculpt turns.

struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // Clay colour, linear RGB.
    base: vec4<f32>,
    // xyz: direction towards the key light, view space; w: its wrap.
    light: vec4<f32>,
    // x: flat (silhouette), y: opacity, z: use vertex colours.
    params: vec4<f32>,
    // Strengths: x key, y sky, z rim, w highlight.
    rig: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // sRGB.
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) position: vec3<f32>,
    // Linear.
    @location(2) color: vec3<f32>,
};

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@vertex
fn vs(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(v.position, 1.0);
    out.normal = (u.view * vec4<f32>(v.normal, 0.0)).xyz;
    out.position = (u.view * vec4<f32>(v.position, 1.0)).xyz;
    out.color = select(vec3<f32>(1.0), to_linear(v.color.rgb), u.params.z > 0.5);
    return out;
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let base = u.base.rgb * in.color;
    let opacity = u.params.y;
    if u.params.x > 0.5 {
        return vec4<f32>(to_srgb(base) * opacity, opacity);
    }
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }
    let v = normalize(-in.position);
    let l = normalize(u.light.xyz);
    let n_dot_l = dot(n, l);
    let wrap = max((n_dot_l + u.light.w) / (1.0 + u.light.w), 0.0);
    let sky = 0.5 + 0.5 * n.y;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0);
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 32.0) * u.rig.w * step(0.0, n_dot_l);
    let light = u.rig.x * wrap + u.rig.y * sky + u.rig.z * rim;
    let colour = base * light + vec3<f32>(spec);
    // Premultiplied, for the blend state.
    let srgb = to_srgb(clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0)));
    return vec4<f32>(srgb * opacity, opacity);
}

// The ghost's first step: where it covers, the depth goes back to the far
// plane, so what was drawn before it cannot hide it. Its pipeline writes no
// colour.
struct Far {
    @location(0) colour: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_far() -> Far {
    var out: Far;
    out.colour = vec4<f32>(0.0);
    out.depth = 1.0;
    return out;
}

// Depth only (its pipeline writes no colour): keeps the ghost's nearest
// surface, so the colour step blends that one alone.
@fragment
fn fs_blank() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
