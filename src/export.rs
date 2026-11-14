use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::graph_viewer::{
    compute_preset_positions, node_radius_for_encoding, node_rgb_for_encoding,
    NodeColorEncoding, NodeSizeEncoding, ViewerLayoutPreset,
};
use crate::bio_entity_bridges::extract_uniprot_pdb_cross_edges;
use crate::models::{
    BioEntityKind, CitationEdge, CorpusAnalytics, DataCiteArtifactKind, Discipline, PaperRecord,
};
use crate::rate_limiter::{
    snapshot_global_audit_log, snapshot_global_policies, CrawlAuditEntry, DomainBucketPolicy,
};

/// Strict RFC 4180 CSV field escaping: wraps any field containing `,`, `"`, `\n`, `\r`,
/// or leading/trailing whitespace in double quotes and doubles internal quotes (`""`).
pub fn csv_escape(field: &str) -> String {
    let needs_quotes = field.contains(',')
        || field.contains('"')
        || field.contains('\n')
        || field.contains('\r')
        || field.starts_with(' ')
        || field.ends_with(' ');
    if needs_quotes {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

pub fn format_bibtex_corpus(papers: &[PaperRecord]) -> String {
    let mut out = String::new();
    for p in papers {
        let key: String = p
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        out.push_str(&format!(
            "@article{{{},\n",
            if key.is_empty() { "paper" } else { &key }
        ));
        out.push_str(&format!(
            "  title = {{{}}},\n",
            p.title.replace('{', "").replace('}', "")
        ));
        out.push_str(&format!(
            "  author = {{{}}},\n",
            p.authors.replace(", ", " and ")
        ));
        out.push_str(&format!("  year = {{{}}},\n", p.year));
        out.push_str(&format!("  journal = {{{}}},\n", p.venue));
        if !p.doi.is_empty() {
            out.push_str(&format!("  doi = {{{}}},\n", p.doi));
        }
        if !p.source_url.is_empty() {
            out.push_str(&format!("  url = {{{}}},\n", p.source_url));
        }
        if p.retraction.status.is_flagged() {
            out.push_str(&format!(
                "  note = {{Integrity status: {} ({})}},\n",
                p.retraction.status.as_str(),
                p.retraction.notice_date
            ));
        }
        if let Some(ref local) = p.local_pdf_path {
            out.push_str(&format!("  file = {{{}}},\n", local));
        }
        out.push_str("}\n\n");
    }
    out
}

/// 1. Paper-level scientometrics, Retraction Watch integrity, & DataCite bridge CSV (`papers_metrics.csv`, 53 columns).
pub fn export_metrics_csv(analytics: &CorpusAnalytics, path: &Path) -> Result<(), String> {
    let mut lines = Vec::with_capacity(analytics.papers.len() + 1);
    lines.push(
        "id,year,discipline,louvain_comm,infomap_module,infomap_flow_pct,reclassified_louvain,reclassified_infomap,authors,title,venue,doi,arxiv_id,pmcid,oa_status,retraction_status,retraction_source,retraction_date,retraction_notice_doi,retraction_watch_record_id,retraction_reasons,post_retraction_cites,cites_retracted_count,datacite_dataset_count,datacite_software_count,datacite_shared_peers,datacite_total_downloads,datacite_artifact_dois,uniprot_bridge_count,pdb_bridge_count,epmc_annotation_count,shared_bio_entity_peers,uniprot_accessions,pdb_structure_ids,bio_entity_bridges,in_citations,out_references,undir_degree,external_citations,v_exp,v_window,accel_dv_dt,immed_5y_pct,beauty_B,awaken_year,hibernation_years,disruption_CD,brandes_CB_undir,brandes_CB_dir,participation_P,louvain_P,infomap_P,within_z,burt_C,bridge_score,guimera_role,oa_pdf_url,local_pdf_path,pdf_sha256,source_url"
            .to_string(),
    );

    for p in &analytics.papers {
        let r = &p.record;
        let ret = &r.retraction;
        lines.push(format!(
            "{},{},{},L{},M{},{:.3},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.4},{:.4},{:.4},{:.1},{:.2},{},{},{:.4},{:.5},{:.5},{:.4},{:.4},{:.4},{:.3},{:.4},{:.4},{},{},{},{},{}",
            csv_escape(&r.id),
            r.year,
            r.disc.code(),
            p.louvain_comm + 1,
            p.infomap_comm + 1,
            p.infomap_flow * 100.0,
            p.reclassified_louvain,
            p.reclassified_infomap,
            csv_escape(&r.authors),
            csv_escape(&r.title),
            csv_escape(&r.venue),
            csv_escape(&r.doi),
            csv_escape(&r.arxiv_id),
            csv_escape(&r.pmcid),
            r.oa_status.as_str(),
            ret.status.as_str(),
            csv_escape(&ret.source),
            csv_escape(&ret.notice_date),
            csv_escape(&ret.notice_doi),
            csv_escape(&ret.record_id),
            csv_escape(&ret.reasons.join(";")),
            p.post_retraction_cites,
            p.cites_retracted_count,
            p.dataset_bridge_count,
            p.software_bridge_count,
            p.shared_artifact_peers,
            r.total_artifact_downloads(),
            csv_escape(&r.formatted_artifact_dois()),
            p.uniprot_bridge_count,
            p.pdb_bridge_count,
            p.epmc_annotation_count,
            p.shared_bio_entity_peers,
            csv_escape(&r.formatted_uniprot_accessions()),
            csv_escape(&r.formatted_pdb_ids()),
            csv_escape(&r.formatted_bio_entities()),
            p.in_deg,
            p.out_deg,
            p.undir_deg,
            r.external_citations.map(|c| c.to_string()).unwrap_or_default(),
            p.v_exp,
            p.v_win,
            p.accel,
            p.immed_5y_pct,
            p.beauty_b,
            p.awaken_year,
            p.hibernation_years,
            p.disruption_cd,
            p.cb_undir,
            p.cb_dir,
            p.part_p,
            p.louvain_part_p,
            p.infomap_part_p,
            p.within_z,
            p.burt_c,
            p.bridge_score,
            csv_escape(p.role_code),
            csv_escape(&r.oa_pdf_url),
            csv_escape(r.local_pdf_path.as_deref().unwrap_or("")),
            csv_escape(r.pdf_sha256.as_deref().unwrap_or("")),
            csv_escape(&r.source_url),
        ));
    }

    fs::write(path, lines.join("\n")).map_err(|e| format!("Failed writing CSV {:?}: {}", path, e))
}

/// 2. Raw crawled/searched catalog CSV (`raw_harvest_catalog.csv`, round-trip importable with integrity & DataCite columns).
pub fn export_raw_papers_csv(papers: &[PaperRecord], path: &Path) -> Result<(), String> {
    let mut lines = Vec::with_capacity(papers.len() + 1);
    lines.push(
        "id,title,authors,year,venue,discipline,doi,arxiv_id,openalex_id,s2_id,pmcid,oa_status,external_citations,retraction_status,retraction_source,retraction_date,retraction_notice_doi,retraction_watch_record_id,retraction_reasons,datacite_dataset_dois,datacite_software_dois,datacite_artifact_dois,uniprot_accessions,pdb_structure_ids,epmc_annotations,bio_entity_bridges,oa_pdf_url,source_url,local_pdf_path,pdf_sha256,concepts,terms,abstract_text"
            .to_string(),
    );

    for r in papers {
        let ds_dois: String = r
            .datacite_links
            .iter()
            .filter(|l| l.artifact_kind == DataCiteArtifactKind::Dataset)
            .map(|l| l.artifact_doi.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let sw_dois: String = r
            .datacite_links
            .iter()
            .filter(|l| l.artifact_kind.is_software_like())
            .map(|l| l.artifact_doi.as_str())
            .collect::<Vec<_>>()
            .join(";");

        lines.push(format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            csv_escape(&r.id),
            csv_escape(&r.title),
            csv_escape(&r.authors),
            r.year,
            csv_escape(&r.venue),
            r.disc.code(),
            csv_escape(&r.doi),
            csv_escape(&r.arxiv_id),
            csv_escape(&r.openalex_id),
            csv_escape(&r.s2_id),
            csv_escape(&r.pmcid),
            r.oa_status.as_str(),
            r.external_citations.map(|c| c.to_string()).unwrap_or_default(),
            r.retraction.status.as_str(),
            csv_escape(&r.retraction.source),
            csv_escape(&r.retraction.notice_date),
            csv_escape(&r.retraction.notice_doi),
            csv_escape(&r.retraction.record_id),
            csv_escape(&r.retraction.reasons.join(";")),
            csv_escape(&ds_dois),
            csv_escape(&sw_dois),
            csv_escape(&r.formatted_artifact_dois()),
            csv_escape(&r.formatted_uniprot_accessions()),
            csv_escape(&r.formatted_pdb_ids()),
            csv_escape(&r.formatted_epmc_annotations()),
            csv_escape(&r.formatted_bio_entities()),
            csv_escape(&r.oa_pdf_url),
            csv_escape(&r.source_url),
            csv_escape(r.local_pdf_path.as_deref().unwrap_or("")),
            csv_escape(r.pdf_sha256.as_deref().unwrap_or("")),
            csv_escape(&r.concepts.join(";")),
            csv_escape(&r.terms.join(";")),
            csv_escape(&r.abstract_text),
        ));
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing raw catalog CSV {:?}: {}", path, e))
}

/// 3. Directed citation edge list with zombie-retraction & shared DataCite bridge columns (`citation_edges.csv`).
pub fn export_edges_csv(
    papers: &[PaperRecord],
    edges: &[CitationEdge],
    analytics: &CorpusAnalytics,
    path: &Path,
) -> Result<(), String> {
    let mut id_to_metrics = HashMap::new();
    for pm in &analytics.papers {
        id_to_metrics.insert(pm.record.id.as_str(), pm);
    }
    let mut id_to_raw = HashMap::new();
    for pr in papers {
        id_to_raw.insert(pr.id.as_str(), pr);
    }
    let mut conduit_lookup: HashMap<(&str, &str), (f64, f64)> = HashMap::new();
    for c in &analytics.conduits {
        conduit_lookup.insert((c.source_id.as_str(), c.target_id.as_str()), (c.eb_undir, c.eb_dir));
    }

    let mut lines = Vec::with_capacity(edges.len() + 1);
    lines.push(
        "source_id,target_id,source_cite,target_cite,source_year,target_year,citation_lag_years,source_disc,target_disc,cross_discipline,source_louvain,target_louvain,cross_louvain,source_infomap,target_infomap,eb_undir,eb_dir,source_retraction_status,target_retraction_status,is_post_retraction_zombie_cite,shared_datacite_artifact_doi,shared_datacite_kind,shared_bio_entity_id,shared_bio_entity_kind,provenance"
            .to_string(),
    );

    for e in edges {
        let s_raw = id_to_raw.get(e.source.as_str());
        let t_raw = id_to_raw.get(e.target.as_str());
        let s_met = id_to_metrics.get(e.source.as_str());
        let t_met = id_to_metrics.get(e.target.as_str());

        let s_yr = s_raw.map(|p| p.year).unwrap_or(0);
        let t_yr = t_raw.map(|p| p.year).unwrap_or(0);
        let lag = s_yr - t_yr;
        let s_disc = s_raw.map(|p| p.disc.code()).unwrap_or("INF");
        let t_disc = t_raw.map(|p| p.disc.code()).unwrap_or("INF");
        let cross_disc = s_disc != t_disc;

        let s_louv = s_met.map(|m| format!("L{}", m.louvain_comm + 1)).unwrap_or_default();
        let t_louv = t_met.map(|m| format!("L{}", m.louvain_comm + 1)).unwrap_or_default();
        let cross_louv = !s_louv.is_empty() && !t_louv.is_empty() && s_louv != t_louv;

        let s_info = s_met.map(|m| format!("M{}", m.infomap_comm + 1)).unwrap_or_default();
        let t_info = t_met.map(|m| format!("M{}", m.infomap_comm + 1)).unwrap_or_default();
        let (eb_u, eb_d) = conduit_lookup
            .get(&(e.source.as_str(), e.target.as_str()))
            .copied()
            .unwrap_or((0.0, 0.0));

        let s_ret = s_raw
            .map(|p| p.retraction.status.as_str())
            .unwrap_or("unscreened");
        let t_ret = t_raw
            .map(|p| p.retraction.status.as_str())
            .unwrap_or("unscreened");
        let is_zombie = t_raw
            .map(|tp| {
                tp.retraction.status.is_retracted_or_withdrawn()
                    && s_yr >= tp.retraction.notice_year.unwrap_or(tp.year)
            })
            .unwrap_or(false);

        let (mut shared_doi, mut shared_kind) = (String::new(), String::new());
        if let Some(rest) = e.provenance.strip_prefix("datacite_shared_") {
            if let Some((k_str, d_str)) = rest.split_once(':') {
                shared_kind = k_str.to_string();
                shared_doi = d_str.to_string();
            }
        } else if let (Some(sp), Some(tp)) = (s_raw, t_raw) {
            let tgt_art_set: HashSet<&str> = tp
                .datacite_links
                .iter()
                .map(|l| l.artifact_doi.as_str())
                .collect();
            if let Some(found) = sp
                .datacite_links
                .iter()
                .find(|l| tgt_art_set.contains(l.artifact_doi.as_str()))
            {
                shared_doi = found.artifact_doi.clone();
                shared_kind = found.artifact_kind.as_str().to_string();
            }
        }

        let (mut shared_bio_id, mut shared_bio_kind) = (String::new(), String::new());
        if let Some(rest) = e.provenance.strip_prefix("bio_shared_") {
            if let Some((k_str, id_str)) = rest.split_once(':') {
                shared_bio_kind = k_str.to_string();
                shared_bio_id = id_str.to_string();
            }
        } else if let (Some(sp), Some(tp)) = (s_raw, t_raw) {
            let tgt_bio: HashSet<(&str, String)> = tp
                .bio_entities
                .iter()
                .map(|b| (b.kind.as_str(), b.entity_id.to_ascii_uppercase()))
                .collect();
            if let Some(found) = sp.bio_entities.iter().find(|b| {
                tgt_bio.contains(&(b.kind.as_str(), b.entity_id.to_ascii_uppercase()))
            }) {
                shared_bio_id = found.entity_id.clone();
                shared_bio_kind = found.kind.as_str().to_string();
            }
        }

        lines.push(format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.5},{:.5},{},{},{},{},{},{},{},{}",
            csv_escape(&e.source),
            csv_escape(&e.target),
            csv_escape(&s_raw.map(|p| p.short_cite()).unwrap_or_default()),
            csv_escape(&t_raw.map(|p| p.short_cite()).unwrap_or_default()),
            s_yr,
            t_yr,
            lag,
            s_disc,
            t_disc,
            cross_disc,
            s_louv,
            t_louv,
            cross_louv,
            s_info,
            t_info,
            eb_u,
            eb_d,
            s_ret,
            t_ret,
            is_zombie,
            csv_escape(&shared_doi),
            csv_escape(&shared_kind),
            csv_escape(&shared_bio_id),
            csv_escape(&shared_bio_kind),
            csv_escape(&e.provenance),
        ));
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing edges CSV {:?}: {}", path, e))
}

/// 4. Tidy long-format annual citation velocity & Sleeping Beauty trajectories (`annual_trajectories.csv`).
pub fn export_annual_trajectories_csv(analytics: &CorpusAnalytics, path: &Path) -> Result<(), String> {
    let mut lines = Vec::new();
    lines.push(
        "paper_id,short_cite,discipline,retraction_status,louvain_comm,infomap_module,pub_year,calendar_year,age_years,annual_citations,cumulative_citations,ke_reference_line,v_window,v_exp,accel_dv_dt"
            .to_string(),
    );

    for p in &analytics.papers {
        let short_c = p.record.short_cite();
        let ret_s = p.record.retraction.status.as_str();
        for offset in 0..analytics.year_span {
            let cal_year = analytics.min_year + (offset as i32);
            if cal_year < p.record.year {
                continue;
            }
            let age = cal_year - p.record.year;
            let c_yr = p.annual_counts.get(offset).copied().unwrap_or(0.0);
            let cum_c = p.cum_series.get(offset).copied().unwrap_or(0.0);
            let ke_ref = p.ke_ref_line.get(offset).copied().unwrap_or(0.0);
            let v_w = p.v_win_series.get(offset).copied().unwrap_or(0.0);
            let v_e = p.v_exp_series.get(offset).copied().unwrap_or(0.0);
            let acc = p.accel_series.get(offset).copied().unwrap_or(0.0);

            lines.push(format!(
                "{},{},{},{},L{},M{},{},{},{},{:.2},{:.2},{:.3},{:.4},{:.4},{:.4}",
                csv_escape(&p.record.id),
                csv_escape(&short_c),
                p.record.disc.code(),
                ret_s,
                p.louvain_comm + 1,
                p.infomap_comm + 1,
                p.record.year,
                cal_year,
                age,
                c_yr,
                cum_c,
                ke_ref,
                v_w,
                v_e,
                acc,
            ));
        }
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing annual trajectories CSV {:?}: {}", path, e))
}

/// 5. Sliding multi-year window Shannon MI, NMI, and within-field drift (`topic_drift_windows.csv`).
pub fn export_topic_drift_csv(analytics: &CorpusAnalytics, path: &Path) -> Result<(), String> {
    let mut lines = Vec::with_capacity(analytics.windows.len() + 1);
    lines.push(
        "window_idx,window_label,start_year,end_year,active_pubs,active_citations,h_disc_bits,h_topic_bits,mi_bits,nmi,mean_pairwise_jsd,pubs_INF,pubs_NET,pubs_EVO,pubs_ML,pubs_EPI,pubs_SCI,drift_INF,drift_NET,drift_EVO,drift_ML,drift_EPI,drift_SCI"
            .to_string(),
    );

    for w in &analytics.windows {
        let mut pair_sum = 0.0;
        let mut pair_cnt = 0usize;
        for i in 0..6 {
            for j in (i + 1)..6 {
                pair_sum += w.jsd_matrix[i][j];
                pair_cnt += 1;
            }
        }
        let mean_jsd = if pair_cnt > 0 {
            pair_sum / (pair_cnt as f64)
        } else {
            0.0
        };

        lines.push(format!(
            "{},{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
            w.w_idx,
            csv_escape(&w.label),
            w.start_year,
            w.end_year,
            w.active_pubs,
            w.active_cites,
            w.h_disc,
            w.h_topic,
            w.mi_bits,
            w.nmi,
            mean_jsd,
            w.pub_counts_by_disc[0],
            w.pub_counts_by_disc[1],
            w.pub_counts_by_disc[2],
            w.pub_counts_by_disc[3],
            w.pub_counts_by_disc[4],
            w.pub_counts_by_disc[5],
            w.within_drift[0],
            w.within_drift[1],
            w.within_drift[2],
            w.within_drift[3],
            w.within_drift[4],
            w.within_drift[5],
        ));
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing topic drift CSV {:?}: {}", path, e))
}

/// 6. Pairwise Jensen-Shannon divergence matrix across sliding windows (`pairwise_jsd_matrix.csv`).
pub fn export_jsd_matrix_csv(analytics: &CorpusAnalytics, path: &Path) -> Result<(), String> {
    let mut lines = Vec::new();
    lines.push(
        "window_idx,window_label,start_year,end_year,disc_a,disc_b,jsd_bits,delta_jsd_vs_prev"
            .to_string(),
    );

    for (idx, w) in analytics.windows.iter().enumerate() {
        for i in 0..6 {
            for j in (i + 1)..6 {
                let cur = w.jsd_matrix[i][j];
                let delta = if idx > 0 {
                    cur - analytics.windows[idx - 1].jsd_matrix[i][j]
                } else {
                    0.0
                };
                lines.push(format!(
                    "{},{},{},{},{},{},{:.5},{:.5}",
                    w.w_idx,
                    csv_escape(&w.label),
                    w.start_year,
                    w.end_year,
                    Discipline::ALL[i].code(),
                    Discipline::ALL[j].code(),
                    cur,
                    delta,
                ));
            }
        }
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing JSD matrix CSV {:?}: {}", path, e))
}

/// 7. Louvain modularity & Infomap flow communities summary CSV with retraction & DataCite aggregates (`communities_summary.csv`).
pub fn export_communities_csv(analytics: &CorpusAnalytics, path: &Path) -> Result<(), String> {
    let comm = &analytics.communities;
    let mut lines = Vec::new();
    lines.push(
        "partition_scheme,cluster_code,size,internal_edges,cut_edges,flow_pct,retracted_members,dataset_bridge_links,software_bridge_links,uniprot_bridge_links,pdb_bridge_links,epmc_annotation_links,dominant_disc,dominant_purity_pct,top_member_id,objective_metric,objective_value,nmi_vs_disc,ari_vs_disc,vi_bits_vs_disc"
            .to_string(),
    );

    for (cid, c) in comm.louvain.clusters.iter().enumerate() {
        let members: Vec<_> = analytics
            .papers
            .iter()
            .filter(|p| p.louvain_comm == cid)
            .collect();
        let ret_cnt = members
            .iter()
            .filter(|p| p.record.retraction.status.is_retracted_or_withdrawn())
            .count();
        let ds_cnt: usize = members.iter().map(|p| p.dataset_bridge_count).sum();
        let sw_cnt: usize = members.iter().map(|p| p.software_bridge_count).sum();
        let up_cnt: usize = members.iter().map(|p| p.uniprot_bridge_count).sum();
        let pdb_cnt: usize = members.iter().map(|p| p.pdb_bridge_count).sum();
        let epmc_cnt: usize = members.iter().map(|p| p.epmc_annotation_count).sum();

        lines.push(format!(
            "Louvain,{},{},{},{},{:.3},{},{},{},{},{},{},{},{:.2},{},Modularity_Q,{:.5},{:.4},{:.4},{:.4}",
            csv_escape(&c.code),
            c.size,
            c.internal_edges,
            c.cut_edges,
            c.flow_mass * 100.0,
            ret_cnt,
            ds_cnt,
            sw_cnt,
            up_cnt,
            pdb_cnt,
            epmc_cnt,
            c.dominant_disc.code(),
            c.dominant_purity * 100.0,
            csv_escape(&c.top_member_id),
            comm.louvain.modularity_q,
            comm.disc_vs_louvain.nmi,
            comm.disc_vs_louvain.adjusted_rand_index,
            comm.disc_vs_louvain.vi_bits,
        ));
    }

    for (mid, m) in comm.infomap.clusters.iter().enumerate() {
        let members: Vec<_> = analytics
            .papers
            .iter()
            .filter(|p| p.infomap_comm == mid)
            .collect();
        let ret_cnt = members
            .iter()
            .filter(|p| p.record.retraction.status.is_retracted_or_withdrawn())
            .count();
        let ds_cnt: usize = members.iter().map(|p| p.dataset_bridge_count).sum();
        let sw_cnt: usize = members.iter().map(|p| p.software_bridge_count).sum();
        let up_cnt: usize = members.iter().map(|p| p.uniprot_bridge_count).sum();
        let pdb_cnt: usize = members.iter().map(|p| p.pdb_bridge_count).sum();
        let epmc_cnt: usize = members.iter().map(|p| p.epmc_annotation_count).sum();

        lines.push(format!(
            "Infomap,{},{},{},{},{:.3},{},{},{},{},{},{},{},{:.2},{},MapEquation_L_bits,{:.5},{:.4},{:.4},{:.4}",
            csv_escape(&m.code),
            m.size,
            m.internal_edges,
            m.cut_edges,
            m.flow_mass * 100.0,
            ret_cnt,
            ds_cnt,
            sw_cnt,
            up_cnt,
            pdb_cnt,
            epmc_cnt,
            m.dominant_disc.code(),
            m.dominant_purity * 100.0,
            csv_escape(&m.top_member_id),
            comm.infomap.codelength_bits,
            comm.disc_vs_infomap.nmi,
            comm.disc_vs_infomap.adjusted_rand_index,
            comm.disc_vs_infomap.vi_bits,
        ));
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing communities CSV {:?}: {}", path, e))
}

/// 8. Dedicated relational Retraction Watch & DataCite Artifact Bridge ledger (`artifact_and_retraction_bridges.csv`).
pub fn export_integrity_and_datacite_csv(
    analytics: &CorpusAnalytics,
    path: &Path,
) -> Result<(), String> {
    // Count how many distinct corpus papers reference each DataCite artifact DOI
    let mut art_peer_counts: HashMap<&str, usize> = HashMap::new();
    for p in &analytics.papers {
        for lk in &p.record.datacite_links {
            if !lk.artifact_doi.is_empty() {
                *art_peer_counts.entry(lk.artifact_doi.as_str()).or_insert(0) += 1;
            }
        }
    }

    let mut lines = Vec::new();
    lines.push(
        "paper_id,paper_year,discipline,louvain_comm,infomap_module,paper_doi,entry_category,status_or_artifact_kind,relation_or_source,linked_doi,repository_or_rw_record_id,artifact_title_or_reasons,notice_date,post_retraction_cites,shared_corpus_papers,datacite_citations,datacite_views,datacite_downloads"
            .to_string(),
    );

    for p in &analytics.papers {
        let r = &p.record;
        let ret = &r.retraction;

        if ret.status.is_flagged() {
            lines.push(format!(
                "{},{},{},L{},M{},{},retraction_notice,{},{},{},{},{},{},{},0,0,0,0",
                csv_escape(&r.id),
                r.year,
                r.disc.code(),
                p.louvain_comm + 1,
                p.infomap_comm + 1,
                csv_escape(&r.doi),
                ret.status.as_str(),
                csv_escape(&ret.source),
                csv_escape(&ret.notice_doi),
                csv_escape(&ret.record_id),
                csv_escape(&ret.reasons.join(";")),
                csv_escape(&ret.notice_date),
                p.post_retraction_cites,
            ));
        }

        for lk in &r.datacite_links {
            let shared_n = art_peer_counts
                .get(lk.artifact_doi.as_str())
                .copied()
                .unwrap_or(1);
            lines.push(format!(
                "{},{},{},L{},M{},{},datacite_bridge,{},{},{},{},{},,0,{},{},{},{}",
                csv_escape(&r.id),
                r.year,
                r.disc.code(),
                p.louvain_comm + 1,
                p.infomap_comm + 1,
                csv_escape(&r.doi),
                lk.artifact_kind.as_str(),
                csv_escape(&lk.relation_type),
                csv_escape(&lk.artifact_doi),
                csv_escape(&lk.repository),
                csv_escape(&lk.artifact_title),
                shared_n,
                lk.citation_count,
                lk.view_count,
                lk.download_count,
            ));
        }
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing integrity & DataCite CSV {:?}: {}", path, e))
}

/// 9. Dedicated relational Europe PMC Annotations, UniProtKB Protein, & RCSB PDB Structure Bridge CSV (`epmc_uniprot_pdb_bridges.csv`).
pub fn export_bio_entity_bridges_csv(
    analytics: &CorpusAnalytics,
    path: &Path,
) -> Result<(), String> {
    let mut bio_peer_counts: HashMap<(BioEntityKind, String), usize> = HashMap::new();
    for p in &analytics.papers {
        for b in &p.record.bio_entities {
            if !b.entity_id.is_empty() {
                *bio_peer_counts
                    .entry((b.kind, b.entity_id.to_ascii_uppercase()))
                    .or_insert(0) += 1;
            }
        }
    }

    let mut lines = Vec::new();
    lines.push(
        "paper_id,paper_year,discipline,louvain_comm,infomap_module,paper_doi,pmcid,entity_id,entity_kind,entity_label,provider,article_section,mention_count,pdb_resolution_angstrom,linked_pdb_ids,shared_corpus_papers,entity_uri"
            .to_string(),
    );

    for p in &analytics.papers {
        let r = &p.record;
        for b in &r.bio_entities {
            let shared_n = bio_peer_counts
                .get(&(b.kind, b.entity_id.to_ascii_uppercase()))
                .copied()
                .unwrap_or(1);
            let res_str = b
                .resolution_angstrom
                .map(|v| format!("{:.2}", v))
                .unwrap_or_default();
            lines.push(format!(
                "{},{},{},L{},M{},{},{},{},{},{},{},{},{},{},{},{},{}",
                csv_escape(&r.id),
                r.year,
                r.disc.code(),
                p.louvain_comm + 1,
                p.infomap_comm + 1,
                csv_escape(&r.doi),
                csv_escape(&r.pmcid),
                csv_escape(&b.entity_id),
                b.kind.as_str(),
                csv_escape(&b.label),
                csv_escape(&b.provider),
                csv_escape(&b.section),
                b.mention_count,
                res_str,
                csv_escape(&b.linked_pdb_ids.join(";")),
                shared_n,
                csv_escape(&b.uri),
            ));
        }
    }

    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing bio-entity bridges CSV {:?}: {}", path, e))
}

/// 10. Per-request Crawl Telemetry & Rate-Limit Audit Log CSV (`crawl_telemetry_audit.csv`, 16 columns).
pub fn export_crawl_audit_csv(entries: &[CrawlAuditEntry], path: &Path) -> Result<(), String> {
    let mut lines = Vec::with_capacity(entries.len() + 1);
    lines.push(
        "seq_id,timestamp_utc,epoch_ms,subsystem,domain,method,outcome,http_status,token_wait_ms,latency_ms,bytes_received,tokens_remaining,retry_after_ms,robots_status,url,note"
            .to_string(),
    );
    for e in entries {
        lines.push(format!(
            "{},{},{},{},{},{},{},{},{},{},{},{:.3},{},{},{},{}",
            e.seq_id,
            csv_escape(&e.timestamp_utc),
            e.epoch_ms,
            csv_escape(&e.subsystem),
            csv_escape(&e.domain),
            csv_escape(&e.method),
            e.outcome.as_str(),
            e.http_status,
            e.token_wait_ms,
            e.latency_ms,
            e.bytes_received,
            e.tokens_remaining,
            e.retry_after_ms.map(|v| v.to_string()).unwrap_or_default(),
            csv_escape(&e.robots_status),
            csv_escape(&e.url),
            csv_escape(&e.note),
        ));
    }
    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing crawl telemetry audit CSV {:?}: {}", path, e))
}

/// Exports per-domain token-bucket rate-limit policies & cumulative counters (`domain_rate_policies.csv`, 19 columns).
pub fn export_domain_policies_csv(
    policies: &[DomainBucketPolicy],
    path: &Path,
) -> Result<(), String> {
    let mut lines = Vec::with_capacity(policies.len() + 1);
    lines.push(
        "domain,label,enabled,paused,refill_rps,burst_capacity,min_spacing_ms,jitter_ms,tokens_available,total_requests,ok_2xx,throttled_waits,total_wait_ms,http_429_count,http_403_captcha_count,robots_blocked_count,network_errors,ema_latency_ms,server_pool_note"
            .to_string(),
    );
    for p in policies {
        lines.push(format!(
            "{},{},{},{},{:.3},{:.2},{},{},{:.2},{},{},{},{},{},{},{},{},{:.1},{}",
            csv_escape(&p.domain),
            csv_escape(&p.label),
            p.enabled,
            p.paused,
            p.refill_rps,
            p.burst_capacity,
            p.min_spacing_ms,
            p.jitter_ms,
            p.tokens_available,
            p.total_requests,
            p.ok_2xx,
            p.throttled_waits,
            p.total_wait_ms,
            p.http_429_count,
            p.http_403_captcha_count,
            p.robots_blocked_count,
            p.network_errors,
            p.ema_latency_ms,
            csv_escape(&p.server_pool_note),
        ));
    }
    fs::write(path, lines.join("\n"))
        .map_err(|e| format!("Failed writing domain rate policies CSV {:?}: {}", path, e))
}

/// Export all 10 scientometric, retraction-integrity, DataCite, Europe PMC / UniProt / PDB, and Crawl Telemetry Audit CSV tables into `out_dir`.
pub fn export_all_csv_suite(
    papers: &[PaperRecord],
    edges: &[CitationEdge],
    analytics: &CorpusAnalytics,
    out_dir: &Path,
) -> Result<Vec<String>, String> {
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("Failed creating CSV export directory {:?}: {}", out_dir, e))?;

    let mut written = Vec::new();

    let p_metrics = out_dir.join("papers_metrics.csv");
    export_metrics_csv(analytics, &p_metrics)?;
    written.push("papers_metrics.csv".to_string());

    let p_raw = out_dir.join("raw_harvest_catalog.csv");
    export_raw_papers_csv(papers, &p_raw)?;
    written.push("raw_harvest_catalog.csv".to_string());

    let p_edges = out_dir.join("citation_edges.csv");
    export_edges_csv(papers, edges, analytics, &p_edges)?;
    written.push("citation_edges.csv".to_string());

    let p_bridges = out_dir.join("artifact_and_retraction_bridges.csv");
    export_integrity_and_datacite_csv(analytics, &p_bridges)?;
    written.push("artifact_and_retraction_bridges.csv".to_string());

    let p_bio = out_dir.join("epmc_uniprot_pdb_bridges.csv");
    export_bio_entity_bridges_csv(analytics, &p_bio)?;
    written.push("epmc_uniprot_pdb_bridges.csv".to_string());

    let p_traj = out_dir.join("annual_trajectories.csv");
    export_annual_trajectories_csv(analytics, &p_traj)?;
    written.push("annual_trajectories.csv".to_string());

    let p_drift = out_dir.join("topic_drift_windows.csv");
    export_topic_drift_csv(analytics, &p_drift)?;
    written.push("topic_drift_windows.csv".to_string());

    let p_jsd = out_dir.join("pairwise_jsd_matrix.csv");
    export_jsd_matrix_csv(analytics, &p_jsd)?;
    written.push("pairwise_jsd_matrix.csv".to_string());

    let p_comm = out_dir.join("communities_summary.csv");
    export_communities_csv(analytics, &p_comm)?;
    written.push("communities_summary.csv".to_string());

    let p_audit = out_dir.join("crawl_telemetry_audit.csv");
    let audit_rows = snapshot_global_audit_log();
    export_crawl_audit_csv(&audit_rows, &p_audit)?;
    written.push("crawl_telemetry_audit.csv".to_string());

    let _ = export_domain_policies_csv(&snapshot_global_policies(), &out_dir.join("domain_rate_policies.csv"));

    Ok(written)
}

pub fn export_bundle_json(
    papers: &[PaperRecord],
    edges: &[CitationEdge],
    analytics: &CorpusAnalytics,
    path: &Path,
) -> Result<(), String> {
    #[derive(Serialize)]
    struct ExportBundle<'a> {
        papers: &'a [PaperRecord],
        edges: &'a [CitationEdge],
        windows: &'a [crate::models::WindowTopicStats],
        conduits: &'a [crate::models::EdgeConduit],
        communities: &'a crate::community_detection::CommunityReport,
    }

    let payload = ExportBundle {
        papers,
        edges,
        windows: &analytics.windows,
        conduits: &analytics.conduits,
        communities: &analytics.communities,
    };
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("JSON serialize error: {}", e))?;
    fs::write(path, text).map_err(|e| format!("Failed writing JSON {:?}: {}", path, e))
}

pub fn export_synthesis_svg(analytics: &CorpusAnalytics, path: &Path) -> Result<(), String> {
    let w = 960.0f64;
    let h = 540.0f64;
    let pad_l = 72.0;
    let pad_r = 36.0;
    let pad_t = 52.0;
    let pad_b = 58.0;
    let pw = w - pad_l - pad_r;
    let ph = h - pad_t - pad_b;

    let max_x = analytics
        .papers
        .iter()
        .map(|p| p.v_exp)
        .fold(0.5f64, f64::max)
        * 1.12;
    let max_y = analytics
        .papers
        .iter()
        .map(|p| p.cb_undir)
        .fold(0.05f64, f64::max)
        * 1.12;

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"{}\" height=\"{}\">\n",
        w as i32, h as i32, w as i32, h as i32
    ));
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#0b1120\"/>\n");
    svg.push_str("<text x=\"72\" y=\"30\" fill=\"#f1f5f9\" font-family=\"Inter, sans-serif\" font-size=\"16\" font-weight=\"600\">CitAtlas — Citation Velocity v_exp vs. Brandes Geodesic Betweenness CB_u</text>\n");

    for g in 0..=4 {
        let gy = pad_t + ph * (g as f64) / 4.0;
        svg.push_str(&format!(
            "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#1e293b\" stroke-width=\"1\"/>\n",
            pad_l, gy, w - pad_r, gy
        ));
    }

    for p in &analytics.papers {
        let cx = pad_l + (p.v_exp / max_x) * pw;
        let cy = pad_t + ph - (p.cb_undir / max_y) * ph;
        let r = 5.0 + ((p.in_deg as f64).sqrt() * 2.5).min(11.0);
        let col = p.record.disc.hex_color();
        if p.record.retraction.status.is_retracted_or_withdrawn() {
            svg.push_str(&format!(
                "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"none\" stroke=\"#ef4444\" stroke-width=\"2.2\" stroke-dasharray=\"3,2\"/>\n",
                cx, cy, r + 4.5
            ));
        } else if p.beauty_b >= 10.0 {
            svg.push_str(&format!(
                "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"none\" stroke=\"#facc15\" stroke-width=\"1.8\" opacity=\"0.8\"/>\n",
                cx, cy, r + 3.5
            ));
        }
        svg.push_str(&format!(
            "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"{}\" fill-opacity=\"0.82\" stroke=\"#f8fafc\" stroke-width=\"0.8\"/>\n",
            cx, cy, r, col
        ));
        if p.cb_undir > max_y * 0.30 || p.v_exp > max_x * 0.42 {
            let clean_lbl = p
                .record
                .short_cite()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"#e2e8f0\" font-family=\"Inter, sans-serif\" font-size=\"11\">{}</text>\n",
                cx + r + 5.0,
                cy + 3.5,
                clean_lbl
            ));
        }
    }

    for (idx, d) in Discipline::ALL.iter().enumerate() {
        let lx = pad_l + (idx as f64) * 138.0;
        svg.push_str(&format!(
            "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"5\" fill=\"{}\"/>\n<text x=\"{:.1}\" y=\"{:.1}\" fill=\"#94a3b8\" font-family=\"monospace\" font-size=\"11\">{} ({})</text>\n",
            lx,
            h - 16.0,
            d.hex_color(),
            lx + 9.0,
            h - 12.0,
            d.code(),
            d.short_name().replace('&', "&amp;")
        ));
    }

    svg.push_str("</svg>\n");
    fs::write(path, svg).map_err(|e| format!("Failed writing SVG {:?}: {}", path, e))
}

