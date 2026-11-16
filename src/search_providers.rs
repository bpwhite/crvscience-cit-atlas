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
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde_json::Value;

use crate::crawler::{
    crawl_biorxiv_medrxiv, crawl_dblp, crawl_europe_pmc, crawl_google_scholar,
    crawl_pubmed_entrez, hydrate_opencitations_and_icite, PoliteWebCrawler,
};
use crate::bio_entity_bridges::merge_bio_entity_into_paper;
use crate::integrity_bridges::{
    apply_retraction_update, parse_crossref_work_retraction, run_full_corpus_screening,
    search_datacite_works,
};
use crate::rate_limiter::{
    acquire_domain_permit_blocking, log_global_crawl_outcome, AcquirePermit, CrawlAuditOutcome,
};
use crate::models::{
    infer_concepts_from_text, CitationEdge, DataCiteArtifactKind, DataCiteLink, Discipline,
    OaStatus, PaperRecord, RetractionRecord, RetractionStatus, SearchHarvestSummary, SearchRequest,
};

pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

pub fn normalize_doi(raw: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("https://doi.org/")
        .or_else(|| trimmed.strip_prefix("http://doi.org/"))
        .or_else(|| trimmed.strip_prefix("doi:"))
        .or_else(|| trimmed.strip_prefix("DOI:"))
        .unwrap_or(trimmed);
    stripped.trim().to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierLookupKind {
    Doi(String),
    Pmid(String),
    Arxiv(String),
    Orcid(String),
}

/// Parses a user-supplied `DOI / PMID / arXiv / ORCID` identifier string into a normalized typed variant.
pub fn parse_identifier_lookup(raw: &str) -> Option<IdentifierLookupKind> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();

    if lower.contains("orcid.org/") || lower.starts_with("orcid:") {
        let id = trimmed
            .rsplit('/')
            .next()
            .unwrap_or(trimmed)
            .strip_prefix("orcid:")
            .or_else(|| trimmed.strip_prefix("ORCID:"))
            .unwrap_or(trimmed.rsplit('/').next().unwrap_or(trimmed))
            .trim()
            .to_ascii_uppercase();
        if !id.is_empty() {
            return Some(IdentifierLookupKind::Orcid(id));
        }
    }
    let clean_orcid = trimmed.trim().to_ascii_uppercase();
    if clean_orcid.len() == 19
        && clean_orcid
            .split('-')
            .enumerate()
            .all(|(i, seg)| {
                seg.len() == 4
                    && seg
                        .chars()
                        .all(|c| c.is_ascii_digit() || (i == 3 && c == 'X'))
            })
    {
        return Some(IdentifierLookupKind::Orcid(clean_orcid));
    }

    if lower.contains("10.") && lower.contains('/') {
        let after_org = trimmed
            .find("10.")
            .map(|idx| &trimmed[idx..])
            .unwrap_or(trimmed);
        let doi = normalize_doi(after_org);
        if doi.starts_with("10.") && doi.contains('/') {
            return Some(IdentifierLookupKind::Doi(doi));
        }
    }

    if lower.starts_with("pmid:") {
        let digits: String = trimmed["pmid:".len()..]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return Some(IdentifierLookupKind::Pmid(digits));
        }
    }
    if lower.contains("pubmed.ncbi.nlm.nih.gov/") {
        let digits: String = trimmed
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return Some(IdentifierLookupKind::Pmid(digits));
        }
    }

    if lower.starts_with("arxiv:") || lower.contains("arxiv.org/") {
        let id_raw = if let Some(rest) = trimmed.split("/abs/").nth(1) {
            rest
        } else if let Some(rest) = trimmed.split("/pdf/").nth(1) {
            rest.trim_end_matches(".pdf")
        } else {
            trimmed.split(':').nth(1).unwrap_or(trimmed)
        };
        let clean = id_raw.trim().to_string();
        if !clean.is_empty() {
            return Some(IdentifierLookupKind::Arxiv(clean));
        }
    }

    // Bare arXiv new-style ID: YYMM.NNNNN(vN)
    if let Some((lhs, rhs)) = trimmed.split_once('.') {
        let rhs_digits = rhs.split('v').next().unwrap_or(rhs);
        if lhs.len() == 4
            && lhs.chars().all(|c| c.is_ascii_digit())
            && (4..=5).contains(&rhs_digits.len())
            && rhs_digits.chars().all(|c| c.is_ascii_digit())
        {
            return Some(IdentifierLookupKind::Arxiv(trimmed.to_string()));
        }
    }

    // Bare numeric PMID
    if (5..=10).contains(&trimmed.len()) && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(IdentifierLookupKind::Pmid(trimmed.to_string()));
    }

    None
}

pub fn has_any_search_terms(req: &SearchRequest) -> bool {
    !req.query.trim().is_empty()
        || !req.authors.trim().is_empty()
        || !req.publication_venue.trim().is_empty()
        || !req.title_words.trim().is_empty()
        || !req.keywords_all.trim().is_empty()
        || !req.identifier_lookup.trim().is_empty()
        || !req.crawl.scholar_cites_cluster_id.trim().is_empty()
        || !req.crawl.local_html_paths.is_empty()
}

pub fn build_effective_query(req: &SearchRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    for field in [
        req.query.trim(),
        req.title_words.trim(),
        req.keywords_all.trim(),
        req.authors.trim(),
        req.publication_venue.trim(),
        req.identifier_lookup.trim(),
    ] {
        if !field.is_empty() && !parts.iter().any(|p| p.eq_ignore_ascii_case(field)) {
            parts.push(field.to_string());
        }
    }
    if parts.is_empty() {
        "science".to_string()
    } else {
        parts.join(" ")
    }
}

fn split_pop_terms(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub fn build_openalex_query_url(
    req: &SearchRequest,
    polite_email: &str,
    api_key: Option<&str>,
) -> String {
    let per_page = req.max_per_provider.clamp(1, 100);
    let mut filters: Vec<String> = Vec::new();
    if req.oa_only {
        filters.push("open_access.is_oa:true".to_string());
    }
    if let Some(y_min) = req.year_min {
        filters.push(format!("from_publication_date:{}-01-01", y_min));
    }
    if let Some(y_max) = req.year_max {
        filters.push(format!("to_publication_date:{}-12-31", y_max));
    }
    if !req.title_words.trim().is_empty() {
        filters.push(format!("title.search:{}", percent_encode(req.title_words.trim())));
    }
    if !req.authors.trim().is_empty() {
        filters.push(format!("raw_author_name.search:{}", percent_encode(req.authors.trim())));
    }
    if !req.publication_venue.trim().is_empty() {
        filters.push(format!(
            "primary_location.source.display_name.search:{}",
            percent_encode(req.publication_venue.trim())
        ));
    }

    let mut search_terms: Vec<String> = Vec::new();
    if !req.query.trim().is_empty() {
        search_terms.push(req.query.trim().to_string());
    }
    if !req.keywords_all.trim().is_empty() {
        search_terms.push(req.keywords_all.trim().to_string());
    }
    match parse_identifier_lookup(&req.identifier_lookup) {
        Some(IdentifierLookupKind::Doi(doi)) => {
            filters.push(format!("doi:https://doi.org/{}", percent_encode(&doi)));
        }
        Some(IdentifierLookupKind::Pmid(pmid)) => {
            filters.push(format!("ids.pmid:https://pubmed.ncbi.nlm.nih.gov/{}", pmid));
        }
        Some(IdentifierLookupKind::Orcid(orcid)) => {
            filters.push(format!("authorships.author.orcid:https://orcid.org/{}", orcid));
        }
        Some(IdentifierLookupKind::Arxiv(aid)) => {
            search_terms.push(aid);
        }
        None if !req.identifier_lookup.trim().is_empty() => {
            search_terms.push(req.identifier_lookup.trim().to_string());
        }
        None => {}
    }

    let mut url = if search_terms.is_empty() {
        format!(
            "https://api.openalex.org/works?per-page={}&mailto={}",
            per_page,
            percent_encode(polite_email)
        )
    } else {
        format!(
            "https://api.openalex.org/works?search={}&per-page={}&mailto={}",
            percent_encode(&search_terms.join(" ")),
            per_page,
            percent_encode(polite_email)
        )
    };
    if !filters.is_empty() {
        url.push_str("&filter=");
        url.push_str(&filters.join(","));
    }
    if let Some(k) = api_key {
        url.push_str("&api_key=");
        url.push_str(&percent_encode(k));
    }
    url
}

pub fn build_crossref_query_url(req: &SearchRequest, polite_email: &str) -> String {
    let rows = req.max_per_provider.clamp(1, 50);
    let mut filters = Vec::new();
    if let Some(y_min) = req.year_min {
        filters.push(format!("from-pub-date:{}", y_min));
    }
    if let Some(y_max) = req.year_max {
        filters.push(format!("until-pub-date:{}", y_max));
    }
    if req.oa_only {
        filters.push("has-full-text:true".to_string());
    }
    if let Some(IdentifierLookupKind::Doi(doi)) = parse_identifier_lookup(&req.identifier_lookup) {
        filters.push(format!("doi:{}", percent_encode(&doi)));
    } else if let Some(IdentifierLookupKind::Orcid(orcid)) =
        parse_identifier_lookup(&req.identifier_lookup)
    {
        filters.push(format!("orcid:{}", percent_encode(&orcid)));
    }

    let mut url = format!(
        "https://api.crossref.org/works?rows={}&mailto={}&select=DOI,title,author,published,container-title,is-referenced-by-count,reference,link,subject",
        rows,
        percent_encode(polite_email)
    );
    let mut biblio: Vec<&str> = Vec::new();
    if !req.query.trim().is_empty() {
        biblio.push(req.query.trim());
    }
    if !req.keywords_all.trim().is_empty() {
        biblio.push(req.keywords_all.trim());
    }
    if !biblio.is_empty() {
        url.push_str("&query.bibliographic=");
        url.push_str(&percent_encode(&biblio.join(" ")));
    } else if req.title_words.trim().is_empty()
        && req.authors.trim().is_empty()
        && req.publication_venue.trim().is_empty()
        && filters.is_empty()
    {
        url.push_str("&query=science");
    }
    if !req.title_words.trim().is_empty() {
        url.push_str("&query.title=");
        url.push_str(&percent_encode(req.title_words.trim()));
    }
    if !req.authors.trim().is_empty() {
        url.push_str("&query.author=");
        url.push_str(&percent_encode(req.authors.trim()));
    }
    if !req.publication_venue.trim().is_empty() {
        url.push_str("&query.container-title=");
        url.push_str(&percent_encode(req.publication_venue.trim()));
    }
    if !filters.is_empty() {
        url.push_str("&filter=");
        url.push_str(&filters.join(","));
    }
    url
}

pub fn build_arxiv_query_url(req: &SearchRequest) -> String {
    let max_res = req.max_per_provider.clamp(1, 50);
    if let Some(IdentifierLookupKind::Arxiv(aid)) = parse_identifier_lookup(&req.identifier_lookup) {
        return format!(
            "https://export.arxiv.org/api/query?id_list={}&start=0&max_results={}",
            percent_encode(&aid),
            max_res
        );
    }

    let mut clauses: Vec<String> = Vec::new();
    if !req.title_words.trim().is_empty() {
        for w in req.title_words.split_whitespace() {
            clauses.push(format!("ti:{}", w));
        }
    }
    if !req.authors.trim().is_empty() {
        clauses.push(format!("au:\"{}\"", req.authors.trim()));
    }
    if !req.publication_venue.trim().is_empty() {
        clauses.push(format!("jr:\"{}\"", req.publication_venue.trim()));
    }
    if !req.keywords_all.trim().is_empty() {
        for w in req.keywords_all.split_whitespace() {
            clauses.push(format!("all:{}", w));
        }
    }
    if !req.query.trim().is_empty() {
        clauses.push(format!("all:{}", req.query.trim()));
    }
    if clauses.is_empty() {
        let eff = build_effective_query(req);
        clauses.push(format!("all:{}", eff));
    }
    let mut search_query = clauses.join(" AND ");
    let excludes = split_pop_terms(&req.exclude_words);
    if !excludes.is_empty() {
        let ex_str = excludes
            .iter()
            .map(|w| format!("all:{}", w))
            .collect::<Vec<_>>()
            .join(" OR ");
        search_query = format!("({}) ANDNOT ({})", search_query, ex_str);
    }

    format!(
        "https://export.arxiv.org/api/query?search_query={}&start=0&max_results={}",
        percent_encode(&search_query),
        max_res
    )
}

pub fn build_semantic_scholar_query_url(req: &SearchRequest) -> String {
    let limit = req.max_per_provider.clamp(1, 50);
    let q_str = build_effective_query(req);
    let mut url = format!(
        "https://api.semanticscholar.org/graph/v1/paper/search?query={}&limit={}&fields=paperId,externalIds,title,authors,year,venue,citationCount,openAccessPdf,fieldsOfStudy,abstract,references",
        percent_encode(&q_str),
        limit
    );
    if !req.publication_venue.trim().is_empty() {
        url.push_str("&venue=");
        url.push_str(&percent_encode(req.publication_venue.trim()));
    }
    if req.oa_only {
        url.push_str("&openAccessPdf");
    }
    if req.year_min.is_some() || req.year_max.is_some() {
        let y1 = req.year_min.map(|y| y.to_string()).unwrap_or_default();
        let y2 = req.year_max.map(|y| y.to_string()).unwrap_or_default();
        url.push_str(&format!("&year={}-{}", y1, y2));
    }
    url
}

pub fn build_openaire_query_url(req: &SearchRequest) -> String {
    let page_size = req.max_per_provider.clamp(1, 100);
    let mut text_terms: Vec<&str> = Vec::new();
    if !req.query.trim().is_empty() {
        text_terms.push(req.query.trim());
    }
    if !req.title_words.trim().is_empty() {
        text_terms.push(req.title_words.trim());
    }
    if !req.keywords_all.trim().is_empty() {
        text_terms.push(req.keywords_all.trim());
    }
    if !req.publication_venue.trim().is_empty() {
        text_terms.push(req.publication_venue.trim());
    }
    let q = if text_terms.is_empty() {
        build_effective_query(req)
    } else {
        text_terms.join(" ")
    };

    let mut url = format!(
        "https://api.openaire.eu/graph/v2/researchProducts?search={}&type=publication&pageSize={}&page=1",
        percent_encode(&q),
        page_size
    );
    if !req.authors.trim().is_empty() {
        url.push_str("&author=");
        url.push_str(&percent_encode(req.authors.trim()));
    }
    match parse_identifier_lookup(&req.identifier_lookup) {
        Some(IdentifierLookupKind::Doi(doi)) => {
            url.push_str("&doi=");
            url.push_str(&percent_encode(&doi));
        }
        Some(IdentifierLookupKind::Orcid(orcid)) => {
            url.push_str("&relCollectedFromOrAuthorOrcid=");
            url.push_str(&percent_encode(&orcid));
        }
        _ => {}
    }
    if let Some(ymin) = req.year_min {
        url.push_str(&format!("&fromPublicationDate={}-01-01", ymin));
    }
    if let Some(ymax) = req.year_max {
        url.push_str(&format!("&toPublicationDate={}-12-31", ymax));
    }
    if req.oa_only {
        url.push_str("&isOpenAccess=true");
    }
    url
}

pub fn build_inspire_hep_query(req: &SearchRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    match parse_identifier_lookup(&req.identifier_lookup) {
        Some(IdentifierLookupKind::Doi(doi)) => parts.push(format!("doi {}", doi)),
        Some(IdentifierLookupKind::Arxiv(aid)) => parts.push(format!("arxiv:{}", aid)),
        Some(IdentifierLookupKind::Orcid(orcid)) => parts.push(format!("a {}", orcid)),
        _ => {}
    }
    if !req.authors.trim().is_empty() {
        parts.push(format!("a \"{}\"", req.authors.trim()));
    }
    if !req.title_words.trim().is_empty() {
        parts.push(format!("t \"{}\"", req.title_words.trim()));
    }
    if !req.publication_venue.trim().is_empty() {
        parts.push(format!("j \"{}\"", req.publication_venue.trim()));
    }
    if !req.query.trim().is_empty() {
        parts.push(req.query.trim().to_string());
    }
    if !req.keywords_all.trim().is_empty() {
        parts.push(req.keywords_all.trim().to_string());
    }
    for ex in split_pop_terms(&req.exclude_words) {
        parts.push(format!("not {}", ex));
    }
    if let Some(ymin) = req.year_min {
        parts.push(format!("de >= {}", ymin));
    }
    if let Some(ymax) = req.year_max {
        parts.push(format!("de <= {}", ymax));
    }
    if parts.is_empty() {
        build_effective_query(req)
    } else {
        parts.join(" and ")
    }
}

pub fn build_nasa_ads_query(req: &SearchRequest) -> String {
    let mut clauses: Vec<String> = Vec::new();
    match parse_identifier_lookup(&req.identifier_lookup) {
        Some(IdentifierLookupKind::Doi(doi)) => clauses.push(format!("doi:\"{}\"", doi)),
        Some(IdentifierLookupKind::Arxiv(aid)) => clauses.push(format!("identifier:arXiv:{}", aid)),
        Some(IdentifierLookupKind::Orcid(orcid)) => clauses.push(format!("orcid:{}", orcid)),
        Some(IdentifierLookupKind::Pmid(pmid)) => clauses.push(format!("identifier:{}", pmid)),
        None => {}
    }
    if !req.query.trim().is_empty() {
        clauses.push(format!("({})", req.query.trim()));
    }
    if !req.keywords_all.trim().is_empty() {
        clauses.push(format!("({})", req.keywords_all.trim()));
    }
    if !req.title_words.trim().is_empty() {
        clauses.push(format!("title:({})", req.title_words.trim()));
    }
    if !req.authors.trim().is_empty() {
        clauses.push(format!("author:\"{}\"", req.authors.trim()));
    }
    if !req.publication_venue.trim().is_empty() {
        clauses.push(format!("pub:\"{}\"", req.publication_venue.trim()));
    }
    if req.year_min.is_some() || req.year_max.is_some() {
        let y_lo = req.year_min.map(|y| y.to_string()).unwrap_or_else(|| "*".to_string());
        let y_hi = req.year_max.map(|y| y.to_string()).unwrap_or_else(|| "*".to_string());
        clauses.push(format!("year:[{} TO {}]", y_lo, y_hi));
    }
    if req.oa_only {
        clauses.push("property:openaccess".to_string());
    }
    let mut solr_q = if clauses.is_empty() {
        format!("({})", build_effective_query(req))
    } else {
        clauses.join(" AND ")
    };
    for ex in split_pop_terms(&req.exclude_words) {
        solr_q.push_str(&format!(" -{}", ex));
    }
    solr_q
}

pub fn passes_pop_exclude_filter(rec: &PaperRecord, exclude_words: &str) -> bool {
    let excludes = split_pop_terms(exclude_words);
    if excludes.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {} {} {}",
        rec.title,
        rec.authors,
        rec.venue,
        rec.abstract_text,
        rec.terms.join(" ")
    )
    .to_ascii_lowercase();
    !excludes
        .iter()
        .any(|ex| haystack.contains(&ex.to_ascii_lowercase()))
}

