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

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::export::format_bibtex_corpus;
use crate::models::{DownloadItemReport, DownloadStatus, PaperRecord};
use crate::rate_limiter::{
    acquire_domain_permit_blocking, log_global_crawl_outcome, AcquirePermit, CrawlAuditOutcome,
};
use crate::search_providers::{normalize_doi, percent_encode, ScholarlyHttpClient};

const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024; // 64 MB safety cap
const MIN_VALID_PDF_BYTES: usize = 1024;

pub fn is_valid_pdf_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_VALID_PDF_BYTES && bytes.starts_with(b"%PDF-")
}

pub fn compute_sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

pub fn build_safe_pdf_filename(paper: &PaperRecord) -> String {
    let first_author: String = paper
        .authors
        .split(',')
        .next()
        .unwrap_or("author")
        .split_whitespace()
        .last()
        .unwrap_or("author")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(18)
        .collect::<String>()
        .to_ascii_lowercase();

    let title_slug: String = paper
        .title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("_");

    format!(
        "{}_{}_{}.pdf",
        paper.year,
        if first_author.is_empty() { "paper" } else { &first_author },
        if title_slug.is_empty() { &paper.id } else { &title_slug }
    )
}

/// Resolves candidate legal open-access PDF URLs in priority order:
/// 1. Direct provider OA PDF URL (OpenAlex / Semantic Scholar / Crossref)
/// 2. Unpaywall v2 DOI lookup (`best_oa_location.url_for_pdf`, then `oa_locations[].url_for_pdf`)
/// 3. Direct arXiv PDF (`https://arxiv.org/pdf/{arxiv_id}.pdf`)
/// 4. Europe PMC OA full-text PDF when PMCID is present
pub fn resolve_candidate_oa_urls(
    http: &ScholarlyHttpClient,
    paper: &PaperRecord,
) -> Vec<(String, String)> {
    let mut candidates: Vec<(String, String)> = Vec::new();

    if !paper.oa_pdf_url.trim().is_empty() && paper.oa_pdf_url.starts_with("http") {
        candidates.push(("provider_oa_url".to_string(), paper.oa_pdf_url.trim().to_string()));
    }

    let doi = normalize_doi(&paper.doi);
    if !doi.is_empty() {
        if let Ok(unpaywall_urls) = query_unpaywall_pdf_urls(http, &doi) {
            for (idx, u) in unpaywall_urls.into_iter().enumerate() {
                if !candidates.iter().any(|(_, existing)| existing == &u) {
                    let stage = if idx == 0 {
                        "unpaywall_best_oa".to_string()
                    } else {
                        format!("unpaywall_oa_loc_{}", idx)
                    };
                    candidates.push((stage, u));
                }
            }
        }
    }

    if !paper.arxiv_id.trim().is_empty() {
        let arx_url = format!("https://arxiv.org/pdf/{}.pdf", paper.arxiv_id.trim());
        if !candidates.iter().any(|(_, existing)| existing == &arx_url) {
            candidates.push(("arxiv_direct_pdf".to_string(), arx_url));
        }
    }

    if !paper.pmcid.trim().is_empty() {
        let pmc_clean = paper.pmcid.trim().to_ascii_uppercase();
        let epmc_url = format!(
            "https://europepmc.org/backend/ptpmcrender.fcgi?accid={}&blobtype=pdf",
            percent_encode(&pmc_clean)
        );
        if !candidates.iter().any(|(_, existing)| existing == &epmc_url) {
            candidates.push(("europe_pmc_pdf".to_string(), epmc_url));
        }
    }

    for t in &paper.terms {
        if let Some(bib) = t.strip_prefix("bibcode:") {
            let clean_bib = bib.trim();
            if !clean_bib.is_empty() {
                let ads_eprint = format!(
                    "https://ui.adsabs.harvard.edu/link_gateway/{}/EPRINT_PDF",
                    percent_encode(clean_bib)
                );
                if !candidates.iter().any(|(_, existing)| existing == &ads_eprint) {
                    candidates.push(("nasa_ads_eprint_pdf".to_string(), ads_eprint));
                }
            }
        }
    }

    candidates
}

