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

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use cit_atlas::analytics::compute_corpus_analytics;
use cit_atlas::corpus::{build_empirical_corpus, generate_synthetic_corpus};
use cit_atlas::downloader::batch_download_corpus_pdfs;
use cit_atlas::export::{
    build_bibliometric_impact_digest, export_all_csv_suite, export_annual_trajectories_csv,
    export_bio_entity_bridges_csv, export_bundle_json, export_communities_csv,
    export_crawl_audit_csv, export_domain_policies_csv, export_edges_csv, export_gexf,
    export_graphml, export_impact_digest_csv, export_impact_digest_json,
    export_integrity_and_datacite_csv, export_jsd_matrix_csv, export_metrics_csv,
    export_raw_papers_csv, export_synthesis_svg, export_topic_drift_csv,
};
use cit_atlas::graph_viewer::{
    compute_preset_positions, NodeColorEncoding, ViewerLayoutPreset,
};
use cit_atlas::gui::{capture_headless_screenshot_view, run_native_gui};
use cit_atlas::integrity_bridges::run_full_corpus_screening;
use cit_atlas::models::{
    AnalyticsParams, CrawlOptions, SearchProvidersConfig, SearchRequest, SyntheticRegime,
};
use cit_atlas::search_providers::{execute_multi_provider_search, lookup_single_doi};
use cit_atlas::rate_limiter::{
    global_limiter_mutex, snapshot_global_audit_log, snapshot_global_policies,
};
use cit_atlas::storage::{import_corpus_file, LedgerStore};

#[derive(Parser, Debug)]
#[command(
    name = "cit-atlas",
    version = "1.8.0",
    about = "Native Rust Citations Analytics Workstation with OpenAIRE Graph, INSPIRE-HEP & NASA ADS Harvesters, 24-Domain Token-Bucket Rate Limiter & Crawl Audit Log, wgpu WGSL 50,000+ Node Graph Renderer, Bio/DataCite Bridges & 10-Table CSV Suite"
)]
struct Cli {
    /// Render a 1600x1000 offscreen GPU screenshot to a PNG file without opening a GUI window
    #[arg(long, global = true, value_name = "OUT_PNG")]
    screenshot: Option<PathBuf>,

    /// Select which workbench domain view to capture with --screenshot (1, 2, or 3)
    #[arg(long, global = true, default_value_t = 1, value_name = "VIEW")]
    screenshot_view: u8,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Launch the interactive 8-domain native desktop GUI with wgpu WGSL 50k+ node graph renderer (default)
    Gui,

    /// Search scholarly APIs & crawlers, screen retractions, track DataCite bridges, and export CSV/GraphML/GEXF
    Search {
        query: String,

        #[arg(long, default_value = "openalex,s2,crossref,arxiv")]
        providers: String,

        #[arg(long, default_value_t = 25)]
        limit: usize,

        #[arg(long, default_value_t = false)]
        oa_only: bool,

        #[arg(long)]
        year_min: Option<i32>,

        #[arg(long)]
        year_max: Option<i32>,

        #[arg(long, default_value_t = 1)]
        snowball_hops: u8,

        #[arg(long, default_value = "")]
        rw_csv: String,

        #[arg(long, default_value_t = true)]
        screen_integrity: bool,

        #[arg(long, default_value = "")]
        openaire_token: String,

        #[arg(long, default_value = "")]
        ads_token: String,

        #[arg(long, default_value = "research@crvscience.com")]
        email: String,

        #[arg(long)]
        db: Option<PathBuf>,

        #[arg(long)]
        download_dir: Option<PathBuf>,

        #[arg(long)]
        export_csv: Option<PathBuf>,

        #[arg(long)]
        export_csv_dir: Option<PathBuf>,

        #[arg(long)]
        export_graphml: Option<PathBuf>,

        #[arg(long)]
        export_gexf: Option<PathBuf>,

        #[arg(long)]
        export_json: Option<PathBuf>,
    },

    /// Politely crawl Google Scholar, PubMed, Europe PMC, DBLP, bioRxiv/medRxiv, and DataCite
    Crawl {
        #[arg(default_value = "")]
        query: String,

        #[arg(long, default_value = "scholar,pubmed,europepmc,dblp,biorxiv")]
        targets: String,

        #[arg(long, default_value_t = 2)]
        pages: usize,

        #[arg(long, default_value_t = 25)]
        limit: usize,

        #[arg(long, default_value_t = 3200)]
        pace_ms: u64,

        #[arg(long, default_value = "")]
        cites_id: String,

        #[arg(long, default_value = "")]
        local_html: String,

        #[arg(long, default_value = "")]
        proxy: String,

        #[arg(long, default_value_t = false)]
        ignore_robots: bool,

        #[arg(long, default_value_t = true)]
        hydrate_edges: bool,

        #[arg(long, default_value_t = true)]
        screen_integrity: bool,

        #[arg(long, default_value = "")]
        rw_csv: String,

        #[arg(long, default_value_t = false)]
        no_self_citations: bool,

        #[arg(long)]
        year_min: Option<i32>,

        #[arg(long)]
        year_max: Option<i32>,

        #[arg(long, default_value_t = false)]
        oa_only: bool,

        #[arg(long, default_value = "research@crvscience.com")]
        email: String,

        #[arg(long)]
        db: Option<PathBuf>,

        #[arg(long)]
        download_dir: Option<PathBuf>,

        #[arg(long)]
        export_csv: Option<PathBuf>,

        #[arg(long)]
        export_csv_dir: Option<PathBuf>,

        #[arg(long)]
        export_graphml: Option<PathBuf>,

        #[arg(long)]
        export_gexf: Option<PathBuf>,
    },

    /// Screen an existing corpus against Crossref Retraction Watch & DataCite dataset/software DOIs
    Screen {
        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        from_file: Option<PathBuf>,

        #[arg(long)]
        rw_csv: Option<PathBuf>,

        #[arg(long, default_value_t = true)]
        crossref: bool,

        #[arg(long, default_value_t = true)]
        datacite: bool,

        #[arg(long, default_value_t = true)]
        epmc_annotations: bool,

        #[arg(long, default_value_t = true)]
        uniprot_pdb: bool,

        #[arg(long, default_value_t = true)]
        link_shared_artifacts: bool,

        #[arg(long, default_value = "research@crvscience.com")]
        email: String,

        #[arg(long)]
        save_db: Option<PathBuf>,

        #[arg(long)]
        export_csv_dir: Option<PathBuf>,
    },

    /// Export citation & DataCite bipartite network to GraphML 1.0 (.graphml) and/or Gephi GEXF 1.3 (.gexf)
    ExportGraph {
        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        from_file: Option<PathBuf>,

        #[arg(long, default_value = "both")]
        format: String,

        #[arg(long, default_value = "force-atlas")]
        layout: String,

        #[arg(long, default_value = "discipline")]
        color_by: String,

        #[arg(long, default_value_t = true)]
        include_datacite_nodes: bool,

        #[arg(long, default_value_t = false)]
        exclude_retracted: bool,

        #[arg(long, default_value = "./cit_atlas_network")]
        out_dir: PathBuf,

        #[arg(long)]
        out_file: Option<PathBuf>,
    },

    /// Export one or all 10 RFC 4180 CSV tables from SQLite, .json/.csl/.bib/.ris/.nbib/.csv/.html/.graphml/.gexf, or the empirical benchmark
    ExportCsv {
        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        from_file: Option<PathBuf>,

        #[arg(long, default_value_t = false)]
        exclude_retracted: bool,

        #[arg(long, default_value = "all")]
        table: String,

        #[arg(long, default_value = "./cit_atlas_csv")]
        out_dir: PathBuf,

        #[arg(long)]
        out_file: Option<PathBuf>,
    },

    /// Batch-download verified Open-Access PDFs (Provider OA -> Unpaywall v2 -> arXiv -> Europe PMC)
    Download {
        #[arg(long)]
        doi: Option<String>,

        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        from_file: Option<PathBuf>,

        #[arg(long, default_value = "./cit_atlas_downloads")]
        out_dir: PathBuf,

        #[arg(long, default_value = "research@crvscience.com")]
        email: String,

        #[arg(long, default_value_t = 450)]
        pace_ms: u64,
    },

    /// Run headless scientometric analysis (Velocity, Ke Beauty B, Brandes CB, Guimera-Amaral P_i, MI/JSD Drift)
    Analyze {
        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        from_file: Option<PathBuf>,

        #[arg(long, default_value_t = 10)]
        window: i32,

        #[arg(long, default_value_t = 5)]
        stride: i32,

        #[arg(long, default_value_t = 6.0)]
        half_life: f64,

        #[arg(long, default_value_t = 2026)]
        horizon: i32,

        #[arg(long, default_value_t = false)]
        exclude_retracted: bool,

        #[arg(long)]
        export_csv: Option<PathBuf>,

        #[arg(long)]
        export_csv_dir: Option<PathBuf>,

        #[arg(long)]
        export_graphml: Option<PathBuf>,

        #[arg(long)]
        export_gexf: Option<PathBuf>,

        #[arg(long)]
        export_json: Option<PathBuf>,

        #[arg(long)]
        export_svg: Option<PathBuf>,
    },

    /// Run multi-level Louvain modularity Q(gamma) and Infomap random-walk flow compression L(M)
    Communities {
        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        from_file: Option<PathBuf>,

        #[arg(long, default_value_t = 1.0)]
        resolution: f64,

        #[arg(long, default_value_t = 0.15)]
        teleport: f64,

        #[arg(long)]
        export_csv: Option<PathBuf>,

        #[arg(long)]
        export_graphml: Option<PathBuf>,

        #[arg(long)]
        export_gexf: Option<PathBuf>,

        #[arg(long)]
        export_json: Option<PathBuf>,
    },


