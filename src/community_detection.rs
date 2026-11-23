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

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use crate::models::Discipline;

/// Partitioning lens selectable across network cartography and community comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartitionScheme {
    FixedDiscipline,
    LouvainModularity,
    InfomapFlow,
}

impl PartitionScheme {
    pub fn label(self) -> &'static str {
        match self {
            PartitionScheme::FixedDiscipline => "Fixed Discipline Labels",
            PartitionScheme::LouvainModularity => "Louvain Modularity Q",
            PartitionScheme::InfomapFlow => "Infomap Flow Compression",
        }
    }
}

/// Distinct 12-color categorical palette for Louvain & Infomap communities.
pub const COMMUNITY_PALETTE: [[u8; 3]; 12] = [
    [56, 189, 248],  // sky
    [52, 211, 153],  // emerald
    [251, 146, 60],  // orange
    [167, 139, 250], // violet
    [244, 63, 94],   // rose
    [250, 204, 21],  // amber
    [45, 212, 191],  // teal
    [232, 121, 249], // fuchsia
    [96, 165, 250],  // blue
    [163, 230, 53],  // lime
    [251, 113, 133], // pink
    [148, 163, 184], // slate
];

pub fn community_rgb(comm_id: usize) -> [u8; 3] {
    COMMUNITY_PALETTE[comm_id % COMMUNITY_PALETTE.len()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityClusterSummary {
    pub cluster_id: usize,
    pub code: String,
    pub size: usize,
    pub internal_edges: usize,
    pub cut_edges: usize,
    pub flow_mass: f64,
    pub dominant_disc: Discipline,
    pub dominant_purity: f64,
    pub disc_breakdown: [usize; 6],
    pub top_member_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionAlignment {
    pub label_a: String,
    pub label_b: String,
    pub mi_bits: f64,
    pub nmi: f64,
    pub vi_bits: f64,
    pub adjusted_rand_index: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LouvainResult {
    pub assignments: Vec<usize>,
    pub num_communities: usize,
    pub modularity_q: f64,
    pub resolution_gamma: f64,
    pub hierarchy_passes: usize,
    pub clusters: Vec<CommunityClusterSummary>,
    pub contingency_disc_by_comm: Vec<[usize; 6]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfomapResult {
    pub assignments: Vec<usize>,
    pub num_modules: usize,
    pub one_level_codelength_bits: f64,
    pub codelength_bits: f64,
    pub relative_compression_pct: f64,
    pub index_codelength_bits: f64,
    pub module_codelength_bits: f64,
    pub teleport_tau: f64,
    pub stationary_flow: Vec<f64>,
    pub clusters: Vec<CommunityClusterSummary>,
    pub contingency_disc_by_mod: Vec<[usize; 6]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityReport {
    pub louvain: LouvainResult,
    pub infomap: InfomapResult,
    pub disc_vs_louvain: PartitionAlignment,
    pub disc_vs_infomap: PartitionAlignment,
    pub louvain_vs_infomap: PartitionAlignment,
    pub louvain_part_p: Vec<f64>,
    pub louvain_within_z: Vec<f64>,
    pub infomap_part_p: Vec<f64>,
    pub infomap_within_z: Vec<f64>,
    pub reclassified_count_louvain: usize,
    pub reclassified_count_infomap: usize,
}

#[inline]
fn plogp(x: f64) -> f64 {
    if x > 1e-16 {
        x * x.log2()
    } else {
        0.0
    }
}

// ============================================================================
// 1. MULTI-LEVEL LOUVAIN MODULARITY Q(gamma) OPTIMIZATION (Blondel et al. 2008)
// ============================================================================

#[derive(Clone)]
struct WeightedGraph {
    n: usize,
    adj: Vec<Vec<(usize, f64)>>, // off-diagonal symmetric neighbors (v != u, weight)
    self_loop_2x: Vec<f64>,      // 2x internal weight accumulated inside supernode
    deg: Vec<f64>,               // self_loop_2x[u] + sum_{v != u} w(u, v)
    two_m: f64,
}

impl WeightedGraph {
    fn from_undirected_adj(undir_adj: &[Vec<usize>]) -> Self {
        let n = undir_adj.len();
        let mut adj = vec![Vec::new(); n];
        let mut deg = vec![0.0f64; n];
        let mut two_m = 0.0f64;

        for (u, nbrs) in undir_adj.iter().enumerate() {
            for &v in nbrs {
                if u != v {
                    adj[u].push((v, 1.0));
                    deg[u] += 1.0;
                    two_m += 1.0;
                }
            }
        }
        Self {
            n,
            adj,
            self_loop_2x: vec![0.0; n],
            deg,
            two_m,
        }
    }

    fn compute_modularity(&self, comm: &[usize], gamma: f64) -> f64 {
        if self.n == 0 || self.two_m <= 1e-12 {
            return 0.0;
        }
        let max_c = comm.iter().copied().max().unwrap_or(0) + 1;
        let mut internal_2x = vec![0.0f64; max_c];
        let mut tot = vec![0.0f64; max_c];

        for u in 0..self.n {
            let cu = comm[u];
            tot[cu] += self.deg[u];
            internal_2x[cu] += self.self_loop_2x[u];
            for &(v, w) in &self.adj[u] {
                if comm[v] == cu {
                    internal_2x[cu] += w;
                }
            }
        }

        let mut q = 0.0;
        for c in 0..max_c {
            if tot[c] > 0.0 {
                q += (internal_2x[c] / self.two_m)
                    - gamma * (tot[c] / self.two_m).powi(2);
            }
        }
        q
    }

    /// Phase 1 of Blondel et al. (2008): greedy local vertex moving until convergence.
    fn local_moving_pass(&self, comm: &mut [usize], gamma: f64) -> bool {
        if self.n <= 1 || self.two_m <= 1e-12 {
            return false;
        }
        let max_c = comm.iter().copied().max().unwrap_or(0).max(self.n) + 1;
        let mut tot = vec![0.0f64; max_c];
        for u in 0..self.n {
            tot[comm[u]] += self.deg[u];
        }

        let mut any_moved = false;
        let mut nbr_comm_weights: HashMap<usize, f64> = HashMap::new();

        for _round in 0..32 {
            let mut moved_this_round = false;
            for u in 0..self.n {
                let c_old = comm[u];
                let k_u = self.deg[u];
                if k_u <= 0.0 {
                    continue;
                }

                nbr_comm_weights.clear();
                nbr_comm_weights.insert(c_old, 0.0);
                for &(v, w) in &self.adj[u] {
                    *nbr_comm_weights.entry(comm[v]).or_insert(0.0) += w;
                }

                // Temporarily remove u from c_old
                tot[c_old] -= k_u;
                let w_old = nbr_comm_weights.get(&c_old).copied().unwrap_or(0.0);
                let delta_leave = -(2.0 * w_old) / self.two_m
                    + gamma * (2.0 * tot[c_old] * k_u) / (self.two_m * self.two_m);

                let mut best_c = c_old;
                let mut best_gain = 0.0f64;

                // Deterministic candidate evaluation ordered by community index
                let mut cand_list: Vec<(usize, f64)> =
                    nbr_comm_weights.iter().map(|(&c, &w)| (c, w)).collect();
                cand_list.sort_unstable_by_key(|&(c, _)| c);

                for (c_cand, w_cand) in cand_list {
                    let delta_join = (2.0 * w_cand) / self.two_m
                        - gamma * (2.0 * tot[c_cand] * k_u) / (self.two_m * self.two_m);
                    let net_gain = delta_leave + delta_join;
                    if net_gain > best_gain + 1e-11 {
                        best_gain = net_gain;
                        best_c = c_cand;
                    }
                }

                comm[u] = best_c;
                tot[best_c] += k_u;
                if best_c != c_old {
                    moved_this_round = true;
                    any_moved = true;
                }
            }
            if !moved_this_round {
                break;
            }
        }
        any_moved
    }

    /// Phase 2 of Blondel et al. (2008): contract communities into weighted supernodes.
    fn contract(&self, comm: &[usize]) -> (WeightedGraph, Vec<usize>) {
        let mut remap: HashMap<usize, usize> = HashMap::new();
        let mut node_to_super = vec![0usize; self.n];
        for (u, &c) in comm.iter().enumerate() {
            let next_id = remap.len();
            let sid = *remap.entry(c).or_insert(next_id);
            node_to_super[u] = sid;
        }
        let k = remap.len();
        let mut super_self_2x = vec![0.0f64; k];
        let mut super_pair_weights: Vec<HashMap<usize, f64>> = vec![HashMap::new(); k];
        let mut super_deg = vec![0.0f64; k];

        for u in 0..self.n {
            let su = node_to_super[u];
            super_self_2x[su] += self.self_loop_2x[u];
            super_deg[su] += self.deg[u];
            for &(v, w) in &self.adj[u] {
                let sv = node_to_super[v];
                if su == sv {
                    super_self_2x[su] += w;
                } else {
                    *super_pair_weights[su].entry(sv).or_insert(0.0) += w;
                }
            }
        }

        let mut super_adj = vec![Vec::new(); k];
        for su in 0..k {
            let mut list: Vec<(usize, f64)> = super_pair_weights[su]
                .iter()
                .map(|(&sv, &w)| (sv, w))
                .collect();
            list.sort_unstable_by_key(|&(sv, _)| sv);
            super_adj[su] = list;
        }

        (
            WeightedGraph {
                n: k,
                adj: super_adj,
                self_loop_2x: super_self_2x,
                deg: super_deg,
                two_m: self.two_m,
            },
            node_to_super,
        )
    }
}

pub fn run_louvain_modularity(
    undir_adj: &[Vec<usize>],
    disciplines: &[Discipline],
    paper_ids: &[String],
    cb_scores: &[f64],
    resolution_gamma: f64,
) -> LouvainResult {
    let n = undir_adj.len();
    let gamma = resolution_gamma.clamp(0.1, 5.0);
    if n == 0 {
        return LouvainResult {
            assignments: Vec::new(),
            num_communities: 0,
            modularity_q: 0.0,
            resolution_gamma: gamma,
            hierarchy_passes: 0,
            clusters: Vec::new(),
            contingency_disc_by_comm: Vec::new(),
        };
    }

    let base_graph = WeightedGraph::from_undirected_adj(undir_adj);
    let mut current_graph = base_graph.clone();
    let mut orig_to_current: Vec<usize> = (0..n).collect();
    let mut passes = 0usize;

    for _level in 0..12 {
        let mut level_comm: Vec<usize> = (0..current_graph.n).collect();
        let moved = current_graph.local_moving_pass(&mut level_comm, gamma);
        passes += 1;
        if !moved {
            break;
        }
        let (next_graph, map_to_super) = current_graph.contract(&level_comm);
        for u in 0..n {
            orig_to_current[u] = map_to_super[orig_to_current[u]];
        }
        if next_graph.n == current_graph.n {
            break;
        }
        current_graph = next_graph;
    }

    // Final fine-grained vertex-level refinement on the uncontracted base graph
    base_graph.local_moving_pass(&mut orig_to_current, gamma);

    let (assignments, k_final) = canonicalize_by_size(&orig_to_current, None);
    let modularity_q = base_graph.compute_modularity(&assignments, gamma);
    let dummy_flow = vec![1.0 / (n as f64); n];
    let (clusters, contingency) = summarize_clusters(
        &assignments,
        k_final,
        "L",
        undir_adj,
        disciplines,
        paper_ids,
        cb_scores,
        &dummy_flow,
    );

    LouvainResult {
        assignments,
        num_communities: k_final,
        modularity_q,
        resolution_gamma: gamma,
        hierarchy_passes: passes,
        clusters,
        contingency_disc_by_comm: contingency,
    }
}

// ============================================================================
// 2. RANDOM-WALK INFOMAP FLOW COMPRESSION (Rosvall & Bergstrom 2008)
// ============================================================================

/// Builds the effective Infomap random-walk transition graph:
/// directed citation edges (`1.0`) supplemented with `0.25` back-flow strictly
/// from reverse `in_adj` directed edges.
pub fn build_infomap_rw_transitions(out_adj: &[Vec<usize>]) -> (Vec<Vec<(usize, f64)>>, Vec<f64>) {
    let n = out_adj.len();
    let mut in_adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (u, nbrs) in out_adj.iter().enumerate() {
        for &v in nbrs {
            if u != v && v < n {
                in_adj[v].push(u);
            }
        }
    }

    let mut rw_out: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    let mut rw_out_sum = vec![0.0f64; n];
    for u in 0..n {
        let mut weights: HashMap<usize, f64> = HashMap::new();
        for &v in &out_adj[u] {
            if u != v && v < n {
                *weights.entry(v).or_insert(0.0) += 1.0;
            }
        }
        for &v in &in_adj[u] {
            if u != v {
                *weights.entry(v).or_insert(0.0) += 0.25;
            }
        }
        let mut list: Vec<(usize, f64)> = weights.into_iter().collect();
        list.sort_unstable_by_key(|&(v, _)| v);
        rw_out_sum[u] = list.iter().map(|&(_, w)| w).sum();
        rw_out[u] = list;
    }
    (rw_out, rw_out_sum)
}

/// Computes ergodic PageRank visit frequencies and optimizes the two-level
/// Rosvall-Bergstrom Map Equation description length L(M) in bits per step.
pub fn run_infomap_compression(
    out_adj: &[Vec<usize>],
    undir_adj: &[Vec<usize>],
    disciplines: &[Discipline],
    paper_ids: &[String],
    cb_scores: &[f64],
    teleport_tau: f64,
) -> InfomapResult {
    let n = out_adj.len();
    let tau = teleport_tau.clamp(0.01, 0.85);
    if n == 0 {
        return InfomapResult {
            assignments: Vec::new(),
            num_modules: 0,
            one_level_codelength_bits: 0.0,
            codelength_bits: 0.0,
            relative_compression_pct: 0.0,
            index_codelength_bits: 0.0,
            module_codelength_bits: 0.0,
            teleport_tau: tau,
            stationary_flow: Vec::new(),
            clusters: Vec::new(),
            contingency_disc_by_mod: Vec::new(),
        };
    }

    let (rw_out, rw_out_sum) = build_infomap_rw_transitions(out_adj);

    // Step 1: Power iteration for exact ergodic stationary distribution p_i
    let inv_n = 1.0 / (n as f64);
    let mut p = vec![inv_n; n];
    let mut next_p = vec![0.0f64; n];

    for _iter in 0..250 {
        let mut dangling_mass = 0.0f64;
        for u in 0..n {
            if rw_out_sum[u] <= 1e-12 {
                dangling_mass += p[u];
            }
        }
        let base_teleport = (tau + (1.0 - tau) * dangling_mass) * inv_n;
        next_p.fill(base_teleport);

        for u in 0..n {
            let denom = rw_out_sum[u];
            if denom > 1e-12 {
                let scale = (1.0 - tau) * p[u] / denom;
                for &(v, w) in &rw_out[u] {
                    next_p[v] += scale * w;
                }
            }
        }

        let diff: f64 = p.iter().zip(&next_p).map(|(a, b)| (a - b).abs()).sum();
        p.copy_from_slice(&next_p);
        if diff < 1e-13 {
            break;
        }
    }

    // Normalize p strictly
    let p_sum: f64 = p.iter().sum::<f64>().max(1e-15);
    for val in &mut p {
        *val /= p_sum;
    }

    // Directed step transition flows f(u -> v) along actual graph edges
    let mut flow_out: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for u in 0..n {
        let denom = rw_out_sum[u];
        if denom > 1e-12 {
            flow_out[u] = rw_out[u]
                .iter()
                .map(|&(v, w)| (v, (1.0 - tau) * p[u] * (w / denom)))
                .collect();
        }
    }

    let node_entropy_term: f64 = p.iter().map(|&pi| plogp(pi)).sum(); // negative
    let one_level_codelength = (-node_entropy_term).max(0.0);

    // Helper: exact two-level Map Equation L(M) = L_index + L_modules
    let eval_map_equation = |mod_of: &[usize]| -> (f64, f64, f64) {
        let k_max = mod_of.iter().copied().max().unwrap_or(0) + 1;
        let mut p_mod = vec![0.0f64; k_max];
        let mut p_dangling = vec![0.0f64; k_max];
        let mut count_mod = vec![0usize; k_max];
        let mut link_exit = vec![0.0f64; k_max];

        for u in 0..n {
            let m = mod_of[u];
            p_mod[m] += p[u];
            count_mod[m] += 1;
            if rw_out_sum[u] <= 1e-12 {
                p_dangling[m] += p[u];
            }
            for &(v, f_uv) in &flow_out[u] {
                if mod_of[v] != m {
                    link_exit[m] += f_uv;
                }
            }
        }

        let mut q_out = vec![0.0f64; k_max];
        let mut q_exit_sum = 0.0f64;
        for m in 0..k_max {
            if count_mod[m] == 0 {
                continue;
            }
            let outside_frac = ((n - count_mod[m]) as f64) * inv_n;
            let tele_exit =
                tau * outside_frac * p_mod[m] + (1.0 - tau) * outside_frac * p_dangling[m];
            q_out[m] = tele_exit + link_exit[m];
            q_exit_sum += q_out[m];
        }

        // Index codebook L_index = q_exit * H(Q) = plogp(q_exit) - sum_m plogp(q_m)
        let sum_plogp_q: f64 = q_out.iter().map(|&qm| plogp(qm)).sum();
        let l_index = if count_mod.iter().filter(|&&c| c > 0).count() <= 1 {
            0.0
        } else {
            (plogp(q_exit_sum) - sum_plogp_q).max(0.0)
        };

        // Module codebooks sum_m p_circ_m * H(P_m) = sum_m plogp(q_m + p_m) - sum_m plogp(q_m) - sum_i plogp(p_i)
        let sum_plogp_pcirc: f64 = (0..k_max)
            .filter(|&m| count_mod[m] > 0)
            .map(|m| plogp(q_out[m] + p_mod[m]))
            .sum();
        let l_modules = (sum_plogp_pcirc - sum_plogp_q - node_entropy_term).max(0.0);
        let l_total = l_index + l_modules;
        (l_total, l_index, l_modules)
    };

    if n > 500 {
        // Fast O(E) multi-scale Infomap flow refinement for 500 .. 150,000+ nodes:
        // Seed initial modules from Louvain super-clusters, then optimize exact two-level Map Equation
        // L(M) via O(deg(u)) incremental module exit/stay flow accumulators.
        let fast_louv = run_louvain_modularity(undir_adj, disciplines, paper_ids, cb_scores, 1.0);
        let mut mod_of = fast_louv.assignments;
        let k_cap = fast_louv.num_communities.max(1) + 1;

        let mut flow_in: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        for u in 0..n {
            for &(v, f_uv) in &flow_out[u] {
                flow_in[v].push((u, f_uv));
            }
        }

        let mut p_mod = vec![0.0f64; k_cap];
        let mut p_dangling = vec![0.0f64; k_cap];
        let mut count_mod = vec![0usize; k_cap];
        let mut link_exit = vec![0.0f64; k_cap];
        for u in 0..n {
            let m = mod_of[u];
            p_mod[m] += p[u];
            count_mod[m] += 1;
            if rw_out_sum[u] <= 1e-12 {
                p_dangling[m] += p[u];
            }
            for &(v, f_uv) in &flow_out[u] {
                if mod_of[v] != m {
                    link_exit[m] += f_uv;
                }
            }
        }

        let compute_q_m = |cnt: usize, pm: f64, pd: f64, lex: f64| -> f64 {
            if cnt == 0 {
                0.0
            } else {
                let out_frac = ((n - cnt) as f64) * inv_n;
                out_frac * (tau * pm + (1.0 - tau) * pd) + lex
            }
        };

        let mut q_out = vec![0.0f64; k_cap];
        let mut q_exit_sum = 0.0f64;
        let mut sum_plogp_q = 0.0f64;
        let mut sum_plogp_pcirc = 0.0f64;
        for m in 0..k_cap {
            if count_mod[m] > 0 {
                q_out[m] = compute_q_m(count_mod[m], p_mod[m], p_dangling[m], link_exit[m]);
                q_exit_sum += q_out[m];
                sum_plogp_q += plogp(q_out[m]);
                sum_plogp_pcirc += plogp(q_out[m] + p_mod[m]);
            }
        }

        let _ = k_cap;
        let mut nbr_flow_to: HashMap<usize, (f64, f64)> = HashMap::new();
        for _pass in 0..6 {
            let mut moved = 0usize;
            for u in 0..n {
                let old_m = mod_of[u];
                if count_mod[old_m] <= 1 && undir_adj[u].is_empty() {
                    continue;
                }
                nbr_flow_to.clear();
                let mut f_u_out_total = 0.0f64;
                for &(v, f_uv) in &flow_out[u] {
                    if v != u {
                        f_u_out_total += f_uv;
                        nbr_flow_to.entry(mod_of[v]).or_insert((0.0, 0.0)).0 += f_uv;
                    }
                }
                for &(w, f_wu) in &flow_in[u] {
                    if w != u {
                        nbr_flow_to.entry(mod_of[w]).or_insert((0.0, 0.0)).1 += f_wu;
                    }
                }

                let (f_u_to_old, f_old_to_u) = nbr_flow_to.get(&old_m).copied().unwrap_or((0.0, 0.0));
                let is_dang = if rw_out_sum[u] <= 1e-12 { p[u] } else { 0.0 };

                let new_cnt_old = count_mod[old_m] - 1;
                let new_pm_old = (p_mod[old_m] - p[u]).max(0.0);
                let new_pd_old = (p_dangling[old_m] - is_dang).max(0.0);
                let new_lex_old = (link_exit[old_m] - (f_u_out_total - f_u_to_old) + f_old_to_u).max(0.0);
                let new_q_old = compute_q_m(new_cnt_old, new_pm_old, new_pd_old, new_lex_old);

                let cur_l_total = (plogp(q_exit_sum) - sum_plogp_q).max(0.0)
                    + (sum_plogp_pcirc - sum_plogp_q - node_entropy_term).max(0.0);
                let mut best_m = old_m;
                let mut best_l = cur_l_total;
                let mut best_cand_tuple = (0usize, 0.0, 0.0, 0.0, 0.0);

                for (&cand_m, &(f_u_to_c, f_c_to_u)) in &nbr_flow_to {
                    if cand_m == old_m {
                        continue;
                    }
                    let new_cnt_c = count_mod[cand_m] + 1;
                    let new_pm_c = p_mod[cand_m] + p[u];
                    let new_pd_c = p_dangling[cand_m] + is_dang;
                    let new_lex_c =
                        (link_exit[cand_m] + (f_u_out_total - f_u_to_c) - f_c_to_u).max(0.0);
                    let new_q_c = compute_q_m(new_cnt_c, new_pm_c, new_pd_c, new_lex_c);

                    let cand_q_sum = (q_exit_sum - q_out[old_m] - q_out[cand_m] + new_q_old + new_q_c).max(0.0);
                    let cand_sum_plogp_q = sum_plogp_q - plogp(q_out[old_m]) - plogp(q_out[cand_m])
                        + plogp(new_q_old)
                        + plogp(new_q_c);
                    let cand_sum_pcirc = sum_plogp_pcirc
                        - plogp(q_out[old_m] + p_mod[old_m])
                        - plogp(q_out[cand_m] + p_mod[cand_m])
                        + if new_cnt_old > 0 { plogp(new_q_old + new_pm_old) } else { 0.0 }
                        + plogp(new_q_c + new_pm_c);

                    let l_try = (plogp(cand_q_sum) - cand_sum_plogp_q).max(0.0)
                        + (cand_sum_pcirc - cand_sum_plogp_q - node_entropy_term).max(0.0);
                    if l_try < best_l - 1e-9 {
                        best_l = l_try;
                        best_m = cand_m;
                        best_cand_tuple = (new_cnt_c, new_pm_c, new_pd_c, new_lex_c, new_q_c);
                    }
                }

                if best_m != old_m {
                    let (nc_c, npm_c, npd_c, nlex_c, nq_c) = best_cand_tuple;
                    q_exit_sum = (q_exit_sum - q_out[old_m] - q_out[best_m] + new_q_old + nq_c).max(0.0);
                    sum_plogp_q = sum_plogp_q - plogp(q_out[old_m]) - plogp(q_out[best_m])
                        + plogp(new_q_old)
                        + plogp(nq_c);
                    sum_plogp_pcirc = sum_plogp_pcirc
                        - plogp(q_out[old_m] + p_mod[old_m])
                        - plogp(q_out[best_m] + p_mod[best_m])
                        + if new_cnt_old > 0 { plogp(new_q_old + new_pm_old) } else { 0.0 }
                        + plogp(nq_c + npm_c);

                    count_mod[old_m] = new_cnt_old;
                    p_mod[old_m] = new_pm_old;
                    p_dangling[old_m] = new_pd_old;
                    link_exit[old_m] = new_lex_old;
                    q_out[old_m] = new_q_old;

                    count_mod[best_m] = nc_c;
                    p_mod[best_m] = npm_c;
                    p_dangling[best_m] = npd_c;
                    link_exit[best_m] = nlex_c;
                    q_out[best_m] = nq_c;

                    mod_of[u] = best_m;
                    moved += 1;
                }
            }
            if moved == 0 {
                break;
            }
        }

        // fast_large_graph_infomap finalize
        let (assignments, k_final) = canonicalize_by_size(&mod_of, Some(&p));
        let (codelength_bits, index_codelength_bits, module_codelength_bits) =
            eval_map_equation(&assignments);
        let relative_compression_pct = if one_level_codelength > 1e-12 {
            ((one_level_codelength - codelength_bits) / one_level_codelength) * 100.0
        } else {
            0.0
        };
        let (clusters, contingency) = summarize_clusters(
            &assignments,
            k_final,
            "M",
            undir_adj,
            disciplines,
            paper_ids,
            cb_scores,
            &p,
        );
        return InfomapResult {
            assignments,
            num_modules: k_final,
            one_level_codelength_bits: one_level_codelength,
            codelength_bits,
            relative_compression_pct,
            index_codelength_bits,
            module_codelength_bits,
            teleport_tau: tau,
            stationary_flow: p,
            clusters,
            contingency_disc_by_mod: contingency,
        };
    }

    // Optimization Stage A: Greedy Agglomerative Module Merging along connected edges
    let mut mod_of: Vec<usize> = (0..n).collect();
    let (mut cur_l, _, _) = eval_map_equation(&mod_of);

    for _merge_step in 0..n {
        let mut candidate_pairs: Vec<(usize, usize)> = Vec::new();
        for u in 0..n {
            let mu = mod_of[u];
            for &v in &undir_adj[u] {
                let mv = mod_of[v];
                if mu < mv {
                    candidate_pairs.push((mu, mv));
                }
            }
        }
        candidate_pairs.sort_unstable();
        candidate_pairs.dedup();
        if candidate_pairs.is_empty() {
            break;
        }

        let mut best_pair: Option<(usize, usize)> = None;
        let mut best_l = cur_l;
        let mut trial = mod_of.clone();

        for (ma, mb) in candidate_pairs {
            trial.copy_from_slice(&mod_of);
            for m in &mut trial {
                if *m == mb {
                    *m = ma;
                }
            }
            let (l_cand, _, _) = eval_map_equation(&trial);
            if l_cand < best_l - 1e-10 {
                best_l = l_cand;
                best_pair = Some((ma, mb));
            }
        }

        if let Some((ma, mb)) = best_pair {
            for m in &mut mod_of {
                if *m == mb {
                    *m = ma;
                }
            }
            cur_l = best_l;
        } else {
            break;
        }
    }

    // Optimization Stage B: Fine-Grained Single-Node Local Moving Refinement
    for _round in 0..16 {
        let mut improved = false;
        for u in 0..n {
            let old_m = mod_of[u];
            let mut nbr_mods: Vec<usize> = undir_adj[u].iter().map(|&v| mod_of[v]).collect();
            nbr_mods.sort_unstable();
            nbr_mods.dedup();

            let mut best_m = old_m;
            let mut best_l = cur_l;
            for cand_m in nbr_mods {
                if cand_m == old_m {
                    continue;
                }
                mod_of[u] = cand_m;
                let (l_try, _, _) = eval_map_equation(&mod_of);
                if l_try < best_l - 1e-10 {
                    best_l = l_try;
                    best_m = cand_m;
                }
            }
            mod_of[u] = best_m;
            if best_m != old_m {
                cur_l = best_l;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }

    let (assignments, k_final) = canonicalize_by_size(&mod_of, Some(&p));
    let (codelength_bits, index_codelength_bits, module_codelength_bits) =
        eval_map_equation(&assignments);
    let relative_compression_pct = if one_level_codelength > 1e-12 {
        ((one_level_codelength - codelength_bits) / one_level_codelength) * 100.0
    } else {
        0.0
    };

    let (clusters, contingency) = summarize_clusters(
        &assignments,
        k_final,
        "M",
        undir_adj,
        disciplines,
        paper_ids,
        cb_scores,
        &p,
    );

    InfomapResult {
        assignments,
        num_modules: k_final,
        one_level_codelength_bits: one_level_codelength,
        codelength_bits,
        relative_compression_pct,
        index_codelength_bits,
        module_codelength_bits,
        teleport_tau: tau,
        stationary_flow: p,
        clusters,
        contingency_disc_by_mod: contingency,
    }
}

// ============================================================================
// 3. PARTITION ALIGNMENT (NMI, VI, ADJUSTED RAND INDEX) & ROLE CARTOGRAPHY
// ============================================================================

pub fn build_full_community_report(
    out_adj: &[Vec<usize>],
    undir_adj: &[Vec<usize>],
    disciplines: &[Discipline],
    paper_ids: &[String],
    cb_scores: &[f64],
    louvain_gamma: f64,
    infomap_tau: f64,
) -> CommunityReport {
    let louvain = run_louvain_modularity(
        undir_adj,
        disciplines,
        paper_ids,
        cb_scores,
        louvain_gamma,
    );
    let infomap = run_infomap_compression(
        out_adj,
        undir_adj,
        disciplines,
        paper_ids,
        cb_scores,
        infomap_tau,
    );

    let disc_labels: Vec<usize> = disciplines.iter().map(|d| d.index()).collect();
    let disc_vs_louvain = compare_partitions(
        "Fixed Disciplines",
        &disc_labels,
        "Louvain Q",
        &louvain.assignments,
    );
    let disc_vs_infomap = compare_partitions(
        "Fixed Disciplines",
        &disc_labels,
        "Infomap Flow",
        &infomap.assignments,
    );
    let louvain_vs_infomap = compare_partitions(
        "Louvain Q",
        &louvain.assignments,
        "Infomap Flow",
        &infomap.assignments,
    );

    let (louvain_part_p, louvain_within_z) = compute_partition_cartography(
        undir_adj,
        &louvain.assignments,
        louvain.num_communities,
    );
    let (infomap_part_p, infomap_within_z) = compute_partition_cartography(
        undir_adj,
        &infomap.assignments,
        infomap.num_modules,
    );

    let reclassified_count_louvain = disciplines
        .iter()
        .enumerate()
        .filter(|&(i, &d)| {
            let cid = louvain.assignments[i];
            louvain
                .clusters
                .get(cid)
                .map(|c| c.dominant_disc != d)
                .unwrap_or(false)
        })
        .count();

    let reclassified_count_infomap = disciplines
        .iter()
        .enumerate()
        .filter(|&(i, &d)| {
            let mid = infomap.assignments[i];
            infomap
                .clusters
                .get(mid)
                .map(|c| c.dominant_disc != d)
                .unwrap_or(false)
        })
        .count();

    CommunityReport {
        louvain,
        infomap,
        disc_vs_louvain,
        disc_vs_infomap,
        louvain_vs_infomap,
        louvain_part_p,
        louvain_within_z,
        infomap_part_p,
        infomap_within_z,
        reclassified_count_louvain,
        reclassified_count_infomap,
    }
}

pub fn compare_partitions(
    label_a: &str,
    part_a: &[usize],
    label_b: &str,
    part_b: &[usize],
) -> PartitionAlignment {
    let n = part_a.len().min(part_b.len());
    if n == 0 {
        return PartitionAlignment {
            label_a: label_a.to_string(),
            label_b: label_b.to_string(),
            mi_bits: 0.0,
            nmi: 1.0,
            vi_bits: 0.0,
            adjusted_rand_index: 1.0,
        };
    }

    let ka = part_a[..n].iter().copied().max().unwrap_or(0) + 1;
    let kb = part_b[..n].iter().copied().max().unwrap_or(0) + 1;
    let mut counts = vec![vec![0usize; kb]; ka];
    let mut row_sums = vec![0usize; ka];
    let mut col_sums = vec![0usize; kb];

    for i in 0..n {
        let a = part_a[i];
        let b = part_b[i];
        counts[a][b] += 1;
        row_sums[a] += 1;
        col_sums[b] += 1;
    }

    let nf = n as f64;
    let mut ha = 0.0f64;
    for &r in &row_sums {
        if r > 0 {
            let p = (r as f64) / nf;
            ha -= p * p.log2();
        }
    }
    let mut hb = 0.0f64;
    for &c in &col_sums {
        if c > 0 {
            let p = (c as f64) / nf;
            hb -= p * p.log2();
        }
    }

    let mut mi = 0.0f64;
    for a in 0..ka {
        for b in 0..kb {
            let nij = counts[a][b];
            if nij > 0 {
                let pij = (nij as f64) / nf;
                let pa = (row_sums[a] as f64) / nf;
                let pb = (col_sums[b] as f64) / nf;
                mi += pij * (pij / (pa * pb)).log2();
            }
        }
    }
    mi = mi.max(0.0);
    let nmi = if (ha + hb) > 1e-12 {
        ((2.0 * mi) / (ha + hb)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let vi = (ha + hb - 2.0 * mi).max(0.0);

    // Hubert-Arabie Adjusted Rand Index (ARI)
    let comb2 = |x: usize| -> f64 {
        if x < 2 {
            0.0
        } else {
            ((x as f64) * ((x - 1) as f64)) * 0.5
        }
    };
    let total_pairs = comb2(n);
    let sum_nij = (0..ka)
        .flat_map(|a| (0..kb).map(move |b| (a, b)))
        .map(|(a, b)| comb2(counts[a][b]))
        .sum::<f64>();
    let sum_a = row_sums.iter().map(|&r| comb2(r)).sum::<f64>();
    let sum_b = col_sums.iter().map(|&c| comb2(c)).sum::<f64>();

    let ari = if total_pairs > 0.0 {
        let expected = (sum_a * sum_b) / total_pairs;
        let max_idx = 0.5 * (sum_a + sum_b);
        let denom = max_idx - expected;
        if denom.abs() > 1e-12 {
            (sum_nij - expected) / denom
        } else if (sum_nij - expected).abs() <= 1e-12 {
            1.0
        } else {
            0.0
        }
    } else {
        1.0
    };

    PartitionAlignment {
        label_a: label_a.to_string(),
        label_b: label_b.to_string(),
        mi_bits: mi,
        nmi,
        vi_bits: vi,
        adjusted_rand_index: ari,
    }
}

fn compute_partition_cartography(
    undir_adj: &[Vec<usize>],
    comm: &[usize],
    num_c: usize,
) -> (Vec<f64>, Vec<f64>) {
    let n = undir_adj.len();
    let k_c = num_c.max(1);
    let mut within_deg = vec![0.0f64; n];
    let mut part_p = vec![0.0f64; n];

    for i in 0..n {
        let nbrs = &undir_adj[i];
        let ki = nbrs.len();
        if ki == 0 {
            continue;
        }
        let mut counts = vec![0usize; k_c];
        for &j in nbrs {
            if comm[j] < k_c {
                counts[comm[j]] += 1;
            }
        }
        if comm[i] < k_c {
            within_deg[i] = counts[comm[i]] as f64;
        }
        let sum_sq: f64 = counts
            .iter()
            .map(|&c| {
                let f = (c as f64) / (ki as f64);
                f * f
            })
            .sum();
        part_p[i] = (1.0 - sum_sq).max(0.0);
    }

    let mut within_z = vec![0.0f64; n];
    for c in 0..k_c {
        let members: Vec<usize> = (0..n).filter(|&i| comm[i] == c).collect();
        if members.is_empty() {
            continue;
        }
        let mean = members.iter().map(|&i| within_deg[i]).sum::<f64>() / (members.len() as f64);
        let var = members
            .iter()
            .map(|&i| (within_deg[i] - mean).powi(2))
            .sum::<f64>()
            / (members.len() as f64);
        let std = var.sqrt();
        for &i in &members {
            within_z[i] = if std > 1e-6 {
                (within_deg[i] - mean) / std
            } else {
                0.0
            };
        }
    }

    (part_p, within_z)
}

fn canonicalize_by_size(raw: &[usize], weights: Option<&[f64]>) -> (Vec<usize>, usize) {
    let n = raw.len();
    let mut stats: HashMap<usize, (usize, f64, usize)> = HashMap::new(); // id -> (count, weight_sum, first_idx)
    for (i, &cid) in raw.iter().enumerate() {
        let w = weights.and_then(|ws| ws.get(i)).copied().unwrap_or(1.0);
        let entry = stats.entry(cid).or_insert((0, 0.0, i));
        entry.0 += 1;
        entry.1 += w;
    }

    let mut ordered: Vec<(usize, (usize, f64, usize))> = stats.into_iter().collect();
    ordered.sort_by(|a, b| {
        b.1 .1
            .partial_cmp(&a.1 .1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1 .0.cmp(&a.1 .0))
            .then_with(|| a.1 .2.cmp(&b.1 .2))
    });

    let mut remap: HashMap<usize, usize> = HashMap::with_capacity(ordered.len());
    for (new_id, (old_id, _)) in ordered.iter().enumerate() {
        remap.insert(*old_id, new_id);
    }

    let out: Vec<usize> = (0..n).map(|i| remap[&raw[i]]).collect();
    (out, ordered.len())
}

#[allow(clippy::too_many_arguments)]
fn summarize_clusters(
    assignments: &[usize],
    k: usize,
    prefix: &str,
    undir_adj: &[Vec<usize>],
    disciplines: &[Discipline],
    paper_ids: &[String],
    cb_scores: &[f64],
    flow: &[f64],
) -> (Vec<CommunityClusterSummary>, Vec<[usize; 6]>) {
    let n = assignments.len();
    let mut contingency = vec![[0usize; 6]; k];
    let mut sizes = vec![0usize; k];
    let mut internal_2x = vec![0usize; k];
    let mut cut_edges = vec![0usize; k];
    let mut flow_mass = vec![0.0f64; k];
    let mut best_member: Vec<(f64, String)> = vec![(-1.0, String::new()); k];

    for i in 0..n {
        let c = assignments[i];
        sizes[c] += 1;
        contingency[c][disciplines[i].index()] += 1;
        flow_mass[c] += flow.get(i).copied().unwrap_or(0.0);
        let score = cb_scores.get(i).copied().unwrap_or(0.0);
        if score > best_member[c].0 {
            best_member[c] = (score, paper_ids[i].clone());
        }
        for &j in &undir_adj[i] {
            if assignments[j] == c {
                internal_2x[c] += 1;
            } else {
                cut_edges[c] += 1;
            }
        }
    }

    let clusters = (0..k)
        .map(|c| {
            let mut dom_idx = 0usize;
            let mut dom_cnt = 0usize;
            for d in 0..6 {
                if contingency[c][d] > dom_cnt {
                    dom_cnt = contingency[c][d];
                    dom_idx = d;
                }
            }
            let purity = if sizes[c] > 0 {
                (dom_cnt as f64) / (sizes[c] as f64)
            } else {
                0.0
            };
            CommunityClusterSummary {
                cluster_id: c,
                code: format!("{}{}", prefix, c + 1),
                size: sizes[c],
                internal_edges: internal_2x[c] / 2,
                cut_edges: cut_edges[c],
                flow_mass: flow_mass[c],
                dominant_disc: Discipline::ALL[dom_idx],
                dominant_purity: purity,
                disc_breakdown: contingency[c],
                top_member_id: best_member[c].1.clone(),
            }
        })
        .collect();

    (clusters, contingency)
}

#[cfg(test)]
mod infomap_flow_tests {
    use super::*;

    #[test]
    fn infomap_forward_backward_transition_ratio_on_abc_is_exact_1_to_025() {
        // Directed chain: A (0) -> B (1) -> C (2)
        let out_adj = vec![vec![1], vec![2], vec![]];
        let undir_adj = vec![vec![1], vec![0, 2], vec![1]];
        let (rw_out, _) = build_infomap_rw_transitions(&out_adj);

        let w_b_to_c = rw_out[1].iter().find(|&&(v, _)| v == 2).map(|&(_, w)| w).unwrap();
        let w_b_to_a = rw_out[1].iter().find(|&&(v, _)| v == 0).map(|&(_, w)| w).unwrap();
        assert!((w_b_to_c - 1.0).abs() < 1e-12, "B->C weight={}", w_b_to_c);
        assert!((w_b_to_a - 0.25).abs() < 1e-12, "B->A back-flow={}", w_b_to_a);
        assert!((w_b_to_c / w_b_to_a - 4.0).abs() < 1e-12);

        let discs = [Discipline::INF, Discipline::INF, Discipline::INF];
        let ids = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let cb = vec![0.0, 1.0, 0.0];
        let tau = 0.15;
        let res = run_infomap_compression(&out_adj, &undir_adj, &discs, &ids, &cb, tau);
        let p = &res.stationary_flow;
        // On A -> B -> C with no dangling nodes, excess flow arriving at C vs A from B is exact 1.0 : 0.25.
        let flow_ratio = (p[2] - tau / 3.0) / (p[0] - tau / 3.0);
        assert!(
            (flow_ratio - 4.0).abs() < 1e-9,
            "Expected exact 1.0:0.25 (=4.0) forward/backward flow ratio, got {}",
            flow_ratio
        );
    }
}
