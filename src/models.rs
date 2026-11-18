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

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Discipline {
    INF,
    NET,
    EVO,
    ML,
    EPI,
    SCI,
}

impl Discipline {
    pub const ALL: [Discipline; 6] = [
        Discipline::INF,
        Discipline::NET,
        Discipline::EVO,
        Discipline::ML,
        Discipline::EPI,
        Discipline::SCI,
    ];

    pub fn index(self) -> usize {
        match self {
            Discipline::INF => 0,
            Discipline::NET => 1,
            Discipline::EVO => 2,
            Discipline::ML => 3,
            Discipline::EPI => 4,
            Discipline::SCI => 5,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Discipline::INF => "INF",
            Discipline::NET => "NET",
            Discipline::EVO => "EVO",
            Discipline::ML => "ML",
            Discipline::EPI => "EPI",
            Discipline::SCI => "SCI",
        }
    }

    pub fn full_name(self) -> &'static str {
        match self {
            Discipline::INF => "Information Theory & Statistical Physics",
            Discipline::NET => "Network Science & Complex Systems",
            Discipline::EVO => "Evolutionary & Systems Biology",
            Discipline::ML => "Machine Learning & Protein AI",
            Discipline::EPI => "Epidemiology & Phylodynamics",
            Discipline::SCI => "Scientometrics & Science of Science",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Discipline::INF => "Info & StatPhys",
            Discipline::NET => "Network Sci",
            Discipline::EVO => "Evo & SysBio",
            Discipline::ML => "ML & Protein AI",
            Discipline::EPI => "Epi & Phylo",
            Discipline::SCI => "Scientometrics",
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Discipline::INF => [56, 189, 248],
            Discipline::NET => [167, 139, 250],
            Discipline::EVO => [52, 211, 153],
            Discipline::ML => [251, 146, 60],
            Discipline::EPI => [244, 63, 94],
            Discipline::SCI => [250, 204, 21],
        }
    }

    pub fn hex_color(self) -> &'static str {
        match self {
            Discipline::INF => "#38bdf8",
            Discipline::NET => "#a78bfa",
            Discipline::EVO => "#34d399",
            Discipline::ML => "#fb923c",
            Discipline::EPI => "#f43f5e",
            Discipline::SCI => "#facc15",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code.trim().to_ascii_uppercase().as_str() {
            "INF" => Discipline::INF,
            "NET" => Discipline::NET,
            "EVO" => Discipline::EVO,
            "ML" => Discipline::ML,
            "EPI" => Discipline::EPI,
            "SCI" => Discipline::SCI,
            _ => Self::infer_from_text(code),
        }
    }

    pub fn infer_from_text(text: &str) -> Self {
        let lower = text.to_ascii_lowercase();
        let epi_kw = [
            "epidemi", "pathogen", "viral", "phylodynam", "contagion", "disease",
            "public health", "outbreak", "nextstrain", "infectious",
        ];
        if epi_kw.iter().any(|k| lower.contains(k)) {
            return Discipline::EPI;
        }
        let sci_kw = [
            "citation", "scientometric", "bibliometric", "sleeping beaut", "disrupt",
            "h-index", "team science", "patent",
        ];
        if sci_kw.iter().any(|k| lower.contains(k)) {
            return Discipline::SCI;
        }
        let ml_kw = [
            "transformer", "neural", "language model", "alphafold", "esmfold",
            "deep learning", "machine learning", "attention", "algorithm",
        ];
        if ml_kw.iter().any(|k| lower.contains(k)) {
            return Discipline::ML;
        }
        let evo_kw = [
            "evolution", "natural selection", "phylogen", "fitness", "protein",
            "molecular", "ecolog", "biolog", "coevolution", "genomic",
        ];
        if evo_kw.iter().any(|k| lower.contains(k)) {
            return Discipline::EVO;
        }
        let net_kw = [
            "network", "betweenness", "community", "graph", "scale-free",
            "modularity", "random walk", "cartography", "topology",
        ];
        if net_kw.iter().any(|k| lower.contains(k)) {
            return Discipline::NET;
        }
        Discipline::INF
    }
}

