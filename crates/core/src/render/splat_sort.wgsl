// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
// Sorting splats back to front on the GPU: a one-pass counting sort on a
// 16-bit depth key, in three dispatches.
//
// 1. `depth_keys`: each splat's view depth, bucketed into 65 536 steps
//    between `lo` (the farthest, key 0) and the nearest, and counted.
// 2. `prefix`: one workgroup turns the counts into each bucket's first
//    place in the order.
// 3. `place`: each splat takes the next place in its bucket.
//
// Splats in one bucket come out in no particular order. They lie within
// 1/65 536 of the depth range of each other, too close for the order
// between them to show.

struct Params {
    // View depth is dot(row.xyz, p) for a splat centre p in file units.
    row: vec4<f32>,
    // The depth of key 0, and keys per unit of depth.
    lo: f32,
    scale: f32,
    count: u32,
    // Invocations per row of a two-dimensional dispatch.
    stride: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> positions: array<f32>;
@group(0) @binding(2) var<storage, read_write> keys: array<u32>;
@group(0) @binding(3) var<storage, read_write> counts: array<atomic<u32>, 65536>;
@group(0) @binding(4) var<storage, read_write> order: array<u32>;

const BUCKETS: u32 = 65536u;
const GROUP: u32 = 256u;

@compute @workgroup_size(256)
fn depth_keys(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x + id.y * params.stride;
    if i >= params.count {
        return;
    }
    let p = vec3<f32>(positions[3u * i], positions[3u * i + 1u], positions[3u * i + 2u]);
    let z = dot(params.row.xyz, p);
    // Beyond the range (floaters) clamps to its ends, still on the right
    // side of everything inside it.
    let key = min(u32(clamp((z - params.lo) * params.scale, 0.0, 65535.0)), BUCKETS - 1u);
    keys[i] = key;
    atomicAdd(&counts[key], 1u);
}

var<workgroup> sums: array<u32, 256>;

@compute @workgroup_size(256)
fn prefix(@builtin(local_invocation_index) t: u32) {
    // Each invocation adds up its own run of 256 buckets...
    let per = BUCKETS / GROUP;
    let base = t * per;
    var total = 0u;
    for (var j = 0u; j < per; j++) {
        total += atomicLoad(&counts[base + j]);
    }
    sums[t] = total;
    workgroupBarrier();
    // ...the runs' totals are summed across the workgroup...
    for (var offset = 1u; offset < GROUP; offset <<= 1u) {
        var before = 0u;
        if t >= offset {
            before = sums[t - offset];
        }
        workgroupBarrier();
        sums[t] += before;
        workgroupBarrier();
    }
    // ...and each bucket gets the place where its splats start.
    var start = sums[t] - total;
    for (var j = 0u; j < per; j++) {
        let n = atomicLoad(&counts[base + j]);
        atomicStore(&counts[base + j], start);
        start += n;
    }
}

@compute @workgroup_size(256)
fn place(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x + id.y * params.stride;
    if i >= params.count {
        return;
    }
    order[atomicAdd(&counts[keys[i]], 1u)] = i;
}
