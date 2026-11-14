# Headless Validation (`crvscience-cit-atlas` v1.8.0)

Deterministic, dry-lab validation suite for [`cit-atlas`](../src/lib.rs) executed via compiled Rust integration tests with zero external HTTP network calls or GUI dependencies.

## Command

```bash
cargo test
# Or run only the headless validation integration suite:
cargo test --test headless_validation -- --nocapture
```

## Test Coverage

1. **T1 (`t1_24_domain_rate_limiter_concurrent_zero_violation_bursts`)**: Initializes all 24 pre-configured domain token buckets (`DomainRateLimiter::new_with_defaults`), executes 128 concurrent burst permit reservations (`32` per provider across `api.openalex.org`, `api.crossref.org`, `eutils.ncbi.nlm.nih.gov`, `api.semanticscholar.org`) over `8` threads asserting `0` `min_spacing_ms` violations and `0` unexpected `CircuitOpen` rejections, validates RFC 9110 `HTTP 429 Retry-After` circuit-breaker tripping, and records the offline audit burst.
2. **T2 (`t2_citation_dag_scientometrics_and_louvain_clustering`)**: Runs `compute_corpus_analytics` over the built-in empirical citation DAG (`build_empirical_corpus`), validating Louvain modularity clustering (`Q`), Infomap flow compression & ergodic PageRank stationary visit flow (`infomap_flow`), Disruption Index (`disruption_cd`), Brandes betweenness centrality (`cb_undir`, `cb_dir`), and DAG-derived HITS hub/authority and Hirsch `h-index`, exporting all per-paper metrics to [`citation_network_metrics.csv`](citation_network_metrics.csv).
3. **T3 (`t3_doi_title_deduplication_and_format_roundtrip_fidelity`)**: Exercises cross-provider deduplication (`merge_and_link_records`, `normalize_doi`) across DOI prefix/case variants, arXiv IDs, OpenAlex/S2 IDs, and punctuation-normalized titles with citation-edge hydration, followed by roundtrip verification of BibTeX (`format_bibtex_corpus` -> `parse_bibtex_text`), GraphML 1.0, Gephi GEXF 1.3, RFC 4180 CSV, and SQLite `LedgerStore` persistence (noting transparently in JSON telemetry that native RIS import/export is not implemented in `cit-atlas` v1.8.0).

## Generated Validation Artifacts

- [`cit_atlas_validation_summary.json`](cit_atlas_validation_summary.json) — Structured JSON telemetry for T1–T3 (mirrored to `app-preprints/releases/cit-atlas-bibliometric-graph-engine-validation-telemetry.json`).
- [`citation_network_metrics.csv`](citation_network_metrics.csv) — Per-paper citation DAG scientometric and community clustering export.
