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

use std::collections::{HashMap, HashSet, VecDeque};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::community_detection::build_full_community_report;
use crate::models::{
    concept_index, AnalyticsParams, CitationEdge, CorpusAnalytics, Discipline, EdgeConduit,
    PaperMetrics, PaperRecord, WindowTopicStats,
};

pub fn compute_corpus_analytics(
    raw_papers: &[PaperRecord],
    raw_edges: &[CitationEdge],
    params: &AnalyticsParams,
) -> CorpusAnalytics {
    let horizon = params.horizon_year;
    let active_records: Vec<PaperRecord> = raw_papers
        .iter()
        .filter(|p| {
            p.year <= horizon
                && (!params.exclude_retracted || !p.retraction.status.is_retracted_or_withdrawn())
        })
        .cloned()
        .collect();
    let n = active_records.len();

    let mut id_to_idx: HashMap<String, usize> = HashMap::with_capacity(n);
    for (idx, p) in active_records.iter().enumerate() {
        id_to_idx.insert(p.id.clone(), idx);
    }

    let min_year = active_records
        .iter()
        .map(|p| p.year)
        .min()
        .unwrap_or(1948);
    let max_year = active_records
        .iter()
        .map(|p| p.year)
        .max()
        .unwrap_or(horizon)
        .max(horizon.min(2026));
    let year_span = ((horizon - min_year + 1).max(1)) as usize;

    let mut seen_edges: HashSet<(usize, usize)> = HashSet::new();
    let mut active_edges: Vec<(usize, usize)> = Vec::new();
    for e in raw_edges {
        if let (Some(&u), Some(&v)) = (id_to_idx.get(&e.source), id_to_idx.get(&e.target)) {
            if u != v && seen_edges.insert((u, v)) {
                active_edges.push((u, v));
            }
        }
    }

    let mut out_adj = vec![Vec::<usize>::new(); n];
    let mut in_adj = vec![Vec::<usize>::new(); n];
    let mut undir_sets = vec![HashSet::<usize>::new(); n];

    for &(u, v) in &active_edges {
        out_adj[u].push(v);
        in_adj[v].push(u);
        undir_sets[u].insert(v);
        undir_sets[v].insert(u);
    }
    for list in &mut in_adj {
        list.sort_unstable_by_key(|&w| (active_records[w].year, w));
    }
    let undir_adj: Vec<Vec<usize>> = undir_sets
        .iter()
        .map(|s| {
            let mut v: Vec<usize> = s.iter().copied().collect();
            v.sort_unstable();
            v
        })
        .collect();

    // 1. Rayon-Parallel Exact Brandes (2001) Vertex & Edge Betweenness Centrality
    let (cb_undir, eb_undir) = run_parallel_brandes(n, &undir_adj, false);
    let (cb_dir, eb_dir) = run_parallel_brandes(n, &out_adj, true);
    let (hits_hub, hits_auth) = compute_hits_on_dag(&out_adj, &in_adj, 30);

    // 1b. Multi-Level Louvain Modularity Q(gamma) & Random-Walk Infomap Flow Compression L(M)
    let disc_vec: Vec<Discipline> = active_records.iter().map(|r| r.disc).collect();
    let id_vec: Vec<String> = active_records.iter().map(|r| r.id.clone()).collect();
    let communities = build_full_community_report(
        &out_adj,
        &undir_adj,
        &disc_vec,
        &id_vec,
        &cb_undir,
        params.louvain_resolution,
        params.infomap_teleport,
    );

    // 2. Guimera-Amaral (2005) (z_i, P_i) & Burt's Structural Hole Constraint C_i
    let mut within_deg = vec![0.0f64; n];
    let mut part_p = vec![0.0f64; n];
    let mut burt_c = vec![0.0f64; n];

    for i in 0..n {
        let nbrs = &undir_adj[i];
        let ki = nbrs.len();
        if ki == 0 {
            part_p[i] = 0.0;
            burt_c[i] = 1.0;
            continue;
        }
        let mut counts_by_disc = [0usize; 6];
        for &j in nbrs {
            counts_by_disc[active_records[j].disc.index()] += 1;
        }
        within_deg[i] = counts_by_disc[active_records[i].disc.index()] as f64;
        let mut sum_sq = 0.0;
        for &cnt in &counts_by_disc {
            let frac = (cnt as f64) / (ki as f64);
            sum_sq += frac * frac;
        }
        part_p[i] = (1.0 - sum_sq).max(0.0);

        let pij = 1.0 / (ki as f64);
        if ki > 96 {
            burt_c[i] = (pij * 1.35).min(1.0);
        } else {
            let mut c_sum = 0.0;
            for &j in nbrs {
                let mut indirect = 0.0;
                for &q in nbrs {
                    if q != j && undir_sets[q].contains(&j) {
                        indirect += pij * (1.0 / (undir_adj[q].len().max(1) as f64));
                    }
                }
                let term = pij + indirect;
                c_sum += term * term;
            }
            burt_c[i] = c_sum.min(1.25);
        }
    }

    let mut within_z = vec![0.0f64; n];
    for d in Discipline::ALL {
        let indices: Vec<usize> = (0..n).filter(|&i| active_records[i].disc == d).collect();
        if indices.is_empty() {
            continue;
        }
        let mean = indices.iter().map(|&i| within_deg[i]).sum::<f64>() / (indices.len() as f64);
        let var = indices
            .iter()
            .map(|&i| (within_deg[i] - mean).powi(2))
            .sum::<f64>()
            / (indices.len() as f64);
        let std = var.sqrt();
        for &i in &indices {
            within_z[i] = if std > 1e-6 {
                (within_deg[i] - mean) / std
            } else {
                0.0
            };
        }
    }

    // 3. Citation Velocity, Acceleration, Immediacy, Ke Sleeping Beauty B, Disruption CD
    let ln2 = std::f64::consts::LN_2;
    let w_len = params.window_width.max(1) as usize;
    let s_len = params.window_stride.max(1) as usize;
    let tau = params.half_life.max(0.5);

    let mut artifact_peer_map: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut bio_entity_peer_map: HashMap<String, HashSet<usize>> = HashMap::new();
    for (idx, r) in active_records.iter().enumerate() {
        for lk in &r.datacite_links {
            if !lk.artifact_doi.is_empty() {
                artifact_peer_map
                    .entry(lk.artifact_doi.clone())
                    .or_default()
                    .insert(idx);
            }
        }
        for b in &r.bio_entities {
            if !b.entity_id.is_empty() {
                let key = format!("{}:{}", b.kind.as_str(), b.entity_id.to_ascii_uppercase());
                bio_entity_peer_map.entry(key).or_default().insert(idx);
            }
        }
    }

    let keep_full_series = n <= 4_000;
    let papers: Vec<PaperMetrics> = (0..n)
        .into_par_iter()
        .map(|i| {
            let rec = active_records[i].clone();
            let pub_yr = rec.year;
            let age_len = ((horizon - pub_yr + 1).max(1)) as usize;
            let mut annual_counts = vec![0.0f64; age_len];

            for &citer_idx in &in_adj[i] {
                let c_yr = active_records[citer_idx].year;
                let t_age = (c_yr - pub_yr).clamp(0, (age_len - 1) as i32) as usize;
                annual_counts[t_age] += 1.0;
            }

            let mut v_win_series = vec![0.0f64; year_span];
            let mut v_exp_series = vec![0.0f64; year_span];
            let mut accel_series = vec![0.0f64; year_span];
            let mut cum_series = vec![0.0f64; year_span];

            let mut running_cum = 0.0;
            for y_idx in 0..year_span {
                let yr = min_year + (y_idx as i32);
                if yr < pub_yr {
                    continue;
                }
                let t_age = (yr - pub_yr) as usize;
                running_cum += annual_counts[t_age];
                cum_series[y_idx] = running_cum;

                let start_age = t_age.saturating_sub(w_len - 1);
                let win_sum: f64 = annual_counts[start_age..=t_age].iter().sum();
                let eff_w = w_len.min(t_age + 1).max(1) as f64;
                v_win_series[y_idx] = win_sum / eff_w;

                let mut exp_sum = 0.0;
                for (a, &cnt) in annual_counts.iter().enumerate().take(t_age + 1) {
                    if cnt > 0.0 {
                        let dt = (t_age - a) as f64;
                        exp_sum += cnt * ((-ln2 * dt) / tau).exp();
                    }
                }
                v_exp_series[y_idx] = exp_sum;
            }

            for y_idx in 0..year_span {
                let yr = min_year + (y_idx as i32);
                if yr < pub_yr {
                    continue;
                }
                let pub_offset = (pub_yr - min_year).max(0) as usize;
                let prev_idx = y_idx.saturating_sub(s_len).max(pub_offset);
                accel_series[y_idx] = v_win_series[y_idx] - v_win_series[prev_idx];
            }

            let in_deg = in_adj[i].len();
            let out_deg = out_adj[i].len();
            let last_idx = year_span - 1;
            let v_exp = v_exp_series[last_idx];
            let v_win = v_win_series[last_idx];
            let accel = accel_series[last_idx];

            let cites_5y: f64 = annual_counts.iter().take(age_len.min(6)).sum();
            let immed_5y_pct = if in_deg > 0 {
                (cites_5y / (in_deg as f64)) * 100.0
            } else {
                0.0
            };

            // Ke et al. (2015) Sleeping Beauty B & Awakening Year t_a
            let mut tm = 0usize;
            let mut c_tm = annual_counts[0];
            for (a, &cnt) in annual_counts.iter().enumerate().skip(1) {
                if cnt >= c_tm && cnt > 0.0 {
                    c_tm = cnt;
                    tm = a;
                }
            }
            let c0 = annual_counts[0];
            let mut beauty_b = 0.0;
            let mut awaken_year = pub_yr;
            let mut ke_ref_line = vec![0.0f64; age_len];

            if tm > 0 && c_tm > c0 {
                let slope = (c_tm - c0) / (tm as f64);
                let denom_dist = ((c_tm - c0).powi(2) + (tm as f64).powi(2)).sqrt().max(1e-9);
                let mut max_dist = -1.0;
                for a in 0..=tm {
                    let lt = c0 + slope * (a as f64);
                    ke_ref_line[a] = lt;
                    beauty_b += (lt - annual_counts[a]) / annual_counts[a].max(1.0);
                    let dist = ((c_tm - c0) * (a as f64)
                        - (tm as f64) * annual_counts[a]
                        + (tm as f64) * c0)
                        .abs()
                        / denom_dist;
                    if dist > max_dist && a < tm {
                        max_dist = dist;
                        awaken_year = pub_yr + (a as i32);
                    }
                }
                for val in ke_ref_line.iter_mut().skip(tm + 1) {
                    *val = c_tm;
                }
            } else {
                ke_ref_line.copy_from_slice(&annual_counts);
            }
            beauty_b = beauty_b.max(0.0);

            // Sparse O(d_in + d_out * log d_in) exact all-time Disruption Index CD_i and 5-year CD5_i (no .take(512) cap)
            let refs_set: HashSet<usize> = out_adj[i].iter().copied().collect();
            let (mut ni, mut nj, mut nk) = (0usize, 0usize, 0usize);
            let (mut ni5, mut nj5, mut nk5) = (0usize, 0usize, 0usize);
            if pub_yr > 0 {
                let win_end = pub_yr.saturating_add(5);
                if refs_set.is_empty() {
                    let lo_all = in_adj[i].partition_point(|&w| active_records[w].year < pub_yr);
                    for &w in &in_adj[i][lo_all..] {
                        if w != i && active_records[w].year > 0 {
                            ni += 1;
                        }
                    }
                    let lo_5 = in_adj[i].partition_point(|&w| active_records[w].year <= pub_yr);
                    let hi_5 = in_adj[i].partition_point(|&w| active_records[w].year <= win_end);
                    for &w in &in_adj[i][lo_5..hi_5] {
                        if w != i {
                            ni5 += 1;
                        }
                    }
                } else {
                    let mut citer_set: HashSet<usize> = HashSet::with_capacity(in_adj[i].len());
                    let mut citer_set_5: HashSet<usize> = HashSet::new();
                    let lo_all = in_adj[i].partition_point(|&w| active_records[w].year < pub_yr);
                    for &w in &in_adj[i][lo_all..] {
                        let c_yr = active_records[w].year;
                        if w != i && c_yr > 0 {
                            citer_set.insert(w);
                            let cites_ref = out_adj[w].iter().any(|r| refs_set.contains(r));
                            if cites_ref {
                                nj += 1;
                            } else {
                                ni += 1;
                            }
                            if c_yr > pub_yr && c_yr <= win_end {
                                citer_set_5.insert(w);
                                if cites_ref {
                                    nj5 += 1;
                                } else {
                                    ni5 += 1;
                                }
                            }
                        }
                    }

                    let mut seen_nk: HashSet<usize> = HashSet::new();
                    let mut seen_nk_5: HashSet<usize> = HashSet::new();
                    for &r_idx in &out_adj[i] {
                        let lo_r_all =
                            in_adj[r_idx].partition_point(|&w| active_records[w].year < pub_yr);
                        for &w in &in_adj[r_idx][lo_r_all..] {
                            if w != i
                                && active_records[w].year > 0
                                && !citer_set.contains(&w)
                                && seen_nk.insert(w)
                            {
                                nk += 1;
                            }
                        }
                        let lo_r_5 =
                            in_adj[r_idx].partition_point(|&w| active_records[w].year <= pub_yr);
                        let hi_r_5 =
                            in_adj[r_idx].partition_point(|&w| active_records[w].year <= win_end);
                        for &w in &in_adj[r_idx][lo_r_5..hi_r_5] {
                            if w != i && !citer_set_5.contains(&w) && seen_nk_5.insert(w) {
                                nk5 += 1;
                            }
                        }
                    }
                }
            }
            let cd_denom = ni + nj + nk;
            let disruption_cd = if cd_denom > 0 {
                ((ni as f64) - (nj as f64)) / (cd_denom as f64)
            } else {
                0.0
            };
            let cd5_denom = ni5 + nj5 + nk5;
            let disruption_cd5 = if cd5_denom > 0 {
                ((ni5 as f64) - (nj5 as f64)) / (cd5_denom as f64)
            } else {
                0.0
            };

            let regime_tag = if beauty_b >= 12.0 && (awaken_year - pub_yr) >= 5 {
                "Delayed Awakening"
            } else if accel > 0.04 {
                "Accelerating Frontier"
            } else if in_deg >= 4 && pub_yr <= 2005 {
                "Foundational Pillar"
            } else if disruption_cd < -0.25 && in_deg >= 2 {
                "Consolidating Synthesis"
            } else {
                "Steady Core"
            };

            let (role_code, role_label) =
                classify_guimera_role(within_z[i], part_p[i], undir_adj[i].len(), n);
            let bridge_score = (cb_undir[i] * part_p[i]).max(0.0).sqrt();

            let post_retraction_cites = if rec.retraction.status.is_retracted_or_withdrawn() {
                let cutoff_yr = rec.retraction.notice_year.unwrap_or(pub_yr);
                in_adj[i]
                    .iter()
                    .filter(|&&citer_idx| active_records[citer_idx].year >= cutoff_yr)
                    .count()
            } else {
                0
            };
            let cites_retracted_count = out_adj[i]
                .iter()
                .filter(|&&ref_idx| {
                    active_records[ref_idx]
                        .retraction
                        .status
                        .is_retracted_or_withdrawn()
                })
                .count();
            let dataset_bridge_count = rec.dataset_count();
            let software_bridge_count = rec.software_count();
            let mut peer_set: HashSet<usize> = HashSet::new();
            for lk in &rec.datacite_links {
                if let Some(peers) = artifact_peer_map.get(&lk.artifact_doi) {
                    for &peer_idx in peers {
                        if peer_idx != i {
                            peer_set.insert(peer_idx);
                        }
                    }
                }
            }
            let shared_artifact_peers = peer_set.len();
            let uniprot_bridge_count = rec.uniprot_count();
            let pdb_bridge_count = rec.pdb_count();
            let epmc_annotation_count = rec.epmc_annotation_count();
            let mut bio_peer_set: HashSet<usize> = HashSet::new();
            for b in &rec.bio_entities {
                let key = format!("{}:{}", b.kind.as_str(), b.entity_id.to_ascii_uppercase());
                if let Some(peers) = bio_entity_peer_map.get(&key) {
                    for &peer_idx in peers {
                        if peer_idx != i {
                            bio_peer_set.insert(peer_idx);
                        }
                    }
                }
            }
            let shared_bio_entity_peers = bio_peer_set.len();
            let rec_disc = rec.disc;

            PaperMetrics {
                idx: i,
                record: rec,
                in_deg,
                out_deg,
                undir_deg: undir_adj[i].len(),
                v_exp,
                v_win,
                accel,
                immed_5y_pct,
                beauty_b,
                awaken_year,
                hibernation_years: awaken_year - pub_yr,
                peak_age_tm: tm,
                disruption_cd,
                disruption_cd5,
                hits_hub: hits_hub[i],
                hits_authority: hits_auth[i],
                cb_undir: cb_undir[i],
                cb_dir: cb_dir[i],
                part_p: part_p[i],
                within_z: within_z[i],
                burt_c: burt_c[i],
                bridge_score,
                role_code,
                role_label,
                regime_tag,
                louvain_comm: communities.louvain.assignments.get(i).copied().unwrap_or(0),
                louvain_part_p: communities.louvain_part_p.get(i).copied().unwrap_or(0.0),
                infomap_comm: communities.infomap.assignments.get(i).copied().unwrap_or(0),
                infomap_part_p: communities.infomap_part_p.get(i).copied().unwrap_or(0.0),
                infomap_flow: communities.infomap.stationary_flow.get(i).copied().unwrap_or(0.0),
                reclassified_louvain: communities
                    .louvain
                    .assignments
                    .get(i)
                    .and_then(|&cid| communities.louvain.clusters.get(cid))
                    .map(|c| c.dominant_disc != rec_disc)
                    .unwrap_or(false),
                reclassified_infomap: communities
                    .infomap
                    .assignments
                    .get(i)
                    .and_then(|&mid| communities.infomap.clusters.get(mid))
                    .map(|c| c.dominant_disc != rec_disc)
                    .unwrap_or(false),
                post_retraction_cites,
                cites_retracted_count,
                dataset_bridge_count,
                software_bridge_count,
                shared_artifact_peers,
                uniprot_bridge_count,
                pdb_bridge_count,
                epmc_annotation_count,
                shared_bio_entity_peers,
                annual_counts: if keep_full_series || i < 32 { annual_counts } else { Vec::new() },
                ke_ref_line: if keep_full_series || i < 32 { ke_ref_line } else { Vec::new() },
                v_win_series: if keep_full_series || i < 32 { v_win_series } else { Vec::new() },
                v_exp_series: if keep_full_series || i < 32 { v_exp_series } else { Vec::new() },
                accel_series: if keep_full_series || i < 32 { accel_series } else { Vec::new() },
                cum_series: if keep_full_series || i < 32 { cum_series } else { Vec::new() },
            }
        })
        .collect();

    // 4. Girvan-Newman Edge Betweenness Conduits
    let mut conduits: Vec<EdgeConduit> = active_edges
        .iter()
        .map(|&(u, v)| {
            let u_key = (u.min(v), u.max(v));
            let eb_u = eb_undir.get(&u_key).copied().unwrap_or(0.0);
            let eb_d = eb_dir.get(&(u, v)).copied().unwrap_or(0.0);
            EdgeConduit {
                u,
                v,
                source_id: papers[u].record.id.clone(),
                target_id: papers[v].record.id.clone(),
                source_cite: papers[u].record.short_cite(),
                target_cite: papers[v].record.short_cite(),
                disc_from: papers[u].record.disc,
                disc_to: papers[v].record.disc,
                cross_disc: papers[u].record.disc != papers[v].record.disc,
                eb_undir: eb_u,
                eb_dir: eb_d,
            }
        })
        .collect();
    conduits.sort_by(|a, b| {
        b.eb_undir
            .partial_cmp(&a.eb_undir)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let cross_edge_count = conduits.iter().filter(|c| c.cross_disc).count();
    if conduits.len() > 5_000 {
        conduits.truncate(5_000);
    }

    // 5. Sliding Multi-Year Window Mutual Information I_w(D; T), NMI, PMI & JSD Drift
    let mut raw_windows: Vec<(i32, i32)> = Vec::new();
    let w_i32 = params.window_width.max(3);
    let s_i32 = params.window_stride.max(1);
    if max_year - min_year <= w_i32 {
        raw_windows.push((min_year, max_year));
    } else {
        let mut ws = min_year;
        while ws <= max_year {
            let we = (ws + w_i32 - 1).min(max_year);
            raw_windows.push((ws, we));
            if we >= max_year {
                break;
            }
            ws += s_i32;
        }
    }

    let alpha = params.smoothing_alpha.max(1e-4);
    let lam = params.discourse_weight.clamp(0.0, 2.0);
    let mut windows_all: Vec<WindowTopicStats> = raw_windows
        .into_iter()
        .enumerate()
        .map(|(w_idx, (ws, we))| {
            let mut raw_counts = [[0.0f64; 12]; 6];
            let mut pub_counts_by_disc = [0usize; 6];
            let mut active_pubs = 0usize;
            let mut active_cites = 0usize;

            for i in 0..n {
                let p = &papers[i];
                let d_idx = p.record.disc.index();
                let mut weight = 0.0;
                if p.record.year >= ws && p.record.year <= we {
                    weight += 1.0;
                    pub_counts_by_disc[d_idx] += 1;
                    active_pubs += 1;
                    active_cites += out_adj[i].len();
                }
                let cites_in_win = in_adj[i]
                    .iter()
                    .filter(|&&c_idx| {
                        let cy = active_records[c_idx].year;
                        cy >= ws && cy <= we
                    })
                    .count();
                weight += lam * (cites_in_win as f64);

                if weight > 0.0 && !p.record.concepts.is_empty() {
                    let share = weight / (p.record.concepts.len() as f64);
                    for cid in &p.record.concepts {
                        if let Some(t_idx) = concept_index(cid) {
                            raw_counts[d_idx][t_idx] += share;
                        }
                    }
                }
            }

            let mut joint_p = [[0.0f64; 12]; 6];
            let mut marg_d = [0.0f64; 6];
            let mut marg_t = [0.0f64; 12];
            let mut total_mass = 0.0;

            for d in 0..6 {
                for t in 0..12 {
                    let v = raw_counts[d][t] + alpha;
                    joint_p[d][t] = v;
                    total_mass += v;
                }
            }
            for d in 0..6 {
                for t in 0..12 {
                    joint_p[d][t] /= total_mass.max(1e-12);
                    marg_d[d] += joint_p[d][t];
                    marg_t[t] += joint_p[d][t];
                }
            }

            let mut h_disc = 0.0;
            let mut h_topic = 0.0;
            let mut mi_bits = 0.0;
            for &pd in &marg_d {
                if pd > 1e-12 {
                    h_disc -= pd * pd.log2();
                }
            }
            for &pt in &marg_t {
                if pt > 1e-12 {
                    h_topic -= pt * pt.log2();
                }
            }

            let mut pmi = [[0.0f64; 12]; 6];
            let mut cond_t_given_d = [[0.0f64; 12]; 6];
            for d in 0..6 {
                for t in 0..12 {
                    let pdt = joint_p[d][t];
                    let denom = (marg_d[d] * marg_t[t]).max(1e-15);
                    let ratio = pdt / denom;
                    pmi[d][t] = ratio.max(1e-12).log2();
                    if pdt > 1e-15 {
                        mi_bits += pdt * ratio.log2();
                    }
                    cond_t_given_d[d][t] = if marg_d[d] > 1e-12 {
                        pdt / marg_d[d]
                    } else {
                        1.0 / 12.0
                    };
                }
            }
            mi_bits = mi_bits.max(0.0);
            let nmi = if (h_disc + h_topic) > 1e-9 {
                (2.0 * mi_bits) / (h_disc + h_topic)
            } else {
                0.0
            };

            let mut jsd_matrix = [[0.0f64; 6]; 6];
            for d1 in 0..6 {
                for d2 in (d1 + 1)..6 {
                    let val = compute_jsd(&cond_t_given_d[d1], &cond_t_given_d[d2]);
                    jsd_matrix[d1][d2] = val;
                    jsd_matrix[d2][d1] = val;
                }
            }

            WindowTopicStats {
                w_idx,
                start_year: ws,
                end_year: we,
                label: format!("{}-{}", ws, we),
                pub_counts_by_disc,
                active_pubs,
                active_cites,
                h_disc,
                h_topic,
                mi_bits,
                nmi,
                pmi,
                cond_t_given_d,
                jsd_matrix,
                within_drift: [0.0; 6],
            }
        })
        .collect();

    let non_empty: Vec<WindowTopicStats> = windows_all
        .iter()
        .filter(|w| w.active_pubs > 0 || w.active_cites > 0)
        .cloned()
        .collect();
    if !non_empty.is_empty() {
        windows_all = non_empty;
    }
    for (idx, w) in windows_all.iter_mut().enumerate() {
        w.w_idx = idx;
    }

    for w in 1..windows_all.len() {
        for d in 0..6 {
            let drift = compute_jsd(
                &windows_all[w - 1].cond_t_given_d[d],
                &windows_all[w].cond_t_given_d[d],
            );
            windows_all[w].within_drift[d] = drift;
        }
    }

    let mut cumulative_disc_drift = [0.0f64; 6];
    if let (Some(first), Some(last)) = (windows_all.first(), windows_all.last()) {
        for d in 0..6 {
            cumulative_disc_drift[d] = compute_jsd(&first.cond_t_given_d[d], &last.cond_t_given_d[d]);
        }
    }

    CorpusAnalytics {
        min_year,
        max_year,
        year_span,
        papers,
        active_edges,
        out_adj,
        in_adj,
        undir_adj,
        conduits,
        cross_edge_count,
        windows: windows_all,
        cumulative_disc_drift,
        communities,
    }
}

const PIVOT_BRANDES_THRESHOLD: usize = 1_500;
const PIVOT_SAMPLE_K: usize = 192;

fn run_parallel_brandes(
    n: usize,
    adj: &[Vec<usize>],
    directed: bool,
) -> (Vec<f64>, HashMap<(usize, usize), f64>) {
    if n == 0 {
        return (Vec::new(), HashMap::new());
    }

    let sources: Vec<usize> = if n <= PIVOT_BRANDES_THRESHOLD {
        (0..n).collect()
    } else {
        // Stratified hub + deterministic uniform pivot estimator O(K * (V + E))
        let mut by_deg: Vec<usize> = (0..n).collect();
        by_deg.sort_by_key(|&u| std::cmp::Reverse(adj[u].len()));
        let mut piv_set: HashSet<usize> = by_deg.into_iter().take(64).collect();
        let stride = (n / (PIVOT_SAMPLE_K - 64).max(1)).max(1);
        let mut idx = 0usize;
        while piv_set.len() < PIVOT_SAMPLE_K.min(n) && idx < n {
            piv_set.insert(idx);
            idx += stride;
        }
        let mut v: Vec<usize> = piv_set.into_iter().collect();
        v.sort_unstable();
        v
    };
    let pivot_scale = (n as f64) / (sources.len().max(1) as f64);
    let collect_edge_eb = n <= 8_000;

    let partials: Vec<(Vec<f64>, Vec<((usize, usize), f64)>)> = sources
        .into_par_iter()
        .map(|s| {
            let mut stack = Vec::with_capacity(n);
            let mut pred = vec![Vec::<usize>::new(); n];
            let mut sigma = vec![0.0f64; n];
            let mut dist = vec![-1i32; n];
            sigma[s] = 1.0;
            dist[s] = 0;

            let mut queue = VecDeque::with_capacity(n);
            queue.push_back(s);
            while let Some(v) = queue.pop_front() {
                stack.push(v);
                for &w in &adj[v] {
                    if dist[w] < 0 {
                        queue.push_back(w);
                        dist[w] = dist[v] + 1;
                    }
                    if dist[w] == dist[v] + 1 {
                        sigma[w] += sigma[v];
                        pred[w].push(v);
                    }
                }
            }

            let mut delta = vec![0.0f64; n];
            let mut local_cb = vec![0.0f64; n];
            let mut local_eb = Vec::new();

            while let Some(w) = stack.pop() {
                for &v in &pred[w] {
                    let c = (sigma[v] / sigma[w].max(1.0)) * (1.0 + delta[w]);
                    delta[v] += c;
                    if collect_edge_eb {
                        let key = if directed {
                            (v, w)
                        } else {
                            (v.min(w), v.max(w))
                        };
                        local_eb.push((key, c));
                    }
                }
                if w != s {
                    local_cb[w] += delta[w];
                }
            }

            (local_cb, local_eb)
        })
        .collect();

    let mut cb = vec![0.0f64; n];
    let mut eb: HashMap<(usize, usize), f64> = HashMap::new();
    for (l_cb, l_eb) in partials {
        for (i, val) in l_cb.into_iter().enumerate() {
            cb[i] += val;
        }
        for (k, val) in l_eb {
            *eb.entry(k).or_insert(0.0) += val;
        }
    }

    let denom = ((n.saturating_sub(1) as f64) * (n.saturating_sub(2) as f64)).max(1.0);
    for val in &mut cb {
        let scaled = *val * pivot_scale;
        *val = if directed {
            scaled / denom
        } else {
            (scaled / 2.0) / ((denom / 2.0).max(1.0))
        };
    }
    for val in eb.values_mut() {
        *val *= pivot_scale;
        if !directed {
            *val /= 2.0;
        }
    }

    (cb, eb)
}

fn classify_guimera_role(z: f64, p: f64, k: usize, n: usize) -> (&'static str, &'static str) {
    if k == 0 {
        return ("R1", "Ultra-peripheral");
    }
    let hub_cut = if n >= 120 { 2.5 } else { 1.65 };
    if z >= hub_cut {
        if p <= 0.30 {
            ("R5", "Provincial Hub")
        } else if p <= 0.75 {
            ("R6", "Connector Hub")
        } else {
            ("R7", "Kinless Hub")
        }
    } else if p <= 0.05 {
        ("R1", "Ultra-peripheral")
    } else if p <= 0.62 {
        ("R2", "Peripheral")
    } else if p <= 0.80 {
        ("R3", "Non-hub Connector")
    } else {
        ("R4", "Kinless Non-hub")
    }
}

pub fn compute_jsd(p: &[f64; 12], q: &[f64; 12]) -> f64 {
    let mut kl_pm = 0.0;
    let mut kl_qm = 0.0;
    for i in 0..12 {
        let m = 0.5 * (p[i] + q[i]);
        if p[i] > 1e-14 && m > 1e-14 {
            kl_pm += p[i] * (p[i] / m).log2();
        }
        if q[i] > 1e-14 && m > 1e-14 {
            kl_qm += q[i] * (q[i] / m).log2();
        }
    }
    (0.5 * (kl_pm + kl_qm)).clamp(0.0, 1.0)
}

pub fn find_shortest_epistemic_path(
    analytics: &CorpusAnalytics,
    src_idx: usize,
    dst_idx: usize,
    directed: bool,
) -> Option<Vec<usize>> {
    let n = analytics.papers.len();
    if src_idx >= n || dst_idx >= n {
        return None;
    }
    if src_idx == dst_idx {
        return Some(vec![src_idx]);
    }
    let adj = if directed {
        &analytics.out_adj
    } else {
        &analytics.undir_adj
    };

    let mut visited = vec![false; n];
    let mut prev = vec![usize::MAX; n];
    let mut q = VecDeque::new();
    visited[src_idx] = true;
    q.push_back(src_idx);

    while let Some(u) = q.pop_front() {
        if u == dst_idx {
            let mut path = Vec::new();
            let mut cur = dst_idx;
            while cur != usize::MAX {
                path.push(cur);
                cur = prev[cur];
            }
            path.reverse();
            return Some(path);
        }
        for &v in &adj[u] {
            if !visited[v] {
                visited[v] = true;
                prev[v] = u;
                q.push_back(v);
            }
        }
    }
    None
}

/// Deterministic HITS Hub & Authority power iteration on the citation DAG (`out_adj` / `in_adj`).
pub fn compute_hits_on_dag(
    out_adj: &[Vec<usize>],
    in_adj: &[Vec<usize>],
    iters: usize,
) -> (Vec<f64>, Vec<f64>) {
    let n = out_adj.len().min(in_adj.len());
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    let init = 1.0 / (n as f64).sqrt();
    let mut hub = vec![init; n];
    let mut auth = vec![init; n];
    if iters == 0 {
        return (hub, auth);
    }

    for _ in 0..iters {
        let mut next_auth = vec![0.0_f64; n];
        for (v, citers) in in_adj.iter().take(n).enumerate() {
            let mut sum = 0.0;
            for &u in citers {
                if u < n {
                    sum += hub[u];
                }
            }
            next_auth[v] = sum;
        }
        let norm_a = next_auth.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm_a > 1e-12 {
            for x in &mut next_auth {
                *x /= norm_a;
            }
        } else {
            next_auth.fill(0.0);
        }

        let mut next_hub = vec![0.0_f64; n];
        for (u, refs) in out_adj.iter().take(n).enumerate() {
            let mut sum = 0.0;
            for &v in refs {
                if v < n {
                    sum += next_auth[v];
                }
            }
            next_hub[u] = sum;
        }
        let norm_h = next_hub.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm_h > 1e-12 {
            for x in &mut next_hub {
                *x /= norm_h;
            }
        } else {
            next_hub.fill(0.0);
        }

        let max_delta = auth
            .iter()
            .zip(next_auth.iter())
            .chain(hub.iter().zip(next_hub.iter()))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f64, f64::max);

        auth = next_auth;
        hub = next_hub;
        if max_delta < 1e-10 {
            break;
        }
    }
    (hub, auth)
}

/// "Publish or Perish"-compatible bibliometric citation impact summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PopCitationMetrics {
    pub total_papers: usize,
    pub total_citations: u64,
    pub min_year: i32,
    pub max_year: i32,
    pub year_span: i32,
    pub cites_per_year: f64,
    pub cites_per_paper: f64,
    pub cites_per_author: f64,
    pub authors_per_paper: f64,
    pub h_index: usize,
    pub g_index: usize,
    pub hi_norm: usize,
    pub hi_annual: f64,
    pub e_index: f64,
    pub ha_index: usize,
}

