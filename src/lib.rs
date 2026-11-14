#![forbid(unsafe_code)]

//! CitAtlas (`cit_atlas` v1.8.0) — Native Rust Citations Analytics Suite,
//! `wgpu` WGSL Compute & Instanced 50,000+ Node Network Graph Renderer,
//! Per-Domain Token-Bucket Rate Limiter & Crawl Telemetry Audit Log (24 Pre-Configured Hosts),
//! Multi-Provider Scholarly Harvester (OpenAlex, Semantic Scholar, Crossref, arXiv,
//! OpenAIRE Graph & ScholExplorer, INSPIRE-HEP, NASA ADS, DataCite, Google Scholar,
//! PubMed, Europe PMC, DBLP, bioRxiv/medRxiv), Retraction Watch & Bio-Entity Bridges,
//! GraphML 1.0 & Gephi GEXF 1.3 (`viz`) Exporters, and 10-Table RFC 4180 CSV Suite.

pub mod analytics;
pub mod bio_entity_bridges;
pub mod community_detection;
pub mod corpus;
pub mod crawler;
pub mod downloader;
pub mod export;
pub mod graph_viewer;
pub mod gui;
pub mod integrity_bridges;
pub mod models;
pub mod rate_limiter;
pub mod search_providers;
pub mod storage;
pub mod wgpu_graph;
