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

use std::collections::{HashMap, HashSet, VecDeque};

use crate::community_detection::community_rgb;
use crate::models::{
    BioEntityKind, CorpusAnalytics, DataCiteArtifactKind, PaperMetrics, RetractionStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerLayoutPreset {
    ForceAtlas2,
    HierarchicalDag,
    CommunityGalaxies,
    RadialEgo,
    CartographicPlane,
}

impl ViewerLayoutPreset {
    pub fn from_name(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "dag" | "hierarchical" | "chronological" | "timeline" => {
                ViewerLayoutPreset::HierarchicalDag
            }
            "galaxies" | "communities" | "clusters" | "orbital" => {
                ViewerLayoutPreset::CommunityGalaxies
            }
            "ego" | "radial" => ViewerLayoutPreset::RadialEgo,
            "cartographic" | "guimera" | "plane" => ViewerLayoutPreset::CartographicPlane,
            _ => ViewerLayoutPreset::ForceAtlas2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewerLayoutPreset::ForceAtlas2 => "ForceAtlas2 Physics",
            ViewerLayoutPreset::HierarchicalDag => "Chronological DAG",
            ViewerLayoutPreset::CommunityGalaxies => "Community Galaxies",
            ViewerLayoutPreset::RadialEgo => "Radial Ego Rings",
            ViewerLayoutPreset::CartographicPlane => "Cartographic (P, z)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeColorEncoding {
    Discipline,
    LouvainCommunity,
    InfomapModule,
    VelocityVExp,
    SleepingBeautyB,
    IntegrityStatus,
    BioEntityRichness,
}

impl NodeColorEncoding {
    pub fn from_name(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "louvain" | "modularity" => NodeColorEncoding::LouvainCommunity,
            "infomap" | "flow" => NodeColorEncoding::InfomapModule,
            "velocity" | "v_exp" => NodeColorEncoding::VelocityVExp,
            "beauty" | "awakening" | "sleeping_beauty" => NodeColorEncoding::SleepingBeautyB,
            "integrity" | "retraction" => NodeColorEncoding::IntegrityStatus,
            "bio" | "bio_entities" | "uniprot" | "pdb" => NodeColorEncoding::BioEntityRichness,
            _ => NodeColorEncoding::Discipline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSizeEncoding {
    BrandesBetweenness,
    InCitations,
    VelocityVExp,
    SleepingBeautyB,
    InfomapFlow,
    DataCiteDownloads,
    BioEntityBridges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgoSpotlightMode {
    FullGraph,
    OneHopEgo,
    TwoHopEgo,
    UpstreamAncestors,
    DownstreamCiters,
}

#[derive(Debug, Clone)]
pub struct BipartiteArtifactNode {
    pub node_id: String,
    pub artifact_doi: String,
    pub title: String,
    pub kind: DataCiteArtifactKind,
    pub repository: String,
    pub downloads: u64,
    pub citations: u64,
    pub linked_paper_indices: Vec<usize>,
    pub pos: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BioNodeFilter {
    AllBioEntities,
    UniProtAndPdbOnly,
    EpmcOntologyOnly,
}

#[derive(Debug, Clone)]
pub struct BipartiteBioEntityNode {
    pub node_id: String,
    pub entity_id: String,
    pub kind: BioEntityKind,
    pub label: String,
    pub uri: String,
    pub provider: String,
    pub total_mentions: u32,
    pub resolution_angstrom: Option<f32>,
    pub linked_pdb_ids: Vec<String>,
    pub linked_paper_indices: Vec<usize>,
    pub pos: [f32; 2],
}

pub struct GraphViewerState {
    pub layout: ViewerLayoutPreset,
    pub color_by: NodeColorEncoding,
    pub size_by: NodeSizeEncoding,
    pub ego_mode: EgoSpotlightMode,

    pub physics_running: bool,
    pub repulsion_strength: f32,
    pub spring_strength: f32,
    pub gravity_strength: f32,
    pub cluster_cohesion: f32,
    pub damping: f32,

    pub curved_edges: bool,
    pub show_arrows: bool,
    pub show_labels: bool,
    pub label_top_k: usize,
    pub show_minimap: bool,
    pub show_datacite_diamonds: bool,
    pub show_bio_entity_nodes: bool,
    pub bio_filter: BioNodeFilter,
    pub highlight_zombie_edges: bool,
    pub min_edge_eb: f32,

    pub pan: [f32; 2],
    pub zoom: f32,

    pub positions: HashMap<String, [f32; 2]>,
    pub velocities: HashMap<String, [f32; 2]>,
    pub pinned_ids: HashSet<String>,
    pub dragged_node_id: Option<String>,
    pub hovered_node_id: Option<String>,
    pub artifact_nodes: Vec<BipartiteArtifactNode>,
    pub bio_entity_nodes: Vec<BipartiteBioEntityNode>,
    pub bio_cross_edges: Vec<(usize, usize)>,
}

impl Default for GraphViewerState {
    fn default() -> Self {
        Self {
            layout: ViewerLayoutPreset::ForceAtlas2,
            color_by: NodeColorEncoding::Discipline,
            size_by: NodeSizeEncoding::BrandesBetweenness,
            ego_mode: EgoSpotlightMode::FullGraph,
            physics_running: false,
            repulsion_strength: 0.42,
            spring_strength: 0.55,
            gravity_strength: 0.08,
            cluster_cohesion: 0.16,
            damping: 0.82,
            curved_edges: true,
            show_arrows: true,
            show_labels: true,
            label_top_k: 18,
            show_minimap: true,
            show_datacite_diamonds: true,
            show_bio_entity_nodes: true,
            bio_filter: BioNodeFilter::AllBioEntities,
            highlight_zombie_edges: true,
            min_edge_eb: 0.0,
            pan: [0.0, 0.0],
            zoom: 1.0,
            positions: HashMap::new(),
            velocities: HashMap::new(),
            pinned_ids: HashSet::new(),
            dragged_node_id: None,
            hovered_node_id: None,
            artifact_nodes: Vec::new(),
            bio_entity_nodes: Vec::new(),
            bio_cross_edges: Vec::new(),
        }
    }
}

impl GraphViewerState {
    pub fn sync_with_analytics(&mut self, analytics: &CorpusAnalytics, focal_id: &str) {
        let valid_ids: HashSet<&str> = analytics
            .papers
            .iter()
            .map(|p| p.record.id.as_str())
            .collect();
        self.pinned_ids.retain(|id| valid_ids.contains(id.as_str()));

        let needs_full_init = self.positions.len() != analytics.papers.len()
            || analytics
                .papers
                .iter()
                .any(|p| !self.positions.contains_key(&p.record.id));

        if needs_full_init {
            self.recompute_preset_layout(analytics, focal_id);
        } else {
            self.rebuild_artifact_nodes(analytics);
        }
    }

    pub fn recompute_preset_layout(&mut self, analytics: &CorpusAnalytics, focal_id: &str) {
        let coords = compute_preset_positions(analytics, self.layout, self.color_by, focal_id);
        for (id, xy) in coords {
            if !self.pinned_ids.contains(&id) {
                self.positions.insert(id.clone(), xy);
                self.velocities.insert(id, [0.0, 0.0]);
            }
        }
        self.rebuild_artifact_nodes(analytics);
    }

    pub fn rebuild_artifact_nodes(&mut self, analytics: &CorpusAnalytics) {
        let mut map: HashMap<String, BipartiteArtifactNode> = HashMap::new();
        for (idx, p) in analytics.papers.iter().enumerate() {
            for lk in &p.record.datacite_links {
                if lk.artifact_doi.is_empty() {
                    continue;
                }
                let entry = map
                    .entry(lk.artifact_doi.clone())
                    .or_insert_with(|| BipartiteArtifactNode {
                        node_id: format!("datacite:{}", lk.artifact_doi),
                        artifact_doi: lk.artifact_doi.clone(),
                        title: lk.artifact_title.clone(),
                        kind: lk.artifact_kind,
                        repository: lk.repository.clone(),
                        downloads: lk.download_count,
                        citations: lk.citation_count,
                        linked_paper_indices: Vec::new(),
                        pos: [0.0, 0.0],
                    });
                entry.downloads = entry.downloads.max(lk.download_count);
                entry.citations = entry.citations.max(lk.citation_count);
                if !entry.linked_paper_indices.contains(&idx) {
                    entry.linked_paper_indices.push(idx);
                }
            }
        }

        let mut list: Vec<BipartiteArtifactNode> = map.into_values().collect();
        list.sort_by(|a, b| a.artifact_doi.cmp(&b.artifact_doi));

        for (k, art) in list.iter_mut().enumerate() {
            let mut sx = 0.0f32;
            let mut sy = 0.0f32;
            let mut cnt = 0usize;
            for &p_idx in &art.linked_paper_indices {
                let pid = &analytics.papers[p_idx].record.id;
                if let Some(&[px, py]) = self.positions.get(pid) {
                    sx += px;
                    sy += py;
                    cnt += 1;
                }
            }
            if cnt > 0 {
                let offset_ang = (k as f32) * 1.91;
                let radius = if cnt == 1 { 0.09 } else { 0.03 };
                art.pos = [
                    ((sx / (cnt as f32)) + offset_ang.cos() * radius).clamp(-0.94, 0.94),
                    ((sy / (cnt as f32)) + offset_ang.sin() * radius).clamp(-0.94, 0.94),
                ];
            }
        }

        self.artifact_nodes = list;
        self.rebuild_bio_entity_nodes(analytics);
    }

    pub fn rebuild_bio_entity_nodes(&mut self, analytics: &CorpusAnalytics) {
        let mut map: HashMap<(BioEntityKind, String), BipartiteBioEntityNode> = HashMap::new();
        for (idx, p) in analytics.papers.iter().enumerate() {
            for b in &p.record.bio_entities {
                if b.entity_id.is_empty() {
                    continue;
                }
                let matches_filter = match self.bio_filter {
                    BioNodeFilter::AllBioEntities => true,
                    BioNodeFilter::UniProtAndPdbOnly => {
                        matches!(b.kind, BioEntityKind::UniProt | BioEntityKind::PdbStructure)
                    }
                    BioNodeFilter::EpmcOntologyOnly => b.kind.is_epmc_ontology_or_term(),
                };
                if !matches_filter {
                    continue;
                }
                let norm_id = b.entity_id.to_ascii_uppercase();
                let key = (b.kind, norm_id.clone());
                let entry = map.entry(key).or_insert_with(|| BipartiteBioEntityNode {
                    node_id: format!("bio:{}:{}", b.kind.as_str(), norm_id),
                    entity_id: norm_id.clone(),
                    kind: b.kind,
                    label: b.label.clone(),
                    uri: b.uri.clone(),
                    provider: b.provider.clone(),
                    total_mentions: 0,
                    resolution_angstrom: b.resolution_angstrom,
                    linked_pdb_ids: b.linked_pdb_ids.clone(),
                    linked_paper_indices: Vec::new(),
                    pos: [0.0, 0.0],
                });
                entry.total_mentions = entry.total_mentions.saturating_add(b.mention_count.max(1));
                if entry.resolution_angstrom.is_none() && b.resolution_angstrom.is_some() {
                    entry.resolution_angstrom = b.resolution_angstrom;
                }
                for pdb in &b.linked_pdb_ids {
                    let up = pdb.to_ascii_uppercase();
                    if !entry.linked_pdb_ids.contains(&up) {
                        entry.linked_pdb_ids.push(up);
                    }
                }
                if !entry.linked_paper_indices.contains(&idx) {
                    entry.linked_paper_indices.push(idx);
                }
            }
        }

        let mut list: Vec<BipartiteBioEntityNode> = map.into_values().collect();
        list.sort_by(|a, b| {
            (a.kind.as_str(), a.entity_id.as_str()).cmp(&(b.kind.as_str(), b.entity_id.as_str()))
        });

        for (k, bn) in list.iter_mut().enumerate() {
            let mut sx = 0.0f32;
            let mut sy = 0.0f32;
            let mut cnt = 0usize;
            for &p_idx in &bn.linked_paper_indices {
                let pid = &analytics.papers[p_idx].record.id;
                if let Some(&[px, py]) = self.positions.get(pid) {
                    sx += px;
                    sy += py;
                    cnt += 1;
                }
            }
            if cnt > 0 {
                let offset_ang = 0.78 + (k as f32) * 2.399_963;
                let radius = if cnt == 1 { 0.105 } else { 0.042 };
                bn.pos = [
                    ((sx / (cnt as f32)) + offset_ang.cos() * radius).clamp(-0.95, 0.95),
                    ((sy / (cnt as f32)) + offset_ang.sin() * radius).clamp(-0.95, 0.95),
                ];
            }
        }

        // Index PDB nodes so UniProt <-> PDB cross-references connect directly
        let mut pdb_idx_map: HashMap<String, usize> = HashMap::new();
        for (idx, bn) in list.iter().enumerate() {
            if bn.kind == BioEntityKind::PdbStructure {
                pdb_idx_map.insert(bn.entity_id.clone(), idx);
            }
        }
        let mut cross_edges: Vec<(usize, usize)> = Vec::new();
        for (u_idx, bn) in list.iter().enumerate() {
            if bn.kind == BioEntityKind::UniProt {
                for pdb_id in &bn.linked_pdb_ids {
                    if let Some(&p_idx) = pdb_idx_map.get(pdb_id) {
                        cross_edges.push((u_idx, p_idx));
                    }
                }
            }
        }

        self.bio_entity_nodes = list;
        self.bio_cross_edges = cross_edges;
    }

    /// Runs a single interactive ForceAtlas2 / spring-electric relaxation step in `[-0.95, 0.95]^2`.
    pub fn step_physics(&mut self, analytics: &CorpusAnalytics) {
        let n = analytics.papers.len();
        if n < 2 {
            return;
        }

        let mut xs = vec![0.0f32; n];
        let mut ys = vec![0.0f32; n];
        for (i, p) in analytics.papers.iter().enumerate() {
            let [x, y] = self
                .positions
                .get(&p.record.id)
                .copied()
                .unwrap_or([0.0, 0.0]);
            xs[i] = x;
            ys[i] = y;
        }

        let k_ideal = (2.1 / (n as f32)).sqrt();
        let mut fx = vec![0.0f32; n];
        let mut fy = vec![0.0f32; n];

        // 1. Degree-weighted ForceAtlas2 repulsion
        for i in 0..n {
            let deg_i = (analytics.papers[i].undir_deg + 1) as f32;
            for j in (i + 1)..n {
                let deg_j = (analytics.papers[j].undir_deg + 1) as f32;
                let dx = xs[i] - xs[j];
                let dy = ys[i] - ys[j];
                let d2 = dx * dx + dy * dy + 0.0012;
                let d = d2.sqrt();
                let rep = (k_ideal * k_ideal * self.repulsion_strength * (deg_i * deg_j).sqrt() * 0.22) / d2;
                let rx = (dx / d) * rep;
                let ry = (dy / d) * rep;
                fx[i] += rx;
                fy[i] += ry;
                fx[j] -= rx;
                fy[j] -= ry;
            }
        }

        // 2. LinLog / Hooke spring attraction along citation edges
        for &(u, v) in &analytics.active_edges {
            let dx = xs[u] - xs[v];
            let dy = ys[u] - ys[v];
            let d = (dx * dx + dy * dy + 0.0004).sqrt();
            let same_comm = match self.color_by {
                NodeColorEncoding::LouvainCommunity => {
                    analytics.papers[u].louvain_comm == analytics.papers[v].louvain_comm
                }
                NodeColorEncoding::InfomapModule => {
                    analytics.papers[u].infomap_comm == analytics.papers[v].infomap_comm
                }
                _ => analytics.papers[u].record.disc == analytics.papers[v].record.disc,
            };
            let mult = if same_comm {
                1.0 + self.cluster_cohesion
            } else {
                0.72
            };
            let attr = ((d * d) / k_ideal.max(0.05)) * self.spring_strength * mult;
            let ax = (dx / d) * attr;
            let ay = (dy / d) * attr;
            fx[u] -= ax;
            fy[u] -= ay;
            fx[v] += ax;
            fy[v] += ay;
        }

        // 3. Central gravity & community anchor pull + velocity damping
        for i in 0..n {
            let id = &analytics.papers[i].record.id;
            if self.pinned_ids.contains(id) || self.dragged_node_id.as_deref() == Some(id.as_str()) {
                self.velocities.insert(id.clone(), [0.0, 0.0]);
                continue;
            }

            let cluster_idx = match self.color_by {
                NodeColorEncoding::LouvainCommunity => analytics.papers[i].louvain_comm,
                NodeColorEncoding::InfomapModule => analytics.papers[i].infomap_comm,
                _ => analytics.papers[i].record.disc.index(),
            };
            let cluster_total = match self.color_by {
                NodeColorEncoding::LouvainCommunity => {
                    analytics.communities.louvain.num_communities.max(1)
                }
                NodeColorEncoding::InfomapModule => {
                    analytics.communities.infomap.num_modules.max(1)
                }
                _ => 6,
            };
            let ang = (2.0 * std::f32::consts::PI * (cluster_idx as f32)) / (cluster_total as f32)
                - std::f32::consts::FRAC_PI_2;
            let anchor_x = ang.cos() * 0.42;
            let anchor_y = ang.sin() * 0.42;

            fx[i] += (anchor_x - xs[i]) * self.cluster_cohesion * 0.35;
            fy[i] += (anchor_y - ys[i]) * self.cluster_cohesion * 0.35;
            fx[i] -= xs[i] * self.gravity_strength;
            fy[i] -= ys[i] * self.gravity_strength;

            let [ mut vx, mut vy ] = self
                .velocities
                .get(id)
                .copied()
                .unwrap_or([0.0, 0.0]);
            vx = (vx + fx[i] * 0.08) * self.damping;
            vy = (vy + fy[i] * 0.08) * self.damping;

            let speed = (vx * vx + vy * vy).sqrt();
            let max_step = 0.035f32;
            if speed > max_step {
                vx = (vx / speed) * max_step;
                vy = (vy / speed) * max_step;
            }

            let nx = (xs[i] + vx).clamp(-0.92, 0.92);
            let ny = (ys[i] + vy).clamp(-0.92, 0.92);
            self.positions.insert(id.clone(), [nx, ny]);
            self.velocities.insert(id.clone(), [vx, vy]);
        }

        self.rebuild_artifact_nodes(analytics);
    }
}

/// Computes normalized `[-0.92, 0.92]^2` layout coordinates for any preset (used by both GUI & CLI GraphML/GEXF exporters).
pub fn compute_preset_positions(
    analytics: &CorpusAnalytics,
    preset: ViewerLayoutPreset,
    color_by: NodeColorEncoding,
    focal_id: &str,
) -> HashMap<String, [f32; 2]> {
    let mut pos = HashMap::new();
    let n = analytics.papers.len();
    if n == 0 {
        return pos;
    }

    match preset {
        ViewerLayoutPreset::CommunityGalaxies => {
            let num_groups = match color_by {
                NodeColorEncoding::LouvainCommunity => {
                    analytics.communities.louvain.num_communities.max(1)
                }
                NodeColorEncoding::InfomapModule => {
                    analytics.communities.infomap.num_modules.max(1)
                }
                _ => 6,
            };

            for g_idx in 0..num_groups {
                let ang_center = (2.0 * std::f32::consts::PI * (g_idx as f32))
                    / (num_groups as f32)
                    - std::f32::consts::FRAC_PI_2;
                let cx = ang_center.cos() * 0.58;
                let cy = ang_center.sin() * 0.58;

                let mut members: Vec<&PaperMetrics> = analytics
                    .papers
                    .iter()
                    .filter(|p| match color_by {
                        NodeColorEncoding::LouvainCommunity => p.louvain_comm == g_idx,
                        NodeColorEncoding::InfomapModule => p.infomap_comm == g_idx,
                        _ => p.record.disc.index() == g_idx,
                    })
                    .collect();
                members.sort_by(|a, b| {
                    b.cb_undir
                        .partial_cmp(&a.cb_undir)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                for (k, p) in members.iter().enumerate() {
                    let bridge_pull = (p.cb_undir as f32 * 0.72).min(0.34);
                    if k == 0 {
                        pos.insert(
                            p.record.id.clone(),
                            [cx * (1.0 - bridge_pull), cy * (1.0 - bridge_pull)],
                        );
                    } else {
                        let ring = 0.13 + 0.055 * (((k - 1) / 6) as f32);
                        let sub_ang = ang_center + ((k as f32) * 2.399_963);
                        pos.insert(
                            p.record.id.clone(),
                            [
                                ((cx + sub_ang.cos() * ring) * (1.0 - bridge_pull))
                                    .clamp(-0.92, 0.92),
                                ((cy + sub_ang.sin() * ring) * (1.0 - bridge_pull))
                                    .clamp(-0.92, 0.92),
                            ],
                        );
                    }
                }
            }
        }
        ViewerLayoutPreset::HierarchicalDag => {
            let span = (analytics.max_year - analytics.min_year).max(1) as f32;
            // Group papers into chronological buckets and order vertically by discipline + barycenter
            let mut sorted: Vec<&PaperMetrics> = analytics.papers.iter().collect();
            sorted.sort_by_key(|p| (p.record.year, p.record.disc.index(), p.idx));

            let mut by_disc_rank = [0usize; 6];
            let mut disc_totals = [0usize; 6];
            for p in &sorted {
                disc_totals[p.record.disc.index()] += 1;
            }

            for p in sorted {
                let d = p.record.disc.index();
                let k = by_disc_rank[d];
                by_disc_rank[d] += 1;
                let xn = -0.86 + 1.72 * (((p.record.year - analytics.min_year) as f32) / span);
                let base_lane = -0.76 + 1.52 * ((d as f32) / 5.0);
                let wave = (((k % 5) as f32) - 2.0) * 0.036;
                pos.insert(
                    p.record.id.clone(),
                    [xn.clamp(-0.92, 0.92), (base_lane + wave).clamp(-0.92, 0.92)],
                );
            }
        }
        ViewerLayoutPreset::RadialEgo => {
            let root_idx = analytics
                .papers
                .iter()
                .position(|p| p.record.id == focal_id)
                .unwrap_or(0);
            let mut dist = vec![usize::MAX; n];
            let mut q = VecDeque::new();
            dist[root_idx] = 0;
            q.push_back(root_idx);
            while let Some(u) = q.pop_front() {
                for &v in &analytics.undir_adj[u] {
                    if dist[v] == usize::MAX {
                        dist[v] = dist[u] + 1;
                        q.push_back(v);
                    }
                }
            }

            let mut rings: HashMap<usize, Vec<usize>> = HashMap::new();
            for (i, &d) in dist.iter().enumerate() {
                let r_bucket = if d == usize::MAX { 4 } else { d.min(4) };
                rings.entry(r_bucket).or_default().push(i);
            }

            pos.insert(analytics.papers[root_idx].record.id.clone(), [0.0, 0.0]);
            for r_step in 1..=4 {
                let Some(nodes) = rings.get(&r_step) else {
                    continue;
                };
                let radius = 0.21 * (r_step as f32);
                for (k, &idx) in nodes.iter().enumerate() {
                    let ang = (2.0 * std::f32::consts::PI * (k as f32)) / (nodes.len() as f32)
                        - std::f32::consts::FRAC_PI_2;
                    pos.insert(
                        analytics.papers[idx].record.id.clone(),
                        [ang.cos() * radius, ang.sin() * radius],
                    );
                }
            }
        }
        ViewerLayoutPreset::CartographicPlane => {
            let mut min_z = -1.5f32;
            let mut max_z = 2.5f32;
            for p in &analytics.papers {
                min_z = min_z.min(p.within_z as f32);
                max_z = max_z.max(p.within_z as f32);
            }
            let z_span = (max_z - min_z).max(0.5);
            for (k, p) in analytics.papers.iter().enumerate() {
                let part = match color_by {
                    NodeColorEncoding::LouvainCommunity => p.louvain_part_p,
                    NodeColorEncoding::InfomapModule => p.infomap_part_p,
                    _ => p.part_p,
                } as f32;
                let jitter_x = (((k % 7) as f32) - 3.0) * 0.008;
                let jitter_y = ((((k / 7) % 5) as f32) - 2.0) * 0.008;
                let xn = (-0.84 + 1.68 * part.clamp(0.0, 0.95) + jitter_x).clamp(-0.92, 0.92);
                let yn = (0.82 - 1.64 * (((p.within_z as f32) - min_z) / z_span) + jitter_y)
                    .clamp(-0.92, 0.92);
                pos.insert(p.record.id.clone(), [xn, yn]);
            }
        }
        ViewerLayoutPreset::ForceAtlas2 => {
            let seed_map = compute_preset_positions(
                analytics,
                ViewerLayoutPreset::CommunityGalaxies,
                color_by,
                focal_id,
            );
            if n > 600 {
                // Fast O(N + E) 16x16 multipole + CSR spring relaxation seed; live wgpu WGSL compute refines at 60-120 FPS
                let mut xs = vec![0.0f32; n];
                let mut ys = vec![0.0f32; n];
                for (i, p) in analytics.papers.iter().enumerate() {
                    let [sx, sy] = seed_map.get(&p.record.id).copied().unwrap_or([0.0, 0.0]);
                    xs[i] = sx * 0.88;
                    ys[i] = sy * 0.88;
                }
                let k_ideal = (2.2 / (n as f32)).sqrt().max(0.004);
                for iter in 0..18 {
                    let temp = 0.045 * (1.0 - (iter as f32) / 22.0);
                    let mut smass = [0.0f32; 256];
                    let mut scx = [0.0f32; 256];
                    let mut scy = [0.0f32; 256];
                    for i in 0..n {
                        let gx = (((xs[i] + 1.0) * 0.5) * 16.0).floor().clamp(0.0, 15.0) as usize;
                        let gy = (((ys[i] + 1.0) * 0.5) * 16.0).floor().clamp(0.0, 15.0) as usize;
                        let c = gy * 16 + gx;
                        let q = ((analytics.papers[i].undir_deg + 1) as f32).sqrt().min(20.0);
                        smass[c] += q;
                        scx[c] += xs[i] * q;
                        scy[c] += ys[i] * q;
                    }
                    let mut dx = vec![0.0f32; n];
                    let mut dy = vec![0.0f32; n];
                    for i in 0..n {
                        let qi = ((analytics.papers[i].undir_deg + 1) as f32).sqrt().min(20.0);
                        for c in 0..256 {
                            if smass[c] <= 0.0 {
                                continue;
                            }
                            let mx = scx[c] / smass[c];
                            let my = scy[c] / smass[c];
                            let ux = xs[i] - mx;
                            let uy = ys[i] - my;
                            let d2 = ux * ux + uy * uy + 0.008;
                            let rep = (k_ideal * k_ideal * qi * smass[c] * 0.14) / d2;
                            let inv_d = d2.sqrt().recip();
                            dx[i] += ux * inv_d * rep;
                            dy[i] += uy * inv_d * rep;
                        }
                    }
                    for &(u, v) in &analytics.active_edges {
                        let ux = xs[u] - xs[v];
                        let uy = ys[u] - ys[v];
                        let d = (ux * ux + uy * uy + 0.0004).sqrt();
                        let attr = (d / k_ideal) * 0.035;
                        let fx = (ux / d) * attr;
                        let fy = (uy / d) * attr;
                        dx[u] -= fx;
                        dy[u] -= fy;
                        dx[v] += fx;
                        dy[v] += fy;
                    }
                    for i in 0..n {
                        dx[i] -= xs[i] * 0.05;
                        dy[i] -= ys[i] * 0.05;
                        let mag = (dx[i] * dx[i] + dy[i] * dy[i]).sqrt().max(1e-5);
                        let step = mag.min(temp);
                        xs[i] = (xs[i] + (dx[i] / mag) * step).clamp(-0.92, 0.92);
                        ys[i] = (ys[i] + (dy[i] / mag) * step).clamp(-0.92, 0.92);
                    }
                }
                for (i, p) in analytics.papers.iter().enumerate() {
                    pos.insert(p.record.id.clone(), [xs[i], ys[i]]);
                }
                return pos;
            }

            let mut xs = vec![0.0f32; n];
            let mut ys = vec![0.0f32; n];
            for (i, p) in analytics.papers.iter().enumerate() {
                let [sx, sy] = seed_map
                    .get(&p.record.id)
                    .copied()
                    .unwrap_or([0.0, 0.0]);
                xs[i] = sx * 0.75;
                ys[i] = sy * 0.75;
            }

            let k_ideal = (2.2 / (n as f32)).sqrt();
            for iter in 0..110 {
                let mut dx = vec![0.0f32; n];
                let mut dy = vec![0.0f32; n];
                let temp = 0.075 * (1.0 - (iter as f32) / 118.0);

                for i in 0..n {
                    let di = (analytics.papers[i].undir_deg + 1) as f32;
                    for j in (i + 1)..n {
                        let dj = (analytics.papers[j].undir_deg + 1) as f32;
                        let ux = xs[i] - xs[j];
                        let uy = ys[i] - ys[j];
                        let d2 = ux * ux + uy * uy + 0.0012;
                        let d = d2.sqrt();
                        let rep = (k_ideal * k_ideal * 0.34 * (di * dj).sqrt() * 0.25) / d2;
                        let fx = (ux / d) * rep;
                        let fy = (uy / d) * rep;
                        dx[i] += fx;
                        dy[i] += fy;
                        dx[j] -= fx;
                        dy[j] -= fy;
                    }
                }
                for &(u, v) in &analytics.active_edges {
                    let ux = xs[u] - xs[v];
                    let uy = ys[u] - ys[v];
                    let d = (ux * ux + uy * uy + 0.0005).sqrt();
                    let attr = ((d * d) / k_ideal.max(0.05)) * 0.54;
                    let fx = (ux / d) * attr;
                    let fy = (uy / d) * attr;
                    dx[u] -= fx;
                    dy[u] -= fy;
                    dx[v] += fx;
                    dy[v] += fy;
                }
                for i in 0..n {
                    dx[i] -= xs[i] * 0.07;
                    dy[i] -= ys[i] * 0.07;
                    let mag = (dx[i] * dx[i] + dy[i] * dy[i]).sqrt().max(1e-5);
                    let step = mag.min(temp);
                    xs[i] = (xs[i] + (dx[i] / mag) * step).clamp(-0.90, 0.90);
                    ys[i] = (ys[i] + (dy[i] / mag) * step).clamp(-0.90, 0.90);
                }
            }
            for (i, p) in analytics.papers.iter().enumerate() {
                pos.insert(p.record.id.clone(), [xs[i], ys[i]]);
            }
        }
    }

    pos
}

/// Computes active node visibility mask for Ego Spotlight modes (`FullGraph`, `OneHop`, `TwoHop`, `UpstreamAncestors`, `DownstreamCiters`).
pub fn compute_spotlight_mask(
    analytics: &CorpusAnalytics,
    focal_id: &str,
    mode: EgoSpotlightMode,
) -> Vec<bool> {
    let n = analytics.papers.len();
    if mode == EgoSpotlightMode::FullGraph || n == 0 {
        return vec![true; n];
    }
    let Some(root) = analytics.papers.iter().position(|p| p.record.id == focal_id) else {
        return vec![true; n];
    };

    let mut mask = vec![false; n];
    mask[root] = true;

    match mode {
        EgoSpotlightMode::FullGraph => mask.fill(true),
        EgoSpotlightMode::OneHopEgo => {
            for &nbr in &analytics.undir_adj[root] {
                mask[nbr] = true;
            }
        }
        EgoSpotlightMode::TwoHopEgo => {
            for &nbr in &analytics.undir_adj[root] {
                mask[nbr] = true;
                for &nbr2 in &analytics.undir_adj[nbr] {
                    mask[nbr2] = true;
                }
            }
        }
        EgoSpotlightMode::UpstreamAncestors => {
            let mut q = VecDeque::new();
            q.push_back(root);
            while let Some(u) = q.pop_front() {
                for &r_idx in &analytics.out_adj[u] {
                    if !mask[r_idx] {
                        mask[r_idx] = true;
                        q.push_back(r_idx);
                    }
                }
            }
        }
        EgoSpotlightMode::DownstreamCiters => {
            let mut q = VecDeque::new();
            q.push_back(root);
            while let Some(u) = q.pop_front() {
                for &c_idx in &analytics.in_adj[u] {
                    if !mask[c_idx] {
                        mask[c_idx] = true;
                        q.push_back(c_idx);
                    }
                }
            }
        }
    }

    mask
}

/// Computes RGB `[u8; 3]` color for a paper node under the chosen `NodeColorEncoding` (shared between GUI & GraphML/GEXF exporters).
pub fn node_rgb_for_encoding(
    p: &PaperMetrics,
    color_by: NodeColorEncoding,
    max_v_exp: f64,
    max_beauty: f64,
) -> [u8; 3] {
    match color_by {
        NodeColorEncoding::Discipline => p.record.disc.rgb(),
        NodeColorEncoding::LouvainCommunity => community_rgb(p.louvain_comm),
        NodeColorEncoding::InfomapModule => community_rgb(p.infomap_comm + 3),
        NodeColorEncoding::VelocityVExp => {
            let t = ((p.v_exp / max_v_exp.max(0.25)).clamp(0.0, 1.0)) as f32;
            lerp_plasma_rgb(t)
        }
        NodeColorEncoding::SleepingBeautyB => {
            let t = ((p.beauty_b / max_beauty.max(5.0)).clamp(0.0, 1.0)) as f32;
            [
                (38.0 + 212.0 * t) as u8,
                (112.0 + 92.0 * t) as u8,
                (215.0 - 190.0 * t) as u8,
            ]
        }
        NodeColorEncoding::IntegrityStatus => match p.record.retraction.status {
            RetractionStatus::Retracted | RetractionStatus::Withdrawn => [239, 68, 68],
            RetractionStatus::ExpressionOfConcern | RetractionStatus::Correction => [250, 204, 21],
            RetractionStatus::Clean => [52, 211, 153],
            RetractionStatus::Unscreened => [148, 163, 184],
        },
        NodeColorEncoding::BioEntityRichness => {
            let total_bio = p.uniprot_bridge_count + p.pdb_bridge_count + p.epmc_annotation_count;
            if p.pdb_bridge_count > 0 && p.uniprot_bridge_count > 0 {
                [6, 182, 212]
            } else if p.uniprot_bridge_count > 0 {
                [245, 158, 11]
            } else if total_bio > 0 {
                [132, 204, 22]
            } else {
                [71, 85, 105]
            }
        }
    }
}

pub fn node_radius_for_encoding(
    p: &PaperMetrics,
    size_by: NodeSizeEncoding,
    max_cb: f64,
    max_v: f64,
    max_b: f64,
) -> f32 {
    let norm = match size_by {
        NodeSizeEncoding::BrandesBetweenness => (p.cb_undir / max_cb.max(0.02)).clamp(0.0, 1.0),
        NodeSizeEncoding::InCitations => ((p.in_deg as f64).sqrt() / 4.5).clamp(0.0, 1.0),
        NodeSizeEncoding::VelocityVExp => (p.v_exp / max_v.max(0.25)).clamp(0.0, 1.0),
        NodeSizeEncoding::SleepingBeautyB => (p.beauty_b / max_b.max(5.0)).clamp(0.0, 1.0),
        NodeSizeEncoding::InfomapFlow => (p.infomap_flow * 14.0).clamp(0.0, 1.0),
        NodeSizeEncoding::DataCiteDownloads => {
            ((p.record.total_artifact_downloads() as f64 + 1.0).ln() / 9.0).clamp(0.0, 1.0)
        }
        NodeSizeEncoding::BioEntityBridges => {
            let bio_n = p.uniprot_bridge_count + p.pdb_bridge_count + p.epmc_annotation_count;
            ((bio_n as f64) / 6.0).clamp(0.0, 1.0)
        }
    };
    5.2 + (norm as f32) * 10.5
}

fn lerp_plasma_rgb(t: f32) -> [u8; 3] {
    if t < 0.5 {
        let u = t * 2.0;
        [
            (30.0 + 165.0 * u) as u8,
            (110.0 - 35.0 * u) as u8,
            (235.0 - 45.0 * u) as u8,
        ]
    } else {
        let u = (t - 0.5) * 2.0;
        [
            (195.0 + 56.0 * u) as u8,
            (75.0 + 135.0 * u) as u8,
            (190.0 - 155.0 * u) as u8,
        ]
    }
}
