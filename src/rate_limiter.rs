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

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

pub const MAX_AUDIT_LOG_ENTRIES: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrawlAuditOutcome {
    Allowed2xx,
    ThrottledThen2xx,
    Http429RateLimited,
    Http403OrCaptcha,
    RobotsDisallowed,
    CircuitBreakerSkipped,
    HttpOtherError,
    NetworkError,
}

impl CrawlAuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            CrawlAuditOutcome::Allowed2xx => "allowed_2xx",
            CrawlAuditOutcome::ThrottledThen2xx => "throttled_2xx",
            CrawlAuditOutcome::Http429RateLimited => "http_429_rate_limited",
            CrawlAuditOutcome::Http403OrCaptcha => "http_403_or_captcha",
            CrawlAuditOutcome::RobotsDisallowed => "robots_disallowed",
            CrawlAuditOutcome::CircuitBreakerSkipped => "circuit_breaker_skipped",
            CrawlAuditOutcome::HttpOtherError => "http_other_error",
            CrawlAuditOutcome::NetworkError => "network_error",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "allowed_2xx" | "ok" => CrawlAuditOutcome::Allowed2xx,
            "throttled_2xx" | "throttled" => CrawlAuditOutcome::ThrottledThen2xx,
            "http_429_rate_limited" | "429" => CrawlAuditOutcome::Http429RateLimited,
            "http_403_or_captcha" | "captcha" | "403" => CrawlAuditOutcome::Http403OrCaptcha,
            "robots_disallowed" | "robots" => CrawlAuditOutcome::RobotsDisallowed,
            "circuit_breaker_skipped" | "circuit_breaker" => {
                CrawlAuditOutcome::CircuitBreakerSkipped
            }
            "http_other_error" => CrawlAuditOutcome::HttpOtherError,
            _ => CrawlAuditOutcome::NetworkError,
        }
    }

    pub fn is_success(self) -> bool {
        matches!(
            self,
            CrawlAuditOutcome::Allowed2xx | CrawlAuditOutcome::ThrottledThen2xx
        )
    }

    pub fn is_blocked_or_throttled_429(self) -> bool {
        matches!(
            self,
            CrawlAuditOutcome::Http429RateLimited
                | CrawlAuditOutcome::Http403OrCaptcha
                | CrawlAuditOutcome::RobotsDisallowed
                | CrawlAuditOutcome::CircuitBreakerSkipped
        )
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            CrawlAuditOutcome::Allowed2xx => [52, 211, 153],         // Emerald
            CrawlAuditOutcome::ThrottledThen2xx => [56, 189, 248],   // Sky Cyan
            CrawlAuditOutcome::Http429RateLimited => [250, 204, 21], // Amber
            CrawlAuditOutcome::Http403OrCaptcha => [239, 68, 68],    // Crimson
            CrawlAuditOutcome::RobotsDisallowed => [251, 146, 60],   // Orange
            CrawlAuditOutcome::CircuitBreakerSkipped => [244, 63, 94],
            CrawlAuditOutcome::HttpOtherError => [167, 139, 250],
            CrawlAuditOutcome::NetworkError => [148, 163, 184],
        }
    }
}

/// Serializable per-domain token-bucket configuration & cumulative telemetry snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainBucketPolicy {
    pub domain: String,
    pub label: String,
    pub refill_rps: f64,
    pub burst_capacity: f64,
    pub min_spacing_ms: u64,
    pub jitter_ms: u64,
    pub enabled: bool,
    pub paused: bool,
    #[serde(default)]
    pub tokens_available: f64,
    #[serde(default)]
    pub cooldown_remaining_ms: u64,
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default)]
    pub ok_2xx: u64,
    #[serde(default)]
    pub throttled_waits: u64,
    #[serde(default)]
    pub total_wait_ms: u64,
    #[serde(default)]
    pub http_429_count: u64,
    #[serde(default)]
    pub http_403_captcha_count: u64,
    #[serde(default)]
    pub robots_blocked_count: u64,
    #[serde(default)]
    pub network_errors: u64,
    #[serde(default)]
    pub bytes_received: u64,
    #[serde(default)]
    pub ema_latency_ms: f64,
    #[serde(default)]
    pub last_status: u16,
    #[serde(default)]
    pub server_pool_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlAuditEntry {
    pub seq_id: u64,
    pub timestamp_utc: String,
    pub epoch_ms: u64,
    pub subsystem: String,
    pub domain: String,
    pub method: String,
    pub url: String,
    pub outcome: CrawlAuditOutcome,
    pub http_status: u16,
    pub token_wait_ms: u64,
    pub latency_ms: u64,
    pub bytes_received: u64,
    pub tokens_remaining: f64,
    pub retry_after_ms: Option<u64>,
    pub robots_status: String,
    pub note: String,
}

#[derive(Debug, Clone)]
pub enum AcquirePermit {
    Granted {
        domain: String,
        wait_ms: u64,
        tokens_after: f64,
    },
    CircuitOpen {
        domain: String,
        reason: String,
        remaining_ms: u64,
    },
}

struct RuntimeDomainBucket {
    policy: DomainBucketPolicy,
    tokens: f64,
    last_refill: Instant,
    next_allowed_at: Option<Instant>,
    cooldown_until: Option<Instant>,
}

