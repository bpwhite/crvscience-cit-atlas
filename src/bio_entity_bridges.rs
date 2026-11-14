use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::crawler::PoliteWebCrawler;
use crate::models::{BioEntityBridgeLink, BioEntityKind, CitationEdge, PaperRecord};
use crate::search_providers::{normalize_doi, percent_encode};

// ============================================================================
// 1. UNIFIED EUROPE PMC ANNOTATIONS + UNIPROTKB + RCSB PDB ORCHESTRATOR
// ============================================================================
pub struct BioEntityScreeningStats {
    pub epmc_annotations_linked: usize,
    pub uniprot_proteins_linked: usize,
    pub pdb_structures_linked: usize,
    pub shared_bio_bridge_edges: usize,
    pub warnings: Vec<String>,
}

pub fn run_bio_entity_screening(
    crawler: &PoliteWebCrawler,
    papers: &mut [PaperRecord],
    edges: &mut Vec<CitationEdge>,
    do_epmc_annotations: bool,
    do_uniprot_pdb: bool,
    emit_shared_bio_edges: bool,
) -> BioEntityScreeningStats {
    let mut warnings = Vec::new();

    // Resolve missing PMCID / PMID for DOI-bearing papers via Europe PMC core search
    if do_epmc_annotations || do_uniprot_pdb {
        let mut id_warns = resolve_missing_pmcids_and_pmids(crawler, papers);
        warnings.append(&mut id_warns);
    }

    // Pass 1: Europe PMC Annotations API (`annotationsByArticleIds`)
    if do_epmc_annotations {
        let mut epmc_warns = track_europe_pmc_annotations_live(crawler, papers);
        warnings.append(&mut epmc_warns);
    }

    // Pass 2: UniProtKB (`rest.uniprot.org`) & RCSB PDB (`data.rcsb.org` / `search.rcsb.org`)
    if do_uniprot_pdb {
        let mut up_warns = track_uniprot_and_pdb_bridges_live(crawler, papers);
        warnings.append(&mut up_warns);
    }

    // Pass 3: Project shared bio-entities onto chronological paper-to-paper citation bridges
    let mut shared_bio_bridge_edges = 0usize;
    if emit_shared_bio_edges {
        let new_edges = synthesize_shared_bio_entity_edges(papers);
        let mut existing: HashSet<(String, String)> = edges
            .iter()
            .map(|e| (e.source.clone(), e.target.clone()))
            .collect();
        for ne in new_edges {
            if existing.insert((ne.source.clone(), ne.target.clone())) {
                edges.push(ne);
                shared_bio_bridge_edges += 1;
            }
        }
    }

    let mut epmc_annotations_linked = 0usize;
    let mut uniprot_proteins_linked = 0usize;
    let mut pdb_structures_linked = 0usize;
    for p in papers.iter() {
        uniprot_proteins_linked += p.uniprot_count();
        pdb_structures_linked += p.pdb_count();
        epmc_annotations_linked += p.epmc_annotation_count();
    }

    BioEntityScreeningStats {
        epmc_annotations_linked,
        uniprot_proteins_linked,
        pdb_structures_linked,
        shared_bio_bridge_edges,
        warnings,
    }
}

/// Deduplicates and merges an incoming `BioEntityBridgeLink` into a paper's `bio_entities` vector.
pub fn merge_bio_entity_into_paper(
    target: &mut Vec<BioEntityBridgeLink>,
    incoming: BioEntityBridgeLink,
) {
    if incoming.entity_id.trim().is_empty() {
        return;
    }
    if let Some(existing) = target
        .iter_mut()
        .find(|e| e.kind == incoming.kind && e.entity_id.eq_ignore_ascii_case(&incoming.entity_id))
    {
        existing.mention_count = existing.mention_count.saturating_add(incoming.mention_count.max(1));
        if (existing.label.is_empty() || existing.label.len() < incoming.label.len())
            && !incoming.label.is_empty()
        {
            existing.label = incoming.label;
        }
        if existing.uri.is_empty() && !incoming.uri.is_empty() {
            existing.uri = incoming.uri;
        }
        if existing.resolution_angstrom.is_none() && incoming.resolution_angstrom.is_some() {
            existing.resolution_angstrom = incoming.resolution_angstrom;
        }
        for pdb in incoming.linked_pdb_ids {
            let norm = pdb.trim().to_ascii_uppercase();
            if !norm.is_empty() && !existing.linked_pdb_ids.contains(&norm) {
                existing.linked_pdb_ids.push(norm);
            }
        }
    } else {
        target.push(incoming);
    }
}