    /// Configure per-domain token-bucket rate limits, simulate burst pacing, and export crawl telemetry audit logs
    RateLimits {
        /// Global multiplier applied to all domain refill rates (e.g. 0.5 for conservative, 2.0 for fast)
        #[arg(long, default_value_t = 1.0)]
        rate_scale: f64,

        /// Comma-separated domain overrides formatted as domain=rps:burst:min_ms (e.g. api.openalex.org=5:8:150)
        #[arg(long, default_value = "")]
        set: String,

        /// Synthesize a 15-request multi-domain dry-run burst to inspect token pacing & audit CSV output
        #[arg(long, default_value_t = false)]
        simulate_burst: bool,

        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        save_db: Option<PathBuf>,

        #[arg(long)]
        export_audit_csv: Option<PathBuf>,

        #[arg(long)]
        export_policies_csv: Option<PathBuf>,
    },

    /// Generate a parametric synthetic multi-decade benchmark graph (supports 50,000+ nodes) and print drift summary
    Benchmark {
        /// Number of synthetic citation nodes (30 to 150,000)
        #[arg(long, default_value_t = 50000)]
        nodes: usize,

        #[arg(long, default_value_t = 2026)]
        seed: u32,

        #[arg(long, default_value_t = 0.18)]
        bridge_beta: f64,

        #[arg(long, default_value = "convergent")]
        regime: String,

        #[arg(long)]
        export_csv_dir: Option<PathBuf>,

        #[arg(long)]
        export_graphml: Option<PathBuf>,

        #[arg(long)]
        export_gexf: Option<PathBuf>,

        #[arg(long)]
        export_json: Option<PathBuf>,
    },

    /// Export comparative bibliometric impact digest (h-index, g-index, i10-index, PageRank, community modularity)
    ImpactDigest {
        #[arg(long)]
        from_db: Option<PathBuf>,

        #[arg(long)]
        from_file: Option<PathBuf>,

        #[arg(long, default_value_t = 2026)]
        horizon: i32,

        #[arg(long, default_value_t = false)]
        exclude_retracted: bool,

        #[arg(long)]
        export_json: Option<PathBuf>,

        #[arg(long)]
        export_csv: Option<PathBuf>,
    },
}

