use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::bio_entity_bridges::run_bio_entity_screening;
use crate::crawler::{extract_year_from_str, PoliteWebCrawler};
use crate::models::{
    infer_concepts_from_text, CitationEdge, DataCiteArtifactKind, DataCiteLink, Discipline,
    OaStatus, PaperRecord, RetractionRecord, RetractionStatus, ScreeningSummary, SearchRequest,
};
use crate::search_providers::{normalize_doi, percent_encode};
use crate::storage::parse_rfc4180_csv;

// ============================================================================
// 1. UNIFIED INTEGRITY & DATACITE BRIDGE ORCHESTRATOR
// ============================================================================
#[allow(clippy::too_many_arguments)]
pub fn run_integrity_and_datacite_screening(
    papers: &mut [PaperRecord],
    edges: &mut Vec<CitationEdge>,
    polite_email: &str,
    proxy_url: &str,
    rw_csv_path: Option<&Path>,
    do_crossref_retractions: bool,
    do_datacite_bridges: bool,
    emit_shared_artifact_edges: bool,
) -> Result<ScreeningSummary, String> {
    run_full_corpus_screening(
        papers,
        edges,
        polite_email,
        proxy_url,
        rw_csv_path,
        do_crossref_retractions,
        do_datacite_bridges,
        true,
        true,
        emit_shared_artifact_edges,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_full_corpus_screening(
    papers: &mut [PaperRecord],
    edges: &mut Vec<CitationEdge>,
    polite_email: &str,
    proxy_url: &str,
    rw_csv_path: Option<&Path>,
    do_crossref_retractions: bool,
    do_datacite_bridges: bool,
    do_epmc_annotations: bool,
    do_uniprot_pdb: bool,
    emit_shared_artifact_edges: bool,
) -> Result<ScreeningSummary, String> {
    let mut warnings = Vec::new();

    // Step A: Offline Retraction Watch GitLab CSV snapshot screening (if provided)
    if let Some(csv_p) = rw_csv_path {
        if !csv_p.as_os_str().is_empty() {
            match fs::read_to_string(csv_p) {
                Ok(csv_text) => {
                    let matched = screen_papers_against_rw_csv(papers, &csv_text);
                    if matched == 0 {
                        warnings.push(format!(
                            "Screened against {:?}: 0 adverse notices matched corpus DOIs/PMIDs.",
                            csv_p
                        ));
                    }
                }
                Err(e) => warnings.push(format!(
                    "Could not read Retraction Watch CSV {:?}: {}",
                    csv_p, e
                )),
            }
        }
    }

    let crawler = PoliteWebCrawler::new(polite_email, proxy_url)?;

    // Step B: Live Crossref REST API (including Retraction Watch & Crossmark updates)
    if do_crossref_retractions {
        let mut cr_warns = screen_crossref_retractions_live(&crawler, papers, polite_email);
        warnings.append(&mut cr_warns);
    }

    // Step C: Live DataCite REST API dataset & software DOI bridge tracking
    if do_datacite_bridges {
        let mut dc_warns = track_datacite_bridges_live(&crawler, papers);
        warnings.append(&mut dc_warns);
    }

    // Step D: Synthesize bipartite shared-artifact citation bridges
    let mut shared_edges_added = 0usize;
    if emit_shared_artifact_edges {
        let new_edges = synthesize_shared_artifact_edges(papers);
        let mut existing_pairs: HashSet<(String, String)> = edges
            .iter()
            .map(|e| (e.source.clone(), e.target.clone()))
            .collect();
        for ne in new_edges {
            if existing_pairs.insert((ne.source.clone(), ne.target.clone())) {
                edges.push(ne);
                shared_edges_added += 1;
            }
        }
    }

    let retractions_flagged = papers
        .iter()
        .filter(|p| p.retraction.status.is_retracted_or_withdrawn())
        .count();
    let corrections_or_concerns = papers
        .iter()
        .filter(|p| {
            matches!(
                p.retraction.status,
                RetractionStatus::ExpressionOfConcern | RetractionStatus::Correction
            )
        })
        .count();
    let datacite_artifacts_linked: usize = papers.iter().map(|p| p.datacite_links.len()).sum();

    let mut bio_stats = run_bio_entity_screening(
        &crawler,
        papers,
        edges,
        do_epmc_annotations,
        do_uniprot_pdb,
        emit_shared_artifact_edges,
    );
    warnings.append(&mut bio_stats.warnings);

    Ok(ScreeningSummary {
        papers_screened: papers.len(),
        retractions_flagged,
        corrections_or_concerns,
        datacite_artifacts_linked,
        shared_artifact_bridge_edges: shared_edges_added,
        epmc_annotations_linked: bio_stats.epmc_annotations_linked,
        uniprot_proteins_linked: bio_stats.uniprot_proteins_linked,
        pdb_structures_linked: bio_stats.pdb_structures_linked,
        shared_bio_bridge_edges: bio_stats.shared_bio_bridge_edges,
        warnings,
    })
}

// ============================================================================
// 2. CROSSREF RETRACTION WATCH LIVE REST SCREENER & JSON PARSER
// ============================================================================
pub fn screen_crossref_retractions_live(
    crawler: &PoliteWebCrawler,
    papers: &mut [PaperRecord],
    polite_email: &str,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Collect indices of papers that have a DOI
    let doi_indices: Vec<(usize, String)> = papers
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.doi.trim().is_empty())
        .map(|(i, p)| (i, normalize_doi(&p.doi)))
        .collect();

    if doi_indices.is_empty() {
        return warnings;
    }

    let mut doi_to_indices: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, d) in &doi_indices {
        doi_to_indices.entry(d.clone()).or_default().push(*idx);
    }

    // Batch-query Crossref /works?filter=doi:...,doi:... (chunks of 25 DOIs per request!)
    let unique_dois: Vec<String> = doi_to_indices.keys().cloned().collect();
    for (chunk_idx, chunk) in unique_dois.chunks(25).enumerate() {
        if chunk_idx > 0 {
            crawler.sleep_polite(280, 60, chunk_idx);
        }
        let filter_expr = chunk
            .iter()
            .map(|d| format!("doi:{}", percent_encode(d)))
            .collect::<Vec<_>>()
            .join(",");
        let url = format!(
            "https://api.crossref.org/works?filter={}&rows={}&mailto={}",
            filter_expr,
            chunk.len(),
            percent_encode(polite_email)
        );

        match crawler.get_json(&url) {
            Ok(val) => {
                if let Some(items) = val
                    .get("message")
                    .and_then(|m| m.get("items"))
                    .and_then(|i| i.as_array())
                {
                    for item in items {
                        let Some(raw_doi) = item.get("DOI").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let clean_doi = normalize_doi(raw_doi);
                        let parsed = parse_crossref_work_retraction(item);
                        if let Some(indices) = doi_to_indices.get(&clean_doi) {
                            for &p_idx in indices {
                                apply_retraction_update(&mut papers[p_idx].retraction, &parsed);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warnings.push(format!(
                    "Crossref Retraction Watch batch {} warning: {}",
                    chunk_idx + 1,
                    e
                ));
            }
        }
    }

    warnings
}

/// Inspects a Crossref `work` JSON object for Retraction Watch (`source: "retraction-watch"`)
/// and publisher Crossmark (`updated-by`, `update-to`, and title markers).
pub fn parse_crossref_work_retraction(work: &Value) -> RetractionRecord {
    let mut best = RetractionRecord {
        status: RetractionStatus::Clean,
        source: "crossref".to_string(),
        notice_doi: String::new(),
        notice_date: String::new(),
        notice_year: None,
        record_id: String::new(),
        reasons: Vec::new(),
    };

    // 1. Check `updated-by` (primary Crossref field for Retraction Watch & Crossmark notices on an original article)
    //    and `update-to` (present when inspecting update notices or Crossmark reverse links)
    for field_name in ["updated-by", "update-to"] {
        if let Some(arr) = work.get(field_name).and_then(|v| v.as_array()) {
            for entry in arr {
                let update_type = entry
                    .get("type")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        entry
                            .get("updated")
                            .and_then(|u| u.get("type"))
                            .and_then(|v| v.as_str())
                    })
                    .unwrap_or("");
                let candidate_status = classify_crossref_update_type(update_type);
                if candidate_status.severity_rank() > best.status.severity_rank() {
                    best.status = candidate_status;
                    let src = entry
                        .get("source")
                        .and_then(|v| v.as_str())
                        .unwrap_or("publisher-crossmark");
                    best.source = src.to_string();
                    if let Some(ndoi) = entry.get("DOI").and_then(|v| v.as_str()) {
                        best.notice_doi = normalize_doi(ndoi);
                    }
                    if let Some(rid) = entry.get("record-id").and_then(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    }) {
                        best.record_id = rid;
                    }
                    if let Some((date_s, yr)) = extract_crossref_updated_date(entry) {
                        best.notice_date = date_s;
                        best.notice_year = Some(yr);
                    }
                    if let Some(lbl) = entry.get("label").and_then(|v| v.as_str()) {
                        if !lbl.trim().is_empty() && !best.reasons.contains(&lbl.to_string()) {
                            best.reasons.push(lbl.trim().to_string());
                        }
                    }
                }
            }
        }
    }

    // 2. Check publisher title prefix marker (e.g. "RETRACTED: ...", "[Retracted] ...")
    if best.status == RetractionStatus::Clean {
        let title = work
            .get("title")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let t_up = title.trim().to_ascii_uppercase();
        if t_up.starts_with("RETRACTED:")
            || t_up.starts_with("RETRACTED ARTICLE:")
            || t_up.starts_with("[RETRACTED]")
        {
            best.status = RetractionStatus::Retracted;
            best.source = "publisher-title".to_string();
        } else if t_up.starts_with("WITHDRAWN:") || t_up.starts_with("[WITHDRAWN]") {
            best.status = RetractionStatus::Withdrawn;
            best.source = "publisher-title".to_string();
        }
    }

    best
}

fn classify_crossref_update_type(update_type: &str) -> RetractionStatus {
    let low = update_type.trim().to_ascii_lowercase();
    if low.contains("retract") || low == "removal" {
        RetractionStatus::Retracted
    } else if low.contains("withdraw") {
        RetractionStatus::Withdrawn
    } else if low.contains("concern") {
        RetractionStatus::ExpressionOfConcern
    } else if low.contains("correct") || low.contains("errat") || low.contains("corrigend") {
        RetractionStatus::Correction
    } else {
        RetractionStatus::Clean
    }
}

fn extract_crossref_updated_date(entry: &Value) -> Option<(String, i32)> {
    let parts = entry
        .get("updated")
        .and_then(|u| u.get("date-parts"))
        .and_then(|dp| dp.as_array())
        .and_then(|outer| outer.first())
        .and_then(|inner| inner.as_array())?;
    let yr = parts.first()?.as_i64()? as i32;
    let mo = parts.get(1).and_then(|v| v.as_i64()).unwrap_or(1);
    let dy = parts.get(2).and_then(|v| v.as_i64()).unwrap_or(1);
    Some((format!("{:04}-{:02}-{:02}", yr, mo, dy), yr))
}

pub fn apply_retraction_update(target: &mut RetractionRecord, incoming: &RetractionRecord) {
    if incoming.status.severity_rank() > target.status.severity_rank()
        || (target.status == RetractionStatus::Unscreened
            && incoming.status == RetractionStatus::Clean)
    {
        *target = incoming.clone();
    } else if incoming.status == target.status && incoming.status.is_flagged() {
        if target.notice_doi.is_empty() && !incoming.notice_doi.is_empty() {
            target.notice_doi = incoming.notice_doi.clone();
        }
        if target.record_id.is_empty() && !incoming.record_id.is_empty() {
            target.record_id = incoming.record_id.clone();
        }
        if target.notice_date.is_empty() && !incoming.notice_date.is_empty() {
            target.notice_date = incoming.notice_date.clone();
            target.notice_year = incoming.notice_year;
        }
        for r in &incoming.reasons {
            if !target.reasons.contains(r) {
                target.reasons.push(r.clone());
            }
        }
    }
}

// ============================================================================
// 3. OFFLINE RETRACTION WATCH GITLAB CSV SNAPSHOT SCREENER
// ============================================================================
/// Screens corpus papers against a local `retraction_watch.csv` snapshot
/// (`Record ID`, `OriginalPaperDOI`, `OriginalPaperPubMedID`, `RetractionDOI`,
/// `RetractionDate`, `RetractionNature`, `Reason`).
pub fn screen_papers_against_rw_csv(papers: &mut [PaperRecord], csv_text: &str) -> usize {
    let rows = parse_rfc4180_csv(csv_text);
    if rows.len() < 2 {
        return 0;
    }

    let header: HashMap<String, usize> = rows[0]
        .iter()
        .enumerate()
        .map(|(idx, h)| {
            let norm = h
                .trim()
                .to_ascii_lowercase()
                .replace([' ', '_', '-'], "");
            (norm, idx)
        })
        .collect();

    let col = |row: &[String], keys: &[&str]| -> String {
        for &k in keys {
            if let Some(&c_idx) = header.get(k) {
                if let Some(v) = row.get(c_idx) {
                    let t = v.trim();
                    if !t.is_empty() && !t.eq_ignore_ascii_case("unavailable") {
                        return t.to_string();
                    }
                }
            }
        }
        String::new()
    };

    let mut doi_lookup: HashMap<String, Vec<usize>> = HashMap::new();
    let mut pmid_lookup: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, p) in papers.iter_mut().enumerate() {
        if p.retraction.status == RetractionStatus::Unscreened {
            p.retraction.status = RetractionStatus::Clean;
            p.retraction.source = "rw-csv-snapshot".to_string();
        }
        if !p.doi.is_empty() {
            doi_lookup
                .entry(normalize_doi(&p.doi))
                .or_default()
                .push(idx);
        }
        for t in &p.terms {
            if let Some(pm) = t.strip_prefix("pmid:") {
                pmid_lookup.entry(pm.to_string()).or_default().push(idx);
            }
        }
        if let Some(pm) = p.id.strip_prefix("pmid_").or_else(|| p.id.strip_prefix("epmc_")) {
            if pm.chars().all(|c| c.is_ascii_digit()) {
                pmid_lookup.entry(pm.to_string()).or_default().push(idx);
            }
        }
    }

    let mut matched_papers = HashSet::new();
    for row in rows.iter().skip(1) {
        let orig_doi = normalize_doi(&col(row, &["originalpaperdoi", "doi"]));
        let orig_pmid = col(row, &["originalpaperpubmedid", "pmid"]);

        let mut hit_indices: Vec<usize> = Vec::new();
        if !orig_doi.is_empty() {
            if let Some(ixs) = doi_lookup.get(&orig_doi) {
                hit_indices.extend(ixs.iter().copied());
            }
        }
        if !orig_pmid.is_empty() && orig_pmid != "0" {
            if let Some(ixs) = pmid_lookup.get(&orig_pmid) {
                for &ix in ixs {
                    if !hit_indices.contains(&ix) {
                        hit_indices.push(ix);
                    }
                }
            }
        }
        if hit_indices.is_empty() {
            continue;
        }

        let nature = col(row, &["retractionnature", "nature", "updatetype"]);
        let status = if nature.to_ascii_lowercase().contains("reinstat") {
            RetractionStatus::Clean
        } else {
            let s = RetractionStatus::from_str_lossy(&nature);
            if s == RetractionStatus::Unscreened {
                RetractionStatus::Retracted
            } else {
                s
            }
        };

        let record_id = col(row, &["recordid", "id"]);
        let notice_doi = normalize_doi(&col(row, &["retractiondoi", "noticedoi"]));
        let notice_date = col(row, &["retractiondate", "date"]);
        let notice_year = extract_year_from_str(&notice_date);
        let reasons: Vec<String> = col(row, &["reason", "reasons"])
            .split(';')
            .map(|r| r.trim().trim_start_matches('+').trim().to_string())
            .filter(|r| !r.is_empty())
            .collect();

        let rec = RetractionRecord {
            status,
            source: "retraction-watch".to_string(),
            notice_doi,
            notice_date,
            notice_year,
            record_id,
            reasons,
        };

        for idx in hit_indices {
            if status == RetractionStatus::Clean
                && nature.to_ascii_lowercase().contains("reinstat")
            {
                papers[idx].retraction = rec.clone();
            } else {
                apply_retraction_update(&mut papers[idx].retraction, &rec);
            }
            if papers[idx].retraction.status.is_flagged() {
                matched_papers.insert(idx);
            }
        }
    }

    matched_papers.len()
}

