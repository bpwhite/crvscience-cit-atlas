// Copyright (C) 2026 CRV Science Network (https://crvscience.com)
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://gnu.org>.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::{egui, egui_wgpu, wgpu};

use crate::graph_viewer::{
    compute_spotlight_mask, node_radius_for_encoding, node_rgb_for_encoding, GraphViewerState,
    NodeColorEncoding,
};
use crate::models::{BioEntityKind, CorpusAnalytics};

pub const GRID_DIM: u32 = 128;
pub const GRID_CELLS: usize = (GRID_DIM * GRID_DIM) as usize; // 16,384 fine spatial-hash cells
pub const SUPER_DIM: u32 = 16;
pub const SUPER_CELLS: usize = (SUPER_DIM * SUPER_DIM) as usize; // 256 multipole supercells
pub const WORLD_EXTENT: f32 = 1.25;

// Node bitflags packed into `GpuNodeRecord.flags`
pub const NODE_FLAG_PINNED: u32 = 1 << 0;
pub const NODE_FLAG_RETRACTED: u32 = 1 << 1;
pub const NODE_FLAG_SLEEPING_BEAUTY: u32 = 1 << 2;
pub const NODE_FLAG_DIAMOND_ARTIFACT: u32 = 1 << 3;
pub const NODE_FLAG_FOCAL: u32 = 1 << 4;
pub const NODE_FLAG_IN_SPOTLIGHT: u32 = 1 << 5;
pub const NODE_FLAG_HEXAGON_UNIPROT: u32 = 1 << 7;
pub const NODE_FLAG_OCTAGON_PDB: u32 = 1 << 8;
pub const NODE_FLAG_TRIANGLE_EPMC: u32 = 1 << 9;

/// 48-byte (3 x 16B) GPU node state aligned for both WGSL `storage, read_write` and `storage, read`.
#[derive(Debug, Clone, Copy)]
pub struct GpuNodeRecord {
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    pub anchor: [f32; 2],
    pub radius: f32,
    pub rgba_packed: u32,
    pub flags: u32,
    pub degree: f32,
    pub pad0: f32,
    pub pad1: f32,
}

/// 16-byte GPU edge instance referencing `nodes[src_idx]` and `nodes[dst_idx]` directly in VRAM.
#[derive(Debug, Clone, Copy)]
pub struct GpuEdgeRecord {
    pub src_idx: u32,
    pub dst_idx: u32,
    pub rgba_packed: u32,
    pub eb_encoded: f32, // >= 10.0 geodesic highlight, >= 5.0 zombie cite, else [0..1] edge betweenness
}

/// 64-byte (4 x 16B) WGSL `SimUniforms` payload.
#[derive(Debug, Clone, Copy)]
pub struct SimUniformsPayload {
    pub node_count: u32,
    pub grid_dim: u32,
    pub super_dim: u32,
    pub run_step: u32,
    pub world_extent: f32,
    pub repulsion_k: f32,
    pub spring_k: f32,
    pub gravity_k: f32,
    pub cohesion_k: f32,
    pub damping: f32,
    pub ideal_len: f32,
    pub max_step: f32,
    pub dragged_idx: i32,
    pub drag_x: f32,
    pub drag_y: f32,
    pub pad0: f32,
}

/// 48-byte (3 x 16B) WGSL `CameraUniforms` payload.
#[derive(Debug, Clone, Copy)]
pub struct CameraUniformsPayload {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub aspect: f32,
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub min_edge_eb: f32,
    pub edge_alpha_scale: f32,
    pub node_scale: f32,
    pub show_arrows: u32,
    pub curved_edges: u32,
    pub hovered_idx: i32,
}