/// Strict XML 1.0 escaping (strips control chars < 0x20 except \t, \n, \r).
pub fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for ch in input.chars() {
        if (ch as u32) < 0x20 && !matches!(ch, '\t' | '\n' | '\r') {
            continue;
        }
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Exports the citation network (and optional bipartite DataCite Dataset/Software artifact nodes)
/// in GraphML 1.0 format (`http://graphml.graphdrawing.org/xmlns`) with layout coordinates, RGB colors,
/// and all scientometric, community, and retraction attributes for Gephi, Cytoscape, igraph, & NetworkX.
pub fn export_graphml(
    papers: &[PaperRecord],
    edges: &[CitationEdge],
    analytics: &CorpusAnalytics,
    positions: Option<&HashMap<String, [f32; 2]>>,
    color_by: NodeColorEncoding,
    include_datacite_nodes: bool,
    path: &Path,
) -> Result<(), String> {
    let fallback_pos;
    let pos_map = match positions {
        Some(m) if !m.is_empty() => m,
        _ => {
            fallback_pos =
                compute_preset_positions(analytics, ViewerLayoutPreset::ForceAtlas2, color_by, "");
            &fallback_pos
        }
    };

    let max_cb = analytics.papers.iter().map(|p| p.cb_undir).fold(0.05, f64::max);
    let max_v = analytics.papers.iter().map(|p| p.v_exp).fold(0.5, f64::max);
    let max_b = analytics.papers.iter().map(|p| p.beauty_b).fold(5.0, f64::max);

    let mut out = String::with_capacity(papers.len() * 950 + edges.len() * 320 + 4096);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\"\n");
    out.push_str("         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n");
    out.push_str("         xsi:schemaLocation=\"http://graphml.graphdrawing.org/xmlns http://graphml.graphdrawing.org/xmlns/1.0/graphml.xsd\">\n");

    let node_keys = [
        ("n_label", "label", "string"),
        ("n_type", "node_type", "string"),
        ("n_title", "title", "string"),
        ("n_authors", "authors", "string"),
        ("n_year", "year", "int"),
        ("n_venue", "venue", "string"),
        ("n_disc", "discipline", "string"),
        ("n_doi", "doi", "string"),
        ("n_url", "source_url", "string"),
        ("n_louvain", "louvain_comm", "string"),
        ("n_infomap", "infomap_module", "string"),
        ("n_flow", "infomap_flow_pct", "double"),
        ("n_indeg", "in_citations", "int"),
        ("n_outdeg", "out_references", "int"),
        ("n_vexp", "v_exp", "double"),
        ("n_vwin", "v_window", "double"),
        ("n_accel", "accel_dv_dt", "double"),
        ("n_beauty", "beauty_b", "double"),
        ("n_awaken", "awaken_year", "int"),
        ("n_cd", "disruption_cd", "double"),
        ("n_cbu", "brandes_cb_undir", "double"),
        ("n_cbd", "brandes_cb_dir", "double"),
        ("n_part", "participation_p", "double"),
        ("n_role", "guimera_role", "string"),
        ("n_ret_status", "retraction_status", "string"),
        ("n_zombie", "post_retraction_cites", "int"),
        ("n_ds_cnt", "datacite_dataset_count", "int"),
        ("n_sw_cnt", "datacite_software_count", "int"),
        ("n_up_cnt", "uniprot_bridge_count", "int"),
        ("n_pdb_cnt", "pdb_bridge_count", "int"),
        ("n_epmc_cnt", "epmc_annotation_count", "int"),
        ("n_x", "x", "double"),
        ("n_y", "y", "double"),
        ("n_size", "size", "double"),
        ("n_r", "r", "int"),
        ("n_g", "g", "int"),
        ("n_b", "b", "int"),
        ("n_hex", "color_hex", "string"),
    ];
    for (kid, attr_name, attr_type) in node_keys {
        out.push_str(&format!(
            "  <key id=\"{}\" for=\"node\" attr.name=\"{}\" attr.type=\"{}\"/>\n",
            kid, attr_name, attr_type
        ));
    }

    let edge_keys = [
        ("e_prov", "provenance", "string"),
        ("e_lag", "citation_lag_years", "int"),
        ("e_xdisc", "cross_discipline", "boolean"),
        ("e_xlouv", "cross_louvain", "boolean"),
        ("e_ebu", "eb_undir", "double"),
        ("e_ebd", "eb_dir", "double"),
        ("e_weight", "weight", "double"),
        ("e_zombie", "is_post_retraction_zombie_cite", "boolean"),
        ("e_dcdoi", "shared_datacite_doi", "string"),
    ];
    for (kid, attr_name, attr_type) in edge_keys {
        out.push_str(&format!(
            "  <key id=\"{}\" for=\"edge\" attr.name=\"{}\" attr.type=\"{}\"/>\n",
            kid, attr_name, attr_type
        ));
    }

    out.push_str("  <graph id=\"CitAtlasNetwork\" edgedefault=\"directed\">\n");

    let mut id_to_metrics = HashMap::new();
    for p in &analytics.papers {
        id_to_metrics.insert(p.record.id.as_str(), p);
        let r = &p.record;
        let [nx, ny] = pos_map.get(&r.id).copied().unwrap_or([0.0, 0.0]);
        let gx = (nx as f64) * 500.0;
        let gy = -(ny as f64) * 500.0;
        let sz = node_radius_for_encoding(
            p,
            NodeSizeEncoding::BrandesBetweenness,
            max_cb,
            max_v,
            max_b,
        ) * 2.0;
        let [cr, cg, cb] = node_rgb_for_encoding(p, color_by, max_v, max_b);
        let hex = format!("#{:02x}{:02x}{:02x}", cr, cg, cb);

        out.push_str(&format!("    <node id=\"{}\">\n", xml_escape(&r.id)));
        out.push_str(&format!("      <data key=\"n_label\">{}</data>\n", xml_escape(&r.short_cite())));
        out.push_str("      <data key=\"n_type\">paper</data>\n");
        out.push_str(&format!("      <data key=\"n_title\">{}</data>\n", xml_escape(&r.title)));
        out.push_str(&format!("      <data key=\"n_authors\">{}</data>\n", xml_escape(&r.authors)));
        out.push_str(&format!("      <data key=\"n_year\">{}</data>\n", r.year));
        out.push_str(&format!("      <data key=\"n_venue\">{}</data>\n", xml_escape(&r.venue)));
        out.push_str(&format!("      <data key=\"n_disc\">{}</data>\n", r.disc.code()));
        out.push_str(&format!("      <data key=\"n_doi\">{}</data>\n", xml_escape(&r.doi)));
        out.push_str(&format!("      <data key=\"n_url\">{}</data>\n", xml_escape(&r.source_url)));
        out.push_str(&format!("      <data key=\"n_louvain\">L{}</data>\n", p.louvain_comm + 1));
        out.push_str(&format!("      <data key=\"n_infomap\">M{}</data>\n", p.infomap_comm + 1));
        out.push_str(&format!("      <data key=\"n_flow\">{:.4}</data>\n", p.infomap_flow * 100.0));
        out.push_str(&format!("      <data key=\"n_indeg\">{}</data>\n", p.in_deg));
        out.push_str(&format!("      <data key=\"n_outdeg\">{}</data>\n", p.out_deg));
        out.push_str(&format!("      <data key=\"n_vexp\">{:.4}</data>\n", p.v_exp));
        out.push_str(&format!("      <data key=\"n_vwin\">{:.4}</data>\n", p.v_win));
        out.push_str(&format!("      <data key=\"n_accel\">{:.4}</data>\n", p.accel));
        out.push_str(&format!("      <data key=\"n_beauty\">{:.2}</data>\n", p.beauty_b));
        out.push_str(&format!("      <data key=\"n_awaken\">{}</data>\n", p.awaken_year));
        out.push_str(&format!("      <data key=\"n_cd\">{:.4}</data>\n", p.disruption_cd));
        out.push_str(&format!("      <data key=\"n_cbu\">{:.5}</data>\n", p.cb_undir));
        out.push_str(&format!("      <data key=\"n_cbd\">{:.5}</data>\n", p.cb_dir));
        out.push_str(&format!("      <data key=\"n_part\">{:.4}</data>\n", p.part_p));
        out.push_str(&format!("      <data key=\"n_role\">{}</data>\n", xml_escape(p.role_code)));
        out.push_str(&format!("      <data key=\"n_ret_status\">{}</data>\n", r.retraction.status.as_str()));
        out.push_str(&format!("      <data key=\"n_zombie\">{}</data>\n", p.post_retraction_cites));
        out.push_str(&format!("      <data key=\"n_ds_cnt\">{}</data>\n", p.dataset_bridge_count));
        out.push_str(&format!("      <data key=\"n_sw_cnt\">{}</data>\n", p.software_bridge_count));
        out.push_str(&format!("      <data key=\"n_up_cnt\">{}</data>\n", p.uniprot_bridge_count));
        out.push_str(&format!("      <data key=\"n_pdb_cnt\">{}</data>\n", p.pdb_bridge_count));
        out.push_str(&format!("      <data key=\"n_epmc_cnt\">{}</data>\n", p.epmc_annotation_count));
        out.push_str(&format!("      <data key=\"n_x\">{:.2}</data>\n", gx));
        out.push_str(&format!("      <data key=\"n_y\">{:.2}</data>\n", gy));
        out.push_str(&format!("      <data key=\"n_size\">{:.2}</data>\n", sz));
        out.push_str(&format!("      <data key=\"n_r\">{}</data>\n", cr));
        out.push_str(&format!("      <data key=\"n_g\">{}</data>\n", cg));
        out.push_str(&format!("      <data key=\"n_b\">{}</data>\n", cb));
        out.push_str(&format!("      <data key=\"n_hex\">{}</data>\n", hex));
        out.push_str("    </node>\n");
    }

    // Optional bipartite DataCite Dataset / Software artifact nodes
    let mut emitted_artifacts: HashSet<String> = HashSet::new();
    let mut bipartite_edges: Vec<(String, String, String)> = Vec::new();
    if include_datacite_nodes {
        for p in &analytics.papers {
            let [px, py] = pos_map.get(&p.record.id).copied().unwrap_or([0.0, 0.0]);
            for lk in &p.record.datacite_links {
                if lk.artifact_doi.is_empty() {
                    continue;
                }
                let art_node_id = format!("datacite:{}", lk.artifact_doi);
                bipartite_edges.push((
                    p.record.id.clone(),
                    art_node_id.clone(),
                    lk.relation_type.clone(),
                ));
                if emitted_artifacts.insert(lk.artifact_doi.clone()) {
                    let ntype = if lk.artifact_kind.is_software_like() {
                        "software"
                    } else {
                        "dataset"
                    };
                    let (cr, cg, cb, hex) = if lk.artifact_kind.is_software_like() {
                        (167u8, 139u8, 250u8, "#a78bfa")
                    } else {
                        (45u8, 212u8, 191u8, "#2dd4bf")
                    };
                    out.push_str(&format!("    <node id=\"{}\">\n", xml_escape(&art_node_id)));
                    out.push_str(&format!("      <data key=\"n_label\">{}</data>\n", xml_escape(&lk.artifact_doi)));
                    out.push_str(&format!("      <data key=\"n_type\">{}</data>\n", ntype));
                    out.push_str(&format!("      <data key=\"n_title\">{}</data>\n", xml_escape(&lk.artifact_title)));
                    out.push_str(&format!("      <data key=\"n_authors\">{}</data>\n", xml_escape(&lk.repository)));
                    out.push_str(&format!("      <data key=\"n_year\">{}</data>\n", p.record.year));
                    out.push_str(&format!("      <data key=\"n_venue\">{}</data>\n", xml_escape(&lk.repository)));
                    out.push_str(&format!("      <data key=\"n_disc\">{}</data>\n", p.record.disc.code()));
                    out.push_str(&format!("      <data key=\"n_doi\">{}</data>\n", xml_escape(&lk.artifact_doi)));
                    out.push_str(&format!("      <data key=\"n_url\">https://doi.org/{}</data>\n", xml_escape(&lk.artifact_doi)));
                    out.push_str(&format!("      <data key=\"n_x\">{:.2}</data>\n", (px as f64) * 500.0 + 38.0));
                    out.push_str(&format!("      <data key=\"n_y\">{:.2}</data>\n", -(py as f64) * 500.0 + 28.0));
                    out.push_str("      <data key=\"n_size\">14.0</data>\n");
                    out.push_str(&format!("      <data key=\"n_r\">{}</data>\n", cr));
                    out.push_str(&format!("      <data key=\"n_g\">{}</data>\n", cg));
                    out.push_str(&format!("      <data key=\"n_b\">{}</data>\n", cb));
                    out.push_str(&format!("      <data key=\"n_hex\">{}</data>\n", hex));
                    out.push_str("    </node>\n");
                }
            }
            for b in &p.record.bio_entities {
                if b.entity_id.is_empty() {
                    continue;
                }
                let norm_id = b.entity_id.to_ascii_uppercase();
                let bio_node_id = format!("bio:{}:{}", b.kind.as_str(), norm_id);
                bipartite_edges.push((
                    p.record.id.clone(),
                    bio_node_id.clone(),
                    format!("{}:{}", b.provider, b.section),
                ));
                if emitted_artifacts.insert(bio_node_id.clone()) {
                    let [cr, cg, cb] = b.kind.rgb();
                    let hex = b.kind.hex_color();
                    out.push_str(&format!("    <node id=\"{}\">\n", xml_escape(&bio_node_id)));
                    out.push_str(&format!("      <data key=\"n_label\">{}</data>\n", xml_escape(&norm_id)));
                    out.push_str(&format!("      <data key=\"n_type\">{}</data>\n", b.kind.as_str()));
                    out.push_str(&format!("      <data key=\"n_title\">{}</data>\n", xml_escape(&b.label)));
                    out.push_str(&format!("      <data key=\"n_authors\">{}</data>\n", xml_escape(&b.provider)));
                    out.push_str(&format!("      <data key=\"n_year\">{}</data>\n", p.record.year));
                    out.push_str(&format!("      <data key=\"n_venue\">{}</data>\n", xml_escape(&b.section)));
                    out.push_str(&format!("      <data key=\"n_disc\">{}</data>\n", p.record.disc.code()));
                    out.push_str(&format!("      <data key=\"n_url\">{}</data>\n", xml_escape(&b.uri)));
                    out.push_str(&format!("      <data key=\"n_x\">{:.2}</data>\n", (px as f64) * 500.0 - 42.0));
                    out.push_str(&format!("      <data key=\"n_y\">{:.2}</data>\n", -(py as f64) * 500.0 - 32.0));
                    out.push_str("      <data key=\"n_size\">14.5</data>\n");
                    out.push_str(&format!("      <data key=\"n_r\">{}</data>\n", cr));
                    out.push_str(&format!("      <data key=\"n_g\">{}</data>\n", cg));
                    out.push_str(&format!("      <data key=\"n_b\">{}</data>\n", cb));
                    out.push_str(&format!("      <data key=\"n_hex\">{}</data>\n", hex));
                    out.push_str("    </node>\n");
                }
            }
        }
    }

    let mut conduit_lookup: HashMap<(&str, &str), (f64, f64)> = HashMap::new();
    for c in &analytics.conduits {
        conduit_lookup.insert((c.source_id.as_str(), c.target_id.as_str()), (c.eb_undir, c.eb_dir));
    }
    let mut id_to_raw = HashMap::new();
    for r in papers {
        id_to_raw.insert(r.id.as_str(), r);
    }

    let mut edge_counter = 0usize;
    for e in edges {
        let Some(s_met) = id_to_metrics.get(e.source.as_str()) else {
            continue;
        };
        let Some(t_met) = id_to_metrics.get(e.target.as_str()) else {
            continue;
        };
        let s_raw = id_to_raw.get(e.source.as_str()).copied().unwrap_or(&s_met.record);
        let t_raw = id_to_raw.get(e.target.as_str()).copied().unwrap_or(&t_met.record);

        let lag = s_raw.year - t_raw.year;
        let xdisc = s_raw.disc != t_raw.disc;
        let xlouv = s_met.louvain_comm != t_met.louvain_comm;
        let (ebu, ebd) = conduit_lookup
            .get(&(e.source.as_str(), e.target.as_str()))
            .copied()
            .unwrap_or((0.0, 0.0));
        let weight = 1.0 + ebu * 10.0;
        let zombie = t_raw.retraction.status.is_retracted_or_withdrawn()
            && s_raw.year >= t_raw.retraction.notice_year.unwrap_or(t_raw.year);

        out.push_str(&format!(
            "    <edge id=\"e{}\" source=\"{}\" target=\"{}\">\n",
            edge_counter,
            xml_escape(&e.source),
            xml_escape(&e.target)
        ));
        out.push_str(&format!("      <data key=\"e_prov\">{}</data>\n", xml_escape(&e.provenance)));
        out.push_str(&format!("      <data key=\"e_lag\">{}</data>\n", lag));
        out.push_str(&format!("      <data key=\"e_xdisc\">{}</data>\n", xdisc));
        out.push_str(&format!("      <data key=\"e_xlouv\">{}</data>\n", xlouv));
        out.push_str(&format!("      <data key=\"e_ebu\">{:.5}</data>\n", ebu));
        out.push_str(&format!("      <data key=\"e_ebd\">{:.5}</data>\n", ebd));
        out.push_str(&format!("      <data key=\"e_weight\">{:.4}</data>\n", weight));
        out.push_str(&format!("      <data key=\"e_zombie\">{}</data>\n", zombie));
        out.push_str("    </edge>\n");
        edge_counter += 1;
    }

    for (src_id, art_id, rel) in bipartite_edges {
        if id_to_metrics.contains_key(src_id.as_str()) {
            out.push_str(&format!(
                "    <edge id=\"e{}\" source=\"{}\" target=\"{}\">\n",
                edge_counter,
                xml_escape(&src_id),
                xml_escape(&art_id)
            ));
            out.push_str(&format!(
                "      <data key=\"e_prov\">datacite:{}</data>\n",
                xml_escape(&rel)
            ));
            out.push_str("      <data key=\"e_weight\">1.5000</data>\n");
            out.push_str("    </edge>\n");
            edge_counter += 1;
        }
    }

    if include_datacite_nodes {
        for (u_bio, p_bio) in extract_uniprot_pdb_cross_edges(papers) {
            if emitted_artifacts.contains(&u_bio) && emitted_artifacts.contains(&p_bio) {
                out.push_str(&format!(
                    "    <edge id=\"e{}\" source=\"{}\" target=\"{}\">\n",
                    edge_counter,
                    xml_escape(&u_bio),
                    xml_escape(&p_bio)
                ));
                out.push_str("      <data key=\"e_prov\">uniprot_pdb_xref</data>\n");
                out.push_str("      <data key=\"e_weight\">2.0000</data>\n");
                out.push_str("    </edge>\n");
                edge_counter += 1;
            }
        }
    }
    out.push_str("  </graph>\n</graphml>\n");
    fs::write(path, out).map_err(|e| format!("Failed writing GraphML {:?}: {}", path, e))
}