fn load_corpus_from_sources(
    from_db: &Option<PathBuf>,
    from_file: &Option<PathBuf>,
) -> (Vec<cit_atlas::models::PaperRecord>, Vec<cit_atlas::models::CitationEdge>) {
    if let Some(ref db_p) = from_db {
        LedgerStore::open(db_p)
            .and_then(|s| s.load_corpus())
            .unwrap_or_else(|e| {
                eprintln!("{}", e);
                std::process::exit(1);
            })
    } else if let Some(ref f_p) = from_file {
        import_corpus_file(f_p).unwrap_or_else(|e| {
            eprintln!("{}", e);
            std::process::exit(1);
        })
    } else {
        build_empirical_corpus()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn attach_parent_console_for_cli() {
    if std::env::args_os().len() > 1 {
        unsafe {
            let _ = windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
}

fn flush_stdio() {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

fn main() {
    #[cfg(windows)]
    attach_parent_console_for_cli();

    let cli = Cli::parse();
    if let Some(out_png) = cli.screenshot {
        if let Err(e) = capture_headless_screenshot_view(&out_png, cli.screenshot_view) {
            eprintln!("Screenshot error: {}", e);
            flush_stdio();
            std::process::exit(1);
        }
        flush_stdio();
        return;
    }
    match cli.command {
        None | Some(Commands::Gui) => {
            if let Err(e) = run_native_gui() {
                eprintln!("GUI error: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Search {
            query,
            providers,
            limit,
            oa_only,
            year_min,
            year_max,
            snowball_hops,
            rw_csv,
            screen_integrity,
            openaire_token,
            ads_token,
            email,
            db,
            download_dir,
            export_csv,
            export_csv_dir,
            export_graphml: graphml_p,
            export_gexf: gexf_p,
            export_json,
        }) => {
            let cfg = SearchProvidersConfig::from_csv_flags(&providers);
            let mut crawl = CrawlOptions::default();
            crawl.screen_retractions = screen_integrity;
            crawl.track_datacite_bridges = screen_integrity;
            crawl.retraction_watch_csv_path = rw_csv;
            crawl.openaire_token = openaire_token;
            crawl.nasa_ads_token = ads_token;
            let req = SearchRequest {
                query,
                providers: cfg,
                max_per_provider: limit,
                year_min,
                year_max,
                oa_only,
                polite_email: email.clone(),
                snowball_hops,
                crawl,
                ..SearchRequest::default()
            };
            run_harvest_pipeline(
                req,
                &email,
                db,
                download_dir,
                export_csv,
                export_csv_dir,
                graphml_p,
                gexf_p,
                export_json,
            );
        }
        Some(Commands::Crawl {
            query,
            targets,
            pages,
            limit,
            pace_ms,
            cites_id,
            local_html,
            proxy,
            ignore_robots,
            hydrate_edges,
            screen_integrity,
            rw_csv,
            no_self_citations,
            year_min,
            year_max,
            oa_only,
            email,
            db,
            download_dir,
            export_csv,
            export_csv_dir,
            export_graphml: graphml_p,
            export_gexf: gexf_p,
        }) => {
            let cfg = SearchProvidersConfig::from_csv_flags(&targets);
            let html_files: Vec<String> = local_html
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let req = SearchRequest {
                query,
                providers: cfg,
                max_per_provider: limit,
                year_min,
                year_max,
                oa_only,
                polite_email: email.clone(),
                snowball_hops: 1,
                crawl: CrawlOptions {
                    max_pages: pages,
                    pace_ms,
                    jitter_ms: 600,
                    respect_robots_txt: !ignore_robots,
                    scholar_cites_cluster_id: cites_id,
                    hydrate_citation_edges: hydrate_edges,
                    filter_self_citations: no_self_citations,
                    proxy_url: proxy,
                    local_html_paths: html_files,
                    screen_retractions: screen_integrity,
                    track_datacite_bridges: screen_integrity,
                    track_epmc_annotations: screen_integrity,
                    track_uniprot_pdb_bridges: screen_integrity,
                    retraction_watch_csv_path: rw_csv,
                    openaire_token: String::new(),
                    nasa_ads_token: String::new(),
                },
                ..SearchRequest::default()
            };
            run_harvest_pipeline(
                req,
                &email,
                db,
                download_dir,
                export_csv,
                export_csv_dir,
                graphml_p,
                gexf_p,
                None,
            );
        }
        Some(Commands::Screen {
            from_db,
            from_file,
            rw_csv,
            crossref,
            datacite,
            epmc_annotations,
            uniprot_pdb,
            link_shared_artifacts,
            email,
            save_db,
            export_csv_dir,
        }) => {
            let (mut papers, mut edges) = load_corpus_from_sources(&from_db, &from_file);
            match run_full_corpus_screening(
                &mut papers,
                &mut edges,
                &email,
                "",
                rw_csv.as_deref(),
                crossref,
                datacite,
                epmc_annotations,
                uniprot_pdb,
                link_shared_artifacts,
            ) {
                Ok(rep) => {
                    println!(
                        "Screening complete: {} papers | {} retracted/withdrawn | {} DataCite DOIs | {} UniProt | {} PDB | {} Europe PMC terms ({} bio-bridge edges)",
                        rep.papers_screened,
                        rep.retractions_flagged,
                        rep.datacite_artifacts_linked,
                        rep.uniprot_proteins_linked,
                        rep.pdb_structures_linked,
                        rep.epmc_annotations_linked,
                        rep.shared_bio_bridge_edges
                    );
                    let target_db = save_db.or(from_db);
                    if let Some(ref db_p) = target_db {
                        if let Err(e) = LedgerStore::open(db_p)
                            .and_then(|mut store| store.save_corpus(&papers, &edges, true))
                        {
                            eprintln!("Failed saving screened corpus to {:?}: {}", db_p, e);
                            flush_stdio();
                            std::process::exit(1);
                        }
                    }
                    let analytics =
                        compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
                    if let Some(ref dir_p) = export_csv_dir {
                        if let Err(e) = export_all_csv_suite(&papers, &edges, &analytics, dir_p) {
                            eprintln!("Failed exporting CSV suite to {:?}: {}", dir_p, e);
                            flush_stdio();
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Screening failed: {}", e);
                    flush_stdio();
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::ExportGraph {
            from_db,
            from_file,
            format,
            layout,
            color_by,
            include_datacite_nodes,
            exclude_retracted,
            out_dir,
            out_file,
        }) => {
            let (papers, edges) = load_corpus_from_sources(&from_db, &from_file);
            let params = AnalyticsParams {
                exclude_retracted,
                ..Default::default()
            };
            let analytics = compute_corpus_analytics(&papers, &edges, &params);
            let preset = ViewerLayoutPreset::from_name(&layout);
            let color_enc = NodeColorEncoding::from_name(&color_by);
            let coords = compute_preset_positions(&analytics, preset, color_enc, "");
            let fmt = format.trim().to_ascii_lowercase();

            if fmt == "both" {
                if let Err(e) = std::fs::create_dir_all(&out_dir) {
                    eprintln!("Failed creating output directory {:?}: {}", out_dir, e);
                    flush_stdio();
                    std::process::exit(1);
                }
                let gml_p = out_dir.join("cit_atlas_network.graphml");
                let gxf_p = out_dir.join("cit_atlas_network.gexf");
                export_graphml(
                    &papers,
                    &edges,
                    &analytics,
                    Some(&coords),
                    color_enc,
                    include_datacite_nodes,
                    &gml_p,
                )
                .expect("GraphML export failed");
                export_gexf(
                    &papers,
                    &edges,
                    &analytics,
                    Some(&coords),
                    color_enc,
                    include_datacite_nodes,
                    &gxf_p,
                )
                .expect("GEXF export failed");
                println!("Exported GraphML -> {:?} and GEXF 1.3 -> {:?}", gml_p, gxf_p);
            } else if fmt == "graphml" {
                let dst = out_file.unwrap_or_else(|| out_dir.join("cit_atlas_network.graphml"));
                if let Some(parent) = dst.parent() {
                    if !parent.as_os_str().is_empty() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            eprintln!("Failed creating output directory {:?}: {}", parent, e);
                            flush_stdio();
                            std::process::exit(1);
                        }
                    }
                }
                export_graphml(
                    &papers,
                    &edges,
                    &analytics,
                    Some(&coords),
                    color_enc,
                    include_datacite_nodes,
                    &dst,
                )
                .expect("GraphML export failed");
                println!("Exported GraphML -> {:?}", dst);
            } else if fmt == "gexf" {
                let dst = out_file.unwrap_or_else(|| out_dir.join("cit_atlas_network.gexf"));
                if let Some(parent) = dst.parent() {
                    if !parent.as_os_str().is_empty() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            eprintln!("Failed creating output directory {:?}: {}", parent, e);
                            flush_stdio();
                            std::process::exit(1);
                        }
                    }
                }
                export_gexf(
                    &papers,
                    &edges,
                    &analytics,
                    Some(&coords),
                    color_enc,
                    include_datacite_nodes,
                    &dst,
                )
                .expect("GEXF export failed");
                println!("Exported GEXF 1.3 -> {:?}", dst);
            } else {
                eprintln!("Unknown --format '{}'. Expected: both, graphml, or gexf.", fmt);
                std::process::exit(1);
            }
        }
        Some(Commands::ExportCsv {
            from_db,
            from_file,
            exclude_retracted,
            table,
            out_dir,
            out_file,
        }) => {
            let (papers, edges) = load_corpus_from_sources(&from_db, &from_file);
            let params = AnalyticsParams {
                exclude_retracted,
                ..Default::default()
            };
            let analytics = compute_corpus_analytics(&papers, &edges, &params);
            let mode = table.trim().to_ascii_lowercase();

            if mode == "all" {
                match export_all_csv_suite(&papers, &edges, &analytics, &out_dir) {
                    Ok(files) => {
                        println!(
                            "Exported {} RFC 4180 CSV tables into {:?}:",
                            files.len(),
                            out_dir
                        );
                        for f in files {
                            println!("  - {}", f);
                        }
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                let target_path =
                    out_file.unwrap_or_else(|| PathBuf::from(format!("cit_atlas_{}.csv", mode)));
                let res = match mode.as_str() {
                    "metrics" | "papers" => export_metrics_csv(&analytics, &target_path),
                    "raw" | "catalog" => export_raw_papers_csv(&papers, &target_path),
                    "edges" | "citations" => {
                        export_edges_csv(&papers, &edges, &analytics, &target_path)
                    }
                    "bridges" | "retractions" | "datacite" => {
                        export_integrity_and_datacite_csv(&analytics, &target_path)
                    }
                    "bio-entities" | "bio" | "uniprot" | "pdb" | "epmc" => {
                        export_bio_entity_bridges_csv(&analytics, &target_path)
                    }
                    "audit" | "crawl-audit" | "telemetry" => {
                        export_crawl_audit_csv(&snapshot_global_audit_log(), &target_path)
                    }
                    "rate-limits" | "policies" => {
                        export_domain_policies_csv(&snapshot_global_policies(), &target_path)
                    }
                    "trajectories" | "annual" | "velocity" => {
                        export_annual_trajectories_csv(&analytics, &target_path)
                    }
                    "drift" | "windows" => export_topic_drift_csv(&analytics, &target_path),
                    "jsd" => export_jsd_matrix_csv(&analytics, &target_path),
                    "communities" | "louvain" | "infomap" => {
                        export_communities_csv(&analytics, &target_path)
                    }
                    "impact-digest" | "impact" | "digest" => {
                        export_impact_digest_csv(&analytics, 2026, &target_path)
                    }
                    other => Err(format!(
                        "Unknown --table '{}'. Expected: all, metrics, raw, edges, bridges, trajectories, drift, jsd, communities, or impact-digest.",
                        other
                    )),
                };
                match res {
                    Ok(_) => println!("Exported '{}' CSV table -> {:?}", mode, target_path),
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(Commands::Download {
            doi,
            from_db,
            from_file,
            out_dir,
            email,
            pace_ms,
        }) => {
            let mut papers = if let Some(ref d) = doi {
                match lookup_single_doi(d, &email) {
                    Ok(rec) => vec![rec],
                    Err(e) => {
                        eprintln!("DOI resolution failed: {}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                load_corpus_from_sources(&from_db, &from_file).0
            };

            match batch_download_corpus_pdfs(&mut papers, &out_dir, &email, pace_ms, None) {
                Ok(reps) => {
                    let ok_c = reps.iter().filter(|r| r.status.is_ok()).count();
                    println!(
                        "Completed: {}/{} verified PDFs written to {:?}.",
                        ok_c,
                        reps.len(),
                        out_dir
                    );
                }
                Err(e) => {
                    eprintln!("Download error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Analyze {
            from_db,
            from_file,
            window,
            stride,
            half_life,
            horizon,
            exclude_retracted,
            export_csv,
            export_csv_dir,
            export_graphml: graphml_p,
            export_gexf: gexf_p,
            export_json,
            export_svg,
        }) => {
            let (papers, edges) = load_corpus_from_sources(&from_db, &from_file);
            let params = AnalyticsParams {
                window_width: window,
                window_stride: stride,
                half_life,
                horizon_year: horizon,
                exclude_retracted,
                ..Default::default()
            };
            let analytics = compute_corpus_analytics(&papers, &edges, &params);
            print_console_digest(&analytics);

            if let Some(ref csv_p) = export_csv {
                export_metrics_csv(&analytics, csv_p).expect("CSV write failed");
            }
            if let Some(ref dir_p) = export_csv_dir {
                export_all_csv_suite(&papers, &edges, &analytics, dir_p)
                    .expect("CSV suite write failed");
            }
            if let Some(ref gml) = graphml_p {
                export_graphml(
                    &papers,
                    &edges,
                    &analytics,
                    None,
                    NodeColorEncoding::Discipline,
                    true,
                    gml,
                )
                .expect("GraphML write failed");
            }
            if let Some(ref gxf) = gexf_p {
                export_gexf(
                    &papers,
                    &edges,
                    &analytics,
                    None,
                    NodeColorEncoding::Discipline,
                    true,
                    gxf,
                )
                .expect("GEXF write failed");
            }
            if let Some(ref json_p) = export_json {
                export_bundle_json(&papers, &edges, &analytics, json_p).expect("JSON write failed");
            }
            if let Some(ref svg_p) = export_svg {
                export_synthesis_svg(&analytics, svg_p).expect("SVG write failed");
            }
        }
        Some(Commands::Communities {
            from_db,
            from_file,
            resolution,
            teleport,
            export_csv,
            export_graphml: graphml_p,
            export_gexf: gexf_p,
            export_json,
        }) => {
            let (papers, edges) = load_corpus_from_sources(&from_db, &from_file);
            let params = AnalyticsParams {
                louvain_resolution: resolution,
                infomap_teleport: teleport,
                ..Default::default()
            };
            let analytics = compute_corpus_analytics(&papers, &edges, &params);
            print_console_digest(&analytics);
            if let Some(ref csv_p) = export_csv {
                export_communities_csv(&analytics, csv_p).expect("Communities CSV write failed");
            }
            if let Some(ref gml) = graphml_p {
                export_graphml(
                    &papers,
                    &edges,
                    &analytics,
                    None,
                    NodeColorEncoding::LouvainCommunity,
                    true,
                    gml,
                )
                .expect("GraphML write failed");
            }
            if let Some(ref gxf) = gexf_p {
                export_gexf(
                    &papers,
                    &edges,
                    &analytics,
                    None,
                    NodeColorEncoding::LouvainCommunity,
                    true,
                    gxf,
                )
                .expect("GEXF write failed");
            }
            if let Some(ref json_p) = export_json {
                export_bundle_json(&papers, &edges, &analytics, json_p).expect("JSON write failed");
            }
        }
        Some(Commands::RateLimits {
            rate_scale,
            set,
            simulate_burst,
            from_db,
            save_db,
            export_audit_csv: audit_csv_p,
            export_policies_csv: pol_csv_p,
        }) => {
            if let Some(ref db_p) = from_db {
                if let Ok(store) = LedgerStore::open(db_p) {
                    if let Ok((pols, logs)) = store.load_rate_policies_and_audit() {
                        if let Ok(mut g) = global_limiter_mutex().lock() {
                            g.restore_persisted_state(&pols, &logs);
                        }
                    }
                }
            }
            if let Ok(mut g) = global_limiter_mutex().lock() {
                g.global_rate_scale = rate_scale.clamp(0.1, 10.0);
                for spec in set.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    if let Some((dom, rhs)) = spec.split_once('=') {
                        let parts: Vec<&str> = rhs.split(':').collect();
                        let rps = parts.first().and_then(|s| s.parse::<f64>().ok()).unwrap_or(2.0);
                        let burst = parts.get(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or(4.0);
                        let min_ms = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(200);
                        g.update_domain_policy(dom.trim(), rps, burst, min_ms, 30, true, false);
                    }
                }
                if simulate_burst {
                    g.synthesize_demo_audit_burst();
                }
            }

            let pols = snapshot_global_policies();
            let logs = snapshot_global_audit_log();
            println!(
                "=== CitAtlas Per-Domain Token-Bucket Rate Policies ({} hosts, scale={:.2}x, {} audit events) ===",
                pols.len(),
                rate_scale,
                logs.len()
            );
            for p in &pols {
                println!(
                    "  - {:<26} rps={:>5.2}  burst={:>4.1}  floor={:>4}ms  tokens={:>4.1}  reqs={} (waits={}, 429={}, robots={})",
                    p.domain,
                    p.refill_rps,
                    p.burst_capacity,
                    p.min_spacing_ms,
                    p.tokens_available,
                    p.total_requests,
                    p.throttled_waits,
                    p.http_429_count,
                    p.robots_blocked_count
                );
            }

            let target_db = save_db.or(from_db);
            if let Some(ref db_p) = target_db {
                if let Err(e) = LedgerStore::open(db_p)
                    .and_then(|mut store| store.save_rate_policies_and_audit(&pols, &logs))
                {
                    eprintln!("Failed saving rate policies & audit to {:?}: {}", db_p, e);
                    flush_stdio();
                    std::process::exit(1);
                }
            }
            if let Some(ref ap) = audit_csv_p {
                export_crawl_audit_csv(&logs, ap).expect("Audit CSV write failed");
            }
            if let Some(ref pp) = pol_csv_p {
                export_domain_policies_csv(&pols, pp).expect("Policies CSV write failed");
            }
        }
        Some(Commands::Benchmark {
            nodes,
            seed,
            bridge_beta,
            regime,
            export_csv_dir,
            export_graphml: graphml_p,
            export_gexf: gexf_p,
            export_json,
        }) => {
            let reg = match regime.to_ascii_lowercase().as_str() {
                "awakening" => SyntheticRegime::Awakening,
                "siloed" => SyntheticRegime::Siloed,
                _ => SyntheticRegime::Convergent,
            };
            let (papers, edges) = generate_synthetic_corpus(nodes, seed, bridge_beta, reg);
            let analytics =
                compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
            print_console_digest(&analytics);
            if let Some(ref dir_p) = export_csv_dir {
                export_all_csv_suite(&papers, &edges, &analytics, dir_p)
                    .expect("CSV suite write failed");
            }
            if let Some(ref gml) = graphml_p {
                export_graphml(
                    &papers,
                    &edges,
                    &analytics,
                    None,
                    NodeColorEncoding::Discipline,
                    true,
                    gml,
                )
                .expect("GraphML write failed");
            }
            if let Some(ref gxf) = gexf_p {
                export_gexf(
                    &papers,
                    &edges,
                    &analytics,
                    None,
                    NodeColorEncoding::Discipline,
                    true,
                    gxf,
                )
                .expect("GEXF write failed");
            }
            if let Some(ref json_p) = export_json {
                export_bundle_json(&papers, &edges, &analytics, json_p).expect("JSON write failed");
            }
        }
        Some(Commands::ImpactDigest {
            from_db,
            from_file,
            horizon,
            exclude_retracted,
            export_json,
            export_csv,
        }) => {
            let (papers, edges) = load_corpus_from_sources(&from_db, &from_file);
            let params = AnalyticsParams {
                horizon_year: horizon,
                exclude_retracted,
                ..Default::default()
            };
            let analytics = compute_corpus_analytics(&papers, &edges, &params);
            let digest = build_bibliometric_impact_digest(&analytics, horizon);
            println!(
                "=== CitAtlas Comparative Bibliometric Impact Digest (Horizon {}, {} papers, {} edges) ===",
                digest.horizon_year, digest.total_papers, digest.total_edges
            );
            println!(
                "  h-index={} | g-index={} | i10-index={} | e-index={:.2} | Louvain Q={:.4} ({} comms) | Infomap L(M)={:.4}b ({} mods, {:.1}% compression)",
                digest.h_index,
                digest.g_index,
                digest.i10_index,
                digest.e_index,
                digest.louvain_modularity_q,
                digest.louvain_communities,
                digest.infomap_codelength_bits,
                digest.infomap_modules,
                digest.infomap_compression_pct
            );
            println!(
                "  PageRank (Infomap Flow): mean={:.5}, max={:.5} ({}) | Mean CB_u={:.5} | Mean CD={:.4}",
                digest.mean_pagerank_flow,
                digest.max_pagerank_flow,
                digest.top_pagerank_paper_id,
                digest.mean_betweenness_cb,
                digest.mean_disruption_cd
            );
            for d in &digest.disciplines {
                println!(
                    "    [{}] {:<38} n={:>3}  cites={:>6}  h={:>2}  g={:>2}  i10={:>2}  PR_mean={:.5}",
                    d.code, d.name, d.paper_count, d.total_citations, d.h_index, d.g_index, d.i10_index, d.mean_pagerank_flow
                );
            }
            if let Some(ref json_p) = export_json {
                export_impact_digest_json(&analytics, horizon, json_p)
                    .expect("Impact digest JSON write failed");
                println!("Exported Impact Digest JSON -> {:?}", json_p);
            }
            if let Some(ref csv_p) = export_csv {
                export_impact_digest_csv(&analytics, horizon, csv_p)
                    .expect("Impact digest CSV write failed");
                println!("Exported Impact Digest CSV -> {:?}", csv_p);
            }
        }
    }
    flush_stdio();
}

#[allow(clippy::too_many_arguments)]
fn run_harvest_pipeline(
    req: SearchRequest,
    email: &str,
    db: Option<PathBuf>,
    download_dir: Option<PathBuf>,
    export_csv: Option<PathBuf>,
    export_csv_dir: Option<PathBuf>,
    export_graphml_p: Option<PathBuf>,
    export_gexf_p: Option<PathBuf>,
    export_json: Option<PathBuf>,
) {
    println!("Harvesting scholarly sources for '{}'...", req.query);
    match execute_multi_provider_search(&req) {
        Ok(mut harvest) => {
            println!(
                "Harvested {} deduplicated papers and {} directed citation edges.",
                harvest.papers.len(),
                harvest.edges.len()
            );
            for (prov, cnt) in &harvest.provider_counts {
                println!("  - {}: {} records", prov, cnt);
            }
            for w in &harvest.warnings {
                eprintln!("  [notice] {}", w);
            }

            if let Some(ref dl_dir) = download_dir {
                if let Err(e) =
                    batch_download_corpus_pdfs(&mut harvest.papers, dl_dir, email, 450, None)
                {
                    eprintln!("Download batch error: {}", e);
                    flush_stdio();
                    std::process::exit(1);
                }
            }

            if let Some(ref db_path) = db {
                if let Err(e) = LedgerStore::open(db_path)
                    .and_then(|mut store| store.save_corpus(&harvest.papers, &harvest.edges, false))
                {
                    eprintln!("Failed saving harvested corpus to {:?}: {}", db_path, e);
                    flush_stdio();
                    std::process::exit(1);
                }
            }

            if !harvest.papers.is_empty() {
                let params = AnalyticsParams::default();
                let analytics =
                    compute_corpus_analytics(&harvest.papers, &harvest.edges, &params);
                print_console_digest(&analytics);

                if let Some(ref csv_p) = export_csv {
                    if let Err(e) = export_metrics_csv(&analytics, csv_p) {
                        eprintln!("Failed exporting metrics CSV {:?}: {}", csv_p, e);
                        flush_stdio();
                        std::process::exit(1);
                    }
                }
                if let Some(ref csv_dir) = export_csv_dir {
                    if let Err(e) =
                        export_all_csv_suite(&harvest.papers, &harvest.edges, &analytics, csv_dir)
                    {
                        eprintln!("Failed exporting CSV suite {:?}: {}", csv_dir, e);
                        flush_stdio();
                        std::process::exit(1);
                    }
                }
                if let Some(ref gml_p) = export_graphml_p {
                    if let Err(e) = export_graphml(
                        &harvest.papers,
                        &harvest.edges,
                        &analytics,
                        None,
                        NodeColorEncoding::Discipline,
                        true,
                        gml_p,
                    ) {
                        eprintln!("Failed exporting GraphML {:?}: {}", gml_p, e);
                        flush_stdio();
                        std::process::exit(1);
                    }
                }
                if let Some(ref gxf_p) = export_gexf_p {
                    if let Err(e) = export_gexf(
                        &harvest.papers,
                        &harvest.edges,
                        &analytics,
                        None,
                        NodeColorEncoding::Discipline,
                        true,
                        gxf_p,
                    ) {
                        eprintln!("Failed exporting GEXF {:?}: {}", gxf_p, e);
                        flush_stdio();
                        std::process::exit(1);
                    }
                }
                if let Some(ref json_p) = export_json {
                    if let Err(e) =
                        export_bundle_json(&harvest.papers, &harvest.edges, &analytics, json_p)
                    {
                        eprintln!("Failed exporting JSON bundle {:?}: {}", json_p, e);
                        flush_stdio();
                        std::process::exit(1);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Harvest failed: {}", e);
            flush_stdio();
            std::process::exit(1);
        }
    }
}

fn print_console_digest(analytics: &cit_atlas::models::CorpusAnalytics) {
    let retracted_n = analytics
        .papers
        .iter()
        .filter(|p| p.record.retraction.status.is_retracted_or_withdrawn())
        .count();
    let ds_total: usize = analytics.papers.iter().map(|p| p.dataset_bridge_count).sum();
    let sw_total: usize = analytics.papers.iter().map(|p| p.software_bridge_count).sum();
    let up_total: usize = analytics.papers.iter().map(|p| p.uniprot_bridge_count).sum();
    let pdb_total: usize = analytics.papers.iter().map(|p| p.pdb_bridge_count).sum();
    let epmc_total: usize = analytics.papers.iter().map(|p| p.epmc_annotation_count).sum();

    println!(
        "\n=== CitAtlas Scientometric Digest ({} papers, {} citations, {} cross-field | {} retracted, {} datasets, {} software, {} UniProt, {} PDB, {} EPMC terms) ===",
        analytics.papers.len(),
        analytics.active_edges.len(),
        analytics.cross_edge_count,
        retracted_n,
        ds_total,
        sw_total,
        up_total,
        pdb_total,
        epmc_total
    );

    let mut by_bridge = analytics.papers.clone();
    by_bridge.sort_by(|a, b| {
        b.cb_undir
            .partial_cmp(&a.cb_undir)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    println!("\nTop 5 Geodesic Betweenness Bridge Papers:");
    for p in by_bridge.iter().take(5) {
        println!(
            "  - {:<14} [{}] CB_u={:>5.1}%  P_i={:.2} ({})  v_exp={:.2}  B={:.1}  integrity={}",
            p.record.id,
            p.record.disc.code(),
            p.cb_undir * 100.0,
            p.part_p,
            p.role_code,
            p.v_exp,
            p.beauty_b,
            p.record.retraction.status.as_str()
        );
    }

    println!(
        "\nTopological Communities: Louvain Q={:.3} ({} comms, NMI vs Disc={:.3}) | Infomap L(M)={:.3}b ({} mods, {:.1}% compression)",
        analytics.communities.louvain.modularity_q,
        analytics.communities.louvain.num_communities,
        analytics.communities.disc_vs_louvain.nmi,
        analytics.communities.infomap.codelength_bits,
        analytics.communities.infomap.num_modules,
        analytics.communities.infomap.relative_compression_pct
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use cit_atlas::analytics::{compute_corpus_analytics, compute_jsd, find_shortest_epistemic_path};
    use cit_atlas::community_detection::{
        compare_partitions, run_infomap_compression, run_louvain_modularity,
    };
    use cit_atlas::corpus::{build_empirical_corpus, generate_synthetic_corpus};
    use cit_atlas::crawler::{is_path_allowed_by_robots, parse_google_scholar_html};
    use cit_atlas::downloader::is_valid_pdf_bytes;
    use cit_atlas::export::{csv_escape, export_all_csv_suite, export_gexf, export_graphml};
    use cit_atlas::graph_viewer::{
        compute_preset_positions, compute_spotlight_mask, EgoSpotlightMode, GraphViewerState,
        NodeColorEncoding, ViewerLayoutPreset,
    };
    use cit_atlas::integrity_bridges::{
        parse_crossref_work_retraction, parse_datacite_list_for_paper,
        screen_papers_against_rw_csv, synthesize_shared_artifact_edges,
    };
    use cit_atlas::models::{
        AnalyticsParams, DataCiteArtifactKind, DataCiteLink, Discipline, RetractionStatus,
        SyntheticRegime,
    };
    use cit_atlas::search_providers::{parse_arxiv_atom_feed, reconstruct_openalex_abstract};
    use cit_atlas::storage::{import_corpus_file, parse_rfc4180_csv};
    use cit_atlas::wgpu_graph::{
        build_gpu_upload_payload, encode_camera_uniforms_bytes, encode_sim_uniforms_bytes,
        CameraUniformsPayload, SimUniformsPayload, SpatialPickGrid, WGSL_GRAPH_COMPUTE,
        WGSL_GRAPH_RENDER,
    };
    use serde_json::json;

    #[test]
    fn empirical_corpus_is_connected_and_finite() {
        let (papers, edges) = build_empirical_corpus();
        assert_eq!(papers.len(), 47);
        assert_eq!(edges.len(), 78);
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let src = analytics
            .papers
            .iter()
            .position(|p| p.record.id == "lin2023")
            .unwrap();
        let dst = analytics
            .papers
            .iter()
            .position(|p| p.record.id == "price1965")
            .unwrap();
        assert!(find_shortest_epistemic_path(&analytics, src, dst, false).is_some());
    }

    #[test]
    fn jsd_bounds_and_symmetry() {
        let mut p = [0.0; 12];
        let mut q = [0.0; 12];
        p[0] = 1.0;
        q[1] = 1.0;
        assert!((compute_jsd(&p, &q) - 1.0).abs() < 1e-9);
        assert!(compute_jsd(&p, &p).abs() < 1e-12);
    }

    #[test]
    fn openalex_inverted_abstract_reconstructs() {
        let inv = json!({ "Mutual": [0], "information": [1], "tracks": [2], "drift": [3] });
        assert_eq!(
            reconstruct_openalex_abstract(&inv),
            "Mutual information tracks drift"
        );
    }

    #[test]
    fn arxiv_atom_parser_extracts_pdf_and_year() {
        let sample = r#"<feed><entry><id>http://arxiv.org/abs/1706.03762v7</id><published>2017-06-12T17:57:34Z</published><title>Attention Is All You Need</title><summary>Transformer model.</summary><author><name>Ashish Vaswani</name></author></entry></feed>"#;
        let parsed = parse_arxiv_atom_feed(sample);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].arxiv_id, "1706.03762");
    }

    #[test]
    fn pdf_magic_header_rejects_html_landing_pages() {
        assert!(!is_valid_pdf_bytes(&vec![b'<'; 2048]));
        let mut real_pdf = b"%PDF-1.7\n".to_vec();
        real_pdf.resize(2048, 0x20);
        assert!(is_valid_pdf_bytes(&real_pdf));
    }

    #[test]
    fn louvain_and_infomap_recover_planted_cliques() {
        let mut undir = vec![Vec::new(); 10];
        let mut out = vec![Vec::new(); 10];
        for i in 0..5 {
            for j in (i + 1)..5 {
                undir[i].push(j);
                undir[j].push(i);
                out[j].push(i);
            }
        }
        for i in 5..10 {
            for j in (i + 1)..10 {
                undir[i].push(j);
                undir[j].push(i);
                out[j].push(i);
            }
        }
        undir[4].push(5);
        undir[5].push(4);
        out[5].push(4);
        let discs = [
            Discipline::INF, Discipline::INF, Discipline::INF, Discipline::INF, Discipline::INF,
            Discipline::EVO, Discipline::EVO, Discipline::EVO, Discipline::EVO, Discipline::EVO,
        ];
        let ids: Vec<String> = (0..10).map(|i| format!("n{}", i)).collect();
        let cb = vec![0.1; 10];
        let louv = run_louvain_modularity(&undir, &discs, &ids, &cb, 1.0);
        assert_eq!(louv.num_communities, 2);
        let info = run_infomap_compression(&out, &undir, &discs, &ids, &cb, 0.15);
        assert!(info.num_modules >= 2);
        let align = compare_partitions("GT", &[0, 0, 0, 0, 0, 1, 1, 1, 1, 1], "Louvain", &louv.assignments);
        assert!((align.nmi - 1.0).abs() < 1e-9);
    }

    #[test]
    fn google_scholar_html_parser_extracts_citations_pdf_and_doi() {
        let sample_html = r#"
        <div class="gs_r gs_or gs_scl" data-cid="k8X9abc123J">
          <div class="gs_ggs gs_fl"><div class="gs_or_ggsm"><a href="https://www.pnas.org/content/pnas/112/24/7426.full.pdf">[PDF] pnas.org</a></div></div>
          <div class="gs_ri">
            <h3 class="gs_rt"><a href="https://doi.org/10.1073/pnas.1424329112">Defining and identifying <b>Sleeping Beauties</b> in science</a></h3>
            <div class="gs_a">Q Ke, E Ferrara, F Radicchi, A Flammini - PNAS, 2015 - National Acad Sciences</div>
            <div class="gs_rs">Parameter-free beauty coefficient B.</div>
            <div class="gs_fl"><a href="/scholar?cites=1549876543210987654&amp;hl=en">Cited by 642</a></div>
          </div>
        </div>"#;
        let hits = parse_google_scholar_html(sample_html).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].cited_by_count, Some(642));
    }

    #[test]
    fn robots_txt_longest_match_precedence() {
        let robots = "User-agent: *\nDisallow: /scholar\nAllow: /scholar/about\n";
        assert!(!is_path_allowed_by_robots(robots, "CitAtlasCrawler", "/scholar?q=entropy"));
        assert!(is_path_allowed_by_robots(robots, "CitAtlasCrawler", "/scholar/about"));
    }

    #[test]
    fn crossref_retraction_watch_json_and_csv_screening() {
        let cr_work = json!({
            "DOI": "10.1016/s0140-6736(20)31180-6",
            "updated-by": [{
                "type": "retraction",
                "source": "retraction-watch",
                "record-id": "22941",
                "updated": { "date-parts": [[2020, 6, 5]] }
            }]
        });
        let parsed = parse_crossref_work_retraction(&cr_work);
        assert_eq!(parsed.status, RetractionStatus::Retracted);

        let (mut papers, _) = build_empirical_corpus();
        let target_doi = papers[0].doi.clone();
        let rw_csv = format!(
            "Record ID,OriginalPaperDOI,OriginalPaperPubMedID,RetractionDOI,RetractionDate,RetractionNature,Reason\n55102,{},0,10.1000/ret.2022,2022-04-15,Retraction,+Unreliable Results;\n",
            target_doi
        );
        assert_eq!(screen_papers_against_rw_csv(&mut papers, &rw_csv), 1);
    }

    #[test]
    fn datacite_dataset_and_software_bridges_and_shared_edges() {
        let dc_payload = json!({
            "data": [{
                "attributes": {
                    "doi": "10.5281/zenodo.778899",
                    "titles": [{ "title": "Benchmark Dataset" }],
                    "publisher": "Zenodo",
                    "types": { "resourceTypeGeneral": "Dataset" },
                    "relatedIdentifiers": [{
                        "relatedIdentifier": "10.1073/pnas.1424329112",
                        "relatedIdentifierType": "DOI",
                        "relationType": "IsSupplementTo"
                    }],
                    "downloadCount": 312
                }
            }]
        });
        let links = parse_datacite_list_for_paper(&dc_payload, "10.1073/pnas.1424329112");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].artifact_kind, DataCiteArtifactKind::Dataset);

        let (mut papers, _) = build_empirical_corpus();
        let base_edges = synthesize_shared_artifact_edges(&papers).len();
        papers[0].datacite_links.push(links[0].clone());
        papers[1].datacite_links.push(links[0].clone());
        let after_edges = synthesize_shared_artifact_edges(&papers);
        assert_eq!(after_edges.len(), base_edges + 1);
        assert!(after_edges
            .iter()
            .any(|e| e.provenance.contains("10.5281/zenodo.778899")));
    }

    #[test]
    fn rfc4180_csv_suite_roundtrip_and_retraction_datacite_columns() {
        let escaped = csv_escape("Title with \"quotes\", commas, and\nnewline");
        let parsed = parse_rfc4180_csv(&format!("col_a,col_b\n{},42\n", escaped));
        assert_eq!(parsed[1][0], "Title with \"quotes\", commas, and\nnewline");

        let (papers, edges) = build_empirical_corpus();
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let tmp_dir = std::env::temp_dir().join("cit_atlas_csv_test_v150");
        let files = export_all_csv_suite(&papers, &edges, &analytics, &tmp_dir).unwrap();
        assert_eq!(files.len(), 10);
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn graph_viewer_layouts_and_force_physics_stay_bounded() {
        let (papers, edges) = build_empirical_corpus();
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        for preset in [
            ViewerLayoutPreset::ForceAtlas2,
            ViewerLayoutPreset::HierarchicalDag,
            ViewerLayoutPreset::CommunityGalaxies,
            ViewerLayoutPreset::RadialEgo,
            ViewerLayoutPreset::CartographicPlane,
        ] {
            let coords = compute_preset_positions(
                &analytics,
                preset,
                NodeColorEncoding::LouvainCommunity,
                "shannon1948",
            );
            assert_eq!(coords.len(), 47);
        }

        let mut viewer = GraphViewerState::default();
        viewer.sync_with_analytics(&analytics, "shannon1948");
        viewer.pinned_ids.insert("shannon1948".to_string());
        let pinned_before = viewer.positions["shannon1948"];
        for _ in 0..3 {
            viewer.step_physics(&analytics);
        }
        assert_eq!(viewer.positions["shannon1948"], pinned_before);
        let ego_mask = compute_spotlight_mask(&analytics, "shannon1948", EgoSpotlightMode::OneHopEgo);
        assert!(ego_mask.iter().filter(|&&b| b).count() >= 2);
    }

    #[test]
    fn graphml_export_and_roundtrip_import_with_datacite_nodes() {
        let (mut papers, edges) = build_empirical_corpus();
        papers[0].datacite_links.push(DataCiteLink {
            artifact_doi: "10.5281/zenodo.999999".to_string(),
            artifact_title: "Shannon Channel Dataset".to_string(),
            artifact_kind: DataCiteArtifactKind::Dataset,
            repository: "Zenodo".to_string(),
            relation_type: "IsSupplementTo".to_string(),
            citation_count: 5,
            view_count: 100,
            download_count: 50,
        });
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let tmp_gml = std::env::temp_dir().join("cit_atlas_test_v150.graphml");
        export_graphml(
            &papers,
            &edges,
            &analytics,
            None,
            NodeColorEncoding::LouvainCommunity,
            true,
            &tmp_gml,
        )
        .unwrap();
        let (reloaded_p, reloaded_e) = import_corpus_file(&tmp_gml).unwrap();
        assert_eq!(reloaded_p.len(), 47);
        assert_eq!(reloaded_e.len(), 78);
        let _ = std::fs::remove_file(&tmp_gml);
    }

    #[test]
    fn gexf_13_viz_export_and_roundtrip_import() {
        let (papers, edges) = build_empirical_corpus();
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let tmp_gxf = std::env::temp_dir().join("cit_atlas_test_v150.gexf");
        export_gexf(
            &papers,
            &edges,
            &analytics,
            None,
            NodeColorEncoding::Discipline,
            true,
            &tmp_gxf,
        )
        .unwrap();
        let (reloaded_p, reloaded_e) = import_corpus_file(&tmp_gxf).unwrap();
        assert_eq!(reloaded_p.len(), 47);
        assert_eq!(reloaded_e.len(), 78);
        let _ = std::fs::remove_file(&tmp_gxf);
    }

    #[test]
    fn massive_50k_corpus_generates_and_analyzes_in_linear_time() {
        let (papers, edges) =
            generate_synthetic_corpus(50_000, 2026, 0.18, SyntheticRegime::Convergent);
        assert_eq!(papers.len(), 50_000);
        assert!(edges.len() >= 100_000);

        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        assert_eq!(analytics.papers.len(), 50_000);
        assert!(analytics.communities.louvain.modularity_q > 0.05);
        assert!(analytics.communities.infomap.num_modules >= 2);
    }

    #[test]
    fn wgpu_csr_and_aligned_byte_encoders_pack_50k_nodes_cleanly() {
        assert!(WGSL_GRAPH_COMPUTE.contains("fn cs_clear_grids"));
        assert!(WGSL_GRAPH_COMPUTE.contains("fn cs_bin_nodes"));
        assert!(WGSL_GRAPH_COMPUTE.contains("fn cs_forces_and_integrate"));
        assert!(WGSL_GRAPH_RENDER.contains("fn vs_edge"));
        assert!(WGSL_GRAPH_RENDER.contains("fn fs_edge"));
        assert!(WGSL_GRAPH_RENDER.contains("fn vs_node"));
        assert!(WGSL_GRAPH_RENDER.contains("fn fs_node"));

        let sim_b = encode_sim_uniforms_bytes(&SimUniformsPayload {
            node_count: 50_000,
            grid_dim: 128,
            super_dim: 16,
            run_step: 1,
            world_extent: 1.25,
            repulsion_k: 0.42,
            spring_k: 0.55,
            gravity_k: 0.08,
            cohesion_k: 0.16,
            damping: 0.82,
            ideal_len: 0.009,
            max_step: 0.03,
            dragged_idx: -1,
            drag_x: 0.0,
            drag_y: 0.0,
            pad0: 0.0,
        });
        assert_eq!(sim_b.len(), 64);

        let cam_b = encode_camera_uniforms_bytes(&CameraUniformsPayload {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            aspect: 1.6,
            viewport_w: 1280.0,
            viewport_h: 800.0,
            min_edge_eb: 0.0,
            edge_alpha_scale: 0.35,
            node_scale: 1.0,
            show_arrows: 1,
            curved_edges: 1,
            hovered_idx: -1,
        });
        assert_eq!(cam_b.len(), 48);

        let (papers, edges) = build_empirical_corpus();
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let mut viewer = GraphViewerState::default();
        viewer.sync_with_analytics(&analytics, "shannon1948");
        let upload = build_gpu_upload_payload(1, &analytics, &viewer, "shannon1948", &HashSet::new());
        assert_eq!(upload.node_bytes.len(), upload.node_count * 48);
        assert_eq!(upload.edge_bytes.len(), upload.edge_count * 16);
    }

    #[test]
    fn spatial_pick_grid_queries_50k_nodes_in_o1_time() {
        let mut coords = Vec::with_capacity(50_000);
        for i in 0..50_000 {
            let x = -1.0 + 2.0 * (((i * 7919) % 10_000) as f32 / 10_000.0);
            let y = -1.0 + 2.0 * (((i * 6271) % 10_000) as f32 / 10_000.0);
            coords.push([x, y]);
        }
        coords[42_123] = [0.1234, -0.4321];

        let mut grid = SpatialPickGrid::default();
        grid.rebuild(&coords);
        let picked = grid.pick_nearest(0.1234, -0.4321, 0.02);
        assert!(picked.is_some());
        let [px, py] = coords[picked.unwrap()];
        assert!((px - 0.1234).abs() <= 0.02 && (py + 0.4321).abs() <= 0.02);
    }

    #[test]
    fn europe_pmc_annotations_parser_extracts_go_efo_chebi_and_uniprot() {
        use cit_atlas::bio_entity_bridges::parse_europe_pmc_annotations_json;
        use cit_atlas::models::BioEntityKind;

        let sample = json!([{
            "source": "PMC",
            "pmcid": "PMC6227811",
            "annotations": [
                {
                    "exact": "Spike glycoprotein",
                    "type": "Gene_Proteins",
                    "section": "Results",
                    "provider": "Europe PMC",
                    "frequency": 4,
                    "tags": [{ "name": "Spike glycoprotein", "uri": "https://identifiers.org/uniprot:P0DTC2" }]
                },
                {
                    "exact": "6VXX",
                    "type": "Accession Numbers",
                    "subType": "pdb",
                    "section": "Methods",
                    "provider": "Europe PMC",
                    "tags": [{ "name": "6VXX", "uri": "https://identifiers.org/pdb:6VXX" }]
                },
                {
                    "exact": "protein folding",
                    "type": "Gene Ontology",
                    "section": "Abstract",
                    "provider": "Europe PMC",
                    "tags": [{ "name": "protein folding", "uri": "http://purl.obolibrary.org/obo/GO_0006457" }]
                },
                {
                    "exact": "COVID-19",
                    "type": "Diseases",
                    "section": "Abstract",
                    "provider": "OpenTargets",
                    "tags": [{ "name": "COVID-19", "uri": "http://www.ebi.ac.uk/efo/EFO_0009747" }]
                }
            ]
        }]);
        let by_art = parse_europe_pmc_annotations_json(&sample);
        let links = by_art.get("PMC:PMC6227811").expect("PMC key missing");
        assert_eq!(links.len(), 4);
        assert!(links.iter().any(|l| l.kind == BioEntityKind::UniProt && l.entity_id == "P0DTC2"));
        assert!(links.iter().any(|l| l.kind == BioEntityKind::PdbStructure && l.entity_id == "6VXX"));
        assert!(links.iter().any(|l| l.kind == BioEntityKind::GeneOntology && l.entity_id == "GO:0006457"));
        assert!(links.iter().any(|l| l.kind == BioEntityKind::DiseaseEfo && l.entity_id == "EFO:0009747"));
    }

    #[test]
    fn uniprot_rest_v2_and_rcsb_pdb_entry_parsers_extract_structural_xrefs() {
        use cit_atlas::bio_entity_bridges::{parse_rcsb_pdb_entry_json, parse_uniprot_entry_json};
        use cit_atlas::models::BioEntityKind;

        let uniprot_json = json!({
            "primaryAccession": "P0DTC2",
            "uniProtkbId": "SPIKE_SARS2",
            "proteinDescription": {
                "recommendedName": { "fullName": { "value": "Spike glycoprotein" } }
            },
            "genes": [{ "geneName": { "value": "S" } }],
            "organism": { "scientificName": "SARS-CoV-2" },
            "uniProtKBCrossReferences": [
                { "database": "PDB", "id": "6VXX" },
                { "database": "PDB", "id": "6VSB" }
            ]
        });
        let up_link = parse_uniprot_entry_json(&uniprot_json).unwrap();
        assert_eq!(up_link.kind, BioEntityKind::UniProt);
        assert_eq!(up_link.entity_id, "P0DTC2");
        assert_eq!(up_link.linked_pdb_ids, vec!["6VXX".to_string(), "6VSB".to_string()]);

        let pdb_json = json!({
            "rcsb_id": "6VXX",
            "struct": { "title": "Structure of the SARS-CoV-2 spike glycoprotein" },
            "exptl": [{ "method": "ELECTRON MICROSCOPY" }],
            "rcsb_entry_info": { "resolution_combined": [2.80] }
        });
        let pdb_link = parse_rcsb_pdb_entry_json(&pdb_json).unwrap();
        assert_eq!(pdb_link.kind, BioEntityKind::PdbStructure);
        assert_eq!(pdb_link.entity_id, "6VXX");
        assert!((pdb_link.resolution_angstrom.unwrap() - 2.80).abs() < 1e-3);
    }

    #[test]
    fn shared_bio_entity_bridges_and_uniprot_pdb_cross_edges_project_cleanly() {
        use cit_atlas::bio_entity_bridges::{
            extract_uniprot_pdb_cross_edges, synthesize_shared_bio_entity_edges,
        };

        let (papers, _) = build_empirical_corpus();
        let bio_edges = synthesize_shared_bio_entity_edges(&papers);
        assert!(!bio_edges.is_empty());
        let cross = extract_uniprot_pdb_cross_edges(&papers);
        assert!(cross.contains(&(
            "bio:uniprot:P0DTC2".to_string(),
            "bio:pdb_structure:6VXX".to_string()
        )));
    }

    #[test]
    fn wgpu_upload_includes_bipartite_bio_entity_nodes_and_sdf_shape_flags() {
        let (papers, edges) = build_empirical_corpus();
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let mut viewer = GraphViewerState::default();
        viewer.sync_with_analytics(&analytics, "jumper2021");
        assert!(!viewer.bio_entity_nodes.is_empty());
        assert!(!viewer.bio_cross_edges.is_empty());

        let upload = build_gpu_upload_payload(2, &analytics, &viewer, "jumper2021", &HashSet::new());
        assert!(upload.node_count > 47 + viewer.artifact_nodes.len());
        assert_eq!(upload.node_bytes.len(), upload.node_count * 48);
        assert!(WGSL_GRAPH_RENDER.contains("is_hex_uniprot"));
        assert!(WGSL_GRAPH_RENDER.contains("is_oct_pdb"));
        assert!(WGSL_GRAPH_RENDER.contains("is_tri_epmc"));
    }

    #[test]
    fn token_bucket_enforces_burst_depletion_refill_and_min_spacing() {
        use std::time::{Duration, Instant};
        use cit_atlas::rate_limiter::{AcquirePermit, DomainRateLimiter};

        let mut lim = DomainRateLimiter::new_with_defaults();
        lim.update_domain_policy("test.scholarly.org", 2.0, 2.0, 400, 0, true, false);
        let t0 = Instant::now();

        // First request consumes 1 token with 0 wait
        let (p1, w1) = lim.reserve_permit_at("https://test.scholarly.org/v1/works", t0);
        assert_eq!(w1, Duration::ZERO);
        assert!(matches!(p1, AcquirePermit::Granted { wait_ms: 0, .. }));

        // Immediate second request hits the 400ms min_spacing floor
        let (_p2, w2) = lim.reserve_permit_at("https://test.scholarly.org/v1/works", t0);
        assert!(w2 >= Duration::from_millis(400));

        // Pausing the domain immediately trips CircuitOpen
        lim.update_domain_policy("test.scholarly.org", 2.0, 2.0, 400, 0, true, true);
        let (p3, _) = lim.reserve_permit_at("https://test.scholarly.org/v1/works", t0 + Duration::from_secs(2));
        assert!(matches!(p3, AcquirePermit::CircuitOpen { .. }));
    }

    #[test]
    fn retry_after_and_crossref_rate_headers_parse_cleanly() {
        use reqwest::header::{HeaderMap, HeaderValue};
        use cit_atlas::rate_limiter::{parse_crossref_rate_headers, parse_retry_after_ms};

        assert_eq!(parse_retry_after_ms("12"), Some(12_000));
        assert_eq!(parse_retry_after_ms("2.5"), Some(2_500));
        assert_eq!(parse_retry_after_ms("Fri, 31 Dec 2026 23:59:59 GMT"), None);

        let mut hdrs = HeaderMap::new();
        hdrs.insert("x-rate-limit-limit", HeaderValue::from_static("50"));
        hdrs.insert("x-rate-limit-interval", HeaderValue::from_static("1s"));
        hdrs.insert("x-api-pool", HeaderValue::from_static("polite"));
        let (rps, note) = parse_crossref_rate_headers(&hdrs).unwrap();
        assert!((rps - 50.0).abs() < 1e-6);
        assert!(note.contains("polite"));
    }

    #[test]
    fn robots_crawl_delay_prefers_specific_agent_over_wildcard() {
        use cit_atlas::rate_limiter::parse_robots_crawl_delay_ms;

        let robots = "User-agent: *\nCrawl-delay: 10\n\nUser-agent: CitAtlasCrawler\nCrawl-delay: 2.5\n";
        assert_eq!(parse_robots_crawl_delay_ms(robots, "CitAtlasCrawler"), Some(2_500));
        assert_eq!(parse_robots_crawl_delay_ms("User-agent: *\nCrawl-delay: 4\n", "CitAtlasCrawler"), Some(4_000));
    }

    #[test]
    fn sqlite_and_csv_roundtrip_for_rate_policies_and_crawl_audit_log() {
        use cit_atlas::export::{export_crawl_audit_csv, export_domain_policies_csv};
        use cit_atlas::rate_limiter::DomainRateLimiter;
        use cit_atlas::storage::LedgerStore;

        let mut lim = DomainRateLimiter::new_with_defaults();
        lim.synthesize_demo_audit_burst();
        let pols = lim.snapshot_policies();
        let logs = lim.snapshot_audit_log();
        assert_eq!(logs.len(), 18);

        let tmp_db = std::env::temp_dir().join("cit_atlas_audit_test_v180.db");
        let _ = std::fs::remove_file(&tmp_db);
        {
            let mut store = LedgerStore::open(&tmp_db).unwrap();
            store.save_rate_policies_and_audit(&pols, &logs).unwrap();
        }
        {
            let store = LedgerStore::open(&tmp_db).unwrap();
            let (loaded_pols, loaded_logs) = store.load_rate_policies_and_audit().unwrap();
            assert_eq!(loaded_pols.len(), pols.len());
            assert_eq!(loaded_logs.len(), 18);
        }
        let tmp_audit_csv = std::env::temp_dir().join("cit_atlas_audit_v170.csv");
        let tmp_pol_csv = std::env::temp_dir().join("cit_atlas_pol_v170.csv");
        export_crawl_audit_csv(&logs, &tmp_audit_csv).unwrap();
        export_domain_policies_csv(&pols, &tmp_pol_csv).unwrap();

        let _ = std::fs::remove_file(&tmp_db);
        let _ = std::fs::remove_file(&tmp_audit_csv);
        let _ = std::fs::remove_file(&tmp_pol_csv);
    }

    #[test]
    fn openaire_graph_and_scholexplorer_parsers_extract_metadata_pids_and_citation_links() {
        use cit_atlas::search_providers::{
            parse_openaire_research_product, parse_scholexplorer_links_json,
        };

        let sample_prod = json!({
            "id": "doi_dedup___::abc123456789",
            "mainTitle": "Information-Theoretic Selection in Complex Microbial Communities",
            "publicationDate": "2024-05-14",
            "authors": [
                { "fullName": "Elena Rostova" },
                { "name": "Marcus", "surname": "Vance" }
            ],
            "pids": [
                { "scheme": "doi", "value": "10.1371/journal.pcbi.1011999" },
                { "scheme": "pmc", "value": "PMC10998877" }
            ],
            "container": { "name": "PLOS Computational Biology" },
            "bestAccessRight": { "label": "OPEN ACCESS" },
            "indicators": { "citationImpact": { "citationCount": 29 } },
            "descriptions": ["Mutual information bounds evolutionary adaptation across trophic guilds."],
            "subjects": [{ "subject": { "value": "Evolutionary Biology" } }]
        });
        let rec = parse_openaire_research_product(&sample_prod, 0).expect("OpenAIRE parse failed");
        assert_eq!(rec.doi, "10.1371/journal.pcbi.1011999");
        assert_eq!(rec.pmcid, "PMC10998877");
        assert_eq!(rec.year, 2024);
        assert_eq!(rec.external_citations, Some(29));

        let scholix = json!({
            "result": [
                {
                    "RelationshipType": { "Name": "References" },
                    "Source": { "Identifier": [{ "IDScheme": "doi", "ID": "10.1371/journal.pcbi.1011999" }] },
                    "Target": {
                        "Type": { "Name": "literature" },
                        "Identifier": [{ "IDScheme": "doi", "ID": "10.1073/pnas.1424329112" }]
                    }
                },
                {
                    "RelationshipType": { "Name": "IsSupplementedBy" },
                    "Source": { "Identifier": [{ "IDScheme": "doi", "ID": "10.1371/journal.pcbi.1011999" }] },
                    "Target": {
                        "Type": { "Name": "dataset" },
                        "Title": "Microbial Guild Time-Series Archive",
                        "Publisher": [{ "name": "Zenodo" }],
                        "Identifier": [{ "IDScheme": "doi", "ID": "10.5281/zenodo.8844221" }]
                    }
                }
            ]
        });
        let (refs, dcs) = parse_scholexplorer_links_json(&scholix, "10.1371/journal.pcbi.1011999");
        assert!(refs.contains(&"DOI:10.1073/pnas.1424329112".to_string()));
        assert_eq!(dcs.len(), 1);
        assert_eq!(dcs[0].artifact_doi, "10.5281/zenodo.8844221");
    }

    #[test]
    fn inspire_hep_literature_parser_extracts_recid_arxiv_and_reference_graph() {
        use cit_atlas::search_providers::{merge_and_link_records, parse_inspire_literature_hit};

        let hit_a = json!({
            "id": "2700101",
            "metadata": {
                "control_number": 2700101,
                "titles": [{ "title": "Entanglement Entropy and Renormalization Group Flows" }],
                "authors": [{ "full_name": "Kovacs, Sandor" }],
                "earliest_date": "2023-11-02",
                "citation_count": 48,
                "citation_count_without_self_citations": 41,
                "dois": [{ "value": "10.1007/JHEP11(2023)042" }],
                "arxiv_eprints": [{ "value": "2310.09876", "categories": ["hep-th"] }],
                "publication_info": [{ "journal_title": "JHEP", "year": 2023 }],
                "references": [
                    { "record": { "$ref": "https://inspirehep.net/api/literature/1900202" } }
                ]
            }
        });
        let hit_b = json!({
            "id": "1900202",
            "metadata": {
                "control_number": 1900202,
                "titles": [{ "title": "Holographic Mutual Information in Quantum Networks" }],
                "authors": [{ "full_name": "Takayanagi, R." }],
                "earliest_date": "2021-03-19",
                "citation_count": 195
            }
        });

        let rec_a = parse_inspire_literature_hit(&hit_a, 0, true).unwrap();
        let rec_b = parse_inspire_literature_hit(&hit_b, 1, false).unwrap();
        assert_eq!(rec_a.external_citations, Some(41));
        assert_eq!(rec_a.arxiv_id, "2310.09876");
        assert!(rec_a.oa_pdf_url.contains("2310.09876.pdf"));

        let (merged, edges) = merge_and_link_records(vec![rec_a, rec_b], false, None, None);
        assert_eq!(merged.len(), 2);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "insp_2700101");
        assert_eq!(edges[0].target, "insp_1900202");
    }

    #[test]
    fn nasa_ads_solr_parser_extracts_bibcode_eprint_gateway_and_bidirectional_citations() {
        use cit_atlas::search_providers::{merge_and_link_records, parse_nasa_ads_doc};

        let doc_seed = json!({
            "bibcode": "2022ApJ...931...44B",
            "title": ["Bayesian Information Geometry of Exoplanet Biosignatures"],
            "author": ["Chen, L.", "Patel, R."],
            "year": "2022",
            "pub": "The Astrophysical Journal",
            "doi": ["10.3847/1538-4357/ac6789"],
            "identifier": ["arXiv:2204.01122"],
            "citation_count": 37,
            "esources": ["EPRINT_PDF", "PUB_PDF"],
            "reference": ["2018ApJ...855...12K"],
            "citation": ["2025AJ....169...88W"]
        });
        let doc_ref = json!({
            "bibcode": "2018ApJ...855...12K",
            "title": ["AtmosphericDisequilibrium Entropy Metrics"],
            "author": ["Krissansen-Totton, J."],
            "year": "2018",
            "esources": ["EPRINT_PDF"]
        });
        let doc_citer = json!({
            "bibcode": "2025AJ....169...88W",
            "title": ["Habitable Worlds Observatory Spectral Yield Optimization"],
            "author": ["Ward, D."],
            "year": "2025"
        });

        let r_seed = parse_nasa_ads_doc(&doc_seed, 0).unwrap();
        let r_ref = parse_nasa_ads_doc(&doc_ref, 1).unwrap();
        let r_citer = parse_nasa_ads_doc(&doc_citer, 2).unwrap();
        assert!(r_ref.oa_pdf_url.contains("ui.adsabs.harvard.edu/link_gateway"));

        let (merged, edges) =
            merge_and_link_records(vec![r_seed.clone(), r_ref.clone(), r_citer.clone()], false, None, None);
        assert_eq!(merged.len(), 3);
        assert!(edges.iter().any(|e| e.source == r_seed.id && e.target == r_ref.id));
        assert!(edges.iter().any(|e| e.source == r_citer.id && e.target == r_seed.id));
    }

    #[test]
    fn rate_limiter_preconfigures_openaire_inspire_and_ads_buckets_and_parses_x_ratelimit_headers() {
        use reqwest::header::{HeaderMap, HeaderValue};
        use cit_atlas::models::SearchProvidersConfig;
        use cit_atlas::rate_limiter::{
            default_scholarly_policies_with_auth, parse_standard_ratelimit_headers,
            CrawlAuditOutcome, DomainRateLimiter,
        };

        let pols = default_scholarly_policies_with_auth(false, false, true, true);
        assert_eq!(pols.len(), 24);
        assert!(pols.iter().any(|p| p.domain == "api.openaire.eu" && (p.refill_rps - 1.8).abs() < 1e-6));
        assert!(pols.iter().any(|p| p.domain == "api.scholexplorer.openaire.eu"));
        assert!(pols.iter().any(|p| p.domain == "inspirehep.net" && (p.refill_rps - 2.4).abs() < 1e-6));
        assert!(pols.iter().any(|p| p.domain == "api.adsabs.harvard.edu" && (p.refill_rps - 4.0).abs() < 1e-6));
        assert!(pols.iter().any(|p| p.domain == "ui.adsabs.harvard.edu"));

        let cfg = SearchProvidersConfig::from_csv_flags("openaire,inspire,ads");
        assert!(cfg.openaire && cfg.inspire_hep && cfg.nasa_ads && !cfg.openalex);

        let mut hdrs = HeaderMap::new();
        hdrs.insert("x-ratelimit-limit", HeaderValue::from_static("5000"));
        hdrs.insert("x-ratelimit-remaining", HeaderValue::from_static("4987"));
        hdrs.insert("x-ratelimit-reset", HeaderValue::from_static("1799999999"));
        let (lim, rem, rst, desc) = parse_standard_ratelimit_headers(&hdrs).unwrap();
        assert_eq!(lim, 5000);
        assert_eq!(rem, 4987);
        assert_eq!(rst, Some(1_799_999_999));
        assert!(desc.contains("4987/5000"));

        let mut limiter = DomainRateLimiter::new_with_defaults();
        let entry = limiter.record_outcome(
            "search_api",
            "GET",
            "https://inspirehep.net/api/literature?q=entropy",
            CrawlAuditOutcome::Http429RateLimited,
            429,
            0,
            85,
            0,
            None,
            "n/a_api",
            "Simulated INSPIRE 429",
        );
        assert!(entry.retry_after_ms.unwrap_or(0) >= 5_500);
    }

    #[test]
    fn ads_bibcode_oa_downloader_fallback_and_client_token_overrides_work() {
        use cit_atlas::downloader::resolve_candidate_oa_urls;
        use cit_atlas::search_providers::{parse_nasa_ads_doc, ScholarlyHttpClient};

        let http = ScholarlyHttpClient::with_tokens(
            "research@crvscience.com",
            "oaire_demo_tok",
            "ads_demo_tok",
        )
        .unwrap();
        assert_eq!(http.openaire_token(), Some("oaire_demo_tok"));
        assert_eq!(http.nasa_ads_token(), Some("ads_demo_tok"));

        let doc = serde_json::json!({
            "bibcode": "2024MNRAS.528.1122G",
            "title": ["Stochastic Information Flow in Accretion Disks"],
            "author": ["Gao, Y."],
            "year": "2024"
        });
        let rec = parse_nasa_ads_doc(&doc, 0).unwrap();
        let stages = resolve_candidate_oa_urls(&http, &rec);
        assert!(stages.iter().any(|(stage, u)| {
            stage == "nasa_ads_eprint_pdf"
                && u.contains("ui.adsabs.harvard.edu/link_gateway/2024MNRAS.528.1122G/EPRINT_PDF")
        }));
    }

    #[test]
    fn comparative_bibliometric_impact_digest_computes_h_g_i10_pagerank_and_exports() {
        use cit_atlas::export::{
            build_bibliometric_impact_digest, export_impact_digest_csv, export_impact_digest_json,
            BibliometricImpactDigest,
        };

        let (papers, edges) = build_empirical_corpus();
        let analytics = compute_corpus_analytics(&papers, &edges, &AnalyticsParams::default());
        let digest = build_bibliometric_impact_digest(&analytics, 2026);
        assert_eq!(digest.total_papers, 47);
        assert!(digest.h_index >= 4);
        assert!(digest.g_index >= digest.h_index);
        assert!(digest.i10_index <= digest.total_papers);
        assert!(digest.louvain_modularity_q > 0.1);
        assert!(digest.max_pagerank_flow > 0.0);
        assert_eq!(digest.disciplines.len(), 6);
        assert_eq!(digest.top_pagerank_papers.len(), 10);

        // Verify i10-index counts papers with >= 10 citations
        let mut boosted = papers.clone();
        boosted[0].external_citations = Some(25);
        boosted[1].external_citations = Some(10);
        let boosted_analytics =
            compute_corpus_analytics(&boosted, &edges, &AnalyticsParams::default());
        let boosted_digest = build_bibliometric_impact_digest(&boosted_analytics, 2026);
        assert!(boosted_digest.i10_index >= 2);

        let tmp_dir = std::env::temp_dir().join(format!(
            "cit_atlas_impact_digest_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).expect("mkdir");
        let json_p = tmp_dir.join("impact_digest.json");
        let csv_p = tmp_dir.join("impact_digest.csv");

        export_impact_digest_json(&analytics, 2026, &json_p).expect("export json");
        export_impact_digest_csv(&analytics, 2026, &csv_p).expect("export csv");

        let parsed: BibliometricImpactDigest = serde_json::from_str(
            &std::fs::read_to_string(&json_p).expect("read json"),
        )
        .expect("parse json");
        assert_eq!(parsed.h_index, digest.h_index);
        assert_eq!(parsed.i10_index, digest.i10_index);

        let csv_raw = std::fs::read_to_string(&csv_p).expect("read csv");
        // 1 header + 1 corpus + 6 disciplines + 10 top_papers = 18 lines
        assert_eq!(csv_raw.lines().count(), 18);
        assert!(csv_raw.contains("louvain_modularity_q"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