// ============================================================================
// 4. DATACITE REST API DATASET & SOFTWARE DOI BRIDGE TRACKER
// ============================================================================
pub fn track_datacite_bridges_live(
    crawler: &PoliteWebCrawler,
    papers: &mut [PaperRecord],
) -> Vec<String> {
    let mut warnings = Vec::new();
    let candidates: Vec<(usize, String)> = papers
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.doi.trim().is_empty())
        .take(35)
        .map(|(i, p)| (i, normalize_doi(&p.doi)))
        .collect();

    for (step, (p_idx, paper_doi)) in candidates.iter().enumerate() {
        if step > 0 {
            crawler.sleep_polite(240, 50, step);
        }
        let q = format!("relatedIdentifiers.relatedIdentifier:\"{}\"", paper_doi);
        let url = format!(
            "https://api.datacite.org/dois?query={}&page%5Bsize%5D=25&publisher=true",
            percent_encode(&q)
        );
        match crawler.get_json(&url) {
            Ok(val) => {
                let parsed_links = parse_datacite_list_for_paper(&val, paper_doi);
                for lk in parsed_links {
                    if !papers[*p_idx]
                        .datacite_links
                        .iter()
                        .any(|existing| existing.artifact_doi == lk.artifact_doi)
                    {
                        papers[*p_idx].datacite_links.push(lk);
                    }
                }
            }
            Err(e) => {
                warnings.push(format!("DataCite lookup skipped for {}: {}", paper_doi, e));
            }
        }
    }

    warnings
}