pub fn pack_rgba_u32(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

pub fn encode_gpu_nodes_bytes(nodes: &[GpuNodeRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(nodes.len() * 48);
    for n in nodes {
        bytes.extend_from_slice(&n.pos[0].to_le_bytes());
        bytes.extend_from_slice(&n.pos[1].to_le_bytes());
        bytes.extend_from_slice(&n.vel[0].to_le_bytes());
        bytes.extend_from_slice(&n.vel[1].to_le_bytes());
        bytes.extend_from_slice(&n.anchor[0].to_le_bytes());
        bytes.extend_from_slice(&n.anchor[1].to_le_bytes());
        bytes.extend_from_slice(&n.radius.to_le_bytes());
        bytes.extend_from_slice(&n.rgba_packed.to_le_bytes());
        bytes.extend_from_slice(&n.flags.to_le_bytes());
        bytes.extend_from_slice(&n.degree.to_le_bytes());
        bytes.extend_from_slice(&n.pad0.to_le_bytes());
        bytes.extend_from_slice(&n.pad1.to_le_bytes());
    }
    bytes
}

pub fn encode_gpu_edges_bytes(edges: &[GpuEdgeRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(edges.len() * 16);
    for e in edges {
        bytes.extend_from_slice(&e.src_idx.to_le_bytes());
        bytes.extend_from_slice(&e.dst_idx.to_le_bytes());
        bytes.extend_from_slice(&e.rgba_packed.to_le_bytes());
        bytes.extend_from_slice(&e.eb_encoded.to_le_bytes());
    }
    bytes
}

pub fn encode_sim_uniforms_bytes(u: &SimUniformsPayload) -> [u8; 64] {
    let mut out = [0u8; 64];
    let words: [[u8; 4]; 16] = [
        u.node_count.to_le_bytes(),
        u.grid_dim.to_le_bytes(),
        u.super_dim.to_le_bytes(),
        u.run_step.to_le_bytes(),
        u.world_extent.to_le_bytes(),
        u.repulsion_k.to_le_bytes(),
        u.spring_k.to_le_bytes(),
        u.gravity_k.to_le_bytes(),
        u.cohesion_k.to_le_bytes(),
        u.damping.to_le_bytes(),
        u.ideal_len.to_le_bytes(),
        u.max_step.to_le_bytes(),
        u.dragged_idx.to_le_bytes(),
        u.drag_x.to_le_bytes(),
        u.drag_y.to_le_bytes(),
        u.pad0.to_le_bytes(),
    ];
    for (i, w) in words.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(w);
    }
    out
}

pub fn encode_camera_uniforms_bytes(c: &CameraUniformsPayload) -> [u8; 48] {
    let mut out = [0u8; 48];
    let words: [[u8; 4]; 12] = [
        c.pan_x.to_le_bytes(),
        c.pan_y.to_le_bytes(),
        c.zoom.to_le_bytes(),
        c.aspect.to_le_bytes(),
        c.viewport_w.to_le_bytes(),
        c.viewport_h.to_le_bytes(),
        c.min_edge_eb.to_le_bytes(),
        c.edge_alpha_scale.to_le_bytes(),
        c.node_scale.to_le_bytes(),
        c.show_arrows.to_le_bytes(),
        c.curved_edges.to_le_bytes(),
        c.hovered_idx.to_le_bytes(),
    ];
    for (i, w) in words.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(w);
    }
    out
}

pub fn encode_u32_slice_bytes(slice: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(slice.len() * 4);
    for &v in slice {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Builds Compressed Sparse Row (CSR) symmetrized adjacency buffers for the WGSL spring kernel,
/// packing `same_cluster` into bit 31 (`0x8000_0000`) of each neighbor entry.
pub fn build_csr_buffers(
    total_nodes: usize,
    undir_adj: &[Vec<usize>],
    bipartite_links: &[(usize, usize)],
    cluster_ids: &[usize],
) -> (Vec<u32>, Vec<u32>) {
    let mut full_adj: Vec<Vec<(usize, bool)>> = vec![Vec::new(); total_nodes];
    for (u, nbrs) in undir_adj.iter().enumerate() {
        for &v in nbrs {
            if u < total_nodes && v < total_nodes && u != v {
                let same_c = cluster_ids.get(u) == cluster_ids.get(v);
                full_adj[u].push((v, same_c));
            }
        }
    }
    for &(paper_idx, art_node_idx) in bipartite_links {
        if paper_idx < total_nodes && art_node_idx < total_nodes {
            full_adj[paper_idx].push((art_node_idx, true));
            full_adj[art_node_idx].push((paper_idx, true));
        }
    }

    let mut offsets = Vec::with_capacity(total_nodes + 1);
    let mut neighbors = Vec::new();
    offsets.push(0u32);
    for row in &full_adj {
        for &(nbr, same_c) in row {
            let packed = (nbr as u32 & 0x7FFF_FFFF) | if same_c { 0x8000_0000 } else { 0 };
            neighbors.push(packed);
        }
        offsets.push(neighbors.len() as u32);
    }
    if neighbors.is_empty() {
        neighbors.push(0);
    }
    (offsets, neighbors)
}

// ============================================================================
// O(1) SPATIAL PICK GRID FOR 50,000 - 150,000+ NODES
// ============================================================================
pub struct SpatialPickGrid {
    dim: usize,
    extent: f32,
    buckets: Vec<Vec<usize>>,
    coords: Vec<[f32; 2]>,
}

impl Default for SpatialPickGrid {
    fn default() -> Self {
        Self::new(64, WORLD_EXTENT)
    }
}

impl SpatialPickGrid {
    pub fn new(dim: usize, extent: f32) -> Self {
        let d = dim.max(8);
        Self {
            dim: d,
            extent: extent.max(0.5),
            buckets: vec![Vec::new(); d * d],
            coords: Vec::new(),
        }
    }

    pub fn rebuild(&mut self, coords: &[[f32; 2]]) {
        self.coords.clear();
        self.coords.extend_from_slice(coords);
        for b in &mut self.buckets {
            b.clear();
        }
        let d_f = self.dim as f32;
        let inv_span = 1.0 / (2.0 * self.extent);
        for (idx, &[x, y]) in coords.iter().enumerate() {
            let gx = (((x + self.extent) * inv_span) * d_f).floor() as isize;
            let gy = (((y + self.extent) * inv_span) * d_f).floor() as isize;
            let cx = gx.clamp(0, (self.dim - 1) as isize) as usize;
            let cy = gy.clamp(0, (self.dim - 1) as isize) as usize;
            self.buckets[cy * self.dim + cx].push(idx);
        }
    }

    /// Finds the nearest node within `max_world_dist` of `(wx, wy)` in O(1) expected time.
    pub fn pick_nearest(&self, wx: f32, wy: f32, max_world_dist: f32) -> Option<usize> {
        if self.coords.is_empty() {
            return None;
        }
        let d_f = self.dim as f32;
        let cell_w = (2.0 * self.extent) / d_f;
        let search_r = ((max_world_dist / cell_w).ceil() as isize).clamp(1, 4);
        let gx = (((wx + self.extent) / (2.0 * self.extent)) * d_f).floor() as isize;
        let gy = (((wy + self.extent) / (2.0 * self.extent)) * d_f).floor() as isize;

        let mut best_idx = None;
        let mut best_d2 = max_world_dist * max_world_dist;

        for oy in (gy - search_r)..=(gy + search_r) {
            if oy < 0 || oy >= self.dim as isize {
                continue;
            }
            for ox in (gx - search_r)..=(gx + search_r) {
                if ox < 0 || ox >= self.dim as isize {
                    continue;
                }
                for &idx in &self.buckets[(oy as usize) * self.dim + (ox as usize)] {
                    let [nx, ny] = self.coords[idx];
                    let dx = nx - wx;
                    let dy = ny - wy;
                    let d2 = dx * dx + dy * dy;
                    if d2 <= best_d2 {
                        best_d2 = d2;
                        best_idx = Some(idx);
                    }
                }
            }
        }
        best_idx
    }

    pub fn coords(&self) -> &[[f32; 2]] {
        &self.coords
    }
}

// ============================================================================
// EMBEDDED WGSL COMPUTE SHADER (128x128 SPATIAL HASH + 16x16 MULTIPOLE + CSR)
// ============================================================================
pub const WGSL_GRAPH_COMPUTE: &str = r#"
struct SimUniforms {
    node_count: u32,
    grid_dim: u32,
    super_dim: u32,
    run_step: u32,
    world_extent: f32,
    repulsion_k: f32,
    spring_k: f32,
    gravity_k: f32,
    cohesion_k: f32,
    damping: f32,
    ideal_len: f32,
    max_step: f32,
    dragged_idx: i32,
    drag_x: f32,
    drag_y: f32,
    pad0: f32,
};

struct GpuNodeState {
    pos: vec2<f32>,
    vel: vec2<f32>,
    anchor: vec2<f32>,
    radius: f32,
    rgba_packed: u32,
    flags: u32,
    degree: f32,
    pad0: f32,
    pad1: f32,
};

@group(0) @binding(0) var<uniform> sim: SimUniforms;
@group(0) @binding(1) var<storage, read_write> nodes: array<GpuNodeState>;
@group(0) @binding(2) var<storage, read_write> cell_head: array<atomic<i32>>;
@group(0) @binding(3) var<storage, read_write> node_next: array<i32>;
@group(0) @binding(4) var<storage, read_write> super_grid: array<atomic<i32>>;
@group(0) @binding(5) var<storage, read> csr_offsets: array<u32>;
@group(0) @binding(6) var<storage, read> csr_neighbors: array<u32>;

@compute @workgroup_size(256)
fn cs_clear_grids(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total_cells = sim.grid_dim * sim.grid_dim;
    if (idx < total_cells) {
        atomicStore(&cell_head[idx], -1);
    }
    let total_super = sim.super_dim * sim.super_dim * 3u;
    if (idx < total_super) {
        atomicStore(&super_grid[idx], 0);
    }
}

@compute @workgroup_size(256)
fn cs_bin_nodes(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= sim.node_count) {
        return;
    }

    if (sim.dragged_idx >= 0 && i == u32(sim.dragged_idx)) {
        nodes[i].pos = vec2<f32>(sim.drag_x, sim.drag_y);
        nodes[i].vel = vec2<f32>(0.0, 0.0);
    }

    let p = nodes[i].pos;
    let ext = max(sim.world_extent, 0.5);
    let norm = clamp((p + vec2<f32>(ext, ext)) / (2.0 * ext), vec2<f32>(0.0, 0.0), vec2<f32>(0.9999, 0.9999));

    let gdim_f = f32(sim.grid_dim);
    let gx = u32(norm.x * gdim_f);
    let gy = u32(norm.y * gdim_f);
    let cell = gy * sim.grid_dim + gx;

    let prev = atomicExchange(&cell_head[cell], i32(i));
    node_next[i] = prev;

    let sdim_f = f32(sim.super_dim);
    let sx = u32(norm.x * sdim_f);
    let sy = u32(norm.y * sdim_f);
    let sc_base = (sy * sim.super_dim + sx) * 3u;

    let charge = clamp(sqrt(nodes[i].degree + 1.0), 1.0, 24.0);
    let q_fp = i32(charge * 64.0);
    let wx_fp = i32(p.x * charge * 64.0);
    let wy_fp = i32(p.y * charge * 64.0);

    atomicAdd(&super_grid[sc_base + 0u], q_fp);
    atomicAdd(&super_grid[sc_base + 1u], wx_fp);
    atomicAdd(&super_grid[sc_base + 2u], wy_fp);
}

@compute @workgroup_size(256)
fn cs_forces_and_integrate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= sim.node_count || sim.run_step == 0u) {
        return;
    }

    let flags = nodes[i].flags;
    if ((flags & 1u) != 0u || (sim.dragged_idx >= 0 && i == u32(sim.dragged_idx))) {
        nodes[i].vel = vec2<f32>(0.0, 0.0);
        return;
    }

    let pos_i = nodes[i].pos;
    let charge_i = clamp(sqrt(nodes[i].degree + 1.0), 1.0, 24.0);
    let k_sq = sim.ideal_len * sim.ideal_len;

    let ext = max(sim.world_extent, 0.5);
    let norm = clamp((pos_i + vec2<f32>(ext, ext)) / (2.0 * ext), vec2<f32>(0.0, 0.0), vec2<f32>(0.9999, 0.9999));
    let gx_i = i32(u32(norm.x * f32(sim.grid_dim)));
    let gy_i = i32(u32(norm.y * f32(sim.grid_dim)));
    let sx_i = i32(u32(norm.x * f32(sim.super_dim)));
    let sy_i = i32(u32(norm.y * f32(sim.super_dim)));

    var force = vec2<f32>(0.0, 0.0);

    // 1. Exact short-range ForceAtlas2 repulsion across 3x3 fine spatial-hash neighborhood
    let gdim_i = i32(sim.grid_dim);
    for (var oy: i32 = -1; oy <= 1; oy = oy + 1) {
        let ny = gy_i + oy;
        if (ny < 0 || ny >= gdim_i) {
            continue;
        }
        for (var ox: i32 = -1; ox <= 1; ox = ox + 1) {
            let nx = gx_i + ox;
            if (nx < 0 || nx >= gdim_i) {
                continue;
            }
            let c_idx = u32(ny) * sim.grid_dim + u32(nx);
            var cur = atomicLoad(&cell_head[c_idx]);
            var guard: u32 = 0u;
            loop {
                if (cur < 0 || guard >= 48u) {
                    break;
                }
                let j = u32(cur);
                if (j != i) {
                    let diff = pos_i - nodes[j].pos;
                    let d2 = dot(diff, diff) + 0.00025;
                    let inv_d = inverseSqrt(d2);
                    let charge_j = clamp(sqrt(nodes[j].degree + 1.0), 1.0, 24.0);
                    let rep = (k_sq * sim.repulsion_k * charge_i * charge_j * 0.18) / d2;
                    force = force + diff * inv_d * rep;
                }
                cur = node_next[j];
                guard = guard + 1u;
            }
        }
    }

    // 2. Long-range 16x16 Barnes-Hut multipole supercell monopole repulsion
    let sdim_i = i32(sim.super_dim);
    for (var sy: i32 = 0; sy < sdim_i; sy = sy + 1) {
        for (var sx: i32 = 0; sx < sdim_i; sx = sx + 1) {
            if (abs(sx - sx_i) <= 0 && abs(sy - sy_i) <= 0) {
                continue;
            }
            let sc_base = (u32(sy) * sim.super_dim + u32(sx)) * 3u;
            let q_raw = atomicLoad(&super_grid[sc_base + 0u]);
            if (q_raw <= 0) {
                continue;
            }
            let q_mass = f32(q_raw) / 64.0;
            let com_x = (f32(atomicLoad(&super_grid[sc_base + 1u])) / 64.0) / q_mass;
            let com_y = (f32(atomicLoad(&super_grid[sc_base + 2u])) / 64.0) / q_mass;
            let diff = pos_i - vec2<f32>(com_x, com_y);
            let d2 = dot(diff, diff) + 0.004;
            let inv_d = inverseSqrt(d2);
            let rep = (k_sq * sim.repulsion_k * charge_i * q_mass * 0.14) / d2;
            force = force + diff * inv_d * rep;
        }
    }

    // 3. Exact CSR incident citation spring attraction
    let start_e = csr_offsets[i];
    let end_e = csr_offsets[i + 1u];
    let inv_ideal = 1.0 / max(sim.ideal_len, 0.008);
    for (var e_idx: u32 = start_e; e_idx < end_e; e_idx = e_idx + 1u) {
        let packed = csr_neighbors[e_idx];
        let nbr = packed & 0x7FFFFFFFu;
        let same_cluster = (packed & 0x80000000u) != 0u;
        if (nbr < sim.node_count && nbr != i) {
            let diff = nodes[nbr].pos - pos_i;
            let d = sqrt(dot(diff, diff) + 0.0001);
            let mult = select(0.72, 1.0 + sim.cohesion_k, same_cluster);
            let attr = (d * inv_ideal) * sim.spring_k * mult * 0.045;
            force = force + (diff / d) * attr;
        }
    }

    // 4. Community galaxy anchor + central gravity + damped Euler step
    let anchor_pull = (nodes[i].anchor - pos_i) * (sim.cohesion_k * 0.18);
    let grav_pull = -pos_i * (sim.gravity_k * 0.22);
    var vel = (nodes[i].vel + (force + anchor_pull + grav_pull) * 0.09) * sim.damping;

    let speed = length(vel);
    if (speed > sim.max_step) {
        vel = (vel / speed) * sim.max_step;
    }

    let bound = ext * 0.94;
    nodes[i].vel = vel;
    nodes[i].pos = clamp(pos_i + vel, vec2<f32>(-bound, -bound), vec2<f32>(bound, bound));
}
"#;

