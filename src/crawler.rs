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
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::{
    infer_concepts_from_text, CitationEdge, Discipline, OaStatus, PaperRecord, SearchRequest,
};
use crate::rate_limiter::{
    acquire_domain_permit_blocking, apply_global_robots_crawl_delay, log_global_crawl_outcome,
    parse_robots_crawl_delay_ms, AcquirePermit, CrawlAuditOutcome,
};
use crate::search_providers::{
    build_effective_query, normalize_doi, parse_identifier_lookup, percent_encode,
    IdentifierLookupKind,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrawlBlockReason {
    RobotsDisallowed(String),
    CaptchaOrRateLimit(String),
    HttpError(String),
}

impl CrawlBlockReason {
    pub fn message(&self) -> String {
        match self {
            CrawlBlockReason::RobotsDisallowed(msg) => msg.clone(),
            CrawlBlockReason::CaptchaOrRateLimit(msg) => msg.clone(),
            CrawlBlockReason::HttpError(msg) => msg.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScholarSerpHit {
    pub cluster_id: String,
    pub title: String,
    pub authors: String,
    pub venue: String,
    pub year: i32,
    pub url: String,
    pub pdf_url: String,
    pub snippet: String,
    pub cited_by_count: Option<u64>,
    pub cites_cluster_id: String,
    pub doi: String,
    pub arxiv_id: String,
    pub pmcid: String,
}

pub struct PoliteWebCrawler {
    client: Client,
    polite_email: String,
    robots_cache: HashMap<String, String>,
    serpapi_key: Option<String>,
    ncbi_api_key: Option<String>,
    gs_cookie: Option<String>,
}

impl PoliteWebCrawler {
    pub fn new(polite_email: &str, proxy_url: &str) -> Result<Self, String> {
        let email = if polite_email.trim().is_empty() {
            "research@crvscience.com"
        } else {
            polite_email.trim()
        };
        let ua = format!(
            "Mozilla/5.0 (compatible; CitAtlasCrawler/1.2; +https://crvscience.com; mailto:{})",
            email
        );

        let mut builder = Client::builder()
            .user_agent(ua)
            .timeout(Duration::from_secs(28));

        let effective_proxy = if !proxy_url.trim().is_empty() {
            Some(proxy_url.trim().to_string())
        } else {
            std::env::var("CRAWL_PROXY_URL").ok().filter(|s| !s.trim().is_empty())
        };

        if let Some(ref px) = effective_proxy {
            let proxy = reqwest::Proxy::all(px)
                .map_err(|e| format!("Invalid proxy URL '{}': {}", px, e))?;
            builder = builder.proxy(proxy);
        }

        let client = builder
            .build()
            .map_err(|e| format!("Failed initializing crawler HTTP client: {}", e))?;

        Ok(Self {
            client,
            polite_email: email.to_string(),
            robots_cache: HashMap::new(),
            serpapi_key: std::env::var("SERPAPI_KEY").ok().filter(|s| !s.is_empty()),
            ncbi_api_key: std::env::var("NCBI_API_KEY").ok().filter(|s| !s.is_empty()),
            gs_cookie: std::env::var("GS_COOKIE").ok().filter(|s| !s.is_empty()),
        })
    }

    pub fn check_robots_allowed(&mut self, origin: &str, path_and_query: &str) -> bool {
        let full_target = format!("{}{}", origin.trim_end_matches('/'), path_and_query);
        let was_cached = self.robots_cache.contains_key(origin);
        let robots_body = if let Some(cached) = self.robots_cache.get(origin) {
            cached.clone()
        } else {
            let robots_url = format!("{}/robots.txt", origin.trim_end_matches('/'));
            let t0 = Instant::now();
            let fetched = match self.client.get(&robots_url).send() {
                Ok(resp) => {
                    let st = resp.status().as_u16();
                    let body = if resp.status().is_success() {
                        resp.text().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    log_global_crawl_outcome(
                        "robots_txt",
                        "GET",
                        &robots_url,
                        if (200..300).contains(&st) {
                            CrawlAuditOutcome::Allowed2xx
                        } else {
                            CrawlAuditOutcome::HttpOtherError
                        },
                        st,
                        0,
                        t0.elapsed().as_millis() as u64,
                        body.len() as u64,
                        None,
                        "fetched_robots",
                        "Cached RFC 9309 robots.txt policy",
                    );
                    body
                }
                Err(_) => String::new(),
            };
            if let Some(delay_ms) = parse_robots_crawl_delay_ms(&fetched, "CitAtlasCrawler") {
                apply_global_robots_crawl_delay(origin, delay_ms);
            }
            self.robots_cache.insert(origin.to_string(), fetched.clone());
            fetched
        };

        let allowed = if robots_body.trim().is_empty() {
            true
        } else {
            is_path_allowed_by_robots(&robots_body, "CitAtlasCrawler", path_and_query)
        };
        if !allowed {
            log_global_crawl_outcome(
                "robots_txt",
                "GET",
                &full_target,
                CrawlAuditOutcome::RobotsDisallowed,
                0,
                0,
                0,
                0,
                None,
                if was_cached { "cached_disallow" } else { "disallowed" },
                "Blocked before network dispatch by RFC 9309 robots.txt",
            );
        }
        allowed
    }

    pub fn sleep_polite(&self, base_ms: u64, jitter_ms: u64, step_index: usize) {
        if base_ms == 0 {
            return;
        }
        let pseudo_jitter = if jitter_ms > 0 {
            (step_index as u64).wrapping_mul(2654435761) % jitter_ms
        } else {
            0
        };
        thread::sleep(Duration::from_millis(base_ms + pseudo_jitter));
    }

    pub fn get_html(&self, url: &str, include_gs_cookie: bool) -> Result<String, CrawlBlockReason> {
        let wait_ms = match acquire_domain_permit_blocking(url) {
            AcquirePermit::Granted { wait_ms, .. } => wait_ms,
            AcquirePermit::CircuitOpen { reason, .. } => {
                log_global_crawl_outcome(
                    "polite_crawler",
                    "GET",
                    url,
                    CrawlAuditOutcome::CircuitBreakerSkipped,
                    0,
                    0,
                    0,
                    0,
                    None,
                    "circuit_open",
                    &reason,
                );
                return Err(CrawlBlockReason::CaptchaOrRateLimit(reason));
            }
        };

        let mut req = self
            .client
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9");
        if include_gs_cookie {
            if let Some(ref ck) = self.gs_cookie {
                req = req.header("Cookie", ck);
            }
        }
        let t0 = Instant::now();
        match req.send() {
            Ok(resp) => {
                let status = resp.status();
                let code = status.as_u16();
                let hdrs = resp.headers().clone();
                if code == 429 {
                    log_global_crawl_outcome(
                        "polite_crawler",
                        "GET",
                        url,
                        CrawlAuditOutcome::Http429RateLimited,
                        code,
                        wait_ms,
                        t0.elapsed().as_millis() as u64,
                        0,
                        Some(&hdrs),
                        "allowed",
                        "HTTP 429 rate-limit challenge",
                    );
                    return Err(CrawlBlockReason::CaptchaOrRateLimit(format!(
                        "HTTP {} rate-limit/challenge from {}",
                        code, url
                    )));
                }
                if code == 403 {
                    log_global_crawl_outcome(
                        "polite_crawler",
                        "GET",
                        url,
                        CrawlAuditOutcome::Http403OrCaptcha,
                        code,
                        wait_ms,
                        t0.elapsed().as_millis() as u64,
                        0,
                        Some(&hdrs),
                        "allowed",
                        "HTTP 403 challenge; domain circuit-breaker armed",
                    );
                    return Err(CrawlBlockReason::CaptchaOrRateLimit(format!(
                        "HTTP {} rate-limit/challenge from {}",
                        code, url
                    )));
                }
                if !status.is_success() {
                    log_global_crawl_outcome(
                        "polite_crawler",
                        "GET",
                        url,
                        CrawlAuditOutcome::HttpOtherError,
                        code,
                        wait_ms,
                        t0.elapsed().as_millis() as u64,
                        0,
                        Some(&hdrs),
                        "allowed",
                        "Non-2xx HTML response",
                    );
                    return Err(CrawlBlockReason::HttpError(format!(
                        "HTTP {} from {}",
                        code, url
                    )));
                }
                let body = resp
                    .text()
                    .map_err(|e| CrawlBlockReason::HttpError(format!("Decode error: {}", e)))?;
                let low = body.to_ascii_lowercase();
                let is_captcha = low.contains("gs_captcha_ccl")
                    || low.contains("id=\"gs_captcha_f\"")
                    || low.contains("unusual traffic from your computer network");
                if is_captcha {
                    log_global_crawl_outcome(
                        "polite_crawler",
                        "GET",
                        url,
                        CrawlAuditOutcome::Http403OrCaptcha,
                        code,
                        wait_ms,
                        t0.elapsed().as_millis() as u64,
                        body.len() as u64,
                        Some(&hdrs),
                        "allowed",
                        "CAPTCHA DOM detected; circuit-breaker armed",
                    );
                } else {
                    log_global_crawl_outcome(
                        "polite_crawler",
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
                        body.len() as u64,
                        Some(&hdrs),
                        "allowed",
                        "HTML SERP fetched",
                    );
                }
                Ok(body)
            }
            Err(e) => {
                log_global_crawl_outcome(
                    "polite_crawler",
                    "GET",
                    url,
                    CrawlAuditOutcome::NetworkError,
                    0,
                    wait_ms,
                    t0.elapsed().as_millis() as u64,
                    0,
                    None,
                    "allowed",
                    &e.to_string(),
                );
                Err(CrawlBlockReason::HttpError(format!("Network error: {}", e)))
            }
        }
    }

    pub fn get_json(&self, url: &str) -> Result<Value, String> {
        let subsys = if url.contains("annotations_api")
            || url.contains("uniprot.org")
            || url.contains("rcsb.org")
        {
            "bio_bridges"
        } else if url.contains("datacite.org") || url.contains("crossref.org") {
            "integrity_datacite"
        } else {
            "polite_crawler"
        };
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                thread::sleep(Duration::from_millis(550 * (1 << attempt)));
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
            let t0 = Instant::now();
            match self.client.get(url).send() {
                Ok(resp) => {
                    let st = resp.status();
                    let code = st.as_u16();
                    let hdrs = resp.headers().clone();
                    if st.is_success() {
                        let text = resp
                            .text()
                            .map_err(|e| format!("Body read error from {}: {}", url, e))?;
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
                            .map_err(|e| format!("JSON parse error from {}: {}", url, e));
                    } else if code == 429 || st.is_server_error() {
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
                            &format!("Attempt {} backoff", attempt + 1),
                        );
                        last_err = format!("HTTP {} from {}", code, url);
                        continue;
                    } else {
                        log_global_crawl_outcome(
                            subsys,
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
                            "Terminal HTTP status",
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
}

/// RFC 9309 longest-match `robots.txt` evaluator supporting `User-agent: *` and specific bot tokens.
pub fn is_path_allowed_by_robots(robots_txt: &str, bot_token: &str, path_and_query: &str) -> bool {
    let bot_low = bot_token.to_ascii_lowercase();
    let mut current_agents: Vec<String> = Vec::new();
    let mut saw_rule_in_group = false;

    let mut specific_rules: Vec<(bool, String)> = Vec::new();
    let mut wildcard_rules: Vec<(bool, String)> = Vec::new();

    for raw_line in robots_txt.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();

        if key == "user-agent" {
            if saw_rule_in_group {
                current_agents.clear();
                saw_rule_in_group = false;
            }
            current_agents.push(val.to_ascii_lowercase());
        } else if key == "allow" || key == "disallow" {
            saw_rule_in_group = true;
            if val.is_empty() {
                // Empty Disallow means allow everything
                continue;
            }
            let is_allow = key == "allow";
            let matches_specific = current_agents
                .iter()
                .any(|a| a != "*" && bot_low.contains(a.as_str()));
            let matches_wildcard = current_agents.iter().any(|a| a == "*");

            if matches_specific {
                specific_rules.push((is_allow, val.to_string()));
            } else if matches_wildcard {
                wildcard_rules.push((is_allow, val.to_string()));
            }
        }
    }

    let rules = if !specific_rules.is_empty() {
        &specific_rules
    } else {
        &wildcard_rules
    };

    let mut best_len: usize = 0;
    let mut best_allow = true;

    for (allow, pattern) in rules {
        if robots_pattern_matches(pattern, path_and_query) {
            let pat_len = pattern.len();
            if pat_len > best_len || (pat_len == best_len && *allow) {
                best_len = pat_len;
                best_allow = *allow;
            }
        }
    }

    best_allow
}

fn robots_pattern_matches(pattern: &str, target_path: &str) -> bool {
    let anchored_end = pattern.ends_with('$');
    let clean_pat = if anchored_end {
        &pattern[..pattern.len() - 1]
    } else {
        pattern
    };

    if !clean_pat.contains('*') {
        if anchored_end {
            return target_path == clean_pat;
        }
        return target_path.starts_with(clean_pat);
    }

    let parts: Vec<&str> = clean_pat.split('*').collect();
    let mut pos = 0usize;
    for (idx, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if idx == 0 {
            if !target_path.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if let Some(found) = target_path[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }

    if anchored_end {
        pos == target_path.len()
    } else {
        true
    }
}

// ============================================================================
// 1. GOOGLE SCHOLAR CRAWLER (LOCAL HTML + SERPAPI + POLITE DIRECT HTML SERP)
// ============================================================================
pub fn crawl_google_scholar(
    crawler: &mut PoliteWebCrawler,
    req: &SearchRequest,
) -> Result<(Vec<PaperRecord>, Vec<String>), String> {
    let mut warnings = Vec::new();
    let mut all_hits: Vec<ScholarSerpHit> = Vec::new();

    // A. Offline saved Google Scholar HTML files (zero network / zero CAPTCHA)
    for local_path in &req.crawl.local_html_paths {
        let trimmed = local_path.trim();
        if trimmed.is_empty() {
            continue;
        }
        match fs::read_to_string(Path::new(trimmed)) {
            Ok(html) => match parse_google_scholar_html(&html) {
                Ok(mut hits) => all_hits.append(&mut hits),
                Err(reason) => warnings.push(format!(
                    "Local HTML '{}' notice: {}",
                    trimmed,
                    reason.message()
                )),
            },
            Err(e) => warnings.push(format!("Could not read local HTML '{}': {}", trimmed, e)),
        }
    }

    // B. Structured SerpApi Google Scholar engine if SERPAPI_KEY is configured
    if all_hits.len() < req.max_per_provider {
        if let Some(api_key) = crawler.serpapi_key.clone() {
            match crawl_google_scholar_serpapi(crawler, req, &api_key) {
                Ok(mut hits) => {
                    all_hits.append(&mut hits);
                }
                Err(e) => warnings.push(format!("Google Scholar SerpApi fallback warning: {}", e)),
            }
        }
    }

    // C. Direct Polite HTML SERP Crawling (scholar.google.com/scholar)
    if all_hits.is_empty() && req.crawl.local_html_paths.is_empty() && crawler.serpapi_key.is_none() {
        let max_pages = req.crawl.max_pages.clamp(1, 10);
        for page in 0..max_pages {
            if all_hits.len() >= req.max_per_provider {
                break;
            }
            let start = page * 10;
            let eff_q = build_effective_query(req);
            let path_and_query = if !req.crawl.scholar_cites_cluster_id.trim().is_empty() {
                let mut pq = format!(
                    "/scholar?hl=en&cites={}&start={}",
                    percent_encode(req.crawl.scholar_cites_cluster_id.trim()),
                    start
                );
                if !eff_q.trim().is_empty() {
                    pq.push_str("&as_epq=");
                    pq.push_str(&percent_encode(eff_q.trim()));
                }
                if let Some(ymin) = req.year_min {
                    pq.push_str(&format!("&as_ylo={}", ymin));
                }
                if let Some(ymax) = req.year_max {
                    pq.push_str(&format!("&as_yhi={}", ymax));
                }
                pq
            } else {
                let mut pq = format!(
                    "/scholar?hl=en&q={}&start={}",
                    percent_encode(eff_q.trim()),
                    start
                );
                if let Some(ymin) = req.year_min {
                    pq.push_str(&format!("&as_ylo={}", ymin));
                }
                if let Some(ymax) = req.year_max {
                    pq.push_str(&format!("&as_yhi={}", ymax));
                }
                pq
            };

            if req.crawl.respect_robots_txt
                && !crawler.check_robots_allowed("https://scholar.google.com", &path_and_query)
            {
                warnings.push(
                    "Google Scholar robots.txt disallows automated '/scholar?' queries. Supply saved SERP .html files, set SERPAPI_KEY, or pass --ignore-robots (or uncheck 'Respect robots.txt' in GUI) for authorized crawling."
                        .to_string(),
                );
                break;
            }

            if page > 0 {
                crawler.sleep_polite(req.crawl.pace_ms, req.crawl.jitter_ms, page);
            }

            let full_url = format!("https://scholar.google.com{}", path_and_query);
            match crawler.get_html(&full_url, true) {
                Ok(html) => match parse_google_scholar_html(&html) {
                    Ok(mut page_hits) => {
                        if page_hits.is_empty() {
                            break;
                        }
                        all_hits.append(&mut page_hits);
                    }
                    Err(block) => {
                        warnings.push(format!("Google Scholar crawl halted on page {}: {}", page + 1, block.message()));
                        break;
                    }
                },
                Err(block) => {
                    warnings.push(format!("Google Scholar crawl halted on page {}: {}", page + 1, block.message()));
                    break;
                }
            }
        }
    }

    all_hits.truncate(req.max_per_provider.max(1));
    let papers: Vec<PaperRecord> = all_hits
        .into_iter()
        .enumerate()
        .map(|(i, h)| scholar_hit_to_paper_record(h, i, &req.crawl.scholar_cites_cluster_id))
        .collect();

    Ok((papers, warnings))
}

fn crawl_google_scholar_serpapi(
    crawler: &PoliteWebCrawler,
    req: &SearchRequest,
    api_key: &str,
) -> Result<Vec<ScholarSerpHit>, String> {
    let mut out = Vec::new();
    let max_pages = req.crawl.max_pages.clamp(1, 5);
    for page in 0..max_pages {
        if out.len() >= req.max_per_provider {
            break;
        }
        let start = page * 10;
        let mut url = format!(
            "https://serpapi.com/search.json?engine=google_scholar&hl=en&num=10&start={}&api_key={}",
            start,
            percent_encode(api_key)
        );
        if !req.crawl.scholar_cites_cluster_id.trim().is_empty() {
            url.push_str("&cites=");
            url.push_str(&percent_encode(req.crawl.scholar_cites_cluster_id.trim()));
        } else {
            let eff_q = build_effective_query(req);
            url.push_str("&q=");
            url.push_str(&percent_encode(eff_q.trim()));
        }
        if let Some(ymin) = req.year_min {
            url.push_str(&format!("&as_ylo={}", ymin));
        }
        if let Some(ymax) = req.year_max {
            url.push_str(&format!("&as_yhi={}", ymax));
        }

        let val = crawler.get_json(&url)?;
        let Some(items) = val.get("organic_results").and_then(|v| v.as_array()) else {
            break;
        };
        if items.is_empty() {
            break;
        }

        for item in items {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if title.is_empty() {
                continue;
            }
            let result_id = item
                .get("result_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let link = item
                .get("link")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let snippet = item
                .get("snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let summary = item
                .get("publication_info")
                .and_then(|p| p.get("summary"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let (authors, venue, year) = parse_gs_byline(summary);

            let cited_by = item
                .get("inline_links")
                .and_then(|il| il.get("cited_by"))
                .and_then(|cb| cb.get("total"))
                .and_then(|v| v.as_u64());
            let cites_id = item
                .get("inline_links")
                .and_then(|il| il.get("cited_by"))
                .and_then(|cb| cb.get("cites_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut pdf_url = String::new();
            if let Some(res_arr) = item.get("resources").and_then(|v| v.as_array()) {
                for r in res_arr {
                    let fmt = r.get("file_format").and_then(|v| v.as_str()).unwrap_or("");
                    let rlink = r.get("link").and_then(|v| v.as_str()).unwrap_or("");
                    if fmt.eq_ignore_ascii_case("PDF") || rlink.to_ascii_lowercase().ends_with(".pdf")
                    {
                        pdf_url = rlink.to_string();
                        break;
                    }
                }
            }

            let combined_urls = format!("{} {} {}", link, pdf_url, snippet);
            out.push(ScholarSerpHit {
                cluster_id: result_id,
                title,
                authors,
                venue,
                year,
                url: link,
                pdf_url,
                snippet,
                cited_by_count: cited_by,
                cites_cluster_id: cites_id,
                doi: extract_doi_from_text(&combined_urls),
                arxiv_id: extract_arxiv_id_from_text(&combined_urls),
                pmcid: extract_pmcid_from_text(&combined_urls),
            });
        }
    }
    Ok(out)
}

/// Pure-Rust parser for Google Scholar HTML search & `cites=` pages.
pub fn parse_google_scholar_html(html: &str) -> Result<Vec<ScholarSerpHit>, CrawlBlockReason> {
    let lower = html.to_ascii_lowercase();
    if lower.contains("gs_captcha_ccl")
        || lower.contains("id=\"gs_captcha_f\"")
        || lower.contains("unusual traffic from your computer network")
        || (lower.contains("g-recaptcha") && !lower.contains("gs_ri"))
    {
        return Err(CrawlBlockReason::CaptchaOrRateLimit(
            "Google Scholar returned a CAPTCHA challenge page. Increase crawl pace_ms, configure proxy/cookie, set SERPAPI_KEY, or import saved .html/.csv files.".to_string(),
        ));
    }

    let mut hits = Vec::new();
    let markers: Vec<usize> = html
        .match_indices("class=\"gs_r")
        .filter(|&(idx, _)| matches!(html.as_bytes().get(idx + 11), Some(b' ') | Some(b'"')))
        .map(|(idx, _)| idx)
        .collect();

    for (m_pos, &start_idx) in markers.iter().enumerate() {
        let block_start = html[..start_idx].rfind('<').unwrap_or(start_idx);
        let block_end = markers
            .get(m_pos + 1)
            .copied()
            .unwrap_or_else(|| html.len());
        let block = &html[block_start..block_end];

        if !block.contains("gs_ri") && !block.contains("gs_rt") {
            continue;
        }

        let cluster_id = extract_attr_value(block, "data-cid").unwrap_or_default();

        // Extract optional right-rail [PDF] link from gs_or_ggsm or gs_ggs
        let mut pdf_url = String::new();
        if let Some(gg_idx) = block.find("gs_or_ggsm").or_else(|| block.find("gs_ggs")) {
            let gg_slice = &block[gg_idx..];
            if let Some(href) = extract_first_href(gg_slice) {
                if href.to_ascii_lowercase().contains("pdf") || gg_slice.contains("[PDF]") {
                    pdf_url = decode_html_entities(&href);
                }
            }
        }

        // Extract title and main outbound URL from <h3 class="gs_rt"...>...</h3>
        let (title, main_url) = if let Some(rt_idx) = block.find("gs_rt") {
            let after_rt = &block[rt_idx..];
            let end_h3 = after_rt.find("</h3>").unwrap_or(after_rt.len());
            let h3_inner = &after_rt[..end_h3];
            let href = extract_first_href(h3_inner)
                .map(|h| decode_html_entities(&h))
                .unwrap_or_default();
            let gt = h3_inner.find('>').map(|i| i + 1).unwrap_or(0);
            let raw_t = strip_html_tags(&h3_inner[gt..]);
            let cleaned_t = clean_gs_title_badges(&raw_t);
            (cleaned_t, href)
        } else {
            continue;
        };

        if title.is_empty() {
            continue;
        }

        // Extract author, venue, year from <div class="gs_a">...</div>
        let byline_raw = extract_div_by_class(block, "gs_a");
        let (authors, venue, year) = parse_gs_byline(&strip_html_tags(&byline_raw));

        // Extract snippet from <div class="gs_rs">...</div>
        let snippet = strip_html_tags(&extract_div_by_class(block, "gs_rs"));

        // Extract Cited by N & cites=CLUSTER_ID from <div class="gs_fl">...</div>
        let fl_html = extract_div_by_class(block, "gs_fl");
        let (cited_by_count, cites_cluster_id) = parse_gs_cited_by(&fl_html);

        let inspect_blob = format!("{} {} {}", main_url, pdf_url, snippet);
        let doi = extract_doi_from_text(&inspect_blob);
        let arxiv_id = extract_arxiv_id_from_text(&inspect_blob);
        let pmcid = extract_pmcid_from_text(&inspect_blob);

        hits.push(ScholarSerpHit {
            cluster_id,
            title,
            authors,
            venue,
            year,
            url: main_url,
            pdf_url,
            snippet,
            cited_by_count,
            cites_cluster_id,
            doi,
            arxiv_id,
            pmcid,
        });
    }

    Ok(hits)
}

fn scholar_hit_to_paper_record(
    hit: ScholarSerpHit,
    idx: usize,
    parent_cites_cluster: &str,
) -> PaperRecord {
    let combined = format!("{} {} {}", hit.title, hit.venue, hit.snippet);
    let disc = Discipline::infer_from_text(&combined);
    let mut concepts = infer_concepts_from_text(&combined, disc);
    let mut terms = concepts.clone();
    if !hit.cites_cluster_id.is_empty() {
        terms.push(format!("gs_cites:{}", hit.cites_cluster_id));
    }

    let first_surname = hit
        .authors
        .split(',')
        .next()
        .unwrap_or("scholar")
        .split_whitespace()
        .last()
        .unwrap_or("scholar")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();

    let id = if !hit.cluster_id.is_empty() {
        format!(
            "{}{}_{}",
            if first_surname.is_empty() { "gs" } else { &first_surname },
            hit.year,
            &hit.cluster_id[..hit.cluster_id.len().min(6)].to_ascii_lowercase()
        )
    } else {
        format!("gs_{}_{}", hit.year, idx + 1)
    };

    let oa_status = if !hit.arxiv_id.is_empty() {
        OaStatus::Arxiv
    } else if !hit.pdf_url.is_empty() {
        OaStatus::Green
    } else {
        OaStatus::Unknown
    };

    let mut raw_refs = Vec::new();
    if !parent_cites_cluster.trim().is_empty() {
        raw_refs.push(format!("GS_CLUSTER:{}", parent_cites_cluster.trim()));
    }

    PaperRecord {
        id,
        title: hit.title,
        authors: if hit.authors.is_empty() {
            "Google Scholar Indexed Author".to_string()
        } else {
            hit.authors
        },
        year: hit.year,
        venue: if hit.venue.is_empty() {
            "Google Scholar Indexed Venue".to_string()
        } else {
            hit.venue
        },
        disc,
        doi: hit.doi,
        arxiv_id: hit.arxiv_id,
        openalex_id: String::new(),
        s2_id: String::new(),
        pmcid: hit.pmcid,
        source_url: if !hit.url.is_empty() {
            hit.url
        } else if !hit.cites_cluster_id.is_empty() {
            format!("https://scholar.google.com/scholar?cites={}", hit.cites_cluster_id)
        } else {
            "https://scholar.google.com".to_string()
        },
        oa_pdf_url: hit.pdf_url,
        oa_status,
        external_citations: hit.cited_by_count,
        abstract_text: hit.snippet,
        terms,
        concepts: {
            concepts.truncate(4);
            concepts
        },
        local_pdf_path: None,
        pdf_sha256: None,
        pdf_bytes: None,
        raw_references: raw_refs,
        retraction: Default::default(),
        datacite_links: Vec::new(),
        bio_entities: Vec::new(),
    }
}

// ============================================================================
// 2. PUBMED / NCBI ENTREZ E-UTILITIES CRAWLER (esearch + esummary + elink)
// ============================================================================
pub fn build_pubmed_term(req: &SearchRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    match parse_identifier_lookup(&req.identifier_lookup) {
        Some(IdentifierLookupKind::Pmid(pmid)) => parts.push(format!("{}[uid]", pmid)),
        Some(IdentifierLookupKind::Doi(doi)) => parts.push(format!("\"{}\"[DOI]", doi)),
        Some(IdentifierLookupKind::Orcid(orcid)) => parts.push(format!("{}[auid]", orcid)),
        _ => {}
    }
    if !req.title_words.trim().is_empty() {
        parts.push(format!("({})[Title]", req.title_words.trim()));
    }
    if !req.authors.trim().is_empty() {
        parts.push(format!("{}[Author]", req.authors.trim()));
    }
    if !req.publication_venue.trim().is_empty() {
        parts.push(format!("\"{}\"[Journal]", req.publication_venue.trim()));
    }
    if !req.query.trim().is_empty() {
        parts.push(format!("({})", req.query.trim()));
    }
    if !req.keywords_all.trim().is_empty() {
        parts.push(format!("({})", req.keywords_all.trim()));
    }
    let mut term = if parts.is_empty() {
        build_effective_query(req)
    } else {
        parts.join(" AND ")
    };
    let excludes: Vec<&str> = req.exclude_words.split_whitespace().collect();
    if !excludes.is_empty() {
        term = format!("({}) NOT ({})", term, excludes.join(" OR "));
    }
    if let (Some(ymin), Some(ymax)) = (req.year_min, req.year_max) {
        term = format!("({}) AND {}:{}[dp]", term, ymin, ymax);
    } else if let Some(ymin) = req.year_min {
        term = format!("({}) AND {}:3000[dp]", term, ymin);
    } else if let Some(ymax) = req.year_max {
        term = format!("({}) AND 1800:{}[dp]", term, ymax);
    }
    term
}

pub fn crawl_pubmed_entrez(
    crawler: &PoliteWebCrawler,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let limit = req.max_per_provider.clamp(1, 100);
    let term = build_pubmed_term(req);

    let mut search_url = format!(
        "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi?db=pubmed&retmode=json&retmax={}&sort=relevance&email={}&term={}",
        limit,
        percent_encode(&crawler.polite_email),
        percent_encode(&term)
    );
    if let Some(ref k) = crawler.ncbi_api_key {
        search_url.push_str("&api_key=");
        search_url.push_str(&percent_encode(k));
    }

    let search_json = crawler.get_json(&search_url)?;
    let Some(id_list) = search_json
        .get("esearchresult")
        .and_then(|r| r.get("idlist"))
        .and_then(|v| v.as_array())
    else {
        return Ok(Vec::new());
    };

    let pmids: Vec<String> = id_list
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    if pmids.is_empty() {
        return Ok(Vec::new());
    }

    crawler.sleep_polite(340, 80, 1);

    let joined_ids = pmids.join(",");
    let mut sum_url = format!(
        "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi?db=pubmed&retmode=json&id={}&email={}",
        percent_encode(&joined_ids),
        percent_encode(&crawler.polite_email)
    );
    if let Some(ref k) = crawler.ncbi_api_key {
        sum_url.push_str("&api_key=");
        sum_url.push_str(&percent_encode(k));
    }

    let sum_json = crawler.get_json(&sum_url)?;
    let Some(result_obj) = sum_json.get("result").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };

    // Optional ELink reference harvest for intra-corpus citation linking
    let mut pmid_refs_map: HashMap<String, Vec<String>> = HashMap::new();
    if req.snowball_hops >= 1 {
        crawler.sleep_polite(340, 80, 2);
        let mut elink_url = format!(
            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/elink.fcgi?dbfrom=pubmed&db=pubmed&linkname=pubmed_pubmed_refs&retmode=json&email={}",
            percent_encode(&crawler.polite_email)
        );
        for pmid in pmids.iter().take(40) {
            elink_url.push_str("&id=");
            elink_url.push_str(&percent_encode(pmid));
        }
        if let Some(ref k) = crawler.ncbi_api_key {
            elink_url.push_str("&api_key=");
            elink_url.push_str(&percent_encode(k));
        }
        if let Ok(el_json) = crawler.get_json(&elink_url) {
            if let Some(linksets) = el_json.get("linksets").and_then(|v| v.as_array()) {
                for ls in linksets {
                    let src_pmid = ls
                        .get("ids")
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_u64().map(|n| n.to_string())))
                        .unwrap_or_default();
                    if src_pmid.is_empty() {
                        continue;
                    }
                    let mut refs = Vec::new();
                    if let Some(dbs) = ls.get("linksetdbs").and_then(|v| v.as_array()) {
                        for db_entry in dbs {
                            if let Some(links) = db_entry.get("links").and_then(|v| v.as_array()) {
                                for lk in links {
                                    if let Some(s) = lk.as_str() {
                                        refs.push(format!("PMID:{}", s));
                                    } else if let Some(n) = lk.as_u64() {
                                        refs.push(format!("PMID:{}", n));
                                    }
                                }
                            }
                        }
                    }
                    if !refs.is_empty() {
                        pmid_refs_map.insert(src_pmid, refs);
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for pmid in &pmids {
        let Some(doc) = result_obj.get(pmid) else {
            continue;
        };
        let title = doc
            .get("title")
            .and_then(|v| v.as_str())
            .map(strip_html_tags)
            .unwrap_or_default()
            .trim_end_matches('.')
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let pubdate = doc.get("pubdate").and_then(|v| v.as_str()).unwrap_or("");
        let year = extract_year_from_str(pubdate).unwrap_or(2024);
        let venue = doc
            .get("fulljournalname")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| doc.get("source").and_then(|v| v.as_str()))
            .unwrap_or("PubMed Indexed Journal")
            .to_string();

        let authors = doc
            .get("authors")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .take(6)
                    .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "PubMed Author".to_string());

        let mut doi = String::new();
        let mut pmcid = String::new();
        if let Some(aids) = doc.get("articleids").and_then(|v| v.as_array()) {
            for aid in aids {
                let idtype = aid.get("idtype").and_then(|v| v.as_str()).unwrap_or("");
                let val = aid.get("value").and_then(|v| v.as_str()).unwrap_or("");
                if idtype == "doi" && !val.is_empty() {
                    doi = normalize_doi(val);
                } else if idtype == "pmc" && !val.is_empty() {
                    pmcid = if val.starts_with("PMC") {
                        val.to_string()
                    } else {
                        format!("PMC{}", val)
                    };
                }
            }
        }

        let oa_pdf_url = if !pmcid.is_empty() {
            format!(
                "https://europepmc.org/backend/ptpmcrender.fcgi?accid={}&blobtype=pdf",
                pmcid
            )
        } else {
            String::new()
        };
        let oa_status = if !pmcid.is_empty() {
            OaStatus::Green
        } else {
            OaStatus::Unknown
        };

        let combined = format!("{} {}", title, venue);
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);
        let mut terms = concepts.clone();
        terms.push(format!("pmid:{}", pmid));

        out.push(PaperRecord {
            id: format!("pmid_{}", pmid),
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
            source_url: format!("https://pubmed.ncbi.nlm.nih.gov/{}/", pmid),
            oa_pdf_url,
            oa_status,
            external_citations: None,
            abstract_text: String::new(),
            terms,
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: pmid_refs_map.remove(pmid).unwrap_or_default(),
            retraction: Default::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }

    Ok(out)
}

// ============================================================================
// 3. EUROPE PMC REST + CITATION GRAPH CRAWLER
// ============================================================================
pub fn build_europe_pmc_query(req: &SearchRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    match parse_identifier_lookup(&req.identifier_lookup) {
        Some(IdentifierLookupKind::Pmid(pmid)) => parts.push(format!("EXT_ID:{}", pmid)),
        Some(IdentifierLookupKind::Doi(doi)) => parts.push(format!("DOI:\"{}\"", doi)),
        Some(IdentifierLookupKind::Orcid(orcid)) => parts.push(format!("AUTHORID:\"{}\"", orcid)),
        _ => {}
    }
    if !req.title_words.trim().is_empty() {
        parts.push(format!("TITLE:\"{}\"", req.title_words.trim()));
    }
    if !req.authors.trim().is_empty() {
        parts.push(format!("AUTH:\"{}\"", req.authors.trim()));
    }
    if !req.publication_venue.trim().is_empty() {
        parts.push(format!("JOURNAL:\"{}\"", req.publication_venue.trim()));
    }
    if !req.query.trim().is_empty() {
        parts.push(format!("({})", req.query.trim()));
    }
    if !req.keywords_all.trim().is_empty() {
        parts.push(format!("({})", req.keywords_all.trim()));
    }
    let mut query = if parts.is_empty() {
        build_effective_query(req)
    } else {
        parts.join(" AND ")
    };
    let excludes: Vec<&str> = req.exclude_words.split_whitespace().collect();
    if !excludes.is_empty() {
        query = format!("({}) NOT ({})", query, excludes.join(" OR "));
    }
    if let (Some(ymin), Some(ymax)) = (req.year_min, req.year_max) {
        query = format!("({}) AND (PUB_YEAR:[{} TO {}])", query, ymin, ymax);
    }
    if req.oa_only {
        query = format!("({}) AND (OPEN_ACCESS:y)", query);
    }
    query
}

pub fn crawl_europe_pmc(
    crawler: &PoliteWebCrawler,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let page_size = req.max_per_provider.clamp(1, 100);
    let query = build_europe_pmc_query(req);

    let url = format!(
        "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={}&resultType=core&pageSize={}&format=json",
        percent_encode(&query),
        page_size
    );
    let val = crawler.get_json(&url)?;
    let Some(results) = val
        .get("resultList")
        .and_then(|rl| rl.get("result"))
        .and_then(|r| r.as_array())
    else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for item in results {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map(strip_html_tags)
            .unwrap_or_default()
            .trim_end_matches('.')
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let year = item
            .get("pubYear")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(2024);
        let doi = item
            .get("doi")
            .and_then(|v| v.as_str())
            .map(normalize_doi)
            .unwrap_or_default();
        let pmid = item
            .get("pmid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let pmcid = item
            .get("pmcid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let authors = item
            .get("authorString")
            .and_then(|v| v.as_str())
            .unwrap_or("Europe PMC Author")
            .to_string();
        let venue = item
            .get("journalInfo")
            .and_then(|j| j.get("journal"))
            .and_then(|j| j.get("title"))
            .and_then(|v| v.as_str())
            .or_else(|| item.get("bookOrReportDetails").and_then(|b| b.get("publisher")).and_then(|v| v.as_str()))
            .unwrap_or("Europe PMC")
            .to_string();
        let abstract_text = item
            .get("abstractText")
            .and_then(|v| v.as_str())
            .map(strip_html_tags)
            .unwrap_or_default();
        let cited_by = item.get("citedByCount").and_then(|v| v.as_u64());
        let is_oa = item
            .get("isOpenAccess")
            .and_then(|v| v.as_str())
            .map(|s| s == "Y")
            .unwrap_or(false);

        let oa_pdf_url = if !pmcid.is_empty() && is_oa {
            format!(
                "https://europepmc.org/backend/ptpmcrender.fcgi?accid={}&blobtype=pdf",
                pmcid
            )
        } else {
            String::new()
        };

        let id = if !pmid.is_empty() {
            format!("epmc_{}", pmid)
        } else if !pmcid.is_empty() {
            pmcid.to_ascii_lowercase()
        } else {
            format!("epmc_{}_{}", year, out.len() + 1)
        };

        let combined = format!("{} {} {}", title, venue, abstract_text);
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);
        let mut terms = concepts.clone();
        if !pmid.is_empty() {
            terms.push(format!("pmid:{}", pmid));
        }

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
            pmcid: pmcid.clone(),
            source_url: if !pmcid.is_empty() {
                format!("https://europepmc.org/article/PMC/{}", pmcid.trim_start_matches("PMC"))
            } else if !pmid.is_empty() {
                format!("https://europepmc.org/article/MED/{}", pmid)
            } else {
                "https://europepmc.org".to_string()
            },
            oa_pdf_url,
            oa_status: if is_oa { OaStatus::Gold } else { OaStatus::Unknown },
            external_citations: cited_by,
            abstract_text,
            terms,
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: Default::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }

    Ok(out)
}

// ============================================================================
// 4. DBLP COMPUTER SCIENCE BIBLIOGRAPHY CRAWLER
// ============================================================================
pub fn crawl_dblp(
    crawler: &PoliteWebCrawler,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let limit = req.max_per_provider.clamp(1, 100);
    let eff_q = build_effective_query(req);
    let url = format!(
        "https://dblp.org/search/publ/api?q={}&h={}&format=json",
        percent_encode(eff_q.trim()),
        limit
    );
    let val = crawler.get_json(&url)?;
    let Some(hits) = val
        .get("result")
        .and_then(|r| r.get("hits"))
        .and_then(|h| h.get("hit"))
        .and_then(|v| v.as_array())
    else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for (idx, h) in hits.iter().enumerate() {
        let Some(info) = h.get("info") else {
            continue;
        };
        let title = info
            .get("title")
            .and_then(|v| v.as_str())
            .map(strip_html_tags)
            .unwrap_or_default()
            .trim_end_matches('.')
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let year = info
            .get("year")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(2024);

        // DBLP authors.author can be either an array or a single object
        let authors = match info.get("authors").and_then(|a| a.get("author")) {
            Some(Value::Array(arr)) => arr
                .iter()
                .take(6)
                .filter_map(|item| {
                    item.as_str()
                        .or_else(|| item.get("text").and_then(|t| t.as_str()))
                })
                .collect::<Vec<_>>()
                .join(", "),
            Some(Value::Object(obj)) => obj
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("DBLP Author")
                .to_string(),
            Some(Value::String(s)) => s.clone(),
            _ => "DBLP Author".to_string(),
        };

        let venue = info
            .get("venue")
            .and_then(|v| {
                v.as_str().map(|s| s.to_string()).or_else(|| {
                    v.as_array()
                        .and_then(|a| a.first())
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string())
                })
            })
            .unwrap_or_else(|| "DBLP Venue".to_string());

        let doi = info
            .get("doi")
            .and_then(|v| v.as_str())
            .map(normalize_doi)
            .unwrap_or_default();

        let ee = match info.get("ee") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(arr)) => arr
                .first()
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        };
        let dblp_url = info
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let arxiv_id = extract_arxiv_id_from_text(&ee);
        let oa_pdf_url = if !arxiv_id.is_empty() {
            format!("https://arxiv.org/pdf/{}.pdf", arxiv_id)
        } else if ee.to_ascii_lowercase().ends_with(".pdf") {
            ee.clone()
        } else {
            String::new()
        };
        let oa_status = if !arxiv_id.is_empty() {
            OaStatus::Arxiv
        } else if info
            .get("access")
            .and_then(|v| v.as_str())
            .map(|s| s == "open")
            .unwrap_or(false)
        {
            OaStatus::Gold
        } else {
            OaStatus::Unknown
        };

        let combined = format!("{} {}", title, venue);
        let disc = Discipline::infer_from_text(&combined);
        let concepts = infer_concepts_from_text(&combined, disc);

        out.push(PaperRecord {
            id: format!("dblp_{}_{}", year, idx + 1),
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
            source_url: if !ee.is_empty() { ee } else { dblp_url },
            oa_pdf_url,
            oa_status,
            external_citations: None,
            abstract_text: String::new(),
            terms: concepts.clone(),
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: Default::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });
    }

    Ok(out)
}

// ============================================================================
// 5. BIORXIV & MEDRXIV CSHL PREPRINT CRAWLER
// ============================================================================
pub fn crawl_biorxiv_medrxiv(
    crawler: &PoliteWebCrawler,
    req: &SearchRequest,
) -> Result<Vec<PaperRecord>, String> {
    let eff_q = build_effective_query(req);
    let query_tokens: Vec<String> = eff_q
        .split_whitespace()
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| t.len() >= 3)
        .collect();

    let start_year = req.year_max.unwrap_or(2026);
    let interval = format!("{}-01-01/{}-12-31", start_year, start_year);

    let mut out = Vec::new();
    for server in ["biorxiv", "medrxiv"] {
        if out.len() >= req.max_per_provider {
            break;
        }
        let url = format!(
            "https://api.biorxiv.org/details/{}/{}/0/json",
            server, interval
        );
        let Ok(val) = crawler.get_json(&url) else {
            continue;
        };
        let Some(collection) = val.get("collection").and_then(|v| v.as_array()) else {
            continue;
        };

        for item in collection {
            if out.len() >= req.max_per_provider {
                break;
            }
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let abstract_text = item
                .get("abstract")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let category = item
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            let hay = format!("{} {} {}", title, category, abstract_text).to_ascii_lowercase();
            if !query_tokens.is_empty() && !query_tokens.iter().any(|tok| hay.contains(tok)) {
                continue;
            }

            let doi = item
                .get("doi")
                .and_then(|v| v.as_str())
                .map(normalize_doi)
                .unwrap_or_default();
            if doi.is_empty() {
                continue;
            }
            let version = item
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("1");
            let date_str = item.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let year = extract_year_from_str(date_str).unwrap_or(start_year);
            let authors = item
                .get("authors")
                .and_then(|v| v.as_str())
                .unwrap_or("Preprint Author")
                .replace("; ", ", ");
            let pub_doi = item
                .get("published")
                .and_then(|v| v.as_str())
                .filter(|s| !s.eq_ignore_ascii_case("NA") && !s.is_empty())
                .map(normalize_doi)
                .unwrap_or_default();

            let pdf_url = format!(
                "https://www.{}.org/content/{}v{}.full.pdf",
                server, doi, version
            );
            let src_url = format!("https://www.{}.org/content/{}v{}", server, doi, version);
            let disc = Discipline::infer_from_text(&hay);
            let concepts = infer_concepts_from_text(&hay, disc);
            let mut raw_refs = Vec::new();
            if !pub_doi.is_empty() {
                raw_refs.push(format!("DOI:{}", pub_doi));
            }

            out.push(PaperRecord {
                id: format!("{}_{}_{}", server, year, out.len() + 1),
                title,
                authors,
                year,
                venue: format!("{} ({})", server, category),
                disc,
                doi,
                arxiv_id: String::new(),
                openalex_id: String::new(),
                s2_id: String::new(),
                pmcid: String::new(),
                source_url: src_url,
                oa_pdf_url: pdf_url,
                oa_status: OaStatus::Green,
                external_citations: None,
                abstract_text,
                terms: concepts.clone(),
                concepts,
                local_pdf_path: None,
                pdf_sha256: None,
                pdf_bytes: None,
                raw_references: raw_refs,
                retraction: Default::default(),
                datacite_links: Vec::new(),
                bio_entities: Vec::new(),
            });
        }
    }

    Ok(out)
}

// ============================================================================
// 6. OPENCITATIONS INDEX V2 & NIH ICITE CITATION GRAPH HYDRATOR
// ============================================================================
pub fn hydrate_opencitations_and_icite(
    crawler: &PoliteWebCrawler,
    papers: &mut [PaperRecord],
    filter_self_citations: bool,
) -> (Vec<CitationEdge>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut extra_edges = Vec::new();

    // Map DOIs and PMIDs inside our corpus
    let mut doi_to_id: HashMap<String, String> = HashMap::new();
    let mut pmid_to_idx: HashMap<String, usize> = HashMap::new();
    for (idx, p) in papers.iter().enumerate() {
        if !p.doi.is_empty() {
            doi_to_id.insert(normalize_doi(&p.doi), p.id.clone());
        }
        for t in &p.terms {
            if let Some(pmid) = t.strip_prefix("pmid:") {
                pmid_to_idx.insert(pmid.to_string(), idx);
            }
        }
        if let Some(pmid) = p.id.strip_prefix("pmid_").or_else(|| p.id.strip_prefix("epmc_")) {
            if pmid.chars().all(|c| c.is_ascii_digit()) {
                pmid_to_idx.insert(pmid.to_string(), idx);
            }
        }
    }

    // A. NIH iCite batch hydration (up to 200 PMIDs in 1 keyless call)
    if !pmid_to_idx.is_empty() {
        let batch_pmids: Vec<String> = pmid_to_idx.keys().take(180).cloned().collect();
        let icite_url = format!(
            "https://icite.od.nih.gov/api/pubs?pmids={}&format=json",
            batch_pmids.join(",")
        );
        match crawler.get_json(&icite_url) {
            Ok(val) => {
                if let Some(data) = val.get("data").and_then(|v| v.as_array()) {
                    for entry in data {
                        let pmid_s = entry
                            .get("pmid")
                            .and_then(|v| {
                                v.as_u64()
                                    .map(|n| n.to_string())
                                    .or_else(|| v.as_str().map(|s| s.to_string()))
                            })
                            .unwrap_or_default();
                        let Some(&p_idx) = pmid_to_idx.get(&pmid_s) else {
                            continue;
                        };
                        if let Some(cc) = entry.get("citation_count").and_then(|v| v.as_u64()) {
                            if papers[p_idx].external_citations.unwrap_or(0) < cc {
                                papers[p_idx].external_citations = Some(cc);
                            }
                        }
                        if let Some(rcr) = entry.get("relative_citation_ratio").and_then(|v| v.as_f64()) {
                            let tag = format!("rcr:{:.2}", rcr);
                            if !papers[p_idx].terms.contains(&tag) {
                                papers[p_idx].terms.push(tag);
                            }
                        }
                        if let Some(refs) = entry.get("references").and_then(|v| v.as_array()) {
                            let src_id = papers[p_idx].id.clone();
                            for r in refs {
                                let tgt_pmid = r
                                    .as_u64()
                                    .map(|n| n.to_string())
                                    .or_else(|| r.as_str().map(|s| s.to_string()))
                                    .unwrap_or_default();
                                if let Some(&tgt_idx) = pmid_to_idx.get(&tgt_pmid) {
                                    let tgt_id = papers[tgt_idx].id.clone();
                                    if src_id != tgt_id {
                                        extra_edges.push(CitationEdge {
                                            source: src_id.clone(),
                                            target: tgt_id,
                                            provenance: "nih_icite_occ".to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => warnings.push(format!("NIH iCite hydration skipped: {}", e)),
        }
    }

    // B. OpenCitations Index v2 outgoing references hydration (capped at 20 DOIs for responsiveness)
    let candidate_dois: Vec<(String, String)> = papers
        .iter()
        .filter(|p| !p.doi.is_empty())
        .take(20)
        .map(|p| (normalize_doi(&p.doi), p.id.clone()))
        .collect();

    let mut seen_pairs: HashSet<(String, String)> = extra_edges
        .iter()
        .map(|e| (e.source.clone(), e.target.clone()))
        .collect();

    for (step, (doi, src_id)) in candidate_dois.iter().enumerate() {
        if step > 0 {
            crawler.sleep_polite(340, 60, step);
        }
        let oc_url = format!(
            "https://api.opencitations.net/index/v2/references/doi:{}",
            percent_encode(doi)
        );
        if let Ok(Value::Array(arr)) = crawler.get_json(&oc_url) {
            for row in arr {
                if filter_self_citations {
                    let author_sc = row.get("author_sc").and_then(|v| v.as_str()).unwrap_or("no");
                    let journal_sc = row.get("journal_sc").and_then(|v| v.as_str()).unwrap_or("no");
                    if author_sc.eq_ignore_ascii_case("yes") || journal_sc.eq_ignore_ascii_case("yes")
                    {
                        continue;
                    }
                }
                let cited_field = row.get("cited").and_then(|v| v.as_str()).unwrap_or("");
                let cited_doi = extract_doi_from_opencitations_field(cited_field);
                if !cited_doi.is_empty() {
                    if let Some(tgt_id) = doi_to_id.get(&cited_doi) {
                        if src_id != tgt_id && seen_pairs.insert((src_id.clone(), tgt_id.clone())) {
                            extra_edges.push(CitationEdge {
                                source: src_id.clone(),
                                target: tgt_id.clone(),
                                provenance: "opencitations_v2".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    // C. OpenAIRE ScholExplorer v3 citation hydration for DOI records
    for (doi, src_id) in candidate_dois.iter().take(12) {
        let sx_url = format!(
            "https://api.scholexplorer.openaire.eu/v3/Links?sourcePid={}&page=0",
            percent_encode(doi)
        );
        if let Ok(val) = crawler.get_json(&sx_url) {
            let (refs, _dcs) = crate::search_providers::parse_scholexplorer_links_json(&val, doi);
            for r in refs {
                if let Some(cited_doi) = r.strip_prefix("DOI:") {
                    if let Some(tgt_id) = doi_to_id.get(cited_doi) {
                        if src_id != tgt_id && seen_pairs.insert((src_id.clone(), tgt_id.clone())) {
                            extra_edges.push(CitationEdge {
                                source: src_id.clone(),
                                target: tgt_id.clone(),
                                provenance: "openaire_scholexplorer_v3".to_string(),
                            });
                        }
                    }
                } else if let Some(citer_doi) = r.strip_prefix("CITED_BY_DOI:") {
                    if let Some(citer_id) = doi_to_id.get(citer_doi) {
                        if citer_id != src_id && seen_pairs.insert((citer_id.clone(), src_id.clone())) {
                            extra_edges.push(CitationEdge {
                                source: citer_id.clone(),
                                target: src_id.clone(),
                                provenance: "openaire_scholexplorer_v3".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    (extra_edges, warnings)
}

fn extract_doi_from_opencitations_field(field: &str) -> String {
    for tok in field.split_whitespace() {
        if let Some(rest) = tok.strip_prefix("doi:") {
            return normalize_doi(rest);
        }
    }
    String::new()
}

// ============================================================================
// PURE-RUST HTML & BIBLIOGRAPHIC STRING HELPERS
// ============================================================================
fn extract_attr_value(src: &str, attr_name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pat = format!("{}={}", attr_name, quote);
        if let Some(pos) = src.find(&pat) {
            let rest = &src[pos + pat.len()..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn extract_first_href(src: &str) -> Option<String> {
    let mut search_slice = src;
    while let Some(a_pos) = search_slice.find("<a ") {
        let after_a = &search_slice[a_pos..];
        let tag_end = after_a.find('>').unwrap_or(after_a.len());
        let tag_header = &after_a[..tag_end];
        if let Some(h) = extract_attr_value(tag_header, "href") {
            if !h.starts_with("javascript:") && !h.starts_with('#') {
                return Some(h);
            }
        }
        search_slice = &after_a[tag_end..];
    }
    None
}

fn extract_div_by_class(block: &str, class_token: &str) -> String {
    let pat_dq = format!("class=\"{}", class_token);
    let pat_sq = format!("class='{}", class_token);
    let Some(pos) = block.find(&pat_dq).or_else(|| block.find(&pat_sq)) else {
        return String::new();
    };
    let after = &block[pos..];
    let Some(gt) = after.find('>') else {
        return String::new();
    };
    let inner = &after[gt + 1..];
    let mut depth = 1i32;
    let mut cursor = 0usize;
    while cursor < inner.len() {
        let next_open = inner[cursor..].find("<div");
        let next_close = inner[cursor..].find("</div>");
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                cursor += o + 4;
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return inner[..cursor + c].to_string();
                }
                cursor += c + 6;
            }
            _ => break,
        }
    }
    inner.to_string()
}

fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = decode_html_entities(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&#8230;", "...")
        .replace("&hellip;", "...")
        .replace("&#8211;", "-")
        .replace("&ndash;", "-")
}

fn clean_gs_title_badges(raw: &str) -> String {
    let mut s = raw.trim();
    loop {
        if let Some(rest) = s.strip_prefix('[') {
            if let Some(close) = rest.find(']') {
                s = rest[close + 1..].trim_start();
                continue;
            }
        }
        break;
    }
    s.to_string()
}

pub fn parse_gs_byline(byline: &str) -> (String, String, i32) {
    let normalized = byline.replace('\u{00a0}', " ").replace('\u{2013}', "-");
    let parts: Vec<&str> = normalized.split(" - ").collect();
    let authors = parts
        .first()
        .map(|s| s.trim().trim_end_matches("...").trim().to_string())
        .unwrap_or_default();

    let venue_part = parts.get(1).copied().unwrap_or("");
    let year = extract_year_from_str(venue_part)
        .or_else(|| extract_year_from_str(&normalized))
        .unwrap_or(2024);

    let venue = venue_part
        .split(',')
        .filter(|seg| extract_year_from_str(seg).is_none())
        .collect::<Vec<_>>()
        .join(",")
        .trim()
        .to_string();

    (authors, venue, year)
}

fn parse_gs_cited_by(fl_html: &str) -> (Option<u64>, String) {
    let mut cited_count: Option<u64> = None;
    let mut cites_id = String::new();

    let mut slice = fl_html;
    while let Some(a_pos) = slice.find("<a ") {
        let after = &slice[a_pos..];
        let Some(end_a) = after.find("</a>") else {
            break;
        };
        let a_block = &after[..end_a];
        if let Some(href) = extract_attr_value(a_block, "href") {
            let clean_href = decode_html_entities(&href);
            if let Some(c_pos) = clean_href.find("cites=") {
                let rest = &clean_href[c_pos + 6..];
                let id_part: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == ',')
                    .collect();
                if !id_part.is_empty() {
                    cites_id = id_part;
                }
                let inner_txt = strip_html_tags(a_block);
                let digits: String = inner_txt.chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<u64>() {
                    cited_count = Some(n);
                }
                break;
            }
        }
        slice = &after[end_a + 4..];
    }

    (cited_count, cites_id)
}

pub fn extract_year_from_str(s: &str) -> Option<i32> {
    let bytes = s.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    for i in 0..=bytes.len() - 4 {
        if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit()) {
            let prev_digit = i > 0 && bytes[i - 1].is_ascii_digit();
            let next_digit = i + 4 < bytes.len() && bytes[i + 4].is_ascii_digit();
            if !prev_digit && !next_digit {
                if let Ok(yr) = s[i..i + 4].parse::<i32>() {
                    if (1850..=2035).contains(&yr) {
                        return Some(yr);
                    }
                }
            }
        }
    }
    None
}

pub fn extract_doi_from_text(s: &str) -> String {
    let Some(pos) = s.find("10.") else {
        return String::new();
    };
    let rest = &s[pos..];
    let Some(slash) = rest.find('/') else {
        return String::new();
    };
    let registrant = &rest[3..slash];
    if registrant.len() < 4 || !registrant.chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    let raw_doi: String = rest
        .chars()
        .take_while(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '(' | ')' | ':' | ';')
        })
        .collect();
    let trimmed = raw_doi
        .trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ')'))
        .to_string();
    normalize_doi(&trimmed)
}

pub fn extract_arxiv_id_from_text(s: &str) -> String {
    let low = s.to_ascii_lowercase();
    for marker in ["arxiv.org/abs/", "arxiv.org/pdf/"] {
        if let Some(pos) = low.find(marker) {
            let rest = &s[pos + marker.len()..];
            let token: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            let cleaned = token.trim_end_matches(".pdf");
            let base = match cleaned.rfind('v') {
                Some(vp)
                    if vp > 4
                        && cleaned[vp + 1..].chars().all(|c| c.is_ascii_digit())
                        && !cleaned[vp + 1..].is_empty() =>
                {
                    &cleaned[..vp]
                }
                _ => cleaned,
            };
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    String::new()
}

pub fn extract_pmcid_from_text(s: &str) -> String {
    if let Some(pos) = s.find("PMC") {
        let digits: String = s[pos + 3..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.len() >= 5 {
            return format!("PMC{}", digits);
        }
    }
    String::new()
}
