// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
// Clay shading: a key light that wraps around the form, a sky fill from
// above and a faint rim, so planes and edges read the way they do on a
// modelling stand. The lighting (studio window, raking or skylight) sets the
// key light's direction and wrap and how strong each part is. Lighting is in
// view space so the light stays put while the sculpt turns. The clay's chalk
// tints it blue and its mask darkens it; a silhouette shows neither.
// Faceted, each triangle is lit by its own plane's normal, found from how
// the position changes across the screen, rather than by the normals
// smoothed over its corners.
// The matcap is a second way to light the clay (see `matcap` below): the
// soft light of a studio clay ball, with hollows and ridges picked out
// from how fast the normal turns across the screen.

struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // Clay colour, linear RGB.
    base: vec4<f32>,
    // xyz: direction towards the key light, view space; w: its wrap.
    light: vec4<f32>,
    // x: flat (silhouette), y: opacity, z: use vertex colours,
    // w: orthographic (the eye looks along -z from everywhere).
    params: vec4<f32>,
    // Strengths: x key, y sky, z rim, w highlight.
    rig: vec4<f32>,
    // x: added to the wireframe's clip depth, which moves it towards the
    // camera by a share of its distance (see `PULL` in clay.rs).
    pull: vec4<f32>,
    // The wireframe's colour, linear RGB, and its opacity.
    wire: vec4<f32>,
    // The chalk's colour, linear RGB; w: how much of its light fully
    // masked clay keeps.
    layers: vec4<f32>,
    // x: faceted (flat shading), each triangle lit as one plane; y: the
    // matcap; z: the length its curvature is scaled by (a share of the
    // camera's distance), view space; w: the mesh's typical edge length,
    // 0 if unknown.
    shading: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // sRGB.
    @location(2) color: vec4<f32>,
    // x: mask, y: chalk, 0..1.
    @location(3) layers: vec4<f32>,
};

struct VsOut {
    // Invariant: the wireframe computes the same positions and must get
    // the same bits, to lie exactly on the clay.
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) position: vec3<f32>,
    // Linear.
    @location(2) color: vec3<f32>,
    // x: mask, y: chalk.
    @location(3) layers: vec2<f32>,
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
    out.layers = v.layers.xy;
    return out;
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

// How much the surface bends, view space, per unit of length: the normal's
// turn along each screen direction over the distance moved on the
// surface, the two added. Positive where it bulges (a ball of radius r
// gives 2 / r), negative in a hollow, whichever way the screen runs.
fn curvature(dnx: vec3<f32>, dny: vec3<f32>, dx: vec3<f32>, dy: vec3<f32>) -> f32 {
    let across = dot(dnx, dx) / max(dot(dx, dx), 1e-20);
    let down = dot(dny, dy) / max(dot(dy, dy), 1e-20);
    return across + down;
}

// Softly brings light above `knee` down towards 1, so the highlight on
// pale clay keeps its shape rather than clipping flat.
fn shoulder(c: vec3<f32>) -> vec3<f32> {
    let knee = 0.75;
    let over = max(c - vec3<f32>(knee), vec3<f32>(0.0));
    return min(c, vec3<f32>(knee)) + over / (vec3<f32>(1.0) + over / (1.0 - knee));
}

// The matcap: the light of a clay ball in a studio, read from the normal
// `n` as the eye `v` sees it, the key light coming from the lighting's
// direction. A broad, soft key wrapped well into the shade; a sky from
// above, darker from below; what turns away from the eye falls off, as
// on a ball; a soft sheen and a faint rim. `bend` is the curvature times
// a length: hollows lose light as if half shut in (an occlusion), and
// ridges catch a little more, so small forms read at a glance.
fn matcap(n: vec3<f32>, v: vec3<f32>, base: vec3<f32>, bend: f32) -> vec3<f32> {
    let l = normalize(u.light.xyz);
    let n_dot_l = dot(n, l);
    let n_dot_v = clamp(dot(n, v), 0.0, 1.0);
    // -1..1, easing into its ends.
    let c = bend / (1.0 + abs(bend));
    let hollow = max(-c, 0.0);
    let ridge = max(c, 0.0);
    let wrap = u.light.w + 0.15;
    let key = pow(max((n_dot_l + wrap) / (1.0 + wrap), 0.0), 1.3);
    let sky = mix(0.15, 1.0, 0.5 + 0.5 * n.y);
    let facing = mix(0.4, 1.0, n_dot_v);
    let occlusion = 1.0 - 0.65 * hollow;
    // The key a touch warm, like the plaster casts' light.
    let warm = vec3<f32>(1.0, 0.975, 0.94);
    let diffuse = (1.1 * u.rig.x * key * warm + vec3<f32>(u.rig.y * sky)) * facing * occlusion;
    let h = normalize(l + v);
    let n_dot_h = max(dot(n, h), 0.0);
    let sheen = (pow(n_dot_h, 28.0) * 2.4 + pow(n_dot_h, 6.0) * 0.8) * u.rig.w;
    let lit_side = smoothstep(-0.1, 0.3, n_dot_l);
    let rim = pow(1.0 - n_dot_v, 4.0) * u.rig.z * 2.0 * (0.6 + 0.4 * n.y);
    let colour = base * (diffuse + vec3<f32>(0.3 * ridge)) + vec3<f32>(sheen * lit_side * occlusion + rim);
    return shoulder(colour);
}