// ============================================================================
// EMBEDDED WGSL HARDWARE INSTANCED EDGE & SDF NODE RENDER SHADER
// ============================================================================
pub const WGSL_GRAPH_RENDER: &str = r#"
struct CameraUniforms {
    pan_x: f32,
    pan_y: f32,
    zoom: f32,
    aspect: f32,
    viewport_w: f32,
    viewport_h: f32,
    min_edge_eb: f32,
    edge_alpha_scale: f32,
    node_scale: f32,
    show_arrows: u32,
    curved_edges: u32,
    hovered_idx: i32,
};

struct GpuNodeState {
    pos: vec2<f32>,
    vel: vec2<f32>,
    anchor: vec2<f32>,
    radius: f32,
    rgba_packed: u32,
    flags: u32,
    degree: f32,
    pad0: f32,
    pad1: f32,
};

struct GpuEdgeInstance {
    src_idx: u32,
    dst_idx: u32,
    rgba_packed: u32,
    eb_encoded: f32,
};

@group(0) @binding(0) var<uniform> cam: CameraUniforms;
@group(0) @binding(1) var<storage, read> nodes: array<GpuNodeState>;
@group(0) @binding(2) var<storage, read> edges: array<GpuEdgeInstance>;

fn unpack_rgba(c: u32) -> vec4<f32> {
    let r = f32(c & 255u) / 255.0;
    let g = f32((c >> 8u) & 255u) / 255.0;
    let b = f32((c >> 16u) & 255u) / 255.0;
    let a = f32((c >> 24u) & 255u) / 255.0;
    return vec4<f32>(r, g, b, a);
}

fn world_to_ndc(w: vec2<f32>) -> vec2<f32> {
    let half_w = cam.viewport_w * 0.44 * cam.zoom;
    let half_h = cam.viewport_h * 0.44 * cam.zoom;
    let sx = cam.viewport_w * 0.5 + cam.pan_x + w.x * half_w;
    let sy = cam.viewport_h * 0.5 + cam.pan_y + w.y * half_h;
    let ndc_x = (sx / max(cam.viewport_w, 1.0)) * 2.0 - 1.0;
    let ndc_y = 1.0 - (sy / max(cam.viewport_h, 1.0)) * 2.0;
    return vec2<f32>(ndc_x, ndc_y);
}

fn world_to_px(w: vec2<f32>) -> vec2<f32> {
    let half_w = cam.viewport_w * 0.44 * cam.zoom;
    let half_h = cam.viewport_h * 0.44 * cam.zoom;
    return vec2<f32>(
        cam.viewport_w * 0.5 + cam.pan_x + w.x * half_w,
        cam.viewport_h * 0.5 + cam.pan_y + w.y * half_h
    );
}

fn px_to_ndc(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        (p.x / max(cam.viewport_w, 1.0)) * 2.0 - 1.0,
        1.0 - (p.y / max(cam.viewport_h, 1.0)) * 2.0
    );
}

struct EdgeVaryings {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) seg_len_px: f32,
};

@vertex
fn vs_edge(
    @builtin(vertex_index) v_idx: u32,
    @builtin(instance_index) inst_idx: u32,
) -> EdgeVaryings {
    var out: EdgeVaryings;
    let e = edges[inst_idx];
    let is_special = e.eb_encoded >= 4.5;
    let raw_eb = select(e.eb_encoded, 0.25, is_special);

    if (!is_special && raw_eb < cam.min_edge_eb) {
        out.clip_pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.color = vec4<f32>(0.0);
        out.uv = vec2<f32>(0.0);
        out.seg_len_px = 0.0;
        return out;
    }

    let s_node = nodes[e.src_idx];
    let t_node = nodes[e.dst_idx];
    let p1 = world_to_px(s_node.pos);
    let p2 = world_to_px(t_node.pos);
    let delta = p2 - p1;
    let len_px = length(delta);

    if (len_px < 1.5) {
        out.clip_pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.color = vec4<f32>(0.0);
        out.uv = vec2<f32>(0.0);
        out.seg_len_px = 0.0;
        return out;
    }

    let dir = delta / len_px;
    let nrm = vec2<f32>(-dir.y, dir.x);
    let base_col = unpack_rgba(e.rgba_packed);

    let both_spot = ((s_node.flags & 32u) != 0u) && ((t_node.flags & 32u) != 0u);
    let alpha_boost = select(0.14, 1.0, both_spot);
    let half_thick = select(0.85 + raw_eb * 2.2, 2.1, is_special) + f32(cam.curved_edges) * min(len_px * 0.06, 14.0);

    var quad_uv = vec2<f32>(0.0, 0.0);
    if (v_idx == 0u) { quad_uv = vec2<f32>(0.0, -1.0); }
    else if (v_idx == 1u) { quad_uv = vec2<f32>(1.0, -1.0); }
    else if (v_idx == 2u) { quad_uv = vec2<f32>(1.0, 1.0); }
    else if (v_idx == 3u) { quad_uv = vec2<f32>(0.0, -1.0); }
    else if (v_idx == 4u) { quad_uv = vec2<f32>(1.0, 1.0); }
    else { quad_uv = vec2<f32>(0.0, 1.0); }

    let px_pos = p1 + dir * (quad_uv.x * len_px) + nrm * (quad_uv.y * half_thick);
    out.clip_pos = vec4<f32>(px_to_ndc(px_pos), 0.0, 1.0);
    out.color = vec4<f32>(
        base_col.rgb,
        clamp(base_col.a * cam.edge_alpha_scale * alpha_boost, 0.0, 0.96)
    );
    out.uv = vec2<f32>(quad_uv.x, quad_uv.y * half_thick);
    out.seg_len_px = len_px;
    return out;
}