fn extended_offline_benchmark_records() -> Vec<PaperRecord> {
    struct Bench {
        id: &'static str,
        title: &'static str,
        authors: &'static str,
        year: i32,
        venue: &'static str,
        disc: Discipline,
        doi: &'static str,
        arxiv_id: &'static str,
        pmid: &'static str,
        cites: u64,
        terms: &'static [&'static str],
    }

    const BENCH: &[Bench] = &[
        Bench {
            id: "hirsch2005",
            title: "An index to quantify an individual's scientific research output",
            authors: "J. E. Hirsch",
            year: 2005,
            venue: "Proceedings of the National Academy of Sciences (PNAS) 102(46): 16569-16572",
            disc: Discipline::SCI,
            doi: "10.1073/pnas.0507655102",
            arxiv_id: "physics/0508025",
            pmid: "16275915",
            cites: 15840,
            terms: &["h-index", "bibliometrics", "citation metrics", "research evaluation"],
        },
        Bench {
            id: "egghe2006",
            title: "Theory and practise of the g-index",
            authors: "L. Egghe",
            year: 2006,
            venue: "Scientometrics 69(1): 131-152",
            disc: Discipline::SCI,
            doi: "10.1007/s11192-006-0144-7",
            arxiv_id: "",
            pmid: "",
            cites: 3420,
            terms: &["g-index", "h-index", "citation distribution", "publish or perish"],
        },
        Bench {
            id: "harzing2008",
            title: "Google Scholar as a new source for citation analysis",
            authors: "A.-W. Harzing, R. van der Wal",
            year: 2008,
            venue: "Ethics in Science and Environmental Politics 8(1): 61-73",
            disc: Discipline::SCI,
            doi: "10.3354/esep00076",
            arxiv_id: "",
            pmid: "",
            cites: 2150,
            terms: &["Publish or Perish", "Google Scholar", "hI,norm", "citation metrics", "bibliometrics"],
        },
        Bench {
            id: "zhang2009",
            title: "The e-index, complementing the h-index for excess citations",
            authors: "C.-T. Zhang",
            year: 2009,
            venue: "PLOS ONE 4(5): e5429",
            disc: Discipline::SCI,
            doi: "10.1371/journal.pone.0005429",
            arxiv_id: "",
            pmid: "19415119",
            cites: 790,
            terms: &["e-index", "h-index", "excess citations", "scientometrics"],
        },
        Bench {
            id: "harzing2014",
            title: "hIa: An individual annual h-index to accommodate disciplinary and career length differences",
            authors: "A.-W. Harzing, S. Alakangas, D. Adams",
            year: 2014,
            venue: "Scientometrics 99(3): 811-821",
            disc: Discipline::SCI,
            doi: "10.1007/s11192-013-1208-0",
            arxiv_id: "",
            pmid: "",
            cites: 410,
            terms: &["hI,annual", "hI,norm", "Publish or Perish", "career length normalization"],
        },
        Bench {
            id: "vaswani2017",
            title: "Attention is all you need",
            authors: "A. Vaswani, N. Shazeer, N. Parmar, J. Uszkoreit, L. Jones, A. N. Gomez, L. Kaiser, I. Polosukhin",
            year: 2017,
            venue: "Advances in Neural Information Processing Systems (NeurIPS) 30",
            disc: Discipline::ML,
            doi: "10.5555/3295222.3295349",
            arxiv_id: "1706.03762",
            pmid: "",
            cites: 138000,
            terms: &["transformer", "self-attention", "sequence transduction", "neural language model"],
        },
        Bench {
            id: "hu2022_lora",
            title: "LoRA: Low-Rank Adaptation of Large Language Models",
            authors: "E. J. Hu, Y. Shen, P. Wallis, Z. Allen-Zhu, Y. Li, S. Wang, L. Wang, W. Chen",
            year: 2022,
            venue: "International Conference on Learning Representations (ICLR)",
            disc: Discipline::ML,
            doi: "10.48550/arXiv.2106.09685",
            arxiv_id: "2106.09685",
            pmid: "",
            cites: 14200,
            terms: &["parameter-efficient fine-tuning", "transformer", "low-rank adaptation"],
        },
        Bench {
            id: "velickovic2018",
            title: "Graph Attention Networks",
            authors: "P. Velickovic, G. Cucurull, A. Casanova, A. Romero, P. Lio, Y. Bengio",
            year: 2018,
            venue: "International Conference on Learning Representations (ICLR)",
            disc: Discipline::NET,
            doi: "10.48550/arXiv.1710.10903",
            arxiv_id: "1710.10903",
            pmid: "",
            cites: 24500,
            terms: &["graph neural network", "attention", "citation network", "node classification"],
        },
        Bench {
            id: "abramson2024_af3",
            title: "Accurate structure prediction of biomolecular interactions with AlphaFold 3",
            authors: "J. Abramson, J. Adler, J. Dunger, R. Evans, T. Green, D. Hassabis, J. Jumper",
            year: 2024,
            venue: "Nature 630: 493-500",
            disc: Discipline::EVO,
            doi: "10.1038/s41586-024-07487-w",
            arxiv_id: "",
            pmid: "38718835",
            cites: 4120,
            terms: &["AlphaFold", "protein structure prediction", "diffusion", "biomolecular complex"],
        },
        Bench {
            id: "lin2023_esmfold",
            title: "Evolutionary-scale prediction of atomic-level protein structure with a language model",
            authors: "Z. Lin, H. Akin, R. Rao, B. Hie, Z. Zhu, W. Lu, A. Rives",
            year: 2023,
            venue: "Science 379(6637): 1123-1130",
            disc: Discipline::EVO,
            doi: "10.1126/science.ade2574",
            arxiv_id: "",
            pmid: "36927031",
            cites: 3180,
            terms: &["protein language model", "ESMFold", "single-sequence structure", "coevolution"],
        },
        Bench {
            id: "jinek2012",
            title: "A programmable dual-RNA-guided DNA endonuclease in adaptive bacterial immunity",
            authors: "M. Jinek, K. Chylinski, I. Fonfara, M. Hauer, J. A. Doudna, E. Charpentier",
            year: 2012,
            venue: "Science 337(6096): 816-821",
            disc: Discipline::EPI,
            doi: "10.1126/science.1225829",
            arxiv_id: "",
            pmid: "22745249",
            cites: 19600,
            terms: &["CRISPR-Cas9", "genome editing", "guide RNA", "endonuclease"],
        },
        Bench {
            id: "abbott2016_ligo",
            title: "Observation of Gravitational Waves from a Binary Black Hole Merger",
            authors: "B. P. Abbott, R. Abbott, T. D. Abbott, M. R. Abernathy, F. Acernese, K. Ackley",
            year: 2016,
            venue: "Physical Review Letters 116(6): 061102",
            disc: Discipline::INF,
            doi: "10.1103/PhysRevLett.116.061102",
            arxiv_id: "1602.03837",
            pmid: "",
            cites: 14900,
            terms: &["gravitational waves", "LIGO", "binary black hole", "general relativity"],
        },
        Bench {
            id: "eht2019_m87",
            title: "First M87 Event Horizon Telescope Results. I. The Shadow of the Supermassive Black Hole",
            authors: "Event Horizon Telescope Collaboration, K. Akiyama, A. Alberdi, W. Alef",
            year: 2019,
            venue: "The Astrophysical Journal Letters 875(1): L1",
            disc: Discipline::INF,
            doi: "10.3847/2041-8213/ab0ec7",
            arxiv_id: "1906.11238",
            pmid: "",
            cites: 4850,
            terms: &["black hole", "VLBI", "Event Horizon Telescope", "astrophysics"],
        },
        Bench {
            id: "he2016_resnet",
            title: "Deep Residual Learning for Image Recognition",
            authors: "K. He, X. Zhang, S. Ren, J. Sun",
            year: 2016,
            venue: "IEEE Conference on Computer Vision and Pattern Recognition (CVPR)",
            disc: Discipline::ML,
            doi: "10.1109/CVPR.2016.90",
            arxiv_id: "1512.03385",
            pmid: "",
            cites: 245000,
            terms: &["residual network", "deep learning", "skip connection", "computer vision"],
        },
    ];

    BENCH
        .iter()
        .map(|b| {
            let mut terms: Vec<String> = b.terms.iter().map(|t| (*t).to_string()).collect();
            if !b.pmid.is_empty() {
                terms.push(format!("pmid:{}", b.pmid));
            }
            PaperRecord {
                id: b.id.to_string(),
                title: b.title.to_string(),
                authors: b.authors.to_string(),
                year: b.year,
                venue: b.venue.to_string(),
                disc: b.disc,
                doi: b.doi.to_string(),
                arxiv_id: b.arxiv_id.to_string(),
                openalex_id: String::new(),
                s2_id: String::new(),
                pmcid: String::new(),
                source_url: if !b.doi.is_empty() {
                    format!("https://doi.org/{}", b.doi)
                } else {
                    String::new()
                },
                oa_pdf_url: if !b.arxiv_id.is_empty() {
                    format!("https://arxiv.org/pdf/{}.pdf", b.arxiv_id)
                } else {
                    String::new()
                },
                oa_status: if !b.arxiv_id.is_empty() {
                    OaStatus::Arxiv
                } else {
                    OaStatus::Bronze
                },
                external_citations: Some(b.cites),
                abstract_text: b.terms.join("; "),
                concepts: infer_concepts_from_text(&format!("{} {}", b.title, b.terms.join(" ")), b.disc),
                terms,
                local_pdf_path: None,
                pdf_sha256: None,
                pdf_bytes: None,
                raw_references: Vec::new(),
                retraction: RetractionRecord::default(),
                datacite_links: Vec::new(),
                bio_entities: Vec::new(),
            }
        })
        .collect()
}