/// Extracts a paper's PubMed ID (`pmid`) from `terms` (`pmid:12345`) or `id` (`pmid_12345` / `epmc_12345`).
pub fn extract_paper_pmid(p: &PaperRecord) -> Option<String> {
    for t in &p.terms {
        if let Some(pm) = t.strip_prefix("pmid:") {
            let trimmed = pm.trim();
            if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
                return Some(trimmed.to_string());
            }
        }
    }
    for prefix in ["pmid_", "epmc_"] {
        if let Some(pm) = p.id.strip_prefix(prefix) {
            if !pm.is_empty() && pm.chars().all(|c| c.is_ascii_digit()) {
                return Some(pm.to_string());
            }
        }
    }
    if let Some(idx) = p.source_url.find("pubmed.ncbi.nlm.nih.gov/") {
        let rest = &p.source_url[idx + "pubmed.ncbi.nlm.nih.gov/".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return Some(digits);
        }
    }
    None
}

fn resolve_missing_pmcids_and_pmids(
    crawler: &PoliteWebCrawler,
    papers: &mut [PaperRecord],
) -> Vec<String> {
    let mut warnings = Vec::new();
    let candidates: Vec<(usize, String)> = papers
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.pmcid.trim().is_empty()
                && extract_paper_pmid(p).is_none()
                && !p.doi.trim().is_empty()
        })
        .take(24)
        .map(|(i, p)| (i, normalize_doi(&p.doi)))
        .collect();

    for (step, (p_idx, doi)) in candidates.iter().enumerate() {
        if step > 0 {
            crawler.sleep_polite(180, 40, step);
        }
        let q = format!("DOI:\"{}\"", doi);
        let url = format!(
            "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={}&format=json&pageSize=1",
            percent_encode(&q)
        );
        match crawler.get_json(&url) {
            Ok(val) => {
                if let Some(first) = val
                    .get("resultList")
                    .and_then(|rl| rl.get("result"))
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.first())
                {
                    if let Some(pmcid) = first.get("pmcid").and_then(|v| v.as_str()) {
                        if !pmcid.trim().is_empty() {
                            papers[*p_idx].pmcid = pmcid.trim().to_string();
                        }
                    }
                    if let Some(pmid) = first.get("pmid").and_then(|v| v.as_str()) {
                        let tok = format!("pmid:{}", pmid.trim());
                        if !pmid.trim().is_empty() && !papers[*p_idx].terms.contains(&tok) {
                            papers[*p_idx].terms.push(tok);
                        }
                    }
                }
            }
            Err(e) => {
                warnings.push(format!("Europe PMC ID lookup skipped for {}: {}", doi, e));
            }
        }
    }
    warnings
}

