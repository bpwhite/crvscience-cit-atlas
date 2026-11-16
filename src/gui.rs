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
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use eframe::egui_wgpu;

use crate::analytics::{compute_corpus_analytics, find_shortest_epistemic_path};
use crate::community_detection::{community_rgb, PartitionScheme};
use crate::corpus::{build_empirical_corpus, generate_synthetic_corpus};
use crate::downloader::{batch_download_corpus_pdfs, download_single_paper_pdf};
use crate::export::{
    export_all_csv_suite, export_annual_trajectories_csv, export_bio_entity_bridges_csv, export_bundle_json,
    export_crawl_audit_csv, export_domain_policies_csv,
    export_communities_csv, export_edges_csv, export_gexf, export_graphml,
    export_integrity_and_datacite_csv, export_jsd_matrix_csv, export_metrics_csv,
    export_raw_papers_csv, export_synthesis_svg, export_topic_drift_csv, format_bibtex_corpus,
};
use crate::graph_viewer::{
    compute_spotlight_mask, node_radius_for_encoding, node_rgb_for_encoding, BioNodeFilter,
    EgoSpotlightMode, GraphViewerState, NodeColorEncoding, NodeSizeEncoding, ViewerLayoutPreset,
};
use crate::integrity_bridges::run_full_corpus_screening;
use crate::models::{
    AnalyticsParams, BioEntityKind, CitationEdge, CorpusAnalytics, Discipline, DownloadItemReport, PaperRecord,
    RetractionStatus, ScreeningSummary, SearchHarvestSummary, SearchRequest, SyntheticRegime,
    CONCEPTS,
};
use crate::search_providers::{
    execute_multi_provider_search, lookup_single_doi, merge_and_link_records, ScholarlyHttpClient,
};
use crate::rate_limiter::{
    global_limiter_mutex, snapshot_global_audit_log, snapshot_global_policies, CrawlAuditOutcome,
};
use crate::storage::{import_corpus_file, LedgerStore};
use crate::wgpu_graph::{
    build_gpu_upload_payload, CameraUniformsPayload, GpuReadbackMirror, PreparedGpuUpload,
    SimUniformsPayload, SpatialPickGrid, WgpuGraphCallback, WgpuGraphPipelineResources, GRID_DIM,
    SUPER_DIM, WORLD_EXTENT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveDomain {
    Observatory,
    NetworkGraphViewer,
    SearchAndDownload,
    VelocityAndAwakening,
    BridgeCartography,
    CommunitiesLouvainInfomap,
    TopicDriftMiJsd,
    LedgerAndLab,
    RateLimiterAndAudit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelocityPlotMetric {
    DecayVelocity,
    WindowVelocity,
    Acceleration,
    Cumulative,
    SleepingBeautyGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphLayoutMode {
    Orbital,
    Chronological,
    ForceDirected,
}

enum WorkerEvent {
    SearchCompleted {
        merge_into_existing: bool,
        result: Result<SearchHarvestSummary, String>,
    },
    DoiLookupCompleted(Result<PaperRecord, String>),
    DownloadProgress {
        done: usize,
        total: usize,
        report: DownloadItemReport,
    },
    DownloadBatchFinished {
        updated_papers: Vec<PaperRecord>,
        result: Result<Vec<DownloadItemReport>, String>,
    },
    SingleDownloadFinished {
        updated_paper: PaperRecord,
        report: DownloadItemReport,
    },
    ScreeningCompleted {
        updated_papers: Vec<PaperRecord>,
        updated_edges: Vec<CitationEdge>,
        result: Result<ScreeningSummary, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcomeFilter {
    All,
    Success2xx,
    ThrottledWaits,
    BlockedOr429,
    Errors,
}

pub struct CitAtlasApp {
    pub raw_papers: Vec<PaperRecord>,
    pub raw_edges: Vec<CitationEdge>,
    pub corpus_label: String,
    pub params: AnalyticsParams,
    pub analytics: CorpusAnalytics,

    // UI state
    pub domain: ActiveDomain,
    pub disc_filter: Option<Discipline>,
    pub filter_text: String,
    pub focused_paper_id: String,
    pub show_dossier_panel: bool,
    pub status_banner: String,

    // Velocity tab
    pub compared_ids: Vec<String>,
    pub vel_metric: VelocityPlotMetric,
    pub show_awakening_quadrant: bool,

    // Bridge & Community Partition state
    pub partition_scheme: PartitionScheme,
    pub only_show_reclassified: bool,
    pub layout_mode: GraphLayoutMode,
    pub node_positions: HashMap<String, [f32; 2]>,
    pub net_pan: Vec2,
    pub net_zoom: f32,
    pub highlight_conduits: bool,
    pub path_src_id: String,
    pub path_dst_id: String,

    // Topic Drift tab
    pub selected_window_idx: usize,
    pub jsd_delta_mode: bool,
    pub selected_concept_idx: usize,
    pub selected_pair: (Discipline, Discipline),

    // Search & Batch OA Downloader state
    pub search_req: SearchRequest,
    pub merge_search_results: bool,
    pub single_doi_input: String,
    pub download_dir: String,
    pub download_pace_ms: u64,
    pub is_busy: bool,
    pub busy_label: String,
    pub download_progress: Option<(usize, usize)>,
    pub download_reports: Vec<DownloadItemReport>,

    // Ledger, Crawler & CSV Export Suite state
    pub db_path_input: String,
    pub import_path_input: String,
    pub csv_export_dir_input: String,
    pub local_scholar_html_input: String,
    pub rw_csv_path_input: String,
    pub graph_viewer: GraphViewerState,
    pub use_wgpu_pipeline: bool,
    pub wgpu_adapter_label: String,
    pub gpu_scene_revision: u64,
    pub gpu_step_once: bool,
    pub gpu_mirror: Arc<Mutex<GpuReadbackMirror>>,
    pub cpu_pick_grid: SpatialPickGrid,
    pub cached_gpu_upload: Option<PreparedGpuUpload>,
    pub fps_ema: f32,
    pub syn_nodes: usize,
    pub syn_seed: u32,
    pub syn_beta: f64,
    pub syn_regime: SyntheticRegime,
    pub audit_outcome_filter: AuditOutcomeFilter,
    pub audit_search_query: String,
    pub custom_domain_input: String,

    worker_tx: Sender<WorkerEvent>,
    worker_rx: Receiver<WorkerEvent>,
}

impl CitAtlasApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut use_wgpu_pipeline = false;
        let mut wgpu_adapter_label = "CPU Painter Fallback".to_string();
        if let Some(wgpu_state) = cc.wgpu_render_state.as_ref() {
            let info = wgpu_state.adapter.get_info();
            wgpu_adapter_label = format!("wgpu {:?} · {}", info.backend, info.name);
            wgpu_state
                .renderer
                .write()
                .callback_resources
                .insert(WgpuGraphPipelineResources::new(
                    &wgpu_state.device,
                    wgpu_state.target_format,
                ));
            use_wgpu_pipeline = true;
        }
        Self::new_with_ctx(&cc.egui_ctx, use_wgpu_pipeline, wgpu_adapter_label)
    }

    pub fn new_with_ctx(
        ctx: &egui::Context,
        use_wgpu_pipeline: bool,
        wgpu_adapter_label: String,
    ) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(13, 18, 30);
        visuals.window_fill = Color32::from_rgb(17, 24, 39);
        ctx.set_visuals(visuals);

        let (raw_papers, raw_edges) = build_empirical_corpus();
        let params = AnalyticsParams::default();
        let analytics = compute_corpus_analytics(&raw_papers, &raw_edges, &params);
        let selected_window_idx = analytics.windows.len().saturating_sub(1);
        let (worker_tx, worker_rx) = mpsc::channel();

        let mut app = Self {
            raw_papers,
            raw_edges,
            corpus_label: "Empirical Landmark Corpus (47 Verified Works, 1948-2026)".to_string(),
            params,
            analytics,
            domain: ActiveDomain::Observatory,
            disc_filter: None,
            filter_text: String::new(),
            focused_paper_id: "shannon1948".to_string(),
            show_dossier_panel: true,
            status_banner: "Ready — 47 verified landmark papers and 78 citation edges loaded.".to_string(),
            compared_ids: vec![
                "shannon1948".to_string(),
                "albert2002".to_string(),
                "rosvall2008".to_string(),
                "rives2021".to_string(),
                "ke2015".to_string(),
            ],
            vel_metric: VelocityPlotMetric::DecayVelocity,
            show_awakening_quadrant: false,
            partition_scheme: PartitionScheme::FixedDiscipline,
            only_show_reclassified: false,
            layout_mode: GraphLayoutMode::Orbital,
            node_positions: HashMap::new(),
            net_pan: Vec2::ZERO,
            net_zoom: 1.0,
            highlight_conduits: true,
            path_src_id: "lin2023".to_string(),
            path_dst_id: "price1965".to_string(),
            selected_window_idx,
            jsd_delta_mode: false,
            selected_concept_idx: 0,
            selected_pair: (Discipline::ML, Discipline::EVO),
            search_req: SearchRequest::default(),
            merge_search_results: false,
            single_doi_input: "10.1073/pnas.1424329112".to_string(),
            download_dir: "./cit_atlas_downloads".to_string(),
            download_pace_ms: 450,
            is_busy: false,
            busy_label: String::new(),
            download_progress: None,
            download_reports: Vec::new(),
            db_path_input: "cit_atlas.db".to_string(),
            import_path_input: String::new(),
            csv_export_dir_input: "./cit_atlas_csv".to_string(),
            local_scholar_html_input: String::new(),
            rw_csv_path_input: String::new(),
            graph_viewer: GraphViewerState::default(),
            use_wgpu_pipeline,
            wgpu_adapter_label,
            gpu_scene_revision: 1,
            gpu_step_once: false,
            gpu_mirror: Arc::new(Mutex::new(GpuReadbackMirror::default())),
            cpu_pick_grid: SpatialPickGrid::default(),
            cached_gpu_upload: None,
            fps_ema: 60.0,
            syn_nodes: 120,
            syn_seed: 2026,
            syn_beta: 0.18,
            syn_regime: SyntheticRegime::Convergent,
            audit_outcome_filter: AuditOutcomeFilter::All,
            audit_search_query: String::new(),
            custom_domain_input: String::new(),
            worker_tx,
            worker_rx,
        };
        app.recompute_layout();
        app.graph_viewer.sync_with_analytics(&app.analytics, &app.focused_paper_id);
        app.rebuild_cpu_pick_grid();
        app
    }

    fn recompute_all(&mut self) {
        self.analytics = compute_corpus_analytics(&self.raw_papers, &self.raw_edges, &self.params);
        if self.selected_window_idx >= self.analytics.windows.len() {
            self.selected_window_idx = self.analytics.windows.len().saturating_sub(1);
        }
        if !self.analytics.papers.is_empty() {
            let valid_ids: HashSet<&str> = self
                .analytics
                .papers
                .iter()
                .map(|p| p.record.id.as_str())
                .collect();
            if !valid_ids.contains(self.focused_paper_id.as_str()) {
                self.focused_paper_id = self.analytics.papers[0].record.id.clone();
            }
            if !valid_ids.contains(self.path_src_id.as_str()) {
                self.path_src_id = self.analytics.papers.last().unwrap().record.id.clone();
            }
            if !valid_ids.contains(self.path_dst_id.as_str()) {
                self.path_dst_id = self.analytics.papers[0].record.id.clone();
            }
            self.compared_ids.retain(|id| valid_ids.contains(id.as_str()));
            if self.compared_ids.is_empty() {
                for p in self.analytics.papers.iter().take(4) {
                    self.compared_ids.push(p.record.id.clone());
                }
            }
        }
        if self.analytics.papers.len() <= 800 {
            self.recompute_layout();
        }
        self.graph_viewer.sync_with_analytics(&self.analytics, &self.focused_paper_id);
        self.rebuild_cpu_pick_grid();
        self.bump_gpu_revision();
    }

    fn bump_gpu_revision(&mut self) {
        self.gpu_scene_revision = self.gpu_scene_revision.wrapping_add(1);
        self.cached_gpu_upload = None;
    }

    fn rebuild_cpu_pick_grid(&mut self) {
        let coords: Vec<[f32; 2]> = self
            .analytics
            .papers
            .iter()
            .map(|p| {
                self.graph_viewer
                    .positions
                    .get(&p.record.id)
                    .copied()
                    .unwrap_or([0.0, 0.0])
            })
            .collect();
        self.cpu_pick_grid.rebuild(&coords);
    }

    fn sync_gpu_mirror_into_viewer(&mut self) {
        if let Ok(mir) = self.gpu_mirror.lock() {
            if mir.coords.len() >= self.analytics.papers.len() && !self.analytics.papers.is_empty() {
                for (i, p) in self.analytics.papers.iter().enumerate() {
                    self.graph_viewer.positions.insert(p.record.id.clone(), mir.coords[i]);
                }
                for (k, art) in self.graph_viewer.artifact_nodes.iter_mut().enumerate() {
                    let g_idx = self.analytics.papers.len() + k;
                    if let Some(&xy) = mir.coords.get(g_idx) {
                        art.pos = xy;
                    }
                }
            }
        }
    }

    fn recompute_layout(&mut self) {
        let mut pos = HashMap::new();
        let n = self.analytics.papers.len();
        if n == 0 {
            self.node_positions = pos;
            return;
        }

        match self.layout_mode {
            GraphLayoutMode::Orbital => {
                for (d_idx, disc) in Discipline::ALL.iter().enumerate() {
                    let ang_center =
                        (2.0 * std::f32::consts::PI * (d_idx as f32)) / 6.0 - std::f32::consts::FRAC_PI_2;
                    let cx = ang_center.cos() * 0.58;
                    let cy = ang_center.sin() * 0.58;
                    let mut group: Vec<&crate::models::PaperMetrics> = self
                        .analytics
                        .papers
                        .iter()
                        .filter(|p| p.record.disc == *disc)
                        .collect();
                    group.sort_by(|a, b| {
                        b.cb_undir
                            .partial_cmp(&a.cb_undir)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for (k, p) in group.iter().enumerate() {
                        let pull = (p.cb_undir as f32 * 0.72).min(0.30);
                        if k == 0 {
                            pos.insert(p.record.id.clone(), [cx * (1.0 - pull), cy * (1.0 - pull)]);
                        } else {
                            let ring = 0.14 + 0.055 * (((k - 1) / 6) as f32);
                            let sub_ang = ang_center + ((k as f32) * 2.399_963);
                            pos.insert(
                                p.record.id.clone(),
                                [
                                    (cx + sub_ang.cos() * ring) * (1.0 - pull),
                                    (cy + sub_ang.sin() * ring) * (1.0 - pull),
                                ],
                            );
                        }
                    }
                }
            }
            GraphLayoutMode::Chronological => {
                let span = (self.analytics.max_year - self.analytics.min_year).max(1) as f32;
                for (d_idx, disc) in Discipline::ALL.iter().enumerate() {
                    let lane_y = -0.74 + 1.48 * ((d_idx as f32) / 5.0);
                    let mut group: Vec<&crate::models::PaperMetrics> = self
                        .analytics
                        .papers
                        .iter()
                        .filter(|p| p.record.disc == *disc)
                        .collect();
                    group.sort_by_key(|p| p.record.year);
                    for (k, p) in group.iter().enumerate() {
                        let xn = -0.84
                            + 1.68 * (((p.record.year - self.analytics.min_year) as f32) / span);
                        let jy = (((k % 3) as f32) - 1.0) * 0.05;
                        pos.insert(p.record.id.clone(), [xn, lane_y + jy]);
                    }
                }
            }
            GraphLayoutMode::ForceDirected => {
                let mut xs = vec![0.0f32; n];
                let mut ys = vec![0.0f32; n];
                for (i, p) in self.analytics.papers.iter().enumerate() {
                    let d_idx = p.record.disc.index() as f32;
                    let ang = (2.0 * std::f32::consts::PI * d_idx) / 6.0 + (i as f32) * 0.41;
                    xs[i] = ang.cos() * 0.45;
                    ys[i] = ang.sin() * 0.45;
                }
                let k_ideal = (2.4 / (n as f32)).sqrt();
                for iter in 0..85 {
                    let mut dx = vec![0.0f32; n];
                    let mut dy = vec![0.0f32; n];
                    let temp = 0.085 * (1.0 - (iter as f32) / 90.0);
                    for i in 0..n {
                        for j in (i + 1)..n {
                            let ux = xs[i] - xs[j];
                            let uy = ys[i] - ys[j];
                            let d2 = ux * ux + uy * uy + 0.001;
                            let d = d2.sqrt();
                            let rep = (k_ideal * k_ideal * 0.40) / d2;
                            let fx = (ux / d) * rep;
                            let fy = (uy / d) * rep;
                            dx[i] += fx;
                            dy[i] += fy;
                            dx[j] -= fx;
                            dy[j] -= fy;
                        }
                    }
                    for &(u, v) in &self.analytics.active_edges {
                        let ux = xs[u] - xs[v];
                        let uy = ys[u] - ys[v];
                        let d = (ux * ux + uy * uy + 0.0005).sqrt();
                        let attr = ((d * d) / k_ideal.max(0.05)) * 0.52;
                        let fx = (ux / d) * attr;
                        let fy = (uy / d) * attr;
                        dx[u] -= fx;
                        dy[u] -= fy;
                        dx[v] += fx;
                        dy[v] += fy;
                    }
                    for i in 0..n {
                        let d_idx = self.analytics.papers[i].record.disc.index() as f32;
                        let ang =
                            (2.0 * std::f32::consts::PI * d_idx) / 6.0 - std::f32::consts::FRAC_PI_2;
                        dx[i] += (ang.cos() * 0.35 - xs[i]) * 0.08;
                        dy[i] += (ang.sin() * 0.35 - ys[i]) * 0.08;
                        let mag = (dx[i] * dx[i] + dy[i] * dy[i]).sqrt().max(1e-5);
                        let step = mag.min(temp);
                        xs[i] = (xs[i] + (dx[i] / mag) * step).clamp(-0.88, 0.88);
                        ys[i] = (ys[i] + (dy[i] / mag) * step).clamp(-0.88, 0.88);
                    }
                }
                for (i, p) in self.analytics.papers.iter().enumerate() {
                    pos.insert(p.record.id.clone(), [xs[i], ys[i]]);
                }
            }
        }
        self.node_positions = pos;
    }

    fn poll_worker_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.worker_rx.try_recv() {
            match ev {
                WorkerEvent::SearchCompleted {
                    merge_into_existing,
                    result,
                } => {
                    self.is_busy = false;
                    self.busy_label.clear();
                    match result {
                        Ok(summary) => {
                            let counts_str = summary
                                .provider_counts
                                .iter()
                                .map(|(p, c)| format!("{}:{}", p, c))
                                .collect::<Vec<_>>()
                                .join(", ");
                            if summary.papers.is_empty() {
                                self.status_banner = format!(
                                    "Search finished with 0 matching works ({}). {}",
                                    counts_str,
                                    summary.warnings.join(" | ")
                                );
                            } else {
                                if merge_into_existing {
                                    let mut combined = self.raw_papers.clone();
                                    combined.extend(summary.papers);
                                    let (merged_p, mut merged_e) = merge_and_link_records(
                                        combined,
                                        false,
                                        None,
                                        None,
                                    );
                                    for old_e in &self.raw_edges {
                                        if !merged_e.iter().any(|e| {
                                            e.source == old_e.source && e.target == old_e.target
                                        }) {
                                            merged_e.push(old_e.clone());
                                        }
                                    }
                                    for new_e in summary.edges {
                                        if !merged_e.iter().any(|e| {
                                            e.source == new_e.source && e.target == new_e.target
                                        }) {
                                            merged_e.push(new_e);
                                        }
                                    }
                                    self.raw_papers = merged_p;
                                    self.raw_edges = merged_e;
                                    self.corpus_label = format!(
                                        "Merged Workspace ({} Papers, {} Edges)",
                                        self.raw_papers.len(),
                                        self.raw_edges.len()
                                    );
                                } else {
                                    self.raw_papers = summary.papers;
                                    self.raw_edges = summary.edges;
                                    self.corpus_label = format!(
                                        "Live Harvested Subgraph ({} Papers, {} Edges)",
                                        self.raw_papers.len(),
                                        self.raw_edges.len()
                                    );
                                }
                                self.recompute_all();
                                self.status_banner = format!(
                                    "Harvested {} deduplicated papers and {} citation edges [{}]. {}",
                                    self.raw_papers.len(),
                                    self.raw_edges.len(),
                                    counts_str,
                                    summary.warnings.join(" | ")
                                );
                            }
                        }
                        Err(e) => {
                            self.status_banner = format!("Search failed: {}", e);
                        }
                    }
                }
                WorkerEvent::DoiLookupCompleted(res) => {
                    self.is_busy = false;
                    self.busy_label.clear();
                    match res {
                        Ok(rec) => {
                            let new_id = rec.id.clone();
                            let mut combined = self.raw_papers.clone();
                            combined.push(rec);
                            let (merged_p, mut merged_e) =
                                merge_and_link_records(combined, false, None, None);
                            for old_e in &self.raw_edges {
                                if !merged_e
                                    .iter()
                                    .any(|e| e.source == old_e.source && e.target == old_e.target)
                                {
                                    merged_e.push(old_e.clone());
                                }
                            }
                            self.raw_papers = merged_p;
                            self.raw_edges = merged_e;
                            self.focused_paper_id = new_id;
                            self.recompute_all();
                            self.status_banner = "Resolved DOI and linked references into active graph.".to_string();
                        }
                        Err(e) => {
                            self.status_banner = format!("DOI lookup failed: {}", e);
                        }
                    }
                }
                WorkerEvent::DownloadProgress {
                    done,
                    total,
                    report,
                } => {
                    self.download_progress = Some((done, total));
                    self.busy_label = format!(
                        "Downloading OA PDFs ({}/{}) — {} [{}]",
                        done,
                        total,
                        report.paper_id,
                        report.status.label()
                    );
                    self.download_reports.push(report);
                }
                WorkerEvent::DownloadBatchFinished {
                    updated_papers,
                    result,
                } => {
                    self.is_busy = false;
                    self.busy_label.clear();
                    self.download_progress = None;
                    self.raw_papers = updated_papers;
                    self.recompute_all();
                    match result {
                        Ok(reps) => {
                            let ok_cnt = reps.iter().filter(|r| r.status.is_ok()).count();
                            self.download_reports = reps;
                            self.status_banner = format!(
                                "Batch download complete: {}/{} verified %PDF- files saved to {} (+ BibTeX & SHA-256 manifest).",
                                ok_cnt,
                                self.download_reports.len(),
                                self.download_dir
                            );
                        }
                        Err(e) => {
                            self.status_banner = format!("Batch download error: {}", e);
                        }
                    }
                }
                WorkerEvent::SingleDownloadFinished {
                    updated_paper,
                    report,
                } => {
                    self.is_busy = false;
                    self.busy_label.clear();
                    if let Some(slot) = self
                        .raw_papers
                        .iter_mut()
                        .find(|p| p.id == updated_paper.id)
                    {
                        *slot = updated_paper;
                    }
                    self.status_banner = format!(
                        "Single PDF fetch for {}: {} ({:.1} KB)",
                        report.paper_id,
                        report.status.label(),
                        (report.byte_len as f64) / 1024.0
                    );
                    self.download_reports.insert(0, report);
                    self.recompute_all();
                }
                WorkerEvent::ScreeningCompleted {
                    updated_papers,
                    updated_edges,
                    result,
                } => {
                    self.is_busy = false;
                    self.busy_label.clear();
                    self.raw_papers = updated_papers;
                    self.raw_edges = updated_edges;
                    self.recompute_all();
                    match result {
                        Ok(s) => {
                            self.status_banner = format!(
                                "Screening complete: {} papers | {} retracted | {} DataCite DOIs | {} UniProt · {} PDB · {} Europe PMC terms ({} bio-bridge edges).",
                                s.papers_screened,
                                s.retractions_flagged,
                                s.datacite_artifacts_linked,
                                s.uniprot_proteins_linked,
                                s.pdb_structures_linked,
                                s.epmc_annotations_linked,
                                s.shared_bio_bridge_edges
                            );
                        }
                        Err(e) => {
                            self.status_banner = format!("Screening failed: {}", e);
                        }
                    }
                }
            }
        }
        if self.is_busy {
            ctx.request_repaint();
        }
    }

    fn matches_paper_filter(&self, p: &crate::models::PaperMetrics) -> bool {
        if let Some(df) = self.disc_filter {
            if p.record.disc != df {
                return false;
            }
        }
        let q = self.filter_text.trim().to_ascii_lowercase();
        if q.is_empty() {
            return true;
        }
        p.record.title.to_ascii_lowercase().contains(&q)
            || p.record.authors.to_ascii_lowercase().contains(&q)
            || p.record.venue.to_ascii_lowercase().contains(&q)
            || p.record.doi.to_ascii_lowercase().contains(&q)
            || p.record
                .terms
                .iter()
                .any(|t| t.to_ascii_lowercase().contains(&q))
            || p.record.bio_entities.iter().any(|b| {
                b.entity_id.to_ascii_lowercase().contains(&q)
                    || b.label.to_ascii_lowercase().contains(&q)
            })
    }

    fn start_background_search(&mut self) {
        if self.is_busy {
            return;
        }
        self.is_busy = true;
        self.busy_label = format!("Harvesting scholarly APIs & crawlers for '{}'...", self.search_req.query);
        let mut req_clone = self.search_req.clone();
        req_clone.crawl.local_html_paths = self
            .local_scholar_html_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        req_clone.crawl.retraction_watch_csv_path = self.rw_csv_path_input.trim().to_string();
        let merge_flag = self.merge_search_results;
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            let result = execute_multi_provider_search(&req_clone);
            let _ = tx.send(WorkerEvent::SearchCompleted {
                merge_into_existing: merge_flag,
                result,
            });
        });
    }

    fn start_background_screening(&mut self) {
        if self.is_busy || self.raw_papers.is_empty() {
            return;
        }
        self.is_busy = true;
        self.busy_label = format!(
            "Screening {} papers via Retraction Watch, DataCite, Europe PMC Annotations & UniProt/PDB...",
            self.raw_papers.len()
        );
        let mut papers_copy = self.raw_papers.clone();
        let mut edges_copy = self.raw_edges.clone();
        let email = self.search_req.polite_email.clone();
        let proxy = self.search_req.crawl.proxy_url.clone();
        let rw_csv_str = self.rw_csv_path_input.trim().to_string();
        let do_ret = self.search_req.crawl.screen_retractions;
        let do_dc = self.search_req.crawl.track_datacite_bridges;
        let do_epmc = self.search_req.crawl.track_epmc_annotations;
        let do_up_pdb = self.search_req.crawl.track_uniprot_pdb_bridges;
        let tx = self.worker_tx.clone();

        thread::spawn(move || {
            let rw_opt = if rw_csv_str.is_empty() {
                None
            } else {
                Some(std::path::Path::new(&rw_csv_str))
            };
            let res = run_full_corpus_screening(
                &mut papers_copy,
                &mut edges_copy,
                &email,
                &proxy,
                rw_opt,
                do_ret,
                do_dc,
                do_epmc,
                do_up_pdb,
                true,
            );
            let _ = tx.send(WorkerEvent::ScreeningCompleted {
                updated_papers: papers_copy,
                updated_edges: edges_copy,
                result: res,
            });
        });
    }

    fn start_background_batch_download(&mut self) {
        if self.is_busy || self.raw_papers.is_empty() {
            return;
        }
        self.is_busy = true;
        self.download_reports.clear();
        self.download_progress = Some((0, self.raw_papers.len()));
        self.busy_label = "Starting verified OA PDF batch download...".to_string();

        let mut papers_copy = self.raw_papers.clone();
        let out_dir = PathBuf::from(self.download_dir.trim());
        let email = self.search_req.polite_email.clone();
        let pace = self.download_pace_ms;
        let tx = self.worker_tx.clone();

        thread::spawn(move || {
            let (prog_tx, prog_rx) = mpsc::channel::<(usize, usize, DownloadItemReport)>();
            let tx_fwd = tx.clone();
            let forwarder = thread::spawn(move || {
                while let Ok((done, total, report)) = prog_rx.recv() {
                    let _ = tx_fwd.send(WorkerEvent::DownloadProgress {
                        done,
                        total,
                        report,
                    });
                }
            });

            let res = batch_download_corpus_pdfs(
                &mut papers_copy,
                &out_dir,
                &email,
                pace,
                Some(&prog_tx),
            );
            drop(prog_tx);
            let _ = forwarder.join();
            let _ = tx.send(WorkerEvent::DownloadBatchFinished {
                updated_papers: papers_copy,
                result: res,
            });
        });
    }

    fn start_single_paper_download(&mut self, paper_id: &str) {
        if self.is_busy {
            return;
        }
        let Some(mut paper) = self
            .raw_papers
            .iter()
            .find(|p| p.id == paper_id)
            .cloned()
        else {
            return;
        };
        self.is_busy = true;
        self.busy_label = format!("Resolving legal OA PDF for {}...", paper.short_cite());
        let out_dir = PathBuf::from(self.download_dir.trim()).join("pdfs");
        let email = self.search_req.polite_email.clone();
        let tx = self.worker_tx.clone();

        thread::spawn(move || {
            match ScholarlyHttpClient::new(&email) {
                Ok(http) => {
                    let report = download_single_paper_pdf(&http, &mut paper, &out_dir);
                    let _ = tx.send(WorkerEvent::SingleDownloadFinished {
                        updated_paper: paper,
                        report,
                    });
                }
                Err(e) => {
                    let rep = DownloadItemReport {
                        paper_id: paper.id.clone(),
                        title: paper.title.clone(),
                        doi: paper.doi.clone(),
                        status: crate::models::DownloadStatus::FailedNetwork(e),
                        resolved_stage: "init".to_string(),
                        final_pdf_url: String::new(),
                        saved_path: None,
                        byte_len: 0,
                        sha256: None,
                    };
                    let _ = tx.send(WorkerEvent::SingleDownloadFinished {
                        updated_paper: paper,
                        report: rep,
                    });
                }
            }
        });
    }
}

impl eframe::App for CitAtlasApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_ui(ctx);
    }
}