#[derive(Debug, Clone)]
pub struct ConceptSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub keywords: &'static [&'static str],
}

pub const CONCEPTS: [ConceptSpec; 12] = [
    ConceptSpec {
        id: "c_info_coding",
        label: "Channel Entropy & Coding",
        keywords: &["entropy", "channel", "coding", "bit", "redundancy", "shannon", "compression", "bottleneck"],
    },
    ConceptSpec {
        id: "c_thermo_agency",
        label: "Stochastic Thermo & Agency",
        keywords: &["thermodynamics", "nonequilibrium", "fluctuation", "semantic", "agency", "transfer entropy", "erasure", "free energy"],
    },
    ConceptSpec {
        id: "c_graph_scaling",
        label: "Scale-Free & Preferential Scaling",
        keywords: &["scale-free", "preferential attachment", "power-law", "hubs", "degree distribution", "percolation"],
    },
    ConceptSpec {
        id: "c_modularity_paths",
        label: "Betweenness & Modular Cartography",
        keywords: &["betweenness", "shortest-path", "geodesic", "modularity", "community", "cartography", "participation", "random walk"],
    },
    ConceptSpec {
        id: "c_evo_selection",
        label: "Evolutionary Info & Selection",
        keywords: &["natural selection", "fitness", "adaptive", "bet-hedging", "fisher information", "regret", "multiplicative weights", "genetic"],
    },
    ConceptSpec {
        id: "c_seq_coevolution",
        label: "Sequence Coevolution & Contacts",
        keywords: &["coevolution", "residue", "contact", "alignment", "phylogeny", "epistasis", "binding site"],
    },
    ConceptSpec {
        id: "c_transformers_lm",
        label: "Self-Attention & Language Models",
        keywords: &["attention", "transformer", "language model", "bidirectional", "masked", "token", "unsupervised"],
    },
    ConceptSpec {
        id: "c_protein_folding",
        label: "Atomic Folding & Structural AI",
        keywords: &["protein structure", "alphafold", "esmfold", "atomic", "folding", "metagenomic", "evoformer"],
    },
    ConceptSpec {
        id: "c_contagion_mobility",
        label: "Mobility & Contagion Geometry",
        keywords: &["airline", "mobility", "metapopulation", "effective distance", "contagion", "wavefront", "epidemic"],
    },
    ConceptSpec {
        id: "c_phylodynamics",
        label: "Pathogen Phylodynamics & Escape",
        keywords: &["phylodynamics", "pathogen", "viral", "escape", "nextstrain", "genomic surveillance", "clade", "disease modules"],
    },
    ConceptSpec {
        id: "c_citation_sleep",
        label: "Citation Aging & Sleeping Beauties",
        keywords: &["citation", "immediacy", "h-index", "sleeping beauty", "awakening", "obsolescence", "bibliometric"],
    },
    ConceptSpec {
        id: "c_novelty_disrupt",
        label: "Team Science, Novelty & Disruption",
        keywords: &["teams", "atypical", "novelty", "disruption", "disruptive", "consolidating", "patents"],
    },
];

pub fn concept_index(concept_id: &str) -> Option<usize> {
    CONCEPTS.iter().position(|c| c.id == concept_id)
}

pub fn concept_label(concept_id: &str) -> &'static str {
    CONCEPTS
        .iter()
        .find(|c| c.id == concept_id)
        .map(|c| c.label)
        .unwrap_or("General Scientific Concept")
}

pub fn home_concepts_for_discipline(disc: Discipline) -> [&'static str; 2] {
    match disc {
        Discipline::INF => ["c_info_coding", "c_thermo_agency"],
        Discipline::NET => ["c_graph_scaling", "c_modularity_paths"],
        Discipline::EVO => ["c_evo_selection", "c_seq_coevolution"],
        Discipline::ML => ["c_transformers_lm", "c_protein_folding"],
        Discipline::EPI => ["c_contagion_mobility", "c_phylodynamics"],
        Discipline::SCI => ["c_citation_sleep", "c_novelty_disrupt"],
    }
}