impl RuntimeDomainBucket {
    fn from_policy(mut p: DomainBucketPolicy, now: Instant) -> Self {
        let cap = p.burst_capacity.max(1.0);
        p.burst_capacity = cap;
        p.refill_rps = p.refill_rps.clamp(0.05, 250.0);
        p.tokens_available = cap;
        Self {
            policy: p,
            tokens: cap,
            last_refill: now,
            next_allowed_at: None,
            cooldown_until: None,
        }
    }

    fn refill_at(&mut self, now: Instant, global_scale: f64) {
        let dt = now
            .checked_duration_since(self.last_refill)
            .unwrap_or(Duration::ZERO)
            .as_secs_f64();
        if dt > 0.0 {
            let eff_rps = (self.policy.refill_rps * global_scale.clamp(0.1, 10.0)).clamp(0.02, 500.0);
            self.tokens = (self.tokens + dt * eff_rps).min(self.policy.burst_capacity.max(1.0));
            self.last_refill = now;
        }
        self.policy.tokens_available = self.tokens;
        self.policy.cooldown_remaining_ms = self
            .cooldown_until
            .and_then(|u| u.checked_duration_since(now))
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
    }
}

pub struct DomainRateLimiter {
    buckets: HashMap<String, RuntimeDomainBucket>,
    ordered_domains: Vec<String>,
    audit_log: VecDeque<CrawlAuditEntry>,
    next_seq_id: u64,
    pub global_rate_scale: f64,
    pub adapt_to_server_headers: bool,
}

impl Default for DomainRateLimiter {
    fn default() -> Self {
        Self::new_with_defaults()
    }
}