fn is_initial_token(tok: &str) -> bool {
    let t = tok.trim();
    if t.is_empty()
        || t.eq_ignore_ascii_case("et")
        || t.eq_ignore_ascii_case("al.")
        || t.eq_ignore_ascii_case("al")
    {
        return false;
    }
    if t.ends_with('.') {
        let parts: Vec<&str> = t
            .trim_end_matches('.')
            .split(['.', '-'])
            .filter(|p| !p.is_empty())
            .collect();
        !parts.is_empty()
            && parts
                .iter()
                .all(|p| p.chars().count() <= 2 && p.chars().all(|c| c.is_alphabetic()))
    } else {
        t.chars().count() == 1 && t.chars().all(|c| c.is_alphabetic())
    }
}

fn is_surname_word(tok: &str) -> bool {
    !tok.contains('.')
        && tok.chars().count() >= 2
        && !tok.eq_ignore_ascii_case("et")
        && !tok.eq_ignore_ascii_case("al")
        && !tok.eq_ignore_ascii_case("and")
        && tok.chars().any(|c| c.is_alphabetic())
}

fn is_unambiguous_surname_particle(lower: &str) -> bool {
    matches!(
        lower,
        "de" | "del"
            | "della"
            | "degli"
            | "dei"
            | "des"
            | "da"
            | "dos"
            | "das"
            | "du"
            | "von"
            | "vom"
            | "st."
            | "ste."
            | "santa"
            | "ibn"
            | "bin"
            | "abu"
            | "ter"
            | "ten"
            | "op"
            | "zu"
            | "zum"
            | "zur"
            | "fitz"
            | "ap"
            | "ab"
    )
}