fn query_unpaywall_pdf_urls(http: &ScholarlyHttpClient, doi: &str) -> Result<Vec<String>, String> {
    let url = format!(
        "https://api.unpaywall.org/v2/{}?email={}",
        percent_encode(doi),
        percent_encode(http.polite_email())
    );
    let json = http.get_json_with_retry(&url, false)?;
    let mut urls = Vec::new();

    if let Some(u) = json
        .pointer("/best_oa_location/url_for_pdf")
        .and_then(|v| v.as_str())
    {
        if u.starts_with("http") {
            urls.push(u.to_string());
        }
    }
    if let Some(locs) = json.get("oa_locations").and_then(|v| v.as_array()) {
        for loc in locs {
            if let Some(u) = loc.get("url_for_pdf").and_then(|v| v.as_str()) {
                if u.starts_with("http") && !urls.iter().any(|x| x == u) {
                    urls.push(u.to_string());
                }
            }
        }
    }
    Ok(urls)
}

pub fn download_single_paper_pdf(
    http: &ScholarlyHttpClient,
    paper: &mut PaperRecord,
    pdf_dir: &Path,
) -> DownloadItemReport {
    if let Err(e) = fs::create_dir_all(pdf_dir) {
        return DownloadItemReport {
            paper_id: paper.id.clone(),
            title: paper.title.clone(),
            doi: paper.doi.clone(),
            status: DownloadStatus::FailedNetwork(format!(
                "Could not create PDF directory {:?}: {}",
                pdf_dir, e
            )),
            resolved_stage: "local_disk_cache".to_string(),
            final_pdf_url: String::new(),
            saved_path: None,
            byte_len: 0,
            sha256: None,
        };
    }
    let target_name = build_safe_pdf_filename(paper);
    let target_path: PathBuf = pdf_dir.join(&target_name);

    // Check existing cached PDF first
    if target_path.exists() {
        if let Ok(bytes) = fs::read(&target_path) {
            if is_valid_pdf_bytes(&bytes) {
                let sha = compute_sha256_hex(&bytes);
                let path_str = target_path.to_string_lossy().to_string();
                paper.local_pdf_path = Some(path_str.clone());
                paper.pdf_sha256 = Some(sha.clone());
                paper.pdf_bytes = Some(bytes.len() as u64);
                return DownloadItemReport {
                    paper_id: paper.id.clone(),
                    title: paper.title.clone(),
                    doi: paper.doi.clone(),
                    status: DownloadStatus::CachedAlreadyValid,
                    resolved_stage: "local_disk_cache".to_string(),
                    final_pdf_url: paper.oa_pdf_url.clone(),
                    saved_path: Some(path_str),
                    byte_len: bytes.len() as u64,
                    sha256: Some(sha),
                };
            }
        }
    }

    let candidates = resolve_candidate_oa_urls(http, paper);
    if candidates.is_empty() {
        return DownloadItemReport {
            paper_id: paper.id.clone(),
            title: paper.title.clone(),
            doi: paper.doi.clone(),
            status: DownloadStatus::NoOpenAccessPdfFound,
            resolved_stage: "none".to_string(),
            final_pdf_url: String::new(),
            saved_path: None,
            byte_len: 0,
            sha256: None,
        };
    }

    let mut saw_non_pdf_landing = false;
    let mut last_err = String::new();

    for (stage, candidate_url) in candidates {
        match fetch_binary_capped(http, &candidate_url) {
            Ok(bytes) => {
                if !is_valid_pdf_bytes(&bytes) {
                    saw_non_pdf_landing = true;
                    continue;
                }
                let sha = compute_sha256_hex(&bytes);
                let tmp_path = pdf_dir.join(format!("{}.part", target_name));
                if let Err(e) = write_atomic_pdf(&tmp_path, &target_path, &bytes) {
                    last_err = e;
                    continue;
                }
                let path_str = target_path.to_string_lossy().to_string();
                paper.local_pdf_path = Some(path_str.clone());
                paper.pdf_sha256 = Some(sha.clone());
                paper.pdf_bytes = Some(bytes.len() as u64);
                if paper.oa_pdf_url.is_empty() {
                    paper.oa_pdf_url = candidate_url.clone();
                }

                return DownloadItemReport {
                    paper_id: paper.id.clone(),
                    title: paper.title.clone(),
                    doi: paper.doi.clone(),
                    status: DownloadStatus::Downloaded,
                    resolved_stage: stage,
                    final_pdf_url: candidate_url,
                    saved_path: Some(path_str),
                    byte_len: bytes.len() as u64,
                    sha256: Some(sha),
                };
            }
            Err(e) => {
                last_err = e;
            }
        }
    }

    let status = if saw_non_pdf_landing {
        DownloadStatus::RejectedNonPdfPayload
    } else if !last_err.is_empty() {
        DownloadStatus::FailedNetwork(last_err)
    } else {
        DownloadStatus::NoOpenAccessPdfFound
    };

    DownloadItemReport {
        paper_id: paper.id.clone(),
        title: paper.title.clone(),
        doi: paper.doi.clone(),
        status,
        resolved_stage: "exhausted_chain".to_string(),
        final_pdf_url: String::new(),
        saved_path: None,
        byte_len: 0,
        sha256: None,
    }
}

