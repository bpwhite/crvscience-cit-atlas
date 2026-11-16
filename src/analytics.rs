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

            // Sparse O(d_in + d_out * avg_in) exact Disruption Index CD_i
            let refs_set: HashSet<usize> = out_adj[i].iter().copied().collect();
            let (mut ni, mut nj, mut nk) = (0usize, 0usize, 0usize);
            if refs_set.is_empty() {
                ni = in_deg;
            } else {
                let mut citer_set: HashSet<usize> = HashSet::with_capacity(in_adj[i].len());
                for &w in &in_adj[i] {
                    if w != i && active_records[w].year >= pub_yr {
                        citer_set.insert(w);
                        let cites_ref = out_adj[w].iter().any(|r| refs_set.contains(r));
                        if cites_ref {
                            nj += 1;
                        } else {
                            ni += 1;
                        }
                    }
                }
                if in_deg > 0 || out_deg <= 24 {
                    let mut seen_nk: HashSet<usize> = HashSet::new();
                    for &r_idx in &out_adj[i] {
                        for &w in &in_adj[r_idx].iter().take(512).copied().collect::<Vec<_>>() {
                            if w != i
                                && active_records[w].year >= pub_yr
                                && !citer_set.contains(&w)
                                && seen_nk.insert(w)
                            {
                                nk += 1;
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