fn is_surname_particle_token(lower: &str) -> bool {
    is_unambiguous_surname_particle(lower)
        || matches!(
            lower,
            "van"
                | "di"
                | "do"
                | "la"
                | "le"
                | "les"
                | "lo"
                | "al"
                | "el"
                | "ben"
                | "mac"
                | "mc"
                | "san"
                | "santo"
                | "st"
                | "ste"
                | "der"
                | "den"
                | "dem"
                | "te"
                | "het"
                | "bar"
                | "af"
                | "av"
        )
}

fn match_compound_particle(t0_lower: &str, t1_lower: &str) -> bool {
    matches!(
        (t0_lower, t1_lower),
        ("de", "la")
            | ("de", "los")
            | ("de", "las")
            | ("de", "lo")
            | ("van", "der")
            | ("van", "den")
            | ("van", "de")
            | ("von", "der")
            | ("von", "dem")
            | ("von", "den")
            | ("op", "den")
            | ("op", "de")
            | ("op", "het")
            | ("in", "de")
            | ("in", "den")
            | ("auf", "der")
    )
}

/// Evaluates whether a comma-separated segment looks like a `"Family"` surname segment
/// (either a single-word surname or a recognized particle/compound surname with no initial tokens).
/// Returns `Some(true)` if unambiguously a surname even when followed by a two-word given name,
/// or `Some(false)` if it starts with a capitalized particle that can also act as a given name.
fn classify_family_name_segment(seg: &str) -> Option<bool> {
    let toks: Vec<&str> = seg.split_whitespace().collect();
    if toks.is_empty() {
        return None;
    }
    if toks.len() == 1 {
        let first = toks[0];
        return if !first.contains('.') && first.chars().count() >= 2 {
            Some(true)
        } else {
            None
        };
    }

    let lower: Vec<String> = toks.iter().map(|t| t.to_ascii_lowercase()).collect();
    let mut idx = 0usize;
    let mut matched_compound = false;

    while idx + 1 < toks.len() {
        if idx + 2 < toks.len() && match_compound_particle(&lower[idx], &lower[idx + 1]) {
            if idx == 0 {
                matched_compound = true;
            }
            idx += 2;
            continue;
        }
        if is_surname_particle_token(&lower[idx]) {
            idx += 1;
            continue;
        }
        break;
    }

    if idx == 0 || idx >= toks.len() {
        return None;
    }

    let remaining = &toks[idx..];
    if remaining.len() > 2 || !remaining.iter().all(|t| is_surname_word(t)) {
        return None;
    }

    let starts_lowercase = toks[0].chars().next().is_some_and(|c| c.is_lowercase());
    let unambiguous =
        starts_lowercase || matched_compound || is_unambiguous_surname_particle(&lower[0]);
    Some(unambiguous)
}