impl CitAtlasApp {
    pub fn configure_screenshot_state(&mut self) {
        self.domain = ActiveDomain::NetworkGraphViewer;
        self.graph_viewer.layout = ViewerLayoutPreset::ForceAtlas2;
        self.graph_viewer.color_by = NodeColorEncoding::LouvainCommunity;
        self.graph_viewer.show_labels = true;
        self.graph_viewer.physics_running = false;
        self.show_dossier_panel = true;
        self.focused_paper_id = "shannon1948".to_string();
        self.recompute_layout();
        self.graph_viewer
            .sync_with_analytics(&self.analytics, &self.focused_paper_id);
        for _ in 0..12 {
            self.graph_viewer.step_physics(&self.analytics);
        }
        self.rebuild_cpu_pick_grid();
        self.gpu_scene_revision = self.gpu_scene_revision.wrapping_add(1);
        self.gpu_step_once = true;
    }

    pub fn configure_screenshot_view(&mut self, view: u8) {
        match view {
            2 => {
                self.domain = ActiveDomain::CommunitiesLouvainInfomap;
                self.focused_paper_id = "rosvall2008".to_string();
                self.show_dossier_panel = true;
            }
            3 => {
                self.domain = ActiveDomain::VelocityAndAwakening;
                self.vel_metric = VelocityPlotMetric::DecayVelocity;
                self.show_awakening_quadrant = false;
                self.focused_paper_id = "ke2015".to_string();
                self.show_dossier_panel = true;
            }
            _ => self.configure_screenshot_state(),
        }
    }

    pub fn update_ui(&mut self, ctx: &egui::Context) {
        self.poll_worker_events(ctx);

        // Top Navigation & Global Parameter Ribbon
        egui::TopBottomPanel::top("cit_atlas_top_ribbon").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("CitAtlas")
                        .size(16.5)
                        .strong()
                        .color(Color32::from_rgb(56, 189, 248)),
                );
                ui.label(RichText::new(&self.corpus_label).size(12.0).weak());
                ui.separator();

                let tabs = [
                    (ActiveDomain::Observatory, "1. Observatory"),
                    (ActiveDomain::NetworkGraphViewer, "2. Network Viewer"),
                    (ActiveDomain::SearchAndDownload, "3. Search & Crawl"),
                    (ActiveDomain::VelocityAndAwakening, "4. Velocity"),
                    (ActiveDomain::BridgeCartography, "5. Bridges"),
                    (ActiveDomain::CommunitiesLouvainInfomap, "6. Louvain & Infomap"),
                    (ActiveDomain::TopicDriftMiJsd, "7. MI & JSD Drift"),
                    (ActiveDomain::LedgerAndLab, "8. SQLite, CSV & Graph"),
                    (ActiveDomain::RateLimiterAndAudit, "9. Rate Limiter & Audit"),
                ];
                for (dom, label) in tabs {
                    if ui.selectable_label(self.domain == dom, label).clicked() {
                        self.domain = dom;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(self.show_dossier_panel, "Paper Dossier")
                        .clicked()
                    {
                        self.show_dossier_panel = !self.show_dossier_panel;
                    }
                });
            });

            ui.separator();

            // Global Sliders & Discipline Filter Pills
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                ui.label("W (yr):");
                if ui
                    .add(egui::Slider::new(&mut self.params.window_width, 3..=25))
                    .changed()
                {
                    changed = true;
                }
                ui.label("Stride S:");
                if ui
                    .add(egui::Slider::new(&mut self.params.window_stride, 1..=10))
                    .changed()
                {
                    changed = true;
                }
                ui.label("Half-life tau:");
                if ui
                    .add(egui::Slider::new(&mut self.params.half_life, 2.0..=20.0))
                    .changed()
                {
                    changed = true;
                }
                ui.label("Horizon:");
                if ui
                    .add(egui::Slider::new(&mut self.params.horizon_year, 1965..=2026))
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(&mut self.params.exclude_retracted, "Hide Retracted")
                    .changed()
                {
                    changed = true;
                }
                ui.separator();
                ui.label("Filter:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter_text)
                        .desired_width(145.0)
                        .hint_text("Author, title, DOI..."),
                );
                if ui
                    .selectable_label(self.disc_filter.is_none(), "ALL")
                    .clicked()
                {
                    self.disc_filter = None;
                }
                for d in Discipline::ALL {
                    let [r, g, b] = d.rgb();
                    if ui
                        .selectable_label(
                            self.disc_filter == Some(d),
                            RichText::new(d.code()).color(Color32::from_rgb(r, g, b)),
                        )
                        .clicked()
                    {
                        self.disc_filter = Some(d);
                    }
                }
            });
            if changed {
                self.recompute_all();
            }
            ui.add_space(2.0);
        });

        // Bottom Status Bar
        egui::TopBottomPanel::bottom("cit_atlas_status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.is_busy {
                    ui.spinner();
                    ui.label(
                        RichText::new(&self.busy_label)
                            .color(Color32::from_rgb(250, 204, 21)),
                    );
                } else {
                    ui.label(RichText::new(&self.status_banner).size(11.5));
                }
            });
        });

        // Right-hand Focal Paper Dossier Drawer
        if self.show_dossier_panel {
            egui::SidePanel::right("cit_atlas_dossier")
                .resizable(true)
                .default_width(335.0)
                .show(ctx, |ui| {
                    self.ui_dossier_panel(ui);
                });
        }

        // Main Central Panel
        let dt = ctx.input(|i| i.unstable_dt).clamp(0.001, 0.25);
        self.fps_ema = self.fps_ema * 0.90 + (1.0 / dt) * 0.10;

        if self.domain == ActiveDomain::NetworkGraphViewer && self.graph_viewer.physics_running {
            if !self.use_wgpu_pipeline {
                self.graph_viewer.step_physics(&self.analytics);
                self.rebuild_cpu_pick_grid();
            }
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.domain {
            ActiveDomain::Observatory => self.ui_observatory(ui),
            ActiveDomain::NetworkGraphViewer => self.ui_network_graph_viewer(ui),
            ActiveDomain::SearchAndDownload => self.ui_search_and_download(ui),
            ActiveDomain::VelocityAndAwakening => self.ui_velocity_and_awakening(ui),
            ActiveDomain::BridgeCartography => self.ui_bridge_cartography(ui),
            ActiveDomain::CommunitiesLouvainInfomap => self.ui_communities(ui),
            ActiveDomain::TopicDriftMiJsd => self.ui_topic_drift(ui),
            ActiveDomain::LedgerAndLab => self.ui_ledger_and_lab(ui),
            ActiveDomain::RateLimiterAndAudit => self.ui_rate_limiter_and_audit(ui),
        });
    }
}

impl CitAtlasApp {
    fn ui_dossier_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Focal Paper Dossier");
        ui.separator();

        let Some(p) = self
            .analytics
            .papers
            .iter()
            .find(|x| x.record.id == self.focused_paper_id)
            .cloned()
        else {
            ui.label("Select any paper node or row to inspect.");
            return;
        };