@fragment
fn fs_edge(in: EdgeVaryings) -> @location(0) vec4<f32> {
    let bow_amp = select(0.0, min(in.seg_len_px * 0.045, 11.0), cam.curved_edges == 1u);
    let center_y = 4.0 * bow_amp * in.uv.x * (1.0 - in.uv.x);
    let dist_y = abs(in.uv.y - center_y);

    var stroke_r = 0.78;
    if (cam.show_arrows == 1u && in.seg_len_px > 26.0 && in.uv.x > 0.84 && in.uv.x < 0.96) {
        let arrow_t = (0.96 - in.uv.x) / 0.12;
        stroke_r = max(stroke_r, arrow_t * 3.4);
    }

    let aa = max(fwidth(dist_y), 0.45);
    let cov = 1.0 - smoothstep(stroke_r - aa * 0.5, stroke_r + aa * 0.8, dist_y);
    let a = in.color.a * cov;
    if (a < 0.004) {
        discard;
    }
    return vec4<f32>(in.color.rgb, a);
}

struct NodeVaryings {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) flags: u32,
    @location(3) radius_px: f32,
};

@vertex
fn vs_node(
    @builtin(vertex_index) v_idx: u32,
    @builtin(instance_index) inst_idx: u32,
) -> NodeVaryings {
    var out: NodeVaryings;
    let n = nodes[inst_idx];
    let center_px = world_to_px(n.pos);

    let is_hovered = cam.hovered_idx >= 0 && u32(cam.hovered_idx) == inst_idx;
    let is_focal = (n.flags & 16u) != 0u;
    let r_base = clamp(n.radius * cam.node_scale * sqrt(clamp(cam.zoom, 0.25, 5.0)), 1.65, 30.0);
    let halo_pad = select(2.5, 6.5, is_focal || is_hovered || ((n.flags & 6u) != 0u));
    let quad_r = r_base + halo_pad;

    var corner = vec2<f32>(0.0, 0.0);
    if (v_idx == 0u) { corner = vec2<f32>(-1.0, -1.0); }
    else if (v_idx == 1u) { corner = vec2<f32>(1.0, -1.0); }
    else if (v_idx == 2u) { corner = vec2<f32>(1.0, 1.0); }
    else if (v_idx == 3u) { corner = vec2<f32>(-1.0, -1.0); }
    else if (v_idx == 4u) { corner = vec2<f32>(1.0, 1.0); }
    else { corner = vec2<f32>(-1.0, 1.0); }

    let px = center_px + corner * quad_r;
    let base_col = unpack_rgba(n.rgba_packed);
    let in_spot = (n.flags & 32u) != 0u;
    let alpha = select(0.18, base_col.a, in_spot);

    var eff_flags = n.flags;
    if (is_hovered) {
        eff_flags = eff_flags | 64u;
    }

    out.clip_pos = vec4<f32>(px_to_ndc(px), 0.0, 1.0);
    out.uv = corner * quad_r;
    out.color = vec4<f32>(base_col.rgb, alpha);
    out.flags = eff_flags;
    out.radius_px = r_base;
    return out;
}

@fragment
fn fs_node(in: NodeVaryings) -> @location(0) vec4<f32> {
    let is_diamond = (in.flags & 8u) != 0u;
    let is_hex_uniprot = (in.flags & 128u) != 0u;
    let is_oct_pdb = (in.flags & 256u) != 0u;
    let is_tri_epmc = (in.flags & 512u) != 0u;

    let d_circle = length(in.uv);
    let d_diamond = (abs(in.uv.x) + abs(in.uv.y)) * 0.78;
    let ax = abs(in.uv.x);
    let ay = abs(in.uv.y);
    let d_hex = max(ax * 0.8660254 + ay * 0.5, ay) * 1.04;
    let d_oct = max(max(ax, ay), (ax + ay) * 0.70710678) * 1.05;
    let d_tri = max(ax * 0.8660254 - in.uv.y * 0.5, in.uv.y) * 1.22;

    var dist_px = d_circle;
    if (is_diamond) {
        dist_px = d_diamond;
    } else if (is_hex_uniprot) {
        dist_px = d_hex;
    } else if (is_oct_pdb) {
        dist_px = d_oct;
    } else if (is_tri_epmc) {
        dist_px = d_tri;
    }
    let aa = max(fwidth(dist_px), 0.65);

    let r = in.radius_px;
    let body_cov = 1.0 - smoothstep(r - aa, r + aa * 0.5, dist_px);

    // Specular rim when radius >= 3.5 px
    let rim_band = smoothstep(r - 1.35 - aa, r - 0.85, dist_px) * body_cov * step(3.5, r);
    let is_pinned = (in.flags & 1u) != 0u;
    let rim_rgb = select(vec3<f32>(0.96, 0.98, 1.0), vec3<f32>(0.98, 0.80, 0.08), is_pinned);
    var rgb = mix(in.color.rgb, rim_rgb, rim_band * 0.72);
    var alpha = in.color.a * body_cov;

    // Outer integrity / Sleeping Beauty / focal / hover rings
    let is_retracted = (in.flags & 2u) != 0u;
    let is_beauty = (in.flags & 4u) != 0u;
    let is_focal_or_hover = (in.flags & 80u) != 0u;

    if (is_retracted) {
        let ring_d = abs(dist_px - (r + 3.2));
        let ring_a = 1.0 - smoothstep(0.9, 0.9 + aa, ring_d);
        rgb = mix(rgb, vec3<f32>(0.94, 0.27, 0.27), ring_a);
        alpha = max(alpha, ring_a * 0.92);
    } else if (is_beauty && r >= 3.0) {
        let ring_d = abs(dist_px - (r + 2.6));
        let ring_a = 1.0 - smoothstep(0.7, 0.7 + aa, ring_d);
        rgb = mix(rgb, vec3<f32>(0.98, 0.80, 0.08), ring_a);
        alpha = max(alpha, ring_a * 0.82);
    }

    if (is_focal_or_hover) {
        let ring_d = abs(dist_px - (r + 4.4));
        let ring_a = 1.0 - smoothstep(1.0, 1.0 + aa, ring_d);
        rgb = mix(rgb, vec3<f32>(0.22, 0.74, 0.97), ring_a);
        alpha = max(alpha, ring_a * 0.95);
    }

    if (alpha < 0.005) {
        discard;
    }
    return vec4<f32>(rgb, alpha);
}
"#;

// ============================================================================
// SHARED GPU READBACK MIRROR & EGUI_WGPU CALLBACK PIPELINE
// ============================================================================
pub struct GpuReadbackMirror {
    pub coords: Vec<[f32; 2]>,
    pub pick_grid: SpatialPickGrid,
    pub gpu_frames_dispatched: u64,
    pub active_node_count: usize,
    pub active_edge_count: usize,
}

impl Default for GpuReadbackMirror {
    fn default() -> Self {
        Self {
            coords: Vec::new(),
            pick_grid: SpatialPickGrid::default(),
            gpu_frames_dispatched: 0,
            active_node_count: 0,
            active_edge_count: 0,
        }
    }
}

pub struct WgpuGraphPipelineResources {
    pub clear_pipeline: wgpu::ComputePipeline,
    pub bin_pipeline: wgpu::ComputePipeline,
    pub force_pipeline: wgpu::ComputePipeline,
    pub edge_render_pipeline: wgpu::RenderPipeline,
    pub node_render_pipeline: wgpu::RenderPipeline,

    pub compute_bgl: wgpu::BindGroupLayout,
    pub render_bgl: wgpu::BindGroupLayout,