/// Fast local scholarly index fallback over `EMPIRICAL_SEED` + curated Publish-or-Perish benchmark
/// works, filtering natively against all PoP fields (Author, Title, Venue, Keywords, Exclude, DOI/PMID/arXiv).
pub fn search_offline_scholarly_index(req: &SearchRequest) -> Vec<PaperRecord> {
    let (mut seed_papers, seed_edges) = crate::corpus::build_empirical_corpus();
    let mut in_counts: HashMap<String, u64> = HashMap::new();
    for e in &seed_edges {
        *in_counts.entry(e.target.clone()).or_insert(0) += 1;
    }
    for p in &mut seed_papers {
        if p.external_citations.is_none() {
            let base_in = in_counts.get(&p.id).copied().unwrap_or(0);
            // Realistic bibliometric floor derived deterministically from age & empirical in-degree
            let age = (2026 - p.year + 1).max(1) as u64;
            p.external_citations = Some(base_in.saturating_mul(180).saturating_add(age * 45 + 120));
        }
    }

    let mut catalog = seed_papers;
    let mut existing_dois: HashSet<String> = catalog
        .iter()
        .map(|p| normalize_doi(&p.doi))
        .filter(|d| !d.is_empty())
        .collect();
    for extra in extended_offline_benchmark_records() {
        let d = normalize_doi(&extra.doi);
        if d.is_empty() || existing_dois.insert(d) {
            catalog.push(extra);
        }
    }

    let id_lookup = parse_identifier_lookup(&req.identifier_lookup);
    let author_tokens = split_pop_terms(&req.authors.to_ascii_lowercase());
    let venue_tokens = split_pop_terms(&req.publication_venue.to_ascii_lowercase());
    let title_tokens = split_pop_terms(&req.title_words.to_ascii_lowercase());
    let kw_all_tokens = split_pop_terms(&req.keywords_all.to_ascii_lowercase());
    let q_tokens = split_pop_terms(&req.query.to_ascii_lowercase());

    let mut scored: Vec<(usize, u64, PaperRecord)> = Vec::new();
    for rec in catalog {
        if let Some(ymin) = req.year_min {
            if rec.year < ymin {
                continue;
            }
        }
        if let Some(ymax) = req.year_max {
            if rec.year > ymax {
                continue;
            }
        }
        if req.oa_only && matches!(rec.oa_status, OaStatus::Closed | OaStatus::Bronze) {
            continue;
        }
        if !passes_pop_exclude_filter(&rec, &req.exclude_words) {
            continue;
        }

        if let Some(ref lk) = id_lookup {
            let matched_id = match lk {
                IdentifierLookupKind::Doi(doi) => normalize_doi(&rec.doi) == *doi,
                IdentifierLookupKind::Arxiv(aid) => {
                    !rec.arxiv_id.is_empty()
                        && rec.arxiv_id.eq_ignore_ascii_case(aid.split('v').next().unwrap_or(aid))
                }
                IdentifierLookupKind::Pmid(pmid) => {
                    rec.terms.iter().any(|t| t == &format!("pmid:{}", pmid))
                }
                IdentifierLookupKind::Orcid(_) => true,
            };
            if !matched_id {
                continue;
            }
        } else if !req.identifier_lookup.trim().is_empty() {
            let needle = req.identifier_lookup.trim().to_ascii_lowercase();
            let hit = rec.doi.to_ascii_lowercase().contains(&needle)
                || rec.arxiv_id.to_ascii_lowercase().contains(&needle)
                || rec.pmcid.to_ascii_lowercase().contains(&needle)
                || rec.terms.iter().any(|t| t.to_ascii_lowercase().contains(&needle));
            if !hit {
                continue;
            }
        }

        let authors_low = rec.authors.to_ascii_lowercase();
        if !author_tokens.is_empty() && !author_tokens.iter().any(|a| authors_low.contains(a)) {
            continue;
        }

        let venue_low = rec.venue.to_ascii_lowercase();
        if !venue_tokens.is_empty() && !venue_tokens.iter().all(|v| venue_low.contains(v)) {
            continue;
        }

        let title_low = rec.title.to_ascii_lowercase();
        if !title_tokens.is_empty() && !title_tokens.iter().all(|t| title_low.contains(t)) {
            continue;
        }

        let blob = format!(
            "{} {} {} {} {} {}",
            title_low,
            authors_low,
            venue_low,
            rec.abstract_text.to_ascii_lowercase(),
            rec.terms.join(" ").to_ascii_lowercase(),
            rec.concepts.join(" ").to_ascii_lowercase(),
        );
        if !kw_all_tokens.is_empty() && !kw_all_tokens.iter().all(|k| blob.contains(k)) {
            continue;
        }

        let q_hits = q_tokens.iter().filter(|t| blob.contains(t.as_str())).count();
        if !q_tokens.is_empty()
            && q_hits == 0
            && author_tokens.is_empty()
            && venue_tokens.is_empty()
            && title_tokens.is_empty()
            && kw_all_tokens.is_empty()
            && req.identifier_lookup.trim().is_empty()
        {
            continue;
        }

        let cites = rec.external_citations.unwrap_or(0);
        scored.push((q_hits, cites, rec));
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let limit = req.max_per_provider.max(1) * 2;
    scored.into_iter().take(limit).map(|(_, _, r)| r).collect()
}

pub fn search_offline_scholarly_only(req: &SearchRequest) -> SearchHarvestSummary {
    let raw_papers = search_offline_scholarly_index(req);
    let count = raw_papers.len();
    let (mut papers, edges) =
        merge_and_link_records(raw_papers, req.oa_only, req.year_min, req.year_max);
    papers.retain(|p| passes_pop_exclude_filter(p, &req.exclude_words));
    SearchHarvestSummary {
        papers,
        edges,
        provider_counts: vec![("Local Scholarly Index".to_string(), count)],
        warnings: Vec::new(),
    }
}

pub struct ScholarlyHttpClient {
    client: Client,
    polite_email: String,
    openalex_api_key: Option<String>,
    s2_api_key: Option<String>,
    openaire_api_token: Option<String>,
    nasa_ads_token: Option<String>,
}

impl ScholarlyHttpClient {
    pub fn new(polite_email: &str) -> Result<Self, String> {
        Self::with_tokens(polite_email, "", "")
    }

    pub fn with_tokens(
        polite_email: &str,
        openaire_override: &str,
        ads_override: &str,
    ) -> Result<Self, String> {
        let email = if polite_email.trim().is_empty() {
            "research@crvscience.com"
        } else {
            polite_email.trim()
        };
        let ua = format!(
            "CitAtlas/1.8 (https://crvscience.com; mailto:{})",
            email
        );
        let client = Client::builder()
            .user_agent(ua)
            .timeout(Duration::from_secs(28))
            .build()
            .map_err(|e| format!("Failed to initialize HTTPS client: {}", e))?;

        let openaire_api_token = Some(openaire_override.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("OPENAIRE_API_TOKEN")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            });

        let nasa_ads_token = Some(ads_override.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                ["ADS_API_TOKEN", "NASA_ADS_TOKEN", "ADS_DEV_KEY"]
                    .iter()
                    .find_map(|k| {
                        std::env::var(k)
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    })
            });

        Ok(Self {
            client,
            polite_email: email.to_string(),
            openalex_api_key: std::env::var("OPENALEX_API_KEY").ok().filter(|s| !s.is_empty()),
            s2_api_key: std::env::var("S2_API_KEY").ok().filter(|s| !s.is_empty()),
            openaire_api_token,
            nasa_ads_token,
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn polite_email(&self) -> &str {
        &self.polite_email
    }

    pub fn openaire_token(&self) -> Option<&str> {
        self.openaire_api_token.as_deref()
    }

    pub fn nasa_ads_token(&self) -> Option<&str> {
        self.nasa_ads_token.as_deref()
    }

    pub fn get_json_with_bearer(
        &self,
        url: &str,
        bearer_token: Option<&str>,
        subsystem: &str,
    ) -> Result<Value, String> {
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                thread::sleep(Duration::from_millis(650 * (1 << attempt)));
            }
            let wait_ms = match acquire_domain_permit_blocking(url) {
                AcquirePermit::Granted { wait_ms, .. } => wait_ms,
                AcquirePermit::CircuitOpen { reason, .. } => {
                    log_global_crawl_outcome(
                        subsystem,
                        "GET",
                        url,
                        CrawlAuditOutcome::CircuitBreakerSkipped,
                        0,
                        0,
                        0,
                        0,
                        None,
                        "n/a_api",
                        &reason,
                    );
                    return Err(reason);
                }
            };
            let mut req = self.client.get(url);
            if let Some(tok) = bearer_token.filter(|s| !s.trim().is_empty()) {
                req = req.header("Authorization", format!("Bearer {}", tok.trim()));
            }
            let t0 = Instant::now();
            match req.send() {
                Ok(resp) => {
                    let status = resp.status();
                    let code = status.as_u16();
                    let hdrs = resp.headers().clone();
                    if status.is_success() {
                        let text = resp
                            .text()
                            .map_err(|e| format!("Text read error from {}: {}", url, e))?;
                        let byte_len = text.len() as u64;
                        log_global_crawl_outcome(
                            subsystem,
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
                            byte_len,
                            Some(&hdrs),
                            "n/a_api",
                            "",
                        );
                        return serde_json::from_str::<Value>(&text)
                            .map_err(|e| format!("JSON decode error from {}: {}", url, e));
                    } else if code == 429 || status.is_server_error() {
                        log_global_crawl_outcome(
                            subsystem,
                            "GET",
                            url,
                            if code == 429 {
                                CrawlAuditOutcome::Http429RateLimited
                            } else {
                                CrawlAuditOutcome::HttpOtherError
                            },
                            code,
                            wait_ms,
                            t0.elapsed().as_millis() as u64,
                            0,
                            Some(&hdrs),
                            "n/a_api",
                            &format!("Retry attempt {}", attempt + 1),
                        );
                        last_err = format!("HTTP {} from {}", code, url);
                        continue;
                    } else {
                        log_global_crawl_outcome(
                            subsystem,
                            "GET",
                            url,
                            if code == 403 {
                                CrawlAuditOutcome::Http403OrCaptcha
                            } else {
                                CrawlAuditOutcome::HttpOtherError
                            },
                            code,
                            wait_ms,
                            t0.elapsed().as_millis() as u64,
                            0,
                            Some(&hdrs),
                            "n/a_api",
                            "HTTP error",
                        );
                        return Err(format!("HTTP {} from {}", code, url));
                    }
                }
                Err(e) => {
                    log_global_crawl_outcome(
                        subsystem,
                        "GET",
                        url,
                        CrawlAuditOutcome::NetworkError,
                        0,
                        wait_ms,
                        t0.elapsed().as_millis() as u64,
                        0,
                        None,
                        "n/a_api",
                        &e.to_string(),
                    );
                    last_err = format!("Request error for {}: {}", url, e);
                }
            }
        }
        Err(last_err)
    }

    pub fn get_json_with_retry(&self, url: &str, is_s2: bool) -> Result<Value, String> {
        let subsys = if url.contains("unpaywall.org") {
            "oa_pdf_downloader"
        } else {
            "search_api"
        };
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                thread::sleep(Duration::from_millis(650 * (1 << attempt)));
            }
            let wait_ms = match acquire_domain_permit_blocking(url) {
                AcquirePermit::Granted { wait_ms, .. } => wait_ms,
                AcquirePermit::CircuitOpen { reason, .. } => {
                    log_global_crawl_outcome(
                        subsys,
                        "GET",
                        url,
                        CrawlAuditOutcome::CircuitBreakerSkipped,
                        0,
                        0,
                        0,
                        0,
                        None,
                        "n/a_api",
                        &reason,
                    );
                    return Err(reason);
                }
            };
            let mut req = self.client.get(url);
            if is_s2 {
                if let Some(ref k) = self.s2_api_key {
                    req = req.header("x-api-key", k);
                }
            }
            let t0 = Instant::now();
            match req.send() {
                Ok(resp) => {
                    let status = resp.status();
                    let code = status.as_u16();
                    let hdrs = resp.headers().clone();
                    if status.is_success() {
                        let text = resp
                            .text()
                            .map_err(|e| format!("Text read error from {}: {}", url, e))?;
                        let byte_len = text.len() as u64;
                        log_global_crawl_outcome(
                            subsys,
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
                            byte_len,
                            Some(&hdrs),
                            "n/a_api",
                            "",
                        );
                        return serde_json::from_str::<Value>(&text)
                            .map_err(|e| format!("JSON decode error from {}: {}", url, e));
                    } else if code == 429 || status.is_server_error() {
                        log_global_crawl_outcome(
                            subsys,
                            "GET",
                            url,
                            if code == 429 {
                                CrawlAuditOutcome::Http429RateLimited
                            } else {
                                CrawlAuditOutcome::HttpOtherError
                            },
                            code,
                            wait_ms,
                            t0.elapsed().as_millis() as u64,
                            0,
                            Some(&hdrs),
                            "n/a_api",
                            &format!("Retry attempt {}", attempt + 1),
                        );
                        last_err = format!("HTTP {} from {}", code, url);
                        continue;
                    } else {
                        log_global_crawl_outcome(
                            subsys,
                            "GET",
                            url,
                            CrawlAuditOutcome::HttpOtherError,
                            code,
                            wait_ms,
                            t0.elapsed().as_millis() as u64,
                            0,
                            Some(&hdrs),
                            "n/a_api",
                            "HTTP error",
                        );
                        return Err(format!("HTTP {} from {}", code, url));
                    }
                }
                Err(e) => {
                    log_global_crawl_outcome(
                        subsys,
                        "GET",
                        url,
                        CrawlAuditOutcome::NetworkError,
                        0,
                        wait_ms,
                        t0.elapsed().as_millis() as u64,
                        0,
                        None,
                        "n/a_api",
                        &e.to_string(),
                    );
                    last_err = format!("Request error for {}: {}", url, e);
                }
            }
        }
        Err(last_err)
    }

    pub fn get_text_with_retry(&self, url: &str) -> Result<String, String> {
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                thread::sleep(Duration::from_millis(650 * (1 << attempt)));
            }
            let wait_ms = match acquire_domain_permit_blocking(url) {
                AcquirePermit::Granted { wait_ms, .. } => wait_ms,
                AcquirePermit::CircuitOpen { reason, .. } => {
                    log_global_crawl_outcome(
                        "search_api",
                        "GET",
                        url,
                        CrawlAuditOutcome::CircuitBreakerSkipped,
                        0,
                        0,
                        0,
                        0,
                        None,
                        "n/a_api",
                        &reason,
                    );
                    return Err(reason);
                }
            };
            let t0 = Instant::now();
            match self.client.get(url).send() {
                Ok(resp) => {
                    let status = resp.status();
                    let code = status.as_u16();
                    let hdrs = resp.headers().clone();
                    if status.is_success() {
                        let text = resp
                            .text()
                            .map_err(|e| format!("Text decode error from {}: {}", url, e))?;
                        log_global_crawl_outcome(
                            "search_api",
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
                            text.len() as u64,
                            Some(&hdrs),
                            "n/a_api",
                            "Atom XML feed",
                        );
                        return Ok(text);
                    } else if code == 429 || status.is_server_error() {
                        log_global_crawl_outcome(
                            "search_api",
                            "GET",
                            url,
                            if code == 429 {
                                CrawlAuditOutcome::Http429RateLimited
                            } else {
                                CrawlAuditOutcome::HttpOtherError
                            },
                            code,
                            wait_ms,
                            t0.elapsed().as_millis() as u64,
                            0,
                            Some(&hdrs),
                            "n/a_api",
                            "Retry backoff",
                        );
                        last_err = format!("HTTP {} from {}", code, url);
                        continue;
                    } else {
                        log_global_crawl_outcome(
                            "search_api",
                            "GET",
                            url,
                            CrawlAuditOutcome::HttpOtherError,
                            code,
                            wait_ms,
                            t0.elapsed().as_millis() as u64,
                            0,
                            Some(&hdrs),
                            "n/a_api",
                            "HTTP error",
                        );
                        return Err(format!("HTTP {} from {}", code, url));
                    }
                }
                Err(e) => {
                    log_global_crawl_outcome(
                        "search_api",
                        "GET",
                        url,
                        CrawlAuditOutcome::NetworkError,
                        0,
                        wait_ms,
                        t0.elapsed().as_millis() as u64,
                        0,
                        None,
                        "n/a_api",
                        &e.to_string(),
                    );
                    last_err = format!("Request error for {}: {}", url, e);
                }
            }
        }
        Err(last_err)
    }
}