impl DomainRateLimiter {
    pub fn new_with_defaults() -> Self {
        let now = Instant::now();
        let has_ncbi_key = std::env::var("NCBI_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .is_some();
        let has_s2_key = std::env::var("S2_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .is_some();
        let has_openaire_token = std::env::var("OPENAIRE_API_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .is_some();
        let has_ads_token = ["ADS_API_TOKEN", "NASA_ADS_TOKEN", "ADS_DEV_KEY"]
            .iter()
            .any(|k| std::env::var(k).ok().filter(|s| !s.trim().is_empty()).is_some());

        let defaults = default_scholarly_policies_with_auth(
            has_ncbi_key,
            has_s2_key,
            has_openaire_token,
            has_ads_token,
        );
        let mut buckets = HashMap::with_capacity(defaults.len());
        let mut ordered_domains = Vec::with_capacity(defaults.len());
        for p in defaults {
            ordered_domains.push(p.domain.clone());
            buckets.insert(p.domain.clone(), RuntimeDomainBucket::from_policy(p, now));
        }

        Self {
            buckets,
            ordered_domains,
            audit_log: VecDeque::with_capacity(512),
            next_seq_id: 1,
            global_rate_scale: 1.0,
            adapt_to_server_headers: true,
        }
    }

    fn ensure_bucket(&mut self, domain: &str, now: Instant) -> &mut RuntimeDomainBucket {
        let norm = normalize_domain_key(domain);
        if !self.buckets.contains_key(&norm) {
            let pol = DomainBucketPolicy {
                domain: norm.clone(),
                label: format!("Dynamic Host ({})", norm),
                refill_rps: 2.5,
                burst_capacity: 4.0,
                min_spacing_ms: 250,
                jitter_ms: 40,
                enabled: true,
                paused: false,
                tokens_available: 4.0,
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
                server_pool_note: "auto-default".to_string(),
            };
            self.ordered_domains.push(norm.clone());
            self.buckets
                .insert(norm.clone(), RuntimeDomainBucket::from_policy(pol, now));
        }
        self.buckets.get_mut(&norm).expect("bucket inserted")
    }

    /// Computes required sleep duration (without blocking while holding mutex) or returns `CircuitOpen`
    /// if the domain is manually paused or under a long `Retry-After` / CAPTCHA circuit-breaker (> 12s).
    pub fn reserve_permit_at(&mut self, url_or_domain: &str, now: Instant) -> (AcquirePermit, Duration) {
        let domain = extract_domain_from_url(url_or_domain);
        let scale = self.global_rate_scale.clamp(0.1, 10.0);
        let seq_hint = self.next_seq_id;
        let bucket = self.ensure_bucket(&domain, now);
        bucket.refill_at(now, scale);

        if bucket.policy.paused {
            return (
                AcquirePermit::CircuitOpen {
                    domain,
                    reason: "Domain manually paused in Rate Limiter policy".to_string(),
                    remaining_ms: 0,
                },
                Duration::ZERO,
            );
        }

        if let Some(cd_until) = bucket.cooldown_until {
            if cd_until > now {
                let rem = cd_until.duration_since(now);
                if rem > Duration::from_secs(12) {
                    return (
                        AcquirePermit::CircuitOpen {
                            domain,
                            reason: format!(
                                "Circuit-breaker cooldown active ({:.1}s remaining)",
                                rem.as_secs_f64()
                            ),
                            remaining_ms: rem.as_millis() as u64,
                        },
                        Duration::ZERO,
                    );
                }
            } else {
                bucket.cooldown_until = None;
            }
        }

        if !bucket.policy.enabled {
            return (
                AcquirePermit::Granted {
                    domain,
                    wait_ms: 0,
                    tokens_after: bucket.tokens,
                },
                Duration::ZERO,
            );
        }

        let eff_rps = (bucket.policy.refill_rps * scale).clamp(0.02, 500.0);
        let token_deficit_dur = if bucket.tokens >= 1.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(((1.0 - bucket.tokens) / eff_rps).clamp(0.0, 30.0))
        };

        let spacing_dur = bucket
            .next_allowed_at
            .and_then(|na| na.checked_duration_since(now))
            .unwrap_or(Duration::ZERO);

        let cooldown_dur = bucket
            .cooldown_until
            .and_then(|cu| cu.checked_duration_since(now))
            .unwrap_or(Duration::ZERO);

        let mut required_wait = token_deficit_dur.max(spacing_dur).max(cooldown_dur);
        if bucket.policy.jitter_ms > 0 && (required_wait > Duration::ZERO || bucket.policy.min_spacing_ms > 0) {
            let jit = seq_hint.wrapping_mul(2654435761) % (bucket.policy.jitter_ms + 1);
            if required_wait > Duration::ZERO {
                required_wait += Duration::from_millis(jit);
            }
        }

        // Advance virtual reservation state so concurrent callers queue cleanly without bursting simultaneously!
        let dispatch_instant = now + required_wait;
        let generated_during_wait = required_wait.as_secs_f64() * eff_rps;
        bucket.tokens = (bucket.tokens + generated_during_wait - 1.0)
            .clamp(0.0, bucket.policy.burst_capacity.max(1.0));
        bucket.last_refill = dispatch_instant;
        bucket.policy.tokens_available = bucket.tokens;

        let base_spacing = if bucket.policy.min_spacing_ms > 0 {
            let scaled_ms = ((bucket.policy.min_spacing_ms as f64) / scale).round() as u64;
            let jit = if bucket.policy.jitter_ms > 0 {
                (seq_hint.wrapping_add(7).wrapping_mul(11400714819323198485))
                    % (bucket.policy.jitter_ms + 1)
            } else {
                0
            };
            Duration::from_millis(scaled_ms + jit)
        } else {
            Duration::ZERO
        };
        bucket.next_allowed_at = Some(dispatch_instant + base_spacing);

        let wait_ms = required_wait.as_millis() as u64;
        if wait_ms > 0 {
            bucket.policy.throttled_waits += 1;
            bucket.policy.total_wait_ms += wait_ms;
        }

        (
            AcquirePermit::Granted {
                domain,
                wait_ms,
                tokens_after: bucket.tokens,
            },
            required_wait,
        )
    }

    pub fn apply_robots_crawl_delay(&mut self, domain_or_url: &str, crawl_delay_ms: u64) {
        if crawl_delay_ms == 0 {
            return;
        }
        let now = Instant::now();
        let b = self.ensure_bucket(domain_or_url, now);
        if crawl_delay_ms > b.policy.min_spacing_ms {
            b.policy.min_spacing_ms = crawl_delay_ms.min(30_000);
            let max_rps = (1000.0 / (b.policy.min_spacing_ms as f64)).max(0.05);
            if b.policy.refill_rps > max_rps {
                b.policy.refill_rps = max_rps;
            }
            b.policy.server_pool_note = format!("robots Crawl-delay={}ms", b.policy.min_spacing_ms);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_outcome(
        &mut self,
        subsystem: &str,
        method: &str,
        url: &str,
        outcome: CrawlAuditOutcome,
        http_status: u16,
        token_wait_ms: u64,
        latency_ms: u64,
        bytes_received: u64,
        headers: Option<&HeaderMap>,
        robots_status: &str,
        custom_note: &str,
    ) -> CrawlAuditEntry {
        let now = Instant::now();
        let domain = extract_domain_from_url(url);
        let adapt = self.adapt_to_server_headers;
        let scale = self.global_rate_scale;

        let mut retry_after_ms: Option<u64> = None;
        let mut header_notes: Vec<String> = Vec::new();
        if !custom_note.trim().is_empty() {
            header_notes.push(custom_note.trim().to_string());
        }

        if let Some(hmap) = headers {
            if let Some(ra_val) = hmap.get("retry-after").and_then(|v| v.to_str().ok()) {
                if let Some(ms) = parse_retry_after_ms(ra_val) {
                    retry_after_ms = Some(ms);
                    header_notes.push(format!("Retry-After={}ms", ms));
                }
            }
            if let Some((adv_rps, pool_desc)) = parse_crossref_rate_headers(hmap) {
                header_notes.push(pool_desc.clone());
                let b_tmp = self.ensure_bucket(&domain, now);
                b_tmp.policy.server_pool_note = pool_desc;
                if adapt && adv_rps > 0.0 {
                    // Stay politely at 80% of server-advertised ceiling
                    let target = (adv_rps * 0.80).clamp(0.2, 50.0);
                    b_tmp.policy.refill_rps = target;
                }
            }
            if let Some((_lim, rem, reset_opt, rl_desc)) = parse_standard_ratelimit_headers(hmap) {
                header_notes.push(rl_desc.clone());
                let b_tmp = self.ensure_bucket(&domain, now);
                b_tmp.policy.server_pool_note = rl_desc;
                if adapt && rem == 0 {
                    let now_epoch = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_secs();
                    let wait_s = reset_opt
                        .map(|r| if r > now_epoch { r - now_epoch } else { r })
                        .unwrap_or(5)
                        .clamp(2, 120);
                    b_tmp.tokens = 0.0;
                    b_tmp.policy.tokens_available = 0.0;
                    b_tmp.cooldown_until = Some(now + Duration::from_secs(wait_s));
                }
            }
        }

        let seq_id = self.next_seq_id;
        self.next_seq_id += 1;

        let bucket = self.ensure_bucket(&domain, now);
        bucket.refill_at(now, scale);
        bucket.policy.total_requests += 1;
        bucket.policy.bytes_received += bytes_received;
        bucket.policy.last_status = http_status;
        if latency_ms > 0 {
            if bucket.policy.ema_latency_ms <= 0.01 {
                bucket.policy.ema_latency_ms = latency_ms as f64;
            } else {
                bucket.policy.ema_latency_ms =
                    bucket.policy.ema_latency_ms * 0.80 + (latency_ms as f64) * 0.20;
            }
        }

        match outcome {
            CrawlAuditOutcome::Allowed2xx | CrawlAuditOutcome::ThrottledThen2xx => {
                bucket.policy.ok_2xx += 1;
            }
            CrawlAuditOutcome::Http429RateLimited => {
                bucket.policy.http_429_count += 1;
                bucket.tokens = 0.0;
                bucket.policy.tokens_available = 0.0;
                // INSPIRE-HEP enforces a 5s sliding IP quota where blocked calls count toward the window
                let min_429_ms = if domain.contains("inspirehep.net") {
                    5_500
                } else {
                    1_000
                };
                let cd_ms = retry_after_ms
                    .unwrap_or(min_429_ms.max(4_500))
                    .clamp(min_429_ms, 120_000);
                retry_after_ms = Some(cd_ms);
                bucket.cooldown_until = Some(now + Duration::from_millis(cd_ms));
                if adapt {
                    bucket.policy.refill_rps = (bucket.policy.refill_rps * 0.70).max(0.10);
                }
            }
            CrawlAuditOutcome::Http403OrCaptcha => {
                bucket.policy.http_403_captcha_count += 1;
                bucket.tokens = 0.0;
                bucket.policy.tokens_available = 0.0;
                // Arm 45-second protective cooldown on CAPTCHA / 403 block
                let cd_ms = retry_after_ms.unwrap_or(45_000);
                bucket.cooldown_until = Some(now + Duration::from_millis(cd_ms));
            }
            CrawlAuditOutcome::RobotsDisallowed => {
                bucket.policy.robots_blocked_count += 1;
            }
            CrawlAuditOutcome::CircuitBreakerSkipped => {}
            CrawlAuditOutcome::HttpOtherError | CrawlAuditOutcome::NetworkError => {
                bucket.policy.network_errors += 1;
            }
        }

        let tokens_remaining = bucket.tokens;
        let (epoch_ms, timestamp_utc) = current_utc_timestamp();
        let entry = CrawlAuditEntry {
            seq_id,
            timestamp_utc,
            epoch_ms,
            subsystem: subsystem.to_string(),
            domain,
            method: method.to_string(),
            url: url.to_string(),
            outcome,
            http_status,
            token_wait_ms,
            latency_ms,
            bytes_received,
            tokens_remaining,
            retry_after_ms,
            robots_status: robots_status.to_string(),
            note: header_notes.join(" | "),
        };

        if self.audit_log.len() >= MAX_AUDIT_LOG_ENTRIES {
            self.audit_log.pop_front();
        }
        self.audit_log.push_back(entry.clone());
        entry
    }

    pub fn snapshot_policies(&mut self) -> Vec<DomainBucketPolicy> {
        let now = Instant::now();
        let scale = self.global_rate_scale;
        for b in self.buckets.values_mut() {
            b.refill_at(now, scale);
        }
        let mut out = Vec::with_capacity(self.ordered_domains.len());
        for d in &self.ordered_domains {
            if let Some(b) = self.buckets.get(d) {
                out.push(b.policy.clone());
            }
        }
        out
    }

    pub fn snapshot_audit_log(&self) -> Vec<CrawlAuditEntry> {
        self.audit_log.iter().cloned().collect()
    }

    pub fn clear_audit_log(&mut self) {
        self.audit_log.clear();
    }

    pub fn reset_all_buckets(&mut self) {
        *self = Self::new_with_defaults();
    }

    pub fn update_domain_policy(
        &mut self,
        domain: &str,
        refill_rps: f64,
        burst_capacity: f64,
        min_spacing_ms: u64,
        jitter_ms: u64,
        enabled: bool,
        paused: bool,
    ) {
        let now = Instant::now();
        let b = self.ensure_bucket(domain, now);
        b.policy.refill_rps = refill_rps.clamp(0.05, 250.0);
        b.policy.burst_capacity = burst_capacity.clamp(1.0, 250.0);
        b.policy.min_spacing_ms = min_spacing_ms.min(60_000);
        b.policy.jitter_ms = jitter_ms.min(10_000);
        b.policy.enabled = enabled;
        b.policy.paused = paused;
        b.tokens = b.tokens.min(b.policy.burst_capacity);
        if !paused {
            b.cooldown_until = None;
        }
        b.policy.tokens_available = b.tokens;
    }

    pub fn restore_persisted_state(
        &mut self,
        policies: &[DomainBucketPolicy],
        entries: &[CrawlAuditEntry],
    ) {
        let now = Instant::now();
        for p in policies {
            let norm = normalize_domain_key(&p.domain);
            if !self.ordered_domains.contains(&norm) {
                self.ordered_domains.push(norm.clone());
            }
            let mut clean_p = p.clone();
            clean_p.domain = norm.clone();
            self.buckets
                .insert(norm, RuntimeDomainBucket::from_policy(clean_p, now));
        }
        self.audit_log.clear();
        for e in entries.iter().rev().take(MAX_AUDIT_LOG_ENTRIES).rev() {
            self.next_seq_id = self.next_seq_id.max(e.seq_id + 1);
            self.audit_log.push_back(e.clone());
        }
    }

    /// Deterministic offline dry-run generator that exercises token-bucket depletion, `min_spacing_ms`
    /// waits, RFC 9309 `robots.txt` blocks (`scholar.google.com`), and RFC 9110 `Retry-After` recovery
    /// so users can inspect live gauges and audit telemetry without making external network calls.
    pub fn synthesize_demo_audit_burst(&mut self) {
        let now = Instant::now();
        let demo_events = [
            ("search_api", "https://api.openalex.org/works?search=information+theory&per-page=25", CrawlAuditOutcome::Allowed2xx, 200u16, 142u64, 48_920u64, "n/a_api", "x-api-pool=polite (10 rps)"),
            ("search_api", "https://api.openalex.org/works?filter=openalex_id:W2100837269", CrawlAuditOutcome::Allowed2xx, 200, 118, 31_400, "n/a_api", "1-hop reference snowball"),
            ("search_api", "https://api.semanticscholar.org/graph/v1/paper/search?query=betweenness", CrawlAuditOutcome::Allowed2xx, 200, 265, 29_110, "n/a_api", "S2AG search batch"),
            ("search_api", "https://api.semanticscholar.org/graph/v1/paper/search?query=modularity", CrawlAuditOutcome::ThrottledThen2xx, 200, 240, 24_880, "n/a_api", "Queued 950ms for 1.0 rps S2 bucket"),
            ("search_api", "https://export.arxiv.org/api/query?search_query=all:protein+folding", CrawlAuditOutcome::ThrottledThen2xx, 200, 310, 19_640, "n/a_api", "Honored arXiv 3000ms min_spacing"),
            ("robots_txt", "https://scholar.google.com/scholar?hl=en&q=sleeping+beauties&start=0", CrawlAuditOutcome::RobotsDisallowed, 0, 18, 0, "disallowed", "RFC 9309 Disallow: /scholar (default gate)"),
            ("polite_crawler", "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi?db=pubmed", CrawlAuditOutcome::Allowed2xx, 200, 155, 8_420, "n/a_api", "NCBI Entrez ESearch (3.0 rps pool)"),
            ("polite_crawler", "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/elink.fcgi?dbfrom=pubmed", CrawlAuditOutcome::ThrottledThen2xx, 200, 192, 14_300, "n/a_api", "Paced 345ms between E-utility calls"),
            ("integrity_datacite", "https://api.crossref.org/works?filter=doi:10.1038/s41586-021-03819-2", CrawlAuditOutcome::Allowed2xx, 200, 134, 22_150, "n/a_api", "x-api-pool=polite | 50/1s"),
            ("integrity_datacite", "https://api.datacite.org/dois?query=relatedIdentifiers.relatedIdentifier", CrawlAuditOutcome::Allowed2xx, 200, 178, 15_900, "n/a_api", "Linked Zenodo software DOI"),
            ("bio_bridges", "https://www.ebi.ac.uk/europepmc/annotations_api/annotationsByArticleIds?articleIds=PMC:PMC6227811", CrawlAuditOutcome::Allowed2xx, 200, 164, 27_600, "n/a_api", "Batch of 8 PMC IDs (GO/EFO/UniProt)"),
            ("bio_bridges", "https://rest.uniprot.org/uniprotkb/search?query=lit_pubmed_id:34265844", CrawlAuditOutcome::Allowed2xx, 200, 149, 12_480, "n/a_api", "Swiss-Prot + PDB cross-references"),
            ("bio_bridges", "https://data.rcsb.org/rest/v1/core/entry/6VXX", CrawlAuditOutcome::Allowed2xx, 200, 112, 18_740, "n/a_api", "Cryo-EM 2.80 A primary citation"),
            ("search_api", "https://api.openaire.eu/graph/v2/researchProducts?search=information+theory&type=publication&pageSize=20", CrawlAuditOutcome::Allowed2xx, 200, 188, 36_410, "n/a_api", "OpenAIRE Graph v2 + BIP! indicators"),
            ("search_api", "https://inspirehep.net/api/literature?q=quantum+entropy&size=20", CrawlAuditOutcome::Allowed2xx, 200, 172, 41_280, "n/a_api", "INSPIRE-HEP 15 req/5s pool (2.4 rps)"),
            ("search_api", "https://api.adsabs.harvard.edu/v1/search/query?q=information+entropy&rows=20", CrawlAuditOutcome::Allowed2xx, 200, 161, 33_950, "n/a_api", "X-RateLimit: 4994/5000 rem"),
            ("polite_crawler", "https://dblp.org/search/publ/api?q=graph+neural&h=25&format=json", CrawlAuditOutcome::Http429RateLimited, 429, 95, 512, "n/a_api", "Simulated HTTP 429 Retry-After=2.0s"),
            ("oa_pdf_downloader", "https://api.unpaywall.org/v2/10.1073/pnas.1424329112", CrawlAuditOutcome::Allowed2xx, 200, 128, 4_120, "n/a_api", "Resolved best_oa_location PDF"),
        ];

        for (sub, url, outc, status, lat, bytes, rob, note) in demo_events {
            let (permit, req_wait) = self.reserve_permit_at(url, now);
            let wait_ms = match permit {
                AcquirePermit::Granted { wait_ms, .. } => wait_ms.max(req_wait.as_millis() as u64),
                AcquirePermit::CircuitOpen { remaining_ms, .. } => remaining_ms,
            };
            self.record_outcome(
                sub,
                "GET",
                url,
                outc,
                status,
                wait_ms,
                lat,
                bytes,
                None,
                rob,
                note,
            );
            // Clear synthetic dblp 429 cooldown immediately after recording so live crawls stay unblocked
            if status == 429 {
                if let Some(b) = self.buckets.get_mut("dblp.org") {
                    b.cooldown_until = None;
                    b.tokens = 1.5;
                    b.policy.tokens_available = 1.5;
                    b.policy.cooldown_remaining_ms = 0;
                }
            }
        }
    }
}

// ============================================================================
// GLOBAL THREAD-SAFE SINGLETON ACCESSORS FOR WORKER THREADS, CLI & EGUI
// ============================================================================
static GLOBAL_LIMITER: OnceLock<Mutex<DomainRateLimiter>> = OnceLock::new();

pub fn global_limiter_mutex() -> &'static Mutex<DomainRateLimiter> {
    GLOBAL_LIMITER.get_or_init(|| Mutex::new(DomainRateLimiter::new_with_defaults()))
}

/// Acquires a per-domain token-bucket permit, sleeping outside the mutex lock if pacing is needed.
pub fn acquire_domain_permit_blocking(url: &str) -> AcquirePermit {
    let (permit, sleep_dur) = match global_limiter_mutex().lock() {
        Ok(mut guard) => guard.reserve_permit_at(url, Instant::now()),
        Err(_) => (
            AcquirePermit::Granted {
                domain: extract_domain_from_url(url),
                wait_ms: 0,
                tokens_after: 1.0,
            },
            Duration::ZERO,
        ),
    };
    if sleep_dur > Duration::ZERO {
        thread::sleep(sleep_dur);
    }
    permit
}

#[allow(clippy::too_many_arguments)]
pub fn log_global_crawl_outcome(
    subsystem: &str,
    method: &str,
    url: &str,
    outcome: CrawlAuditOutcome,
    http_status: u16,
    token_wait_ms: u64,
    latency_ms: u64,
    bytes_received: u64,
    headers: Option<&HeaderMap>,
    robots_status: &str,
    note: &str,
) {
    if let Ok(mut guard) = global_limiter_mutex().lock() {
        guard.record_outcome(
            subsystem,
            method,
            url,
            outcome,
            http_status,
            token_wait_ms,
            latency_ms,
            bytes_received,
            headers,
            robots_status,
            note,
        );
    }
}

pub fn apply_global_robots_crawl_delay(domain_or_url: &str, delay_ms: u64) {
    if delay_ms > 0 {
        if let Ok(mut guard) = global_limiter_mutex().lock() {
            guard.apply_robots_crawl_delay(domain_or_url, delay_ms);
        }
    }
}

pub fn snapshot_global_policies() -> Vec<DomainBucketPolicy> {
    global_limiter_mutex()
        .lock()
        .map(|mut g| g.snapshot_policies())
        .unwrap_or_default()
}

pub fn snapshot_global_audit_log() -> Vec<CrawlAuditEntry> {
    global_limiter_mutex()
        .lock()
        .map(|g| g.snapshot_audit_log())
        .unwrap_or_default()
}

// ============================================================================
// DEFAULT DOMAIN PRESETS & RFC 9110 / RFC 9309 HEADER PARSERS
// ============================================================================
pub fn default_scholarly_policies(has_ncbi_key: bool, has_s2_key: bool) -> Vec<DomainBucketPolicy> {
    let has_openaire = std::env::var("OPENAIRE_API_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_some();
    let has_ads = ["ADS_API_TOKEN", "NASA_ADS_TOKEN", "ADS_DEV_KEY"]
        .iter()
        .any(|k| std::env::var(k).ok().filter(|s| !s.trim().is_empty()).is_some());
    default_scholarly_policies_with_auth(has_ncbi_key, has_s2_key, has_openaire, has_ads)
}

pub fn default_scholarly_policies_with_auth(
    has_ncbi_key: bool,
    has_s2_key: bool,
    has_openaire_token: bool,
    has_ads_token: bool,
) -> Vec<DomainBucketPolicy> {
    let mk = |domain: &str, label: &str, rps: f64, burst: f64, min_ms: u64, jit_ms: u64, note: &str| {
        DomainBucketPolicy {
            domain: domain.to_string(),
            label: label.to_string(),
            refill_rps: rps,
            burst_capacity: burst,
            min_spacing_ms: min_ms,
            jitter_ms: jit_ms,
            enabled: true,
            paused: false,
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
            server_pool_note: note.to_string(),
        }
    };

    vec![
        mk("scholar.google.com", "Google Scholar SERP", 0.28, 1.0, 3200, 600, "robots-gated + 3.2s pace"),
        mk("api.openalex.org", "OpenAlex Works API", 8.0, 10.0, 100, 20, "polite pool (10 rps)"),
        mk(
            "api.semanticscholar.org",
            "Semantic Scholar S2AG",
            if has_s2_key { 5.0 } else { 1.0 },
            if has_s2_key { 6.0 } else { 2.0 },
            if has_s2_key { 180 } else { 950 },
            40,
            if has_s2_key { "S2_API_KEY pool" } else { "public pool (1 rps)" },
        ),
        mk("api.crossref.org", "Crossref REST & Retraction Watch", 8.0, 10.0, 100, 25, "polite mailto pool"),
        mk("export.arxiv.org", "arXiv Atom API", 0.33, 1.0, 3000, 120, "arXiv 1 req / 3s rule"),
        mk("arxiv.org", "arXiv Direct OA PDF", 1.0, 2.0, 900, 80, "OA PDF stream"),
        mk(
            "eutils.ncbi.nlm.nih.gov",
            "PubMed NCBI E-Utilities",
            if has_ncbi_key { 9.0 } else { 2.8 },
            if has_ncbi_key { 8.0 } else { 3.0 },
            if has_ncbi_key { 110 } else { 345 },
            25,
            if has_ncbi_key { "NCBI_API_KEY (10 rps)" } else { "keyless (3 rps max)" },
        ),
        mk("www.ebi.ac.uk", "Europe PMC Search & Annotations", 5.0, 8.0, 160, 35, "EMBL-EBI REST"),
        mk("europepmc.org", "Europe PMC Full-Text PDF", 3.0, 4.0, 300, 50, "OA PDF renderer"),
        mk("rest.uniprot.org", "UniProtKB REST v2", 4.5, 6.0, 180, 35, "Swiss-Prot JSON"),
        mk("data.rcsb.org", "RCSB PDB Core Data API", 5.0, 8.0, 160, 30, "3D entry metadata"),
        mk("search.rcsb.org", "RCSB PDB Search v2", 4.0, 6.0, 200, 35, "DOI citation search"),
        mk("api.datacite.org", "DataCite Datasets & Software", 4.0, 6.0, 200, 40, "JSON:API v2"),
        mk("api.opencitations.net", "OpenCitations Index v2", 4.0, 6.0, 200, 35, "CC0 COCI citations"),
        mk("icite.od.nih.gov", "NIH iCite Open Citations", 5.0, 6.0, 180, 30, "200-PMID batch"),
        mk("dblp.org", "DBLP CS Bibliography", 1.5, 2.0, 650, 60, "publ/api JSON"),
        mk("api.biorxiv.org", "bioRxiv & medRxiv API", 2.5, 4.0, 350, 45, "CSHL interval JSON"),
        mk(
            "api.openaire.eu",
            "OpenAIRE Graph API v2/v3",
            if has_openaire_token { 1.8 } else { 0.50 },
            if has_openaire_token { 5.0 } else { 3.0 },
            if has_openaire_token { 500 } else { 1100 },
            45,
            if has_openaire_token {
                "OPENAIRE_API_TOKEN (7200/hr)"
            } else {
                "keyless (60/hr sliding window)"
            },
        ),
        mk(
            "api.scholexplorer.openaire.eu",
            "OpenAIRE ScholExplorer v3 Links",
            2.0,
            4.0,
            450,
            45,
            "Scholix citation & dataset links",
        ),
        mk(
            "inspirehep.net",
            "INSPIRE-HEP Literature & Citations",
            2.4,
            6.0,
            360,
            40,
            "15 req / 5s IP window (>=5s 429 wait)",
        ),
        mk(
            "api.adsabs.harvard.edu",
            "NASA ADS Search & Citation API",
            4.0,
            8.0,
            220,
            35,
            if has_ads_token {
                "ADS_API_TOKEN (5000 req/day)"
            } else {
                "Bearer ADS_API_TOKEN required (5000/day)"
            },
        ),
        mk(
            "ui.adsabs.harvard.edu",
            "NASA ADS Link Gateway PDF",
            1.5,
            3.0,
            600,
            60,
            "link_gateway EPRINT_PDF / ADS_PDF",
        ),
        mk("api.unpaywall.org", "Unpaywall OA Resolver v2", 5.0, 8.0, 150, 25, "email= polite pool"),
        mk("serpapi.com", "SerpApi Scholar Relay", 3.0, 5.0, 250, 20, "SERPAPI_KEY relay"),
    ]
}

pub fn extract_domain_from_url(url_or_domain: &str) -> String {
    let trimmed = url_or_domain.trim();
    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let host_port = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let host = host_port
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(host_port)
        .split(':')
        .next()
        .unwrap_or(host_port);
    normalize_domain_key(host)
}

pub fn normalize_domain_key(raw: &str) -> String {
    let h = raw.trim().trim_matches('/').to_ascii_lowercase();
    if h.is_empty() {
        "unknown.host".to_string()
    } else {
        h
    }
}

/// Parses RFC 9110 `Retry-After` header `delay-seconds` (supporting integer & decimal seconds such as `"5"` or `"2.5"`).
pub fn parse_retry_after_ms(raw: &str) -> Option<u64> {
    let t = raw.trim();
    if let Ok(secs_u64) = t.parse::<u64>() {
        return Some(secs_u64.saturating_mul(1_000));
    }
    if let Ok(secs_f64) = t.parse::<f64>() {
        if secs_f64.is_finite() && secs_f64 >= 0.0 {
            return Some((secs_f64 * 1_000.0).round() as u64);
        }
    }
    None
}

/// Parses Crossref `x-rate-limit-limit` (e.g. `50`), `x-rate-limit-interval` (e.g. `1s`), and `x-api-pool`.
pub fn parse_crossref_rate_headers(headers: &HeaderMap) -> Option<(f64, String)> {
    let limit_opt = headers
        .get("x-rate-limit-limit")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok());
    let interval_opt = headers
        .get("x-rate-limit-interval")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_ascii_lowercase());
    let pool = headers
        .get("x-api-pool")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let Some(limit) = limit_opt else {
        if !pool.is_empty() {
            return Some((0.0, format!("x-api-pool={}", pool)));
        }
        return None;
    };
    let secs = interval_opt
        .as_deref()
        .map(|iv| {
            let num_str = iv.trim_end_matches('s').trim();
            num_str.parse::<f64>().unwrap_or(1.0).max(0.1)
        })
        .unwrap_or(1.0);
    let rps = (limit / secs).clamp(0.1, 500.0);
    let desc = if !pool.is_empty() {
        format!(
            "x-api-pool={} ({:.0}/{}s = {:.1} rps)",
            pool, limit, secs, rps
        )
    } else {
        format!("x-rate-limit={:.0}/{}s ({:.1} rps)", limit, secs, rps)
    };
    Some((rps, desc))
}

/// Parses standard `X-RateLimit-Limit`, `X-RateLimit-Remaining`, and `X-RateLimit-Reset` headers
/// emitted by NASA ADS (`api.adsabs.harvard.edu`), OpenAIRE (`api.openaire.eu`), and API gateways.
pub fn parse_standard_ratelimit_headers(
    headers: &HeaderMap,
) -> Option<(u64, u64, Option<u64>, String)> {
    let limit = headers
        .get("x-ratelimit-limit")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())?;
    let remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())?;
    let reset = headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let desc = match reset {
        Some(r) => format!("X-RateLimit: {}/{} rem (reset={})", remaining, limit, r),
        None => format!("X-RateLimit: {}/{} rem", remaining, limit),
    };
    Some((limit, remaining, reset, desc))
}