    pub sim_uniform_buf: wgpu::Buffer,
    pub cam_uniform_buf: wgpu::Buffer,
    pub cell_head_buf: wgpu::Buffer,
    pub super_grid_buf: wgpu::Buffer,

    pub node_buf: wgpu::Buffer,
    pub node_next_buf: wgpu::Buffer,
    pub csr_offsets_buf: wgpu::Buffer,
    pub csr_neighbors_buf: wgpu::Buffer,
    pub edge_buf: wgpu::Buffer,
    pub readback_buf: wgpu::Buffer,

    pub compute_bg: wgpu::BindGroup,
    pub render_bg: wgpu::BindGroup,

    pub node_capacity: usize,
    pub edge_capacity: usize,
    pub csr_neighbor_capacity: usize,
    pub uploaded_revision: u64,
    pub node_count: u32,
    pub edge_count: u32,

    pub readback_in_flight: Arc<AtomicBool>,
    pub readback_ready: Arc<AtomicBool>,
    pub frame_counter: u64,
}

impl WgpuGraphPipelineResources {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CitAtlas WGSL Graph Compute"),
            source: wgpu::ShaderSource::Wgsl(WGSL_GRAPH_COMPUTE.into()),
        });
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CitAtlas WGSL Instanced Graph Render"),
            source: wgpu::ShaderSource::Wgsl(WGSL_GRAPH_RENDER.into()),
        });

        let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("CitAtlas Compute BGL"),
            entries: &[
                bgl_entry(0, wgpu::ShaderStages::COMPUTE, wgpu::BufferBindingType::Uniform),
                bgl_entry(
                    1,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                ),
                bgl_entry(
                    2,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                ),
                bgl_entry(
                    3,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                ),
                bgl_entry(
                    4,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                ),
                bgl_entry(
                    5,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                bgl_entry(
                    6,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
            ],
        });

        let render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("CitAtlas Render BGL"),
            entries: &[
                bgl_entry(
                    0,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Uniform,
                ),
                bgl_entry(
                    1,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                bgl_entry(
                    2,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
            ],
        });

        let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("CitAtlas Compute Pipeline Layout"),
            bind_group_layouts: &[&compute_bgl],
            push_constant_ranges: &[],
        });
        let render_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("CitAtlas Render Pipeline Layout"),
            bind_group_layouts: &[&render_bgl],
            push_constant_ranges: &[],
        });

        let clear_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_clear_grids"),
            layout: Some(&compute_layout),
            module: &compute_shader,
            entry_point: Some("cs_clear_grids"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bin_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_bin_nodes"),
            layout: Some(&compute_layout),
            module: &compute_shader,
            entry_point: Some("cs_bin_nodes"),
            compilation_options: Default::default(),
            cache: None,
        });
        let force_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_forces_and_integrate"),
            layout: Some(&compute_layout),
            module: &compute_shader,
            entry_point: Some("cs_forces_and_integrate"),
            compilation_options: Default::default(),
            cache: None,
        });

        let edge_render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("CitAtlas Edge Instanced Pipeline"),
            layout: Some(&render_layout),
            vertex: wgpu::VertexState {
                module: &render_shader,
                entry_point: Some("vs_edge"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &render_shader,
                entry_point: Some("fs_edge"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let node_render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("CitAtlas Node Instanced SDF Pipeline"),
            layout: Some(&render_layout),
            vertex: wgpu::VertexState {
                module: &render_shader,
                entry_point: Some("vs_node"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &render_shader,
                entry_point: Some("fs_node"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let sim_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SimUniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cam_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("CameraUniforms"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cell_head_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SpatialHashCellHead"),
            size: (GRID_CELLS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let super_grid_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("MultipoleSuperGrid"),
            size: (SUPER_CELLS * 3 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let init_nodes = 1024usize;
        let init_edges = 4096usize;
        let (node_buf, node_next_buf, csr_offsets_buf, csr_neighbors_buf, edge_buf, readback_buf) =
            allocate_dynamic_buffers(device, init_nodes, init_edges, init_edges * 2);

        let compute_bg = create_compute_bind_group(
            device,
            &compute_bgl,
            &sim_uniform_buf,
            &node_buf,
            &cell_head_buf,
            &node_next_buf,
            &super_grid_buf,
            &csr_offsets_buf,
            &csr_neighbors_buf,
        );
        let render_bg = create_render_bind_group(
            device,
            &render_bgl,
            &cam_uniform_buf,
            &node_buf,
            &edge_buf,
        );

        Self {
            clear_pipeline,
            bin_pipeline,
            force_pipeline,
            edge_render_pipeline,
            node_render_pipeline,
            compute_bgl,
            render_bgl,
            sim_uniform_buf,
            cam_uniform_buf,
            cell_head_buf,
            super_grid_buf,
            node_buf,
            node_next_buf,
            csr_offsets_buf,
            csr_neighbors_buf,
            edge_buf,
            readback_buf,
            compute_bg,
            render_bg,
            node_capacity: init_nodes,
            edge_capacity: init_edges,
            csr_neighbor_capacity: init_edges * 2,
            uploaded_revision: 0,
            node_count: 0,
            edge_count: 0,
            readback_in_flight: Arc::new(AtomicBool::new(false)),
            readback_ready: Arc::new(AtomicBool::new(false)),
            frame_counter: 0,
        }
    }

    pub fn ensure_capacity(
        &mut self,
        device: &wgpu::Device,
        req_nodes: usize,
        req_edges: usize,
        req_csr_nbrs: usize,
    ) {
        if req_nodes <= self.node_capacity
            && req_edges <= self.edge_capacity
            && req_csr_nbrs <= self.csr_neighbor_capacity
        {
            return;
        }
        let new_n = req_nodes.max(self.node_capacity).next_power_of_two().max(1024);
        let new_e = req_edges.max(self.edge_capacity).next_power_of_two().max(4096);
        let new_csr = req_csr_nbrs
            .max(self.csr_neighbor_capacity)
            .next_power_of_two()
            .max(4096);

        let (node_buf, node_next_buf, csr_offsets_buf, csr_neighbors_buf, edge_buf, readback_buf) =
            allocate_dynamic_buffers(device, new_n, new_e, new_csr);

        self.node_buf = node_buf;
        self.node_next_buf = node_next_buf;
        self.csr_offsets_buf = csr_offsets_buf;
        self.csr_neighbors_buf = csr_neighbors_buf;
        self.edge_buf = edge_buf;
        self.readback_buf = readback_buf;
        self.node_capacity = new_n;
        self.edge_capacity = new_e;
        self.csr_neighbor_capacity = new_csr;
        self.readback_in_flight.store(false, Ordering::SeqCst);
        self.readback_ready.store(false, Ordering::SeqCst);

        self.compute_bg = create_compute_bind_group(
            device,
            &self.compute_bgl,
            &self.sim_uniform_buf,
            &self.node_buf,
            &self.cell_head_buf,
            &self.node_next_buf,
            &self.super_grid_buf,
            &self.csr_offsets_buf,
            &self.csr_neighbors_buf,
        );
        self.render_bg = create_render_bind_group(
            device,
            &self.render_bgl,
            &self.cam_uniform_buf,
            &self.node_buf,
            &self.edge_buf,
        );
    }
}

fn bgl_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn allocate_dynamic_buffers(
    device: &wgpu::Device,
    node_cap: usize,
    edge_cap: usize,
    csr_cap: usize,
) -> (
    wgpu::Buffer,
    wgpu::Buffer,
    wgpu::Buffer,
    wgpu::Buffer,
    wgpu::Buffer,
    wgpu::Buffer,
) {
    let node_bytes = (node_cap * 48) as u64;
    let node_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("GpuNodeStateBuffer"),
        size: node_bytes,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let node_next_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("GpuNodeNextChain"),
        size: (node_cap * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let csr_offsets_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("CsrOffsetsBuffer"),
        size: ((node_cap + 2) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let csr_neighbors_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("CsrNeighborsBuffer"),
        size: (csr_cap.max(4) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let edge_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("GpuEdgeInstanceBuffer"),
        size: (edge_cap.max(4) * 16) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("GpuNodeReadbackStaging"),
        size: node_bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (
        node_buf,
        node_next_buf,
        csr_offsets_buf,
        csr_neighbors_buf,
        edge_buf,
        readback_buf,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_compute_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sim_u: &wgpu::Buffer,
    nodes: &wgpu::Buffer,
    cell_head: &wgpu::Buffer,
    node_next: &wgpu::Buffer,
    super_grid: &wgpu::Buffer,
    csr_off: &wgpu::Buffer,
    csr_nbr: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("CitAtlas Compute BindGroup"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: sim_u.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: nodes.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: cell_head.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: node_next.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: super_grid.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: csr_off.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: csr_nbr.as_entire_binding() },
        ],
    })
}

fn create_render_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    cam_u: &wgpu::Buffer,
    nodes: &wgpu::Buffer,
    edges: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("CitAtlas Render BindGroup"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: cam_u.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: nodes.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: edges.as_entire_binding() },
        ],
    })
}

/// Prepared scene upload snapshot created only when corpus/layout/visual-encoding revision changes.
#[derive(Clone)]
pub struct PreparedGpuUpload {
    pub revision: u64,
    pub node_bytes: Arc<Vec<u8>>,
    pub edge_bytes: Arc<Vec<u8>>,
    pub csr_offsets_bytes: Arc<Vec<u8>>,
    pub csr_neighbors_bytes: Arc<Vec<u8>>,
    pub node_count: usize,
    pub edge_count: usize,
    pub csr_neighbor_count: usize,
}

/// Builds a full `PreparedGpuUpload` from `CorpusAnalytics` and `GraphViewerState`.
pub fn build_gpu_upload_payload(
    revision: u64,
    analytics: &CorpusAnalytics,
    viewer: &GraphViewerState,
    focal_id: &str,
    geodesic_pairs: &HashSet<(usize, usize)>,
) -> PreparedGpuUpload {
    let n_papers = analytics.papers.len();
    let n_artifacts = if viewer.show_datacite_diamonds {
        viewer.artifact_nodes.len()
    } else {
        0
    };
    let n_bio = if viewer.show_bio_entity_nodes {
        viewer.bio_entity_nodes.len()
    } else {
        0
    };
    let total_nodes = n_papers + n_artifacts + n_bio;

    let max_cb = analytics.papers.iter().map(|p| p.cb_undir).fold(0.05, f64::max);
    let max_v = analytics.papers.iter().map(|p| p.v_exp).fold(0.5, f64::max);
    let max_b = analytics.papers.iter().map(|p| p.beauty_b).fold(5.0, f64::max);
    let spot_mask = compute_spotlight_mask(analytics, focal_id, viewer.ego_mode);

    let cluster_total = match viewer.color_by {
        NodeColorEncoding::LouvainCommunity => analytics.communities.louvain.num_communities.max(1),
        NodeColorEncoding::InfomapModule => analytics.communities.infomap.num_modules.max(1),
        _ => 6,
    };

    let mut gpu_nodes = Vec::with_capacity(total_nodes.max(1));
    let mut cluster_ids = Vec::with_capacity(total_nodes);

    for (i, p) in analytics.papers.iter().enumerate() {
        let [px, py] = viewer
            .positions
            .get(&p.record.id)
            .copied()
            .unwrap_or([0.0, 0.0]);
        let [vx, vy] = viewer
            .velocities
            .get(&p.record.id)
            .copied()
            .unwrap_or([0.0, 0.0]);

        let c_idx = match viewer.color_by {
            NodeColorEncoding::LouvainCommunity => p.louvain_comm,
            NodeColorEncoding::InfomapModule => p.infomap_comm,
            _ => p.record.disc.index(),
        };
        cluster_ids.push(c_idx);
        let ang = (2.0 * std::f32::consts::PI * (c_idx as f32)) / (cluster_total as f32)
            - std::f32::consts::FRAC_PI_2;
        let anchor = [ang.cos() * 0.45, ang.sin() * 0.45];

        let [cr, cg, cb] = node_rgb_for_encoding(p, viewer.color_by, max_v, max_b);
        let radius = node_radius_for_encoding(p, viewer.size_by, max_cb, max_v, max_b);

        let mut flags = 0u32;
        if viewer.pinned_ids.contains(&p.record.id) {
            flags |= NODE_FLAG_PINNED;
        }
        if p.record.retraction.status.is_retracted_or_withdrawn() {
            flags |= NODE_FLAG_RETRACTED;
        }
        if p.beauty_b >= 10.0 {
            flags |= NODE_FLAG_SLEEPING_BEAUTY;
        }
        if p.record.id == focal_id {
            flags |= NODE_FLAG_FOCAL;
        }
        if spot_mask.get(i).copied().unwrap_or(true) {
            flags |= NODE_FLAG_IN_SPOTLIGHT;
        }

        gpu_nodes.push(GpuNodeRecord {
            pos: [px, py],
            vel: [vx, vy],
            anchor,
            radius,
            rgba_packed: pack_rgba_u32(cr, cg, cb, 236),
            flags,
            degree: p.undir_deg as f32,
            pad0: 0.0,
            pad1: 0.0,
        });
    }

    let mut bipartite_links: Vec<(usize, usize)> = Vec::new();
    if viewer.show_datacite_diamonds {
        for (k, art) in viewer.artifact_nodes.iter().enumerate() {
            let art_idx = n_papers + k;
            cluster_ids.push(0);
            let (cr, cg, cb) = if art.kind.is_software_like() {
                (167u8, 139u8, 250u8)
            } else {
                (45u8, 212u8, 191u8)
            };
            let any_spot = art
                .linked_paper_indices
                .iter()
                .any(|&pi| spot_mask.get(pi).copied().unwrap_or(true));
            let mut flags = NODE_FLAG_DIAMOND_ARTIFACT;
            if any_spot {
                flags |= NODE_FLAG_IN_SPOTLIGHT;
            }
            for &pi in &art.linked_paper_indices {
                bipartite_links.push((pi, art_idx));
            }
            gpu_nodes.push(GpuNodeRecord {
                pos: art.pos,
                vel: [0.0, 0.0],
                anchor: art.pos,
                radius: 6.4,
                rgba_packed: pack_rgba_u32(cr, cg, cb, 225),
                flags,
                degree: art.linked_paper_indices.len() as f32,
                pad0: 0.0,
                pad1: 0.0,
            });
        }
    }

    let mut bio_bipartite_links: Vec<(usize, usize, BioEntityKind)> = Vec::new();
    let mut bio_cross_links: Vec<(usize, usize)> = Vec::new();
    if viewer.show_bio_entity_nodes {
        let bio_base = n_papers + n_artifacts;
        for (k, bn) in viewer.bio_entity_nodes.iter().enumerate() {
            let bio_idx = bio_base + k;
            cluster_ids.push(0);
            let [cr, cg, cb] = bn.kind.rgb();
            let any_spot = bn
                .linked_paper_indices
                .iter()
                .any(|&pi| spot_mask.get(pi).copied().unwrap_or(true));
            let mut flags = match bn.kind {
                BioEntityKind::UniProt => NODE_FLAG_HEXAGON_UNIPROT,
                BioEntityKind::PdbStructure => NODE_FLAG_OCTAGON_PDB,
                _ => NODE_FLAG_TRIANGLE_EPMC,
            };
            if any_spot {
                flags |= NODE_FLAG_IN_SPOTLIGHT;
            }
            for &pi in &bn.linked_paper_indices {
                bio_bipartite_links.push((pi, bio_idx, bn.kind));
            }
            let rad = match bn.kind {
                BioEntityKind::UniProt => 7.0,
                BioEntityKind::PdbStructure => 6.8,
                _ => 6.0,
            };
            gpu_nodes.push(GpuNodeRecord {
                pos: bn.pos,
                vel: [0.0, 0.0],
                anchor: bn.pos,
                radius: rad,
                rgba_packed: pack_rgba_u32(cr, cg, cb, 232),
                flags,
                degree: bn.linked_paper_indices.len() as f32,
                pad0: 0.0,
                pad1: 0.0,
            });
        }
        for &(u_rel, p_rel) in &viewer.bio_cross_edges {
            if u_rel < viewer.bio_entity_nodes.len() && p_rel < viewer.bio_entity_nodes.len() {
                bio_cross_links.push((bio_base + u_rel, bio_base + p_rel));
            }
        }
    }

    let mut conduit_eb: HashMap<(usize, usize), f32> = HashMap::new();
    for c in &analytics.conduits {
        conduit_eb.insert((c.u, c.v), c.eb_undir as f32);
    }

    let mut gpu_edges = Vec::with_capacity(analytics.active_edges.len() + bipartite_links.len());
    for &(u, v) in &analytics.active_edges {
        let is_geo = geodesic_pairs.contains(&(u, v));
        let is_zombie = viewer.highlight_zombie_edges
            && analytics.papers[v]
                .record
                .retraction
                .status
                .is_retracted_or_withdrawn()
            && analytics.papers[u].record.year
                >= analytics.papers[v]
                    .record
                    .retraction
                    .notice_year
                    .unwrap_or(analytics.papers[v].record.year);

        let eb = conduit_eb.get(&(u, v)).copied().unwrap_or(0.01);
        let (rgba, eb_enc) = if is_geo {
            (pack_rgba_u32(34, 211, 238, 245), 12.0)
        } else if is_zombie {
            (pack_rgba_u32(248, 113, 113, 230), 6.0)
        } else if analytics.papers[u].record.disc != analytics.papers[v].record.disc {
            (pack_rgba_u32(148, 163, 184, 155), eb)
        } else {
            (pack_rgba_u32(100, 116, 139, 115), eb)
        };

        gpu_edges.push(GpuEdgeRecord {
            src_idx: u as u32,
            dst_idx: v as u32,
            rgba_packed: rgba,
            eb_encoded: eb_enc,
        });
    }

    for &(pi, art_idx) in &bipartite_links {
        gpu_edges.push(GpuEdgeRecord {
            src_idx: pi as u32,
            dst_idx: art_idx as u32,
            rgba_packed: pack_rgba_u32(45, 212, 191, 180),
            eb_encoded: 0.15,
        });
    }

    let mut all_bipartite_csr = bipartite_links;
    for &(pi, bio_idx, kind) in &bio_bipartite_links {
        let [cr, cg, cb] = kind.rgb();
        gpu_edges.push(GpuEdgeRecord {
            src_idx: pi as u32,
            dst_idx: bio_idx as u32,
            rgba_packed: pack_rgba_u32(cr, cg, cb, 182),
            eb_encoded: 0.16,
        });
        all_bipartite_csr.push((pi, bio_idx));
    }
    for &(u_bio, p_bio) in &bio_cross_links {
        gpu_edges.push(GpuEdgeRecord {
            src_idx: u_bio as u32,
            dst_idx: p_bio as u32,
            rgba_packed: pack_rgba_u32(34, 211, 238, 220),
            eb_encoded: 0.22,
        });
        all_bipartite_csr.push((u_bio, p_bio));
    }

    let (csr_off, csr_nbr) = build_csr_buffers(
        total_nodes,
        &analytics.undir_adj,
        &all_bipartite_csr,
        &cluster_ids,
    );

    PreparedGpuUpload {
        revision,
        node_bytes: Arc::new(encode_gpu_nodes_bytes(&gpu_nodes)),
        edge_bytes: Arc::new(encode_gpu_edges_bytes(&gpu_edges)),
        csr_offsets_bytes: Arc::new(encode_u32_slice_bytes(&csr_off)),
        csr_neighbors_bytes: Arc::new(encode_u32_slice_bytes(&csr_nbr)),
        node_count: total_nodes,
        edge_count: gpu_edges.len(),
        csr_neighbor_count: csr_nbr.len(),
    }
}

/// Per-frame `egui_wgpu::CallbackTrait` implementation driving WGSL compute & instanced rendering.
pub struct WgpuGraphCallback {
    pub upload: Option<PreparedGpuUpload>,
    pub sim: SimUniformsPayload,
    pub cam: CameraUniformsPayload,
    pub mirror: Arc<Mutex<GpuReadbackMirror>>,
}

impl egui_wgpu::CallbackTrait for WgpuGraphCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = callback_resources.get_mut::<WgpuGraphPipelineResources>() else {
            return Vec::new();
        };

        // 1. Harvest any completed async GPU -> CPU position readback
        let _ = device.poll(wgpu::Maintain::Poll);
        if res.readback_ready.swap(false, Ordering::SeqCst) {
            let byte_len = (res.node_count as usize) * 48;
            if byte_len > 0 {
                let slice = res.readback_buf.slice(0..byte_len as u64);
                let data = slice.get_mapped_range();
                if let Ok(mut mir) = self.mirror.lock() {
                    let count = res.node_count as usize;
                    mir.coords.resize(count, [0.0, 0.0]);
                    for i in 0..count {
                        let base = i * 48;
                        if base + 8 <= data.len() {
                            let x = f32::from_le_bytes([
                                data[base],
                                data[base + 1],
                                data[base + 2],
                                data[base + 3],
                            ]);
                            let y = f32::from_le_bytes([
                                data[base + 4],
                                data[base + 5],
                                data[base + 6],
                                data[base + 7],
                            ]);
                            mir.coords[i] = [x, y];
                        }
                    }
                    let coords_clone = mir.coords.clone();
                    mir.pick_grid.rebuild(&coords_clone);
                }
                drop(data);
            }
            res.readback_buf.unmap();
            res.readback_in_flight.store(false, Ordering::SeqCst);
        }

        // 2. Upload topology & node states only when scene revision changed
        if let Some(ref up) = self.upload {
            if up.revision != res.uploaded_revision {
                if res.readback_in_flight.load(Ordering::SeqCst)
                    || res.readback_ready.load(Ordering::SeqCst)
                {
                    res.readback_buf.unmap();
                    let _ = device.poll(wgpu::Maintain::Poll);
                    res.readback_in_flight.store(false, Ordering::SeqCst);
                    res.readback_ready.store(false, Ordering::SeqCst);
                }
                res.ensure_capacity(
                    device,
                    up.node_count.max(1),
                    up.edge_count.max(1),
                    up.csr_neighbor_count.max(1),
                );
                if !up.node_bytes.is_empty() {
                    queue.write_buffer(&res.node_buf, 0, &up.node_bytes);
                }
                if !up.edge_bytes.is_empty() {
                    queue.write_buffer(&res.edge_buf, 0, &up.edge_bytes);
                }
                if !up.csr_offsets_bytes.is_empty() {
                    queue.write_buffer(&res.csr_offsets_buf, 0, &up.csr_offsets_bytes);
                }
                if !up.csr_neighbors_bytes.is_empty() {
                    queue.write_buffer(&res.csr_neighbors_buf, 0, &up.csr_neighbors_bytes);
                }
                res.node_count = up.node_count as u32;
                res.edge_count = up.edge_count as u32;
                res.uploaded_revision = up.revision;
            }
        }

        // 3. Always update 64B SimUniforms and 48B CameraUniforms
        let mut sim_payload = self.sim;
        sim_payload.node_count = res.node_count;
        queue.write_buffer(&res.sim_uniform_buf, 0, &encode_sim_uniforms_bytes(&sim_payload));
        queue.write_buffer(&res.cam_uniform_buf, 0, &encode_camera_uniforms_bytes(&self.cam));

        // 4. Dispatch 3-pass WGSL Compute Pipeline on a dedicated encoder so
        // `queue.submit` runs strictly before `map_async` is invoked on `readback_buf`.
        if res.node_count > 0 && (sim_payload.run_step == 1 || sim_payload.dragged_idx >= 0) {
            let mut compute_enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("CitAtlas Dedicated Compute & Readback Encoder"),
            });
            let clear_groups = ((GRID_CELLS as u32) + 255) / 256;
            let node_groups = (res.node_count + 255) / 256;

            {
                let mut cpass = compute_enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("CitAtlas WGSL Graph Force Pass"),
                    timestamp_writes: None,
                });
                cpass.set_bind_group(0, &res.compute_bg, &[]);

                cpass.set_pipeline(&res.clear_pipeline);
                cpass.dispatch_workgroups(clear_groups, 1, 1);

                cpass.set_pipeline(&res.bin_pipeline);
                cpass.dispatch_workgroups(node_groups, 1, 1);

                cpass.set_pipeline(&res.force_pipeline);
                cpass.dispatch_workgroups(node_groups, 1, 1);
            }

            res.frame_counter += 1;
            if let Ok(mut mir) = self.mirror.lock() {
                mir.gpu_frames_dispatched = res.frame_counter;
                mir.active_node_count = res.node_count as usize;
                mir.active_edge_count = res.edge_count as usize;
            }

            // Copy to readback staging buffer on every 6th physics frame
            let mut readback_bytes: u64 = 0;
            if res.frame_counter % 6 == 0
                && !res.readback_in_flight.load(Ordering::SeqCst)
                && !res.readback_ready.load(Ordering::SeqCst)
            {
                let copy_bytes = (res.node_count as u64) * 48;
                compute_enc.copy_buffer_to_buffer(
                    &res.node_buf,
                    0,
                    &res.readback_buf,
                    0,
                    copy_bytes,
                );
                readback_bytes = copy_bytes;
            }

            queue.submit(std::iter::once(compute_enc.finish()));

            // Only map readback buffer AFTER `compute_enc` has been submitted to `queue`
            if readback_bytes > 0 && !res.readback_in_flight.swap(true, Ordering::SeqCst) {
                let ready_flag = Arc::clone(&res.readback_ready);
                let inflight_flag = Arc::clone(&res.readback_in_flight);
                res.readback_buf
                    .slice(0..readback_bytes)
                    .map_async(wgpu::MapMode::Read, move |map_res| {
                        if map_res.is_ok() {
                            ready_flag.store(true, Ordering::SeqCst);
                        } else {
                            inflight_flag.store(false, Ordering::SeqCst);
                        }
                    });
            }
        } else if let Ok(mut mir) = self.mirror.lock() {
            mir.active_node_count = res.node_count as usize;
            mir.active_edge_count = res.edge_count as usize;
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = callback_resources.get::<WgpuGraphPipelineResources>() else {
            return;
        };
        if res.node_count == 0 {
            return;
        }

        render_pass.set_bind_group(0, &res.render_bg, &[]);

        if res.edge_count > 0 {
            render_pass.set_pipeline(&res.edge_render_pipeline);
            render_pass.draw(0..6, 0..res.edge_count);
        }

        render_pass.set_pipeline(&res.node_render_pipeline);
        render_pass.draw(0..6, 0..res.node_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui_wgpu::CallbackTrait;

    fn block_on_test<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct ThreadWaker(std::thread::Thread);
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(out) => return out,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    #[test]
    fn consecutive_drag_frames_with_mod6_readbacks_do_not_panic() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let Some(adapter) = block_on_test(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })) else {
            return;
        };
        let Ok((device, queue)) = block_on_test(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("cit_atlas_drag_test_device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        )) else {
            return;
        };

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut resources = egui_wgpu::CallbackResources::default();
        resources.insert(WgpuGraphPipelineResources::new(&device, format));

        let n_nodes = 12usize;
        let gpu_nodes: Vec<GpuNodeRecord> = (0..n_nodes)
            .map(|i| {
                let ang = (i as f32) * std::f32::consts::TAU / (n_nodes as f32);
                let pos = [0.45 * ang.cos(), 0.45 * ang.sin()];
                GpuNodeRecord {
                    pos,
                    vel: [0.0, 0.0],
                    anchor: pos,
                    radius: 7.0,
                    rgba_packed: pack_rgba_u32(56, 189, 248, 255),
                    flags: 0,
                    degree: 2.0,
                    pad0: 0.0,
                    pad1: 0.0,
                }
            })
            .collect();
        let gpu_edges: Vec<GpuEdgeRecord> = (0..n_nodes)
            .map(|i| GpuEdgeRecord {
                src_idx: i as u32,
                dst_idx: ((i + 1) % n_nodes) as u32,
                rgba_packed: pack_rgba_u32(148, 163, 184, 180),
                eb_encoded: 0.5,
            })
            .collect();
        let mut csr_off = Vec::with_capacity(n_nodes + 1);
        let mut csr_nbr = Vec::with_capacity(n_nodes * 2);
        for i in 0..n_nodes {
            csr_off.push(csr_nbr.len() as u32);
            csr_nbr.push(((i + 1) % n_nodes) as u32);
            csr_nbr.push(((i + n_nodes - 1) % n_nodes) as u32);
        }
        csr_off.push(csr_nbr.len() as u32);

        let initial_upload = PreparedGpuUpload {
            revision: 1,
            node_bytes: Arc::new(encode_gpu_nodes_bytes(&gpu_nodes)),
            edge_bytes: Arc::new(encode_gpu_edges_bytes(&gpu_edges)),
            csr_offsets_bytes: Arc::new(encode_u32_slice_bytes(&csr_off)),
            csr_neighbors_bytes: Arc::new(encode_u32_slice_bytes(&csr_nbr)),
            node_count: n_nodes,
            edge_count: gpu_edges.len(),
            csr_neighbor_count: csr_nbr.len(),
        };

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("drag_test_target"),
            size: wgpu::Extent3d {
                width: 640,
                height: 480,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mirror = Arc::new(Mutex::new(GpuReadbackMirror::default()));
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [640, 480],
            pixels_per_point: 1.0,
        };

        // Drive 36 consecutive frames while dragging node 0 across mod-6 readback boundaries
        // (frames 6, 12, 18, 24, 30, 36), with physics paused so drag alone activates compute.
        for frame in 0..36u32 {
            let drag_x = -0.3 + (frame as f32) * 0.02;
            let drag_y = 0.25 - (frame as f32) * 0.012;
            let cb = WgpuGraphCallback {
                upload: if frame == 0 { Some(initial_upload.clone()) } else { None },
                sim: SimUniformsPayload {
                    node_count: n_nodes as u32,
                    grid_dim: GRID_DIM,
                    super_dim: SUPER_DIM,
                    run_step: 0,
                    world_extent: WORLD_EXTENT,
                    repulsion_k: 0.45,
                    spring_k: 0.70,
                    gravity_k: 0.15,
                    cohesion_k: 0.35,
                    damping: 0.85,
                    ideal_len: 0.28,
                    max_step: 0.032,
                    dragged_idx: 0,
                    drag_x,
                    drag_y,
                    pad0: 0.0,
                },
                cam: CameraUniformsPayload {
                    pan_x: 0.0,
                    pan_y: 0.0,
                    zoom: 1.0,
                    aspect: 640.0 / 480.0,
                    viewport_w: 640.0,
                    viewport_h: 480.0,
                    min_edge_eb: 0.0,
                    edge_alpha_scale: 0.85,
                    node_scale: 1.0,
                    show_arrows: 1,
                    curved_edges: 0,
                    hovered_idx: 0,
                },
                mirror: Arc::clone(&mirror),
            };

            let mut egui_enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("drag_test_frame_enc"),
            });
            let extra = cb.prepare(&device, &queue, &screen_desc, &mut egui_enc, &mut resources);
            {
                let mut rpass = egui_enc
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("drag_test_rpass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    })
                    .forget_lifetime();
                let paint_info = egui::PaintCallbackInfo {
                    viewport: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 480.0)),
                    clip_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 480.0)),
                    pixels_per_point: 1.0,
                    screen_size_px: [640, 480],
                };
                cb.paint(paint_info, &mut rpass, &resources);
            }
            let mut bufs = extra;
            bufs.push(egui_enc.finish());
            queue.submit(bufs);
            let _ = device.poll(wgpu::Maintain::Wait);
        }

        // One final poll-harvest prepare frame after frame 36 readback completes
        let mut final_enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("drag_test_final_harvest"),
        });
        let final_cb = WgpuGraphCallback {
            upload: None,
            sim: SimUniformsPayload {
                node_count: n_nodes as u32,
                grid_dim: GRID_DIM,
                super_dim: SUPER_DIM,
                run_step: 0,
                world_extent: WORLD_EXTENT,
                repulsion_k: 0.45,
                spring_k: 0.70,
                gravity_k: 0.15,
                cohesion_k: 0.35,
                damping: 0.85,
                ideal_len: 0.28,
                max_step: 0.032,
                dragged_idx: -1,
                drag_x: 0.0,
                drag_y: 0.0,
                pad0: 0.0,
            },
            cam: CameraUniformsPayload {
                pan_x: 0.0,
                pan_y: 0.0,
                zoom: 1.0,
                aspect: 640.0 / 480.0,
                viewport_w: 640.0,
                viewport_h: 480.0,
                min_edge_eb: 0.0,
                edge_alpha_scale: 0.85,
                node_scale: 1.0,
                show_arrows: 1,
                curved_edges: 0,
                hovered_idx: -1,
            },
            mirror: Arc::clone(&mirror),
        };
        let _ = final_cb.prepare(&device, &queue, &screen_desc, &mut final_enc, &mut resources);

        let mir = mirror.lock().unwrap();
        assert_eq!(mir.gpu_frames_dispatched, 36);
        assert_eq!(mir.coords.len(), n_nodes);
        assert!(
            (mir.coords[0][0] - (-0.3 + 35.0 * 0.02)).abs() < 0.05,
            "Dragged node 0 x coordinate should track final drag target, got {:?}",
            mir.coords[0]
        );
    }
}