        let [r, g, b] = p.record.disc.rgb();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{} · {}", p.record.disc.code(), p.record.disc.short_name()))
                    .strong()
                    .color(Color32::from_rgb(r, g, b)),
            );
            ui.label(RichText::new(format!("({})", p.record.year)).monospace());
        });

        ui.add_space(4.0);
        ui.label(RichText::new(&p.record.title).strong().size(14.0));
        ui.label(&p.record.authors);
        ui.label(RichText::new(&p.record.venue).italics().weak());
        if !p.record.doi.is_empty() {
            ui.label(RichText::new(format!("DOI: {}", p.record.doi)).monospace().size(11.0));
        }

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if !p.record.source_url.is_empty() {
                ui.hyperlink_to("Primary Source", &p.record.source_url);
            }
            if ui
                .add_enabled(!self.is_busy, egui::Button::new("Download OA PDF"))
                .clicked()
            {
                self.start_single_paper_download(&p.record.id);
            }
            if ui.button("Toggle Velocity Curve").clicked() {
                if let Some(pos) = self.compared_ids.iter().position(|x| x == &p.record.id) {
                    if self.compared_ids.len() > 1 {
                        self.compared_ids.remove(pos);
                    }
                } else {
                    if self.compared_ids.len() >= 5 {
                        self.compared_ids.remove(0);
                    }
                    self.compared_ids.push(p.record.id.clone());
                }
            }
            if ui.button("Focus in Network Viewer").clicked() {
                self.domain = ActiveDomain::NetworkGraphViewer;
                if self.graph_viewer.layout == ViewerLayoutPreset::RadialEgo {
                    self.graph_viewer.recompute_preset_layout(&self.analytics, &self.focused_paper_id);
                }
            }
            if ui.button("Set Geodesic Origin").clicked() {
                self.path_src_id = p.record.id.clone();
                self.domain = ActiveDomain::BridgeCartography;
            }
        });

        if let Some(ref local_path) = p.record.local_pdf_path {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("Verified Local PDF: {}", local_path))
                    .color(Color32::from_rgb(52, 211, 153))
                    .size(11.0),
            );
        }

        if p.record.retraction.status != RetractionStatus::Unscreened {
            ui.add_space(4.0);
            let (badge_col, badge_txt) = match p.record.retraction.status {
                RetractionStatus::Retracted | RetractionStatus::Withdrawn => (
                    Color32::from_rgb(248, 113, 113),
                    format!(
                        "INTEGRITY ALERT: {} ({} · {}) — {} post-retraction cites",
                        p.record.retraction.status.as_str().to_uppercase(),
                        if p.record.retraction.source.is_empty() {
                            "crossref"
                        } else {
                            &p.record.retraction.source
                        },
                        if p.record.retraction.notice_date.is_empty() {
                            "date n/a"
                        } else {
                            &p.record.retraction.notice_date
                        },
                        p.post_retraction_cites
                    ),
                ),
                RetractionStatus::ExpressionOfConcern | RetractionStatus::Correction => (
                    Color32::from_rgb(250, 204, 21),
                    format!(
                        "Notice: {} ({})",
                        p.record.retraction.status.as_str(),
                        p.record.retraction.notice_date
                    ),
                ),
                _ => (
                    Color32::from_rgb(52, 211, 153),
                    "Integrity Screened: Clean (No Retraction Found)".to_string(),
                ),
            };
            ui.label(RichText::new(badge_txt).color(badge_col).strong().size(11.5));
            if !p.record.retraction.reasons.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "Reasons: {}",
                        p.record.retraction.reasons.join("; ")
                    ))
                    .size(11.0)
                    .weak(),
                );
            }
        }

        ui.separator();
        egui::Grid::new("dossier_metrics_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                ui.label("In-Corpus Citations:");
                ui.monospace(format!("{}", p.in_deg));
                ui.end_row();

                if let Some(ext) = p.record.external_citations {
                    ui.label("External Citations:");
                    ui.monospace(format!("{}", ext));
                    ui.end_row();
                }

                ui.label("Decay Velocity v_exp:");
                ui.monospace(format!("{:.3}", p.v_exp));
                ui.end_row();

                ui.label("Window Velocity v_W:");
                ui.monospace(format!("{:.3} /yr", p.v_win));
                ui.end_row();

                ui.label("Acceleration dv/dt:");
                ui.monospace(format!("{:+.3}", p.accel));
                ui.end_row();

                ui.label("Ke Sleeping Beauty B:");
                ui.monospace(format!("{:.2} (t_a={})", p.beauty_b, p.awaken_year));
                ui.end_row();

                ui.label("Disruption Index CD:");
                ui.monospace(format!("{:+.3}", p.disruption_cd));
                ui.end_row();

                ui.label("Brandes Geodesic CB_u:");
                ui.monospace(format!("{:.2}%", p.cb_undir * 100.0));
                ui.end_row();

                ui.label("Brandes Lineage CB_d:");
                ui.monospace(format!("{:.2}%", p.cb_dir * 100.0));
                ui.end_row();

                ui.label("Participation P_i:");
                ui.monospace(format!("{:.3} ({})", p.part_p, p.role_code));
                ui.end_row();

                ui.label("Burt Constraint C_i:");
                ui.monospace(format!("{:.3}", p.burt_c));
                ui.end_row();

                ui.label("Louvain Community:");
                ui.monospace(format!(
                    "L{} (P={:.2}{})",
                    p.louvain_comm + 1,
                    p.louvain_part_p,
                    if p.reclassified_louvain { " · Bridge-Shifted" } else { "" }
                ));
                ui.end_row();

                ui.label("Infomap Flow Module:");
                ui.monospace(format!(
                    "M{} ({:.2}% flow{})",
                    p.infomap_comm + 1,
                    p.infomap_flow * 100.0,
                    if p.reclassified_infomap { " · Bridge-Shifted" } else { "" }
                ));
                ui.end_row();

                ui.label("DataCite Bridges:");
                ui.monospace(format!(
                    "{} datasets · {} software ({} shared peers)",
                    p.dataset_bridge_count, p.software_bridge_count, p.shared_artifact_peers
                ));
                ui.end_row();

                ui.label("Bio-Entity Bridges:");
                ui.monospace(format!(
                    "{} UniProt · {} PDB · {} EPMC ({} peers)",
                    p.uniprot_bridge_count,
                    p.pdb_bridge_count,
                    p.epmc_annotation_count,
                    p.shared_bio_entity_peers
                ));
                ui.end_row();

                if p.cites_retracted_count > 0 {
                    ui.label("Cites Retracted Works:");
                    ui.colored_label(
                        Color32::from_rgb(248, 113, 113),
                        format!("{}", p.cites_retracted_count),
                    );
                    ui.end_row();
                }
            });

        ui.separator();
        ui.push_id("dossier_scroll", |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if !p.record.datacite_links.is_empty() {
                    ui.label(RichText::new("Linked DataCite Datasets & Software").strong());
                    for lk in &p.record.datacite_links {
                        ui.horizontal_wrapped(|ui| {
                            ui.monospace(format!("[{} · {}]", lk.artifact_kind.as_str(), lk.relation_type));
                            ui.hyperlink_to(&lk.artifact_doi, format!("https://doi.org/{}", lk.artifact_doi));
                            ui.weak(format!("({} dl)", lk.download_count));
                        });
                    }
                    ui.separator();
                }
                if !p.record.bio_entities.is_empty() {
                    ui.label(RichText::new("Europe PMC Annotations & UniProt / PDB Bridges").strong());
                    for b in &p.record.bio_entities {
                        let [cr, cg, cb] = b.kind.rgb();
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                Color32::from_rgb(cr, cg, cb),
                                format!("[{}]", b.kind.short_badge()),
                            );
                            if !b.uri.is_empty() {
                                ui.hyperlink_to(&b.entity_id, &b.uri);
                            } else {
                                ui.monospace(&b.entity_id);
                            }
                            ui.label(RichText::new(&b.label).size(11.0));
                            if let Some(res) = b.resolution_angstrom {
                                ui.weak(format!("({:.2} A)", res));
                            } else if b.mention_count > 1 {
                                ui.weak(format!("({}x)", b.mention_count));
                            }
                        });
                    }
                    ui.separator();
                }
                if !p.record.abstract_text.is_empty() {
                    ui.label(RichText::new("Abstract").strong());
                    ui.label(RichText::new(&p.record.abstract_text).size(11.5));
                    ui.separator();
                }
                ui.label(RichText::new("Incoming Citers in Graph").strong());
                let citers = &self.analytics.in_adj[p.idx];
                if citers.is_empty() {
                    ui.weak("None within active horizon.");
                } else {
                    for &c_idx in citers {
                        let cp = &self.analytics.papers[c_idx];
                        if ui
                            .small_button(format!(
                                "[{} {}] {}",
                                cp.record.disc.code(),
                                cp.record.year,
                                cp.record.title
                            ))
                            .clicked()
                        {
                            self.focused_paper_id = cp.record.id.clone();
                        }
                    }
                }

                ui.add_space(6.0);
                ui.label(RichText::new("Outgoing In-Corpus References").strong());
                let refs = &self.analytics.out_adj[p.idx];
                if refs.is_empty() {
                    ui.weak("Root node in active corpus.");
                } else {
                    for &r_idx in refs {
                        let rp = &self.analytics.papers[r_idx];
                        if ui
                            .small_button(format!(
                                "[{} {}] {}",
                                rp.record.disc.code(),
                                rp.record.year,
                                rp.record.title
                            ))
                            .clicked()
                        {
                            self.focused_paper_id = rp.record.id.clone();
                        }
                    }
                }
            });
        });
    }

    // ========================================================================
    // DOMAIN 1: EPISTEMIC OBSERVATORY
    // ========================================================================
    fn ui_observatory(&mut self, ui: &mut egui::Ui) {
        let n = self.analytics.papers.len();
        let e = self.analytics.active_edges.len();
        let cross_pct = if e > 0 {
            (self.analytics.cross_edge_count as f64 / e as f64) * 100.0
        } else {
            0.0
        };

        // Top KPI Row
        ui.columns(4, |cols| {
            cols[0].group(|ui| {
                ui.weak("Active Citation Graph");
                ui.label(RichText::new(format!("{} V / {} E", n, e)).strong().size(16.0));
                ui.label(format!("{:.1}% cross-discipline citations", cross_pct));
            });

            if let Some(top_v) = self
                .analytics
                .papers
                .iter()
                .max_by(|a, b| a.v_exp.partial_cmp(&b.v_exp).unwrap_or(std::cmp::Ordering::Equal))
            {
                cols[1].group(|ui| {
                    ui.weak("Peak Decay Velocity v_exp");
                    ui.label(
                        RichText::new(format!("{:.2}", top_v.v_exp))
                            .strong()
                            .size(16.0)
                            .color(Color32::from_rgb(56, 189, 248)),
                    );
                    ui.label(top_v.record.short_cite());
                });
            }

            if let Some(top_b) = self.analytics.papers.iter().max_by(|a, b| {
                a.cb_undir
                    .partial_cmp(&b.cb_undir)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                cols[2].group(|ui| {
                    ui.weak("Top Geodesic Bridge CB_u");
                    ui.label(
                        RichText::new(format!("{:.1}%", top_b.cb_undir * 100.0))
                            .strong()
                            .size(16.0)
                            .color(Color32::from_rgb(167, 139, 250)),
                    );
                    ui.label(format!("{} (P={:.2})", top_b.record.short_cite(), top_b.part_p));
                });
            }

            if let Some(last_w) = self.analytics.windows.last() {
                cols[3].group(|ui| {
                    ui.weak("Latest Window NMI(D; T)");
                    ui.label(
                        RichText::new(format!("{:.3} NMI", last_w.nmi))
                            .strong()
                            .size(16.0)
                            .color(Color32::from_rgb(52, 211, 153)),
                    );
                    ui.label(format!("I(D;T) = {:.3} bits ({})", last_w.mi_bits, last_w.label));
                });
            }
        });

        ui.add_space(6.0);

        // Dual Charts: Left = Window Output + NMI; Right = Velocity vs Bridge Scatter
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.label(RichText::new("Multi-Window Corpus Activity & Epistemic NMI(D; T)").strong());
                self.paint_overview_windows(ui);
            });
            cols[1].group(|ui| {
                ui.label(
                    RichText::new("Velocity v_exp vs. Brandes Betweenness CB_u (Click Node)")
                        .strong(),
                );
                self.paint_overview_scatter(ui);
            });
        });

        ui.add_space(6.0);
        ui.label(RichText::new("Scientometric Corpus Leaderboard").strong());
        let mut filtered: Vec<crate::models::PaperMetrics> = self
            .analytics
            .papers
            .iter()
            .filter(|p| self.matches_paper_filter(p))
            .cloned()
            .collect();
        filtered.sort_by(|a, b| {
            b.cb_undir
                .partial_cmp(&a.cb_undir)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        ui.push_id("observatory_table_scroll", |ui| {
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("observatory_grid")
                    .striped(true)
                    .num_columns(10)
                    .show(ui, |ui| {
                        ui.strong("Paper");
                        ui.strong("Disc");
                        ui.strong("Year");
                        ui.strong("In-Cites");
                        ui.strong("v_exp");
                        ui.strong("dv/dt");
                        ui.strong("Beauty B");
                        ui.strong("CD");
                        ui.strong("CB_u");
                        ui.strong("Part P_i");
                        ui.end_row();

                        for p in filtered.iter().take(250) {
                            let [r, g, b] = p.record.disc.rgb();
                            if ui
                                .selectable_label(
                                    self.focused_paper_id == p.record.id,
                                    &p.record.title,
                                )
                                .clicked()
                            {
                                self.focused_paper_id = p.record.id.clone();
                                self.show_dossier_panel = true;
                            }
                            ui.colored_label(Color32::from_rgb(r, g, b), p.record.disc.code());
                            ui.monospace(format!("{}", p.record.year));
                            ui.monospace(format!("{}", p.in_deg));
                            ui.monospace(format!("{:.2}", p.v_exp));
                            ui.monospace(format!("{:+.2}", p.accel));
                            ui.monospace(format!("{:.1}", p.beauty_b));
                            ui.monospace(format!("{:+.2}", p.disruption_cd));
                            ui.monospace(format!("{:.1}%", p.cb_undir * 100.0));
                            ui.monospace(format!("{:.2}", p.part_p));
                            ui.end_row();
                        }
                    });
            });
        });
    }

    fn paint_overview_windows(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 210.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::click());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));

        let wins = &self.analytics.windows;
        if wins.is_empty() {
            return;
        }
        let pad_l = 36.0;
        let pad_r = 36.0;
        let pad_t = 14.0;
        let pad_b = 26.0;
        let pw = (rect.width() - pad_l - pad_r).max(40.0);
        let ph = (rect.height() - pad_t - pad_b).max(40.0);

        let max_stack = wins
            .iter()
            .map(|w| w.pub_counts_by_disc.iter().sum::<usize>())
            .max()
            .unwrap_or(1)
            .max(1) as f32;

        let slot_w = pw / (wins.len() as f32);
        let bar_w = (slot_w * 0.62).clamp(6.0, 32.0);

        let mut nmi_pts = Vec::with_capacity(wins.len());
        for (idx, w) in wins.iter().enumerate() {
            let cx = rect.left() + pad_l + slot_w * (idx as f32 + 0.5);
            let mut cur_y = rect.top() + pad_t + ph;
            for (d_idx, d) in Discipline::ALL.iter().enumerate() {
                let cnt = w.pub_counts_by_disc[d_idx];
                if cnt == 0 {
                    continue;
                }
                let seg_h = ((cnt as f32) / max_stack) * ph;
                let [r, g, b] = d.rgb();
                let seg_rect = Rect::from_min_max(
                    Pos2::new(cx - bar_w * 0.5, cur_y - seg_h),
                    Pos2::new(cx + bar_w * 0.5, cur_y),
                );
                painter.rect_filled(seg_rect, 0.0, Color32::from_rgba_unmultiplied(r, g, b, 200));
                cur_y -= seg_h;
            }
            let ny = rect.top() + pad_t + ph * (1.0 - (w.nmi as f32).clamp(0.0, 1.0));
            nmi_pts.push(Pos2::new(cx, ny));

            if wins.len() <= 8 || idx % 2 == 0 {
                painter.text(
                    Pos2::new(cx, rect.bottom() - 10.0),
                    Align2::CENTER_CENTER,
                    format!("{}", w.start_year),
                    FontId::monospace(9.5),
                    Color32::from_rgb(148, 163, 184),
                );
            }
        }

        for pair in nmi_pts.windows(2) {
            painter.line_segment(
                [pair[0], pair[1]],
                Stroke::new(2.2, Color32::from_rgb(16, 185, 129)),
            );
        }
        for pt in nmi_pts {
            painter.circle_filled(pt, 3.2, Color32::from_rgb(16, 185, 129));
        }
    }

    fn paint_overview_scatter(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 210.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::click());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));

        if self.analytics.papers.is_empty() {
            return;
        }
        let pad_l = 38.0;
        let pad_r = 18.0;
        let pad_t = 16.0;
        let pad_b = 24.0;
        let pw = (rect.width() - pad_l - pad_r).max(40.0);
        let ph = (rect.height() - pad_t - pad_b).max(40.0);

        let max_x = self
            .analytics
            .papers
            .iter()
            .map(|p| p.v_exp as f32)
            .fold(0.5f32, f32::max)
            * 1.12;
        let max_y = self
            .analytics
            .papers
            .iter()
            .map(|p| p.cb_undir as f32)
            .fold(0.05f32, f32::max)
            * 1.12;

        let click_pos = if resp.clicked() {
            resp.interact_pointer_pos()
        } else {
            None
        };

        let mut next_focus: Option<String> = None;
        for p in &self.analytics.papers {
            if !self.matches_paper_filter(p) {
                continue;
            }
            let cx = rect.left() + pad_l + ((p.v_exp as f32) / max_x) * pw;
            let cy = rect.top() + pad_t + ph - ((p.cb_undir as f32) / max_y) * ph;
            let center = Pos2::new(cx, cy);
            let rad = 4.2 + ((p.in_deg as f32).sqrt() * 2.2).min(9.5);
            let [r, g, b] = p.record.disc.rgb();

            if p.beauty_b >= 10.0 {
                painter.circle_stroke(
                    center,
                    rad + 3.0,
                    Stroke::new(1.5, Color32::from_rgb(250, 204, 21)),
                );
            }
            painter.circle_filled(center, rad, Color32::from_rgba_unmultiplied(r, g, b, 215));
            if p.record.id == self.focused_paper_id {
                painter.circle_stroke(center, rad + 1.2, Stroke::new(2.0, Color32::WHITE));
            }

            if let Some(cp) = click_pos {
                if cp.distance(center) <= rad + 4.0 {
                    next_focus = Some(p.record.id.clone());
                }
            }
        }
        if let Some(fid) = next_focus {
            self.focused_paper_id = fid;
            self.show_dossier_panel = true;
        }
    }

    // ========================================================================
    // DOMAIN 2: MULTI-PROVIDER SEARCH & VERIFIED OA PDF BATCH DOWNLOADER
    // ========================================================================

    // ========================================================================
    // DOMAIN 2: BUILT-IN INTERACTIVE NETWORK GRAPH VIEWER & GRAPHML/GEXF STUDIO
    // ========================================================================
    fn ui_network_graph_viewer(&mut self, ui: &mut egui::Ui) {
        // Row 1: Layout Preset, Live Physics Engine, Color & Size Encodings, One-Click GraphML/GEXF Export
        let mut relayout = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Layout:");
            for preset in [
                ViewerLayoutPreset::ForceAtlas2,
                ViewerLayoutPreset::HierarchicalDag,
                ViewerLayoutPreset::CommunityGalaxies,
                ViewerLayoutPreset::RadialEgo,
                ViewerLayoutPreset::CartographicPlane,
            ] {
                if ui
                    .selectable_value(&mut self.graph_viewer.layout, preset, preset.label())
                    .clicked()
                {
                    relayout = true;
                }
            }

            ui.separator();
            let play_lbl = if self.graph_viewer.physics_running {
                "Pause Physics"
            } else {
                "Live ForceAtlas2"
            };
            if ui
                .button(RichText::new(play_lbl).strong().color(if self.graph_viewer.physics_running {
                    Color32::from_rgb(250, 204, 21)
                } else {
                    Color32::from_rgb(52, 211, 153)
                }))
                .clicked()
            {
                self.graph_viewer.physics_running = !self.graph_viewer.physics_running;
            }
            if ui.button("Step").clicked() {
                if self.use_wgpu_pipeline {
                    self.gpu_step_once = true;
                } else {
                    self.graph_viewer.step_physics(&self.analytics);
                    self.rebuild_cpu_pick_grid();
                }
            }
            if ui
                .button(RichText::new("50k-Node GPU Scale Test").color(Color32::from_rgb(56, 189, 248)).strong())
                .clicked()
            {
                let (p, e) = generate_synthetic_corpus(
                    50_000,
                    self.syn_seed,
                    self.syn_beta,
                    SyntheticRegime::Convergent,
                );
                self.raw_papers = p;
                self.raw_edges = e;
                self.corpus_label = format!(
                    "50,000-Node Scale-Free GPU Benchmark ({} Directed Edges)",
                    self.raw_edges.len()
                );
                self.graph_viewer.show_labels = false;
                self.recompute_all();
                self.graph_viewer.physics_running = true;
                self.status_banner = format!(
                    "Generated 50,000 citation nodes and {} edges — simulating live on {}!",
                    self.raw_edges.len(),
                    self.wgpu_adapter_label
                );
            }
            if ui.button("Fit View").clicked() {
                self.graph_viewer.pan = [0.0, 0.0];
                self.graph_viewer.zoom = 1.0;
            }
            if !self.focused_paper_id.is_empty() {
                let is_pinned = self.graph_viewer.pinned_ids.contains(&self.focused_paper_id);
                if ui
                    .selectable_label(is_pinned, if is_pinned { "Unpin Focal" } else { "Pin Focal" })
                    .clicked()
                {
                    if is_pinned {
                        self.graph_viewer.pinned_ids.remove(&self.focused_paper_id);
                    } else {
                        self.graph_viewer.pinned_ids.insert(self.focused_paper_id.clone());
                    }
                }
            }

            ui.separator();
            if ui
                .button(RichText::new("Export .graphml").strong())
                .clicked()
            {
                self.sync_gpu_mirror_into_viewer();
                let p = std::path::Path::new("cit_atlas_network.graphml");
                match export_graphml(
                    &self.raw_papers,
                    &self.raw_edges,
                    &self.analytics,
                    Some(&self.graph_viewer.positions),
                    self.graph_viewer.color_by,
                    self.graph_viewer.show_datacite_diamonds || self.graph_viewer.show_bio_entity_nodes,
                    p,
                ) {
                    Ok(_) => {
                        self.status_banner =
                            "Exported cit_atlas_network.graphml with live viewer coordinates & 34 attributes.".to_string();
                    }
                    Err(e) => self.status_banner = e,
                }
            }
            if ui
                .button(RichText::new("Export .gexf").strong())
                .clicked()
            {
                self.sync_gpu_mirror_into_viewer();
                let p = std::path::Path::new("cit_atlas_network.gexf");
                match export_gexf(
                    &self.raw_papers,
                    &self.raw_edges,
                    &self.analytics,
                    Some(&self.graph_viewer.positions),
                    self.graph_viewer.color_by,
                    self.graph_viewer.show_datacite_diamonds || self.graph_viewer.show_bio_entity_nodes,
                    p,
                ) {
                    Ok(_) => {
                        self.status_banner =
                            "Exported cit_atlas_network.gexf (Gephi 1.3 viz colors, sizes & coordinates).".to_string();
                    }
                    Err(e) => self.status_banner = e,
                }
            }
        });

        // Row 2: Color, Size, Spotlight Neighborhood & Edge Rendering Controls
        ui.horizontal_wrapped(|ui| {
            ui.label("Color:");
            for (enc, lbl) in [
                (NodeColorEncoding::Discipline, "Discipline"),
                (NodeColorEncoding::LouvainCommunity, "Louvain Q"),
                (NodeColorEncoding::InfomapModule, "Infomap"),
                (NodeColorEncoding::VelocityVExp, "Velocity"),
                (NodeColorEncoding::SleepingBeautyB, "Beauty B"),
                (NodeColorEncoding::IntegrityStatus, "Integrity"),
                (NodeColorEncoding::BioEntityRichness, "Bio-Entities"),
            ] {
                if ui
                    .selectable_value(&mut self.graph_viewer.color_by, enc, lbl)
                    .clicked()
                    && matches!(
                        self.graph_viewer.layout,
                        ViewerLayoutPreset::CommunityGalaxies | ViewerLayoutPreset::CartographicPlane
                    )
                {
                    relayout = true;
                }
            }

            ui.separator();
            ui.label("Size:");
            egui::ComboBox::from_id_salt("viewer_size_combo")
                .selected_text(match self.graph_viewer.size_by {
                    NodeSizeEncoding::BrandesBetweenness => "Brandes CB_u",
                    NodeSizeEncoding::InCitations => "In-Citations",
                    NodeSizeEncoding::VelocityVExp => "Decay Velocity",
                    NodeSizeEncoding::SleepingBeautyB => "Sleeping Beauty B",
                    NodeSizeEncoding::InfomapFlow => "Infomap Flow",
                    NodeSizeEncoding::DataCiteDownloads => "DataCite Downloads",
                    NodeSizeEncoding::BioEntityBridges => "Bio-Entity Bridges",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.graph_viewer.size_by,
                        NodeSizeEncoding::BrandesBetweenness,
                        "Brandes Betweenness CB_u",
                    );
                    ui.selectable_value(
                        &mut self.graph_viewer.size_by,
                        NodeSizeEncoding::InCitations,
                        "In-Corpus Citations",
                    );
                    ui.selectable_value(
                        &mut self.graph_viewer.size_by,
                        NodeSizeEncoding::VelocityVExp,
                        "Decay Velocity v_exp",
                    );
                    ui.selectable_value(
                        &mut self.graph_viewer.size_by,
                        NodeSizeEncoding::SleepingBeautyB,
                        "Sleeping Beauty B",
                    );
                    ui.selectable_value(
                        &mut self.graph_viewer.size_by,
                        NodeSizeEncoding::InfomapFlow,
                        "Infomap Ergodic Flow",
                    );
                    ui.selectable_value(
                        &mut self.graph_viewer.size_by,
                        NodeSizeEncoding::DataCiteDownloads,
                        "DataCite Artifact Downloads",
                    );
                    ui.selectable_value(
                        &mut self.graph_viewer.size_by,
                        NodeSizeEncoding::BioEntityBridges,
                        "Bio-Entity Bridges (UniProt/PDB/EPMC)",
                    );
                });

            ui.separator();
            ui.label("Spotlight:");
            for (em, lbl) in [
                (EgoSpotlightMode::FullGraph, "All"),
                (EgoSpotlightMode::OneHopEgo, "1-Hop"),
                (EgoSpotlightMode::TwoHopEgo, "2-Hop"),
                (EgoSpotlightMode::UpstreamAncestors, "References"),
                (EgoSpotlightMode::DownstreamCiters, "Citers"),
            ] {
                ui.selectable_value(&mut self.graph_viewer.ego_mode, em, lbl);
            }

            ui.separator();
            ui.checkbox(&mut self.graph_viewer.curved_edges, "Curved Arcs");
            ui.checkbox(&mut self.graph_viewer.show_arrows, "Arrows");
            ui.checkbox(&mut self.graph_viewer.show_datacite_diamonds, "DataCite");
            if ui
                .checkbox(&mut self.graph_viewer.show_bio_entity_nodes, "UniProt/PDB/EPMC")
                .changed()
            {
                self.bump_gpu_revision();
            }
            egui::ComboBox::from_id_salt("bio_node_filter_combo")
                .selected_text(match self.graph_viewer.bio_filter {
                    BioNodeFilter::AllBioEntities => "All Bio",
                    BioNodeFilter::UniProtAndPdbOnly => "UniProt+PDB",
                    BioNodeFilter::EpmcOntologyOnly => "GO/EFO/ChEBI",
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(
                            &mut self.graph_viewer.bio_filter,
                            BioNodeFilter::AllBioEntities,
                            "All Bio-Entities (UniProt + PDB + Europe PMC)",
                        )
                        .clicked()
                    {
                        self.graph_viewer.rebuild_bio_entity_nodes(&self.analytics);
                        self.bump_gpu_revision();
                    }
                    if ui
                        .selectable_value(
                            &mut self.graph_viewer.bio_filter,
                            BioNodeFilter::UniProtAndPdbOnly,
                            "Structural & Protein Only (UniProt + PDB)",
                        )
                        .clicked()
                    {
                        self.graph_viewer.rebuild_bio_entity_nodes(&self.analytics);
                        self.bump_gpu_revision();
                    }
                    if ui
                        .selectable_value(
                            &mut self.graph_viewer.bio_filter,
                            BioNodeFilter::EpmcOntologyOnly,
                            "Europe PMC Ontology Only (GO / EFO / ChEBI / Taxon)",
                        )
                        .clicked()
                    {
                        self.graph_viewer.rebuild_bio_entity_nodes(&self.analytics);
                        self.bump_gpu_revision();
                    }
                });
            ui.checkbox(&mut self.graph_viewer.show_labels, "Labels");
            ui.checkbox(&mut self.graph_viewer.show_minimap, "Minimap");
        });

        ui.collapsing("ForceAtlas2 Physics & Edge Threshold Tuning", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Repulsion:");
                ui.add(egui::Slider::new(&mut self.graph_viewer.repulsion_strength, 0.05..=1.50));
                ui.label("Spring:");
                ui.add(egui::Slider::new(&mut self.graph_viewer.spring_strength, 0.10..=1.50));
                ui.label("Cluster Cohesion:");
                ui.add(egui::Slider::new(&mut self.graph_viewer.cluster_cohesion, 0.0..=0.80));
                ui.label("Min Edge EB:");
                ui.add(egui::Slider::new(&mut self.graph_viewer.min_edge_eb, 0.0..=0.25));
                ui.label("Top Labels:");
                ui.add(egui::Slider::new(&mut self.graph_viewer.label_top_k, 0..=60));
            });
        });

        if relayout {
            self.graph_viewer
                .recompute_preset_layout(&self.analytics, &self.focused_paper_id);
            self.rebuild_cpu_pick_grid();
            self.bump_gpu_revision();
        }

        // Interactive Graph Viewport Canvas (wgpu WGSL Compute + Instanced SDF Render, with CPU fallback)
        if self.use_wgpu_pipeline {
            self.paint_wgpu_network_viewer(ui);
        } else {
            self.paint_interactive_network_viewer(ui);
        }
    }


    fn paint_wgpu_network_viewer(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        let desired = Vec2::new(avail.x.max(320.0), (avail.y - 4.0).max(360.0));
        let (resp, painter) = ui.allocate_painter(desired, Sense::click_and_drag());
        let rect = resp.rect;

        painter.rect_filled(rect, 6.0, Color32::from_rgb(7, 11, 21));
        painter.rect_stroke(
            rect,
            6.0,
            Stroke::new(1.0, Color32::from_rgb(30, 41, 59)),
            egui::StrokeKind::Inside,
        );

        let n = self.analytics.papers.len();
        if n == 0 {
            return;
        }

        // Cursor-centered smooth scroll-wheel zoom (0.18x to 12.0x for 50k+ node exploration)
        let pointer_pos = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.hover_pos()));
        if resp.hovered() {
            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_y.abs() > 0.1 {
                let old_zoom = self.graph_viewer.zoom;
                let factor = (1.0 + scroll_y * 0.0020).clamp(0.80, 1.25);
                let new_zoom = (old_zoom * factor).clamp(0.18, 12.0);
                if let Some(pp) = pointer_pos {
                    let rel = pp - rect.center() - Vec2::new(self.graph_viewer.pan[0], self.graph_viewer.pan[1]);
                    let ratio = new_zoom / old_zoom.max(0.01);
                    self.graph_viewer.pan[0] -= rel.x * (ratio - 1.0);
                    self.graph_viewer.pan[1] -= rel.y * (ratio - 1.0);
                }
                self.graph_viewer.zoom = new_zoom;
            }
        }

        let half_w = rect.width() * 0.44 * self.graph_viewer.zoom;
        let half_h = rect.height() * 0.44 * self.graph_viewer.zoom;
        let origin = Pos2::new(
            rect.center().x + self.graph_viewer.pan[0],
            rect.center().y + self.graph_viewer.pan[1],
        );

        let world_to_screen = |wx: f32, wy: f32| -> Pos2 {
            Pos2::new(origin.x + wx * half_w, origin.y + wy * half_h)
        };
        let screen_to_world = |sp: Pos2| -> [f32; 2] {
            [
                ((sp.x - origin.x) / half_w.max(1.0)).clamp(-1.18, 1.18),
                ((sp.y - origin.y) / half_h.max(1.0)).clamp(-1.18, 1.18),
            ]
        };

        // O(1) SpatialPickGrid hover & click hit-testing across Paper, DataCite, and Bio-Entity nodes
        let n_art_vis = if self.graph_viewer.show_datacite_diamonds {
            self.graph_viewer.artifact_nodes.len()
        } else {
            0
        };
        let n_bio_vis = if self.graph_viewer.show_bio_entity_nodes {
            self.graph_viewer.bio_entity_nodes.len()
        } else {
            0
        };
        let total_vis_nodes = n + n_art_vis + n_bio_vis;
        let pick_radius_world = (14.0 / half_w.min(half_h).max(12.0)).clamp(0.0025, 0.08);
        let mut hovered_any_idx: Option<usize> = None;
        if let Some(pp) = pointer_pos {
            if rect.contains(pp) {
                let [wx, wy] = screen_to_world(pp);
                hovered_any_idx = if let Ok(mir) = self.gpu_mirror.lock() {
                    if mir.coords.len() >= total_vis_nodes {
                        mir.pick_grid
                            .pick_nearest(wx, wy, pick_radius_world)
                            .filter(|&idx| idx < total_vis_nodes)
                    } else {
                        self.cpu_pick_grid.pick_nearest(wx, wy, pick_radius_world)
                    }
                } else {
                    self.cpu_pick_grid.pick_nearest(wx, wy, pick_radius_world)
                };
                // Fallback direct check on bipartite DataCite & Bio-Entity seed positions before first readback
                if hovered_any_idx.is_none() {
                    let mut best_d2 = pick_radius_world * pick_radius_world;
                    for (k, art) in self.graph_viewer.artifact_nodes.iter().take(n_art_vis).enumerate() {
                        let d2 = (art.pos[0] - wx).powi(2) + (art.pos[1] - wy).powi(2);
                        if d2 <= best_d2 {
                            best_d2 = d2;
                            hovered_any_idx = Some(n + k);
                        }
                    }
                    for (k, bn) in self.graph_viewer.bio_entity_nodes.iter().take(n_bio_vis).enumerate() {
                        let d2 = (bn.pos[0] - wx).powi(2) + (bn.pos[1] - wy).powi(2);
                        if d2 <= best_d2 {
                            best_d2 = d2;
                            hovered_any_idx = Some(n + n_art_vis + k);
                        }
                    }
                }
            }
        }
        let hovered_idx = hovered_any_idx.filter(|&i| i < n);
        self.graph_viewer.hovered_node_id =
            hovered_idx.map(|i| self.analytics.papers[i].record.id.clone());

        if resp.drag_started() {
            if let Some(h_idx) = hovered_idx {
                let nid = self.analytics.papers[h_idx].record.id.clone();
                self.focused_paper_id = nid.clone();
                self.graph_viewer.dragged_node_id = Some(nid);
                self.bump_gpu_revision();
            } else {
                self.graph_viewer.dragged_node_id = None;
            }
        }

        let mut dragged_idx_i32: i32 = -1;
        let mut drag_world = [0.0f32, 0.0f32];
        if resp.dragged() {
            if let Some(ref drag_id) = self.graph_viewer.dragged_node_id.clone() {
                if let Some(pp) = pointer_pos {
                    drag_world = screen_to_world(pp);
                    self.graph_viewer.positions.insert(drag_id.clone(), drag_world);
                    if let Some(pos_idx) = self.analytics.papers.iter().position(|p| &p.record.id == drag_id) {
                        dragged_idx_i32 = pos_idx as i32;
                    }
                }
            } else {
                let delta = resp.drag_delta();
                self.graph_viewer.pan[0] += delta.x;
                self.graph_viewer.pan[1] += delta.y;
            }
        }

        if resp.drag_stopped() {
            self.graph_viewer.dragged_node_id = None;
        }

        if resp.clicked() {
            let target_paper_idx = hovered_idx.or_else(|| {
                let any_i = hovered_any_idx?;
                if any_i >= n && any_i < n + n_art_vis {
                    self.graph_viewer.artifact_nodes[any_i - n]
                        .linked_paper_indices
                        .first()
                        .copied()
                } else if any_i >= n + n_art_vis && any_i < total_vis_nodes {
                    self.graph_viewer.bio_entity_nodes[any_i - n - n_art_vis]
                        .linked_paper_indices
                        .first()
                        .copied()
                } else {
                    None
                }
            });
            if let Some(p_idx) = target_paper_idx {
                let new_focus = self.analytics.papers[p_idx].record.id.clone();
                if self.focused_paper_id != new_focus {
                    self.focused_paper_id = new_focus;
                    self.show_dossier_panel = true;
                    if self.graph_viewer.layout == ViewerLayoutPreset::RadialEgo {
                        self.graph_viewer
                            .recompute_preset_layout(&self.analytics, &self.focused_paper_id);
                    }
                    self.bump_gpu_revision();
                }
            }
        }

        // Detect visual-encoding / filter state hash so changing Color/Size/Spotlight/DataCite updates GPU buffers cleanly
        let state_sig = {
            let mut s: u64 = (n as u64).wrapping_mul(1_000_003);
            s ^= (self.graph_viewer.color_by as u64) << 8;
            s ^= (self.graph_viewer.size_by as u64) << 16;
            s ^= (self.graph_viewer.ego_mode as u64) << 24;
            s ^= (self.graph_viewer.show_datacite_diamonds as u64) << 32;
            s ^= (self.graph_viewer.show_bio_entity_nodes as u64) << 34;
            s ^= (self.graph_viewer.bio_filter as u64) << 38;
            s ^= (self.graph_viewer.pinned_ids.len() as u64) << 42;
            for b in self.focused_paper_id.bytes() {
                s = s.wrapping_mul(1099511628211).wrapping_add(b as u64);
            }
            s
        };
        if self.cached_gpu_upload.as_ref().map(|u| u.revision) != Some(self.gpu_scene_revision ^ state_sig) {
            let mut geodesic_pairs: HashSet<(usize, usize)> = HashSet::new();
            if n <= 8_000 {
                if let (Some(s_i), Some(t_i)) = (
                    self.analytics.papers.iter().position(|p| p.record.id == self.path_src_id),
                    self.analytics.papers.iter().position(|p| p.record.id == self.path_dst_id),
                ) {
                    if let Some(path_vec) =
                        find_shortest_epistemic_path(&self.analytics, s_i, t_i, self.params.bridge_directed)
                    {
                        for w in path_vec.windows(2) {
                            geodesic_pairs.insert((w[0], w[1]));
                            geodesic_pairs.insert((w[1], w[0]));
                        }
                    }
                }
            }
            let rev = (self.gpu_scene_revision ^ state_sig).max(1);
            self.cached_gpu_upload = Some(build_gpu_upload_payload(
                rev,
                &self.analytics,
                &self.graph_viewer,
                &self.focused_paper_id,
                &geodesic_pairs,
            ));
        }

        let run_step = if self.graph_viewer.physics_running || self.gpu_step_once {
            self.gpu_step_once = false;
            1u32
        } else {
            0u32
        };

        // Scale LOD alpha & node size automatically with corpus density (from 47 papers up to 100,000+ nodes)
        let density_node_scale = if n >= 25_000 {
            0.34
        } else if n >= 5_000 {
            0.52
        } else if n >= 500 {
            0.78
        } else {
            1.0
        };
        let density_edge_alpha = if self.analytics.active_edges.len() >= 100_000 {
            (0.14 * self.graph_viewer.zoom.sqrt()).clamp(0.08, 0.65)
        } else if self.analytics.active_edges.len() >= 10_000 {
            (0.32 * self.graph_viewer.zoom.sqrt()).clamp(0.16, 0.80)
        } else {
            0.88
        };

        let ideal_len = (2.1 / (n as f32).sqrt()).clamp(0.004, 0.35);
        let sim = SimUniformsPayload {
            node_count: n as u32,
            grid_dim: GRID_DIM,
            super_dim: SUPER_DIM,
            run_step,
            world_extent: WORLD_EXTENT,
            repulsion_k: self.graph_viewer.repulsion_strength,
            spring_k: self.graph_viewer.spring_strength,
            gravity_k: self.graph_viewer.gravity_strength,
            cohesion_k: self.graph_viewer.cluster_cohesion,
            damping: self.graph_viewer.damping,
            ideal_len,
            max_step: if n >= 10_000 { 0.018 } else { 0.032 },
            dragged_idx: dragged_idx_i32,
            drag_x: drag_world[0],
            drag_y: drag_world[1],
            pad0: 0.0,
        };
        let cam = CameraUniformsPayload {
            pan_x: self.graph_viewer.pan[0],
            pan_y: self.graph_viewer.pan[1],
            zoom: self.graph_viewer.zoom,
            aspect: rect.width() / rect.height().max(1.0),
            viewport_w: rect.width(),
            viewport_h: rect.height(),
            min_edge_eb: self.graph_viewer.min_edge_eb,
            edge_alpha_scale: density_edge_alpha,
            node_scale: density_node_scale,
            show_arrows: u32::from(self.graph_viewer.show_arrows),
            curved_edges: u32::from(self.graph_viewer.curved_edges),
            hovered_idx: hovered_any_idx.map(|i| i as i32).unwrap_or(-1),
        };

        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            WgpuGraphCallback {
                upload: self.cached_gpu_upload.clone(),
                sim,
                cam,
                mirror: Arc::clone(&self.gpu_mirror),
            },
        ));

        // Crisp HUD Telemetry Pill (Top-Right)
        let hud_txt = format!(
            "{}  |  {} Papers + {} DC + {} Bio  |  {} Edges  |  {:.0} FPS  |  {:.2}x",
            self.wgpu_adapter_label,
            n,
            n_art_vis,
            n_bio_vis,
            self.analytics.active_edges.len(),
            self.fps_ema,
            self.graph_viewer.zoom
        );
        let hud_w = (hud_txt.len() as f32 * 6.5 + 18.0).min(rect.width() - 20.0);
        let hud_rect = Rect::from_min_size(
            Pos2::new(rect.right() - hud_w - 10.0, rect.top() + 10.0),
            Vec2::new(hud_w, 24.0),
        );
        painter.rect_filled(hud_rect, 4.0, Color32::from_rgba_unmultiplied(15, 23, 42, 224));
        painter.text(
            Pos2::new(hud_rect.left() + 9.0, hud_rect.center().y),
            Align2::LEFT_CENTER,
            hud_txt,
            FontId::monospace(10.5),
            Color32::from_rgb(52, 211, 153),
        );

        // Top-K Bridge Landmark Labels Overlay (when enabled)
        if self.graph_viewer.show_labels && self.graph_viewer.label_top_k > 0 {
            let mut top_ixs: Vec<usize> = (0..n).collect();
            top_ixs.sort_by(|&a, &b| {
                self.analytics.papers[b]
                    .cb_undir
                    .partial_cmp(&self.analytics.papers[a].cb_undir)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            top_ixs.truncate(self.graph_viewer.label_top_k.min(60));
            let mir_guard = self.gpu_mirror.lock().ok();
            for idx in top_ixs {
                let p = &self.analytics.papers[idx];
                let [wx, wy] = mir_guard
                    .as_ref()
                    .and_then(|m| m.coords.get(idx).copied())
                    .or_else(|| self.graph_viewer.positions.get(&p.record.id).copied())
                    .unwrap_or([0.0, 0.0]);
                let sp = world_to_screen(wx, wy);
                if rect.contains(sp) {
                    painter.text(
                        Pos2::new(sp.x + 7.0, sp.y),
                        Align2::LEFT_CENTER,
                        p.record.short_cite(),
                        FontId::proportional(10.8),
                        Color32::from_rgb(226, 232, 240),
                    );
                }
            }
        }

        // Hover Tooltip Card (Paper, DataCite Artifact, or UniProt/PDB/Europe PMC Bio-Entity)
        let tooltip_info: Option<(String, Color32)> = if let Some(h_idx) = hovered_idx {
            let hp = &self.analytics.papers[h_idx];
            Some((
                format!(
                    "{} [{} · L{} · M{}]\n{}\nCB_u={:.2}% · v_exp={:.2} · UniProt={} · PDB={} · EPMC={} · Integrity={}",
                    hp.record.short_cite(),
                    hp.record.disc.code(),
                    hp.louvain_comm + 1,
                    hp.infomap_comm + 1,
                    hp.record.title,
                    hp.cb_undir * 100.0,
                    hp.v_exp,
                    hp.uniprot_bridge_count,
                    hp.pdb_bridge_count,
                    hp.epmc_annotation_count,
                    hp.record.retraction.status.as_str()
                ),
                Color32::from_rgb(56, 189, 248),
            ))
        } else if let Some(any_i) = hovered_any_idx {
            if any_i >= n && any_i < n + n_art_vis {
                let art = &self.graph_viewer.artifact_nodes[any_i - n];
                Some((
                    format!(
                        "[DataCite {}] {}\n{}\nRepo: {} · Downloads: {} · Linked Papers in Corpus: {}",
                        art.kind.as_str(),
                        art.artifact_doi,
                        art.title,
                        art.repository,
                        art.downloads,
                        art.linked_paper_indices.len()
                    ),
                    Color32::from_rgb(45, 212, 191),
                ))
            } else if any_i >= n + n_art_vis && any_i < total_vis_nodes {
                let bn = &self.graph_viewer.bio_entity_nodes[any_i - n - n_art_vis];
                let [cr, cg, cb] = bn.kind.rgb();
                let extra = if let Some(res) = bn.resolution_angstrom {
                    format!("Resolution: {:.2} A · ", res)
                } else if !bn.linked_pdb_ids.is_empty() {
                    format!("PDB Xrefs: {} · ", bn.linked_pdb_ids.join(","))
                } else {
                    String::new()
                };
                Some((
                    format!(
                        "[{} · {}] {}\n{}\n{}Mentions: {} · Linked Papers in Corpus: {}",
                        bn.kind.short_badge(),
                        bn.provider,
                        bn.entity_id,
                        bn.label,
                        extra,
                        bn.total_mentions,
                        bn.linked_paper_indices.len()
                    ),
                    Color32::from_rgb(cr, cg, cb),
                ))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((tip_txt, border_col)) = tooltip_info {
            let box_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 12.0, rect.top() + 12.0),
                Vec2::new(430.0f32.min(rect.width() - 24.0), 58.0),
            );
            painter.rect_filled(box_rect, 5.0, Color32::from_rgba_unmultiplied(15, 23, 42, 236));
            painter.rect_stroke(
                box_rect,
                5.0,
                Stroke::new(1.0, border_col),
                egui::StrokeKind::Inside,
            );
            painter.text(
                Pos2::new(box_rect.left() + 8.0, box_rect.top() + 8.0),
                Align2::LEFT_TOP,
                tip_txt,
                FontId::monospace(10.8),
                Color32::from_rgb(241, 245, 249),
            );
        }

        // Strided O(1) Radar Minimap (caps at <= 1,500 sample dots even on 100,000-node graphs)
        if self.graph_viewer.show_minimap && rect.width() > 420.0 && rect.height() > 280.0 {
            let mm_w = 138.0;
            let mm_h = 98.0;
            let mm_rect = Rect::from_min_size(
                Pos2::new(rect.right() - mm_w - 10.0, rect.bottom() - mm_h - 10.0),
                Vec2::new(mm_w, mm_h),
            );
            painter.rect_filled(mm_rect, 4.0, Color32::from_rgba_unmultiplied(15, 23, 42, 218));
            painter.rect_stroke(
                mm_rect,
                4.0,
                Stroke::new(1.0, Color32::from_rgb(51, 65, 85)),
                egui::StrokeKind::Inside,
            );
            let step = (n / 1_200).max(1);
            let mir_guard = self.gpu_mirror.lock().ok();
            let mut idx = 0usize;
            while idx < n {
                let p = &self.analytics.papers[idx];
                let [wx, wy] = mir_guard
                    .as_ref()
                    .and_then(|m| m.coords.get(idx).copied())
                    .or_else(|| self.graph_viewer.positions.get(&p.record.id).copied())
                    .unwrap_or([0.0, 0.0]);
                let mx = mm_rect.center().x + (wx / WORLD_EXTENT) * (mm_w * 0.45);
                let my = mm_rect.center().y + (wy / WORLD_EXTENT) * (mm_h * 0.45);
                if mm_rect.contains(Pos2::new(mx, my)) {
                    let [cr, cg, cb] = p.record.disc.rgb();
                    painter.circle_filled(Pos2::new(mx, my), 1.4, Color32::from_rgb(cr, cg, cb));
                }
                idx += step;
            }
            let vp_scale = (1.0 / self.graph_viewer.zoom).clamp(0.08, 1.0);
            let vp_cx = mm_rect.center().x
                - (self.graph_viewer.pan[0] / half_w.max(1.0)) * (mm_w * 0.44);
            let vp_cy = mm_rect.center().y
                - (self.graph_viewer.pan[1] / half_h.max(1.0)) * (mm_h * 0.44);
            let vp_rect = Rect::from_center_size(
                Pos2::new(vp_cx, vp_cy),
                Vec2::new(mm_w * 0.88 * vp_scale, mm_h * 0.88 * vp_scale),
            )
            .intersect(mm_rect);
            painter.rect_stroke(
                vp_rect,
                2.0,
                Stroke::new(1.0, Color32::from_rgb(56, 189, 248)),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn paint_interactive_network_viewer(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        let desired = Vec2::new(avail.x.max(320.0), (avail.y - 4.0).max(360.0));
        let (resp, painter) = ui.allocate_painter(desired, Sense::click_and_drag());
        let rect = resp.rect;

        painter.rect_filled(rect, 6.0, Color32::from_rgb(8, 13, 24));
        painter.rect_stroke(
            rect,
            6.0,
            Stroke::new(1.0, Color32::from_rgb(30, 41, 59)),
            egui::StrokeKind::Inside,
        );

        let n = self.analytics.papers.len();
        if n == 0 {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Corpus contains 0 active papers under current filter/horizon.",
                FontId::proportional(13.0),
                Color32::from_rgb(148, 163, 184),
            );
            return;
        }

        // Scroll-wheel zoom centered on pointer
        if resp.hovered() {
            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_y.abs() > 0.1 {
                let factor = (1.0 + scroll_y * 0.0018).clamp(0.82, 1.22);
                self.graph_viewer.zoom = (self.graph_viewer.zoom * factor).clamp(0.28, 6.0);
            }
        }

        let half_w = rect.width() * 0.44 * self.graph_viewer.zoom;
        let half_h = rect.height() * 0.44 * self.graph_viewer.zoom;
        let origin = Pos2::new(
            rect.center().x + self.graph_viewer.pan[0],
            rect.center().y + self.graph_viewer.pan[1],
        );

        let world_to_screen = |wx: f32, wy: f32| -> Pos2 {
            Pos2::new(origin.x + wx * half_w, origin.y + wy * half_h)
        };
        let screen_to_world = |sp: Pos2| -> [f32; 2] {
            [
                ((sp.x - origin.x) / half_w.max(1.0)).clamp(-0.96, 0.96),
                ((sp.y - origin.y) / half_h.max(1.0)).clamp(-0.96, 0.96),
            ]
        };

        // Subtle coordinate grid
        for step in -2..=2 {
            let f = (step as f32) * 0.45;
            let gx = origin.x + f * half_w;
            let gy = origin.y + f * half_h;
            if gx > rect.left() && gx < rect.right() {
                painter.line_segment(
                    [Pos2::new(gx, rect.top() + 2.0), Pos2::new(gx, rect.bottom() - 2.0)],
                    Stroke::new(0.6, Color32::from_rgba_unmultiplied(30, 41, 59, 110)),
                );
            }
            if gy > rect.top() && gy < rect.bottom() {
                painter.line_segment(
                    [Pos2::new(rect.left() + 2.0, gy), Pos2::new(rect.right() - 2.0, gy)],
                    Stroke::new(0.6, Color32::from_rgba_unmultiplied(30, 41, 59, 110)),
                );
            }
        }

        let max_cb = self.analytics.papers.iter().map(|p| p.cb_undir).fold(0.05, f64::max);
        let max_v = self.analytics.papers.iter().map(|p| p.v_exp).fold(0.5, f64::max);
        let max_b = self.analytics.papers.iter().map(|p| p.beauty_b).fold(5.0, f64::max);

        // Precompute screen positions & radii
        let mut screen_pts = vec![rect.center(); n];
        let mut radii = vec![6.0f32; n];
        for (i, p) in self.analytics.papers.iter().enumerate() {
            let [wx, wy] = self
                .graph_viewer
                .positions
                .get(&p.record.id)
                .copied()
                .unwrap_or([0.0, 0.0]);
            screen_pts[i] = world_to_screen(wx, wy);
            radii[i] = node_radius_for_encoding(p, self.graph_viewer.size_by, max_cb, max_v, max_b)
                * self.graph_viewer.zoom.sqrt().clamp(0.75, 1.65);
        }

        // Hit-test pointer for hover, click selection, and individual node dragging
        let pointer_pos = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.hover_pos()));
        let mut hovered_idx: Option<usize> = None;
        if let Some(pp) = pointer_pos {
            if rect.contains(pp) {
                let mut best_d = 18.0f32;
                for i in 0..n {
                    let d = pp.distance(screen_pts[i]);
                    if d <= radii[i] + 5.0 && d < best_d {
                        best_d = d;
                        hovered_idx = Some(i);
                    }
                }
            }
        }
        self.graph_viewer.hovered_node_id =
            hovered_idx.map(|i| self.analytics.papers[i].record.id.clone());

        if resp.drag_started() {
            if let Some(h_idx) = hovered_idx {
                let nid = self.analytics.papers[h_idx].record.id.clone();
                self.focused_paper_id = nid.clone();
                self.graph_viewer.dragged_node_id = Some(nid);
            } else {
                self.graph_viewer.dragged_node_id = None;
            }
        }

        if resp.dragged() {
            if let Some(ref drag_id) = self.graph_viewer.dragged_node_id.clone() {
                if let Some(pp) = pointer_pos {
                    let wxy = screen_to_world(pp);
                    self.graph_viewer.positions.insert(drag_id.clone(), wxy);
                    self.graph_viewer.rebuild_artifact_nodes(&self.analytics);
                }
            } else {
                let delta = resp.drag_delta();
                self.graph_viewer.pan[0] += delta.x;
                self.graph_viewer.pan[1] += delta.y;
            }
        }

        if resp.drag_stopped() {
            self.graph_viewer.dragged_node_id = None;
        }

        if resp.clicked() {
            if let Some(h_idx) = hovered_idx {
                self.focused_paper_id = self.analytics.papers[h_idx].record.id.clone();
                self.show_dossier_panel = true;
                if self.graph_viewer.layout == ViewerLayoutPreset::RadialEgo {
                    self.graph_viewer
                        .recompute_preset_layout(&self.analytics, &self.focused_paper_id);
                }
            }
        }

        let spotlight_mask = compute_spotlight_mask(
            &self.analytics,
            &self.focused_paper_id,
            self.graph_viewer.ego_mode,
        );

        // Shortest geodesic path overlay (from path_src_id to path_dst_id)
        let mut geodesic_pairs: HashSet<(usize, usize)> = HashSet::new();
        if let (Some(s_i), Some(t_i)) = (
            self.analytics.papers.iter().position(|p| p.record.id == self.path_src_id),
            self.analytics.papers.iter().position(|p| p.record.id == self.path_dst_id),
        ) {
            if let Some(path_vec) =
                find_shortest_epistemic_path(&self.analytics, s_i, t_i, self.params.bridge_directed)
            {
                for w in path_vec.windows(2) {
                    geodesic_pairs.insert((w[0], w[1]));
                    geodesic_pairs.insert((w[1], w[0]));
                }
            }
        }

        let mut conduit_eb: HashMap<(usize, usize), f64> = HashMap::new();
        for c in &self.analytics.conduits {
            conduit_eb.insert((c.u, c.v), c.eb_undir);
        }

        // 1. Draw Bipartite DataCite Artifact Links & Diamond Nodes (if toggled)
        if self.graph_viewer.show_datacite_diamonds {
            for art in &self.graph_viewer.artifact_nodes {
                let a_screen = world_to_screen(art.pos[0], art.pos[1]);
                if !rect.contains(a_screen) {
                    continue;
                }
                let any_vis = art.linked_paper_indices.iter().any(|&pi| spotlight_mask[pi]);
                let alpha = if any_vis { 185 } else { 38 };
                let art_col = if art.kind.is_software_like() {
                    Color32::from_rgba_unmultiplied(167, 139, 250, alpha)
                } else {
                    Color32::from_rgba_unmultiplied(45, 212, 191, alpha)
                };

                for &pi in &art.linked_paper_indices {
                    if rect.contains(screen_pts[pi]) {
                        painter.line_segment(
                            [screen_pts[pi], a_screen],
                            Stroke::new(1.1, art_col),
                        );
                    }
                }

                let d = 6.2 * self.graph_viewer.zoom.sqrt().clamp(0.8, 1.5);
                let diamond = vec![
                    Pos2::new(a_screen.x, a_screen.y - d),
                    Pos2::new(a_screen.x + d, a_screen.y),
                    Pos2::new(a_screen.x, a_screen.y + d),
                    Pos2::new(a_screen.x - d, a_screen.y),
                ];
                painter.add(egui::Shape::convex_polygon(
                    diamond,
                    art_col,
                    Stroke::new(1.0, Color32::from_rgb(241, 245, 249)),
                ));
            }
        }

        // 1b. Draw Bipartite Europe PMC, UniProt Hexagon & PDB Octagon Bridge Nodes (if toggled)
        if self.graph_viewer.show_bio_entity_nodes {
            let mut bio_screens = Vec::with_capacity(self.graph_viewer.bio_entity_nodes.len());
            for bn in &self.graph_viewer.bio_entity_nodes {
                let b_screen = world_to_screen(bn.pos[0], bn.pos[1]);
                bio_screens.push(b_screen);
                if !rect.contains(b_screen) {
                    continue;
                }
                let any_vis = bn.linked_paper_indices.iter().any(|&pi| spotlight_mask[pi]);
                let alpha = if any_vis { 205 } else { 42 };
                let [cr, cg, cb] = bn.kind.rgb();
                let bio_col = Color32::from_rgba_unmultiplied(cr, cg, cb, alpha);

                for &pi in &bn.linked_paper_indices {
                    if rect.contains(screen_pts[pi]) {
                        painter.line_segment([screen_pts[pi], b_screen], Stroke::new(1.1, bio_col));
                    }
                }

                let d = 6.4 * self.graph_viewer.zoom.sqrt().clamp(0.8, 1.5);
                let poly: Vec<Pos2> = match bn.kind {
                    BioEntityKind::UniProt => (0..6)
                        .map(|k| {
                            let a = (k as f32) * std::f32::consts::FRAC_PI_3;
                            Pos2::new(b_screen.x + a.cos() * d, b_screen.y + a.sin() * d)
                        })
                        .collect(),
                    BioEntityKind::PdbStructure => (0..8)
                        .map(|k| {
                            let a = (k as f32) * std::f32::consts::FRAC_PI_4 + 0.39;
                            Pos2::new(b_screen.x + a.cos() * d, b_screen.y + a.sin() * d)
                        })
                        .collect(),
                    _ => vec![
                        Pos2::new(b_screen.x, b_screen.y - d * 1.08),
                        Pos2::new(b_screen.x + d * 0.98, b_screen.y + d * 0.78),
                        Pos2::new(b_screen.x - d * 0.98, b_screen.y + d * 0.78),
                    ],
                };
                painter.add(egui::Shape::convex_polygon(
                    poly,
                    bio_col,
                    Stroke::new(1.0, Color32::from_rgb(241, 245, 249)),
                ));
            }
            for &(u_rel, p_rel) in &self.graph_viewer.bio_cross_edges {
                if let (Some(&p1), Some(&p2)) = (bio_screens.get(u_rel), bio_screens.get(p_rel)) {
                    if rect.contains(p1) || rect.contains(p2) {
                        painter.line_segment(
                            [p1, p2],
                            Stroke::new(1.4, Color32::from_rgba_unmultiplied(34, 211, 238, 210)),
                        );
                    }
                }
            }
        }

        // 2. Draw Directed Citation Edges (Curved Quadratic Bezier or Straight + Arrowheads)
        for &(u, v) in &self.analytics.active_edges {
            let eb = conduit_eb.get(&(u, v)).copied().unwrap_or(0.0) as f32;
            if eb < self.graph_viewer.min_edge_eb && !geodesic_pairs.contains(&(u, v)) {
                continue;
            }
            let p1 = screen_pts[u];
            let p2 = screen_pts[v];
            if !rect.expand(80.0).contains(p1) && !rect.expand(80.0).contains(p2) {
                continue;
            }

            let both_spot = spotlight_mask[u] && spotlight_mask[v];
            let is_geo = geodesic_pairs.contains(&(u, v));
            let is_zombie = self.graph_viewer.highlight_zombie_edges
                && self.analytics.papers[v]
                    .record
                    .retraction
                    .status
                    .is_retracted_or_withdrawn()
                && self.analytics.papers[u].record.year
                    >= self.analytics.papers[v]
                        .record
                        .retraction
                        .notice_year
                        .unwrap_or(self.analytics.papers[v].record.year);

            let (stroke_w, edge_col) = if is_geo {
                (2.7, Color32::from_rgb(34, 211, 238))
            } else if is_zombie {
                (2.1, Color32::from_rgba_unmultiplied(248, 113, 113, if both_spot { 225 } else { 60 }))
            } else if !both_spot {
                (0.7, Color32::from_rgba_unmultiplied(71, 85, 105, 34))
            } else if self.analytics.papers[u].record.disc != self.analytics.papers[v].record.disc {
                (1.35 + eb * 4.0, Color32::from_rgba_unmultiplied(148, 163, 184, 150))
            } else {
                (1.0 + eb * 3.0, Color32::from_rgba_unmultiplied(100, 116, 139, 105))
            };

            let dir = p2 - p1;
            let dist = dir.length();
            if dist < 2.0 {
                continue;
            }
            let unit = dir / dist;
            let perp = Vec2::new(-unit.y, unit.x);
            let tgt_stop = p2 - unit * (radii[v] + 2.0);

            if self.graph_viewer.curved_edges {
                let ctrl = p1 + dir * 0.5 + perp * (dist * 0.13).min(38.0);
                let mut prev = p1;
                let steps = 12;
                for s in 1..=steps {
                    let t = (s as f32) / (steps as f32);
                    let omt = 1.0 - t;
                    let cur = Pos2::new(
                        omt * omt * p1.x + 2.0 * omt * t * ctrl.x + t * t * tgt_stop.x,
                        omt * omt * p1.y + 2.0 * omt * t * ctrl.y + t * t * tgt_stop.y,
                    );
                    painter.line_segment([prev, cur], Stroke::new(stroke_w, edge_col));
                    prev = cur;
                }
                if self.graph_viewer.show_arrows && both_spot && dist > 22.0 {
                    let tang = (tgt_stop - ctrl).normalized();
                    let nrm = Vec2::new(-tang.y, tang.x);
                    let a_len = (4.8 + stroke_w * 1.1).min(9.0);
                    let tip = tgt_stop;
                    let left = tip - tang * a_len + nrm * (a_len * 0.52);
                    let right = tip - tang * a_len - nrm * (a_len * 0.52);
                    painter.add(egui::Shape::convex_polygon(
                        vec![tip, left, right],
                        edge_col,
                        Stroke::NONE,
                    ));
                }
            } else {
                painter.line_segment([p1, tgt_stop], Stroke::new(stroke_w, edge_col));
                if self.graph_viewer.show_arrows && both_spot && dist > 22.0 {
                    let a_len = (4.8 + stroke_w * 1.1).min(9.0);
                    let left = tgt_stop - unit * a_len + perp * (a_len * 0.52);
                    let right = tgt_stop - unit * a_len - perp * (a_len * 0.52);
                    painter.add(egui::Shape::convex_polygon(
                        vec![tgt_stop, left, right],
                        edge_col,
                        Stroke::NONE,
                    ));
                }
            }
        }

        // Determine top-K nodes by importance for crisp non-overlapping labels
        let mut label_rank: Vec<usize> = (0..n).collect();
        label_rank.sort_by(|&a, &b| {
            self.analytics.papers[b]
                .cb_undir
                .partial_cmp(&self.analytics.papers[a].cb_undir)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_label_set: HashSet<usize> = label_rank
            .into_iter()
            .take(self.graph_viewer.label_top_k)
            .collect();

        // 3. Draw Paper Nodes, Integrity Halos, Sleeping Beauty Rings & Labels
        for (i, p) in self.analytics.papers.iter().enumerate() {
            let center = screen_pts[i];
            if !rect.expand(24.0).contains(center) {
                continue;
            }
            let r = radii[i];
            let in_spot = spotlight_mask[i] && self.matches_paper_filter(p);
            let alpha = if in_spot { 238 } else { 45 };
            let [cr, cg, cb] =
                node_rgb_for_encoding(p, self.graph_viewer.color_by, max_v, max_b);
            let fill = Color32::from_rgba_unmultiplied(cr, cg, cb, alpha);

            let is_focal = p.record.id == self.focused_paper_id;
            let is_pinned = self.graph_viewer.pinned_ids.contains(&p.record.id);

            if p.record.retraction.status.is_retracted_or_withdrawn() {
                painter.circle_stroke(
                    center,
                    r + 4.5,
                    Stroke::new(2.2, Color32::from_rgb(239, 68, 68)),
                );
            } else if p.beauty_b >= 10.0 && in_spot {
                painter.circle_stroke(
                    center,
                    r + 3.2,
                    Stroke::new(1.5, Color32::from_rgba_unmultiplied(250, 204, 21, 190)),
                );
            }

            if is_focal {
                painter.circle_stroke(
                    center,
                    r + 6.0,
                    Stroke::new(2.2, Color32::from_rgb(56, 189, 248)),
                );
            }

            painter.circle_filled(center, r, fill);
            painter.circle_stroke(
                center,
                r,
                Stroke::new(
                    if is_pinned { 2.0 } else { 0.9 },
                    if is_pinned {
                        Color32::from_rgb(250, 204, 21)
                    } else {
                        Color32::from_rgba_unmultiplied(248, 250, 252, alpha)
                    },
                ),
            );

            if self.graph_viewer.show_labels
                && in_spot
                && (is_focal || hovered_idx == Some(i) || top_label_set.contains(&i))
            {
                painter.text(
                    Pos2::new(center.x + r + 4.5, center.y),
                    Align2::LEFT_CENTER,
                    p.record.short_cite(),
                    FontId::proportional(11.0),
                    Color32::from_rgb(226, 232, 240),
                );
            }
        }

        // 4. Live Hover Tooltip Overlay
        if let Some(h_idx) = hovered_idx {
            let hp = &self.analytics.papers[h_idx];
            let tip_txt = format!(
                "{} [{} · L{} · M{}]\n{}\nCB_u={:.1}% · v_exp={:.2} · B={:.1} · Integrity={}",
                hp.record.short_cite(),
                hp.record.disc.code(),
                hp.louvain_comm + 1,
                hp.infomap_comm + 1,
                hp.record.title,
                hp.cb_undir * 100.0,
                hp.v_exp,
                hp.beauty_b,
                hp.record.retraction.status.as_str()
            );
            let box_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 12.0, rect.top() + 12.0),
                Vec2::new(430.0f32.min(rect.width() - 24.0), 58.0),
            );
            painter.rect_filled(
                box_rect,
                5.0,
                Color32::from_rgba_unmultiplied(15, 23, 42, 236),
            );
            painter.rect_stroke(
                box_rect,
                5.0,
                Stroke::new(1.0, Color32::from_rgb(56, 189, 248)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                Pos2::new(box_rect.left() + 8.0, box_rect.top() + 8.0),
                Align2::LEFT_TOP,
                tip_txt,
                FontId::monospace(10.8),
                Color32::from_rgb(241, 245, 249),
            );
        }

        // 5. Bottom-Right Radar Minimap
        if self.graph_viewer.show_minimap && rect.width() > 420.0 && rect.height() > 280.0 {
            let mm_w = 136.0;
            let mm_h = 96.0;
            let mm_rect = Rect::from_min_size(
                Pos2::new(rect.right() - mm_w - 10.0, rect.bottom() - mm_h - 10.0),
                Vec2::new(mm_w, mm_h),
            );
            painter.rect_filled(
                mm_rect,
                4.0,
                Color32::from_rgba_unmultiplied(15, 23, 42, 218),
            );
            painter.rect_stroke(
                mm_rect,
                4.0,
                Stroke::new(1.0, Color32::from_rgb(51, 65, 85)),
                egui::StrokeKind::Inside,
            );
            for p in &self.analytics.papers {
                let [wx, wy] = self
                    .graph_viewer
                    .positions
                    .get(&p.record.id)
                    .copied()
                    .unwrap_or([0.0, 0.0]);
                let mx = mm_rect.center().x + wx * (mm_w * 0.44);
                let my = mm_rect.center().y + wy * (mm_h * 0.44);
                let [cr, cg, cb] = p.record.disc.rgb();
                painter.circle_filled(Pos2::new(mx, my), 1.8, Color32::from_rgb(cr, cg, cb));
            }
            let vp_scale = (1.0 / self.graph_viewer.zoom).clamp(0.15, 1.0);
            let vp_cx = mm_rect.center().x
                - (self.graph_viewer.pan[0] / half_w.max(1.0)) * (mm_w * 0.44);
            let vp_cy = mm_rect.center().y
                - (self.graph_viewer.pan[1] / half_h.max(1.0)) * (mm_h * 0.44);
            let vp_rect = Rect::from_center_size(
                Pos2::new(vp_cx, vp_cy),
                Vec2::new(mm_w * 0.88 * vp_scale, mm_h * 0.88 * vp_scale),
            )
            .intersect(mm_rect);
            painter.rect_stroke(
                vp_rect,
                2.0,
                Stroke::new(1.0, Color32::from_rgb(56, 189, 248)),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn ui_search_and_download(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |cols| {
            // Left: 13-Source API Search & Polite Web Crawler Studio
            cols[0].group(|ui| {
                ui.heading("Scholarly APIs & Polite Web Crawler (13 Repositories & Citation Graphs)");
                ui.weak("Harvests OpenAlex, Semantic Scholar, Crossref, arXiv, OpenAIRE Graph + ScholExplorer, INSPIRE-HEP, NASA ADS, DataCite, Google Scholar, PubMed, Europe PMC, DBLP, and bioRxiv/medRxiv.");
                ui.add_space(6.0);

                ui.label("Search / Crawl Query:");
                ui.add(egui::TextEdit::singleline(&mut self.search_req.query).desired_width(f32::INFINITY));

                ui.horizontal_wrapped(|ui| {
                    ui.strong("APIs:");
                    ui.checkbox(&mut self.search_req.providers.openalex, "OpenAlex");
                    ui.checkbox(&mut self.search_req.providers.semantic_scholar, "Semantic Scholar");
                    ui.checkbox(&mut self.search_req.providers.crossref, "Crossref");
                    ui.checkbox(&mut self.search_req.providers.arxiv, "arXiv");
                    ui.checkbox(&mut self.search_req.providers.openaire, "OpenAIRE Graph");
                    ui.checkbox(&mut self.search_req.providers.inspire_hep, "INSPIRE-HEP");
                    ui.checkbox(&mut self.search_req.providers.nasa_ads, "NASA ADS");
                    ui.checkbox(&mut self.search_req.providers.datacite, "DataCite");
                });

                ui.horizontal_wrapped(|ui| {
                    ui.strong("Crawlers:");
                    ui.checkbox(&mut self.search_req.providers.google_scholar, "Google Scholar");
                    ui.checkbox(&mut self.search_req.providers.pubmed, "PubMed");
                    ui.checkbox(&mut self.search_req.providers.europe_pmc, "Europe PMC");
                    ui.checkbox(&mut self.search_req.providers.dblp, "DBLP");
                    ui.checkbox(&mut self.search_req.providers.biorxiv_medrxiv, "bioRxiv/medRxiv");
                });

                ui.horizontal_wrapped(|ui| {
                    ui.label("Max / Source:");
                    ui.add(egui::Slider::new(&mut self.search_req.max_per_provider, 5..=60));
                    ui.checkbox(&mut self.search_req.oa_only, "OA Only");
                    let mut snowball = self.search_req.snowball_hops > 0;
                    if ui.checkbox(&mut snowball, "1-Hop Snowball").changed() {
                        self.search_req.snowball_hops = if snowball { 1 } else { 0 };
                    }
                });

                ui.collapsing("Polite Crawler & API Tokens (OpenAIRE, NASA ADS, Scholar SERP & Hydration)", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("OpenAIRE Token:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_req.crawl.openaire_token)
                                .password(true)
                                .desired_width(145.0)
                                .hint_text("Optional (or OPENAIRE_API_TOKEN)"),
                        );
                        ui.label("NASA ADS Token:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_req.crawl.nasa_ads_token)
                                .password(true)
                                .desired_width(145.0)
                                .hint_text("Bearer (or ADS_API_TOKEN)"),
                        );
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("SERP Pages:");
                        ui.add(egui::Slider::new(&mut self.search_req.crawl.max_pages, 1..=8));
                        ui.label("Crawl Pace (ms):");
                        ui.add(egui::Slider::new(&mut self.search_req.crawl.pace_ms, 500..=8000));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.search_req.crawl.respect_robots_txt, "Respect robots.txt");
                        ui.checkbox(&mut self.search_req.crawl.hydrate_citation_edges, "Hydrate OpenCitations + iCite");
                        ui.checkbox(&mut self.search_req.crawl.filter_self_citations, "Drop Self-Citations");
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.search_req.crawl.screen_retractions, "Crossref Retraction Watch");
                        ui.checkbox(&mut self.search_req.crawl.track_datacite_bridges, "DataCite Dataset/Software Bridges");
                        ui.checkbox(&mut self.search_req.crawl.track_epmc_annotations, "Europe PMC Annotations (GO/EFO/ChEBI)");
                        ui.checkbox(&mut self.search_req.crawl.track_uniprot_pdb_bridges, "UniProtKB + RCSB PDB Bridges");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Local RW CSV:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.rw_csv_path_input)
                                .desired_width(240.0)
                                .hint_text("Optional retraction_watch.csv"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Scholar cites= ID:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_req.crawl.scholar_cites_cluster_id)
                                .desired_width(145.0)
                                .hint_text("Optional cluster ID"),
                        );
                        ui.label("Proxy:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_req.crawl.proxy_url)
                                .desired_width(150.0)
                                .hint_text("http://127.0.0.1:8080"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Saved Scholar .html:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.local_scholar_html_input)
                                .desired_width(240.0)
                                .hint_text("path/to/scholar_page1.html"),
                        );
                    });
                });

                ui.horizontal_wrapped(|ui| {
                    ui.label("Polite Pool Email:");
                    ui.add(egui::TextEdit::singleline(&mut self.search_req.polite_email).desired_width(200.0));
                    ui.checkbox(&mut self.merge_search_results, "Merge Into Current Graph");
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.is_busy,
                            egui::Button::new(RichText::new("Run Search / Crawl & Build Graph").strong()),
                        )
                        .clicked()
                    {
                        self.start_background_search();
                    }
                    if ui
                        .add_enabled(
                            !self.is_busy && !self.raw_papers.is_empty(),
                            egui::Button::new("Screen Corpus (Retractions + DataCite + Europe PMC + UniProt/PDB)"),
                        )
                        .clicked()
                    {
                        self.start_background_screening();
                    }
                    if ui.button("Export Raw Catalog CSV").clicked() {
                        let out_dir = PathBuf::from(self.download_dir.trim());
                        let _ = std::fs::create_dir_all(&out_dir);
                        let csv_p = out_dir.join("raw_harvest_catalog.csv");
                        match export_raw_papers_csv(&self.raw_papers, &csv_p) {
                            Ok(_) => self.status_banner = format!("Saved raw catalog CSV to {:?}", csv_p),
                            Err(e) => self.status_banner = e,
                        }
                    }
                });

                ui.separator();
                ui.label(RichText::new("Single DOI Direct Resolver").strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.single_doi_input)
                            .desired_width(240.0)
                            .hint_text("10.1073/pnas.1424329112"),
                    );
                    if ui
                        .add_enabled(!self.is_busy, egui::Button::new("Resolve & Add DOI"))
                        .clicked()
                    {
                        let doi = self.single_doi_input.clone();
                        let email = self.search_req.polite_email.clone();
                        let tx = self.worker_tx.clone();
                        self.is_busy = true;
                        self.busy_label = format!("Resolving DOI {}...", doi);
                        thread::spawn(move || {
                            let res = lookup_single_doi(&doi, &email);
                            let _ = tx.send(WorkerEvent::DoiLookupCompleted(res));
                        });
                    }
                });
            });

            // Right: Verified Legal Open-Access PDF Batch Downloader
            cols[1].group(|ui| {
                ui.heading("Verified Open-Access PDF Batch Downloader");
                ui.weak("4-Stage Resolution: Provider OA URL -> Unpaywall v2 DOI -> arXiv Direct PDF -> Europe PMC. Verifies %PDF- binary magic header, computes SHA-256, and writes BibTeX + audit manifest.");
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label("Output Folder:");
                    ui.add(egui::TextEdit::singleline(&mut self.download_dir).desired_width(240.0));
                });

                ui.horizontal(|ui| {
                    ui.label("Polite Request Pacing (ms):");
                    ui.add(egui::Slider::new(&mut self.download_pace_ms, 100..=2000));
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.is_busy && !self.raw_papers.is_empty(),
                            egui::Button::new(
                                RichText::new(format!(
                                    "Batch Download OA PDFs ({} Corpus Papers)",
                                    self.raw_papers.len()
                                ))
                                .strong(),
                            ),
                        )
                        .clicked()
                    {
                        self.start_background_batch_download();
                    }

                    if ui.button("Export BibTeX (.bib)").clicked() {
                        let out_dir = PathBuf::from(self.download_dir.trim());
                        let _ = std::fs::create_dir_all(&out_dir);
                        let bib_path = out_dir.join("corpus_references.bib");
                        let bib = format_bibtex_corpus(&self.raw_papers);
                        match std::fs::write(&bib_path, bib) {
                            Ok(_) => {
                                self.status_banner =
                                    format!("Saved BibTeX library to {:?}", bib_path);
                            }
                            Err(e) => {
                                self.status_banner = format!("BibTeX write failed: {}", e);
                            }
                        }
                    }
                });

                if let Some((done, total)) = self.download_progress {
                    let frac = (done as f32) / (total.max(1) as f32);
                    ui.add_space(6.0);
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .text(format!("Verified {}/{} papers", done, total)),
                    );
                }
            });
        });

        ui.add_space(8.0);
        ui.label(
            RichText::new(format!(
                "PDF Download & Integrity Verification Log ({} entries)",
                self.download_reports.len()
            ))
            .strong(),
        );

        ui.push_id("download_log_scroll", |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("download_reports_grid")
                    .striped(true)
                    .num_columns(6)
                    .show(ui, |ui| {
                        ui.strong("Paper ID");
                        ui.strong("DOI");
                        ui.strong("Stage");
                        ui.strong("Status");
                        ui.strong("Size (KB)");
                        ui.strong("SHA-256 / Saved File");
                        ui.end_row();

                        for r in &self.download_reports {
                            if ui.small_button(&r.paper_id).clicked() {
                                self.focused_paper_id = r.paper_id.clone();
                                self.show_dossier_panel = true;
                            }
                            ui.monospace(if r.doi.is_empty() { "-" } else { &r.doi });
                            ui.monospace(&r.resolved_stage);
                            let st_col = if r.status.is_ok() {
                                Color32::from_rgb(52, 211, 153)
                            } else {
                                Color32::from_rgb(248, 113, 113)
                            };
                            ui.colored_label(st_col, r.status.label());
                            ui.monospace(format!("{:.1}", (r.byte_len as f64) / 1024.0));
                            let info = match (&r.sha256, &r.saved_path) {
                                (Some(sha), Some(p)) => {
                                    format!("{}.. -> {}", &sha[..sha.len().min(10)], p)
                                }
                                _ => "-".to_string(),
                            };
                            ui.monospace(info);
                            ui.end_row();
                        }
                    });
            });
        });
    }

    // ========================================================================
    // DOMAIN 3: CITATION VELOCITY, ACCELERATION & SLEEPING BEAUTIES
    // ========================================================================
    fn ui_velocity_and_awakening(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Trajectory Metric:");
            ui.selectable_value(&mut self.vel_metric, VelocityPlotMetric::DecayVelocity, "v_exp(t)");
            ui.selectable_value(&mut self.vel_metric, VelocityPlotMetric::WindowVelocity, "v_W(t)");
            ui.selectable_value(&mut self.vel_metric, VelocityPlotMetric::Acceleration, "dv/dt");
            ui.selectable_value(&mut self.vel_metric, VelocityPlotMetric::Cumulative, "Cumulative C(t)");
            ui.selectable_value(
                &mut self.vel_metric,
                VelocityPlotMetric::SleepingBeautyGeometry,
                "Ke Sleeping Beauty (c_t vs l_t)",
            );

            ui.separator();
            ui.checkbox(&mut self.show_awakening_quadrant, "Awakening Lag vs Beauty B");
        });

        ui.add_space(4.0);
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.label(RichText::new("Temporal Velocity / Ke Awakening Trajectories").strong());
                self.paint_velocity_curves(ui);
            });
            cols[1].group(|ui| {
                ui.label(
                    RichText::new(if self.show_awakening_quadrant {
                        "Hibernation Span (t_a - pubYear) vs. Ke Beauty Coefficient B"
                    } else {
                        "Velocity v_exp vs. Acceleration dv/dt Phase Portrait"
                    })
                    .strong(),
                );
                self.paint_velocity_phase(ui);
            });
        });

        ui.add_space(6.0);
        ui.label(RichText::new("Velocity, Price Immediacy, Disruption CD & Awakening Ledger").strong());
        let mut rows: Vec<crate::models::PaperMetrics> = self
            .analytics
            .papers
            .iter()
            .filter(|p| self.matches_paper_filter(p))
            .cloned()
            .collect();
        rows.sort_by(|a, b| b.v_exp.partial_cmp(&a.v_exp).unwrap_or(std::cmp::Ordering::Equal));

        ui.push_id("velocity_ledger_scroll", |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                egui::Grid::new("velocity_ledger_grid")
                    .striped(true)
                    .spacing([6.0, 4.0])
                    .num_columns(10)
                    .show(ui, |ui| {
                        ui.strong("Plot");
                        ui.strong("Paper");
                        ui.strong("Regime");
                        ui.strong("Year");
                        ui.strong("v_exp");
                        ui.strong("v_W");
                        ui.strong("dv/dt");
                        ui.strong("5y Immed");
                        ui.strong("Beauty B");
                        ui.strong("Awaken t_a");
                        ui.end_row();

                        for p in &rows {
                            let active = self.compared_ids.contains(&p.record.id);
                            if ui.selectable_label(active, if active { "On" } else { "+" }).clicked() {
                                if active && self.compared_ids.len() > 1 {
                                    self.compared_ids.retain(|x| x != &p.record.id);
                                } else if !active {
                                    if self.compared_ids.len() >= 5 {
                                        self.compared_ids.remove(0);
                                    }
                                    self.compared_ids.push(p.record.id.clone());
                                }
                            }
                            let short_title = if p.record.title.chars().count() > 38 {
                                format!("{}...", p.record.title.chars().take(35).collect::<String>())
                            } else {
                                p.record.title.clone()
                            };
                            if ui.link(short_title).on_hover_text(&p.record.title).clicked() {
                                self.focused_paper_id = p.record.id.clone();
                                self.show_dossier_panel = true;
                            }
                            ui.label(p.regime_tag);
                            ui.monospace(format!("{}", p.record.year));
                            ui.monospace(format!("{:.2}", p.v_exp));
                            ui.monospace(format!("{:.2}", p.v_win));
                            ui.monospace(format!("{:+.2}", p.accel));
                            ui.monospace(format!("{:.0}%", p.immed_5y_pct));
                            ui.monospace(format!("{:.1}", p.beauty_b));
                            ui.monospace(format!("{} (+{}y)", p.awaken_year, p.hibernation_years));
                            ui.end_row();
                        }
                    });
            });
        });
    }

    fn paint_velocity_curves(&self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 240.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::hover());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));

        let pad_l = 40.0;
        let pad_r = 18.0;
        let pad_t = 16.0;
        let pad_b = 24.0;
        let pw = (rect.width() - pad_l - pad_r).max(40.0);
        let ph = (rect.height() - pad_t - pad_b).max(40.0);

        let selected: Vec<&crate::models::PaperMetrics> = self
            .compared_ids
            .iter()
            .filter_map(|id| self.analytics.papers.iter().find(|p| &p.record.id == id))
            .collect();
        if selected.is_empty() {
            return;
        }

        if self.vel_metric == VelocityPlotMetric::SleepingBeautyGeometry {
            let focal = selected[0];
            let age_len = focal.annual_counts.len().max(1);
            let max_c = focal
                .annual_counts
                .iter()
                .chain(focal.ke_ref_line.iter())
                .copied()
                .fold(1.5f64, f64::max) as f32
                * 1.15;

            let bw = ((pw / (age_len as f32)) * 0.65).clamp(2.5, 12.0);
            for (a, &c_val) in focal.annual_counts.iter().enumerate() {
                let x = rect.left() + pad_l + ((a as f32) / ((age_len - 1).max(1) as f32)) * pw;
                let bh = ((c_val as f32) / max_c) * ph;
                let col = if focal.record.year + (a as i32) == focal.awaken_year {
                    Color32::from_rgb(244, 63, 94)
                } else {
                    Color32::from_rgb(56, 189, 248)
                };
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(x - bw * 0.5, rect.top() + pad_t + ph - bh),
                        Pos2::new(x + bw * 0.5, rect.top() + pad_t + ph),
                    ),
                    0.0,
                    col,
                );
            }

            if focal.peak_age_tm > 0 {
                let mut ref_pts = Vec::new();
                for a in 0..=focal.peak_age_tm {
                    let x =
                        rect.left() + pad_l + ((a as f32) / ((age_len - 1).max(1) as f32)) * pw;
                    let y =
                        rect.top() + pad_t + ph - ((focal.ke_ref_line[a] as f32) / max_c) * ph;
                    ref_pts.push(Pos2::new(x, y));
                }
                for pair in ref_pts.windows(2) {
                    painter.line_segment(
                        [pair[0], pair[1]],
                        Stroke::new(2.0, Color32::from_rgb(250, 204, 21)),
                    );
                }
            }
            painter.text(
                Pos2::new(rect.left() + pad_l + 6.0, rect.top() + 14.0),
                Align2::LEFT_CENTER,
                format!(
                    "{} — B = {:.1}, Awakening t_a = {}",
                    focal.record.short_cite(),
                    focal.beauty_b,
                    focal.awaken_year
                ),
                FontId::monospace(11.0),
                Color32::from_rgb(250, 204, 21),
            );
            return;
        }

        let colors = [
            Color32::from_rgb(56, 189, 248),
            Color32::from_rgb(244, 63, 94),
            Color32::from_rgb(52, 211, 153),
            Color32::from_rgb(250, 204, 21),
            Color32::from_rgb(167, 139, 250),
        ];

        let mut min_v = 0.0f32;
        let mut max_v = 0.5f32;
        for p in &selected {
            let series = match self.vel_metric {
                VelocityPlotMetric::DecayVelocity => &p.v_exp_series,
                VelocityPlotMetric::WindowVelocity => &p.v_win_series,
                VelocityPlotMetric::Acceleration => &p.accel_series,
                VelocityPlotMetric::Cumulative => &p.cum_series,
                VelocityPlotMetric::SleepingBeautyGeometry => &p.v_exp_series,
            };
            for &val in series {
                min_v = min_v.min(val as f32);
                max_v = max_v.max(val as f32);
            }
        }
        max_v *= 1.12;
        let span_v = (max_v - min_v).max(1e-5);
        let y_span = self.analytics.year_span.max(2);

        for (s_idx, p) in selected.iter().enumerate() {
            let col = colors[s_idx % colors.len()];
            let series = match self.vel_metric {
                VelocityPlotMetric::DecayVelocity => &p.v_exp_series,
                VelocityPlotMetric::WindowVelocity => &p.v_win_series,
                VelocityPlotMetric::Acceleration => &p.accel_series,
                VelocityPlotMetric::Cumulative => &p.cum_series,
                VelocityPlotMetric::SleepingBeautyGeometry => &p.v_exp_series,
            };
            let mut pts = Vec::new();
            for (y_idx, &val) in series.iter().enumerate() {
                let yr = self.analytics.min_year + (y_idx as i32);
                if yr < p.record.year {
                    continue;
                }
                let x = rect.left() + pad_l + ((y_idx as f32) / ((y_span - 1) as f32)) * pw;
                let y = rect.top() + pad_t + ph - (((val as f32) - min_v) / span_v) * ph;
                pts.push(Pos2::new(x, y));
            }
            for pair in pts.windows(2) {
                painter.line_segment([pair[0], pair[1]], Stroke::new(2.0, col));
            }
        }
    }

    fn paint_velocity_phase(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 240.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::click());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));

        let pad_l = 40.0;
        let pad_r = 18.0;
        let pad_t = 16.0;
        let pad_b = 24.0;
        let pw = (rect.width() - pad_l - pad_r).max(40.0);
        let ph = (rect.height() - pad_t - pad_b).max(40.0);

        let click_pos = if resp.clicked() {
            resp.interact_pointer_pos()
        } else {
            None
        };

        if self.show_awakening_quadrant {
            let max_x = self
                .analytics
                .papers
                .iter()
                .map(|p| p.hibernation_years as f32)
                .fold(5.0f32, f32::max)
                * 1.1;
            let max_y = self
                .analytics
                .papers
                .iter()
                .map(|p| p.beauty_b as f32)
                .fold(5.0f32, f32::max)
                * 1.1;

            for p in &self.analytics.papers {
                let cx = rect.left() + pad_l + ((p.hibernation_years as f32) / max_x) * pw;
                let cy = rect.top() + pad_t + ph - ((p.beauty_b as f32) / max_y) * ph;
                let center = Pos2::new(cx, cy);
                let [r, g, b] = p.record.disc.rgb();
                painter.circle_filled(center, 5.2, Color32::from_rgb(r, g, b));
                if let Some(cp) = click_pos {
                    if cp.distance(center) <= 8.0 {
                        self.focused_paper_id = p.record.id.clone();
                    }
                }
            }
        } else {
            let max_x = self
                .analytics
                .papers
                .iter()
                .map(|p| p.v_exp as f32)
                .fold(0.5f32, f32::max)
                * 1.12;
            let abs_y = self
                .analytics
                .papers
                .iter()
                .map(|p| (p.accel as f32).abs())
                .fold(0.15f32, f32::max)
                * 1.2;
            let zero_y = rect.top() + pad_t + ph * 0.5;
            painter.line_segment(
                [Pos2::new(rect.left() + pad_l, zero_y), Pos2::new(rect.right() - pad_r, zero_y)],
                Stroke::new(1.0, Color32::from_rgb(71, 85, 105)),
            );

            for p in &self.analytics.papers {
                let cx = rect.left() + pad_l + ((p.v_exp as f32) / max_x) * pw;
                let cy = zero_y - ((p.accel as f32) / abs_y) * (ph * 0.5);
                let center = Pos2::new(cx, cy);
                let [r, g, b] = p.record.disc.rgb();
                painter.circle_filled(center, 5.2, Color32::from_rgb(r, g, b));
                if let Some(cp) = click_pos {
                    if cp.distance(center) <= 8.0 {
                        self.focused_paper_id = p.record.id.clone();
                    }
                }
            }
        }
    }

    // ========================================================================
    // DOMAIN 4: BETWEENNESS-CENTRALITY BRIDGE CARTOGRAPHY
    // ========================================================================
    fn ui_bridge_cartography(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Centrality Mode:");
            ui.selectable_value(&mut self.params.bridge_directed, false, "Undirected Geodesic CB_u");
            ui.selectable_value(&mut self.params.bridge_directed, true, "Directed Lineage DAG CB_d");

            ui.separator();
            ui.label("Layout:");
            let mut relayout = false;
            if ui
                .selectable_value(&mut self.layout_mode, GraphLayoutMode::Orbital, "Orbital")
                .clicked()
            {
                relayout = true;
            }
            if ui
                .selectable_value(
                    &mut self.layout_mode,
                    GraphLayoutMode::Chronological,
                    "Chronological",
                )
                .clicked()
            {
                relayout = true;
            }
            if ui
                .selectable_value(&mut self.layout_mode, GraphLayoutMode::ForceDirected, "Force")
                .clicked()
            {
                relayout = true;
            }
            if relayout {
                self.recompute_layout();
            }

            ui.separator();
            ui.label("Modules:");
            ui.selectable_value(&mut self.partition_scheme, PartitionScheme::FixedDiscipline, "Disciplines");
            ui.selectable_value(&mut self.partition_scheme, PartitionScheme::LouvainModularity, "Louvain Q");
            ui.selectable_value(&mut self.partition_scheme, PartitionScheme::InfomapFlow, "Infomap Flow");

            ui.checkbox(&mut self.highlight_conduits, "Conduits");
            if ui.button("Reset Pan/Zoom").clicked() {
                self.net_pan = Vec2::ZERO;
                self.net_zoom = 1.0;
                self.recompute_layout();
            }
        });

        // Shortest Epistemic Path Selector
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Shortest Epistemic Geodesic:").strong());
            egui::ComboBox::from_label("Origin")
                .selected_text(self.path_src_id.clone())
                .show_ui(ui, |ui| {
                    for p in self.analytics.papers.iter().take(250) {
                        ui.selectable_value(
                            &mut self.path_src_id,
                            p.record.id.clone(),
                            format!("[{} {}] {}", p.record.disc.code(), p.record.year, p.record.short_cite()),
                        );
                    }
                });
            egui::ComboBox::from_label("Target")
                .selected_text(self.path_dst_id.clone())
                .show_ui(ui, |ui| {
                    for p in self.analytics.papers.iter().take(250) {
                        ui.selectable_value(
                            &mut self.path_dst_id,
                            p.record.id.clone(),
                            format!("[{} {}] {}", p.record.disc.code(), p.record.year, p.record.short_cite()),
                        );
                    }
                });
            if ui.button("Swap").clicked() {
                std::mem::swap(&mut self.path_src_id, &mut self.path_dst_id);
            }
        });

        let src_idx = self
            .analytics
            .papers
            .iter()
            .position(|p| p.record.id == self.path_src_id);
        let dst_idx = self
            .analytics
            .papers
            .iter()
            .position(|p| p.record.id == self.path_dst_id);
        let path_indices = match (src_idx, dst_idx) {
            (Some(s), Some(t)) => {
                find_shortest_epistemic_path(&self.analytics, s, t, self.params.bridge_directed)
            }
            _ => None,
        };

        if let Some(ref p_vec) = path_indices {
            let chain = p_vec
                .iter()
                .map(|&idx| {
                    let pm = &self.analytics.papers[idx];
                    format!("{} [{}]", pm.record.short_cite(), pm.record.disc.code())
                })
                .collect::<Vec<_>>()
                .join(" -> ");
            ui.label(
                RichText::new(format!(
                    "Geodesic ({} hops): {}",
                    p_vec.len().saturating_sub(1),
                    chain
                ))
                .color(Color32::from_rgb(34, 211, 238))
                .monospace(),
            );
        } else {
            ui.colored_label(
                Color32::from_rgb(250, 204, 21),
                "No directed path between selected pair (switch to Undirected Geodesic CB_u to traverse shared epistemic bridges).",
            );
        }

        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.label(RichText::new("Interactive Citation Network & Bridge Conduits (Drag/Click)").strong());
                self.paint_bridge_network(ui, path_indices.as_deref());
            });
            cols[1].group(|ui| {
                ui.label(RichText::new("Guimera-Amaral (P_i, z_i) Role Cartography").strong());
                self.paint_cartography_plane(ui);
            });
        });
    }

    fn paint_bridge_network(&mut self, ui: &mut egui::Ui, path_indices: Option<&[usize]>) {
        let desired = Vec2::new(ui.available_width(), 340.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::click_and_drag());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(9, 13, 22));

        if resp.dragged() {
            self.net_pan += resp.drag_delta();
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                let factor = if scroll > 0.0 { 1.08 } else { 1.0 / 1.08 };
                self.net_zoom = (self.net_zoom * factor).clamp(0.45, 3.5);
            }
        }

        let center = rect.center() + self.net_pan;
        let scale = rect.width().min(rect.height()) * 0.44 * self.net_zoom;
        let to_screen = |nx: f32, ny: f32| Pos2::new(center.x + nx * scale, center.y + ny * scale);

        let mut path_edges = HashSet::new();
        let mut path_nodes = HashSet::new();
        if let Some(p_slice) = path_indices {
            for &idx in p_slice {
                path_nodes.insert(idx);
            }
            for w in p_slice.windows(2) {
                path_edges.insert((w[0].min(w[1]), w[0].max(w[1])));
            }
        }

        for cond in &self.analytics.conduits {
            let Some(&p_u) = self.node_positions.get(&cond.source_id) else {
                continue;
            };
            let Some(&p_v) = self.node_positions.get(&cond.target_id) else {
                continue;
            };
            let su = to_screen(p_u[0], p_u[1]);
            let sv = to_screen(p_v[0], p_v[1]);
            let on_path = path_edges.contains(&(cond.u.min(cond.v), cond.u.max(cond.v)));

            let stroke = if on_path {
                Stroke::new(2.8, Color32::from_rgb(34, 211, 238))
            } else if self.highlight_conduits && cond.cross_disc {
                Stroke::new(1.5, Color32::from_rgba_unmultiplied(244, 114, 182, 165))
            } else {
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(148, 163, 184, 55))
            };
            painter.line_segment([su, sv], stroke);
        }

        let max_cb = self
            .analytics
            .papers
            .iter()
            .map(|p| {
                if self.params.bridge_directed {
                    p.cb_dir
                } else {
                    p.cb_undir
                }
            })
            .fold(0.02f64, f64::max) as f32;

        let click_pos = if resp.clicked() {
            resp.interact_pointer_pos()
        } else {
            None
        };

        for p in &self.analytics.papers {
            let Some(&np) = self.node_positions.get(&p.record.id) else {
                continue;
            };
            let sc = to_screen(np[0], np[1]);
            let cb_val = if self.params.bridge_directed {
                p.cb_dir as f32
            } else {
                p.cb_undir as f32
            };
            let rad = (4.5 + (cb_val / max_cb) * 10.5) * self.net_zoom.sqrt();
            let [r, g, b] = match self.partition_scheme {
                PartitionScheme::FixedDiscipline => p.record.disc.rgb(),
                PartitionScheme::LouvainModularity => community_rgb(p.louvain_comm),
                PartitionScheme::InfomapFlow => community_rgb(p.infomap_comm),
            };

            if path_nodes.contains(&p.idx) {
                painter.circle_stroke(sc, rad + 3.5, Stroke::new(2.4, Color32::from_rgb(34, 211, 238)));
            }
            painter.circle_filled(sc, rad, Color32::from_rgb(r, g, b));
            if p.record.id == self.focused_paper_id {
                painter.circle_stroke(sc, rad + 1.5, Stroke::new(2.0, Color32::WHITE));
            }
            if cb_val > max_cb * 0.30 || path_nodes.contains(&p.idx) {
                painter.text(
                    Pos2::new(sc.x, sc.y - rad - 6.0),
                    Align2::CENTER_CENTER,
                    p.record.short_cite(),
                    FontId::proportional(10.0),
                    Color32::from_rgb(226, 232, 240),
                );
            }
            if let Some(cp) = click_pos {
                if cp.distance(sc) <= rad + 4.0 {
                    self.focused_paper_id = p.record.id.clone();
                    self.show_dossier_panel = true;
                }
            }
        }
    }

    fn paint_cartography_plane(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 340.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::click());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(9, 13, 22));

        let pad_l = 38.0;
        let pad_r = 18.0;
        let pad_t = 18.0;
        let pad_b = 26.0;
        let pw = (rect.width() - pad_l - pad_r).max(40.0);
        let ph = (rect.height() - pad_t - pad_b).max(40.0);

        let min_z = -1.8f32;
        let max_z = self
            .analytics
            .papers
            .iter()
            .map(|p| p.within_z as f32)
            .fold(3.2f32, f32::max)
            + 0.4;
        let hub_cut = if self.analytics.papers.len() >= 120 {
            2.5f32
        } else {
            1.65f32
        };

        let x_of = |p: f32| rect.left() + pad_l + p.clamp(0.0, 1.0) * pw;
        let y_of = |z: f32| rect.top() + pad_t + ph - ((z - min_z) / (max_z - min_z)) * ph;

        let y_hub = y_of(hub_cut);
        painter.line_segment(
            [Pos2::new(rect.left() + pad_l, y_hub), Pos2::new(rect.right() - pad_r, y_hub)],
            Stroke::new(1.0, Color32::from_rgb(71, 85, 105)),
        );
        for p_cut in [0.05f32, 0.62, 0.80] {
            let x = x_of(p_cut);
            painter.line_segment(
                [Pos2::new(x, y_hub), Pos2::new(x, rect.top() + pad_t + ph)],
                Stroke::new(1.0, Color32::from_rgb(51, 65, 85)),
            );
        }

        let click_pos = if resp.clicked() {
            resp.interact_pointer_pos()
        } else {
            None
        };

        for p in &self.analytics.papers {
            let (px_val, pz_val, [r, g, b]) = match self.partition_scheme {
                PartitionScheme::FixedDiscipline => (p.part_p, p.within_z, p.record.disc.rgb()),
                PartitionScheme::LouvainModularity => (
                    p.louvain_part_p,
                    self.analytics.communities.louvain_within_z.get(p.idx).copied().unwrap_or(0.0),
                    community_rgb(p.louvain_comm),
                ),
                PartitionScheme::InfomapFlow => (
                    p.infomap_part_p,
                    self.analytics.communities.infomap_within_z.get(p.idx).copied().unwrap_or(0.0),
                    community_rgb(p.infomap_comm),
                ),
            };
            let center = Pos2::new(x_of(px_val as f32), y_of(pz_val as f32));
            let rad = 4.5 + ((p.cb_undir as f32) * 20.0).min(8.5);
            painter.circle_filled(center, rad, Color32::from_rgb(r, g, b));
            if let Some(cp) = click_pos {
                if cp.distance(center) <= rad + 4.0 {
                    self.focused_paper_id = p.record.id.clone();
                    self.show_dossier_panel = true;
                }
            }
        }
    }

    // ========================================================================
    // DOMAIN 5: LOUVAIN MODULARITY Q & INFOMAP RANDOM-WALK FLOW COMPRESSION
    // ========================================================================
    fn ui_communities(&mut self, ui: &mut egui::Ui) {
        let mut recompute = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Louvain Resolution gamma:").strong());
            if ui
                .add(egui::Slider::new(&mut self.params.louvain_resolution, 0.25..=3.00))
                .changed()
            {
                recompute = true;
            }
            ui.separator();
            ui.label(RichText::new("Infomap Teleportation tau:").strong());
            if ui
                .add(egui::Slider::new(&mut self.params.infomap_teleport, 0.02..=0.50))
                .changed()
            {
                recompute = true;
            }
            ui.separator();
            ui.checkbox(
                &mut self.only_show_reclassified,
                "Filter Ledger to Bridge-Reclassified Papers",
            );
        });
        if recompute {
            self.recompute_all();
        }

        let comm = &self.analytics.communities;
        ui.add_space(4.0);

        // 4 KPI Cards comparing Fixed Disciplines vs Louvain Q vs Infomap L(M)
        ui.columns(4, |cols| {
            cols[0].group(|ui| {
                ui.weak("Louvain Modularity Q(gamma)");
                ui.label(
                    RichText::new(format!(
                        "Q = {:.3} ({} communities)",
                        comm.louvain.modularity_q, comm.louvain.num_communities
                    ))
                    .strong()
                    .size(15.0)
                    .color(Color32::from_rgb(56, 189, 248)),
                );
                ui.label(format!(
                    "{} hierarchy passes | {} shifted vs. Disc",
                    comm.louvain.hierarchy_passes, comm.reclassified_count_louvain
                ));
            });

            cols[1].group(|ui| {
                ui.weak("Infomap Map Equation L(M)");
                ui.label(
                    RichText::new(format!(
                        "{:.3} bits ({} modules)",
                        comm.infomap.codelength_bits, comm.infomap.num_modules
                    ))
                    .strong()
                    .size(15.0)
                    .color(Color32::from_rgb(52, 211, 153)),
                );
                ui.label(format!(
                    "{:+.1}% vs 1-level {:.2}b (Index {:.2}b + Mod {:.2}b)",
                    -comm.infomap.relative_compression_pct,
                    comm.infomap.one_level_codelength_bits,
                    comm.infomap.index_codelength_bits,
                    comm.infomap.module_codelength_bits
                ));
            });

            cols[2].group(|ui| {
                ui.weak("Disciplines vs. Louvain Q");
                ui.label(
                    RichText::new(format!(
                        "NMI = {:.3} | ARI = {:.3}",
                        comm.disc_vs_louvain.nmi, comm.disc_vs_louvain.adjusted_rand_index
                    ))
                    .strong()
                    .size(15.0)
                    .color(Color32::from_rgb(167, 139, 250)),
                );
                ui.label(format!(
                    "MI = {:.3}b | VI = {:.3}b",
                    comm.disc_vs_louvain.mi_bits, comm.disc_vs_louvain.vi_bits
                ));
            });

            cols[3].group(|ui| {
                ui.weak("Disciplines vs. Infomap (Louvain<->Infomap)");
                ui.label(
                    RichText::new(format!(
                        "NMI = {:.3} | ARI = {:.3}",
                        comm.disc_vs_infomap.nmi, comm.disc_vs_infomap.adjusted_rand_index
                    ))
                    .strong()
                    .size(15.0)
                    .color(Color32::from_rgb(250, 204, 21)),
                );
                ui.label(format!(
                    "Louvain vs Infomap NMI = {:.3} (ARI = {:.3})",
                    comm.louvain_vs_infomap.nmi, comm.louvain_vs_infomap.adjusted_rand_index
                ));
            });
        });

        ui.add_space(6.0);

        // Dual Contingency Heatmaps: Fixed Disciplines x Louvain & Fixed Disciplines x Infomap
        let louvain_mat = comm.louvain.contingency_disc_by_comm.clone();
        let infomap_mat = comm.infomap.contingency_disc_by_mod.clone();
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.label(
                    RichText::new("Contingency Matrix: Fixed Disciplines (cols) x Louvain L# (rows)")
                        .strong(),
                );
                Self::paint_contingency_heatmap(ui, &louvain_mat, "L");
            });
            cols[1].group(|ui| {
                ui.label(
                    RichText::new("Contingency Matrix: Fixed Disciplines (cols) x Infomap M# (rows)")
                        .strong(),
                );
                Self::paint_contingency_heatmap(ui, &infomap_mat, "M");
            });
        });

        ui.add_space(6.0);

        // Bottom Split: Left = Discovered Modules Summary, Right = Paper-Level Partition & Boundary-Spanner Ledger
        let louvain_clusters = comm.louvain.clusters.clone();
        let infomap_clusters = comm.infomap.clusters.clone();
        let mut paper_rows: Vec<crate::models::PaperMetrics> = self
            .analytics
            .papers
            .iter()
            .filter(|p| {
                self.matches_paper_filter(p)
                    && (!self.only_show_reclassified
                        || p.reclassified_louvain
                        || p.reclassified_infomap)
            })
            .cloned()
            .collect();
        paper_rows.sort_by(|a, b| {
            b.infomap_flow
                .partial_cmp(&a.infomap_flow)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut next_focus: Option<String> = None;
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.label(RichText::new("Discovered Louvain & Infomap Cluster Summary").strong());
                ui.push_id("comm_clusters_scroll", |ui| {
                    egui::ScrollArea::vertical().max_height(215.0).show(ui, |ui| {
                        egui::Grid::new("comm_clusters_grid")
                            .striped(true)
                            .num_columns(7)
                            .show(ui, |ui| {
                                ui.strong("Module");
                                ui.strong("Size");
                                ui.strong("Int E");
                                ui.strong("Cut E");
                                ui.strong("Flow %");
                                ui.strong("Dom Disc");
                                ui.strong("Top Bridge");
                                ui.end_row();

                                for c in louvain_clusters.iter().chain(infomap_clusters.iter()) {
                                    let [r, g, b] = community_rgb(c.cluster_id);
                                    ui.colored_label(Color32::from_rgb(r, g, b), &c.code);
                                    ui.monospace(format!("{}", c.size));
                                    ui.monospace(format!("{}", c.internal_edges));
                                    ui.monospace(format!("{}", c.cut_edges));
                                    ui.monospace(format!("{:.1}%", c.flow_mass * 100.0));
                                    ui.monospace(format!(
                                        "{} ({:.0}%)",
                                        c.dominant_disc.code(),
                                        c.dominant_purity * 100.0
                                    ));
                                    if ui.small_button(&c.top_member_id).clicked() {
                                        next_focus = Some(c.top_member_id.clone());
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                });
            });

            cols[1].group(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Paper Partition & Boundary-Spanner Ledger ({} shown)",
                        paper_rows.len()
                    ))
                    .strong(),
                );
                ui.push_id("comm_papers_scroll", |ui| {
                    egui::ScrollArea::vertical().max_height(215.0).show(ui, |ui| {
                        egui::Grid::new("comm_papers_grid")
                            .striped(true)
                            .num_columns(6)
                            .show(ui, |ui| {
                                ui.strong("Paper");
                                ui.strong("Disc");
                                ui.strong("Louvain");
                                ui.strong("Infomap");
                                ui.strong("Flow %");
                                ui.strong("Alignment");
                                ui.end_row();

                                for p in &paper_rows {
                                    if ui.link(p.record.short_cite()).clicked() {
                                        next_focus = Some(p.record.id.clone());
                                    }
                                    let [dr, dg, db] = p.record.disc.rgb();
                                    ui.colored_label(Color32::from_rgb(dr, dg, db), p.record.disc.code());
                                    let [lr, lg, lb] = community_rgb(p.louvain_comm);
                                    ui.colored_label(
                                        Color32::from_rgb(lr, lg, lb),
                                        format!("L{}", p.louvain_comm + 1),
                                    );
                                    let [mr, mg, mb] = community_rgb(p.infomap_comm);
                                    ui.colored_label(
                                        Color32::from_rgb(mr, mg, mb),
                                        format!("M{}", p.infomap_comm + 1),
                                    );
                                    ui.monospace(format!("{:.2}%", p.infomap_flow * 100.0));
                                    let tag = match (p.reclassified_louvain, p.reclassified_infomap) {
                                        (true, true) => "Cross-Field (L+M)",
                                        (true, false) => "Cross-Field (L)",
                                        (false, true) => "Cross-Field (M)",
                                        (false, false) => "Core Aligned",
                                    };
                                    ui.monospace(tag);
                                    ui.end_row();
                                }
                            });
                    });
                });
            });
        });

        if let Some(fid) = next_focus {
            self.focused_paper_id = fid;
            self.show_dossier_panel = true;
        }
    }

    fn paint_contingency_heatmap(ui: &mut egui::Ui, matrix: &[[usize; 6]], row_prefix: &str) {
        let rows = matrix.len().min(10);
        let desired = Vec2::new(ui.available_width(), 195.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::hover());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));
        if rows == 0 {
            return;
        }

        let max_cell = matrix
            .iter()
            .take(rows)
            .flat_map(|r| r.iter())
            .copied()
            .max()
            .unwrap_or(1)
            .max(1) as f32;

        let pad_l = 38.0;
        let pad_t = 22.0;
        let cw = ((rect.width() - pad_l - 10.0) / 6.0).max(20.0);
        let ch = ((rect.height() - pad_t - 8.0) / (rows as f32)).max(14.0);

        for (d_idx, d) in Discipline::ALL.iter().enumerate() {
            let [r, g, b] = d.rgb();
            painter.text(
                Pos2::new(rect.left() + pad_l + cw * (d_idx as f32 + 0.5), rect.top() + 11.0),
                Align2::CENTER_CENTER,
                d.code(),
                FontId::monospace(10.0),
                Color32::from_rgb(r, g, b),
            );
        }

        for r_idx in 0..rows {
            let [cr, cg, cb] = community_rgb(r_idx);
            let y = rect.top() + pad_t + (r_idx as f32) * ch;
            painter.text(
                Pos2::new(rect.left() + pad_l - 6.0, y + ch * 0.5),
                Align2::RIGHT_CENTER,
                format!("{}{}", row_prefix, r_idx + 1),
                FontId::monospace(9.5),
                Color32::from_rgb(cr, cg, cb),
            );
            for d_idx in 0..6 {
                let cnt = matrix[r_idx][d_idx];
                let x = rect.left() + pad_l + (d_idx as f32) * cw;
                let cell = Rect::from_min_max(
                    Pos2::new(x + 1.0, y + 1.0),
                    Pos2::new(x + cw - 1.0, y + ch - 1.0),
                );
                let fill = if cnt == 0 {
                    Color32::from_rgb(17, 24, 39)
                } else {
                    let alpha = (45.0 + ((cnt as f32) / max_cell) * 195.0) as u8;
                    Color32::from_rgba_unmultiplied(56, 189, 248, alpha)
                };
                painter.rect_filled(cell, 2.0, fill);
                if cnt > 0 {
                    painter.text(
                        cell.center(),
                        Align2::CENTER_CENTER,
                        format!("{}", cnt),
                        FontId::monospace(10.0),
                        Color32::WHITE,
                    );
                }
            }
        }
    }

    // ========================================================================
    // DOMAIN 5: MUTUAL-INFORMATION & JSD TOPIC DRIFT
    // ========================================================================
    fn ui_topic_drift(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Window:");
            if !self.analytics.windows.is_empty() {
                let cur_lbl = self.analytics.windows[self.selected_window_idx].label.clone();
                egui::ComboBox::from_label("Active Window")
                    .selected_text(cur_lbl)
                    .show_ui(ui, |ui| {
                        for (idx, w) in self.analytics.windows.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.selected_window_idx,
                                idx,
                                format!("w_{}: {} (NMI={:.3})", idx + 1, w.label, w.nmi),
                            );
                        }
                    });
            }
            ui.label("Dirichlet alpha:");
            if ui
                .add(egui::Slider::new(&mut self.params.smoothing_alpha, 0.01..=0.50))
                .changed()
            {
                changed = true;
            }
            ui.label("Citation Discourse wt:");
            if ui
                .add(egui::Slider::new(&mut self.params.discourse_weight, 0.0..=1.0))
                .changed()
            {
                changed = true;
            }
            ui.checkbox(&mut self.jsd_delta_mode, "Show Net Convergence Delta JSD(w_k) - JSD(w_1)");
        });
        if changed {
            self.recompute_all();
        }

        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.label(
                    RichText::new("Multi-Window Shannon I_w(D;T), NMI_w(D;T) & Within-Field JSD")
                        .strong(),
                );
                self.paint_mi_trajectory(ui);
            });
            cols[1].group(|ui| {
                ui.label(
                    RichText::new("Pairwise Inter-Discipline JSD Matrix (Click Cell)")
                        .strong(),
                );
                self.paint_jsd_matrix(ui);
            });
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("PMI_w(d, t) Concept Migration Across Windows:").strong());
            egui::ComboBox::from_label("Concept Cluster")
                .selected_text(CONCEPTS[self.selected_concept_idx].label)
                .show_ui(ui, |ui| {
                    for (idx, c) in CONCEPTS.iter().enumerate() {
                        ui.selectable_value(&mut self.selected_concept_idx, idx, c.label);
                    }
                });
        });
        self.paint_pmi_heatmap(ui);
    }

    fn paint_mi_trajectory(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 220.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::click());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));

        let wins = &self.analytics.windows;
        if wins.is_empty() {
            return;
        }
        let pad_l = 36.0;
        let pad_r = 24.0;
        let pad_t = 16.0;
        let pad_b = 24.0;
        let pw = (rect.width() - pad_l - pad_r).max(40.0);
        let ph = (rect.height() - pad_t - pad_b).max(40.0);

        let x_of = |idx: usize| {
            if wins.len() == 1 {
                rect.left() + pad_l + pw * 0.5
            } else {
                rect.left() + pad_l + ((idx as f32) / ((wins.len() - 1) as f32)) * pw
            }
        };
        let y_of = |val: f32| rect.top() + pad_t + ph * (1.0 - val.clamp(0.0, 1.0));

        // Discipline within-drift lines
        for d in Discipline::ALL {
            let [r, g, b] = d.rgb();
            let col = Color32::from_rgba_unmultiplied(r, g, b, 150);
            let pts: Vec<Pos2> = wins
                .iter()
                .enumerate()
                .map(|(i, w)| Pos2::new(x_of(i), y_of(w.within_drift[d.index()] as f32)))
                .collect();
            for pair in pts.windows(2) {
                painter.line_segment([pair[0], pair[1]], Stroke::new(1.3, col));
            }
        }

        // Solid emerald NMI curve
        let nmi_pts: Vec<Pos2> = wins
            .iter()
            .enumerate()
            .map(|(i, w)| Pos2::new(x_of(i), y_of(w.nmi as f32)))
            .collect();
        for pair in nmi_pts.windows(2) {
            painter.line_segment(
                [pair[0], pair[1]],
                Stroke::new(2.5, Color32::from_rgb(16, 185, 129)),
            );
        }
        for (i, &pt) in nmi_pts.iter().enumerate() {
            let rad = if i == self.selected_window_idx { 5.0 } else { 3.2 };
            painter.circle_filled(pt, rad, Color32::from_rgb(16, 185, 129));
        }

        if resp.clicked() {
            if let Some(cp) = resp.interact_pointer_pos() {
                if let Some((best_i, _)) = nmi_pts
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        (cp.x - a.1.x)
                            .abs()
                            .partial_cmp(&(cp.x - b.1.x).abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                {
                    self.selected_window_idx = best_i;
                }
            }
        }
    }

    fn paint_jsd_matrix(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 220.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::click());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));

        let wins = &self.analytics.windows;
        if wins.is_empty() {
            return;
        }
        let cur = &wins[self.selected_window_idx];
        let base = &wins[0];

        let pad_l = 42.0;
        let pad_t = 24.0;
        let cw = ((rect.width() - pad_l - 12.0) / 6.0).max(20.0);
        let ch = ((rect.height() - pad_t - 10.0) / 6.0).max(18.0);

        let click_pos = if resp.clicked() {
            resp.interact_pointer_pos()
        } else {
            None
        };

        for r in 0..6 {
            let d_row = Discipline::ALL[r];
            let [rr, rg, rb] = d_row.rgb();
            painter.text(
                Pos2::new(rect.left() + pad_l + cw * (r as f32 + 0.5), rect.top() + 11.0),
                Align2::CENTER_CENTER,
                d_row.code(),
                FontId::monospace(10.5),
                Color32::from_rgb(rr, rg, rb),
            );
            painter.text(
                Pos2::new(rect.left() + pad_l - 6.0, rect.top() + pad_t + ch * (r as f32 + 0.5)),
                Align2::RIGHT_CENTER,
                d_row.code(),
                FontId::monospace(10.5),
                Color32::from_rgb(rr, rg, rb),
            );

            for c in 0..6 {
                let x = rect.left() + pad_l + (c as f32) * cw;
                let y = rect.top() + pad_t + (r as f32) * ch;
                let cell = Rect::from_min_max(Pos2::new(x + 1.0, y + 1.0), Pos2::new(x + cw - 1.0, y + ch - 1.0));
                let val = if self.jsd_delta_mode {
                    cur.jsd_matrix[r][c] - base.jsd_matrix[r][c]
                } else {
                    cur.jsd_matrix[r][c]
                };

                let fill = if r == c {
                    Color32::from_rgb(30, 41, 59)
                } else if self.jsd_delta_mode {
                    let mag = ((val.abs() / 0.45).min(1.0) * 180.0) as u8 + 30;
                    if val < 0.0 {
                        Color32::from_rgba_unmultiplied(16, 185, 129, mag)
                    } else {
                        Color32::from_rgba_unmultiplied(244, 63, 94, mag)
                    }
                } else {
                    let norm = (val / 0.95).clamp(0.0, 1.0);
                    Color32::from_rgb(
                        (18.0 + norm * 105.0) as u8,
                        (155.0 - norm * 95.0) as u8,
                        (185.0 + norm * 50.0) as u8,
                    )
                };
                painter.rect_filled(cell, 2.0, fill);
                painter.text(
                    cell.center(),
                    Align2::CENTER_CENTER,
                    if r == c {
                        "0.00".to_string()
                    } else if self.jsd_delta_mode {
                        format!("{:+.2}", val)
                    } else {
                        format!("{:.2}", val)
                    },
                    FontId::monospace(10.0),
                    Color32::WHITE,
                );

                if let Some(cp) = click_pos {
                    if r != c && cell.contains(cp) {
                        self.selected_pair = (Discipline::ALL[r], Discipline::ALL[c]);
                    }
                }
            }
        }
    }

    fn paint_pmi_heatmap(&self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width(), 175.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::hover());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 15, 26));

        let wins = &self.analytics.windows;
        if wins.is_empty() {
            return;
        }
        let pad_l = 44.0;
        let pad_b = 22.0;
        let cw = ((rect.width() - pad_l - 12.0) / (wins.len() as f32)).max(16.0);
        let ch = ((rect.height() - pad_b - 8.0) / 6.0).max(16.0);

        for (d_idx, d) in Discipline::ALL.iter().enumerate() {
            let [r, g, b] = d.rgb();
            let y = rect.top() + 4.0 + (d_idx as f32) * ch;
            painter.text(
                Pos2::new(rect.left() + pad_l - 6.0, y + ch * 0.5),
                Align2::RIGHT_CENTER,
                d.code(),
                FontId::monospace(10.0),
                Color32::from_rgb(r, g, b),
            );
            for (w_idx, w) in wins.iter().enumerate() {
                let x = rect.left() + pad_l + (w_idx as f32) * cw;
                let cell = Rect::from_min_max(Pos2::new(x + 1.0, y + 1.0), Pos2::new(x + cw - 1.0, y + ch - 1.0));
                let val = w.pmi[d_idx][self.selected_concept_idx];
                let alpha = (((val.abs() / 2.2).min(1.0) * 185.0) as u8).max(25);
                let fill = if val >= 0.0 {
                    Color32::from_rgba_unmultiplied(56, 189, 248, alpha)
                } else {
                    Color32::from_rgba_unmultiplied(71, 85, 105, alpha / 2)
                };
                painter.rect_filled(cell, 2.0, fill);
                if cw >= 36.0 {
                    painter.text(
                        cell.center(),
                        Align2::CENTER_CENTER,
                        format!("{:+.1}", val),
                        FontId::monospace(9.5),
                        Color32::WHITE,
                    );
                }
            }
        }
    }

    // ========================================================================
    // DOMAIN 6: SQLITE LEDGER, EXPORTS & SYNTHETIC BENCHMARK LAB
    // ========================================================================
    fn ui_ledger_and_lab(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.heading("WAL SQLite Ledger & File Import/Export");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("SQLite DB Path:");
                    ui.add(egui::TextEdit::singleline(&mut self.db_path_input).desired_width(200.0));
                });

                ui.horizontal_wrapped(|ui| {
                    if ui.button("Save Corpus to SQLite").clicked() {
                        match LedgerStore::open(std::path::Path::new(&self.db_path_input))
                            .and_then(|mut s| s.save_corpus(&self.raw_papers, &self.raw_edges, true))
                        {
                            Ok(_) => {
                                self.status_banner =
                                    format!("Saved {} papers to {}", self.raw_papers.len(), self.db_path_input);
                            }
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Load Corpus from SQLite").clicked() {
                        match LedgerStore::open(std::path::Path::new(&self.db_path_input))
                            .and_then(|s| s.load_corpus())
                        {
                            Ok((p, e)) if !p.is_empty() => {
                                self.raw_papers = p;
                                self.raw_edges = e;
                                self.corpus_label = format!("SQLite Ledger ({})", self.db_path_input);
                                self.recompute_all();
                                self.status_banner = "Loaded corpus from SQLite ledger.".to_string();
                            }
                            Ok(_) => self.status_banner = "SQLite database contains 0 papers.".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Restore 47-Paper Empirical Benchmark").clicked() {
                        let (p, e) = build_empirical_corpus();
                        self.raw_papers = p;
                        self.raw_edges = e;
                        self.corpus_label =
                            "Empirical Landmark Corpus (47 Verified Works, 1948-2026)".to_string();
                        self.recompute_all();
                        self.status_banner = "Restored verified empirical benchmark corpus.".to_string();
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Import .csv/.bib/.json/.graphml/.gexf:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.import_path_input)
                            .desired_width(185.0)
                            .hint_text("papers.csv, scholar.html, refs.bib"),
                    );
                    if ui.button("Import File").clicked() {
                        match import_corpus_file(std::path::Path::new(self.import_path_input.trim())) {
                            Ok((p, e)) => {
                                self.raw_papers = p;
                                self.raw_edges = e;
                                self.corpus_label = format!("Imported ({})", self.import_path_input);
                                self.recompute_all();
                                self.status_banner = "Imported external corpus file (.csv/.bib/.json/.html).".to_string();
                            }
                            Err(err) => self.status_banner = err,
                        }
                    }
                });

                ui.separator();
                ui.label(RichText::new("RFC 4180 Multi-Table CSV Export Suite & Bundles").strong());
                ui.horizontal(|ui| {
                    ui.label("CSV Suite Folder:");
                    ui.add(egui::TextEdit::singleline(&mut self.csv_export_dir_input).desired_width(180.0));
                    if ui
                        .button(RichText::new("Export All 10 CSV Tables").strong())
                        .clicked()
                    {
                        let out_d = std::path::Path::new(self.csv_export_dir_input.trim());
                        match export_all_csv_suite(&self.raw_papers, &self.raw_edges, &self.analytics, out_d) {
                            Ok(files) => {
                                self.status_banner = format!(
                                    "Exported {} CSV tables to {}: {}",
                                    files.len(),
                                    self.csv_export_dir_input,
                                    files.join(", ")
                                );
                            }
                            Err(e) => self.status_banner = e,
                        }
                    }
                });

                ui.horizontal_wrapped(|ui| {
                    if ui.button("Papers Metrics CSV").clicked() {
                        let p = std::path::Path::new("papers_metrics.csv");
                        match export_metrics_csv(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported papers_metrics.csv (53 columns incl. Retractions & DataCite)".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Bridges & Retractions CSV").clicked() {
                        let p = std::path::Path::new("artifact_and_retraction_bridges.csv");
                        match export_integrity_and_datacite_csv(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported artifact_and_retraction_bridges.csv".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Bio-Entity Bridges CSV").clicked() {
                        let p = std::path::Path::new("epmc_uniprot_pdb_bridges.csv");
                        match export_bio_entity_bridges_csv(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported epmc_uniprot_pdb_bridges.csv (Europe PMC + UniProt + PDB)".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Crawl Audit CSV").clicked() {
                        let p = std::path::Path::new("crawl_telemetry_audit.csv");
                        let rows = snapshot_global_audit_log();
                        match export_crawl_audit_csv(&rows, p) {
                            Ok(_) => self.status_banner = format!("Exported crawl_telemetry_audit.csv ({} entries)", rows.len()),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Raw Catalog CSV").clicked() {
                        let p = std::path::Path::new("raw_harvest_catalog.csv");
                        match export_raw_papers_csv(&self.raw_papers, p) {
                            Ok(_) => self.status_banner = "Exported raw_harvest_catalog.csv".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Citation Edges CSV").clicked() {
                        let p = std::path::Path::new("citation_edges.csv");
                        match export_edges_csv(&self.raw_papers, &self.raw_edges, &self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported citation_edges.csv".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Annual Curves CSV").clicked() {
                        let p = std::path::Path::new("annual_trajectories.csv");
                        match export_annual_trajectories_csv(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported annual_trajectories.csv".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Topic Drift CSV").clicked() {
                        let p = std::path::Path::new("topic_drift_windows.csv");
                        match export_topic_drift_csv(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported topic_drift_windows.csv".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("JSD Matrix CSV").clicked() {
                        let p = std::path::Path::new("pairwise_jsd_matrix.csv");
                        match export_jsd_matrix_csv(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported pairwise_jsd_matrix.csv".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Communities CSV").clicked() {
                        let p = std::path::Path::new("communities_summary.csv");
                        match export_communities_csv(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported communities_summary.csv".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Export GraphML (.graphml)").clicked() {
                        let p = std::path::Path::new("cit_atlas_network.graphml");
                        match export_graphml(
                            &self.raw_papers,
                            &self.raw_edges,
                            &self.analytics,
                            Some(&self.graph_viewer.positions),
                            self.graph_viewer.color_by,
                            self.graph_viewer.show_datacite_diamonds || self.graph_viewer.show_bio_entity_nodes,
                            p,
                        ) {
                            Ok(_) => self.status_banner = "Exported cit_atlas_network.graphml (with layout & attributes)".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Export GEXF 1.3 (.gexf)").clicked() {
                        let p = std::path::Path::new("cit_atlas_network.gexf");
                        match export_gexf(
                            &self.raw_papers,
                            &self.raw_edges,
                            &self.analytics,
                            Some(&self.graph_viewer.positions),
                            self.graph_viewer.color_by,
                            self.graph_viewer.show_datacite_diamonds || self.graph_viewer.show_bio_entity_nodes,
                            p,
                        ) {
                            Ok(_) => self.status_banner = "Exported cit_atlas_network.gexf (Gephi 1.3 viz colors & coords)".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Graph JSON").clicked() {
                        let p = std::path::Path::new("cit_atlas_bundle.json");
                        match export_bundle_json(&self.raw_papers, &self.raw_edges, &self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported cit_atlas_bundle.json".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                    if ui.button("Synthesis SVG").clicked() {
                        let p = std::path::Path::new("cit_atlas_synthesis.svg");
                        match export_synthesis_svg(&self.analytics, p) {
                            Ok(_) => self.status_banner = "Exported cit_atlas_synthesis.svg".to_string(),
                            Err(e) => self.status_banner = e,
                        }
                    }
                });
            });

            cols[1].group(|ui| {
                ui.heading("Parametric Synthetic Multi-Decade Drift Generator");
                ui.weak("Seeded Barabasi-Albert preferential attachment + exponential aging + controlled cross-disciplinary concept drift.");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Paper Count N:");
                    ui.add(egui::Slider::new(&mut self.syn_nodes, 40..=100_000).logarithmic(true));
                });
                ui.horizontal(|ui| {
                    ui.label("Cross-Field Bridge Rate beta:");
                    ui.add(egui::Slider::new(&mut self.syn_beta, 0.02..=0.55));
                });
                ui.horizontal(|ui| {
                    ui.label("PRNG Seed:");
                    ui.add(egui::DragValue::new(&mut self.syn_seed));
                    ui.selectable_value(&mut self.syn_regime, SyntheticRegime::Convergent, "Convergent");
                    ui.selectable_value(&mut self.syn_regime, SyntheticRegime::Awakening, "Awakening");
                    ui.selectable_value(&mut self.syn_regime, SyntheticRegime::Siloed, "Siloed");
                });
                if ui.button("Generate Synthetic Benchmark Graph").clicked() {
                    let (p, e) = generate_synthetic_corpus(
                        self.syn_nodes,
                        self.syn_seed,
                        self.syn_beta,
                        self.syn_regime,
                    );
                    self.raw_papers = p;
                    self.raw_edges = e;
                    self.corpus_label = format!(
                        "Synthetic Simulation (N={}, regime={})",
                        self.syn_nodes,
                        self.syn_regime.as_str()
                    );
                    self.recompute_all();
                    self.status_banner = "Generated synthetic multi-decade benchmark graph.".to_string();
                }

                ui.separator();
                ui.label(RichText::new("Plain-Text Scientometric Formulations").strong());
                ui.monospace("v_exp(i, t) = sum_{k=t0..t} c_i(k) * exp(-ln(2)*(t - k)/tau)");
                ui.monospace("B_i = sum_{t=0..t_m} (l_t - c_t) / max(1, c_t)  [Ke et al. 2015]");
                ui.monospace("CB(v) = sum_{s!=v!=t} sigma_st(v) / sigma_st    [Brandes 2001]");
                ui.monospace("P_i = 1 - sum_{s in D} (k_is / k_i)^2           [Guimera-Amaral 2005]");
                ui.monospace("I_w(D; T) = sum_{d,t} P_w(d,t) * log2(P_w(d,t) / (P_w(d)*P_w(t)))");
                ui.monospace("JSD(P || Q) = 0.5*KL(P || M) + 0.5*KL(Q || M), M = 0.5*(P + Q)");
            });
        });
    }


    // ========================================================================
    // DOMAIN 9: PER-DOMAIN TOKEN-BUCKET RATE LIMITER & CRAWL TELEMETRY AUDIT
    // ========================================================================
    fn ui_rate_limiter_and_audit(&mut self, ui: &mut egui::Ui) {
        let mut policies = snapshot_global_policies();
        let audit_rows = snapshot_global_audit_log();

        let total_reqs: u64 = policies.iter().map(|p| p.total_requests).sum();
        let total_2xx: u64 = policies.iter().map(|p| p.ok_2xx).sum();
        let total_waits: u64 = policies.iter().map(|p| p.throttled_waits).sum();
        let sum_wait_ms: u64 = policies.iter().map(|p| p.total_wait_ms).sum();
        let total_429: u64 = policies.iter().map(|p| p.http_429_count).sum();
        let total_403: u64 = policies.iter().map(|p| p.http_403_captcha_count).sum();
        let total_robots: u64 = policies.iter().map(|p| p.robots_blocked_count).sum();
        let total_bytes: u64 = policies.iter().map(|p| p.bytes_received).sum();
        let paused_domains = policies
            .iter()
            .filter(|p| p.paused || p.cooldown_remaining_ms > 0)
            .count();

        // Top KPI Cards
        ui.columns(4, |cols| {
            cols[0].group(|ui| {
                ui.weak("Audited Outbound HTTP Calls");
                let ok_pct = if total_reqs > 0 {
                    (total_2xx as f64 / total_reqs as f64) * 100.0
                } else {
                    100.0
                };
                ui.label(
                    RichText::new(format!("{} reqs ({:.1}% 2xx)", total_reqs, ok_pct))
                        .strong()
                        .size(15.5)
                        .color(Color32::from_rgb(52, 211, 153)),
                );
                ui.label(format!(
                    "{:.2} MB payload across {} configured domains",
                    (total_bytes as f64) / (1024.0 * 1024.0),
                    policies.len()
                ));
            });

            cols[1].group(|ui| {
                ui.weak("Token-Bucket Pacing & Queuing");
                ui.label(
                    RichText::new(format!(
                        "{} paced ({:.2}s wait)",
                        total_waits,
                        (sum_wait_ms as f64) / 1000.0
                    ))
                    .strong()
                    .size(15.5)
                    .color(Color32::from_rgb(56, 189, 248)),
                );
                let mean_wait = if total_waits > 0 {
                    (sum_wait_ms as f64) / (total_waits as f64)
                } else {
                    0.0
                };
                ui.label(format!("Mean pacing delay when throttled: {:.0} ms", mean_wait));
            });

            cols[2].group(|ui| {
                ui.weak("Safety Gates & Rate Challenges");
                let blocked = total_429 + total_403 + total_robots;
                let col = if blocked == 0 {
                    Color32::from_rgb(52, 211, 153)
                } else {
                    Color32::from_rgb(250, 204, 21)
                };
                ui.label(
                    RichText::new(format!(
                        "{} robots · {} HTTP 429 · {} 403/CAPTCHA",
                        total_robots, total_429, total_403
                    ))
                    .strong()
                    .size(14.5)
                    .color(col),
                );
                ui.label(format!("{} domain circuit-breakers active/paused", paused_domains));
            });

            cols[3].group(|ui| {
                ui.weak("Global Rate Scaling & Header Adaptation");
                if let Ok(mut guard) = global_limiter_mutex().lock() {
                    ui.horizontal(|ui| {
                        ui.label("Scale:");
                        ui.add(
                            egui::Slider::new(&mut guard.global_rate_scale, 0.20..=3.00)
                                .text("x rps"),
                        );
                    });
                    ui.checkbox(
                        &mut guard.adapt_to_server_headers,
                        "Auto-adapt to Crossref & Retry-After",
                    );
                }
            });
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(
                    RichText::new("Simulate 18-Request Multi-Domain Burst")
                        .color(Color32::from_rgb(56, 189, 248))
                        .strong(),
                )
                .clicked()
            {
                if let Ok(mut g) = global_limiter_mutex().lock() {
                    g.synthesize_demo_audit_burst();
                }
                policies = snapshot_global_policies();
                self.status_banner =
                    "Synthesized 18 multi-domain crawl & API audit events (including OpenAIRE, INSPIRE-HEP, NASA ADS, robots.txt & Retry-After)."
                        .to_string();
            }
            if ui.button("Reset Token Buckets").clicked() {
                if let Ok(mut g) = global_limiter_mutex().lock() {
                    g.reset_all_buckets();
                }
                policies = snapshot_global_policies();
                self.status_banner = "Reset all per-domain token buckets to scholarly defaults.".to_string();
            }
            if ui.button("Clear Audit Log").clicked() {
                if let Ok(mut g) = global_limiter_mutex().lock() {
                    g.clear_audit_log();
                }
                self.status_banner = "Cleared in-memory crawl telemetry audit log.".to_string();
            }
            if ui.button("Save Policies & Audit to SQLite").clicked() {
                let pols = snapshot_global_policies();
                let logs = snapshot_global_audit_log();
                match LedgerStore::open(std::path::Path::new(&self.db_path_input))
                    .and_then(|mut s| s.save_rate_policies_and_audit(&pols, &logs))
                {
                    Ok(_) => {
                        self.status_banner = format!(
                            "Saved {} domain policies and {} audit entries to {}",
                            pols.len(),
                            logs.len(),
                            self.db_path_input
                        );
                    }
                    Err(e) => self.status_banner = e,
                }
            }
            if ui.button("Load Policies & Audit from SQLite").clicked() {
                match LedgerStore::open(std::path::Path::new(&self.db_path_input))
                    .and_then(|s| s.load_rate_policies_and_audit())
                {
                    Ok((pols, logs)) => {
                        if let Ok(mut g) = global_limiter_mutex().lock() {
                            g.restore_persisted_state(&pols, &logs);
                        }
                        policies = snapshot_global_policies();
                        self.status_banner = format!(
                            "Restored {} domain policies and {} audit rows from {}",
                            pols.len(),
                            logs.len(),
                            self.db_path_input
                        );
                    }
                    Err(e) => self.status_banner = e,
                }
            }
            if ui.button("Export Audit CSV").clicked() {
                let p = std::path::Path::new("crawl_telemetry_audit.csv");
                match export_crawl_audit_csv(&audit_rows, p) {
                    Ok(_) => {
                        self.status_banner = format!(
                            "Exported crawl_telemetry_audit.csv ({} rows)",
                            audit_rows.len()
                        );
                    }
                    Err(e) => self.status_banner = e,
                }
            }
            if ui.button("Export Policies CSV").clicked() {
                let p = std::path::Path::new("domain_rate_policies.csv");
                match export_domain_policies_csv(&policies, p) {
                    Ok(_) => {
                        self.status_banner = "Exported domain_rate_policies.csv".to_string();
                    }
                    Err(e) => self.status_banner = e,
                }
            }

            ui.separator();
            ui.label("Add Host:");
            ui.add(
                egui::TextEdit::singleline(&mut self.custom_domain_input)
                    .desired_width(145.0)
                    .hint_text("api.openaire.eu"),
            );
            if ui.button("Add Policy").clicked() && !self.custom_domain_input.trim().is_empty() {
                if let Ok(mut g) = global_limiter_mutex().lock() {
                    g.update_domain_policy(
                        self.custom_domain_input.trim(),
                        3.0,
                        5.0,
                        250,
                        40,
                        true,
                        false,
                    );
                }
                self.custom_domain_input.clear();
                policies = snapshot_global_policies();
            }
        });

        ui.add_space(4.0);
        // Split View: Top Per-Domain Token-Bucket Matrix + Bottom Latency/Wait Timeline & Audit Log
        ui.group(|ui| {
            ui.label(
                RichText::new("Configurable Per-Domain Token-Bucket Policies & Live Fill Gauges")
                    .strong(),
            );
            ui.push_id("domain_bucket_matrix_scroll", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(225.0)
                    .show(ui, |ui| {
                        egui::Grid::new("domain_bucket_policy_grid")
                            .striped(true)
                            .num_columns(11)
                            .show(ui, |ui| {
                                ui.strong("Active");
                                ui.strong("Gate");
                                ui.strong("Domain");
                                ui.strong("Bucket Fill");
                                ui.strong("Refill (rps)");
                                ui.strong("Burst B");
                                ui.strong("Min Floor (ms)");
                                ui.strong("Jitter (ms)");
                                ui.strong("Reqs / 2xx");
                                ui.strong("Waits / 429 / Rob");
                                ui.strong("Pool / Server Header");
                                ui.end_row();

                                for p in &mut policies {
                                    let mut changed = false;
                                    if ui.checkbox(&mut p.enabled, "").changed() {
                                        changed = true;
                                    }
                                    let gate_lbl = if p.paused {
                                        RichText::new("PAUSED").color(Color32::from_rgb(248, 113, 113))
                                    } else if p.cooldown_remaining_ms > 0 {
                                        RichText::new(format!("CD {:.1}s", p.cooldown_remaining_ms as f64 / 1000.0))
                                            .color(Color32::from_rgb(250, 204, 21))
                                    } else {
                                        RichText::new("OPEN").color(Color32::from_rgb(52, 211, 153))
                                    };
                                    if ui.small_button(gate_lbl).clicked() {
                                        p.paused = !p.paused;
                                        changed = true;
                                    }
                                    ui.monospace(&p.domain).on_hover_text(&p.label);

                                    let fill_frac = (p.tokens_available / p.burst_capacity.max(1.0))
                                        .clamp(0.0, 1.0) as f32;
                                    ui.add(
                                        egui::ProgressBar::new(fill_frac)
                                            .desired_width(92.0)
                                            .text(format!("{:.1}/{:.0}", p.tokens_available, p.burst_capacity)),
                                    );

                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut p.refill_rps)
                                                .speed(0.1)
                                                .range(0.05..=100.0),
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut p.burst_capacity)
                                                .speed(0.5)
                                                .range(1.0..=100.0),
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut p.min_spacing_ms)
                                                .speed(25.0)
                                                .range(0..=30_000),
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut p.jitter_ms)
                                                .speed(10.0)
                                                .range(0..=5_000),
                                        )
                                        .changed()
                                    {
                                        changed = true;
                                    }

                                    ui.monospace(format!("{}/{}", p.total_requests, p.ok_2xx));
                                    ui.monospace(format!(
                                        "{}w · {}r · {}b",
                                        p.throttled_waits, p.http_429_count, p.robots_blocked_count
                                    ));
                                    ui.weak(&p.server_pool_note);
                                    ui.end_row();

                                    if changed {
                                        if let Ok(mut g) = global_limiter_mutex().lock() {
                                            g.update_domain_policy(
                                                &p.domain,
                                                p.refill_rps,
                                                p.burst_capacity,
                                                p.min_spacing_ms,
                                                p.jitter_ms,
                                                p.enabled,
                                                p.paused,
                                            );
                                        }
                                    }
                                }
                            });
                    });
            });
        });

        ui.add_space(4.0);
        // Stacked HTTP Latency + Token-Bucket Wait Timeline Chart
        self.paint_audit_latency_timeline(ui, &audit_rows);

        ui.add_space(4.0);
        // Filterable Crawl Telemetry Audit Log Ledger
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "Crawl Telemetry Audit Log ({} recorded events)",
                    audit_rows.len()
                ))
                .strong(),
            );
            ui.separator();
            for (flt, lbl) in [
                (AuditOutcomeFilter::All, "All"),
                (AuditOutcomeFilter::Success2xx, "2xx OK"),
                (AuditOutcomeFilter::ThrottledWaits, "Paced / Waited"),
                (AuditOutcomeFilter::BlockedOr429, "Robots / 429 / 403"),
                (AuditOutcomeFilter::Errors, "Errors"),
            ] {
                ui.selectable_value(&mut self.audit_outcome_filter, flt, lbl);
            }
            ui.separator();
            ui.label("Search Log:");
            ui.add(
                egui::TextEdit::singleline(&mut self.audit_search_query)
                    .desired_width(185.0)
                    .hint_text("Domain, subsystem, DOI, status..."),
            );
        });

        let q_low = self.audit_search_query.trim().to_ascii_lowercase();
        let filtered: Vec<_> = audit_rows
            .iter()
            .rev()
            .filter(|e| {
                let pass_flt = match self.audit_outcome_filter {
                    AuditOutcomeFilter::All => true,
                    AuditOutcomeFilter::Success2xx => e.outcome.is_success(),
                    AuditOutcomeFilter::ThrottledWaits => e.token_wait_ms > 0,
                    AuditOutcomeFilter::BlockedOr429 => e.outcome.is_blocked_or_throttled_429(),
                    AuditOutcomeFilter::Errors => matches!(
                        e.outcome,
                        CrawlAuditOutcome::HttpOtherError | CrawlAuditOutcome::NetworkError
                    ),
                };
                if !pass_flt {
                    return false;
                }
                if q_low.is_empty() {
                    return true;
                }
                e.domain.to_ascii_lowercase().contains(&q_low)
                    || e.subsystem.to_ascii_lowercase().contains(&q_low)
                    || e.url.to_ascii_lowercase().contains(&q_low)
                    || e.note.to_ascii_lowercase().contains(&q_low)
                    || e.outcome.as_str().contains(&q_low)
            })
            .take(300)
            .collect();

        ui.push_id("crawl_audit_ledger_scroll", |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("crawl_audit_ledger_grid")
                    .striped(true)
                    .num_columns(11)
                    .show(ui, |ui| {
                        ui.strong("#");
                        ui.strong("UTC Time");
                        ui.strong("Subsystem");
                        ui.strong("Domain");
                        ui.strong("Outcome");
                        ui.strong("HTTP");
                        ui.strong("Wait (ms)");
                        ui.strong("Latency (ms)");
                        ui.strong("Tokens");
                        ui.strong("Robots");
                        ui.strong("URL & Server Header / Gate Note");
                        ui.end_row();

                        for e in filtered {
                            ui.monospace(format!("{}", e.seq_id));
                            let short_ts = e
                                .timestamp_utc
                                .split('T')
                                .nth(1)
                                .unwrap_or(&e.timestamp_utc);
                            ui.monospace(short_ts);
                            ui.monospace(&e.subsystem);
                            ui.monospace(&e.domain);
                            let [cr, cg, cb] = e.outcome.rgb();
                            ui.colored_label(Color32::from_rgb(cr, cg, cb), e.outcome.as_str());
                            ui.monospace(if e.http_status == 0 {
                                "-".to_string()
                            } else {
                                format!("{}", e.http_status)
                            });
                            ui.monospace(format!("{}", e.token_wait_ms));
                            ui.monospace(format!("{}", e.latency_ms));
                            ui.monospace(format!("{:.2}", e.tokens_remaining));
                            ui.monospace(&e.robots_status);
                            let short_url = if e.url.len() > 64 {
                                format!("{}...", &e.url[..64])
                            } else {
                                e.url.clone()
                            };
                            let combined = if e.note.is_empty() {
                                short_url
                            } else {
                                format!("{}  [{}]", short_url, e.note)
                            };
                            ui.label(RichText::new(combined).size(11.0))
                                .on_hover_text(&e.url);
                            ui.end_row();
                        }
                    });
            });
        });
    }

    fn paint_audit_latency_timeline(
        &self,
        ui: &mut egui::Ui,
        entries: &[crate::rate_limiter::CrawlAuditEntry],
    ) {
        let desired = Vec2::new(ui.available_width(), 92.0);
        let (resp, painter) = ui.allocate_painter(desired, Sense::hover());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::from_rgb(9, 14, 25));
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, Color32::from_rgb(30, 41, 59)),
            egui::StrokeKind::Inside,
        );

        painter.text(
            Pos2::new(rect.left() + 8.0, rect.top() + 10.0),
            Align2::LEFT_CENTER,
            "Recent Request Telemetry Bars: Cyan = HTTP Latency (ms)  |  Amber = Token-Bucket Pacing Wait (ms)  |  Crimson = 429 / Robots / CAPTCHA Gate",
            FontId::monospace(9.8),
            Color32::from_rgb(148, 163, 184),
        );

        if entries.is_empty() {
            painter.text(
                Pos2::new(rect.center().x, rect.center().y + 8.0),
                Align2::CENTER_CENTER,
                "No outbound HTTP calls recorded yet — run a Search/Crawl/Screen or click 'Simulate 15-Request Multi-Domain Burst' above.",
                FontId::proportional(11.5),
                Color32::from_rgb(100, 116, 139),
            );
            return;
        }

        let tail: Vec<_> = entries.iter().rev().take(100).rev().collect();
        let max_total_ms = tail
            .iter()
            .map(|e| (e.latency_ms + e.token_wait_ms).max(100))
            .max()
            .unwrap_or(500) as f32;

        let pad_l = 10.0;
        let pad_r = 10.0;
        let pad_t = 22.0;
        let pad_b = 6.0;
        let plot_w = (rect.width() - pad_l - pad_r).max(40.0);
        let plot_h = (rect.height() - pad_t - pad_b).max(20.0);
        let bar_step = (plot_w / (tail.len().max(1) as f32)).clamp(2.5, 18.0);
        let bar_w = (bar_step - 1.2).max(1.5);
        let base_y = rect.bottom() - pad_b;

        for (i, e) in tail.iter().enumerate() {
            let x = rect.left() + pad_l + (i as f32) * bar_step;
            if e.outcome.is_blocked_or_throttled_429() && e.latency_ms == 0 {
                let b_rect = Rect::from_min_max(
                    Pos2::new(x, base_y - plot_h * 0.65),
                    Pos2::new(x + bar_w, base_y),
                );
                let [cr, cg, cb] = e.outcome.rgb();
                painter.rect_filled(b_rect, 1.5, Color32::from_rgb(cr, cg, cb));
                continue;
            }
            let lat_h = ((e.latency_ms as f32) / max_total_ms * plot_h).clamp(1.5, plot_h);
            let wait_h = ((e.token_wait_ms as f32) / max_total_ms * plot_h)
                .clamp(0.0, (plot_h - lat_h).max(0.0));

            let lat_col = if e.outcome.is_blocked_or_throttled_429() {
                Color32::from_rgb(239, 68, 68)
            } else {
                Color32::from_rgb(56, 189, 248)
            };
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x, base_y - lat_h), Pos2::new(x + bar_w, base_y)),
                1.0,
                lat_col,
            );
            if wait_h > 0.5 {
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(x, base_y - lat_h - wait_h),
                        Pos2::new(x + bar_w, base_y - lat_h),
                    ),
                    1.0,
                    Color32::from_rgb(245, 158, 11),
                );
            }
        }
    }
}