fn is_family_name_segment(seg: &str) -> bool {
    classify_family_name_segment(seg).is_some()
}

fn strip_et_al(input: &str) -> (String, bool) {
    let mut s = input.trim().to_string();
    let mut found = false;
    loop {
        let lower = s.to_ascii_lowercase();
        let Some(pos) = lower.find("et al.") else {
            break;
        };
        found = true;
        let end = pos + "et al.".len();
        let mut start = pos;
        let before = &s[..pos];
        if let Some(trimmed_before) = before.strip_suffix(", ") {
            start = trimmed_before.len();
        } else if let Some(trimmed_before) = before.strip_suffix(',') {
            start = trimmed_before.len();
        }
        let mut rebuilt = String::with_capacity(s.len());
        rebuilt.push_str(s[..start].trim_end());
        let after = s[end..].trim_start();
        if !rebuilt.is_empty() && !after.is_empty() {
            rebuilt.push(' ');
        }
        rebuilt.push_str(after);
        s = rebuilt;
    }
    (s, found)
}

fn split_on_and_conjunction(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for tok in s.split_whitespace() {
        if tok.eq_ignore_ascii_case("and") {
            if !cur.is_empty() {
                out.push(cur.join(" "));
                cur.clear();
            }
        } else {
            cur.push(tok);
        }
    }
    if !cur.is_empty() {
        out.push(cur.join(" "));
    }
    out
}