/// Exports the citation & DataCite network in Gephi GEXF 1.3 format (`http://gexf.net/1.3` + `viz`
/// namespace `http://gexf.net/1.3/viz`) with pre-computed `<viz:position>`, `<viz:color>`, `<viz:size>`,
/// and static scientometric `<attvalues>`.
pub fn export_gexf(
    papers: &[PaperRecord],
    edges: &[CitationEdge],
    analytics: &CorpusAnalytics,
    positions: Option<&HashMap<String, [f32; 2]>>,
    color_by: NodeColorEncoding,
    include_datacite_nodes: bool,
    path: &Path,
) -> Result<(), String> {
    let fallback_pos;
    let pos_map = match positions {
        Some(m) if !m.is_empty() => m,
        _ => {
            fallback_pos =
                compute_preset_positions(analytics, ViewerLayoutPreset::ForceAtlas2, color_by, "");
            &fallback_pos
        }
    };

    let max_cb = analytics.papers.iter().map(|p| p.cb_undir).fold(0.05, f64::max);
    let max_v = analytics.papers.iter().map(|p| p.v_exp).fold(0.5, f64::max);
    let max_b = analytics.papers.iter().map(|p| p.beauty_b).fold(5.0, f64::max);

    let mut out = String::with_capacity(papers.len() * 980 + edges.len() * 320 + 4096);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<gexf xmlns=\"http://gexf.net/1.3\"\n");
    out.push_str("      xmlns:viz=\"http://gexf.net/1.3/viz\"\n");
    out.push_str("      xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n");
    out.push_str("      xsi:schemaLocation=\"http://gexf.net/1.3 http://gexf.net/1.3/gexf.xsd\"\n");
    out.push_str("      version=\"1.3\">\n");
    out.push_str("  <meta lastmodifieddate=\"2026-11-14\">\n");
    out.push_str("    <creator>CitAtlas v1.6.0 Native Rust Scientometrics Suite</creator>\n");
    out.push_str("    <description>Citation Velocity, Brandes Betweenness Bridges, Louvain/Infomap Communities, Retraction Watch Integrity, and DataCite Artifact Network</description>\n");
    out.push_str("  </meta>\n");
    out.push_str("  <graph mode=\"static\" defaultedgetype=\"directed\">\n");

    let node_attrs = [
        ("0", "node_type", "string"),
        ("1", "title", "string"),
        ("2", "authors", "string"),
        ("3", "year", "integer"),
        ("4", "venue", "string"),
        ("5", "discipline", "string"),
        ("6", "doi", "string"),
        ("7", "louvain_comm", "string"),
        ("8", "infomap_module", "string"),
        ("9", "infomap_flow_pct", "double"),
        ("10", "in_citations", "integer"),
        ("11", "v_exp", "double"),
        ("12", "beauty_b", "double"),
        ("13", "brandes_cb_undir", "double"),
        ("14", "participation_p", "double"),
        ("15", "guimera_role", "string"),
        ("16", "retraction_status", "string"),
        ("17", "post_retraction_cites", "integer"),
        ("18", "datacite_dataset_count", "integer"),
        ("19", "datacite_software_count", "integer"),
        ("20", "uniprot_bridge_count", "integer"),
        ("21", "pdb_bridge_count", "integer"),
        ("22", "epmc_annotation_count", "integer"),
    ];
    out.push_str("    <attributes class=\"node\" mode=\"static\">\n");
    for (aid, title, atype) in node_attrs {
        out.push_str(&format!(
            "      <attribute id=\"{}\" title=\"{}\" type=\"{}\"/>\n",
            aid, title, atype
        ));
    }
    out.push_str("    </attributes>\n");

    let edge_attrs = [
        ("0", "provenance", "string"),
        ("1", "citation_lag_years", "integer"),
        ("2", "cross_discipline", "boolean"),
        ("3", "cross_louvain", "boolean"),
        ("4", "eb_undir", "double"),
        ("5", "is_post_retraction_zombie_cite", "boolean"),
    ];
    out.push_str("    <attributes class=\"edge\" mode=\"static\">\n");
    for (aid, title, atype) in edge_attrs {
        out.push_str(&format!(
            "      <attribute id=\"{}\" title=\"{}\" type=\"{}\"/>\n",
            aid, title, atype
        ));
    }
    out.push_str("    </attributes>\n");

    out.push_str("    <nodes>\n");
    let mut id_to_metrics = HashMap::new();
    for p in &analytics.papers {
        id_to_metrics.insert(p.record.id.as_str(), p);
        let r = &p.record;
        let [nx, ny] = pos_map.get(&r.id).copied().unwrap_or([0.0, 0.0]);
        let gx = (nx as f64) * 500.0;
        let gy = -(ny as f64) * 500.0;
        let sz = node_radius_for_encoding(
            p,
            NodeSizeEncoding::BrandesBetweenness,
            max_cb,
            max_v,
            max_b,
        ) * 2.0;
        let [cr, cg, cb] = node_rgb_for_encoding(p, color_by, max_v, max_b);

        out.push_str(&format!(
            "      <node id=\"{}\" label=\"{}\">\n",
            xml_escape(&r.id),
            xml_escape(&r.short_cite())
        ));
        out.push_str("        <attvalues>\n");
        out.push_str("          <attvalue for=\"0\" value=\"paper\"/>\n");
        out.push_str(&format!("          <attvalue for=\"1\" value=\"{}\"/>\n", xml_escape(&r.title)));
        out.push_str(&format!("          <attvalue for=\"2\" value=\"{}\"/>\n", xml_escape(&r.authors)));
        out.push_str(&format!("          <attvalue for=\"3\" value=\"{}\"/>\n", r.year));
        out.push_str(&format!("          <attvalue for=\"4\" value=\"{}\"/>\n", xml_escape(&r.venue)));
        out.push_str(&format!("          <attvalue for=\"5\" value=\"{}\"/>\n", r.disc.code()));
        out.push_str(&format!("          <attvalue for=\"6\" value=\"{}\"/>\n", xml_escape(&r.doi)));
        out.push_str(&format!("          <attvalue for=\"7\" value=\"L{}\"/>\n", p.louvain_comm + 1));
        out.push_str(&format!("          <attvalue for=\"8\" value=\"M{}\"/>\n", p.infomap_comm + 1));
        out.push_str(&format!("          <attvalue for=\"9\" value=\"{:.4}\"/>\n", p.infomap_flow * 100.0));
        out.push_str(&format!("          <attvalue for=\"10\" value=\"{}\"/>\n", p.in_deg));
        out.push_str(&format!("          <attvalue for=\"11\" value=\"{:.4}\"/>\n", p.v_exp));
        out.push_str(&format!("          <attvalue for=\"12\" value=\"{:.2}\"/>\n", p.beauty_b));
        out.push_str(&format!("          <attvalue for=\"13\" value=\"{:.5}\"/>\n", p.cb_undir));
        out.push_str(&format!("          <attvalue for=\"14\" value=\"{:.4}\"/>\n", p.part_p));
        out.push_str(&format!("          <attvalue for=\"15\" value=\"{}\"/>\n", xml_escape(p.role_code)));
        out.push_str(&format!("          <attvalue for=\"16\" value=\"{}\"/>\n", r.retraction.status.as_str()));
        out.push_str(&format!("          <attvalue for=\"17\" value=\"{}\"/>\n", p.post_retraction_cites));
        out.push_str(&format!("          <attvalue for=\"18\" value=\"{}\"/>\n", p.dataset_bridge_count));
        out.push_str(&format!("          <attvalue for=\"19\" value=\"{}\"/>\n", p.software_bridge_count));
        out.push_str(&format!("          <attvalue for=\"20\" value=\"{}\"/>\n", p.uniprot_bridge_count));
        out.push_str(&format!("          <attvalue for=\"21\" value=\"{}\"/>\n", p.pdb_bridge_count));
        out.push_str(&format!("          <attvalue for=\"22\" value=\"{}\"/>\n", p.epmc_annotation_count));
        out.push_str("        </attvalues>\n");
        out.push_str(&format!("        <viz:size value=\"{:.2}\"/>\n", sz));
        out.push_str(&format!("        <viz:position x=\"{:.2}\" y=\"{:.2}\" z=\"0.0\"/>\n", gx, gy));
        out.push_str(&format!("        <viz:color r=\"{}\" g=\"{}\" b=\"{}\" a=\"0.92\"/>\n", cr, cg, cb));
        out.push_str("      </node>\n");
    }

    let mut emitted_artifacts: HashSet<String> = HashSet::new();
    let mut bipartite_edges: Vec<(String, String, String)> = Vec::new();
    if include_datacite_nodes {
        for p in &analytics.papers {
            let [px, py] = pos_map.get(&p.record.id).copied().unwrap_or([0.0, 0.0]);
            for lk in &p.record.datacite_links {
                if lk.artifact_doi.is_empty() {
                    continue;
                }
                let art_id = format!("datacite:{}", lk.artifact_doi);
                bipartite_edges.push((p.record.id.clone(), art_id.clone(), lk.relation_type.clone()));
                if emitted_artifacts.insert(lk.artifact_doi.clone()) {
                    let ntype = if lk.artifact_kind.is_software_like() { "software" } else { "dataset" };
                    let (cr, cg, cb) = if lk.artifact_kind.is_software_like() {
                        (167u8, 139u8, 250u8)
                    } else {
                        (45u8, 212u8, 191u8)
                    };
                    out.push_str(&format!(
                        "      <node id=\"{}\" label=\"{}\">\n",
                        xml_escape(&art_id),
                        xml_escape(&lk.artifact_doi)
                    ));
                    out.push_str("        <attvalues>\n");
                    out.push_str(&format!("          <attvalue for=\"0\" value=\"{}\"/>\n", ntype));
                    out.push_str(&format!("          <attvalue for=\"1\" value=\"{}\"/>\n", xml_escape(&lk.artifact_title)));
                    out.push_str(&format!("          <attvalue for=\"2\" value=\"{}\"/>\n", xml_escape(&lk.repository)));
                    out.push_str(&format!("          <attvalue for=\"3\" value=\"{}\"/>\n", p.record.year));
                    out.push_str(&format!("          <attvalue for=\"5\" value=\"{}\"/>\n", p.record.disc.code()));
                    out.push_str(&format!("          <attvalue for=\"6\" value=\"{}\"/>\n", xml_escape(&lk.artifact_doi)));
                    out.push_str("        </attvalues>\n");
                    out.push_str("        <viz:size value=\"14.00\"/>\n");
                    out.push_str(&format!(
                        "        <viz:position x=\"{:.2}\" y=\"{:.2}\" z=\"0.0\"/>\n",
                        (px as f64) * 500.0 + 38.0,
                        -(py as f64) * 500.0 + 28.0
                    ));
                    out.push_str(&format!("        <viz:color r=\"{}\" g=\"{}\" b=\"{}\" a=\"0.92\"/>\n", cr, cg, cb));
                    out.push_str("      </node>\n");
                }
            }
            for b in &p.record.bio_entities {
                if b.entity_id.is_empty() {
                    continue;
                }
                let norm_id = b.entity_id.to_ascii_uppercase();
                let bio_id = format!("bio:{}:{}", b.kind.as_str(), norm_id);
                bipartite_edges.push((p.record.id.clone(), bio_id.clone(), format!("{}:{}", b.provider, b.section)));
                if emitted_artifacts.insert(bio_id.clone()) {
                    let [cr, cg, cb] = b.kind.rgb();
                    out.push_str(&format!(
                        "      <node id=\"{}\" label=\"{}\">\n",
                        xml_escape(&bio_id),
                        xml_escape(&norm_id)
                    ));
                    out.push_str("        <attvalues>\n");
                    out.push_str(&format!("          <attvalue for=\"0\" value=\"{}\"/>\n", b.kind.as_str()));
                    out.push_str(&format!("          <attvalue for=\"1\" value=\"{}\"/>\n", xml_escape(&b.label)));
                    out.push_str(&format!("          <attvalue for=\"2\" value=\"{}\"/>\n", xml_escape(&b.provider)));
                    out.push_str(&format!("          <attvalue for=\"3\" value=\"{}\"/>\n", p.record.year));
                    out.push_str(&format!("          <attvalue for=\"5\" value=\"{}\"/>\n", p.record.disc.code()));
                    out.push_str("        </attvalues>\n");
                    out.push_str("        <viz:size value=\"14.50\"/>\n");
                    out.push_str(&format!(
                        "        <viz:position x=\"{:.2}\" y=\"{:.2}\" z=\"0.0\"/>\n",
                        (px as f64) * 500.0 - 42.0,
                        -(py as f64) * 500.0 - 32.0
                    ));
                    out.push_str(&format!("        <viz:color r=\"{}\" g=\"{}\" b=\"{}\" a=\"0.92\"/>\n", cr, cg, cb));
                    out.push_str("      </node>\n");
                }
            }
        }
    }
    out.push_str("    </nodes>\n");

    let mut conduit_lookup: HashMap<(&str, &str), (f64, f64)> = HashMap::new();
    for c in &analytics.conduits {
        conduit_lookup.insert((c.source_id.as_str(), c.target_id.as_str()), (c.eb_undir, c.eb_dir));
    }
    let mut id_to_raw = HashMap::new();
    for r in papers {
        id_to_raw.insert(r.id.as_str(), r);
    }

    out.push_str("    <edges>\n");
    let mut edge_counter = 0usize;
    for e in edges {
        let Some(s_met) = id_to_metrics.get(e.source.as_str()) else { continue };
        let Some(t_met) = id_to_metrics.get(e.target.as_str()) else { continue };
        let s_raw = id_to_raw.get(e.source.as_str()).copied().unwrap_or(&s_met.record);
        let t_raw = id_to_raw.get(e.target.as_str()).copied().unwrap_or(&t_met.record);
        let lag = s_raw.year - t_raw.year;
        let xdisc = s_raw.disc != t_raw.disc;
        let xlouv = s_met.louvain_comm != t_met.louvain_comm;
        let (ebu, _) = conduit_lookup
            .get(&(e.source.as_str(), e.target.as_str()))
            .copied()
            .unwrap_or((0.0, 0.0));
        let weight = 1.0 + ebu * 10.0;
        let zombie = t_raw.retraction.status.is_retracted_or_withdrawn()
            && s_raw.year >= t_raw.retraction.notice_year.unwrap_or(t_raw.year);

        out.push_str(&format!(
            "      <edge id=\"{}\" source=\"{}\" target=\"{}\" weight=\"{:.4}\">\n",
            edge_counter,
            xml_escape(&e.source),
            xml_escape(&e.target),
            weight
        ));
        out.push_str("        <attvalues>\n");
        out.push_str(&format!("          <attvalue for=\"0\" value=\"{}\"/>\n", xml_escape(&e.provenance)));
        out.push_str(&format!("          <attvalue for=\"1\" value=\"{}\"/>\n", lag));
        out.push_str(&format!("          <attvalue for=\"2\" value=\"{}\"/>\n", xdisc));
        out.push_str(&format!("          <attvalue for=\"3\" value=\"{}\"/>\n", xlouv));
        out.push_str(&format!("          <attvalue for=\"4\" value=\"{:.5}\"/>\n", ebu));
        out.push_str(&format!("          <attvalue for=\"5\" value=\"{}\"/>\n", zombie));
        out.push_str("        </attvalues>\n");
        out.push_str("      </edge>\n");
        edge_counter += 1;
    }

    for (src_id, art_id, rel) in bipartite_edges {
        if id_to_metrics.contains_key(src_id.as_str()) {
            out.push_str(&format!(
                "      <edge id=\"{}\" source=\"{}\" target=\"{}\" weight=\"1.5000\">\n",
                edge_counter,
                xml_escape(&src_id),
                xml_escape(&art_id)
            ));
            out.push_str("        <attvalues>\n");
            out.push_str(&format!("          <attvalue for=\"0\" value=\"datacite:{}\"/>\n", xml_escape(&rel)));
            out.push_str("        </attvalues>\n");
            out.push_str("      </edge>\n");
            edge_counter += 1;
        }
    }
    if include_datacite_nodes {
        for (u_bio, p_bio) in extract_uniprot_pdb_cross_edges(papers) {
            if emitted_artifacts.contains(&u_bio) && emitted_artifacts.contains(&p_bio) {
                out.push_str(&format!(
                    "      <edge id=\"{}\" source=\"{}\" target=\"{}\" weight=\"2.0000\">\n",
                    edge_counter,
                    xml_escape(&u_bio),
                    xml_escape(&p_bio)
                ));
                out.push_str("        <attvalues>\n");
                out.push_str("          <attvalue for=\"0\" value=\"uniprot_pdb_xref\"/>\n");
                out.push_str("        </attvalues>\n");
                out.push_str("      </edge>\n");
                edge_counter += 1;
            }
        }
    }
    out.push_str("    </edges>\n  </graph>\n</gexf>\n");
    fs::write(path, out).map_err(|e| format!("Failed writing GEXF {:?}: {}", path, e))
}