pub fn execute_multi_provider_search(req: &SearchRequest) -> Result<SearchHarvestSummary, String> {
    let http = ScholarlyHttpClient::with_tokens(
        &req.polite_email,
        &req.crawl.openaire_token,
        &req.crawl.nasa_ads_token,
    )?;
    let mut raw_papers: Vec<PaperRecord> = Vec::new();
    let mut provider_counts: Vec<(String, usize)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    if req.providers.openalex {
        match search_openalex(&http, req) {
            Ok(mut list) => {
                provider_counts.push(("OpenAlex".to_string(), list.len()));
                if req.snowball_hops >= 1 && !list.is_empty() {
                    match snowball_openalex_references(&http, &list, 25) {
                        Ok(mut cited_neighbors) => {
                            list.append(&mut cited_neighbors);
                        }
                        Err(w) => warnings.push(format!("OpenAlex snowball warning: {}", w)),
                    }
                }
                raw_papers.append(&mut list);
            }
            Err(e) => warnings.push(format!("OpenAlex search unavailable: {}", e)),
        }
    }

    if req.providers.semantic_scholar {
        match search_semantic_scholar(&http, req) {
            Ok(mut list) => {
                provider_counts.push(("Semantic Scholar".to_string(), list.len()));
                raw_papers.append(&mut list);
            }
            Err(e) => warnings.push(format!("Semantic Scholar search unavailable: {}", e)),
        }
    }

    if req.providers.crossref {
        match search_crossref(&http, req) {
            Ok(mut list) => {
                provider_counts.push(("Crossref".to_string(), list.len()));
                raw_papers.append(&mut list);
            }
            Err(e) => warnings.push(format!("Crossref search unavailable: {}", e)),
        }
    }

    if req.providers.arxiv {
        match search_arxiv(&http, req) {
            Ok(mut list) => {
                provider_counts.push(("arXiv".to_string(), list.len()));
                raw_papers.append(&mut list);
            }
            Err(e) => warnings.push(format!("arXiv search unavailable: {}", e)),
        }
    }

    if req.providers.openaire {
        match search_openaire_graph(&http, req) {
            Ok(mut list) => {
                let _ = harvest_openaire_scholexplorer_links(&http, &mut list, 12);
                provider_counts.push(("OpenAIRE Graph".to_string(), list.len()));
                if req.snowball_hops >= 1 && !list.is_empty() {
                    match snowball_openaire_references(&http, &list, 20) {
                        Ok(mut neighbors) => list.append(&mut neighbors),
                        Err(w) => warnings.push(format!("OpenAIRE snowball warning: {}", w)),
                    }
                }
                raw_papers.append(&mut list);
            }
            Err(e) => warnings.push(format!("OpenAIRE Graph search unavailable: {}", e)),
        }
    }

    if req.providers.inspire_hep {
        match search_inspire_hep(&http, req) {
            Ok(mut list) => {
                provider_counts.push(("INSPIRE-HEP".to_string(), list.len()));
                if req.snowball_hops >= 1 && !list.is_empty() {
                    match snowball_inspire_hep(&http, &list, 20, req.crawl.filter_self_citations) {
                        Ok(mut neighbors) => list.append(&mut neighbors),
                        Err(w) => warnings.push(format!("INSPIRE-HEP snowball warning: {}", w)),
                    }
                }
                raw_papers.append(&mut list);
            }
            Err(e) => warnings.push(format!("INSPIRE-HEP search unavailable: {}", e)),
        }
    }

    if req.providers.nasa_ads {
        match search_nasa_ads(&http, req) {
            Ok(mut list) => {
                provider_counts.push(("NASA ADS".to_string(), list.len()));
                if req.snowball_hops >= 1 && !list.is_empty() {
                    match snowball_nasa_ads_bibcodes(&http, &list, 25) {
                        Ok(mut neighbors) => list.append(&mut neighbors),
                        Err(w) => warnings.push(format!("NASA ADS snowball warning: {}", w)),
                    }
                }
                raw_papers.append(&mut list);
            }
            Err(e) => warnings.push(format!("NASA ADS search unavailable: {}", e)),
        }
    }

    let any_crawler = req.providers.google_scholar
        || req.providers.pubmed
        || req.providers.europe_pmc
        || req.providers.dblp
        || req.providers.biorxiv_medrxiv
        || req.providers.datacite
        || !req.crawl.local_html_paths.is_empty()
        || req.crawl.hydrate_citation_edges;

    let mut maybe_crawler = if any_crawler {
        match PoliteWebCrawler::new(&req.polite_email, &req.crawl.proxy_url) {
            Ok(c) => Some(c),
            Err(e) => {
                warnings.push(format!("Crawler client warning: {}", e));
                None
            }
        }
    } else {
        None
    };

    if let Some(ref mut crawler) = maybe_crawler {
        if req.providers.google_scholar || !req.crawl.local_html_paths.is_empty() {
            match crawl_google_scholar(crawler, req) {
                Ok((mut list, mut gs_warns)) => {
                    provider_counts.push(("Google Scholar".to_string(), list.len()));
                    raw_papers.append(&mut list);
                    warnings.append(&mut gs_warns);
                }
                Err(e) => warnings.push(format!("Google Scholar crawl unavailable: {}", e)),
            }
        }

        if req.providers.pubmed {
            match crawl_pubmed_entrez(crawler, req) {
                Ok(mut list) => {
                    provider_counts.push(("PubMed".to_string(), list.len()));
                    raw_papers.append(&mut list);
                }
                Err(e) => warnings.push(format!("PubMed crawl unavailable: {}", e)),
            }
        }

        if req.providers.europe_pmc {
            match crawl_europe_pmc(crawler, req) {
                Ok(mut list) => {
                    provider_counts.push(("Europe PMC".to_string(), list.len()));
                    raw_papers.append(&mut list);
                }
                Err(e) => warnings.push(format!("Europe PMC crawl unavailable: {}", e)),
            }
        }

        if req.providers.dblp {
            match crawl_dblp(crawler, req) {
                Ok(mut list) => {
                    provider_counts.push(("DBLP".to_string(), list.len()));
                    raw_papers.append(&mut list);
                }
                Err(e) => warnings.push(format!("DBLP crawl unavailable: {}", e)),
            }
        }

        if req.providers.biorxiv_medrxiv {
            match crawl_biorxiv_medrxiv(crawler, req) {
                Ok(mut list) => {
                    provider_counts.push(("bioRxiv/medRxiv".to_string(), list.len()));
                    raw_papers.append(&mut list);
                }
                Err(e) => warnings.push(format!("bioRxiv/medRxiv crawl unavailable: {}", e)),
            }
        }

        if req.providers.datacite {
            match search_datacite_works(crawler, req) {
                Ok(mut list) => {
                    provider_counts.push(("DataCite".to_string(), list.len()));
                    raw_papers.append(&mut list);
                }
                Err(e) => warnings.push(format!("DataCite search unavailable: {}", e)),
            }
        }
    }

    if raw_papers.is_empty() && has_any_search_terms(req) {
        let mut offline_hits = search_offline_scholarly_index(req);
        if !offline_hits.is_empty() {
            provider_counts.push(("Offline Scholarly Index".to_string(), offline_hits.len()));
            raw_papers.append(&mut offline_hits);
            warnings.push(
                "Remote providers returned 0 hits (offline or filtered); populated instant matches from Offline Scholarly Index."
                    .to_string(),
            );
        }
    }

    let (mut papers, mut edges) =
        merge_and_link_records(raw_papers, req.oa_only, req.year_min, req.year_max);
    papers.retain(|p| passes_pop_exclude_filter(p, &req.exclude_words));

    if req.crawl.hydrate_citation_edges
        && !papers.is_empty()
        && (req.providers.google_scholar
            || req.providers.pubmed
            || req.providers.europe_pmc
            || req.providers.dblp
            || req.providers.biorxiv_medrxiv
            || req.providers.openaire
            || req.providers.inspire_hep)
    {
        if let Some(ref crawler) = maybe_crawler {
            let (hydrated_edges, mut h_warns) = hydrate_opencitations_and_icite(
                crawler,
                &mut papers,
                req.crawl.filter_self_citations,
            );
            for he in hydrated_edges {
                if !edges.iter().any(|e| e.source == he.source && e.target == he.target) {
                    edges.push(he);
                }
            }
            warnings.append(&mut h_warns);
        }
    }

    if !papers.is_empty()
        && (req.crawl.screen_retractions
            || req.crawl.track_datacite_bridges
            || req.crawl.track_epmc_annotations
            || req.crawl.track_uniprot_pdb_bridges
            || !req.crawl.retraction_watch_csv_path.trim().is_empty())
    {
        let rw_opt = if req.crawl.retraction_watch_csv_path.trim().is_empty() {
            None
        } else {
            Some(std::path::Path::new(req.crawl.retraction_watch_csv_path.trim()))
        };
        match run_full_corpus_screening(
            &mut papers,
            &mut edges,
            &req.polite_email,
            &req.crawl.proxy_url,
            rw_opt,
            req.crawl.screen_retractions,
            req.crawl.track_datacite_bridges,
            req.crawl.track_epmc_annotations,
            req.crawl.track_uniprot_pdb_bridges,
            true,
        ) {
            Ok(scr) => {
                warnings.extend(scr.warnings);
            }
            Err(e) => warnings.push(format!("Integrity/DataCite screening warning: {}", e)),
        }
    }

    Ok(SearchHarvestSummary {
        papers,
        edges,
        provider_counts,
        warnings,
    })
}

pub fn lookup_single_doi(doi: &str, polite_email: &str) -> Result<PaperRecord, String> {
    let clean = normalize_doi(doi);
    if clean.is_empty() {
        return Err("Empty DOI supplied".to_string());
    }
    let http = ScholarlyHttpClient::new(polite_email)?;
    let mut url = format!(
        "https://api.openalex.org/works/https://doi.org/{}?mailto={}",
        percent_encode(&clean),
        percent_encode(http.polite_email())
    );
    if let Some(ref k) = http.openalex_api_key {
        url.push_str("&api_key=");
        url.push_str(&percent_encode(k));
    }
    if let Ok(val) = http.get_json_with_retry(&url, false) {
        if let Some(rec) = parse_openalex_work(&val, 0) {
            return Ok(rec);
        }
    }

    let cr_url = format!(
        "https://api.crossref.org/works/{}?mailto={}",
        percent_encode(&clean),
        percent_encode(http.polite_email())
    );
    let val = http.get_json_with_retry(&cr_url, false)?;
    let msg = val.get("message").ok_or("Missing message in Crossref DOI response")?;
    parse_crossref_item(msg, 0).ok_or_else(|| "Could not parse Crossref DOI metadata".to_string())
}

// ============================================================================
// PROVIDER 1: OPENALEX (https://api.openalex.org/works)
// ============================================================================
fn search_openalex(http: &ScholarlyHttpClient, req: &SearchRequest) -> Result<Vec<PaperRecord>, String> {
    let url = build_openalex_query_url(req, http.polite_email(), http.openalex_api_key.as_deref());

    let json = http.get_json_with_retry(&url, false)?;
    let results = json
        .get("results")
        .and_then(|v| v.as_array())
        .ok_or("OpenAlex JSON missing results array")?;

    let mut out = Vec::with_capacity(results.len());
    for (idx, item) in results.iter().enumerate() {
        if let Some(rec) = parse_openalex_work(item, idx) {
            out.push(rec);
        }
    }
    Ok(out)
}