pub fn run_native_gui() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([960.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CitAtlas — Native Citations Analytics & Literature Harvester",
        options,
        Box::new(|cc| Ok(Box::new(CitAtlasApp::new(cc)))),
    )
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};

    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn write_rgba_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if (crc & 1) != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(tag);
        out.extend_from_slice(payload);
        out.extend_from_slice(&crc32(&out[start..]).to_be_bytes());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let row_bytes = width as usize * 4;
    let mut raw = Vec::with_capacity(height as usize * (1 + row_bytes));
    for row in rgba.chunks_exact(row_bytes) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in &raw {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    let mut zlib = Vec::with_capacity(raw.len() + 32);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let chunks: Vec<&[u8]> = if raw.is_empty() { vec![&[]] } else { raw.chunks(65535).collect() };
    for (i, chunk) in chunks.iter().enumerate() {
        zlib.push(if i + 1 == chunks.len() { 1 } else { 0 });
        let len = chunk.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut png = Vec::with_capacity(zlib.len() + 64);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(&mut png, b"IDAT", &zlib);
    push_chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}

pub fn capture_headless_screenshot(
    out_png: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    capture_headless_screenshot_view(out_png, 1)
}

pub fn capture_headless_screenshot_view(
    out_png: &std::path::Path,
    view: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    use eframe::{egui_wgpu, wgpu};

    const W: u32 = 1600;
    const H: u32 = 1000;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok_or("No suitable wgpu adapter found for offscreen capture")?;

    let info = adapter.get_info();
    let adapter_label = format!("wgpu {:?} · {}", info.backend, info.name);

    let (device, queue) = block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("cit_atlas_headless_device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: 8192,
                ..wgpu::Limits::default()
            },
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    renderer
        .callback_resources
        .insert(WgpuGraphPipelineResources::new(&device, format));

    let tex_extent = wgpu::Extent3d {
        width: W,
        height: H,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cit_atlas_offscreen_target"),
        size: tex_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let tex_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bytes_per_row = W * 4;
    let readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cit_atlas_offscreen_readback"),
        size: (bytes_per_row * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(1.0);
    let mut app = CitAtlasApp::new_with_ctx(&ctx, true, adapter_label);
    app.configure_screenshot_view(view);

    for frame_idx in 0..4u32 {
        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(W as f32, H as f32),
            )),
            time: Some(frame_idx as f64 * 0.016),
            predicted_dt: 1.0 / 60.0,
            ..Default::default()
        };
        raw_input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(1.0);

        let full_output = ctx.run(raw_input, |ctx| app.update_ui(ctx));
        let clipped_primitives =
            ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [W, H],
            pixels_per_point: 1.0,
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("cit_atlas_offscreen_encoder"),
        });
        for (id, image_delta) in &full_output.textures_delta.set {
            renderer.update_texture(&device, &queue, *id, image_delta);
        }
        let user_cmds = renderer.update_buffers(
            &device,
            &queue,
            &mut encoder,
            &clipped_primitives,
            &screen_descriptor,
        );

        let bg = ctx.style().visuals.panel_fill.to_normalized_gamma_f32();
        {
            let mut rpass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("cit_atlas_offscreen_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &tex_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: bg[0] as f64,
                                g: bg[1] as f64,
                                b: bg[2] as f64,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                })
                .forget_lifetime();
            renderer.render(&mut rpass, &clipped_primitives, &screen_descriptor);
        }

        if frame_idx == 3 {
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback_buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(H),
                    },
                },
                tex_extent,
            );
        }

        queue.submit(user_cmds.into_iter().chain(std::iter::once(encoder.finish())));
        let _ = device.poll(wgpu::Maintain::Wait);
        for id in &full_output.textures_delta.free {
            renderer.free_texture(id);
        }
    }

    let slice = readback_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    let _ = device.poll(wgpu::Maintain::Wait);
    rx.recv()??;
    let rgba = slice.get_mapped_range().to_vec();
    readback_buf.unmap();

    write_rgba_png(out_png, W, H, &rgba)?;
    println!(
        "Saved offscreen GPU screenshot ({}x{} RGBA) to {}",
        W,
        H,
        out_png.display()
    );
    Ok(())
}
