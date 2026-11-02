// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
// Clay shading: a soft key light that wraps around the form, a sky fill from
// above and a faint rim, so planes and edges read the way they do on a
// modelling stand under a studio window. Lighting is in view space so the
// light stays put while the sculpt turns.

struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // Clay colour, linear RGB.
    base: vec4<f32>,
    // Direction towards the key light, view space.
    light: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) position: vec3<f32>,
};

@vertex
fn vs(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(v.position, 1.0);
    out.normal = (u.view * vec4<f32>(v.normal, 0.0)).xyz;
    out.position = (u.view * vec4<f32>(v.position, 1.0)).xyz;
    return out;
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }
    let v = normalize(-in.position);
    let l = normalize(u.light.xyz);
    let n_dot_l = dot(n, l);
    let wrap = max((n_dot_l + 0.35) / 1.35, 0.0);
    let sky = 0.5 + 0.5 * n.y;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0);
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 32.0) * 0.05 * step(0.0, n_dot_l);
    let light = 0.78 * wrap + 0.30 * sky + 0.10 * rim;
    let colour = u.base.rgb * light + vec3<f32>(spec);
    return vec4<f32>(to_srgb(clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}
