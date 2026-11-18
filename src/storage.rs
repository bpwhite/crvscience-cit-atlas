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
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection};

use crate::crawler::extract_year_from_str;
use crate::models::{
    infer_concepts_from_text, BioEntityBridgeLink, BioEntityKind, CitationEdge,
    DataCiteArtifactKind, DataCiteLink, Discipline, DownloadItemReport, OaStatus, PaperRecord,
    RetractionRecord, RetractionStatus,
};
use crate::rate_limiter::{CrawlAuditEntry, CrawlAuditOutcome, DomainBucketPolicy};
use crate::search_providers::normalize_doi;

pub struct LedgerStore {
    conn: Connection,
}

impl LedgerStore {
    pub fn open(db_path: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open SQLite ledger {:?}: {}", db_path, e))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS papers (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 authors TEXT NOT NULL,
                 year INTEGER NOT NULL,
                 venue TEXT NOT NULL,
                 disc TEXT NOT NULL,
                 doi TEXT NOT NULL,
                 arxiv_id TEXT NOT NULL,
                 openalex_id TEXT NOT NULL,
                 s2_id TEXT NOT NULL,
                 pmcid TEXT NOT NULL,
                 source_url TEXT NOT NULL,
                 oa_pdf_url TEXT NOT NULL,
                 oa_status TEXT NOT NULL,
                 external_citations INTEGER,
                 abstract_text TEXT NOT NULL,
                 terms_json TEXT NOT NULL,
                 concepts_json TEXT NOT NULL,
                 local_pdf_path TEXT,
                 pdf_sha256 TEXT,
                 pdf_bytes INTEGER,
                 retraction_json TEXT NOT NULL DEFAULT '{}',
                 datacite_json TEXT NOT NULL DEFAULT '[]',
                 bio_entities_json TEXT NOT NULL DEFAULT '[]'
             );
             CREATE TABLE IF NOT EXISTS citations (
                 source_id TEXT NOT NULL,
                 target_id TEXT NOT NULL,
                 provenance TEXT NOT NULL,
                 PRIMARY KEY (source_id, target_id)
             );
             CREATE TABLE IF NOT EXISTS downloads (
                 paper_id TEXT PRIMARY KEY,
                 doi TEXT NOT NULL,
                 status TEXT NOT NULL,
                 resolved_stage TEXT NOT NULL,
                 final_pdf_url TEXT NOT NULL,
                 saved_path TEXT,
                  byte_len INTEGER NOT NULL,
                  sha256 TEXT
              );
              CREATE TABLE IF NOT EXISTS domain_rate_policies (
                  domain TEXT PRIMARY KEY,
                  label TEXT NOT NULL,
                  refill_rps REAL NOT NULL,
                  burst_capacity REAL NOT NULL,
                  min_spacing_ms INTEGER NOT NULL,
                  jitter_ms INTEGER NOT NULL,
                  enabled INTEGER NOT NULL,
                  paused INTEGER NOT NULL,
                  server_pool_note TEXT NOT NULL DEFAULT ''
              );
              CREATE TABLE IF NOT EXISTS crawl_audit_log (
                  seq_id INTEGER PRIMARY KEY,
                  timestamp_utc TEXT NOT NULL,
                  epoch_ms INTEGER NOT NULL,
                  subsystem TEXT NOT NULL,
                  domain TEXT NOT NULL,
                  method TEXT NOT NULL,
                  url TEXT NOT NULL,
                  outcome TEXT NOT NULL,
                  http_status INTEGER NOT NULL,
                  token_wait_ms INTEGER NOT NULL,
                  latency_ms INTEGER NOT NULL,
                  bytes_received INTEGER NOT NULL,
                  tokens_remaining REAL NOT NULL,
                  retry_after_ms INTEGER,
                  robots_status TEXT NOT NULL,
                  note TEXT NOT NULL
              );",
        )
        .map_err(|e| format!("Failed initializing SQLite schema: {}", e))?;

        // Seamless in-place schema migration for v1.0-v1.2 SQLite databases
        let _ = conn.execute(
            "ALTER TABLE papers ADD COLUMN retraction_json TEXT NOT NULL DEFAULT '{}'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE papers ADD COLUMN datacite_json TEXT NOT NULL DEFAULT '[]'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE papers ADD COLUMN bio_entities_json TEXT NOT NULL DEFAULT '[]'",
            [],
        );

        Ok(Self { conn })
    }

    pub fn save_corpus(
        &mut self,
        papers: &[PaperRecord],
        edges: &[CitationEdge],
        replace_existing: bool,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("SQLite transaction error: {}", e))?;

        if replace_existing {
            tx.execute("DELETE FROM papers", [])
                .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM citations", [])
                .map_err(|e| e.to_string())?;
        }

        for p in papers {
            let terms_json = serde_json::to_string(&p.terms).unwrap_or_else(|_| "[]".to_string());
            let concepts_json =
                serde_json::to_string(&p.concepts).unwrap_or_else(|_| "[]".to_string());
            let retraction_json =
                serde_json::to_string(&p.retraction).unwrap_or_else(|_| "{}".to_string());
            let datacite_json =
                serde_json::to_string(&p.datacite_links).unwrap_or_else(|_| "[]".to_string());
            let bio_entities_json =
                serde_json::to_string(&p.bio_entities).unwrap_or_else(|_| "[]".to_string());
            let ext_cites = p.external_citations.map(|c| c as i64);
            let p_bytes = p.pdf_bytes.map(|b| b as i64);

            tx.execute(
                "INSERT OR REPLACE INTO papers (
                    id, title, authors, year, venue, disc, doi, arxiv_id, openalex_id, s2_id, pmcid,
                    source_url, oa_pdf_url, oa_status, external_citations, abstract_text,
                    terms_json, concepts_json, local_pdf_path, pdf_sha256, pdf_bytes,
                    retraction_json, datacite_json, bio_entities_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
                params![
                    p.id,
                    p.title,
                    p.authors,
                    p.year,
                    p.venue,
                    p.disc.code(),
                    p.doi,
                    p.arxiv_id,
                    p.openalex_id,
                    p.s2_id,
                    p.pmcid,
                    p.source_url,
                    p.oa_pdf_url,
                    p.oa_status.as_str(),
                    ext_cites,
                    p.abstract_text,
                    terms_json,
                    concepts_json,
                    p.local_pdf_path,
                    p.pdf_sha256,
                    p_bytes,
                    retraction_json,
                    datacite_json,
                    bio_entities_json,
                ],
            )
            .map_err(|e| format!("Failed inserting paper {}: {}", p.id, e))?;
        }

        for e in edges {
            tx.execute(
                "INSERT OR REPLACE INTO citations (source_id, target_id, provenance) VALUES (?1, ?2, ?3)",
                params![e.source, e.target, e.provenance],
            )
            .map_err(|err| format!("Failed inserting citation edge: {}", err))?;
        }

        tx.commit().map_err(|e| format!("Commit error: {}", e))
    }

    pub fn record_downloads(&mut self, reports: &[DownloadItemReport]) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        for r in reports {
            tx.execute(
                "INSERT OR REPLACE INTO downloads (
                    paper_id, doi, status, resolved_stage, final_pdf_url, saved_path, byte_len, sha256
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    r.paper_id,
                    r.doi,
                    r.status.label(),
                    r.resolved_stage,
                    r.final_pdf_url,
                    r.saved_path,
                    r.byte_len as i64,
                    r.sha256,
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn load_corpus(&self) -> Result<(Vec<PaperRecord>, Vec<CitationEdge>), String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, authors, year, venue, disc, doi, arxiv_id, openalex_id, s2_id, pmcid,
                        source_url, oa_pdf_url, oa_status, external_citations, abstract_text,
                        terms_json, concepts_json, local_pdf_path, pdf_sha256, pdf_bytes,
                        retraction_json, datacite_json, bio_entities_json
                 FROM papers ORDER BY year ASC, id ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let disc_code: String = row.get(5)?;
                let oa_str: String = row.get(13)?;
                let ext_c: Option<i64> = row.get(14)?;
                let terms_raw: String = row.get(16)?;
                let concepts_raw: String = row.get(17)?;
                let p_bytes: Option<i64> = row.get(20)?;
                let ret_raw: String = row.get(21)?;
                let dc_raw: String = row.get(22)?;
                let bio_raw: String = row.get(23).unwrap_or_else(|_| "[]".to_string());

                let terms: Vec<String> = serde_json::from_str(&terms_raw).unwrap_or_default();
                let concepts: Vec<String> = serde_json::from_str(&concepts_raw).unwrap_or_default();
                let retraction: RetractionRecord =
                    serde_json::from_str(&ret_raw).unwrap_or_default();
                let datacite_links: Vec<DataCiteLink> =
                    serde_json::from_str(&dc_raw).unwrap_or_default();
                let bio_entities: Vec<BioEntityBridgeLink> =
                    serde_json::from_str(&bio_raw).unwrap_or_default();

                Ok(PaperRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    authors: row.get(2)?,
                    year: row.get(3)?,
                    venue: row.get(4)?,
                    disc: Discipline::from_code(&disc_code),
                    doi: row.get(6)?,
                    arxiv_id: row.get(7)?,
                    openalex_id: row.get(8)?,
                    s2_id: row.get(9)?,
                    pmcid: row.get(10)?,
                    source_url: row.get(11)?,
                    oa_pdf_url: row.get(12)?,
                    oa_status: OaStatus::from_str_lossy(&oa_str),
                    external_citations: ext_c.map(|v| v.max(0) as u64),
                    abstract_text: row.get(15)?,
                    terms,
                    concepts,
                    local_pdf_path: row.get(18)?,
                    pdf_sha256: row.get(19)?,
                    pdf_bytes: p_bytes.map(|v| v.max(0) as u64),
                    raw_references: Vec::new(),
                    retraction,
                    datacite_links,
                    bio_entities,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut papers = Vec::new();
        for r in rows {
            papers.push(r.map_err(|e| e.to_string())?);
        }

        let mut e_stmt = self
            .conn
            .prepare("SELECT source_id, target_id, provenance FROM citations")
            .map_err(|e| e.to_string())?;
        let e_rows = e_stmt
            .query_map([], |row| {
                Ok(CitationEdge {
                    source: row.get(0)?,
                    target: row.get(1)?,
                    provenance: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut edges = Vec::new();
        for e in e_rows {
            edges.push(e.map_err(|err| err.to_string())?);
        }

        Ok((papers, edges))
    }

    pub fn save_rate_policies_and_audit(
        &mut self,
        policies: &[DomainBucketPolicy],
        entries: &[CrawlAuditEntry],
    ) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        for p in policies {
            tx.execute(
                "INSERT OR REPLACE INTO domain_rate_policies (
                    domain, label, refill_rps, burst_capacity, min_spacing_ms, jitter_ms,
                    enabled, paused, server_pool_note
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    p.domain,
                    p.label,
                    p.refill_rps,
                    p.burst_capacity,
                    p.min_spacing_ms as i64,
                    p.jitter_ms as i64,
                    i32::from(p.enabled),
                    i32::from(p.paused),
                    p.server_pool_note,
                ],
            )
            .map_err(|e| format!("Failed saving domain policy {}: {}", p.domain, e))?;
        }
        tx.execute("DELETE FROM crawl_audit_log", [])
            .map_err(|e| e.to_string())?;
        for e in entries {
            tx.execute(
                "INSERT OR REPLACE INTO crawl_audit_log (
                    seq_id, timestamp_utc, epoch_ms, subsystem, domain, method, url,
                    outcome, http_status, token_wait_ms, latency_ms, bytes_received,
                    tokens_remaining, retry_after_ms, robots_status, note
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    e.seq_id as i64,
                    e.timestamp_utc,
                    e.epoch_ms as i64,
                    e.subsystem,
                    e.domain,
                    e.method,
                    e.url,
                    e.outcome.as_str(),
                    e.http_status as i64,
                    e.token_wait_ms as i64,
                    e.latency_ms as i64,
                    e.bytes_received as i64,
                    e.tokens_remaining,
                    e.retry_after_ms.map(|v| v as i64),
                    e.robots_status,
                    e.note,
                ],
            )
            .map_err(|err| format!("Failed saving crawl audit entry: {}", err))?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn load_rate_policies_and_audit(
        &self,
    ) -> Result<(Vec<DomainBucketPolicy>, Vec<CrawlAuditEntry>), String> {
        let mut p_stmt = self
            .conn
            .prepare(
                "SELECT domain, label, refill_rps, burst_capacity, min_spacing_ms, jitter_ms,
                        enabled, paused, server_pool_note
                 FROM domain_rate_policies ORDER BY domain ASC",
            )
            .map_err(|e| e.to_string())?;
        let p_rows = p_stmt
            .query_map([], |row| {
                let burst: f64 = row.get(3)?;
                let en: i32 = row.get(6)?;
                let pa: i32 = row.get(7)?;
                let min_s: i64 = row.get(4)?;
                let jit_s: i64 = row.get(5)?;
                Ok(DomainBucketPolicy {
                    domain: row.get(0)?,
                    label: row.get(1)?,
                    refill_rps: row.get(2)?,
                    burst_capacity: burst,
                    min_spacing_ms: min_s.max(0) as u64,
                    jitter_ms: jit_s.max(0) as u64,
                    enabled: en != 0,
                    paused: pa != 0,
                    tokens_available: burst,
                    cooldown_remaining_ms: 0,
                    total_requests: 0,
                    ok_2xx: 0,
                    throttled_waits: 0,
                    total_wait_ms: 0,
                    http_429_count: 0,
                    http_403_captcha_count: 0,
                    robots_blocked_count: 0,
                    network_errors: 0,
                    bytes_received: 0,
                    ema_latency_ms: 0.0,
                    last_status: 0,
                    server_pool_note: row.get(8)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut policies = Vec::new();
        for r in p_rows {
            policies.push(r.map_err(|e| e.to_string())?);
        }

        let mut a_stmt = self
            .conn
            .prepare(
                "SELECT seq_id, timestamp_utc, epoch_ms, subsystem, domain, method, url,
                        outcome, http_status, token_wait_ms, latency_ms, bytes_received,
                        tokens_remaining, retry_after_ms, robots_status, note
                 FROM crawl_audit_log ORDER BY seq_id ASC",
            )
            .map_err(|e| e.to_string())?;
        let a_rows = a_stmt
            .query_map([], |row| {
                let seq: i64 = row.get(0)?;
                let ep: i64 = row.get(2)?;
                let out_s: String = row.get(7)?;
                let st_c: i64 = row.get(8)?;
                let tw: i64 = row.get(9)?;
                let lat: i64 = row.get(10)?;
                let by: i64 = row.get(11)?;
                let ra: Option<i64> = row.get(13)?;
                Ok(CrawlAuditEntry {
                    seq_id: seq.max(0) as u64,
                    timestamp_utc: row.get(1)?,
                    epoch_ms: ep.max(0) as u64,
                    subsystem: row.get(3)?,
                    domain: row.get(4)?,
                    method: row.get(5)?,
                    url: row.get(6)?,
                    outcome: CrawlAuditOutcome::from_str_lossy(&out_s),
                    http_status: st_c.clamp(0, 999) as u16,
                    token_wait_ms: tw.max(0) as u64,
                    latency_ms: lat.max(0) as u64,
                    bytes_received: by.max(0) as u64,
                    tokens_remaining: row.get(12)?,
                    retry_after_ms: ra.map(|v| v.max(0) as u64),
                    robots_status: row.get(14)?,
                    note: row.get(15)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        for r in a_rows {
            entries.push(r.map_err(|e| e.to_string())?);
        }

        Ok((policies, entries))
    }
}

pub fn import_corpus_file(path: &Path) -> Result<(Vec<PaperRecord>, Vec<CitationEdge>), String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("Failed reading {:?}: {}", path, e))?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "json" {
        #[derive(serde::Deserialize)]
        struct JsonBundle {
            #[serde(default)]
            papers: Vec<PaperRecord>,
            #[serde(default)]
            edges: Vec<CitationEdge>,
        }
        if let Ok(bundle) = serde_json::from_str::<JsonBundle>(&text) {
            if !bundle.papers.is_empty() {
                return Ok((bundle.papers, bundle.edges));
            }
        }
        let csl_papers = parse_csl_json_text(&text)
            .map_err(|e| format!("Invalid corpus JSON / CSL-JSON: {}", e))?;
        if csl_papers.is_empty() {
            return Err("JSON file contains 0 papers".to_string());
        }
        Ok((csl_papers, Vec::new()))
    } else if ext == "csl" || ext == "csljson" {
        let papers = parse_csl_json_text(&text)?;
        if papers.is_empty() {
            return Err("No valid CSL-JSON bibliography items found".to_string());
        }
        Ok((papers, Vec::new()))
    } else if ext == "ris" {
        let papers = parse_ris_text(&text);
        if papers.is_empty() {
            return Err("No valid RIS reference records found".to_string());
        }
        Ok((papers, Vec::new()))
    } else if ext == "nbib" || ext == "medline" {
        let papers = parse_nbib_text(&text);
        if papers.is_empty() {
            return Err("No valid PubMed NBIB / MEDLINE records found".to_string());
        }
        Ok((papers, Vec::new()))
    } else if ext == "csv" {
        let (papers, mut edges) = parse_corpus_csv(&text)?;
        if edges.is_empty() {
            if let Some(parent) = path.parent() {
                let sibling_edges = parent.join("citation_edges.csv");
                if sibling_edges != path && sibling_edges.exists() {
                    if let Ok(edge_txt) = fs::read_to_string(&sibling_edges) {
                        edges = parse_edges_csv(&edge_txt);
                    }
                }
            }
        }
        Ok((papers, edges))
    } else if ext == "graphml" {
        parse_graphml_corpus(&text)
    } else if ext == "gexf" {
        parse_gexf_corpus(&text)
    } else if ext == "html" || ext == "htm" {
        let hits = crate::crawler::parse_google_scholar_html(&text)
            .map_err(|b| b.message())?;
        if hits.is_empty() {
            return Err("No Google Scholar SERP entries found in HTML file".to_string());
        }
        let mut req = crate::models::SearchRequest::default();
        req.crawl.local_html_paths = vec![path.to_string_lossy().to_string()];
        req.max_per_provider = hits.len();
        let mut crawler = crate::crawler::PoliteWebCrawler::new(&req.polite_email, "")?;
        let (papers, _) = crate::crawler::crawl_google_scholar(&mut crawler, &req)?;
        Ok((papers, Vec::new()))
    } else {
        let papers = parse_bibtex_text(&text);
        if papers.is_empty() {
            return Err("No BibTeX @article/@inproceedings entries found".to_string());
        }
        Ok((papers, Vec::new()))
    }
}

/// RFC 4180 CSV parser handling quoted fields, doubled quotes (`""`), and multiline cells.
pub fn parse_rfc4180_csv(text: &str) -> Vec<Vec<String>> {
    let cleaned = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_field = String::new();
    let mut in_quotes = false;
    let mut chars = cleaned.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current_field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                current_field.push(ch);
            }
        } else {
            match ch {
                '"' => in_quotes = true,
                ',' => {
                    current_row.push(current_field);
                    current_field = String::new();
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    current_row.push(current_field);
                    current_field = String::new();
                    if current_row.iter().any(|c| !c.trim().is_empty()) {
                        rows.push(current_row);
                    }
                    current_row = Vec::new();
                }
                '\n' => {
                    current_row.push(current_field);
                    current_field = String::new();
                    if current_row.iter().any(|c| !c.trim().is_empty()) {
                        rows.push(current_row);
                    }
                    current_row = Vec::new();
                }
                _ => current_field.push(ch),
            }
        }
    }

    if !current_field.is_empty() || !current_row.is_empty() {
        current_row.push(current_field);
        if current_row.iter().any(|c| !c.trim().is_empty()) {
            rows.push(current_row);
        }
    }

    rows
}

fn parse_serialized_bio_tokens(raw: &str) -> Vec<BioEntityBridgeLink> {
    let mut out = Vec::new();
    for entry in raw.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = entry.splitn(3, ':').collect();
        let (kind, eid, lbl) = if parts.len() == 3 {
            let k = BioEntityKind::from_str_lossy(parts[0]);
            let id_s = if matches!(k, BioEntityKind::GeneOntology | BioEntityKind::DiseaseEfo | BioEntityKind::ChemicalChebi | BioEntityKind::OrganismTaxon) && parts[2].chars().all(|c| c.is_ascii_digit()) {
                format!("{}:{}", parts[1], parts[2])
            } else {
                parts[1].to_string()
            };
            (k, id_s.clone(), parts[2].to_string())
        } else if parts.len() == 2 {
            let k = BioEntityKind::from_str_lossy(parts[0]);
            (k, parts[1].to_string(), parts[1].to_string())
        } else {
            continue;
        };
        if eid.is_empty() {
            continue;
        }
        let uri = match kind {
            BioEntityKind::UniProt => format!("https://www.uniprot.org/uniprotkb/{}", eid),
            BioEntityKind::PdbStructure => format!("https://www.rcsb.org/structure/{}", eid),
            _ => "https://europepmc.org/AnnotationsApi".to_string(),
        };
        out.push(BioEntityBridgeLink {
            entity_id: eid,
            kind,
            label: lbl,
            uri,
            provider: "Imported".to_string(),
            section: "Imported".to_string(),
            mention_count: 1,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: None,
        });
    }
    out
}

fn parse_serialized_datacite_tokens(raw: &str) -> Vec<DataCiteLink> {
    let mut links = Vec::new();
    for entry in raw.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = entry.splitn(3, ':').collect();
        if parts.len() == 3 {
            let kind = DataCiteArtifactKind::from_resource_type_general(parts[0]);
            let rel = parts[1].to_string();
            let doi = normalize_doi(parts[2]);
            if !doi.is_empty() {
                links.push(DataCiteLink {
                    artifact_doi: doi,
                    artifact_title: "Imported DataCite Artifact".to_string(),
                    artifact_kind: kind,
                    repository: "DataCite".to_string(),
                    relation_type: rel,
                    citation_count: 0,
                    view_count: 0,
                    download_count: 0,
                });
            }
        } else {
            let doi = normalize_doi(entry);
            if !doi.is_empty() {
                links.push(DataCiteLink {
                    artifact_doi: doi,
                    artifact_title: "Imported DataCite Artifact".to_string(),
                    artifact_kind: DataCiteArtifactKind::Dataset,
                    repository: "DataCite".to_string(),
                    relation_type: "Linked".to_string(),
                    citation_count: 0,
                    view_count: 0,
                    download_count: 0,
                });
            }
        }
    }
    links
}

/// Parses CitAtlas `papers_metrics.csv` / `raw_harvest_catalog.csv` (including Retraction Watch &
/// DataCite columns) as well as external Google Scholar / Publish-or-Perish / Zotero CSV exports.
pub fn parse_corpus_csv(text: &str) -> Result<(Vec<PaperRecord>, Vec<CitationEdge>), String> {
    let rows = parse_rfc4180_csv(text);
    if rows.len() < 2 {
        return Err("CSV file requires a header row and at least 1 data row".to_string());
    }

    let header: HashMap<String, usize> = rows[0]
        .iter()
        .enumerate()
        .map(|(idx, col)| {
            let norm = col
                .trim()
                .to_ascii_lowercase()
                .replace([' ', '-'], "_");
            (norm, idx)
        })
        .collect();

    let get_col = |row: &[String], aliases: &[&str]| -> String {
        for &alias in aliases {
            if let Some(&col_idx) = header.get(alias) {
                if let Some(val) = row.get(col_idx) {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
            }
        }
        String::new()
    };

    if header.contains_key("source_id") && header.contains_key("target_id") && !header.contains_key("title") {
        return Err("This CSV is an edge list (source_id,target_id). Import the corresponding papers CSV (citation_edges.csv in the same folder is linked automatically).".to_string());
    }

    let mut papers = Vec::new();
    for (r_idx, row) in rows.iter().skip(1).enumerate() {
        let title = get_col(row, &["title", "item_title", "document_title", "article_title"]);
        if title.is_empty() {
            continue;
        }
        let authors = {
            let a = get_col(row, &["authors", "author", "creator"]);
            if a.is_empty() {
                "CSV Imported Author".to_string()
            } else {
                a
            }
        };
        let year = get_col(row, &["year", "publication_year", "pub_year"])
            .parse::<i32>()
            .unwrap_or(2024);
        let venue = {
            let v = get_col(
                row,
                &["venue", "source", "publication", "journal", "publication_title", "container_title"],
            );
            if v.is_empty() {
                "CSV Imported Venue".to_string()
            } else {
                v
            }
        };
        let doi = normalize_doi(&get_col(row, &["doi"]));
        let arxiv_id = get_col(row, &["arxiv_id", "arxiv", "eprint"]);
        let openalex_id = get_col(row, &["openalex_id"]);
        let s2_id = get_col(row, &["s2_id", "paper_id"]);
        let pmcid = get_col(row, &["pmcid", "pmc"]);
        let source_url = get_col(row, &["source_url", "articleurl", "article_url", "url", "link"]);
        let oa_pdf_url = get_col(row, &["oa_pdf_url", "fulltexturl", "fulltext_url", "pdf_url"]);
        let oa_status = OaStatus::from_str_lossy(&get_col(row, &["oa_status"]));
        let ext_cites = get_col(
            row,
            &["external_citations", "cites", "cited_by", "citation_count", "in_citations"],
        )
        .parse::<u64>()
        .ok();
        let abstract_text = get_col(row, &["abstract_text", "abstract", "snippet"]);

        let disc_str = get_col(row, &["discipline", "disc"]);
        let combined = format!("{} {} {}", title, venue, abstract_text);
        let disc = if !disc_str.is_empty() {
            Discipline::from_code(&disc_str)
        } else {
            Discipline::infer_from_text(&combined)
        };

        let concepts_raw = get_col(row, &["concepts"]);
        let concepts = if !concepts_raw.is_empty() {
            concepts_raw
                .split([';', ','])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            infer_concepts_from_text(&combined, disc)
        };

        let terms_raw = get_col(row, &["terms", "keywords"]);
        let terms = if !terms_raw.is_empty() {
            terms_raw
                .split([';', ','])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            concepts.clone()
        };

        let id = {
            let raw_id = get_col(row, &["id", "citekey", "key"]);
            if !raw_id.is_empty() {
                raw_id
            } else {
                format!("csv_{}_{}", year, r_idx + 1)
            }
        };

        let local_pdf = {
            let lp = get_col(row, &["local_pdf_path", "file"]);
            if lp.is_empty() { None } else { Some(lp) }
        };
        let pdf_sha = {
            let sh = get_col(row, &["pdf_sha256", "sha256"]);
            if sh.is_empty() { None } else { Some(sh) }
        };

        let ret_status = RetractionStatus::from_str_lossy(&get_col(row, &["retraction_status"]));
        let ret_date = get_col(row, &["retraction_date"]);
        let retraction = RetractionRecord {
            status: ret_status,
            source: get_col(row, &["retraction_source"]),
            notice_doi: normalize_doi(&get_col(row, &["retraction_notice_doi"])),
            notice_year: extract_year_from_str(&ret_date),
            notice_date: ret_date,
            record_id: get_col(row, &["retraction_watch_record_id"]),
            reasons: get_col(row, &["retraction_reasons"])
                .split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        };

        let datacite_links =
            parse_serialized_datacite_tokens(&get_col(row, &["datacite_artifact_dois"]));
        let bio_entities =
            parse_serialized_bio_tokens(&get_col(row, &["bio_entity_bridges", "bio_entities"]));

        papers.push(PaperRecord {
            id,
            title,
            authors,
            year,
            venue,
            disc,
            doi,
            arxiv_id,
            openalex_id,
            s2_id,
            pmcid,
            source_url,
            oa_pdf_url,
            oa_status,
            external_citations: ext_cites,
            abstract_text,
            terms,
            concepts,
            local_pdf_path: local_pdf,
            pdf_sha256: pdf_sha,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction,
            datacite_links,
            bio_entities,
        });
    }

    if papers.is_empty() {
        return Err("No valid paper records with a 'title' column found in CSV".to_string());
    }

    Ok((papers, Vec::new()))
}

pub fn parse_edges_csv(text: &str) -> Vec<CitationEdge> {
    let rows = parse_rfc4180_csv(text);
    if rows.len() < 2 {
        return Vec::new();
    }
    let header: HashMap<String, usize> = rows[0]
        .iter()
        .enumerate()
        .map(|(i, c)| (c.trim().to_ascii_lowercase(), i))
        .collect();
    let Some(&s_col) = header.get("source_id").or_else(|| header.get("source")) else {
        return Vec::new();
    };
    let Some(&t_col) = header.get("target_id").or_else(|| header.get("target")) else {
        return Vec::new();
    };
    let p_col = header.get("provenance").copied();

    let mut out = Vec::new();
    for row in rows.iter().skip(1) {
        let src = row.get(s_col).map(|s| s.trim()).unwrap_or("");
        let tgt = row.get(t_col).map(|s| s.trim()).unwrap_or("");
        if !src.is_empty() && !tgt.is_empty() && src != tgt {
            let prov = p_col
                .and_then(|c| row.get(c))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "csv_import".to_string());
            out.push(CitationEdge {
                source: src.to_string(),
                target: tgt.to_string(),
                provenance: prov,
            });
        }
    }
    out
}

pub fn parse_bibtex_text(text: &str) -> Vec<PaperRecord> {
    let mut out = Vec::new();
    for (idx, chunk) in text.split('@').skip(1).enumerate() {
        let Some(brace_pos) = chunk.find('{') else {
            continue;
        };
        let body = &chunk[brace_pos + 1..];
        let cite_key = body
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if cite_key.is_empty() {
            continue;
        }
        let title = extract_bib_field(body, "title");
        if title.is_empty() {
            continue;
        }
        let authors = {
            let a = extract_bib_field(body, "author");
            if a.is_empty() {
                "BibTeX Author".to_string()
            } else {
                normalize_bibtex_authors(&a)
            }
        };
        let year = extract_bib_field(body, "year")
            .parse::<i32>()
            .unwrap_or(2020);
        let venue = {
            let j = extract_bib_field(body, "journal");
            if !j.is_empty() {
                j
            } else {
                let b = extract_bib_field(body, "booktitle");
                if !b.is_empty() {
                    b
                } else {
                    "BibTeX Imported Venue".to_string()
                }
            }
        };
        let doi = normalize_doi(&extract_bib_field(body, "doi"));
        let url = extract_bib_field(body, "url");
        let kw = extract_bib_field(body, "keywords");
        let combined = format!("{} {} {}", title, venue, kw);
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);

        out.push(PaperRecord {
            id: if cite_key.is_empty() {
                format!("bib_{}", idx)
            } else {
                cite_key
            },
            title,
            authors,
            year,
            venue,
            disc,
            source_url: if !url.is_empty() {
                url
            } else if !doi.is_empty() {
                format!("https://doi.org/{}", doi)
            } else {
                String::new()
            },
            doi,
            arxiv_id: extract_bib_field(body, "eprint"),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Unknown,
            external_citations: None,
            abstract_text: extract_bib_field(body, "abstract"),
            terms: if kw.is_empty() {
                concepts.clone()
            } else {
                kw.split(',').map(|s| s.trim().to_string()).collect()
            },
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: RetractionRecord::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }
    out
}

fn extract_bib_field(body: &str, field_name: &str) -> String {
    let needle = field_name.to_ascii_lowercase();
    for line in body.lines() {
        let l_trim = line.trim_start();
        if l_trim.to_ascii_lowercase().starts_with(&needle) {
            let after = l_trim[needle.len()..].trim_start();
            if let Some(eq_rest) = after.strip_prefix('=') {
                let val = eq_rest
                    .trim()
                    .trim_end_matches(',')
                    .trim()
                    .trim_start_matches('{')
                    .trim_start_matches('"')
                    .trim_end_matches('}')
                    .trim_end_matches('"')
                    .trim();
                return val.to_string();
            }
        }
    }
    String::new()
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn xml_attr(tag_header: &str, attr_name: &str) -> Option<String> {
    for q in ['"', '\''] {
        let pat = format!("{}={}", attr_name, q);
        if let Some(pos) = tag_header.find(&pat) {
            let rest = &tag_header[pos + pat.len()..];
            if let Some(end) = rest.find(q) {
                return Some(xml_unescape(&rest[..end]));
            }
        }
    }
    None
}

/// Round-trip GraphML 1.0 importer reconstructing `PaperRecord` and `CitationEdge` collections
/// (skipping bipartite `datacite:...` artifact nodes when reconstructing paper records).
pub fn parse_graphml_corpus(xml: &str) -> Result<(Vec<PaperRecord>, Vec<CitationEdge>), String> {
    let mut key_id_to_name: HashMap<String, String> = HashMap::new();
    for seg in xml.split("<key ").skip(1) {
        let end = seg.find('>').unwrap_or(seg.len());
        let hdr = &seg[..end];
        if let (Some(kid), Some(aname)) = (xml_attr(hdr, "id"), xml_attr(hdr, "attr.name")) {
            key_id_to_name.insert(kid, aname);
        }
    }

    let mut papers = Vec::new();
    let mut paper_ids = std::collections::HashSet::new();

    for seg in xml.split("<node ").skip(1) {
        let Some(close_node) = seg.find("</node>") else {
            continue;
        };
        let block = &seg[..close_node];
        let hdr_end = block.find('>').unwrap_or(block.len());
        let Some(node_id) = xml_attr(&block[..hdr_end], "id") else {
            continue;
        };
        if node_id.starts_with("datacite:") || node_id.starts_with("bio:") {
            continue;
        }

        let mut fields: HashMap<String, String> = HashMap::new();
        for dseg in block[hdr_end..].split("<data ").skip(1) {
            let Some(gt) = dseg.find('>') else { continue };
            let Some(end_d) = dseg.find("</data>") else { continue };
            if end_d < gt {
                continue;
            }
            if let Some(kid) = xml_attr(&dseg[..gt], "key") {
                let col_name = key_id_to_name.get(&kid).cloned().unwrap_or(kid);
                let val = xml_unescape(dseg[gt + 1..end_d].trim());
                fields.insert(col_name, val);
            }
        }

        if let Some(nt) = fields.get("node_type").map(|s| s.as_str()) {
            if nt != "paper" {
                continue;
            }
        }

        let title = fields
            .get("title")
            .or_else(|| fields.get("label"))
            .cloned()
            .unwrap_or_else(|| node_id.clone());
        let authors = fields
            .get("authors")
            .cloned()
            .unwrap_or_else(|| "GraphML Author".to_string());
        let year = fields
            .get("year")
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(2024);
        let venue = fields
            .get("venue")
            .cloned()
            .unwrap_or_else(|| "GraphML Venue".to_string());
        let doi = normalize_doi(fields.get("doi").map(|s| s.as_str()).unwrap_or(""));
        let source_url = fields.get("source_url").cloned().unwrap_or_default();
        let disc = fields
            .get("discipline")
            .map(|d| Discipline::from_code(d))
            .unwrap_or_else(|| Discipline::infer_from_text(&title));
        let concepts = infer_concepts_from_text(&format!("{} {}", title, venue), disc);
        let ret_status = fields
            .get("retraction_status")
            .map(|s| RetractionStatus::from_str_lossy(s))
            .unwrap_or(RetractionStatus::Unscreened);

        paper_ids.insert(node_id.clone());
        papers.push(PaperRecord {
            id: node_id,
            title,
            authors,
            year,
            venue,
            disc,
            doi,
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            source_url,
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Unknown,
            external_citations: None,
            abstract_text: String::new(),
            terms: concepts.clone(),
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: RetractionRecord {
                status: ret_status,
                ..Default::default()
            },
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }

    if papers.is_empty() {
        return Err("No paper <node> elements found in GraphML file".to_string());
    }

    let mut edges = Vec::new();
    for eseg in xml.split("<edge ").skip(1) {
        let end_hdr = eseg.find('>').unwrap_or(eseg.len());
        let hdr = &eseg[..end_hdr];
        let Some(src) = xml_attr(hdr, "source") else { continue };
        let Some(tgt) = xml_attr(hdr, "target") else { continue };
        if src != tgt && paper_ids.contains(&src) && paper_ids.contains(&tgt) {
            edges.push(CitationEdge {
                source: src,
                target: tgt,
                provenance: "graphml_import".to_string(),
            });
        }
    }

    Ok((papers, edges))
}

/// Round-trip GEXF 1.2/1.3 importer reconstructing `PaperRecord` and `CitationEdge` collections.
pub fn parse_gexf_corpus(xml: &str) -> Result<(Vec<PaperRecord>, Vec<CitationEdge>), String> {
    let mut node_attr_names: HashMap<String, String> = HashMap::new();
    if let Some(attr_start) = xml.find("<attributes class=\"node\"") {
        let rest = &xml[attr_start..];
        let attr_end = rest.find("</attributes>").unwrap_or(rest.len());
        for aseg in rest[..attr_end].split("<attribute ").skip(1) {
            let gt = aseg.find('>').unwrap_or(aseg.len());
            let hdr = &aseg[..gt];
            if let (Some(aid), Some(title)) = (xml_attr(hdr, "id"), xml_attr(hdr, "title")) {
                node_attr_names.insert(aid, title);
            }
        }
    }

    let mut papers = Vec::new();
    let mut paper_ids = std::collections::HashSet::new();

    for nseg in xml.split("<node ").skip(1) {
        let Some(close_n) = nseg.find("</node>") else { continue };
        let block = &nseg[..close_n];
        let gt = block.find('>').unwrap_or(block.len());
        let hdr = &block[..gt];
        let Some(node_id) = xml_attr(hdr, "id") else { continue };
        if node_id.starts_with("datacite:") || node_id.starts_with("bio:") {
            continue;
        }
        let label = xml_attr(hdr, "label").unwrap_or_else(|| node_id.clone());

        let mut fields: HashMap<String, String> = HashMap::new();
        for av in block[gt..].split("<attvalue ").skip(1) {
            let av_end = av.find('>').unwrap_or(av.len());
            let av_hdr = &av[..av_end];
            if let (Some(fid), Some(val)) = (xml_attr(av_hdr, "for"), xml_attr(av_hdr, "value")) {
                let name = node_attr_names.get(&fid).cloned().unwrap_or(fid);
                fields.insert(name, val);
            }
        }

        if let Some(nt) = fields.get("node_type").map(|s| s.as_str()) {
            if nt != "paper" {
                continue;
            }
        }

        let title = fields.get("title").cloned().unwrap_or(label);
        let authors = fields
            .get("authors")
            .cloned()
            .unwrap_or_else(|| "GEXF Author".to_string());
        let year = fields
            .get("year")
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(2024);
        let venue = fields
            .get("venue")
            .cloned()
            .unwrap_or_else(|| "GEXF Venue".to_string());
        let doi = normalize_doi(fields.get("doi").map(|s| s.as_str()).unwrap_or(""));
        let disc = fields
            .get("discipline")
            .map(|d| Discipline::from_code(d))
            .unwrap_or_else(|| Discipline::infer_from_text(&title));
        let concepts = infer_concepts_from_text(&format!("{} {}", title, venue), disc);
        let ret_status = fields
            .get("retraction_status")
            .map(|s| RetractionStatus::from_str_lossy(s))
            .unwrap_or(RetractionStatus::Unscreened);

        paper_ids.insert(node_id.clone());
        papers.push(PaperRecord {
            id: node_id,
            title,
            authors,
            year,
            venue,
            disc,
            doi,
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            source_url: String::new(),
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Unknown,
            external_citations: None,
            abstract_text: String::new(),
            terms: concepts.clone(),
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: RetractionRecord {
                status: ret_status,
                ..Default::default()
            },
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }

    if papers.is_empty() {
        return Err("No paper <node> elements found in GEXF file".to_string());
    }

    let mut edges = Vec::new();
    for eseg in xml.split("<edge ").skip(1) {
        let gt = eseg.find('>').unwrap_or(eseg.len());
        let hdr = &eseg[..gt];
        let Some(src) = xml_attr(hdr, "source") else { continue };
        let Some(tgt) = xml_attr(hdr, "target") else { continue };
        if src != tgt && paper_ids.contains(&src) && paper_ids.contains(&tgt) {
            edges.push(CitationEdge {
                source: src,
                target: tgt,
                provenance: "gexf_import".to_string(),
            });
        }
    }

    Ok((papers, edges))
}

fn normalize_bibtex_authors(raw: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for tok in raw.split_whitespace() {
        if tok.eq_ignore_ascii_case("and") {
            if !current.is_empty() {
                parts.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(tok);
        }
    }
    if !current.is_empty() {
        parts.push(current.join(" "));
    }

    let mut normalized: Vec<String> = Vec::new();
    for part in parts {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p.eq_ignore_ascii_case("others") {
            normalized.push("et al.".to_string());
        } else if p.matches(',').count() == 1 {
            let is_two_initial_last_pairs = if let Some((left, right)) = p.split_once(',') {
                left.split_whitespace().next().is_some_and(|t| {
                    t.ends_with('.')
                        && t.chars().count() <= 3
                        && !t.eq_ignore_ascii_case("st.")
                        && !t.eq_ignore_ascii_case("ste.")
                }) && right.split_whitespace().count() >= 2
            } else {
                false
            };
            if is_two_initial_last_pairs {
                normalized.push(p.to_string());
            } else {
                let n = normalize_person_name(p);
                if !n.is_empty() {
                    normalized.push(n);
                }
            }
        } else if p.matches(',').count() == 0 {
            let n = normalize_person_name(p);
            if !n.is_empty() {
                normalized.push(n);
            }
        } else {
            normalized.push(p.to_string());
        }
    }

    if normalized.is_empty() {
        "BibTeX Author".to_string()
    } else {
        normalized.join(", ")
    }
}

fn normalize_person_name(raw: &str) -> String {
    let clean = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return String::new();
    }
    if let Some((family, given)) = clean.split_once(',') {
        let fam = family.trim();
        let giv = given.trim();
        if !fam.is_empty() && !giv.is_empty() {
            return format!("{} {}", giv, fam);
        }
        if !fam.is_empty() {
            return fam.to_string();
        }
    }
    clean
}

fn strip_simple_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses RIS (`TY  - ...` / `ER  -`) bibliographic reference files into `PaperRecord` entries.
pub fn parse_ris_text(text: &str) -> Vec<PaperRecord> {
    let cleaned = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut records: Vec<HashMap<String, Vec<String>>> = Vec::new();
    let mut current: HashMap<String, Vec<String>> = HashMap::new();
    let mut last_tag: Option<String> = None;

    for raw_line in cleaned.lines() {
        let line = raw_line.trim_end_matches('\r');
        let bytes = line.as_bytes();
        let is_tag = bytes.len() >= 6
            && bytes[0].is_ascii_alphanumeric()
            && bytes[1].is_ascii_alphanumeric()
            && &bytes[2..6] == b"  - ";
        let is_tag_short = !is_tag
            && bytes.len() >= 5
            && bytes[0].is_ascii_alphanumeric()
            && bytes[1].is_ascii_alphanumeric()
            && &bytes[2..5] == b"  -";

        if is_tag || is_tag_short {
            let tag = line[..2].to_ascii_uppercase();
            let val = if is_tag { &line[6..] } else { &line[5..] }.trim();
            if tag == "ER" {
                if !current.is_empty() {
                    records.push(std::mem::take(&mut current));
                }
                last_tag = None;
                continue;
            }
            if tag == "TY" && !current.is_empty() {
                records.push(std::mem::take(&mut current));
            }
            if !val.is_empty() {
                current.entry(tag.clone()).or_default().push(val.to_string());
            }
            last_tag = Some(tag);
        } else if !line.trim().is_empty() {
            if let Some(ref tag) = last_tag {
                if let Some(vec) = current.get_mut(tag) {
                    if let Some(last) = vec.last_mut() {
                        if !last.is_empty() {
                            last.push(' ');
                        }
                        last.push_str(line.trim());
                    }
                }
            }
        }
    }
    if !current.is_empty() {
        records.push(current);
    }

    let first_tag = |map: &HashMap<String, Vec<String>>, tags: &[&str]| -> String {
        for t in tags {
            if let Some(vals) = map.get(*t) {
                if let Some(v) = vals.iter().find(|s| !s.trim().is_empty()) {
                    return v.trim().to_string();
                }
            }
        }
        String::new()
    };

    let mut out = Vec::new();
    for (idx, map) in records.iter().enumerate() {
        let title = first_tag(map, &["TI", "T1", "CT"]);
        if title.is_empty() {
            continue;
        }
        let mut author_list: Vec<String> = Vec::new();
        for tag in ["AU", "A1", "A2"] {
            if let Some(vals) = map.get(tag) {
                for v in vals {
                    let norm = normalize_person_name(v);
                    if !norm.is_empty() && !author_list.contains(&norm) {
                        author_list.push(norm);
                    }
                }
                if !author_list.is_empty() {
                    break;
                }
            }
        }
        let authors = if author_list.is_empty() {
            "RIS Author".to_string()
        } else {
            author_list.join(", ")
        };

        let year_str = first_tag(map, &["PY", "Y1", "DA"]);
        let year = extract_year_from_str(&year_str).unwrap_or(2020);
        let venue = {
            let v = first_tag(map, &["JF", "JO", "T2", "JA", "BT"]);
            if v.is_empty() {
                "RIS Imported Venue".to_string()
            } else {
                v
            }
        };
        let doi = normalize_doi(&first_tag(map, &["DO", "DI"]));
        let abstract_text = first_tag(map, &["AB", "N2"]);
        let url = first_tag(map, &["UR"]);
        let pdf_url = first_tag(map, &["L1"]);
        let mut kw_terms: Vec<String> = map
            .get("KW")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|s| {
                s.split([';', ','])
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
            })
            .collect();

        let combined = format!("{} {} {} {}", title, venue, abstract_text, kw_terms.join(" "));
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);
        if kw_terms.is_empty() {
            kw_terms = concepts.clone();
        }
        let raw_id = first_tag(map, &["ID", "AN"]);
        let id = if !raw_id.is_empty() {
            raw_id
        } else if !doi.is_empty() {
            format!("ris_{}", doi.replace(['/', ':'], "_"))
        } else {
            format!("ris_{}_{}", year, idx + 1)
        };

        out.push(PaperRecord {
            id,
            title,
            authors,
            year,
            venue,
            disc,
            source_url: if !url.is_empty() {
                url
            } else if !doi.is_empty() {
                format!("https://doi.org/{}", doi)
            } else {
                String::new()
            },
            doi,
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            oa_pdf_url: pdf_url,
            oa_status: OaStatus::Unknown,
            external_citations: None,
            abstract_text,
            terms: kw_terms,
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: RetractionRecord::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }
    out
}

/// Parses PubMed NBIB / MEDLINE (`PMID- ...`, `TI  - ...`, `FAU - ...`) export files into `PaperRecord` entries.
pub fn parse_nbib_text(text: &str) -> Vec<PaperRecord> {
    let cleaned = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut blocks: Vec<HashMap<String, Vec<String>>> = Vec::new();
    let mut current: HashMap<String, Vec<String>> = HashMap::new();
    let mut last_tag: Option<String> = None;

    for raw_line in cleaned.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            last_tag = None;
            continue;
        }
        if line.starts_with("      ") || line.starts_with('\t') {
            if let Some(ref tag) = last_tag {
                if let Some(vec) = current.get_mut(tag) {
                    if let Some(last) = vec.last_mut() {
                        if !last.is_empty() {
                            last.push(' ');
                        }
                        last.push_str(line.trim());
                    }
                }
            }
            continue;
        }
        if let Some(dash_pos) = line.find('-') {
            let tag_raw = &line[..dash_pos];
            let tag = tag_raw.trim();
            if (2..=4).contains(&tag.len())
                && tag.chars().all(|c| c.is_ascii_alphanumeric())
                && (dash_pos == 4 || line[dash_pos..].starts_with("- "))
            {
                let tag_up = tag.to_ascii_uppercase();
                let val = line[dash_pos + 1..].trim();
                if tag_up == "PMID" && !current.is_empty() {
                    blocks.push(std::mem::take(&mut current));
                }
                if !val.is_empty() {
                    current
                        .entry(tag_up.clone())
                        .or_default()
                        .push(val.to_string());
                }
                last_tag = Some(tag_up);
            }
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }

    let first_tag = |map: &HashMap<String, Vec<String>>, tags: &[&str]| -> String {
        for t in tags {
            if let Some(vals) = map.get(*t) {
                if let Some(v) = vals.iter().find(|s| !s.trim().is_empty()) {
                    return v.trim().to_string();
                }
            }
        }
        String::new()
    };

    let mut out = Vec::new();
    for (idx, map) in blocks.iter().enumerate() {
        let title = first_tag(map, &["TI", "BTI"])
            .trim_end_matches('.')
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let pmid = first_tag(map, &["PMID"]);
        let mut authors_vec: Vec<String> = Vec::new();
        if let Some(fau_list) = map.get("FAU") {
            for a in fau_list {
                let norm = normalize_person_name(a);
                if !norm.is_empty() {
                    authors_vec.push(norm);
                }
            }
        } else if let Some(au_list) = map.get("AU") {
            for a in au_list {
                let t = a.trim();
                if !t.is_empty() {
                    authors_vec.push(t.to_string());
                }
            }
        }
        let authors = if authors_vec.is_empty() {
            "PubMed Author".to_string()
        } else {
            authors_vec.join(", ")
        };

        let dp = first_tag(map, &["DP"]);
        let year = extract_year_from_str(&dp).unwrap_or(2020);
        let venue = {
            let v = first_tag(map, &["JT", "TA"]);
            if v.is_empty() {
                "PubMed / MEDLINE".to_string()
            } else {
                v
            }
        };

        let mut doi = String::new();
        let mut pmcid = first_tag(map, &["PMC"]);
        for tag in ["LID", "AID"] {
            if let Some(vals) = map.get(tag) {
                for v in vals {
                    let low = v.to_ascii_lowercase();
                    if doi.is_empty() && low.contains("[doi]") {
                        let raw_d = v.split("[doi]").next().unwrap_or("").trim();
                        doi = normalize_doi(raw_d);
                    } else if pmcid.is_empty() && low.contains("[pmc]") {
                        let raw_p = v.split("[pmc]").next().unwrap_or("").trim();
                        if !raw_p.is_empty() {
                            pmcid = raw_p.to_ascii_uppercase();
                        }
                    }
                }
            }
        }
        let oa_pdf_url = if !pmcid.is_empty() {
            format!("https://pmc.ncbi.nlm.nih.gov/articles/{}/pdf/", pmcid)
        } else {
            String::new()
        };
        let oa_status = if !pmcid.is_empty() {
            OaStatus::Green
        } else {
            OaStatus::Unknown
        };
        let abstract_text = first_tag(map, &["AB"]);
        let mut terms: Vec<String> = Vec::new();
        if !pmid.is_empty() {
            terms.push(format!("pmid:{}", pmid));
        }
        for tag in ["OT", "MH"] {
            if let Some(vals) = map.get(tag) {
                for v in vals {
                    let clean_kw = v.trim_start_matches('*').trim();
                    if !clean_kw.is_empty() && !terms.contains(&clean_kw.to_string()) {
                        terms.push(clean_kw.to_string());
                    }
                }
            }
        }

        let combined = format!("{} {} {} {}", title, venue, abstract_text, terms.join(" "));
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);
        if terms.is_empty() {
            terms = concepts.clone();
        }
        let id = if !pmid.is_empty() {
            format!("pmid_{}", pmid)
        } else {
            format!("nbib_{}_{}", year, idx + 1)
        };
        let source_url = if !pmid.is_empty() {
            format!("https://pubmed.ncbi.nlm.nih.gov/{}/", pmid)
        } else if !doi.is_empty() {
            format!("https://doi.org/{}", doi)
        } else {
            String::new()
        };

        out.push(PaperRecord {
            id,
            title,
            authors,
            year,
            venue,
            disc,
            doi,
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid,
            source_url,
            oa_pdf_url,
            oa_status,
            external_citations: None,
            abstract_text,
            terms,
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: RetractionRecord::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }
    out
}

/// Parses CSL-JSON (`[...]` or `{"items": [...]}`) reference exports into `PaperRecord` entries.
pub fn parse_csl_json_text(text: &str) -> Result<Vec<PaperRecord>, String> {
    let cleaned = text.strip_prefix('\u{feff}').unwrap_or(text);
    let root: serde_json::Value =
        serde_json::from_str(cleaned).map_err(|e| format!("Invalid CSL-JSON syntax: {}", e))?;
    let items: &[serde_json::Value] = if let Some(arr) = root.as_array() {
        arr
    } else if let Some(arr) = root.get("items").and_then(|v| v.as_array()) {
        arr
    } else {
        return Err("CSL-JSON must be a JSON array or an object containing an 'items' array".to_string());
    };

    let mut out = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let Some(obj) = item.as_object() else { continue };
        let title_raw = obj
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let title = strip_simple_html_tags(title_raw);
        if title.is_empty() {
            continue;
        }

        let mut authors_vec: Vec<String> = Vec::new();
        if let Some(arr) = obj.get("author").and_then(|v| v.as_array()) {
            for a in arr {
                if let Some(lit) = a.get("literal").and_then(|v| v.as_str()) {
                    let norm = normalize_person_name(lit);
                    if !norm.is_empty() {
                        authors_vec.push(norm);
                    }
                } else {
                    let given = a.get("given").and_then(|v| v.as_str()).unwrap_or("").trim();
                    let family = a.get("family").and_then(|v| v.as_str()).unwrap_or("").trim();
                    let full = format!("{} {}", given, family).trim().to_string();
                    if !full.is_empty() {
                        authors_vec.push(full);
                    }
                }
            }
        }
        let authors = if authors_vec.is_empty() {
            "CSL-JSON Author".to_string()
        } else {
            authors_vec.join(", ")
        };

        let year = obj
            .get("issued")
            .and_then(|iss| {
                iss.get("date-parts")
                    .and_then(|dp| dp.as_array())
                    .and_then(|outer| outer.first())
                    .and_then(|inner| inner.as_array())
                    .and_then(|parts| parts.first())
                    .and_then(|y| {
                        y.as_i64()
                            .map(|n| n as i32)
                            .or_else(|| y.as_str().and_then(extract_year_from_str))
                    })
                    .or_else(|| {
                        iss.get("raw")
                            .and_then(|r| r.as_str())
                            .and_then(extract_year_from_str)
                    })
            })
            .or_else(|| {
                obj.get("year")
                    .and_then(|y| y.as_i64().map(|n| n as i32))
            })
            .unwrap_or(2020);

        let venue = obj
            .get("container-title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("CSL-JSON Imported Venue")
            .trim()
            .to_string();

        let doi_raw = obj
            .get("DOI")
            .or_else(|| obj.get("doi"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let doi = normalize_doi(doi_raw);
        let url = obj
            .get("URL")
            .or_else(|| obj.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let abstract_text = strip_simple_html_tags(
            obj.get("abstract")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );

        let mut terms: Vec<String> = Vec::new();
        if let Some(kw) = obj.get("keyword").and_then(|v| v.as_str()) {
            for part in kw.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()) {
                terms.push(part.to_string());
            }
        }
        if let Some(cats) = obj.get("categories").and_then(|v| v.as_array()) {
            for c in cats {
                if let Some(s) = c.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    if !terms.contains(&s.to_string()) {
                        terms.push(s.to_string());
                    }
                }
            }
        }

        let combined = format!("{} {} {} {}", title, venue, abstract_text, terms.join(" "));
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);
        if terms.is_empty() {
            terms = concepts.clone();
        }

        let raw_id = obj
            .get("id")
            .or_else(|| obj.get("citation-key"))
            .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())))
            .unwrap_or_default();
        let id = if !raw_id.trim().is_empty() {
            raw_id.trim().to_string()
        } else if !doi.is_empty() {
            format!("csl_{}", doi.replace(['/', ':'], "_"))
        } else {
            format!("csl_{}_{}", year, idx + 1)
        };

        out.push(PaperRecord {
            id,
            title,
            authors,
            year,
            venue,
            disc,
            source_url: if !url.is_empty() {
                url
            } else if !doi.is_empty() {
                format!("https://doi.org/{}", doi)
            } else {
                String::new()
            },
            doi,
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Unknown,
            external_citations: None,
            abstract_text,
            terms,
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: RetractionRecord::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod import_tests {
    use super::*;

    #[test]
    fn ris_import_parses_multirecord_authors_doi_abstract_and_file() {
        let ris = "\u{feff}TY  - JOUR\r\nID  - shannon1948\r\nTI  - A Mathematical Theory of Communication\r\nAU  - Shannon, Claude E.\r\nAU  - Weaver, Warren\r\nPY  - 1948/07//\r\nJF  - Bell System Technical Journal\r\nDO  - 10.1002/j.1538-7305.1948.tb01338.x\r\nAB  - The fundamental problem of communication is that of\r\n  reproducing at one point either exactly or approximately a message.\r\nKW  - entropy; channel capacity\r\nER  -\r\n\r\nTY  - JOUR\r\nTI  - Deep Residual Learning\r\nAU  - He, Kaiming\r\nPY  - 2016\r\nJO  - CVPR\r\nDO  - https://doi.org/10.1109/CVPR.2016.90\r\nER  -\r\n";
        let parsed = parse_ris_text(ris);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "shannon1948");
        assert_eq!(parsed[0].year, 1948);
        assert_eq!(parsed[0].authors, "Claude E. Shannon, Warren Weaver");
        assert_eq!(parsed[0].doi, "10.1002/j.1538-7305.1948.tb01338.x");
        assert!(parsed[0].abstract_text.contains("reproducing at one point"));
        assert_eq!(parsed[1].year, 2016);
        assert_eq!(parsed[1].authors, "Kaiming He");

        let tmp = std::env::temp_dir().join(format!("cit_atlas_test_{}.ris", std::process::id()));
        std::fs::write(&tmp, ris).unwrap();
        let (papers, edges) = import_corpus_file(&tmp).unwrap();
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(papers.len(), 2);
        assert!(edges.is_empty());
    }

    #[test]
    fn nbib_import_parses_pubmed_multiline_fau_lid_doi_pmc_and_file() {
        let nbib = "\u{feff}PMID- 34265844\r\nOWN - NLM\r\nTI  - Highly accurate protein structure prediction with\r\n      AlphaFold.\r\nFAU - Jumper, John\r\nAU  - Jumper J\r\nFAU - Hassabis, Demis\r\nAU  - Hassabis D\r\nDP  - 2021 Aug\r\nJT  - Nature\r\nTA  - Nature\r\nLID - 10.1038/s41586-021-03819-2 [doi]\r\nAID - PMC8371605 [pmc]\r\nAB  - Proteins are essential to life, and understanding their structure\r\n      can facilitate a mechanistic understanding of their function.\r\nOT  - AlphaFold\r\nMH  - *Protein Folding\r\n\r\nPMID- 12345678\r\nTI  - Second MEDLINE Paper\r\nAU  - Smith A\r\nDP  - 2019\r\nTA  - Bioinformatics\r\n";
        let parsed = parse_nbib_text(nbib);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "pmid_34265844");
        assert_eq!(parsed[0].year, 2021);
        assert_eq!(parsed[0].authors, "John Jumper, Demis Hassabis");
        assert_eq!(parsed[0].doi, "10.1038/s41586-021-03819-2");
        assert_eq!(parsed[0].pmcid, "PMC8371605");
        assert_eq!(parsed[0].oa_status, OaStatus::Green);
        assert!(parsed[0].terms.contains(&"pmid:34265844".to_string()));
        assert!(parsed[0].abstract_text.contains("mechanistic understanding"));
        assert_eq!(parsed[1].id, "pmid_12345678");
        assert_eq!(parsed[1].authors, "Smith A");

        let tmp = std::env::temp_dir().join(format!("cit_atlas_test_{}.nbib", std::process::id()));
        std::fs::write(&tmp, nbib).unwrap();
        let (papers, _) = import_corpus_file(&tmp).unwrap();
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(papers.len(), 2);
    }

    #[test]
    fn csl_json_import_handles_array_and_items_wrapper_and_json_ext() {
        let csl_arr = r#"[
          {
            "id": "wu2019",
            "title": "Large teams develop and <i>small</i> teams disrupt science and technology",
            "author": [{"given": "Lingfei", "family": "Wu"}, {"given": "James A.", "family": "Evans"}],
            "issued": {"date-parts": [[2019, 2]]},
            "container-title": "Nature",
            "DOI": "10.1038/s41586-019-0941-9",
            "keyword": "disruption, scientometrics"
          }
        ]"#;
        let parsed_arr = parse_csl_json_text(csl_arr).unwrap();
        assert_eq!(parsed_arr.len(), 1);
        assert_eq!(parsed_arr[0].id, "wu2019");
        assert_eq!(parsed_arr[0].year, 2019);
        assert_eq!(parsed_arr[0].title, "Large teams develop and small teams disrupt science and technology");
        assert_eq!(parsed_arr[0].authors, "Lingfei Wu, James A. Evans");

        let csl_wrapped = format!(r#"{{"items": {}}}"#, csl_arr);
        let tmp_json = std::env::temp_dir().join(format!("cit_atlas_csl_{}.json", std::process::id()));
        std::fs::write(&tmp_json, &csl_wrapped).unwrap();
        let (p_json, _) = import_corpus_file(&tmp_json).unwrap();
        let _ = std::fs::remove_file(&tmp_json);
        assert_eq!(p_json.len(), 1);
        assert_eq!(p_json[0].doi, "10.1038/s41586-019-0941-9");

        let tmp_csl = std::env::temp_dir().join(format!("cit_atlas_csl_{}.csl", std::process::id()));
        std::fs::write(&tmp_csl, csl_arr).unwrap();
        let (p_csl, _) = import_corpus_file(&tmp_csl).unwrap();
        let _ = std::fs::remove_file(&tmp_csl);
        assert_eq!(p_csl.len(), 1);
    }

    #[test]
    fn bibtex_import_normalizes_family_given_authors() {
        let bib = r#"
@article{shannon1948,
  title = {A Mathematical Theory of Communication},
  author = {Shannon, Claude E. and Weaver, Warren},
  year = {1948},
  journal = {Bell System Technical Journal},
  doi = {10.1002/j.1538-7305.1948.tb01338.x}
}

@article{smith2020,
  title = {Single Author Study},
  author = {Smith, Alice},
  year = {2020},
  journal = {Journal of Testing}
}

@article{stpierre2021,
  title = {Particle Surname Study},
  author = {St. Pierre, Georges E. and de la Cruz, Juan},
  year = {2021},
  journal = {Journal of Particles}
}
"#;
        let parsed = parse_bibtex_text(bib);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].authors, "Claude E. Shannon, Warren Weaver");
        assert_eq!(parsed[1].authors, "Alice Smith");
        assert_eq!(parsed[2].authors, "Georges E. St. Pierre, Juan de la Cruz");
        assert_eq!(crate::analytics::count_paper_authors(&parsed[0].authors), 2);
        assert_eq!(crate::analytics::count_paper_authors(&parsed[1].authors), 1);
        assert_eq!(crate::analytics::count_paper_authors(&parsed[2].authors), 2);
    }
}