fn count_comma_author_group(group: &str) -> usize {
    let segments: Vec<&str> = group
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return 0;
    }
    if segments.len() == 1 {
        return 1;
    }

    // Single-author "Family, Given" (e.g. "Shannon, Claude", "de la Cruz, Juan", "van der Waals, Johannes D.")
    if segments.len() == 2 {
        if let Some(unambiguous_two_word_given) = classify_family_name_segment(segments[0]) {
            let seg1_toks: Vec<&str> = segments[1].split_whitespace().collect();
            let all_initials =
                !seg1_toks.is_empty() && seg1_toks.iter().all(|t| is_initial_token(t));
            let single_or_given_plus_init = !seg1_toks.is_empty()
                && ((seg1_toks.len() == 1
                    && !seg1_toks[0].eq_ignore_ascii_case("et")
                    && !seg1_toks[0].eq_ignore_ascii_case("al."))
                    || (seg1_toks.len() == 2
                        && !seg1_toks[0].contains('.')
                        && !seg1_toks[0].eq_ignore_ascii_case("et")
                        && is_initial_token(seg1_toks[1])));
            let two_word_given = unambiguous_two_word_given
                && seg1_toks.len() == 2
                && seg1_toks
                    .iter()
                    .all(|t| !t.eq_ignore_ascii_case("et") && !t.eq_ignore_ascii_case("al."));
            if all_initials || single_or_given_plus_init || two_word_given {
                return 1;
            }
        }
    }

    // Alternating "Family, Given, Family2, Given2, ..."
    if segments.len() >= 4 && segments.len() % 2 == 0 {
        let all_pairs = segments.chunks_exact(2).all(|pair| {
            if !is_family_name_segment(pair[0]) {
                return false;
            }
            let toks: Vec<&str> = pair[1].split_whitespace().collect();
            if toks.is_empty() {
                return false;
            }
            let all_init = toks.iter().all(|t| is_initial_token(t));
            let single_given =
                toks.len() == 1 && !toks[0].contains('.') && toks[0].chars().count() >= 2;
            let given_plus_init =
                toks.len() == 2 && !toks[0].contains('.') && is_initial_token(toks[1]);
            all_init || single_given || given_plus_init
        });
        if all_pairs {
            return segments.len() / 2;
        }
    }

    // General comma-separated list: only merge trailing initials into a recognized surname segment
    let mut author_count = 0usize;
    let mut i = 0usize;
    while i < segments.len() {
        author_count += 1;
        if i + 1 < segments.len() && is_family_name_segment(segments[i]) {
            let next_toks: Vec<&str> = segments[i + 1].split_whitespace().collect();
            if !next_toks.is_empty() && next_toks.iter().all(|t| is_initial_token(t)) {
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    author_count
}

/// Counts distinct authors in a bibliography string such as `"A. Smith, B. Jones"` or `"Shannon, Claude"`.
pub fn count_paper_authors(authors: &str) -> usize {
    let trimmed = authors.trim();
    if trimmed.is_empty() {
        return 1;
    }
    let (cleaned, has_et_al) = strip_et_al(trimmed);
    if cleaned.is_empty() {
        return 1;
    }

    let mut author_count = 0usize;
    if cleaned.contains(';') {
        for semi_part in cleaned.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            for and_part in split_on_and_conjunction(semi_part) {
                author_count += count_comma_author_group(&and_part);
            }
        }
    } else {
        for and_part in split_on_and_conjunction(&cleaned) {
            author_count += count_comma_author_group(&and_part);
        }
    }
    (author_count + usize::from(has_et_al)).max(1)
}

pub fn effective_record_citations(rec: &PaperRecord, in_deg: usize) -> u64 {
    rec.external_citations.unwrap_or(in_deg as u64).max(in_deg as u64)
}

pub fn compute_pop_citation_metrics_from_pairs(
    items: &[(&PaperRecord, u64)],
    horizon_year: i32,
) -> PopCitationMetrics {
    if items.is_empty() {
        return PopCitationMetrics::default();
    }

    let total_papers = items.len();
    let mut cites: Vec<u64> = Vec::with_capacity(total_papers);
    let mut norm_cites: Vec<f64> = Vec::with_capacity(total_papers);
    let mut annual_rates: Vec<f64> = Vec::with_capacity(total_papers);

    let mut total_citations: u64 = 0;
    let mut total_author_norm_cites: f64 = 0.0;
    let mut total_authors_sum: usize = 0;
    let mut min_year = i32::MAX;
    let mut max_year = i32::MIN;

    for &(rec, c) in items {
        let a = count_paper_authors(&rec.authors);
        let age = (horizon_year - rec.year + 1).max(1) as f64;
        let c_norm = (c as f64) / (a as f64);
        let c_rate = (c as f64) / age;

        min_year = min_year.min(rec.year);
        max_year = max_year.max(rec.year);
        total_citations = total_citations.saturating_add(c);
        total_author_norm_cites += c_norm;
        total_authors_sum += a;

        cites.push(c);
        norm_cites.push(c_norm);
        annual_rates.push(c_rate);
    }

    let year_span = (horizon_year - min_year + 1).max(1);
    let cites_per_year = (total_citations as f64) / (year_span as f64);
    let cites_per_paper = (total_citations as f64) / (total_papers as f64);
    let cites_per_author = total_author_norm_cites / (total_papers as f64);
    let authors_per_paper = (total_authors_sum as f64) / (total_papers as f64);

    cites.sort_unstable_by(|a, b| b.cmp(a));
    let mut h_index = 0usize;
    let mut h_core_sum: u64 = 0;
    for (i, &c) in cites.iter().enumerate() {
        let rank = (i + 1) as u64;
        if c >= rank {
            h_index = i + 1;
            h_core_sum = h_core_sum.saturating_add(c);
        } else {
            break;
        }
    }

    let excess = h_core_sum.saturating_sub((h_index as u64) * (h_index as u64));
    let e_index = (excess as f64).sqrt();

    let mut g_index = 0usize;
    let mut running_sum: u64 = 0;
    for (i, &c) in cites.iter().enumerate() {
        running_sum = running_sum.saturating_add(c);
        let g = (i + 1) as u64;
        if running_sum >= g * g {
            g_index = i + 1;
        }
    }

    norm_cites.sort_by(|a, b| b.total_cmp(a));
    let mut hi_norm = 0usize;
    for (i, &cn) in norm_cites.iter().enumerate() {
        if cn >= (i + 1) as f64 {
            hi_norm = i + 1;
        } else {
            break;
        }
    }
    let hi_annual = (hi_norm as f64) / (year_span as f64);

    annual_rates.sort_by(|a, b| b.total_cmp(a));
    let mut ha_index = 0usize;
    for (i, &rate) in annual_rates.iter().enumerate() {
        if rate >= (i + 1) as f64 {
            ha_index = i + 1;
        } else {
            break;
        }
    }

    PopCitationMetrics {
        total_papers,
        total_citations,
        min_year,
        max_year,
        year_span,
        cites_per_year,
        cites_per_paper,
        cites_per_author,
        authors_per_paper,
        h_index,
        g_index,
        hi_norm,
        hi_annual,
        e_index,
        ha_index,
    }
}

pub fn compute_pop_citation_metrics(
    papers: &[PaperMetrics],
    horizon_year: i32,
) -> PopCitationMetrics {
    let pairs: Vec<(&PaperRecord, u64)> = papers
        .iter()
        .map(|pm| (&pm.record, effective_record_citations(&pm.record, pm.in_deg)))
        .collect();
    compute_pop_citation_metrics_from_pairs(&pairs, horizon_year)
}

pub fn compute_pop_citation_metrics_records(
    records: &[PaperRecord],
    horizon_year: i32,
) -> PopCitationMetrics {
    let pairs: Vec<(&PaperRecord, u64)> = records
        .iter()
        .map(|r| (r, r.external_citations.unwrap_or(0)))
        .collect();
    compute_pop_citation_metrics_from_pairs(&pairs, horizon_year)
}

#[cfg(test)]
mod pop_tests {
    use super::*;
    use crate::models::OaStatus;

    fn mk_paper(id: &str, authors: &str, year: i32, cites: u64) -> PaperRecord {
        PaperRecord {
            id: id.to_string(),
            title: id.to_string(),
            authors: authors.to_string(),
            year,
            venue: "Test Journal".to_string(),
            disc: Discipline::INF,
            doi: String::new(),
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            source_url: String::new(),
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Gold,
            external_citations: Some(cites),
            abstract_text: String::new(),
            terms: Vec::new(),
            concepts: Vec::new(),
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: Default::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        }
    }

    #[test]
    fn pop_metrics_compute_exact_h_g_e_hi_and_ha() {
        let recs = vec![
            mk_paper("p1", "A. Alpha", 2021, 30),         // norm 30, age 6 -> 5.0/yr
            mk_paper("p2", "A. Alpha, B. Beta", 2022, 12), // norm 6,  age 5 -> 2.4/yr
            mk_paper("p3", "A. Alpha, B. Beta, C. Gamma", 2024, 9), // norm 3, age 3 -> 3.0/yr
            mk_paper("p4", "A. Alpha, B. Beta", 2025, 2),  // norm 1,  age 2 -> 1.0/yr
        ];
        // Horizon 2026 => year_span = 2026 - 2021 + 1 = 6
        let m = compute_pop_citation_metrics_records(&recs, 2026);
        assert_eq!(m.total_papers, 4);
        assert_eq!(m.total_citations, 53);
        assert_eq!(m.h_index, 3); // [30>=1, 12>=2, 9>=3, 2<4]
        assert_eq!(m.g_index, 4); // sum=53 >= 16
        // e-index = sqrt((30 + 12 + 9) - 9) = sqrt(42)
        assert!((m.e_index - 42.0f64.sqrt()).abs() < 1e-6);
        // norm_cites desc = [30, 6, 3, 1] -> hi_norm = 3 (3 >= 3, 1 < 4)
        assert_eq!(m.hi_norm, 3);
        assert!((m.hi_annual - 0.5).abs() < 1e-6);
        // annual_rates desc = [5.0, 3.0, 2.4, 1.0] -> ha_index = 2 (5.0>=1, 3.0>=2, 2.4<3)
        assert_eq!(m.ha_index, 2);
        // Mean author-normalized citations per paper = (30 + 6 + 3 + 1) / 4 = 10.0
        assert!((m.cites_per_author - 10.0).abs() < 1e-6);
        assert!((m.authors_per_paper - 2.0).abs() < 1e-6);
        assert!((m.cites_per_paper - 13.25).abs() < 1e-6);
    }

    #[test]
    fn author_counter_handles_last_comma_initials() {
        assert_eq!(count_paper_authors("Shannon, C. E."), 1);
        assert_eq!(count_paper_authors("Shannon, Claude"), 1);
        assert_eq!(count_paper_authors("Smith, Alice"), 1);
        assert_eq!(count_paper_authors("Shannon, Claude E."), 1);
        assert_eq!(count_paper_authors("Shannon, Claude and Weaver, Warren"), 2);
        assert_eq!(count_paper_authors("Shannon, Claude, Weaver, Warren"), 2);
        assert_eq!(count_paper_authors("A. Smith, Y. Li"), 2);
        assert_eq!(count_paper_authors("X. A, Y. B, Z. C, W. D, V. E"), 5);
        assert_eq!(count_paper_authors("A. X, B. Y, C. Z, et al."), 4);
        assert_eq!(count_paper_authors("Smith, Alice; Jones, Bob"), 2);
        assert_eq!(count_paper_authors("Smith, A.; Jones, B.; Lee, C."), 3);
        assert_eq!(count_paper_authors("A. Smith, B. Jones"), 2);
        assert_eq!(count_paper_authors("Alice Smith, Bob Jones"), 2);
        assert_eq!(count_paper_authors("A. B. C. Smith"), 1);
        assert_eq!(count_paper_authors("Alice Smith and Bob Jones"), 2);
        assert_eq!(count_paper_authors(""), 1);
        assert_eq!(count_paper_authors("   "), 1);
        assert_eq!(count_paper_authors("C. E. Shannon, W. Weaver"), 2);
        assert_eq!(count_paper_authors("A. Vaswani, N. Shazeer, N. Parmar et al."), 4);
    }

    #[test]
    fn author_counter_handles_particle_and_multiword_family_given_surnames() {
        // Single particle / compound "Family, Given" authors
        assert_eq!(count_paper_authors("de la Cruz, Juan"), 1);
        assert_eq!(count_paper_authors("de la Cruz, Juan Carlos"), 1);
        assert_eq!(count_paper_authors("van Rossum, Guido"), 1);
        assert_eq!(count_paper_authors("van der Waals, Johannes D."), 1);
        assert_eq!(count_paper_authors("von Neumann, John"), 1);
        assert_eq!(count_paper_authors("St. Pierre, Georges"), 1);
        assert_eq!(count_paper_authors("St. Pierre, Georges E."), 1);
        assert_eq!(count_paper_authors("del Rio, Carlos"), 1);
        assert_eq!(count_paper_authors("de los Santos, Maria"), 1);
        assert_eq!(count_paper_authors("de las Casas, Bartolome"), 1);
        assert_eq!(count_paper_authors("van den Berg, Pieter"), 1);
        assert_eq!(count_paper_authors("van de Kamp, Peter"), 1);
        assert_eq!(count_paper_authors("von der Leyen, Ursula"), 1);
        assert_eq!(count_paper_authors("von dem Bussche, Axel"), 1);
        assert_eq!(count_paper_authors("da Vinci, Leonardo"), 1);
        assert_eq!(count_paper_authors("do Nascimento, Edson"), 1);
        assert_eq!(count_paper_authors("dos Santos, Jose"), 1);
        assert_eq!(count_paper_authors("das Neves, Antonio"), 1);
        assert_eq!(count_paper_authors("di Matteo, Roberto"), 1);
        assert_eq!(count_paper_authors("du Bois, W. E. B."), 1);
        assert_eq!(count_paper_authors("Le Guin, Ursula K."), 1);
        assert_eq!(count_paper_authors("La Forge, Geordi"), 1);
        assert_eq!(count_paper_authors("San Martin, Jose"), 1);
        assert_eq!(count_paper_authors("Santa Cruz, Alonso"), 1);
        assert_eq!(count_paper_authors("al Khwarizmi, Muhammad"), 1);
        assert_eq!(count_paper_authors("el Baz, Farouk"), 1);
        assert_eq!(count_paper_authors("ben Gurion, David"), 1);
        assert_eq!(count_paper_authors("bin Laden, Osama"), 1);
        assert_eq!(count_paper_authors("ibn Sina, Abu"), 1);
        assert_eq!(count_paper_authors("Abu Bakr, Abdullah"), 1);
        assert_eq!(count_paper_authors("Mac Leod, John"), 1);
        assert_eq!(count_paper_authors("Mc Donald, Ronald"), 1);
        assert_eq!(count_paper_authors("ter Haar, Dirk"), 1);
        assert_eq!(count_paper_authors("ten Kate, Jan"), 1);
        assert_eq!(count_paper_authors("op den Velde, Jan"), 1);
        assert_eq!(count_paper_authors("O'Connor, Mary"), 1);
        assert_eq!(count_paper_authors("Li, Wei; Zhang, Min"), 2);

        // Alternating multi-author "Family, Given" with particle surnames
        assert_eq!(count_paper_authors("de la Cruz, Juan, O'Connor, Mary"), 2);
        assert_eq!(count_paper_authors("van Rossum, Guido, Stroustrup, Bjarne"), 2);
        assert_eq!(
            count_paper_authors("van der Waals, Johannes D., von Neumann, John"),
            2
        );
        assert_eq!(
            count_paper_authors("St. Pierre, Georges, du Bois, W. E. B."),
            2
        );
        assert_eq!(
            count_paper_authors("de la Cruz, J., O'Connor, M., Stroustrup, B."),
            3
        );
        assert_eq!(
            count_paper_authors("de la Cruz, Juan, O'Connor, Mary, et al."),
            3
        );

        // Non-regression on "Given Family" multi-author lists (including particles inside Given Family)
        assert_eq!(count_paper_authors("A. Smith, Y. Li"), 2);
        assert_eq!(count_paper_authors("Alice Smith, Bob Jones"), 2);
        assert_eq!(count_paper_authors("A. Smith, B. Jones"), 2);
        assert_eq!(count_paper_authors("X. A, Y. B, Z. C, W. D, V. E"), 5);
        assert_eq!(count_paper_authors("A. X, B. Y, C. Z, et al."), 4);
        assert_eq!(count_paper_authors("Juan de la Cruz, Mary O'Connor"), 2);
        assert_eq!(count_paper_authors("Guido van Rossum, Bjarne Stroustrup"), 2);
        assert_eq!(count_paper_authors("Ben Smith, Alice Jones"), 2);
        assert_eq!(count_paper_authors("Al Gore, Bill Clinton"), 2);
    }

    #[test]
    fn pop_metrics_ten_paper_cites_per_author_mean_handles_mixed_author_formats() {
        let recs = vec![
            mk_paper("p1", "Shannon, Claude", 2015, 18),                 // 1 author -> 18.0
            mk_paper("p2", "Smith, Alice", 2016, 18),                    // 1 author -> 18.0
            mk_paper("p3", "A. Smith, Y. Li", 2017, 36),                 // 2 authors -> 18.0
            mk_paper("p4", "X. A, Y. B, Z. C, W. D, V. E", 2018, 90),    // 5 authors -> 18.0
            mk_paper("p5", "A. X, B. Y, C. Z, et al.", 2019, 72),        // 4 authors -> 18.0
            mk_paper("p6", "Shannon, C. E.", 2020, 18),                  // 1 author -> 18.0
            mk_paper("p7", "Smith, Alice; Jones, Bob", 2021, 36),        // 2 authors -> 18.0
            mk_paper("p8", "Alice Smith, Bob Jones", 2022, 36),          // 2 authors -> 18.0
            mk_paper("p9", "Alice Smith and Bob Jones", 2023, 36),       // 2 authors -> 18.0
            mk_paper("p10", "A. B. C. Smith", 2024, 18),                 // 1 author -> 18.0
        ];
        let m = compute_pop_citation_metrics_records(&recs, 2026);
        assert_eq!(m.total_papers, 10);
        assert!((m.cites_per_author - 18.0).abs() < 1e-9, "got {}", m.cites_per_author);
        assert!((m.authors_per_paper - 2.1).abs() < 1e-9, "got {}", m.authors_per_paper);
    }
}

#[cfg(test)]
mod cd5_hits_tests {
    use super::*;
    use crate::models::OaStatus;

    fn mk_node(id: &str, year: i32) -> PaperRecord {
        PaperRecord {
            id: id.to_string(),
            title: id.to_string(),
            authors: "A. Researcher".to_string(),
            year,
            venue: "Test Venue".to_string(),
            disc: Discipline::SCI,
            doi: String::new(),
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            source_url: String::new(),
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Unknown,
            external_citations: None,
            abstract_text: String::new(),
            terms: Vec::new(),
            concepts: Vec::new(),
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: Default::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        }
    }

    fn edge(src: &str, dst: &str) -> CitationEdge {
        CitationEdge {
            source: src.to_string(),
            target: dst.to_string(),
            provenance: "test".to_string(),
        }
    }

    #[test]
    fn disruption_cd5_respects_5yr_window_and_zero_year_edges() {
        let papers = vec![
            mk_node("ref_a", 2005),
            mk_node("focal", 2010),
            mk_node("same_yr", 2010),   // t_p excluded from CD5
            mk_node("in_ni_1", 2011),   // t_p + 1: cites focal only -> n_i
            mk_node("in_nj_1", 2015),   // t_p + 5: cites focal + ref_a -> n_j
            mk_node("in_nk_1", 2013),   // t_p + 3: cites ref_a only -> n_k
            mk_node("pre_focal", 2008), // < t_p: cites ref_a -> excluded
            mk_node("late_ni", 2016),   // t_p + 6: cites focal only -> excluded from CD5, included in all-time CD
            mk_node("zero_yr", 0),      // year == 0 -> CD5 == 0.0
        ];
        let edges = vec![
            edge("focal", "ref_a"),
            edge("same_yr", "focal"),
            edge("in_ni_1", "focal"),
            edge("in_nj_1", "focal"),
            edge("in_nj_1", "ref_a"),
            edge("in_nk_1", "ref_a"),
            edge("pre_focal", "ref_a"),
            edge("late_ni", "focal"),
            edge("in_ni_1", "zero_yr"),
        ];
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let focal = analytics.papers.iter().find(|p| p.record.id == "focal").unwrap();
        // CD5: n_i = 1 (in_ni_1), n_j = 1 (in_nj_1), n_k = 1 (in_nk_1) => (1 - 1)/3 = 0.0
        assert!((focal.disruption_cd5 - 0.0).abs() < 1e-9, "got {}", focal.disruption_cd5);
        // All-time CD: n_i = 3 (same_yr, in_ni_1, late_ni), n_j = 1, n_k = 1 => (3 - 1)/5 = 0.4
        assert!((focal.disruption_cd - 0.4).abs() < 1e-9, "got {}", focal.disruption_cd);

        let zero = analytics.papers.iter().find(|p| p.record.id == "zero_yr").unwrap();
        assert_eq!(zero.disruption_cd5, 0.0);
        assert_eq!(zero.disruption_cd, 0.0);
    }

    #[test]
    fn disruption_cd5_removes_512_citer_cap() {
        let mut papers = Vec::new();
        let mut edges = Vec::new();
        papers.push(mk_node("ref_hub", 2000));
        papers.push(mk_node("focal", 2010));
        edges.push(edge("focal", "ref_hub"));

        // 600 pre-focal citers of ref_hub (must not trigger 512 cap or hide the 600 in-window citers)
        for k in 0..600 {
            let id = format!("pre_{}", k);
            papers.push(mk_node(&id, 2005));
            edges.push(edge(&id, "ref_hub"));
        }
        // 600 in-window citers of ref_hub only -> n_k = 600
        for k in 0..600 {
            let id = format!("nk_{}", k);
            papers.push(mk_node(&id, 2012));
            edges.push(edge(&id, "ref_hub"));
        }
        // 100 in-window citers of focal only -> n_i = 100
        for k in 0..100 {
            let id = format!("ni_{}", k);
            papers.push(mk_node(&id, 2014));
            edges.push(edge(&id, "focal"));
        }

        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let focal = analytics.papers.iter().find(|p| p.record.id == "focal").unwrap();
        let expected = 100.0 / 700.0;
        assert!(
            (focal.disruption_cd5 - expected).abs() < 1e-9,
            "expected {}, got {}",
            expected,
            focal.disruption_cd5
        );
    }

    #[test]
    fn hits_on_dag_handles_empty_zero_edges_and_hub_authority_duality() {
        let (h0, a0) = compute_hits_on_dag(&[], &[], 30);
        assert!(h0.is_empty() && a0.is_empty());

        let (hz, az) = compute_hits_on_dag(&[vec![], vec![]], &[vec![], vec![]], 30);
        assert_eq!(hz, vec![0.0, 0.0]);
        assert_eq!(az, vec![0.0, 0.0]);

        // Bipartite DAG: node 0 (hub) cites 1 and 2 (authorities)
        let papers = vec![mk_node("hub", 2022), mk_node("auth1", 2010), mk_node("auth2", 2012)];
        let edges = vec![edge("hub", "auth1"), edge("hub", "auth2")];
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let (hh, aa) = compute_hits_on_dag(&analytics.out_adj, &analytics.in_adj, 30);
        assert!(hh[0] > 0.99);
        assert!(aa[1] > 0.70 && aa[2] > 0.70);
        let h_norm: f64 = hh.iter().map(|x| x * x).sum::<f64>().sqrt();
        let a_norm: f64 = aa.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((h_norm - 1.0).abs() < 1e-9);
        assert!((a_norm - 1.0).abs() < 1e-9);
        for (i, pm) in analytics.papers.iter().enumerate() {
            assert!((pm.hits_hub - hh[i]).abs() < 1e-12);
            assert!((pm.hits_authority - aa[i]).abs() < 1e-12);
        }
    }
}
