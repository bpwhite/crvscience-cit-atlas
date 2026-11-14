# CitAtlas (`cit-atlas` v1.8.0)

Native safe-Rust (`#![forbid(unsafe_code)]`, 17 modules, 30 unit tests) desktop (`eframe`/`egui` + `wgpu` `24.0` WGSL compute/instanced renderer) and `clap` CLI scientometrics workstation.

## Core Capabilities
- **13-Provider Scholarly Search & Citation Graph Harvester (`src/search_providers.rs`, `src/crawler.rs`)**:
  - Live REST & citation graph harvesters for [OpenAIRE Graph v2/v3](https://graph.openaire.eu/docs/apis/graph-api/) + ScholExplorer v3 (`3.6B+` literature/dataset/software links), [INSPIRE-HEP](https://github.com/inspirehep/rest-api-doc) (`recid` & `refersto:recid:` citation expansion), and [NASA ADS](https://github.com/adsabs/adsabs-dev-api) (Solr `reference`/`citation` `bibcode` graphs + `link_gateway` OA PDFs), alongside [OpenAlex](https://github.com/ourresearch/openalex-docs/blob/main/api-guide-for-llms.md), [Semantic Scholar](https://api.semanticscholar.org/api-docs/), [Crossref](https://www.crossref.org/documentation/retrieve-metadata/rest-api/), [arXiv](https://info.arxiv.org/help/api/index.html), [DataCite](https://support.datacite.org/docs/api), [PubMed E-Utilities](https://www.ncbi.nlm.nih.gov/books/NBK25497/), [Europe PMC](https://europepmc.org/AnnotationsApi), [DBLP](https://dblp.org/faq/How+to+use+the+dblp+search+API), [bioRxiv/medRxiv](https://api.biorxiv.org/), and polite `robots.txt`-gated Google Scholar SERP crawling.
- **24-Domain Configurable Token-Bucket Rate Limiter & Crawl Telemetry Audit Log (`src/rate_limiter.rs`, Tab `9. Rate Limiter & Audit`)**:
  - Pre-calibrated per-domain token buckets (`refill_rps`, `burst_capacity`, `min_spacing_ms`, `jitter_ms`, pause/circuit-breaker switches) honoring [OpenAIRE Terms of Use](https://graph.openaire.eu/docs/apis/terms/) (`60 req/hr` keyless vs. `7,200 req/hr` with `OPENAIRE_API_TOKEN`), [INSPIRE-HEP](https://github.com/inspirehep/rest-api-doc) (`15 req / 5s` IP window + mandatory `5.5s` `429` cooldown), [NASA ADS](https://github.com/adsabs/adsabs-dev-api) (`5,000 req/day` + `X-RateLimit-*` header tracking), [Crossref `x-rate-limit-*`](https://www.crossref.org/documentation/retrieve-metadata/rest-api/access-and-authentication/), and [RFC 9110 `Retry-After`](https://www.rfc-editor.org/info/rfc9110/).
- **`wgpu` WGSL 50,000+ Node Network Graph Viewer, Bio/DataCite Bridges & 10-Table CSV Suite (`src/wgpu_graph.rs`, `src/bio_entity_bridges.rs`, `src/export.rs`)**:
  - Multi-pass WGSL compute force simulation, instanced multi-shape SDF rendering, Retraction Watch screening, UniProtKB/PDB structural bridges, GraphML 1.0 + Gephi GEXF 1.3 (`viz`), and 10 RFC 4180 CSV exports.

## Quick Start
```bash
cargo test --release
cargo run --release
cargo run --release -- search "quantum mutual information" --providers openaire,inspire,ads --export-csv-dir ./cit_atlas_csv
cargo run --release -- rate-limits --simulate-burst --export-policies-csv ./domain_rate_policies.csv --export-audit-csv ./crawl_telemetry_audit.csv
```