// ============================================================================
// 2. EUROPE PMC ANNOTATIONS API (`annotationsByArticleIds`)
// ============================================================================
/// Queries `https://www.ebi.ac.uk/europepmc/annotations_api/annotationsByArticleIds`
/// in batches of 8 `articleIds` (`PMC:PMCxxxxxx` or `MED:<pmid>`).
pub fn track_europe_pmc_annotations_live(
    crawler: &PoliteWebCrawler,
    papers: &mut [PaperRecord],
) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut key_to_indices: HashMap<String, Vec<usize>> = HashMap::new();
    let mut ordered_keys: Vec<String> = Vec::new();

    for (idx, p) in papers.iter().enumerate() {
        let article_key = if !p.pmcid.trim().is_empty() {
            let clean_pmc = if p.pmcid.trim().to_ascii_uppercase().starts_with("PMC") {
                p.pmcid.trim().to_ascii_uppercase()
            } else {
                format!("PMC{}", p.pmcid.trim())
            };
            Some(format!("PMC:{}", clean_pmc))
        } else {
            extract_paper_pmid(p).map(|pm| format!("MED:{}", pm))
        };

        if let Some(k) = article_key {
            let entry = key_to_indices.entry(k.clone()).or_default();
            if entry.is_empty() && ordered_keys.len() < 48 {
                ordered_keys.push(k);
            }
            entry.push(idx);
        }
    }

    for (batch_idx, chunk) in ordered_keys.chunks(8).enumerate() {
        if batch_idx > 0 {
            crawler.sleep_polite(240, 50, batch_idx);
        }
        let joined = chunk.join(",");
        let url = format!(
            "https://www.ebi.ac.uk/europepmc/annotations_api/annotationsByArticleIds?articleIds={}&format=JSON",
            percent_encode(&joined)
        );
        match crawler.get_json(&url) {
            Ok(val) => {
                let by_article = parse_europe_pmc_annotations_json(&val);
                for (art_key, links) in by_article {
                    if let Some(p_indices) = key_to_indices.get(&art_key) {
                        for &p_idx in p_indices {
                            for lk in &links {
                                merge_bio_entity_into_paper(
                                    &mut papers[p_idx].bio_entities,
                                    lk.clone(),
                                );
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warnings.push(format!(
                    "Europe PMC Annotations batch {} warning: {}",
                    batch_idx + 1,
                    e
                ));
            }
        }
    }

    warnings
}

/// Parses the JSON array (or `{ "articles": [...] }` wrapper) returned by Europe PMC's
/// `annotationsByArticleIds` endpoint into canonical `BioEntityBridgeLink` items keyed by
/// `"PMC:PMCxxxxxx"` and/or `"MED:<pmid>"`.
pub fn parse_europe_pmc_annotations_json(
    val: &Value,
) -> HashMap<String, Vec<BioEntityBridgeLink>> {
    let mut result: HashMap<String, Vec<BioEntityBridgeLink>> = HashMap::new();
    let articles_opt = val
        .as_array()
        .or_else(|| val.get("articles").and_then(|a| a.as_array()));
    let Some(articles) = articles_opt else {
        return result;
    };

    for art in articles {
        let source = art
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        let ext_id = art
            .get("extId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let pmcid = art
            .get("pmcid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();

        let mut target_keys = Vec::new();
        if !pmcid.is_empty() {
            let norm_pmc = if pmcid.starts_with("PMC") {
                pmcid.clone()
            } else {
                format!("PMC{}", pmcid)
            };
            target_keys.push(format!("PMC:{}", norm_pmc));
        }
        if !source.is_empty() && !ext_id.is_empty() {
            let key = if source == "PMC" && !ext_id.to_ascii_uppercase().starts_with("PMC") {
                format!("PMC:PMC{}", ext_id)
            } else {
                format!("{}:{}", source, ext_id)
            };
            if !target_keys.contains(&key) {
                target_keys.push(key);
            }
        }
        if target_keys.is_empty() {
            continue;
        }

        let Some(ann_arr) = art.get("annotations").and_then(|v| v.as_array()) else {
            continue;
        };

        let mut parsed_links: Vec<BioEntityBridgeLink> = Vec::new();
        for ann in ann_arr {
            let ann_type = ann.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let sub_type = ann.get("subType").and_then(|v| v.as_str()).unwrap_or("");
            let exact = ann
                .get("exact")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let section = ann
                .get("section")
                .and_then(|v| v.as_str())
                .unwrap_or("Article")
                .trim();
            let provider = ann
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("Europe PMC")
                .trim();
            let freq = ann
                .get("frequency")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1) as u32;

            let tags = ann
                .get("tags")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            if tags.is_empty() {
                if let Some((kind, eid, uri)) = classify_epmc_annotation(ann_type, sub_type, exact, "") {
                    merge_bio_entity_into_paper(
                        &mut parsed_links,
                        BioEntityBridgeLink {
                            entity_id: eid,
                            kind,
                            label: exact.to_string(),
                            uri,
                            provider: provider.to_string(),
                            section: section.to_string(),
                            mention_count: freq,
                            linked_pdb_ids: Vec::new(),
                            resolution_angstrom: None,
                        },
                    );
                }
            } else {
                for tag in &tags {
                    let tag_name = tag
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(exact)
                        .trim();
                    let tag_uri = tag
                        .get("uri")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if let Some((kind, eid, canon_uri)) =
                        classify_epmc_annotation(ann_type, sub_type, tag_name, tag_uri)
                    {
                        let label = if !tag_name.is_empty() && !tag_name.eq_ignore_ascii_case(&eid) {
                            tag_name.to_string()
                        } else if !exact.is_empty() {
                            exact.to_string()
                        } else {
                            eid.clone()
                        };
                        merge_bio_entity_into_paper(
                            &mut parsed_links,
                            BioEntityBridgeLink {
                                entity_id: eid,
                                kind,
                                label,
                                uri: canon_uri,
                                provider: provider.to_string(),
                                section: section.to_string(),
                                mention_count: freq,
                                linked_pdb_ids: Vec::new(),
                                resolution_angstrom: None,
                            },
                        );
                    }
                }
            }

            // Cap per-article annotations to top 24 distinct grounded bio-entities to avoid stopword noise
            if parsed_links.len() >= 24 {
                break;
            }
        }

        for k in target_keys {
            let slot = result.entry(k).or_default();
            for lk in &parsed_links {
                merge_bio_entity_into_paper(slot, lk.clone());
            }
        }
    }

    result
}

fn classify_epmc_annotation(
    ann_type: &str,
    sub_type: &str,
    name_or_exact: &str,
    raw_uri: &str,
) -> Option<(BioEntityKind, String, String)> {
    let u_low = raw_uri.to_ascii_lowercase();
    let t_low = ann_type.to_ascii_lowercase();
    let s_low = sub_type.to_ascii_lowercase();

    // 1. UniProtKB protein accessions from URI or Gene_Proteins / Accession Numbers
    if u_low.contains("uniprot") {
        let acc = extract_trailing_token(raw_uri).to_ascii_uppercase();
        if is_plausible_uniprot_accession(&acc) {
            return Some((
                BioEntityKind::UniProt,
                acc.clone(),
                format!("https://www.uniprot.org/uniprotkb/{}", acc),
            ));
        }
    }

    // 2. RCSB / PDBe 3D macromolecular structure IDs
    if u_low.contains("/pdb/")
        || u_low.contains("identifiers.org/pdb:")
        || u_low.contains("rcsb.org/structure/")
        || u_low.contains("pdbe.org/")
        || s_low == "pdb"
    {
        let raw_id = if !raw_uri.is_empty() {
            extract_trailing_token(raw_uri)
        } else {
            name_or_exact.trim().to_string()
        };
        let pdb_id = raw_id.to_ascii_uppercase();
        if is_plausible_pdb_id(&pdb_id) {
            return Some((
                BioEntityKind::PdbStructure,
                pdb_id.clone(),
                format!("https://www.rcsb.org/structure/{}", pdb_id),
            ));
        }
    }

    // 3. Gene Ontology (GO:xxxxxxx)
    if u_low.contains("go_") || u_low.contains("go:") || t_low.contains("gene ontology") {
        let tok = extract_ontology_curie(raw_uri, name_or_exact, "GO");
        if !tok.is_empty() {
            let digits = tok.trim_start_matches("GO:");
            return Some((
                BioEntityKind::GeneOntology,
                format!("GO:{}", digits),
                if !raw_uri.is_empty() {
                    raw_uri.to_string()
                } else {
                    format!("http://purl.obolibrary.org/obo/GO_{}", digits)
                },
            ));
        }
    }

    // 4. Experimental Factor Ontology / Diseases (EFO / MONDO / DOID / UMLS)
    if u_low.contains("/efo_")
        || u_low.contains("mondo_")
        || u_low.contains("doid_")
        || t_low.contains("disease")
        || t_low.contains("experimental method")
    {
        let tail = extract_trailing_token(raw_uri);
        let eid = if !tail.is_empty() {
            tail.replace('_', ":").to_ascii_uppercase()
        } else if !name_or_exact.trim().is_empty() {
            format!("EFO:{}", sanitize_slug(name_or_exact))
        } else {
            return None;
        };
        let uri = if !raw_uri.is_empty() {
            raw_uri.to_string()
        } else {
            "https://www.ebi.ac.uk/efo/".to_string()
        };
        return Some((BioEntityKind::DiseaseEfo, eid, uri));
    }

    // 5. ChEBI / Chemical entities
    if u_low.contains("chebi") || t_low.contains("chemical") {
        let tok = extract_ontology_curie(raw_uri, name_or_exact, "CHEBI");
        let eid = if !tok.is_empty() {
            tok
        } else if !name_or_exact.trim().is_empty() {
            format!("CHEBI:{}", sanitize_slug(name_or_exact))
        } else {
            return None;
        };
        let uri = if !raw_uri.is_empty() {
            raw_uri.to_string()
        } else {
            "https://www.ebi.ac.uk/chebi/".to_string()
        };
        return Some((BioEntityKind::ChemicalChebi, eid, uri));
    }

    // 6. NCBI Taxonomy Organisms
    if u_low.contains("taxonomy") || t_low.contains("organism") {
        let tail = extract_trailing_token(raw_uri);
        let eid = if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            format!("TAXON:{}", tail)
        } else if !name_or_exact.trim().is_empty() {
            format!("TAXON:{}", sanitize_slug(name_or_exact))
        } else {
            return None;
        };
        let uri = if !raw_uri.is_empty() {
            raw_uri.to_string()
        } else {
            "https://www.ncbi.nlm.nih.gov/taxonomy".to_string()
        };
        return Some((BioEntityKind::OrganismTaxon, eid, uri));
    }

    // 7. Accession Numbers (UniProt, AlphaFold, GenBank, Ensembl, InterPro, Pfam)
    if t_low.contains("accession") || t_low.contains("gene_protein") || t_low.contains("proteins") {
        let cand = if !raw_uri.is_empty() {
            extract_trailing_token(raw_uri)
        } else {
            name_or_exact.trim().to_string()
        };
        let up = cand.to_ascii_uppercase();
        if s_low.contains("uniprot") && is_plausible_uniprot_accession(&up) {
            return Some((
                BioEntityKind::UniProt,
                up.clone(),
                format!("https://www.uniprot.org/uniprotkb/{}", up),
            ));
        }
        if !cand.is_empty() && cand.len() >= 3 {
            let prefix = if !sub_type.is_empty() {
                sub_type.to_ascii_uppercase()
            } else {
                "ACC".to_string()
            };
            let eid = if up.contains(':') {
                up
            } else {
                format!("{}:{}", prefix, cand)
            };
            let uri = if !raw_uri.is_empty() {
                raw_uri.to_string()
            } else {
                "https://europepmc.org/AnnotationsApi".to_string()
            };
            return Some((BioEntityKind::AccessionOther, eid, uri));
        }
    }

    None
}

// ============================================================================
// 3. UNIPROTKB (`rest.uniprot.org`) & RCSB PDB (`data.rcsb.org`) LIVE TRACKERS
// ============================================================================
pub fn track_uniprot_and_pdb_bridges_live(
    crawler: &PoliteWebCrawler,
    papers: &mut [PaperRecord],
) -> Vec<String> {
    let mut warnings = Vec::new();

    // A. Query UniProtKB (`rest.uniprot.org/uniprotkb/search`) for papers with PubMed IDs
    let pmid_candidates: Vec<(usize, String)> = papers
        .iter()
        .enumerate()
        .filter_map(|(i, p)| extract_paper_pmid(p).map(|pm| (i, pm)))
        .take(24)
        .collect();

    for (step, (p_idx, pmid)) in pmid_candidates.iter().enumerate() {
        if step > 0 {
            crawler.sleep_polite(220, 45, step);
        }
        let q = format!("(lit_pubmed_id:{}) AND (reviewed:true)", pmid);
        let url = format!(
            "https://rest.uniprot.org/uniprotkb/search?query={}&fields=accession,id,protein_name,gene_primary,organism_name,xref_pdb&format=json&size=12",
            percent_encode(&q)
        );
        match crawler.get_json(&url) {
            Ok(val) => {
                let links = parse_uniprot_search_results_json(&val);
                for lk in links {
                    // Also materialize cross-referenced PDB structures onto the paper!
                    let linked_pdbs = lk.linked_pdb_ids.clone();
                    let prot_label = lk.label.clone();
                    merge_bio_entity_into_paper(&mut papers[*p_idx].bio_entities, lk);
                    for pdb_id in linked_pdbs.into_iter().take(4) {
                        if is_plausible_pdb_id(&pdb_id) {
                            merge_bio_entity_into_paper(
                                &mut papers[*p_idx].bio_entities,
                                BioEntityBridgeLink {
                                    entity_id: pdb_id.clone(),
                                    kind: BioEntityKind::PdbStructure,
                                    label: format!("3D Structure for {}", prot_label),
                                    uri: format!("https://www.rcsb.org/structure/{}", pdb_id),
                                    provider: "UniProt-PDB-Xref".to_string(),
                                    section: "UniProtCrossRef".to_string(),
                                    mention_count: 1,
                                    linked_pdb_ids: Vec::new(),
                                    resolution_angstrom: None,
                                },
                            );
                        }
                    }
                }
            }
            Err(e) => {
                warnings.push(format!("UniProtKB search skipped for PMID {}: {}", pmid, e));
            }
        }
    }

    // B. Query RCSB PDB Search v2 for primary citations matching corpus DOIs
    let doi_candidates: Vec<(usize, String)> = papers
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.doi.trim().is_empty())
        .take(20)
        .map(|(i, p)| (i, normalize_doi(&p.doi)))
        .collect();

    for (step, (p_idx, doi)) in doi_candidates.iter().enumerate() {
        if step > 0 {
            crawler.sleep_polite(210, 40, step);
        }
        let search_payload = json!({
            "query": {
                "type": "terminal",
                "service": "text",
                "parameters": {
                    "attribute": "rcsb_primary_citation.pdbx_database_id_DOI",
                    "operator": "exact_match",
                    "value": doi
                }
            },
            "return_type": "entry",
            "request_options": {
                "paginate": { "start": 0, "rows": 8 }
            }
        });
        let q_encoded = percent_encode(&search_payload.to_string());
        let search_url = format!(
            "https://search.rcsb.org/rcsbsearch/v2/query?json={}",
            q_encoded
        );
        if let Ok(s_val) = crawler.get_json(&search_url) {
            if let Some(res_set) = s_val.get("result_set").and_then(|v| v.as_array()) {
                for hit in res_set.iter().take(6) {
                    let Some(pdb_id) = hit.get("identifier").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let norm_pdb = pdb_id.trim().to_ascii_uppercase();
                    if !is_plausible_pdb_id(&norm_pdb) {
                        continue;
                    }
                    let entry_url = format!(
                        "https://data.rcsb.org/rest/v1/core/entry/{}",
                        percent_encode(&norm_pdb)
                    );
                    if let Ok(entry_val) = crawler.get_json(&entry_url) {
                        if let Some(pdb_link) = parse_rcsb_pdb_entry_json(&entry_val) {
                            merge_bio_entity_into_paper(&mut papers[*p_idx].bio_entities, pdb_link);
                        }
                    } else {
                        merge_bio_entity_into_paper(
                            &mut papers[*p_idx].bio_entities,
                            BioEntityBridgeLink {
                                entity_id: norm_pdb.clone(),
                                kind: BioEntityKind::PdbStructure,
                                label: format!("RCSB PDB Primary Structure {}", norm_pdb),
                                uri: format!("https://www.rcsb.org/structure/{}", norm_pdb),
                                provider: "RCSB PDB".to_string(),
                                section: "PrimaryCitation".to_string(),
                                mention_count: 1,
                                linked_pdb_ids: Vec::new(),
                                resolution_angstrom: None,
                            },
                        );
                    }
                }
            }
        }
    }

    // C. Hydrate any PDB entries discovered via Europe PMC / UniProt that still lack resolution/title
    let mut unhydrated_pdbs: HashSet<String> = HashSet::new();
    for p in papers.iter() {
        for b in &p.bio_entities {
            if b.kind == BioEntityKind::PdbStructure
                && b.resolution_angstrom.is_none()
                && unhydrated_pdbs.len() < 16
            {
                unhydrated_pdbs.insert(b.entity_id.clone());
            }
        }
    }
    let mut hydrated_map: HashMap<String, BioEntityBridgeLink> = HashMap::new();
    for (step, pdb_id) in unhydrated_pdbs.iter().enumerate() {
        if step > 0 {
            crawler.sleep_polite(160, 35, step);
        }
        let entry_url = format!(
            "https://data.rcsb.org/rest/v1/core/entry/{}",
            percent_encode(pdb_id)
        );
        if let Ok(entry_val) = crawler.get_json(&entry_url) {
            if let Some(parsed) = parse_rcsb_pdb_entry_json(&entry_val) {
                hydrated_map.insert(pdb_id.clone(), parsed);
            }
        }
    }
    if !hydrated_map.is_empty() {
        for p in papers.iter_mut() {
            let updates: Vec<BioEntityBridgeLink> = p
                .bio_entities
                .iter()
                .filter_map(|b| {
                    if b.kind == BioEntityKind::PdbStructure {
                        hydrated_map.get(&b.entity_id).cloned()
                    } else {
                        None
                    }
                })
                .collect();
            for u in updates {
                merge_bio_entity_into_paper(&mut p.bio_entities, u);
            }
        }
    }

    warnings
}

/// Parses post-2022 UniProtKB REST `/uniprotkb/search` JSON (`{ "results": [ ... ] }`).
pub fn parse_uniprot_search_results_json(val: &Value) -> Vec<BioEntityBridgeLink> {
    let mut out = Vec::new();
    let Some(arr) = val.get("results").and_then(|v| v.as_array()) else {
        return out;
    };
    for entry in arr {
        if let Some(link) = parse_uniprot_entry_json(entry) {
            out.push(link);
        }
    }
    out
}

/// Parses a single post-2022 UniProtKB JSON record (`https://rest.uniprot.org/uniprotkb/{acc}.json`),
/// extracting `primaryAccession`, `uniProtkbId`, `proteinDescription`, `genes`, `organism`,
/// and `uniProtKBCrossReferences` (`database == "PDB"`).
pub fn parse_uniprot_entry_json(entry: &Value) -> Option<BioEntityBridgeLink> {
    let acc = entry
        .get("primaryAccession")
        .and_then(|v| v.as_str())?
        .trim()
        .to_ascii_uppercase();
    if acc.is_empty() {
        return None;
    }

    let kb_id = entry
        .get("uniProtkbId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let prot_name = entry
        .get("proteinDescription")
        .and_then(|pd| {
            pd.get("recommendedName")
                .or_else(|| {
                    pd.get("submissionNames")
                        .and_then(|a| a.as_array())
                        .and_then(|a| a.first())
                })
                .and_then(|rn| rn.get("fullName"))
                .and_then(|fnm| fnm.get("value"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("");
    let gene_sym = entry
        .get("genes")
        .and_then(|g| g.as_array())
        .and_then(|a| a.first())
        .and_then(|g0| g0.get("geneName"))
        .and_then(|gn| gn.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let org_name = entry
        .get("organism")
        .and_then(|o| o.get("scientificName"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut linked_pdb_ids = Vec::new();
    if let Some(xrefs) = entry
        .get("uniProtKBCrossReferences")
        .and_then(|v| v.as_array())
    {
        for xr in xrefs {
            let db = xr.get("database").and_then(|v| v.as_str()).unwrap_or("");
            if db.eq_ignore_ascii_case("PDB") {
                if let Some(pid) = xr.get("id").and_then(|v| v.as_str()) {
                    let norm_p = pid.trim().to_ascii_uppercase();
                    if is_plausible_pdb_id(&norm_p) && !linked_pdb_ids.contains(&norm_p) {
                        linked_pdb_ids.push(norm_p);
                    }
                }
            }
        }
    }

    let mut label = if !prot_name.is_empty() && !gene_sym.is_empty() {
        format!("{} ({})", prot_name, gene_sym)
    } else if !prot_name.is_empty() {
        prot_name.to_string()
    } else if !kb_id.is_empty() {
        kb_id.to_string()
    } else {
        acc.clone()
    };
    if !org_name.is_empty() {
        label.push_str(&format!(" [{}]", org_name));
    }

    Some(BioEntityBridgeLink {
        entity_id: acc.clone(),
        kind: BioEntityKind::UniProt,
        label,
        uri: format!("https://www.uniprot.org/uniprotkb/{}", acc),
        provider: "UniProtKB".to_string(),
        section: "CuratedReference".to_string(),
        mention_count: 1,
        linked_pdb_ids,
        resolution_angstrom: None,
    })
}

/// Parses an RCSB PDB Data API v1 Core Entry JSON (`https://data.rcsb.org/rest/v1/core/entry/{id}`).
pub fn parse_rcsb_pdb_entry_json(val: &Value) -> Option<BioEntityBridgeLink> {
    let pdb_id = val
        .get("rcsb_id")
        .and_then(|v| v.as_str())
        .or_else(|| {
            val.get("entry")
                .and_then(|e| e.get("id"))
                .and_then(|v| v.as_str())
        })?
        .trim()
        .to_ascii_uppercase();
    if !is_plausible_pdb_id(&pdb_id) {
        return None;
    }

    let title = val
        .get("struct")
        .and_then(|s| s.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("Macromolecular 3D Structure")
        .trim();
    let method = val
        .get("exptl")
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .and_then(|e0| e0.get("method"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let resolution = val
        .get("rcsb_entry_info")
        .and_then(|ri| ri.get("resolution_combined"))
        .and_then(|rc| {
            rc.as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_f64())
                .or_else(|| rc.as_f64())
        })
        .map(|r| r as f32);

    let label = match (method.is_empty(), resolution) {
        (false, Some(res)) => format!("{} ({}, {:.2} A)", title, method, res),
        (false, None) => format!("{} ({})", title, method),
        (true, Some(res)) => format!("{} ({:.2} A)", title, res),
        (true, None) => title.to_string(),
    };

    Some(BioEntityBridgeLink {
        entity_id: pdb_id.clone(),
        kind: BioEntityKind::PdbStructure,
        label,
        uri: format!("https://www.rcsb.org/structure/{}", pdb_id),
        provider: "RCSB PDB".to_string(),
        section: "PrimaryCitation".to_string(),
        mention_count: 1,
        linked_pdb_ids: Vec::new(),
        resolution_angstrom: resolution,
    })
}

// ============================================================================
// 4. SHARED BIO-ENTITY PAPER BRIDGES & UNIPROT <-> PDB STRUCTURAL EDGES
// ============================================================================
/// Projects shared `UniProt`, `PdbStructure`, `GeneOntology`, `DiseaseEfo`, and `ChemicalChebi`
/// entities across corpus papers into chronological directed citation-complement edges.
pub fn synthesize_shared_bio_entity_edges(papers: &[PaperRecord]) -> Vec<CitationEdge> {
    let mut entity_to_papers: HashMap<(BioEntityKind, String), Vec<usize>> = HashMap::new();
    for (idx, p) in papers.iter().enumerate() {
        for b in &p.bio_entities {
            if b.entity_id.is_empty() || b.kind == BioEntityKind::OrganismTaxon {
                continue;
            }
            let key = (b.kind, b.entity_id.to_ascii_uppercase());
            let vec = entity_to_papers.entry(key).or_default();
            if !vec.contains(&idx) {
                vec.push(idx);
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for ((kind, eid), mut members) in entity_to_papers {
        if members.len() < 2 {
            continue;
        }
        members.sort_by_key(|&i| (papers[i].year, papers[i].id.clone()));
        let anchor = members[0];
        for &later in members.iter().skip(1) {
            let src = papers[later].id.clone();
            let tgt = papers[anchor].id.clone();
            if src != tgt && seen.insert((src.clone(), tgt.clone())) {
                out.push(CitationEdge {
                    source: src,
                    target: tgt,
                    provenance: format!("bio_shared_{}:{}", kind.as_str(), eid),
                });
            }
        }
    }
    out
}

/// Returns deduplicated `(uniprot_node_id, pdb_node_id)` pairs (e.g., `("bio:uniprot:P0DTC2", "bio:pdb_structure:6VXX")`)
/// whenever a `UniProt` entity cross-references a `PdbStructure` entity that is also present in the corpus.
pub fn extract_uniprot_pdb_cross_edges(papers: &[PaperRecord]) -> Vec<(String, String)> {
    let mut active_pdbs: HashSet<String> = HashSet::new();
    for p in papers {
        for b in &p.bio_entities {
            if b.kind == BioEntityKind::PdbStructure {
                active_pdbs.insert(b.entity_id.to_ascii_uppercase());
            }
        }
    }
    let mut cross_pairs: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for p in papers {
        for b in &p.bio_entities {
            if b.kind == BioEntityKind::UniProt {
                let u_node = format!("bio:uniprot:{}", b.entity_id.to_ascii_uppercase());
                for pdb in &b.linked_pdb_ids {
                    let norm_p = pdb.to_ascii_uppercase();
                    if active_pdbs.contains(&norm_p) {
                        let p_node = format!("bio:pdb_structure:{}", norm_p);
                        if seen.insert((u_node.clone(), p_node.clone())) {
                            cross_pairs.push((u_node.clone(), p_node));
                        }
                    }
                }
            }
        }
    }
    cross_pairs
}

// ============================================================================
// 5. IDENTIFIER VALIDATORS & CURIE HELPERS
// ============================================================================
pub fn is_plausible_uniprot_accession(s: &str) -> bool {
    let len = s.len();
    if len != 6 && len != 10 {
        return false;
    }
    let bytes = s.as_bytes();
    bytes[0].is_ascii_uppercase()
        && bytes[1].is_ascii_digit()
        && bytes.iter().all(|b| b.is_ascii_alphanumeric())
}

pub fn is_plausible_pdb_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 4
        && bytes[0].is_ascii_digit()
        && bytes[1..].iter().all(|b| b.is_ascii_alphanumeric())
}

fn extract_trailing_token(uri: &str) -> String {
    let trimmed = uri.trim().trim_end_matches('/');
    let after_slash = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let after_hash = after_slash.rsplit('#').next().unwrap_or(after_slash);
    after_hash
        .rsplit(':')
        .next()
        .unwrap_or(after_hash)
        .trim()
        .to_string()
}

fn extract_ontology_curie(uri: &str, fallback: &str, prefix: &str) -> String {
    let p_up = prefix.to_ascii_uppercase();
    for src in [uri, fallback] {
        let up = src.to_ascii_uppercase();
        for sep in [format!("{}_", p_up), format!("{}:", p_up)] {
            if let Some(pos) = up.find(&sep) {
                let rest = &up[pos + sep.len()..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    return format!("{}:{}", p_up, digits);
                }
            }
        }
    }
    String::new()
}

fn sanitize_slug(raw: &str) -> String {
    let mut slug = String::new();
    for ch in raw.chars().take(28) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_uppercase());
        } else if (ch == ' ' || ch == '-' || ch == '_') && !slug.ends_with('_') {
            slug.push('_');
        }
    }
    slug.trim_matches('_').to_string()
}
