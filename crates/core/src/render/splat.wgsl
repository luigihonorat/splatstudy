// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
// Gaussian splats. Each splat is a 3D Gaussian; seen through the camera it
// becomes, to first order, a 2D Gaussian on the screen (EWA splatting):
// Σ' = J W Σ Wᵀ Jᵀ, with W the turn into view space and J the Jacobian of
// the perspective divide at the splat's centre. The vertex shader draws a
// quad over ±3σ along the 2D Gaussian's axes; the fragment shader fades
// it out with exp(-r²/2). Splats come back to front (sorted on the CPU),
// premultiplied, over whatever the pass already holds; depth is tested
// against the clay but not written.

struct Uniforms {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // File coordinates to the clay's space.
    model: mat4x4<f32>,
    // Target width and height in pixels, then the focal lengths in pixels.
    viewport: vec4<f32>,
    // The crop box in file coordinates; crop_min.w > 0.5 turns it on.
    crop_min: vec4<f32>,
    crop_max: vec4<f32>,
    // Silhouette colour, display values.
    flat_colour: vec4<f32>,
    // x: opacity, y: flat.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// Three floats per splat.
@group(0) @binding(1) var<storage, read> positions: array<f32>;
// Six floats per splat: xx, xy, xz, yy, yz, zz, in file units.
@group(0) @binding(2) var<storage, read> covariances: array<f32>;
// RGBA8, opacity in alpha.
@group(0) @binding(3) var<storage, read> colours: array<u32>;
// Splat indices, back to front.
@group(0) @binding(4) var<storage, read> order: array<u32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // Position in the 2D Gaussian's frame, in standard deviations.
    @location(0) local: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

/// Splats are never larger than this many pixels across their main axis.
const MAX_RADIUS: f32 = 2048.0;

@vertex
fn vs(@builtin(vertex_index) corner_index: u32, @builtin(instance_index) n: u32) -> VsOut {
    var out: VsOut;
    // Outside the depth range: dropped by clipping.
    out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    out.local = vec2<f32>(0.0);
    out.colour = vec4<f32>(0.0);

    let i = order[n];
    let p = vec3<f32>(positions[3u * i], positions[3u * i + 1u], positions[3u * i + 2u]);
    if u.crop_min.w > 0.5 && (any(p < u.crop_min.xyz) || any(p > u.crop_max.xyz)) {
        return out;
    }
    let view = u.view * u.model * vec4<f32>(p, 1.0);
    let depth = -view.z;
    if depth <= 1e-4 {
        return out;
    }
    let clip = u.proj * view;
    let ndc = clip.xyz / clip.w;
    if abs(ndc.x) > 1.3 || abs(ndc.y) > 1.3 || ndc.z < 0.0 || ndc.z > 1.0 {
        return out;
    }

    let c = 6u * i;
    let sigma = mat3x3<f32>(
        covariances[c], covariances[c + 1u], covariances[c + 2u],
        covariances[c + 1u], covariances[c + 3u], covariances[c + 4u],
        covariances[c + 2u], covariances[c + 4u], covariances[c + 5u],
    );
    let mv = u.view * u.model;
    let w = mat3x3<f32>(mv[0].xyz, mv[1].xyz, mv[2].xyz);
    let cov_view = w * sigma * transpose(w);

    // Screen x = fx · x / depth (and y likewise, up), in pixels. Off-screen
    // splats are taken as if a little inside the frustum, so the Jacobian
    // doesn't stretch them without bound.
    let size = u.viewport.xy;
    let focal = u.viewport.zw;
    let limit = 1.3 * 0.5 * size / focal;
    let xy = clamp(view.xy / depth, -limit, limit) * depth;
    let j = mat3x3<f32>(
        vec3<f32>(focal.x / depth, 0.0, 0.0),
        vec3<f32>(0.0, focal.y / depth, 0.0),
        vec3<f32>(focal.x * xy.x / (depth * depth), focal.y * xy.y / (depth * depth), 0.0),
    );
    let cov = j * cov_view * transpose(j);
    // At least about a pixel wide, so tiny splats still cover a sample
    // instead of flickering.
    let a = cov[0][0] + 0.3;
    let b = cov[0][1];
    let d = cov[1][1] + 0.3;

    let mid = 0.5 * (a + d);
    let r = sqrt(max(0.25 * (a - d) * (a - d) + b * b, 0.0));
    let l1 = mid + r;
    let l2 = max(mid - r, 0.01);
    var axis = vec2<f32>(1.0, 0.0);
    if abs(b) > 1e-12 {
        axis = normalize(vec2<f32>(b, l1 - a));
    } else if d > a {
        axis = vec2<f32>(0.0, 1.0);
    }
    let minor = vec2<f32>(-axis.y, axis.x);
    let r1 = min(3.0 * sqrt(l1), MAX_RADIUS);
    let r2 = min(3.0 * sqrt(l2), MAX_RADIUS);

    let corner = vec2<f32>(f32(corner_index & 1u), f32((corner_index >> 1u) & 1u)) * 2.0 - 1.0;
    let offset = corner.x * r1 * axis + corner.y * r2 * minor;
    out.clip = vec4<f32>(ndc.xy + offset * 2.0 / size, ndc.z, 1.0);
    out.local = corner * vec2<f32>(r1 / sqrt(l1), r2 / sqrt(l2));
    out.colour = unpack4x8unorm(colours[i]);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let r2 = dot(in.local, in.local);
    if r2 > 9.0 {
        discard;
    }
    let alpha = min(in.colour.a * exp(-0.5 * r2) * u.params.x, 1.0);
    if alpha < 1.0 / 255.0 {
        discard;
    }
    let rgb = select(in.colour.rgb, u.flat_colour.rgb, u.params.y > 0.5);
    return vec4<f32>(rgb * alpha, alpha);
}