fn fetch_binary_capped(http: &ScholarlyHttpClient, url: &str) -> Result<Vec<u8>, String> {
    let wait_ms = match acquire_domain_permit_blocking(url) {
        AcquirePermit::Granted { wait_ms, .. } => wait_ms,
        AcquirePermit::CircuitOpen { reason, .. } => {
            log_global_crawl_outcome(
                "oa_pdf_downloader",
                "GET",
                url,
                CrawlAuditOutcome::CircuitBreakerSkipped,
                0,
                0,
                0,
                0,
                None,
                "n/a_pdf",
                &reason,
            );
            return Err(reason);
        }
    };
    let t0 = Instant::now();
    let resp = match http
        .client()
        .get(url)
        .header("Accept", "application/pdf,*/*;q=0.8")
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            log_global_crawl_outcome(
                "oa_pdf_downloader",
                "GET",
                url,
                CrawlAuditOutcome::NetworkError,
                0,
                wait_ms,
                t0.elapsed().as_millis() as u64,
                0,
                None,
                "n/a_pdf",
                &e.to_string(),
            );
            return Err(format!("GET failed: {}", e));
        }
    };

    let code = resp.status().as_u16();
    let hdrs = resp.headers().clone();
    if !resp.status().is_success() {
        log_global_crawl_outcome(
            "oa_pdf_downloader",
            "GET",
            url,
            if code == 429 {
                CrawlAuditOutcome::Http429RateLimited
            } else if code == 403 {
                CrawlAuditOutcome::Http403OrCaptcha
            } else {
                CrawlAuditOutcome::HttpOtherError
            },
            code,
            wait_ms,
            t0.elapsed().as_millis() as u64,
            0,
            Some(&hdrs),
            "n/a_pdf",
            "PDF candidate rejected by status",
        );
        return Err(format!("HTTP {}", code));
    }

    let mut buf = Vec::new();
    resp.take(MAX_PDF_BYTES)
        .read_to_end(&mut buf)
        .map_err(|e| format!("Stream read error: {}", e))?;
    let valid_magic = is_valid_pdf_bytes(&buf);
    log_global_crawl_outcome(
        "oa_pdf_downloader",
        "GET",
        url,
        if wait_ms > 0 {
            CrawlAuditOutcome::ThrottledThen2xx
        } else {
            CrawlAuditOutcome::Allowed2xx
        },
        code,
        wait_ms,
        t0.elapsed().as_millis() as u64,
        buf.len() as u64,
        Some(&hdrs),
        "n/a_pdf",
        if valid_magic {
            "%PDF- binary verified"
        } else {
            "HTML landing page rejected (not %PDF-)"
        },
    );
    Ok(buf)
}