/// Extracts `Crawl-delay` (in milliseconds) from a `robots.txt` document, giving precedence to
/// rules matching `bot_token` over wildcard `User-agent: *`.
pub fn parse_robots_crawl_delay_ms(robots_txt: &str, bot_token: &str) -> Option<u64> {
    let bot_low = bot_token.to_ascii_lowercase();
    let mut current_agents: Vec<String> = Vec::new();
    let mut saw_directive = false;
    let mut specific_delay_ms: Option<u64> = None;
    let mut wildcard_delay_ms: Option<u64> = None;

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
            if saw_directive {
                current_agents.clear();
                saw_directive = false;
            }
            current_agents.push(val.to_ascii_lowercase());
        } else {
            saw_directive = true;
            if key == "crawl-delay" {
                if let Ok(secs) = val.parse::<f64>() {
                    if secs.is_finite() && secs > 0.0 {
                        let ms = (secs * 1_000.0).round() as u64;
                        let is_spec = current_agents
                            .iter()
                            .any(|a| a != "*" && bot_low.contains(a.as_str()));
                        let is_wild = current_agents.iter().any(|a| a == "*");
                        if is_spec {
                            specific_delay_ms = Some(ms);
                        } else if is_wild && wildcard_delay_ms.is_none() {
                            wildcard_delay_ms = Some(ms);
                        }
                    }
                }
            }
        }
    }

    specific_delay_ms.or(wildcard_delay_ms)
}

/// Pure-Rust civil UTC ISO-8601 formatter (`YYYY-MM-DDTHH:MM:SSZ`) using Howard Hinnant's
/// proleptic Gregorian calendar algorithm (zero external crate dependencies).
pub fn current_utc_timestamp() -> (u64, String) {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let epoch_ms = dur.as_millis() as u64;
    let total_secs = dur.as_secs() as i64;
    let days = total_secs.div_euclid(86_400);
    let rem_secs = total_secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    let hh = rem_secs / 3_600;
    let mm = (rem_secs % 3_600) / 60;
    let ss = rem_secs % 60;

    (
        epoch_ms,
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, m, d, hh, mm, ss),
    )
}