pub fn infer_concepts_from_text(text: &str, fallback_disc: Discipline) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    for c in &CONCEPTS {
        if c.keywords.iter().any(|kw| lower.contains(kw)) {
            out.push(c.id.to_string());
        }
        if out.len() >= 3 {
            break;
        }
    }
    if out.is_empty() {
        out.push(home_concepts_for_discipline(fallback_disc)[0].to_string());
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OaStatus {
    Gold,
    Green,
    Hybrid,
    Bronze,
    Arxiv,
    Closed,
    Unknown,
}

impl OaStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OaStatus::Gold => "gold",
            OaStatus::Green => "green",
            OaStatus::Hybrid => "hybrid",
            OaStatus::Bronze => "bronze",
            OaStatus::Arxiv => "arxiv",
            OaStatus::Closed => "closed",
            OaStatus::Unknown => "unknown",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "gold" => OaStatus::Gold,
            "green" => OaStatus::Green,
            "hybrid" => OaStatus::Hybrid,
            "bronze" => OaStatus::Bronze,
            "arxiv" => OaStatus::Arxiv,
            "closed" => OaStatus::Closed,
            _ => OaStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetractionStatus {
    Unscreened,
    Clean,
    Retracted,
    Withdrawn,
    ExpressionOfConcern,
    Correction,
}

impl Default for RetractionStatus {
    fn default() -> Self {
        RetractionStatus::Unscreened
    }
}

impl RetractionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RetractionStatus::Unscreened => "unscreened",
            RetractionStatus::Clean => "clean",
            RetractionStatus::Retracted => "retracted",
            RetractionStatus::Withdrawn => "withdrawn",
            RetractionStatus::ExpressionOfConcern => "expression_of_concern",
            RetractionStatus::Correction => "correction",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "clean" | "none" | "ok" | "reinstatement" => RetractionStatus::Clean,
            "retracted" | "retraction" | "removal" => RetractionStatus::Retracted,
            "withdrawn" | "withdrawal" => RetractionStatus::Withdrawn,
            "expression_of_concern" | "eoc" | "concern" => RetractionStatus::ExpressionOfConcern,
            "correction" | "erratum" | "corrigendum" => RetractionStatus::Correction,
            _ => RetractionStatus::Unscreened,
        }
    }

    pub fn is_retracted_or_withdrawn(self) -> bool {
        matches!(self, RetractionStatus::Retracted | RetractionStatus::Withdrawn)
    }

    pub fn is_flagged(self) -> bool {
        matches!(
            self,
            RetractionStatus::Retracted
                | RetractionStatus::Withdrawn
                | RetractionStatus::ExpressionOfConcern
                | RetractionStatus::Correction
        )
    }

    pub fn severity_rank(self) -> u8 {
        match self {
            RetractionStatus::Retracted => 5,
            RetractionStatus::Withdrawn => 4,
            RetractionStatus::ExpressionOfConcern => 3,
            RetractionStatus::Correction => 2,
            RetractionStatus::Clean => 1,
            RetractionStatus::Unscreened => 0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetractionRecord {
    pub status: RetractionStatus,
    pub source: String,
    pub notice_doi: String,
    pub notice_date: String,
    pub notice_year: Option<i32>,
    pub record_id: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataCiteArtifactKind {
    Dataset,
    Software,
    Workflow,
    Model,
    Other,
}

impl Default for DataCiteArtifactKind {
    fn default() -> Self {
        DataCiteArtifactKind::Dataset
    }
}

impl DataCiteArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DataCiteArtifactKind::Dataset => "Dataset",
            DataCiteArtifactKind::Software => "Software",
            DataCiteArtifactKind::Workflow => "Workflow",
            DataCiteArtifactKind::Model => "Model",
            DataCiteArtifactKind::Other => "Other",
        }
    }

    pub fn from_resource_type_general(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "dataset" | "collection" => DataCiteArtifactKind::Dataset,
            "software" | "computationalnotebook" => DataCiteArtifactKind::Software,
            "workflow" => DataCiteArtifactKind::Workflow,
            "model" => DataCiteArtifactKind::Model,
            _ => DataCiteArtifactKind::Other,
        }
    }

    pub fn is_software_like(self) -> bool {
        matches!(
            self,
            DataCiteArtifactKind::Software
                | DataCiteArtifactKind::Workflow
                | DataCiteArtifactKind::Model
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCiteLink {
    pub artifact_doi: String,
    pub artifact_title: String,
    pub artifact_kind: DataCiteArtifactKind,
    pub repository: String,
    pub relation_type: String,
    pub citation_count: u64,
    pub view_count: u64,
    pub download_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BioEntityKind {
    UniProt,
    PdbStructure,
    GeneOntology,
    DiseaseEfo,
    ChemicalChebi,
    OrganismTaxon,
    AccessionOther,
}

impl Default for BioEntityKind {
    fn default() -> Self {
        BioEntityKind::UniProt
    }
}

impl BioEntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BioEntityKind::UniProt => "uniprot",
            BioEntityKind::PdbStructure => "pdb_structure",
            BioEntityKind::GeneOntology => "epmc_go",
            BioEntityKind::DiseaseEfo => "epmc_efo",
            BioEntityKind::ChemicalChebi => "epmc_chebi",
            BioEntityKind::OrganismTaxon => "epmc_taxon",
            BioEntityKind::AccessionOther => "epmc_accession",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            BioEntityKind::UniProt => "UniProt",
            BioEntityKind::PdbStructure => "PDB",
            BioEntityKind::GeneOntology => "GO",
            BioEntityKind::DiseaseEfo => "EFO/Disease",
            BioEntityKind::ChemicalChebi => "ChEBI",
            BioEntityKind::OrganismTaxon => "Taxon",
            BioEntityKind::AccessionOther => "Accession",
        }
    }

    pub fn from_str_lossy(raw: &str) -> Self {
        let low = raw.trim().to_ascii_lowercase();
        if low.contains("uniprot") || low == "protein" {
            BioEntityKind::UniProt
        } else if low.contains("pdb") || low == "structure" {
            BioEntityKind::PdbStructure
        } else if low.contains("go") || low.contains("ontology") {
            BioEntityKind::GeneOntology
        } else if low.contains("efo") || low.contains("disease") || low.contains("mondo") {
            BioEntityKind::DiseaseEfo
        } else if low.contains("chebi") || low.contains("chem") {
            BioEntityKind::ChemicalChebi
        } else if low.contains("taxon") || low.contains("organism") {
            BioEntityKind::OrganismTaxon
        } else {
            BioEntityKind::AccessionOther
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            BioEntityKind::UniProt => [245, 158, 11],       // Amber-Gold (#f59e0b)
            BioEntityKind::PdbStructure => [6, 182, 212],   // Structural Cyan (#06b6d4)
            BioEntityKind::GeneOntology => [132, 204, 22],  // Ontology Lime (#84cc16)
            BioEntityKind::DiseaseEfo => [236, 72, 153],    // Phenotype Magenta (#ec4899)
            BioEntityKind::ChemicalChebi => [251, 113, 133],// ChEBI Coral (#fb7185)
            BioEntityKind::OrganismTaxon => [52, 211, 153], // Taxon Emerald (#34d399)
            BioEntityKind::AccessionOther => [148, 163, 184],
        }
    }

    pub fn hex_color(self) -> &'static str {
        match self {
            BioEntityKind::UniProt => "#f59e0b",
            BioEntityKind::PdbStructure => "#06b6d4",
            BioEntityKind::GeneOntology => "#84cc16",
            BioEntityKind::DiseaseEfo => "#ec4899",
            BioEntityKind::ChemicalChebi => "#fb7185",
            BioEntityKind::OrganismTaxon => "#34d399",
            BioEntityKind::AccessionOther => "#94a3b8",
        }
    }

    pub fn is_epmc_ontology_or_term(self) -> bool {
        !matches!(self, BioEntityKind::UniProt | BioEntityKind::PdbStructure)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BioEntityBridgeLink {
    pub entity_id: String,
    pub kind: BioEntityKind,
    pub label: String,
    pub uri: String,
    pub provider: String,
    pub section: String,
    pub mention_count: u32,
    #[serde(default)]
    pub linked_pdb_ids: Vec<String>,
    #[serde(default)]
    pub resolution_angstrom: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperRecord {
    pub id: String,
    pub title: String,
    pub authors: String,
    pub year: i32,
    pub venue: String,
    pub disc: Discipline,
    pub doi: String,
    pub arxiv_id: String,
    pub openalex_id: String,
    pub s2_id: String,
    pub pmcid: String,
    pub source_url: String,
    pub oa_pdf_url: String,
    pub oa_status: OaStatus,
    pub external_citations: Option<u64>,
    pub abstract_text: String,
    pub terms: Vec<String>,
    pub concepts: Vec<String>,
    pub local_pdf_path: Option<String>,
    pub pdf_sha256: Option<String>,
    pub pdf_bytes: Option<u64>,
    #[serde(default)]
    pub raw_references: Vec<String>,
    #[serde(default)]
    pub retraction: RetractionRecord,
    #[serde(default)]
    pub datacite_links: Vec<DataCiteLink>,
    #[serde(default)]
    pub bio_entities: Vec<BioEntityBridgeLink>,
}

impl PaperRecord {
    pub fn short_cite(&self) -> String {
        let first_author = self
            .authors
            .split(',')
            .next()
            .unwrap_or(&self.authors)
            .split_whitespace()
            .last()
            .unwrap_or(&self.authors);
        format!("{} ({})", first_author, self.year)
    }

    pub fn dataset_count(&self) -> usize {
        self.datacite_links
            .iter()
            .filter(|l| l.artifact_kind == DataCiteArtifactKind::Dataset)
            .count()
    }

    pub fn software_count(&self) -> usize {
        self.datacite_links
            .iter()
            .filter(|l| l.artifact_kind.is_software_like())
            .count()
    }

    pub fn total_artifact_downloads(&self) -> u64 {
        self.datacite_links.iter().map(|l| l.download_count).sum()
    }

    pub fn formatted_artifact_dois(&self) -> String {
        self.datacite_links
            .iter()
            .map(|l| {
                format!(
                    "{}:{}:{}",
                    l.artifact_kind.as_str(),
                    if l.relation_type.is_empty() {
                        "Linked"
                    } else {
                        &l.relation_type
                    },
                    l.artifact_doi
                )
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn uniprot_count(&self) -> usize {
        self.bio_entities
            .iter()
            .filter(|b| b.kind == BioEntityKind::UniProt)
            .count()
    }

    pub fn pdb_count(&self) -> usize {
        self.bio_entities
            .iter()
            .filter(|b| b.kind == BioEntityKind::PdbStructure)
            .count()
    }

    pub fn epmc_annotation_count(&self) -> usize {
        self.bio_entities
            .iter()
            .filter(|b| b.kind.is_epmc_ontology_or_term())
            .count()
    }

    pub fn formatted_uniprot_accessions(&self) -> String {
        self.bio_entities
            .iter()
            .filter(|b| b.kind == BioEntityKind::UniProt)
            .map(|b| b.entity_id.as_str())
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn formatted_pdb_ids(&self) -> String {
        self.bio_entities
            .iter()
            .filter(|b| b.kind == BioEntityKind::PdbStructure)
            .map(|b| b.entity_id.as_str())
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn formatted_epmc_annotations(&self) -> String {
        self.bio_entities
            .iter()
            .filter(|b| b.kind.is_epmc_ontology_or_term())
            .map(|b| format!("{}:{}", b.kind.as_str(), b.entity_id))
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn formatted_bio_entities(&self) -> String {
        self.bio_entities
            .iter()
            .map(|b| {
                let clean_lbl = b.label.replace([';', ':'], " ");
                format!("{}:{}:{}", b.kind.as_str(), b.entity_id, clean_lbl)
            })
            .collect::<Vec<_>>()
            .join(";")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CitationEdge {
    pub source: String,
    pub target: String,
    pub provenance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyntheticRegime {
    Convergent,
    Awakening,
    Siloed,
}

impl SyntheticRegime {
    pub fn as_str(self) -> &'static str {
        match self {
            SyntheticRegime::Convergent => "convergent",
            SyntheticRegime::Awakening => "awakening",
            SyntheticRegime::Siloed => "siloed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsParams {
    pub window_width: i32,
    pub window_stride: i32,
    pub half_life: f64,
    pub horizon_year: i32,
    pub smoothing_alpha: f64,
    pub discourse_weight: f64,
    pub bridge_directed: bool,
    pub louvain_resolution: f64,
    pub infomap_teleport: f64,
    #[serde(default)]
    pub exclude_retracted: bool,
}

impl Default for AnalyticsParams {
    fn default() -> Self {
        Self {
            window_width: 10,
            window_stride: 5,
            half_life: 6.0,
            horizon_year: 2026,
            smoothing_alpha: 0.05,
            discourse_weight: 0.35,
            bridge_directed: false,
            louvain_resolution: 1.0,
            infomap_teleport: 0.15,
            exclude_retracted: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperMetrics {
    pub idx: usize,
    pub record: PaperRecord,
    pub in_deg: usize,
    pub out_deg: usize,
    pub undir_deg: usize,
    pub v_exp: f64,
    pub v_win: f64,
    pub accel: f64,
    pub immed_5y_pct: f64,
    pub beauty_b: f64,
    pub awaken_year: i32,
    pub hibernation_years: i32,
    pub peak_age_tm: usize,
    pub disruption_cd: f64,
    pub disruption_cd5: f64,
    pub hits_hub: f64,
    pub hits_authority: f64,
    pub cb_undir: f64,
    pub cb_dir: f64,
    pub part_p: f64,
    pub within_z: f64,
    pub burt_c: f64,
    pub bridge_score: f64,
    pub role_code: &'static str,
    pub role_label: &'static str,
    pub regime_tag: &'static str,
    pub louvain_comm: usize,
    pub louvain_part_p: f64,
    pub infomap_comm: usize,
    pub infomap_part_p: f64,
    pub infomap_flow: f64,
    pub reclassified_louvain: bool,
    pub reclassified_infomap: bool,
    pub post_retraction_cites: usize,
    pub cites_retracted_count: usize,
    pub dataset_bridge_count: usize,
    pub software_bridge_count: usize,
    pub shared_artifact_peers: usize,
    pub uniprot_bridge_count: usize,
    pub pdb_bridge_count: usize,
    pub epmc_annotation_count: usize,
    pub shared_bio_entity_peers: usize,
    pub annual_counts: Vec<f64>,
    pub ke_ref_line: Vec<f64>,
    pub v_win_series: Vec<f64>,
    pub v_exp_series: Vec<f64>,
    pub accel_series: Vec<f64>,
    pub cum_series: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConduit {
    pub u: usize,
    pub v: usize,
    pub source_id: String,
    pub target_id: String,
    pub source_cite: String,
    pub target_cite: String,
    pub disc_from: Discipline,
    pub disc_to: Discipline,
    pub cross_disc: bool,
    pub eb_undir: f64,
    pub eb_dir: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowTopicStats {
    pub w_idx: usize,
    pub start_year: i32,
    pub end_year: i32,
    pub label: String,
    pub pub_counts_by_disc: [usize; 6],
    pub active_pubs: usize,
    pub active_cites: usize,
    pub h_disc: f64,
    pub h_topic: f64,
    pub mi_bits: f64,
    pub nmi: f64,
    pub pmi: [[f64; 12]; 6],
    pub cond_t_given_d: [[f64; 12]; 6],
    pub jsd_matrix: [[f64; 6]; 6],
    pub within_drift: [f64; 6],
}

#[derive(Debug, Clone)]
pub struct CorpusAnalytics {
    pub min_year: i32,
    pub max_year: i32,
    pub year_span: usize,
    pub papers: Vec<PaperMetrics>,
    pub active_edges: Vec<(usize, usize)>,
    pub out_adj: Vec<Vec<usize>>,
    pub in_adj: Vec<Vec<usize>>,
    pub undir_adj: Vec<Vec<usize>>,
    pub conduits: Vec<EdgeConduit>,
    pub cross_edge_count: usize,
    pub windows: Vec<WindowTopicStats>,
    pub cumulative_disc_drift: [f64; 6],
    pub communities: crate::community_detection::CommunityReport,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SearchProvidersConfig {
    pub openalex: bool,
    pub semantic_scholar: bool,
    pub crossref: bool,
    pub arxiv: bool,
    #[serde(default)]
    pub openaire: bool,
    #[serde(default)]
    pub inspire_hep: bool,
    #[serde(default)]
    pub nasa_ads: bool,
    #[serde(default)]
    pub google_scholar: bool,
    #[serde(default)]
    pub pubmed: bool,
    #[serde(default)]
    pub europe_pmc: bool,
    #[serde(default)]
    pub dblp: bool,
    #[serde(default)]
    pub biorxiv_medrxiv: bool,
    #[serde(default)]
    pub datacite: bool,
}

impl Default for SearchProvidersConfig {
    fn default() -> Self {
        Self {
            openalex: true,
            semantic_scholar: true,
            crossref: true,
            arxiv: true,
            openaire: false,
            inspire_hep: false,
            nasa_ads: false,
            google_scholar: false,
            pubmed: false,
            europe_pmc: false,
            dblp: false,
            biorxiv_medrxiv: false,
            datacite: false,
        }
    }
}

impl SearchProvidersConfig {
    pub fn from_csv_flags(spec: &str) -> Self {
        let low = spec.to_ascii_lowercase();
        let all = low.split(',').any(|t| t.trim() == "all");
        let has = |keys: &[&str]| {
            all || low.split(',').any(|tok| {
                let t = tok.trim();
                keys.iter().any(|k| t == *k || t.contains(*k))
            })
        };
        Self {
            openalex: has(&["openalex"]),
            semantic_scholar: has(&["s2", "semantic"]),
            crossref: has(&["crossref"]),
            arxiv: has(&["arxiv"]),
            openaire: has(&["openaire", "scholexplorer"]),
            inspire_hep: has(&["inspire", "inspirehep", "inspire_hep", "inspire-hep"]),
            nasa_ads: has(&["ads", "nasa", "nasa_ads", "nasa-ads", "adsabs"]),
            google_scholar: has(&["scholar", "gs", "google_scholar", "googlescholar"]),
            pubmed: has(&["pubmed", "ncbi"]),
            europe_pmc: has(&["europepmc", "epmc", "europe_pmc"]),
            dblp: has(&["dblp"]),
            biorxiv_medrxiv: has(&["biorxiv", "medrxiv"]),
            datacite: has(&["datacite", "zenodo", "dryad", "figshare"]),
        }
    }

    pub fn all_open_apis() -> Self {
        Self {
            openalex: true,
            semantic_scholar: true,
            crossref: true,
            arxiv: true,
            openaire: true,
            inspire_hep: false,
            nasa_ads: false,
            google_scholar: false,
            pubmed: true,
            europe_pmc: true,
            dblp: true,
            biorxiv_medrxiv: true,
            datacite: true,
        }
    }

    pub fn biomed_preset() -> Self {
        Self {
            openalex: true,
            semantic_scholar: true,
            crossref: true,
            arxiv: false,
            openaire: false,
            inspire_hep: false,
            nasa_ads: false,
            google_scholar: false,
            pubmed: true,
            europe_pmc: true,
            dblp: false,
            biorxiv_medrxiv: true,
            datacite: true,
        }
    }

    pub fn cs_ai_preset() -> Self {
        Self {
            openalex: true,
            semantic_scholar: true,
            crossref: true,
            arxiv: true,
            openaire: false,
            inspire_hep: false,
            nasa_ads: false,
            google_scholar: false,
            pubmed: false,
            europe_pmc: false,
            dblp: true,
            biorxiv_medrxiv: false,
            datacite: true,
        }
    }

    pub fn physics_astro_preset() -> Self {
        Self {
            openalex: true,
            semantic_scholar: false,
            crossref: true,
            arxiv: true,
            openaire: true,
            inspire_hep: true,
            nasa_ads: true,
            google_scholar: false,
            pubmed: false,
            europe_pmc: false,
            dblp: false,
            biorxiv_medrxiv: false,
            datacite: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlOptions {
    pub max_pages: usize,
    pub pace_ms: u64,
    pub jitter_ms: u64,
    pub respect_robots_txt: bool,
    pub scholar_cites_cluster_id: String,
    pub hydrate_citation_edges: bool,
    pub filter_self_citations: bool,
    pub proxy_url: String,
    pub local_html_paths: Vec<String>,
    #[serde(default = "default_true")]
    pub screen_retractions: bool,
    #[serde(default = "default_true")]
    pub track_datacite_bridges: bool,
    #[serde(default = "default_true")]
    pub track_epmc_annotations: bool,
    #[serde(default = "default_true")]
    pub track_uniprot_pdb_bridges: bool,
    #[serde(default)]
    pub retraction_watch_csv_path: String,
    #[serde(default)]
    pub openaire_token: String,
    #[serde(default)]
    pub nasa_ads_token: String,
}

impl Default for CrawlOptions {
    fn default() -> Self {
        Self {
            max_pages: 2,
            pace_ms: 3200,
            jitter_ms: 600,
            respect_robots_txt: true,
            scholar_cites_cluster_id: String::new(),
            hydrate_citation_edges: true,
            filter_self_citations: false,
            proxy_url: String::new(),
            local_html_paths: Vec::new(),
            screen_retractions: true,
            track_datacite_bridges: true,
            track_epmc_annotations: true,
            track_uniprot_pdb_bridges: true,
            retraction_watch_csv_path: String::new(),
            openaire_token: String::new(),
            nasa_ads_token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub providers: SearchProvidersConfig,
    pub max_per_provider: usize,
    pub year_min: Option<i32>,
    pub year_max: Option<i32>,
    pub oa_only: bool,
    pub polite_email: String,
    pub snowball_hops: u8,
    #[serde(default)]
    pub crawl: CrawlOptions,
    #[serde(default)]
    pub authors: String,
    #[serde(default)]
    pub publication_venue: String,
    #[serde(default)]
    pub title_words: String,
    #[serde(default)]
    pub keywords_all: String,
    #[serde(default)]
    pub exclude_words: String,
    #[serde(default)]
    pub identifier_lookup: String,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: "information theory evolutionary biology protein language model".to_string(),
            providers: SearchProvidersConfig::default(),
            max_per_provider: 20,
            year_min: None,
            year_max: None,
            oa_only: false,
            polite_email: "research@crvscience.com".to_string(),
            snowball_hops: 1,
            crawl: CrawlOptions::default(),
            authors: String::new(),
            publication_venue: String::new(),
            title_words: String::new(),
            keywords_all: String::new(),
            exclude_words: String::new(),
            identifier_lookup: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHarvestSummary {
    pub papers: Vec<PaperRecord>,
    pub edges: Vec<CitationEdge>,
    pub provider_counts: Vec<(String, usize)>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreeningSummary {
    pub papers_screened: usize,
    pub retractions_flagged: usize,
    pub corrections_or_concerns: usize,
    pub datacite_artifacts_linked: usize,
    pub shared_artifact_bridge_edges: usize,
    pub epmc_annotations_linked: usize,
    pub uniprot_proteins_linked: usize,
    pub pdb_structures_linked: usize,
    pub shared_bio_bridge_edges: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DownloadStatus {
    Downloaded,
    CachedAlreadyValid,
    NoOpenAccessPdfFound,
    RejectedNonPdfPayload,
    FailedNetwork(String),
}

impl DownloadStatus {
    pub fn label(&self) -> String {
        match self {
            DownloadStatus::Downloaded => "OK (%PDF Verified)".to_string(),
            DownloadStatus::CachedAlreadyValid => "Cached (SHA-256 Verified)".to_string(),
            DownloadStatus::NoOpenAccessPdfFound => "No Legal OA PDF Found".to_string(),
            DownloadStatus::RejectedNonPdfPayload => "Rejected (Landing HTML, not %PDF)".to_string(),
            DownloadStatus::FailedNetwork(e) => format!("Error: {}", e),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, DownloadStatus::Downloaded | DownloadStatus::CachedAlreadyValid)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItemReport {
    pub paper_id: String,
    pub title: String,
    pub doi: String,
    pub status: DownloadStatus,
    pub resolved_stage: String,
    pub final_pdf_url: String,
    pub saved_path: Option<String>,
    pub byte_len: u64,
    pub sha256: Option<String>,
}