fn snowball_openalex_references(
    http: &ScholarlyHttpClient,
    seeds: &[PaperRecord],
    max_refs: usize,
) -> Result<Vec<PaperRecord>, String> {
    let existing_oa: HashSet<&str> = seeds
        .iter()
        .map(|s| s.openalex_id.as_str())
        .filter(|s| !s.is_empty())
        .collect();

    let mut ref_counts: HashMap<String, usize> = HashMap::new();
    for s in seeds {
        for r in &s.raw_references {
            if let Some(short_w) = r.strip_prefix("OA:") {
                if !existing_oa.contains(short_w) && short_w.starts_with('W') {
                    *ref_counts.entry(short_w.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    if ref_counts.is_empty() {
        return Ok(Vec::new());
    }

    let mut ranked: Vec<(String, usize)> = ref_counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let batch: Vec<String> = ranked.into_iter().take(max_refs.min(40)).map(|x| x.0).collect();

    let mut url = format!(
        "https://api.openalex.org/works?filter=openalex_id:{}&per-page={}&mailto={}",
        batch.join("|"),
        batch.len(),
        percent_encode(http.polite_email())
    );
    if let Some(ref k) = http.openalex_api_key {
        url.push_str("&api_key=");
        url.push_str(&percent_encode(k));
    }

    let json = http.get_json_with_retry(&url, false)?;
    let arr = json
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(arr
        .iter()
        .enumerate()
        .filter_map(|(i, w)| parse_openalex_work(w, 1000 + i))
        .collect())
}

fn parse_openalex_work(w: &Value, idx: usize) -> Option<PaperRecord> {
    let full_oa_id = w.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let oa_short = full_oa_id.rsplit('/').next().unwrap_or("").to_string();
    let title = w
        .get("display_name")
        .or_else(|| w.get("title"))
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let year = w
        .get("publication_year")
        .and_then(|v| v.as_i64())
        .unwrap_or(2020) as i32;

    let authors = w
        .get("authorships")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let names: Vec<&str> = arr
                .iter()
                .take(4)
                .filter_map(|a| a.pointer("/author/display_name").and_then(|n| n.as_str()))
                .collect();
            if names.is_empty() {
                "OpenAlex Author".to_string()
            } else if arr.len() > 4 {
                format!("{} et al.", names.join(", "))
            } else {
                names.join(", ")
            }
        })
        .unwrap_or_else(|| "OpenAlex Author".to_string());

    let venue = w
        .pointer("/primary_location/source/display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("OpenAlex Indexed Venue")
        .to_string();

    let doi = w
        .get("doi")
        .and_then(|v| v.as_str())
        .map(normalize_doi)
        .unwrap_or_default();

    let pmcid = w
        .pointer("/ids/pmcid")
        .and_then(|v| v.as_str())
        .map(|s| s.rsplit('/').next().unwrap_or(s).to_uppercase())
        .unwrap_or_default();

    let oa_status = w
        .pointer("/open_access/oa_status")
        .and_then(|v| v.as_str())
        .map(OaStatus::from_str_lossy)
        .unwrap_or(OaStatus::Unknown);

    let oa_pdf_url = w
        .pointer("/best_oa_location/pdf_url")
        .and_then(|v| v.as_str())
        .or_else(|| w.pointer("/primary_location/pdf_url").and_then(|v| v.as_str()))
        .or_else(|| w.pointer("/open_access/oa_url").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let cited_by = w.get("cited_by_count").and_then(|v| v.as_u64());
    let abstract_text = w
        .get("abstract_inverted_index")
        .map(reconstruct_openalex_abstract)
        .unwrap_or_default();

    let mut topic_terms: Vec<String> = Vec::new();
    if let Some(pt) = w.pointer("/primary_topic/display_name").and_then(|v| v.as_str()) {
        topic_terms.push(pt.to_string());
    }
    if let Some(kws) = w.get("keywords").and_then(|v| v.as_array()) {
        for kw in kws.iter().take(6) {
            if let Some(dn) = kw.get("display_name").and_then(|v| v.as_str()) {
                topic_terms.push(dn.to_string());
            }
        }
    }

    let combined_text = format!("{} {} {} {}", title, venue, topic_terms.join(" "), abstract_text);
    let disc = Discipline::infer_from_text(&combined_text);
    let concepts = infer_concepts_from_text(&combined_text, disc);

    let mut raw_refs = Vec::new();
    if let Some(refs) = w.get("referenced_works").and_then(|v| v.as_array()) {
        for r in refs {
            if let Some(rs) = r.as_str() {
                let short_r = rs.rsplit('/').next().unwrap_or(rs);
                if !short_r.is_empty() {
                    raw_refs.push(format!("OA:{}", short_r));
                }
            }
        }
    }

    let source_url = if !doi.is_empty() {
        format!("https://doi.org/{}", doi)
    } else if !full_oa_id.is_empty() {
        full_oa_id.to_string()
    } else {
        "https://openalex.org".to_string()
    };

    let id = if !oa_short.is_empty() {
        format!("oa_{}", oa_short.to_ascii_lowercase())
    } else {
        format!("oa_{}", idx)
    };

    Some(PaperRecord {
        id,
        title,
        authors,
        year,
        venue,
        disc,
        doi,
        arxiv_id: String::new(),
        openalex_id: oa_short,
        s2_id: String::new(),
        pmcid,
        source_url,
        oa_pdf_url,
        oa_status,
        external_citations: cited_by,
        abstract_text,
        terms: if topic_terms.is_empty() {
            concepts.clone()
        } else {
            topic_terms
        },
        concepts,
        local_pdf_path: None,
        pdf_sha256: None,
        pdf_bytes: None,
        raw_references: raw_refs,
        retraction: if w.get("is_retracted").and_then(|v| v.as_bool()).unwrap_or(false) {
            RetractionRecord {
                status: RetractionStatus::Retracted,
                source: "openalex".to_string(),
                ..Default::default()
            }
        } else {
            RetractionRecord::default()
        },
        datacite_links: Vec::new(),
        bio_entities: Vec::new(),
    })
}

pub fn reconstruct_openalex_abstract(inv: &Value) -> String {
    let Some(obj) = inv.as_object() else {
        return String::new();
    };
    let mut max_pos = 0usize;
    let mut positioned: Vec<(usize, &str)> = Vec::new();
    for (word, positions) in obj {
        if let Some(arr) = positions.as_array() {
            for p in arr {
                if let Some(idx) = p.as_u64() {
                    let u = idx as usize;
                    if u < 4000 {
                        max_pos = max_pos.max(u);
                        positioned.push((u, word.as_str()));
                    }
                }
            }
        }
    }
    if positioned.is_empty() {
        return String::new();
    }
    let mut words = vec![""; max_pos + 1];
    for (u, w) in positioned {
        if u < words.len() {
            words[u] = w;
        }
    }
    words
        .into_iter()
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ============================================================================
// PROVIDER 2: SEMANTIC SCHOLAR (https://api.semanticscholar.org/graph/v1)
// ============================================================================
fn search_semantic_scholar(
    http: &ScholarlyHttpClient,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let url = build_semantic_scholar_query_url(req);

    let json = http.get_json_with_retry(&url, true)?;
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("Semantic Scholar response missing data array")?;

    let mut out = Vec::with_capacity(data.len());
    for (idx, item) in data.iter().enumerate() {
        let Some(title) = item.get("title").and_then(|v| v.as_str()) else {
            continue;
        };
        let s2_id = item
            .get("paperId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let year = item.get("year").and_then(|v| v.as_i64()).unwrap_or(2020) as i32;
        let venue = item
            .get("venue")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("Semantic Scholar Indexed")
            .to_string();

        let authors = item
            .get("authors")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let names: Vec<&str> = arr
                    .iter()
                    .take(4)
                    .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                    .collect();
                if names.is_empty() {
                    "S2 Author".to_string()
                } else if arr.len() > 4 {
                    format!("{} et al.", names.join(", "))
                } else {
                    names.join(", ")
                }
            })
            .unwrap_or_else(|| "S2 Author".to_string());

        let doi = item
            .pointer("/externalIds/DOI")
            .and_then(|v| v.as_str())
            .map(normalize_doi)
            .unwrap_or_default();
        let arxiv_id = item
            .pointer("/externalIds/ArXiv")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let pmcid = item
            .pointer("/externalIds/PubMedCentral")
            .and_then(|v| v.as_str())
            .map(|s| {
                if s.starts_with("PMC") {
                    s.to_string()
                } else {
                    format!("PMC{}", s)
                }
            })
            .unwrap_or_default();

        let oa_pdf_url = item
            .pointer("/openAccessPdf/url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let oa_status = if !oa_pdf_url.is_empty() {
            OaStatus::Green
        } else if !arxiv_id.is_empty() {
            OaStatus::Arxiv
        } else {
            OaStatus::Unknown
        };

        let abstract_text = item
            .get("abstract")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cited_by = item.get("citationCount").and_then(|v| v.as_u64());

        let mut fields = Vec::new();
        if let Some(arr) = item.get("fieldsOfStudy").and_then(|v| v.as_array()) {
            for f in arr {
                if let Some(fs) = f.as_str() {
                    fields.push(fs.to_string());
                }
            }
        }
        let combined = format!("{} {} {} {}", title, venue, fields.join(" "), abstract_text);
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);

        let mut raw_refs = Vec::new();
        if let Some(refs) = item.get("references").and_then(|v| v.as_array()) {
            for r in refs {
                if let Some(pid) = r.get("paperId").and_then(|v| v.as_str()) {
                    if !pid.is_empty() {
                        raw_refs.push(format!("S2:{}", pid));
                    }
                }
            }
        }

        let source_url = if !doi.is_empty() {
            format!("https://doi.org/{}", doi)
        } else if !s2_id.is_empty() {
            format!("https://www.semanticscholar.org/paper/{}", s2_id)
        } else {
            "https://www.semanticscholar.org".to_string()
        };

        out.push(PaperRecord {
            id: if !s2_id.is_empty() {
                format!("s2_{}", &s2_id[..s2_id.len().min(12)])
            } else {
                format!("s2_{}", idx)
            },
            title: title.to_string(),
            authors,
            year,
            venue,
            disc,
            doi,
            arxiv_id,
            openalex_id: String::new(),
            s2_id,
            pmcid,
            source_url,
            oa_pdf_url,
            oa_status,
            external_citations: cited_by,
            abstract_text,
            terms: if fields.is_empty() { concepts.clone() } else { fields },
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: raw_refs,
            retraction: RetractionRecord::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }
    Ok(out)
}

// ============================================================================
// PROVIDER 3: CROSSREF REST API (https://api.crossref.org/works)
// ============================================================================
fn search_crossref(
    http: &ScholarlyHttpClient,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let url = build_crossref_query_url(req, http.polite_email());

    let json = http.get_json_with_retry(&url, false)?;
    let items = json
        .pointer("/message/items")
        .and_then(|v| v.as_array())
        .ok_or("Crossref response missing message.items")?;

    Ok(items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| parse_crossref_item(item, i))
        .collect())
}

fn parse_crossref_item(item: &Value, idx: usize) -> Option<PaperRecord> {
    let title = item
        .get("title")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let doi = item
        .get("DOI")
        .and_then(|v| v.as_str())
        .map(normalize_doi)
        .unwrap_or_default();

    let year = item
        .pointer("/published/date-parts/0/0")
        .and_then(|v| v.as_i64())
        .unwrap_or(2020) as i32;

    let authors = item
        .get("author")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let names: Vec<String> = arr
                .iter()
                .take(4)
                .filter_map(|a| {
                    let fam = a.get("family").and_then(|v| v.as_str()).unwrap_or("");
                    let giv = a.get("given").and_then(|v| v.as_str()).unwrap_or("");
                    if fam.is_empty() {
                        None
                    } else if giv.is_empty() {
                        Some(fam.to_string())
                    } else {
                        Some(format!("{} {}", giv, fam))
                    }
                })
                .collect();
            if names.is_empty() {
                "Crossref Author".to_string()
            } else if arr.len() > 4 {
                format!("{} et al.", names.join(", "))
            } else {
                names.join(", ")
            }
        })
        .unwrap_or_else(|| "Crossref Author".to_string());

    let venue = item
        .get("container-title")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("Crossref Indexed Journal")
        .to_string();

    let cited_by = item.get("is-referenced-by-count").and_then(|v| v.as_u64());

    let mut oa_pdf_url = String::new();
    if let Some(links) = item.get("link").and_then(|v| v.as_array()) {
        for lk in links {
            let ctype = lk
                .get("content-type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if ctype.contains("pdf") {
                if let Some(u) = lk.get("URL").and_then(|v| v.as_str()) {
                    oa_pdf_url = u.to_string();
                    break;
                }
            }
        }
    }

    let mut subjects = Vec::new();
    if let Some(subs) = item.get("subject").and_then(|v| v.as_array()) {
        for s in subs.iter().take(5) {
            if let Some(st) = s.as_str() {
                subjects.push(st.to_string());
            }
        }
    }

    let mut raw_refs = Vec::new();
    if let Some(refs) = item.get("reference").and_then(|v| v.as_array()) {
        for r in refs {
            if let Some(rdoi) = r.get("DOI").and_then(|v| v.as_str()) {
                let norm = normalize_doi(rdoi);
                if !norm.is_empty() {
                    raw_refs.push(format!("DOI:{}", norm));
                }
            }
        }
    }

    let combined = format!("{} {} {}", title, venue, subjects.join(" "));
    let disc = Discipline::infer_from_text(&combined);
    let concepts = infer_concepts_from_text(&combined, disc);

    Some(PaperRecord {
        id: if !doi.is_empty() {
            format!(
                "cr_{}",
                doi.chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .take(16)
                    .collect::<String>()
            )
        } else {
            format!("cr_{}", idx)
        },
        title,
        authors,
        year,
        venue,
        disc,
        source_url: if !doi.is_empty() {
            format!("https://doi.org/{}", doi)
        } else {
            "https://www.crossref.org".to_string()
        },
        doi,
        arxiv_id: String::new(),
        openalex_id: String::new(),
        s2_id: String::new(),
        pmcid: String::new(),
        oa_pdf_url,
        oa_status: OaStatus::Unknown,
        external_citations: cited_by,
        abstract_text: String::new(),
        terms: if subjects.is_empty() { concepts.clone() } else { subjects },
        concepts,
        local_pdf_path: None,
        pdf_sha256: None,
        pdf_bytes: None,
        raw_references: raw_refs,
        retraction: parse_crossref_work_retraction(item),
        datacite_links: Vec::new(),
        bio_entities: Vec::new(),
    })
}

// ============================================================================
// PROVIDER 4: ARXIV ATOM API (https://export.arxiv.org/api/query)
// ============================================================================
fn search_arxiv(
    http: &ScholarlyHttpClient,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let url = build_arxiv_query_url(req);
    let xml = http.get_text_with_retry(&url)?;
    Ok(parse_arxiv_atom_feed(&xml))
}

pub fn parse_arxiv_atom_feed(xml: &str) -> Vec<PaperRecord> {
    let mut out = Vec::new();
    for (idx, chunk) in xml.split("<entry>").skip(1).enumerate() {
        let Some(entry) = chunk.split("</entry>").next() else {
            continue;
        };
        let raw_id = extract_xml_tag(entry, "id");
        let arxiv_id_ver = raw_id
            .split("/abs/")
            .nth(1)
            .unwrap_or("")
            .trim()
            .to_string();
        let arxiv_id = arxiv_id_ver
            .rsplit_once('v')
            .and_then(|(base, ver)| {
                if ver.chars().all(|c| c.is_ascii_digit()) {
                    Some(base.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| arxiv_id_ver.clone());

        let title = collapse_ws(&extract_xml_tag(entry, "title"));
        if title.is_empty() {
            continue;
        }
        let summary = collapse_ws(&extract_xml_tag(entry, "summary"));
        let published = extract_xml_tag(entry, "published");
        let year = published
            .get(0..4)
            .and_then(|y| y.parse::<i32>().ok())
            .unwrap_or(2024);

        let mut author_names = Vec::new();
        for a_chunk in entry.split("<author>").skip(1) {
            if let Some(a_body) = a_chunk.split("</author>").next() {
                let nm = collapse_ws(&extract_xml_tag(a_body, "name"));
                if !nm.is_empty() {
                    author_names.push(nm);
                }
            }
        }
        let authors = if author_names.is_empty() {
            "arXiv Author".to_string()
        } else if author_names.len() > 4 {
            format!("{} et al.", author_names[..4].join(", "))
        } else {
            author_names.join(", ")
        };

        let doi = normalize_doi(&extract_xml_tag(entry, "arxiv:doi"));
        let pdf_url = if !arxiv_id.is_empty() {
            format!("https://arxiv.org/pdf/{}.pdf", arxiv_id)
        } else {
            String::new()
        };
        let abs_url = if !arxiv_id.is_empty() {
            format!("https://arxiv.org/abs/{}", arxiv_id)
        } else {
            "https://arxiv.org".to_string()
        };

        let combined = format!("{} {}", title, summary);
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);

        out.push(PaperRecord {
            id: if !arxiv_id.is_empty() {
                format!(
                    "arx_{}",
                    arxiv_id
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .collect::<String>()
                )
            } else {
                format!("arx_{}", idx)
            },
            title,
            authors,
            year,
            venue: format!("arXiv Preprint ({})", arxiv_id),
            disc,
            doi,
            arxiv_id,
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            source_url: abs_url,
            oa_pdf_url: pdf_url,
            oa_status: OaStatus::Arxiv,
            external_citations: None,
            abstract_text: summary,
            terms: concepts.clone(),
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

fn extract_xml_tag(src: &str, tag: &str) -> String {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let Some(start_tag) = src.find(&open) else {
        return String::new();
    };
    let rest = &src[start_tag + open.len()..];
    let Some(gt) = rest.find('>') else {
        return String::new();
    };
    let inner = &rest[gt + 1..];
    let Some(end_idx) = inner.find(&close) else {
        return String::new();
    };
    inner[..end_idx]
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ============================================================================
// PROVIDER 5: OPENAIRE GRAPH v2/v3 & SCHOLEXPLORER v3 CITATION LINKS
// ============================================================================
fn search_openaire_graph(
    http: &ScholarlyHttpClient,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let url = build_openaire_query_url(req);

    let json = http.get_json_with_bearer(&url, http.openaire_token(), "search_api")?;
    let items = json
        .get("results")
        .and_then(|v| v.as_array())
        .or_else(|| json.pointer("/response/results").and_then(|v| v.as_array()))
        .ok_or("OpenAIRE Graph JSON missing results array")?;

    let mut out = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        if let Some(rec) = parse_openaire_research_product(item, idx) {
            out.push(rec);
        }
    }
    Ok(out)
}

pub fn parse_openaire_research_product(item: &Value, idx: usize) -> Option<PaperRecord> {
    let title = item
        .get("mainTitle")
        .or_else(|| item.get("title"))
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let openaire_id = item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let year = item
        .get("publicationDate")
        .and_then(|v| v.as_str())
        .and_then(|s| s.get(0..4))
        .and_then(|y| y.parse::<i32>().ok())
        .unwrap_or(2023);

    let authors = item
        .get("authors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let names: Vec<String> = arr
                .iter()
                .take(4)
                .filter_map(|a| {
                    a.get("fullName")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .or_else(|| {
                            let given = a.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                            let sur = a.get("surname").and_then(|v| v.as_str()).unwrap_or("").trim();
                            let comb = format!("{} {}", given, sur).trim().to_string();
                            if comb.is_empty() { None } else { Some(comb) }
                        })
                })
                .filter(|s| !s.is_empty())
                .collect();
            if names.is_empty() {
                "OpenAIRE Author".to_string()
            } else if arr.len() > 4 {
                format!("{} et al.", names.join(", "))
            } else {
                names.join(", ")
            }
        })
        .unwrap_or_else(|| "OpenAIRE Author".to_string());

    let mut doi = String::new();
    let mut arxiv_id = String::new();
    let mut pmcid = String::new();
    let mut pmid_tag = String::new();
    if let Some(pids) = item.get("pids").and_then(|v| v.as_array()) {
        for p in pids {
            let scheme = p
                .get("scheme")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let val = p.get("value").and_then(|v| v.as_str()).unwrap_or("").trim();
            if val.is_empty() {
                continue;
            }
            match scheme.as_str() {
                "doi" if doi.is_empty() => doi = normalize_doi(val),
                "arxiv" if arxiv_id.is_empty() => {
                    arxiv_id = val.trim_start_matches("arXiv:").to_string();
                }
                "pmc" | "pmcid" if pmcid.is_empty() => {
                    pmcid = if val.to_ascii_uppercase().starts_with("PMC") {
                        val.to_ascii_uppercase()
                    } else {
                        format!("PMC{}", val)
                    };
                }
                "pmid" if pmid_tag.is_empty() => {
                    pmid_tag = format!("pmid:{}", val);
                }
                _ => {}
            }
        }
    }

    let venue = item
        .pointer("/container/name")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("publisher").and_then(|v| v.as_str()))
        .unwrap_or("OpenAIRE Indexed Repository")
        .trim()
        .to_string();

    let access_label = item
        .pointer("/bestAccessRight/label")
        .or_else(|| item.pointer("/bestAccessRight/code"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let oa_status = if access_label.contains("open") {
        OaStatus::Green
    } else if access_label.contains("closed") || access_label.contains("restricted") {
        OaStatus::Closed
    } else {
        OaStatus::Unknown
    };

    let mut oa_pdf_url = String::new();
    let mut landing_url = String::new();
    if let Some(instances) = item.get("instances").and_then(|v| v.as_array()) {
        for inst in instances {
            let inst_oa = inst
                .pointer("/accessRight/label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if let Some(urls) = inst.get("urls").and_then(|v| v.as_array()) {
                for u in urls.iter().filter_map(|x| x.as_str()) {
                    if landing_url.is_empty() && u.starts_with("http") {
                        landing_url = u.to_string();
                    }
                    if oa_pdf_url.is_empty()
                        && u.starts_with("http")
                        && (u.to_ascii_lowercase().ends_with(".pdf")
                            || u.contains("/pdf/")
                            || inst_oa.contains("open"))
                    {
                        if u.to_ascii_lowercase().contains("pdf") {
                            oa_pdf_url = u.to_string();
                        }
                    }
                }
            }
        }
    }
    if oa_pdf_url.is_empty() && !arxiv_id.is_empty() {
        oa_pdf_url = format!("https://arxiv.org/pdf/{}.pdf", arxiv_id);
    }

    let cited_by = item
        .pointer("/indicators/citationImpact/citationCount")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)));

    let abstract_text = item
        .get("descriptions")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(collapse_ws)
        .unwrap_or_default();

    let mut terms: Vec<String> = Vec::new();
    if let Some(subjs) = item.get("subjects").and_then(|v| v.as_array()) {
        for s in subjs.iter().take(6) {
            let sv = s
                .pointer("/subject/value")
                .or_else(|| s.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if !sv.is_empty() && !terms.iter().any(|x| x == sv) {
                terms.push(sv.to_string());
            }
        }
    }
    if !openaire_id.is_empty() {
        terms.push(format!("openaire_id:{}", openaire_id));
    }
    if !pmid_tag.is_empty() {
        terms.push(pmid_tag);
    }

    let combined = format!("{} {} {} {}", title, venue, terms.join(" "), abstract_text);
    let disc = Discipline::infer_from_text(&combined);
    let concepts = infer_concepts_from_text(&combined, disc);
    if terms.is_empty() {
        terms = concepts.clone();
    }

    let source_url = if !doi.is_empty() {
        format!("https://doi.org/{}", doi)
    } else if !landing_url.is_empty() {
        landing_url
    } else if !openaire_id.is_empty() {
        format!("https://explore.openaire.eu/search/publication?articleId={}", openaire_id)
    } else {
        "https://explore.openaire.eu".to_string()
    };

    let slug: String = if !doi.is_empty() {
        doi.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect()
    } else if !openaire_id.is_empty() {
        openaire_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .rev()
            .take(14)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        format!("{}", idx)
    };

    Some(PaperRecord {
        id: format!("oaire_{}", slug.to_ascii_lowercase()),
        title,
        authors,
        year,
        venue,
        disc,
        doi,
        arxiv_id,
        openalex_id: String::new(),
        s2_id: String::new(),
        pmcid,
        source_url,
        oa_pdf_url,
        oa_status,
        external_citations: cited_by,
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
    })
}

/// Parses OpenAIRE ScholExplorer v3 (`https://api.scholexplorer.openaire.eu/v3/Links`) JSON payload,
/// extracting outgoing/incoming publication references (`DOI:...`, `CITED_BY_DOI:...`) and linked
/// DataCite datasets/software (`DataCiteLink`).
pub fn parse_scholexplorer_links_json(
    val: &Value,
    seed_doi: &str,
) -> (Vec<String>, Vec<DataCiteLink>) {
    let norm_seed = normalize_doi(seed_doi);
    let mut raw_refs = Vec::new();
    let mut dc_links = Vec::new();

    let Some(arr) = val
        .get("result")
        .or_else(|| val.get("results"))
        .and_then(|v| v.as_array())
    else {
        return (raw_refs, dc_links);
    };

    for row in arr {
        let rel_name = row
            .pointer("/RelationshipType/Name")
            .or_else(|| row.pointer("/RelationshipType/SubType"))
            .and_then(|v| v.as_str())
            .unwrap_or("References")
            .trim()
            .to_string();

        let extract_doi = |node: Option<&Value>| -> String {
            let Some(n) = node else {
                return String::new();
            };
            if let Some(ids) = n.get("Identifier").and_then(|v| v.as_array()) {
                for id_obj in ids {
                    let sch = id_obj
                        .get("IDScheme")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let raw = id_obj.get("ID").and_then(|v| v.as_str()).unwrap_or("");
                    if sch == "doi" || raw.contains("10.") {
                        let d = normalize_doi(raw);
                        if !d.is_empty() {
                            return d;
                        }
                    }
                }
            }
            String::new()
        };

        let src_node = row.get("Source");
        let tgt_node = row.get("Target");
        let src_doi = extract_doi(src_node);
        let tgt_doi = extract_doi(tgt_node);

        let peer_node = if src_doi == norm_seed {
            tgt_node
        } else if tgt_doi == norm_seed {
            src_node
        } else {
            tgt_node
        };
        let peer_doi = if src_doi == norm_seed {
            tgt_doi.clone()
        } else if tgt_doi == norm_seed {
            src_doi.clone()
        } else {
            tgt_doi.clone()
        };
        if peer_doi.is_empty() || peer_doi == norm_seed {
            continue;
        }

        let peer_type = peer_node
            .and_then(|n| n.pointer("/Type/Name").or_else(|| n.get("Type")))
            .and_then(|v| v.as_str())
            .unwrap_or("literature")
            .to_ascii_lowercase();

        if peer_type.contains("dataset") || peer_type.contains("software") {
            let kind = if peer_type.contains("software") {
                DataCiteArtifactKind::Software
            } else {
                DataCiteArtifactKind::Dataset
            };
            let art_title = peer_node
                .and_then(|n| n.get("Title"))
                .and_then(|v| v.as_str())
                .unwrap_or("OpenAIRE ScholExplorer Linked Artifact")
                .trim()
                .to_string();
            let repo = peer_node
                .and_then(|n| n.get("Publisher"))
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|p| p.get("name").and_then(|v| v.as_str()))
                .unwrap_or("OpenAIRE ScholExplorer")
                .to_string();
            if !dc_links.iter().any(|d: &DataCiteLink| d.artifact_doi == peer_doi) {
                dc_links.push(DataCiteLink {
                    artifact_doi: peer_doi,
                    artifact_title: art_title,
                    artifact_kind: kind,
                    repository: repo,
                    relation_type: rel_name,
                    citation_count: 0,
                    view_count: 0,
                    download_count: 0,
                });
            }
        } else {
            let rel_low = rel_name.to_ascii_lowercase();
            let tag = if (src_doi == norm_seed && rel_low.contains("isreferencedby"))
                || (tgt_doi == norm_seed && (rel_low.contains("references") || rel_low.contains("cites")))
            {
                format!("CITED_BY_DOI:{}", peer_doi)
            } else {
                format!("DOI:{}", peer_doi)
            };
            if !raw_refs.contains(&tag) {
                raw_refs.push(tag);
            }
        }
    }

    (raw_refs, dc_links)
}

fn harvest_openaire_scholexplorer_links(
    http: &ScholarlyHttpClient,
    papers: &mut [PaperRecord],
    max_seeds: usize,
) -> Result<usize, String> {
    let mut linked = 0usize;
    for p in papers.iter_mut().filter(|p| !p.doi.is_empty()).take(max_seeds) {
        let url = format!(
            "https://api.scholexplorer.openaire.eu/v3/Links?sourcePid={}&page=0",
            percent_encode(&p.doi)
        );
        if let Ok(val) = http.get_json_with_bearer(&url, None, "search_api") {
            let (refs, dcs) = parse_scholexplorer_links_json(&val, &p.doi);
            for r in refs {
                if !p.raw_references.contains(&r) {
                    p.raw_references.push(r);
                    linked += 1;
                }
            }
            for dl in dcs {
                if !p.datacite_links.iter().any(|x| x.artifact_doi == dl.artifact_doi) {
                    p.datacite_links.push(dl);
                }
            }
        }
    }
    Ok(linked)
}

fn snowball_openaire_references(
    http: &ScholarlyHttpClient,
    seeds: &[PaperRecord],
    max_refs: usize,
) -> Result<Vec<PaperRecord>, String> {
    let existing_dois: HashSet<&str> = seeds
        .iter()
        .map(|s| s.doi.as_str())
        .filter(|d| !d.is_empty())
        .collect();
    let mut candidate_dois: Vec<String> = Vec::new();
    for s in seeds {
        for r in &s.raw_references {
            let d_opt = r
                .strip_prefix("DOI:")
                .or_else(|| r.strip_prefix("CITED_BY_DOI:"));
            if let Some(d) = d_opt {
                if !existing_dois.contains(d) && !candidate_dois.iter().any(|x| x == d) {
                    candidate_dois.push(d.to_string());
                    if candidate_dois.len() >= max_refs.min(20) {
                        break;
                    }
                }
            }
        }
        if candidate_dois.len() >= max_refs.min(20) {
            break;
        }
    }
    if candidate_dois.is_empty() {
        return Ok(Vec::new());
    }
    let joined = candidate_dois
        .iter()
        .map(|d| percent_encode(d))
        .collect::<Vec<_>>()
        .join(",");
    let url = format!(
        "https://api.openaire.eu/graph/v2/researchProducts?pid={}&pageSize={}",
        joined,
        candidate_dois.len()
    );
    let json = http.get_json_with_bearer(&url, http.openaire_token(), "search_api")?;
    let arr = json
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .iter()
        .enumerate()
        .filter_map(|(i, item)| parse_openaire_research_product(item, 2000 + i))
        .collect())
}

// ============================================================================
// PROVIDER 6: INSPIRE-HEP REST API (https://inspirehep.net/api/literature)
// ============================================================================
const INSPIRE_FIELDS: &str = "titles,authors.full_name,publication_info,earliest_date,preprint_date,citation_count,citation_count_without_self_citations,dois,arxiv_eprints,abstracts,inspire_categories,collaborations,documents,references,control_number";

fn search_inspire_hep(
    http: &ScholarlyHttpClient,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let size = req.max_per_provider.clamp(1, 100);
    let q_str = build_inspire_hep_query(req);
    let url = format!(
        "https://inspirehep.net/api/literature?q={}&size={}&page=1&sort=mostrecent&fields={}",
        percent_encode(&q_str),
        size,
        INSPIRE_FIELDS
    );
    let json = http.get_json_with_bearer(&url, None, "search_api")?;
    let hits = json
        .pointer("/hits/hits")
        .and_then(|v| v.as_array())
        .ok_or("INSPIRE-HEP JSON missing hits.hits array")?;

    let mut out = Vec::with_capacity(hits.len());
    for (idx, hit) in hits.iter().enumerate() {
        if let Some(rec) = parse_inspire_literature_hit(hit, idx, req.crawl.filter_self_citations) {
            out.push(rec);
        }
    }
    Ok(out)
}

pub fn parse_inspire_literature_hit(
    hit: &Value,
    idx: usize,
    filter_self_citations: bool,
) -> Option<PaperRecord> {
    let meta = hit.get("metadata").unwrap_or(hit);
    let title = meta
        .get("titles")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find_map(|t| t.get("title").and_then(|v| v.as_str())))?
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let recid = meta
        .get("control_number")
        .and_then(|v| {
            v.as_u64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        })
        .or_else(|| hit.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();

    let year = meta
        .get("earliest_date")
        .or_else(|| meta.get("preprint_date"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.get(0..4))
        .and_then(|y| y.parse::<i32>().ok())
        .or_else(|| {
            meta.get("publication_info")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.iter().find_map(|p| p.get("year").and_then(|y| y.as_i64())))
                .map(|y| y as i32)
        })
        .unwrap_or(2023);

    let collab = meta
        .get("collaborations")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("value").and_then(|v| v.as_str()))
        .unwrap_or("");

    let authors = meta
        .get("authors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let names: Vec<&str> = arr
                .iter()
                .take(4)
                .filter_map(|a| a.get("full_name").and_then(|v| v.as_str()))
                .collect();
            if !names.is_empty() {
                if arr.len() > 4 {
                    format!("{} et al.", names.join("; "))
                } else {
                    names.join("; ")
                }
            } else if !collab.is_empty() {
                format!("{} Collaboration", collab)
            } else {
                "INSPIRE-HEP Author".to_string()
            }
        })
        .unwrap_or_else(|| {
            if !collab.is_empty() {
                format!("{} Collaboration", collab)
            } else {
                "INSPIRE-HEP Author".to_string()
            }
        });

    let doi = meta
        .get("dois")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find_map(|d| d.get("value").and_then(|v| v.as_str())))
        .map(normalize_doi)
        .unwrap_or_default();

    let arxiv_id = meta
        .get("arxiv_eprints")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|a| a.get("value").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim()
        .to_string();

    let venue = meta
        .get("publication_info")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find_map(|p| p.get("journal_title").and_then(|v| v.as_str()))
        })
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            if !arxiv_id.is_empty() {
                format!("INSPIRE-HEP / arXiv ({})", arxiv_id)
            } else {
                "INSPIRE-HEP Literature".to_string()
            }
        });

    let mut oa_pdf_url = meta
        .get("documents")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find_map(|d| d.get("url").and_then(|v| v.as_str()))
        })
        .filter(|u| u.starts_with("http"))
        .unwrap_or("")
        .to_string();
    let mut oa_status = OaStatus::Unknown;
    if oa_pdf_url.is_empty() && !arxiv_id.is_empty() {
        oa_pdf_url = format!("https://arxiv.org/pdf/{}.pdf", arxiv_id);
        oa_status = OaStatus::Arxiv;
    } else if !oa_pdf_url.is_empty() {
        oa_status = OaStatus::Green;
    }

    let cited_by = if filter_self_citations {
        meta.get("citation_count_without_self_citations")
            .or_else(|| meta.get("citation_count"))
            .and_then(|v| v.as_u64())
    } else {
        meta.get("citation_count").and_then(|v| v.as_u64())
    };

    let abstract_text = meta
        .get("abstracts")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|a| a.get("value").and_then(|v| v.as_str()))
        .map(collapse_ws)
        .unwrap_or_default();

    let mut terms: Vec<String> = Vec::new();
    if let Some(cats) = meta.get("inspire_categories").and_then(|v| v.as_array()) {
        for c in cats.iter().take(5) {
            if let Some(t) = c.get("term").and_then(|v| v.as_str()) {
                terms.push(t.to_string());
            }
        }
    }
    if !recid.is_empty() {
        terms.push(format!("inspire_recid:{}", recid));
    }

    let mut raw_refs = Vec::new();
    if let Some(refs) = meta.get("references").and_then(|v| v.as_array()) {
        for r in refs {
            if let Some(ref_url) = r.pointer("/record/$ref").and_then(|v| v.as_str()) {
                let tgt_recid = ref_url.rsplit('/').next().unwrap_or("").trim();
                if !tgt_recid.is_empty() && tgt_recid.chars().all(|c| c.is_ascii_digit()) {
                    let tag = format!("INSPIRE:{}", tgt_recid);
                    if !raw_refs.contains(&tag) {
                        raw_refs.push(tag);
                    }
                }
            }
            if let Some(r_dois) = r.pointer("/reference/dois").and_then(|v| v.as_array()) {
                for d_val in r_dois.iter().filter_map(|x| x.as_str()) {
                    let nd = normalize_doi(d_val);
                    if !nd.is_empty() {
                        let tag = format!("DOI:{}", nd);
                        if !raw_refs.contains(&tag) {
                            raw_refs.push(tag);
                        }
                    }
                }
            }
            if let Some(r_arx) = r.pointer("/reference/arxiv_eprint").and_then(|v| v.as_str()) {
                let clean_a = r_arx.trim().trim_start_matches("arXiv:");
                if !clean_a.is_empty() {
                    let tag = format!("ARXIV:{}", clean_a);
                    if !raw_refs.contains(&tag) {
                        raw_refs.push(tag);
                    }
                }
            }
        }
    }

    let combined = format!("{} {} {} {}", title, venue, terms.join(" "), abstract_text);
    let disc = Discipline::infer_from_text(&combined);
    let concepts = infer_concepts_from_text(&combined, disc);
    if terms.is_empty() {
        terms = concepts.clone();
    }

    let source_url = if !recid.is_empty() {
        format!("https://inspirehep.net/literature/{}", recid)
    } else if !doi.is_empty() {
        format!("https://doi.org/{}", doi)
    } else {
        "https://inspirehep.net".to_string()
    };

    Some(PaperRecord {
        id: if !recid.is_empty() {
            format!("insp_{}", recid)
        } else {
            format!("insp_{}", idx)
        },
        title,
        authors,
        year,
        venue,
        disc,
        doi,
        arxiv_id,
        openalex_id: String::new(),
        s2_id: String::new(),
        pmcid: String::new(),
        source_url,
        oa_pdf_url,
        oa_status,
        external_citations: cited_by,
        abstract_text,
        terms,
        concepts,
        local_pdf_path: None,
        pdf_sha256: None,
        pdf_bytes: None,
        raw_references: raw_refs,
        retraction: RetractionRecord::default(),
        datacite_links: Vec::new(),
        bio_entities: Vec::new(),
    })
}