@fragment
fn fs(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // The triangle's own plane, view space, facing the eye whichever side
    // of it shows: across the screen x goes right and y goes down, so
    // dpdy × dpdx points out of the screen. Taken before any branch, as
    // derivatives must be, and so is the curvature, from the smoothed
    // normal.
    let dx = dpdx(in.position);
    let dy = dpdy(in.position);
    let plane = cross(dy, dx);
    let rounded = normalize(in.normal);
    var bend = curvature(dpdx(rounded), dpdy(rounded), dx, dy);
    // The curvature is one value per triangle: where triangles are many
    // pixels wide it would show as bands, so it fades out.
    let edge_pixels = u.shading.w * inverseSqrt(max(max(dot(dx, dx), dot(dy, dy)), 1e-20));
    bend *= 1.0 - smoothstep(5.0, 14.0, edge_pixels);
    var base = u.base.rgb * in.color;
    let opacity = u.params.y;
    if u.params.x > 0.5 {
        return vec4<f32>(to_srgb(base) * opacity, opacity);
    }
    base = mix(base, u.layers.rgb, in.layers.y);
    base *= mix(1.0, u.layers.w, in.layers.x);
    var n = rounded;
    if !front {
        n = -n;
    }
    // A plane seen edge on has no width on screen: keep the smooth normal.
    if u.shading.x > 0.5 && dot(plane, plane) > 0.0 {
        n = normalize(plane);
    }
    let v = select(normalize(-in.position), vec3<f32>(0.0, 0.0, 1.0), u.params.w > 0.5);
    if u.shading.y > 0.5 {
        // Seen from inside, a bulge is a hollow.
        let side = select(-1.0, 1.0, front);
        let colour = matcap(n, v, base, side * bend * u.shading.z);
        let srgb = to_srgb(clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0)));
        return vec4<f32>(srgb * opacity, opacity);
    }
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

// The wireframe: the clay's triangles drawn again over it, each corner
// read from the mesh's buffers by its place in the index buffer, so it
// knows which of its triangle's three corners it is.

// The vertices as words: position (3), normal (3), colour (1), layers (1).
@group(1) @binding(0) var<storage, read> vertices: array<f32>;
@group(1) @binding(1) var<storage, read> indices: array<u32>;
// Two bits per triangle, sixteen to a word: the corner opposite the edge
// its quad hides (the diagonal), or 3 when every edge shows.
@group(1) @binding(2) var<storage, read> diagonals: array<u32>;

struct WireOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) bary: vec3<f32>,
    @location(1) @interpolate(flat) hidden: u32,
};

@vertex
fn vs_wire(@builtin(vertex_index) i: u32) -> WireOut {
    let v = indices[i] * 8u;
    let p = vec3<f32>(vertices[v], vertices[v + 1u], vertices[v + 2u]);
    let corner = i % 3u;
    let triangle = i / 3u;
    var out: WireOut;
    // Exactly where the clay is, then a little nearer: with a
    // perspective, adding to z alone draws the same points as the
    // triangle scaled towards the camera.
    out.clip = u.view_proj * vec4<f32>(p, 1.0);
    out.clip.z += u.pull.x;
    out.bary = vec3<f32>(f32(corner == 0u), f32(corner == 1u), f32(corner == 2u));
    out.hidden = (diagonals[triangle / 16u] >> (triangle % 16u * 2u)) & 3u;
    return out;
}

@fragment
fn fs_wire(in: WireOut) -> @location(0) vec4<f32> {
    // How far each edge is, in pixels: the coordinate that is 0 along it,
    // over its change from one pixel to the next.
    let step = max(fwidth(in.bary), vec3<f32>(1e-6));
    let shown = vec3<bool>(in.hidden != 0u, in.hidden != 1u, in.hidden != 2u);
    let d = select(vec3<f32>(1e6), in.bary / step, shown);
    let nearest = min(d.x, min(d.y, d.z));
    // Each triangle draws its half of the line along an edge: about a
    // pixel wide in all, with soft sides.
    let line = 1.0 - smoothstep(0.0, 1.0, nearest);
    // Triangles a few pixels across would turn the clay solid: there the
    // lines fade to an even tint, which says the mesh is dense.
    let size = max(step.x, max(step.y, step.z));
    let dense = smoothstep(0.15, 0.5, size);
    let alpha = u.wire.w * line * mix(1.0, 0.35, dense);
    return vec4<f32>(to_srgb(u.wire.rgb) * alpha, alpha);
}