/// Parses DataCite `/dois` JSON:API response and extracts dataset, software, workflow,
/// and model DOIs linked to `paper_doi` (or general query hits if `paper_doi` is empty).
pub fn parse_datacite_list_for_paper(val: &Value, paper_doi: &str) -> Vec<DataCiteLink> {
    let mut links = Vec::new();
    let Some(data_arr) = val.get("data").and_then(|v| v.as_array()) else {
        return links;
    };
    let norm_paper_doi = normalize_doi(paper_doi);

    for item in data_arr {
        let Some(attrs) = item.get("attributes") else {
            continue;
        };
        let artifact_doi = attrs
            .get("doi")
            .and_then(|v| v.as_str())
            .map(normalize_doi)
            .unwrap_or_default();
        if artifact_doi.is_empty() || artifact_doi == norm_paper_doi {
            continue;
        }

        let rtype = attrs
            .get("types")
            .and_then(|t| t.get("resourceTypeGeneral"))
            .and_then(|v| v.as_str())
            .unwrap_or("Dataset");
        let artifact_kind = DataCiteArtifactKind::from_resource_type_general(rtype);

        let artifact_title = attrs
            .get("titles")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|t| t.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("DataCite Research Artifact")
            .trim()
            .to_string();

        let repository = attrs
            .get("publisher")
            .and_then(|p| {
                p.as_str()
                    .or_else(|| p.get("name").and_then(|n| n.as_str()))
            })
            .or_else(|| {
                item.get("relationships")
                    .and_then(|r| r.get("client"))
                    .and_then(|c| c.get("data"))
                    .and_then(|d| d.get("id"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("DataCite Repository")
            .to_string();

        let mut relation_type = String::from("References");
        if let Some(rel_ids) = attrs.get("relatedIdentifiers").and_then(|v| v.as_array()) {
            for ri in rel_ids {
                let r_id = ri
                    .get("relatedIdentifier")
                    .and_then(|v| v.as_str())
                    .map(normalize_doi)
                    .unwrap_or_default();
                if norm_paper_doi.is_empty() || r_id == norm_paper_doi {
                    if let Some(rt) = ri.get("relationType").and_then(|v| v.as_str()) {
                        relation_type = rt.to_string();
                        break;
                    }
                }
            }
        }

        let citation_count = attrs
            .get("citationCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let view_count = attrs.get("viewCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let download_count = attrs
            .get("downloadCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        links.push(DataCiteLink {
            artifact_doi,
            artifact_title,
            artifact_kind,
            repository,
            relation_type,
            citation_count,
            view_count,
            download_count,
        });
    }

    links
}

/// Direct DataCite `/dois` search provider for discovering citable datasets & research software
/// (`Zenodo`, `Dryad`, `Figshare`, `GBIF`) as nodes in the scholarly graph.
pub fn search_datacite_works(
    crawler: &PoliteWebCrawler,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let limit = req.max_per_provider.clamp(1, 100);
    let url = format!(
        "https://api.datacite.org/dois?query={}&page%5Bsize%5D={}&publisher=true",
        percent_encode(req.query.trim()),
        limit
    );
    let val = crawler.get_json(&url)?;
    let Some(data_arr) = val.get("data").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for (idx, item) in data_arr.iter().enumerate() {
        let Some(attrs) = item.get("attributes") else {
            continue;
        };
        let doi = attrs
            .get("doi")
            .and_then(|v| v.as_str())
            .map(normalize_doi)
            .unwrap_or_default();
        if doi.is_empty() {
            continue;
        }
        let title = attrs
            .get("titles")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|t| t.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let year = attrs
            .get("publicationYear")
            .and_then(|v| v.as_i64())
            .map(|y| y as i32)
            .unwrap_or(2024);

        let authors = attrs
            .get("creators")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .take(6)
                    .filter_map(|c| c.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "DataCite Creator".to_string());

        let rtype = attrs
            .get("types")
            .and_then(|t| t.get("resourceTypeGeneral"))
            .and_then(|v| v.as_str())
            .unwrap_or("Dataset");
        let kind = DataCiteArtifactKind::from_resource_type_general(rtype);
        let repo = attrs
            .get("publisher")
            .and_then(|p| p.as_str().or_else(|| p.get("name").and_then(|n| n.as_str())))
            .unwrap_or("DataCite")
            .to_string();

        let desc = attrs
            .get("descriptions")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|d| d.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let cite_cnt = attrs.get("citationCount").and_then(|v| v.as_u64());
        let view_cnt = attrs.get("viewCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let dl_cnt = attrs
            .get("downloadCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let mut raw_refs = Vec::new();
        if let Some(rel_ids) = attrs.get("relatedIdentifiers").and_then(|v| v.as_array()) {
            for ri in rel_ids {
                let id_type = ri
                    .get("relatedIdentifierType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if id_type.eq_ignore_ascii_case("DOI") {
                    if let Some(rdoi) = ri.get("relatedIdentifier").and_then(|v| v.as_str()) {
                        let clean_r = normalize_doi(rdoi);
                        if !clean_r.is_empty() {
                            raw_refs.push(format!("DOI:{}", clean_r));
                        }
                    }
                }
            }
        }

        let combined = format!("{} {} {}", title, repo, desc);
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);

        let self_link = DataCiteLink {
            artifact_doi: doi.clone(),
            artifact_title: title.clone(),
            artifact_kind: kind,
            repository: repo.clone(),
            relation_type: "SelfArtifact".to_string(),
            citation_count: cite_cnt.unwrap_or(0),
            view_count: view_cnt,
            download_count: dl_cnt,
        };

        out.push(PaperRecord {
            id: format!("dc_{}_{}", year, idx + 1),
            title,
            authors,
            year,
            venue: format!("{} ({})", repo, kind.as_str()),
            disc,
            source_url: attrs
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("https://doi.org/{}", doi)),
            doi,
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Gold,
            external_citations: cite_cnt,
            abstract_text: desc,
            terms: concepts.clone(),
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: raw_refs,
            retraction: RetractionRecord::default(),
            datacite_links: vec![self_link],
            bio_entities: Vec::new(),
        });
    }

    Ok(out)
}

/// Projects bipartite `(Paper <-> DataCite Artifact DOI)` links onto chronological
/// paper-to-paper bridge edges whenever two papers cite or supplement the same dataset or software DOI.
pub fn synthesize_shared_artifact_edges(papers: &[PaperRecord]) -> Vec<CitationEdge> {
    let mut artifact_to_papers: HashMap<String, (DataCiteArtifactKind, Vec<usize>)> = HashMap::new();
    for (idx, p) in papers.iter().enumerate() {
        for lk in &p.datacite_links {
            if lk.artifact_doi.is_empty() {
                continue;
            }
            let entry = artifact_to_papers
                .entry(lk.artifact_doi.clone())
                .or_insert_with(|| (lk.artifact_kind, Vec::new()));
            if !entry.1.contains(&idx) {
                entry.1.push(idx);
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (art_doi, (kind, mut member_ixs)) in artifact_to_papers {
        if member_ixs.len() < 2 {
            continue;
        }
        // Sort chronologically so newer papers point back to the foundational paper introducing/sharing the artifact
        member_ixs.sort_by_key(|&i| (papers[i].year, papers[i].id.clone()));
        let anchor_idx = member_ixs[0];
        for &later_idx in member_ixs.iter().skip(1) {
            let src = papers[later_idx].id.clone();
            let tgt = papers[anchor_idx].id.clone();
            if src != tgt && seen.insert((src.clone(), tgt.clone())) {
                out.push(CitationEdge {
                    source: src,
                    target: tgt,
                    provenance: format!("datacite_shared_{}:{}", kind.as_str().to_ascii_lowercase(), art_doi),
                });
            }
        }
    }

    out
}