fn snowball_inspire_hep(
    http: &ScholarlyHttpClient,
    seeds: &[PaperRecord],
    max_neighbors: usize,
    filter_self_citations: bool,
) -> Result<Vec<PaperRecord>, String> {
    let mut seed_recids: HashSet<String> = HashSet::new();
    for s in seeds {
        for t in &s.terms {
            if let Some(rid) = t.strip_prefix("inspire_recid:") {
                seed_recids.insert(rid.to_string());
            }
        }
    }

    let mut ref_counts: HashMap<String, usize> = HashMap::new();
    for s in seeds {
        for r in &s.raw_references {
            if let Some(rid) = r.strip_prefix("INSPIRE:") {
                if !seed_recids.contains(rid) {
                    *ref_counts.entry(rid.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    let mut neighbors = Vec::new();

    // 1. Outgoing top referenced INSPIRE recids
    if !ref_counts.is_empty() {
        let mut ranked: Vec<(String, usize)> = ref_counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));
        let top_ids: Vec<String> = ranked
            .into_iter()
            .take(max_neighbors.min(20))
            .map(|(r, _)| format!("recid:{}", r))
            .collect();
        let q_expr = top_ids.join(" or ");
        let url = format!(
            "https://inspirehep.net/api/literature?q={}&size={}&fields={}",
            percent_encode(&q_expr),
            top_ids.len(),
            INSPIRE_FIELDS
        );
        if let Ok(json) = http.get_json_with_bearer(&url, None, "search_api") {
            if let Some(hits) = json.pointer("/hits/hits").and_then(|v| v.as_array()) {
                for (i, h) in hits.iter().enumerate() {
                    if let Some(rec) = parse_inspire_literature_hit(h, 3000 + i, filter_self_citations)
                    {
                        neighbors.push(rec);
                    }
                }
            }
        }
    }

    // 2. Incoming citing works via `refersto:recid:{id}`
    if !seed_recids.is_empty() {
        let citer_clauses: Vec<String> = seed_recids
            .iter()
            .take(10)
            .map(|r| format!("refersto:recid:{}", r))
            .collect();
        let q_citers = citer_clauses.join(" or ");
        let url = format!(
            "https://inspirehep.net/api/literature?q={}&size={}&sort=mostcited&fields={}",
            percent_encode(&q_citers),
            max_neighbors.min(20),
            INSPIRE_FIELDS
        );
        if let Ok(json) = http.get_json_with_bearer(&url, None, "search_api") {
            if let Some(hits) = json.pointer("/hits/hits").and_then(|v| v.as_array()) {
                for (i, h) in hits.iter().enumerate() {
                    if let Some(rec) = parse_inspire_literature_hit(h, 3500 + i, filter_self_citations)
                    {
                        neighbors.push(rec);
                    }
                }
            }
        }
    }

    Ok(neighbors)
}

// ============================================================================
// PROVIDER 7: NASA ASTROPHYSICS DATA SYSTEM (https://api.adsabs.harvard.edu/v1)
// ============================================================================
const ADS_FL_FIELDS: &str = "bibcode,title,author,year,pub,doi,identifier,citation_count,read_count,abstract,keyword,property,esources,reference,citation";

fn search_nasa_ads(
    http: &ScholarlyHttpClient,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let Some(token) = http.nasa_ads_token() else {
        return Err(
            "NASA ADS requires a Bearer token: set ADS_API_TOKEN (or NASA_ADS_TOKEN) or enter it under Search & Crawl -> Polite Crawler & API Tokens"
                .to_string(),
        );
    };

    let rows = req.max_per_provider.clamp(1, 100);
    let solr_q = build_nasa_ads_query(req);

    let url = format!(
        "https://api.adsabs.harvard.edu/v1/search/query?q={}&rows={}&sort=citation_count+desc&fl={}",
        percent_encode(&solr_q),
        rows,
        ADS_FL_FIELDS
    );

    let json = http.get_json_with_bearer(&url, Some(token), "search_api")?;
    let docs = json
        .pointer("/response/docs")
        .and_then(|v| v.as_array())
        .ok_or("NASA ADS JSON missing response.docs array")?;

    let mut out = Vec::with_capacity(docs.len());
    for (idx, doc) in docs.iter().enumerate() {
        if let Some(rec) = parse_nasa_ads_doc(doc, idx) {
            out.push(rec);
        }
    }
    Ok(out)
}

pub fn parse_nasa_ads_doc(doc: &Value, idx: usize) -> Option<PaperRecord> {
    let bibcode = doc
        .get("bibcode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let title = doc
        .get("title")
        .and_then(|v| {
            v.as_array()
                .and_then(|a| a.first())
                .and_then(|t| t.as_str())
                .or_else(|| v.as_str())
        })?
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let year = doc
        .get("year")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.trim().parse::<i32>().ok())
                .or_else(|| v.as_i64().map(|n| n as i32))
        })
        .unwrap_or(2023);

    let authors = doc
        .get("author")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let names: Vec<&str> = arr.iter().take(4).filter_map(|a| a.as_str()).collect();
            if names.is_empty() {
                "NASA ADS Author".to_string()
            } else if arr.len() > 4 {
                format!("{} et al.", names.join("; "))
            } else {
                names.join("; ")
            }
        })
        .unwrap_or_else(|| "NASA ADS Author".to_string());

    let venue = doc
        .get("pub")
        .and_then(|v| v.as_str())
        .unwrap_or("NASA ADS Indexed Publication")
        .trim()
        .to_string();

    let mut doi = doc
        .get("doi")
        .and_then(|v| {
            v.as_array()
                .and_then(|a| a.first())
                .and_then(|s| s.as_str())
                .or_else(|| v.as_str())
        })
        .map(normalize_doi)
        .unwrap_or_default();

    let mut arxiv_id = String::new();
    if let Some(idents) = doc.get("identifier").and_then(|v| v.as_array()) {
        for id_str in idents.iter().filter_map(|x| x.as_str()) {
            let trimmed = id_str.trim();
            if arxiv_id.is_empty() {
                if let Some(rest) = trimmed
                    .strip_prefix("arXiv:")
                    .or_else(|| trimmed.strip_prefix("arxiv:"))
                {
                    arxiv_id = rest.trim().to_string();
                }
            }
            if doi.is_empty() && trimmed.starts_with("10.") {
                doi = normalize_doi(trimmed);
            }
        }
    }

    let esources: Vec<String> = doc
        .get("esources")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_ascii_uppercase()))
                .collect()
        })
        .unwrap_or_default();
    let props: Vec<String> = doc
        .get("property")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_ascii_uppercase()))
                .collect()
        })
        .unwrap_or_default();

    let mut oa_pdf_url = String::new();
    let mut oa_status = OaStatus::Unknown;
    if !arxiv_id.is_empty() {
        oa_pdf_url = format!("https://arxiv.org/pdf/{}.pdf", arxiv_id);
        oa_status = OaStatus::Arxiv;
    } else if !bibcode.is_empty() && esources.iter().any(|e| e == "EPRINT_PDF") {
        oa_pdf_url = format!(
            "https://ui.adsabs.harvard.edu/link_gateway/{}/EPRINT_PDF",
            percent_encode(&bibcode)
        );
        oa_status = OaStatus::Green;
    } else if !bibcode.is_empty() && esources.iter().any(|e| e == "ADS_PDF") {
        oa_pdf_url = format!(
            "https://ui.adsabs.harvard.edu/link_gateway/{}/ADS_PDF",
            percent_encode(&bibcode)
        );
        oa_status = OaStatus::Bronze;
    } else if props.iter().any(|p| p == "OPENACCESS") && !bibcode.is_empty() {
        oa_status = OaStatus::Gold;
    }

    let cited_by = doc.get("citation_count").and_then(|v| v.as_u64());
    let abstract_text = doc
        .get("abstract")
        .and_then(|v| v.as_str())
        .map(collapse_ws)
        .unwrap_or_default();

    let mut terms: Vec<String> = Vec::new();
    if let Some(kws) = doc.get("keyword").and_then(|v| v.as_array()) {
        for kw in kws.iter().take(6).filter_map(|x| x.as_str()) {
            let clean = kw.trim();
            if !clean.is_empty() {
                terms.push(clean.to_string());
            }
        }
    }
    if !bibcode.is_empty() {
        terms.push(format!("bibcode:{}", bibcode));
    }

    let mut raw_refs = Vec::new();
    if let Some(refs) = doc.get("reference").and_then(|v| v.as_array()) {
        for r in refs.iter().filter_map(|x| x.as_str()) {
            let rb = r.trim();
            if !rb.is_empty() {
                raw_refs.push(format!("BIBCODE:{}", rb));
            }
        }
    }
    if let Some(cits) = doc.get("citation").and_then(|v| v.as_array()) {
        for c in cits.iter().take(120).filter_map(|x| x.as_str()) {
            let cb = c.trim();
            if !cb.is_empty() {
                raw_refs.push(format!("CITED_BY_BIBCODE:{}", cb));
            }
        }
    }

    let combined = format!("{} {} {} {}", title, venue, terms.join(" "), abstract_text);
    let disc = Discipline::infer_from_text(&combined);
    let concepts = infer_concepts_from_text(&combined, disc);
    if terms.is_empty() {
        terms = concepts.clone();
    }

    let source_url = if !bibcode.is_empty() {
        format!("https://ui.adsabs.harvard.edu/abs/{}/abstract", percent_encode(&bibcode))
    } else if !doi.is_empty() {
        format!("https://doi.org/{}", doi)
    } else {
        "https://ui.adsabs.harvard.edu".to_string()
    };

    let slug: String = if !bibcode.is_empty() {
        bibcode
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    } else {
        format!("{}", idx)
    };

    Some(PaperRecord {
        id: format!("ads_{}", slug),
        title,
        authors,
        year,
        venue,
        disc,
        doi,
        arxiv_id,
        openalex_id: String::new(),
        s2_id: String::new(),
        pmcid: String::new(),
        source_url,
        oa_pdf_url,
        oa_status,
        external_citations: cited_by,
        abstract_text,
        terms,
        concepts,
        local_pdf_path: None,
        pdf_sha256: None,
        pdf_bytes: None,
        raw_references: raw_refs,
        retraction: RetractionRecord::default(),
        datacite_links: Vec::new(),
        bio_entities: Vec::new(),
    })
}