fn write_atomic_pdf(tmp_path: &Path, final_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = fs::File::create(tmp_path).map_err(|e| format!("Create temp file failed: {}", e))?;
    f.write_all(bytes)
        .map_err(|e| format!("Write PDF bytes failed: {}", e))?;
    f.sync_all()
        .map_err(|e| format!("Sync PDF file failed: {}", e))?;
    drop(f);
    fs::rename(tmp_path, final_path).map_err(|e| format!("Atomic rename failed: {}", e))
}

pub fn batch_download_corpus_pdfs(
    papers: &mut [PaperRecord],
    out_dir: &Path,
    polite_email: &str,
    pace_ms: u64,
    progress_tx: Option<&Sender<(usize, usize, DownloadItemReport)>>,
) -> Result<Vec<DownloadItemReport>, String> {
    let http = ScholarlyHttpClient::new(polite_email)?;
    let pdf_dir = out_dir.join("pdfs");
    fs::create_dir_all(&pdf_dir)
        .map_err(|e| format!("Could not create download directory {:?}: {}", pdf_dir, e))?;

    let total = papers.len();
    let mut reports = Vec::with_capacity(total);

    for (idx, paper) in papers.iter_mut().enumerate() {
        if idx > 0 && pace_ms > 0 {
            thread::sleep(Duration::from_millis(pace_ms));
        }
        let rep = download_single_paper_pdf(&http, paper, &pdf_dir);
        if let Some(tx) = progress_tx {
            let _ = tx.send((idx + 1, total, rep.clone()));
        }
        reports.push(rep);
    }

    // Write BibTeX sidecar & Markdown/JSON audit manifests
    let bib_content = format_bibtex_corpus(papers);
    let bib_path = out_dir.join("corpus_references.bib");
    fs::write(&bib_path, bib_content)
        .map_err(|e| format!("Failed writing BibTeX manifest {:?}: {}", bib_path, e))?;

    let manifest_json = serde_json::to_string_pretty(&reports)
        .map_err(|e| format!("Failed serializing download manifest JSON: {}", e))?;
    let manifest_path = out_dir.join("download_manifest.json");
    fs::write(&manifest_path, manifest_json)
        .map_err(|e| format!("Failed writing download manifest {:?}: {}", manifest_path, e))?;

    let md_report = format_markdown_download_report(&reports);
    let md_path = out_dir.join("download_report.md");
    fs::write(&md_path, md_report)
        .map_err(|e| format!("Failed writing download report {:?}: {}", md_path, e))?;

    Ok(reports)
}

fn format_markdown_download_report(reports: &[DownloadItemReport]) -> String {
    let ok_count = reports.iter().filter(|r| r.status.is_ok()).count();
    let mut out = String::new();
    out.push_str("# CitAtlas Open-Access PDF Batch Download Report\n\n");
    out.push_str(&format!(
        "- Total Evaluated: {}\n- Verified PDFs Saved/Cached: {}\n\n",
        reports.len(),
        ok_count
    ));
    out.push_str("| Paper ID | DOI | Stage | Status | Size (KB) | SHA-256 Prefix |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for r in reports {
        let sha_short = r
            .sha256
            .as_deref()
            .map(|s| &s[..s.len().min(12)])
            .unwrap_or("-");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {:.1} | {} |\n",
            r.paper_id,
            if r.doi.is_empty() { "-" } else { &r.doi },
            r.resolved_stage,
            r.status.label(),
            (r.byte_len as f64) / 1024.0,
            sha_short
        ));
    }
    out
}