fn snowball_nasa_ads_bibcodes(
    http: &ScholarlyHttpClient,
    seeds: &[PaperRecord],
    max_neighbors: usize,
) -> Result<Vec<PaperRecord>, String> {
    let Some(token) = http.nasa_ads_token() else {
        return Ok(Vec::new());
    };
    let mut existing: HashSet<String> = HashSet::new();
    for s in seeds {
        for t in &s.terms {
            if let Some(b) = t.strip_prefix("bibcode:") {
                existing.insert(b.to_string());
            }
        }
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in seeds {
        for r in &s.raw_references {
            let b_opt = r
                .strip_prefix("BIBCODE:")
                .or_else(|| r.strip_prefix("CITED_BY_BIBCODE:"));
            if let Some(b) = b_opt {
                if !existing.contains(b) {
                    *counts.entry(b.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    if counts.is_empty() {
        return Ok(Vec::new());
    }

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let quoted: Vec<String> = ranked
        .into_iter()
        .take(max_neighbors.min(25))
        .map(|(b, _)| format!("\"{}\"", b.replace('"', "")))
        .collect();
    let solr_q = format!("bibcode:({})", quoted.join(" OR "));
    let url = format!(
        "https://api.adsabs.harvard.edu/v1/search/query?q={}&rows={}&fl={}",
        percent_encode(&solr_q),
        quoted.len(),
        ADS_FL_FIELDS
    );

    let json = http.get_json_with_bearer(&url, Some(token), "search_api")?;
    let docs = json
        .pointer("/response/docs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(docs
        .iter()
        .enumerate()
        .filter_map(|(i, d)| parse_nasa_ads_doc(d, 4000 + i))
        .collect())
}

// ============================================================================
// CROSS-PROVIDER DEDUPLICATION & CITATION EDGE LINKER
// ============================================================================
pub fn merge_and_link_records(
    records: Vec<PaperRecord>,
    oa_only: bool,
    year_min: Option<i32>,
    year_max: Option<i32>,
) -> (Vec<PaperRecord>, Vec<CitationEdge>) {
    let mut merged: Vec<PaperRecord> = Vec::new();
    let mut doi_map: HashMap<String, usize> = HashMap::new();
    let mut arxiv_map: HashMap<String, usize> = HashMap::new();
    let mut oa_map: HashMap<String, usize> = HashMap::new();
    let mut s2_map: HashMap<String, usize> = HashMap::new();
    let mut title_map: HashMap<String, usize> = HashMap::new();

    for rec in records {
        if let Some(ymin) = year_min {
            if rec.year < ymin {
                continue;
            }
        }
        if let Some(ymax) = year_max {
            if rec.year > ymax {
                continue;
            }
        }

        let norm_title = format!(
            "{}_{}",
            rec.title
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase(),
            rec.year
        );

        let existing_idx = (!rec.doi.is_empty())
            .then(|| doi_map.get(&rec.doi).copied())
            .flatten()
            .or_else(|| {
                (!rec.arxiv_id.is_empty())
                    .then(|| arxiv_map.get(&rec.arxiv_id).copied())
                    .flatten()
            })
            .or_else(|| {
                (!rec.openalex_id.is_empty())
                    .then(|| oa_map.get(&rec.openalex_id).copied())
                    .flatten()
            })
            .or_else(|| {
                (!rec.s2_id.is_empty())
                    .then(|| s2_map.get(&rec.s2_id).copied())
                    .flatten()
            })
            .or_else(|| title_map.get(&norm_title).copied());

        if let Some(idx) = existing_idx {
            let cur = &mut merged[idx];
            if cur.doi.is_empty() && !rec.doi.is_empty() {
                cur.doi = rec.doi.clone();
            }
            if cur.arxiv_id.is_empty() && !rec.arxiv_id.is_empty() {
                cur.arxiv_id = rec.arxiv_id.clone();
            }
            if cur.openalex_id.is_empty() && !rec.openalex_id.is_empty() {
                cur.openalex_id = rec.openalex_id.clone();
            }
            if cur.s2_id.is_empty() && !rec.s2_id.is_empty() {
                cur.s2_id = rec.s2_id.clone();
            }
            if cur.pmcid.is_empty() && !rec.pmcid.is_empty() {
                cur.pmcid = rec.pmcid.clone();
            }
            if cur.oa_pdf_url.is_empty() && !rec.oa_pdf_url.is_empty() {
                cur.oa_pdf_url = rec.oa_pdf_url.clone();
                cur.oa_status = rec.oa_status;
            }
            if cur.abstract_text.len() < rec.abstract_text.len() {
                cur.abstract_text = rec.abstract_text.clone();
            }
            if cur.external_citations.unwrap_or(0) < rec.external_citations.unwrap_or(0) {
                cur.external_citations = rec.external_citations;
            }
            for c in rec.concepts {
                if !cur.concepts.contains(&c) && cur.concepts.len() < 4 {
                    cur.concepts.push(c);
                }
            }
            for t in rec.terms {
                let is_id_tag = t.starts_with("pmid:")
                    || t.starts_with("gs_cites:")
                    || t.starts_with("inspire_recid:")
                    || t.starts_with("bibcode:")
                    || t.starts_with("openaire_id:");
                if is_id_tag && !cur.terms.contains(&t) {
                    cur.terms.push(t);
                }
            }
            for r in rec.raw_references {
                if !cur.raw_references.contains(&r) {
                    cur.raw_references.push(r);
                }
            }
            apply_retraction_update(&mut cur.retraction, &rec.retraction);
            for dl in rec.datacite_links {
                if !cur
                    .datacite_links
                    .iter()
                    .any(|existing| existing.artifact_doi == dl.artifact_doi)
                {
                    cur.datacite_links.push(dl);
                }
            }
            for be in rec.bio_entities {
                merge_bio_entity_into_paper(&mut cur.bio_entities, be);
            }
        } else {
            let idx = merged.len();
            if !rec.doi.is_empty() {
                doi_map.insert(rec.doi.clone(), idx);
            }
            if !rec.arxiv_id.is_empty() {
                arxiv_map.insert(rec.arxiv_id.clone(), idx);
            }
            if !rec.openalex_id.is_empty() {
                oa_map.insert(rec.openalex_id.clone(), idx);
            }
            if !rec.s2_id.is_empty() {
                s2_map.insert(rec.s2_id.clone(), idx);
            }
            title_map.insert(norm_title, idx);
            merged.push(rec);
        }
    }

    if oa_only {
        merged.retain(|p| !p.oa_pdf_url.is_empty() || !p.arxiv_id.is_empty() || !p.pmcid.is_empty());
    }

    // Re-index maps after optional OA filter
    doi_map.clear();
    arxiv_map.clear();
    oa_map.clear();
    s2_map.clear();
    let mut id_map: HashMap<String, usize> = HashMap::new();
    let mut pmid_map: HashMap<String, usize> = HashMap::new();
    let mut gs_cluster_map: HashMap<String, usize> = HashMap::new();
    let mut inspire_map: HashMap<String, usize> = HashMap::new();
    let mut bibcode_map: HashMap<String, usize> = HashMap::new();
    for (i, p) in merged.iter().enumerate() {
        id_map.insert(p.id.clone(), i);
        if !p.doi.is_empty() {
            doi_map.insert(p.doi.clone(), i);
        }
        if !p.arxiv_id.is_empty() {
            arxiv_map.insert(p.arxiv_id.clone(), i);
        }
        if !p.openalex_id.is_empty() {
            oa_map.insert(p.openalex_id.clone(), i);
        }
        if !p.s2_id.is_empty() {
            s2_map.insert(p.s2_id.clone(), i);
        }
        for t in &p.terms {
            if let Some(pmid) = t.strip_prefix("pmid:") {
                pmid_map.insert(pmid.to_string(), i);
            } else if let Some(gsc) = t.strip_prefix("gs_cites:") {
                gs_cluster_map.insert(gsc.to_string(), i);
            } else if let Some(insp) = t.strip_prefix("inspire_recid:") {
                inspire_map.insert(insp.to_string(), i);
            } else if let Some(bib) = t.strip_prefix("bibcode:") {
                bibcode_map.insert(bib.to_string(), i);
            }
        }
    }

    let mut edges = Vec::new();
    let mut edge_set = HashSet::new();
    for (u_idx, p) in merged.iter().enumerate() {
        for raw_ref in &p.raw_references {
            let mut reverse_citer = false;
            let peer_idx = if let Some(w) = raw_ref.strip_prefix("OA:") {
                oa_map.get(w).copied()
            } else if let Some(s2) = raw_ref.strip_prefix("S2:") {
                s2_map.get(s2).copied()
            } else if let Some(d) = raw_ref.strip_prefix("DOI:") {
                doi_map.get(d).copied()
            } else if let Some(arx) = raw_ref.strip_prefix("ARXIV:") {
                arxiv_map.get(arx).copied()
            } else if let Some(pm) = raw_ref.strip_prefix("PMID:") {
                pmid_map.get(pm).copied()
            } else if let Some(gsc) = raw_ref.strip_prefix("GS_CLUSTER:") {
                gs_cluster_map.get(gsc).copied()
            } else if let Some(insp) = raw_ref.strip_prefix("INSPIRE:") {
                inspire_map.get(insp).copied()
            } else if let Some(bib) = raw_ref.strip_prefix("BIBCODE:") {
                bibcode_map.get(bib).copied()
            } else if let Some(cb_bib) = raw_ref.strip_prefix("CITED_BY_BIBCODE:") {
                reverse_citer = true;
                bibcode_map.get(cb_bib).copied()
            } else if let Some(cb_doi) = raw_ref.strip_prefix("CITED_BY_DOI:") {
                reverse_citer = true;
                doi_map.get(cb_doi).copied()
            } else {
                id_map.get(raw_ref).copied()
            };

            if let Some(v_idx) = peer_idx {
                let (src_i, tgt_i) = if reverse_citer {
                    (v_idx, u_idx)
                } else {
                    (u_idx, v_idx)
                };
                if src_i != tgt_i && edge_set.insert((src_i, tgt_i)) {
                    edges.push(CitationEdge {
                        source: merged[src_i].id.clone(),
                        target: merged[tgt_i].id.clone(),
                        provenance: "provider_reference_match".to_string(),
                    });
                }
            }
        }
    }

    (merged, edges)
}

#[cfg(test)]
mod pop_search_tests {
    use super::*;
    use crate::crawler::{build_europe_pmc_query, build_pubmed_term};

    #[test]
    fn identifier_lookup_classifies_doi_pmid_arxiv_orcid() {
        assert_eq!(
            parse_identifier_lookup("https://doi.org/10.1073/pnas.0507655102"),
            Some(IdentifierLookupKind::Doi("10.1073/pnas.0507655102".to_string()))
        );
        assert_eq!(
            parse_identifier_lookup("PMID:16275915"),
            Some(IdentifierLookupKind::Pmid("16275915".to_string()))
        );
        assert_eq!(
            parse_identifier_lookup("arXiv:1706.03762"),
            Some(IdentifierLookupKind::Arxiv("1706.03762".to_string()))
        );
        assert_eq!(
            parse_identifier_lookup("https://orcid.org/0000-0002-1825-0097"),
            Some(IdentifierLookupKind::Orcid("0000-0002-1825-0097".to_string()))
        );
    }

    #[test]
    fn provider_url_builders_map_pop_fields_natively() {
        let mut req = SearchRequest::default();
        req.query = String::new();
        req.authors = "Harzing".to_string();
        req.publication_venue = "Scientometrics".to_string();
        req.title_words = "individual annual".to_string();
        req.keywords_all = "career length".to_string();
        req.exclude_words = "retracted".to_string();
        req.year_min = Some(2005);
        req.year_max = Some(2025);

        let oa_url = build_openalex_query_url(&req, "test@crvscience.com", None);
        assert!(oa_url.contains("title.search:"));
        assert!(oa_url.contains("raw_author_name.search:"));
        assert!(oa_url.contains("primary_location.source.display_name.search:"));

        let cr_url = build_crossref_query_url(&req, "test@crvscience.com");
        assert!(cr_url.contains("query.author="));
        assert!(cr_url.contains("query.container-title="));
        assert!(cr_url.contains("query.title="));

        let arx_url = build_arxiv_query_url(&req);
        assert!(arx_url.contains("ti%3Aindividual"));
        assert!(arx_url.contains("ANDNOT"));

        let pm_term = build_pubmed_term(&req);
        assert!(pm_term.contains("[Title]"));
        assert!(pm_term.contains("[Author]"));
        assert!(pm_term.contains("[Journal]"));
        assert!(pm_term.contains("NOT (retracted)"));

        let epmc_q = build_europe_pmc_query(&req);
        assert!(epmc_q.contains("TITLE:\"individual annual\""));
        assert!(epmc_q.contains("AUTH:\"Harzing\""));
        assert!(epmc_q.contains("JOURNAL:\"Scientometrics\""));
    }

    #[test]
    fn offline_scholarly_index_filters_author_venue_exclude_and_doi() {
        let mut req = SearchRequest::default();
        req.query = String::new();
        req.authors = "Vaswani".to_string();
        let hits = search_offline_scholarly_index(&req);
        assert!(!hits.is_empty());
        assert!(hits[0].title.to_ascii_lowercase().contains("attention"));

        let mut exclude_req = SearchRequest::default();
        exclude_req.query = "Publish or Perish".to_string();
        exclude_req.exclude_words = "disciplinary".to_string();
        let filtered = search_offline_scholarly_index(&exclude_req);
        assert!(!filtered.iter().any(|p| p.id == "harzing2014"));

        let mut doi_req = SearchRequest::default();
        doi_req.query = String::new();
        doi_req.identifier_lookup = "10.1073/pnas.0507655102".to_string();
        let doi_hits = search_offline_scholarly_index(&doi_req);
        assert_eq!(doi_hits.len(), 1);
        assert_eq!(doi_hits[0].id, "hirsch2005");
    }
}
