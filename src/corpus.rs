use crate::models::{
    home_concepts_for_discipline, BioEntityBridgeLink, BioEntityKind, CitationEdge,
    DataCiteArtifactKind, DataCiteLink, Discipline, OaStatus, PaperRecord, SyntheticRegime,
};

struct SeedPaper {
    id: &'static str,
    title: &'static str,
    authors: &'static str,
    year: i32,
    venue: &'static str,
    disc: Discipline,
    doi: &'static str,
    arxiv_id: &'static str,
    pmcid: &'static str,
    url: &'static str,
    oa_pdf_url: &'static str,
    oa_status: OaStatus,
    terms: &'static [&'static str],
    concepts: &'static [&'static str],
    cites: &'static [&'static str],
}

const EMPIRICAL_SEED: &[SeedPaper] = &[
    // INF (7)
    SeedPaper {
        id: "shannon1948",
        title: "A mathematical theory of communication",
        authors: "C. E. Shannon",
        year: 1948,
        venue: "Bell System Technical Journal 27: 379-423",
        disc: Discipline::INF,
        doi: "10.1002/j.1538-7305.1948.tb01338.x",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC6227811/",
        oa_pdf_url: "",
        oa_status: OaStatus::Bronze,
        terms: &["Shannon entropy", "communication channel", "coding theorem", "uncertainty reduction", "bit rate"],
        concepts: &["c_info_coding"],
        cites: &[],
    },
    SeedPaper {
        id: "shannon1951",
        title: "Prediction and entropy of printed English",
        authors: "C. E. Shannon",
        year: 1951,
        venue: "Bell System Technical Journal 30(1): 50-64",
        disc: Discipline::INF,
        doi: "10.1002/j.1538-7305.1951.tb01366.x",
        arxiv_id: "",
        pmcid: "",
        url: "https://www.princeton.edu/~wbialek/rome/refs/shannon_51.pdf",
        oa_pdf_url: "https://www.princeton.edu/~wbialek/rome/refs/shannon_51.pdf",
        oa_status: OaStatus::Green,
        terms: &["sequence prediction", "language entropy", "redundancy bounds", "next-letter guessing"],
        concepts: &["c_info_coding", "c_transformers_lm"],
        cites: &["shannon1948"],
    },
    SeedPaper {
        id: "schreiber2000",
        title: "Measuring information transfer",
        authors: "T. Schreiber",
        year: 2000,
        venue: "Physical Review Letters 85(2): 461-464",
        disc: Discipline::INF,
        doi: "10.1103/PhysRevLett.85.461",
        arxiv_id: "physics/0001042",
        pmcid: "",
        url: "https://link.aps.org/doi/10.1103/PhysRevLett.85.461",
        oa_pdf_url: "https://arxiv.org/pdf/physics/0001042.pdf",
        oa_status: OaStatus::Arxiv,
        terms: &["transfer entropy", "directed information flow", "Markov dynamics", "causal coupling"],
        concepts: &["c_info_coding", "c_thermo_agency"],
        cites: &[],
    },
    SeedPaper {
        id: "tishby2000",
        title: "The information bottleneck method",
        authors: "N. Tishby, F. C. Pereira, W. Bialek",
        year: 2000,
        venue: "arXiv:physics/0004057 (Allerton Conf.)",
        disc: Discipline::INF,
        doi: "10.48550/arXiv.physics/0004057",
        arxiv_id: "physics/0004057",
        pmcid: "",
        url: "https://arxiv.org/abs/physics/0004057",
        oa_pdf_url: "https://arxiv.org/pdf/physics/0004057.pdf",
        oa_status: OaStatus::Arxiv,
        terms: &["information bottleneck", "rate distortion", "relevant compression", "variational mutual information"],
        concepts: &["c_info_coding", "c_thermo_agency"],
        cites: &["shannon1948"],
    },
    SeedPaper {
        id: "seifert2012",
        title: "Stochastic thermodynamics, fluctuation theorems and molecular machines",
        authors: "U. Seifert",
        year: 2012,
        venue: "Reports on Progress in Physics 75(12): 126001",
        disc: Discipline::INF,
        doi: "10.1088/0034-4885/75/12/126001",
        arxiv_id: "1205.4176",
        pmcid: "",
        url: "https://pubmed.ncbi.nlm.nih.gov/23168354/",
        oa_pdf_url: "https://arxiv.org/pdf/1205.4176.pdf",
        oa_status: OaStatus::Green,
        terms: &["stochastic thermodynamics", "fluctuation theorems", "entropy production", "molecular machines"],
        concepts: &["c_thermo_agency"],
        cites: &[],
    },
    SeedPaper {
        id: "parrondo2015",
        title: "Thermodynamics of information",
        authors: "J. M. R. Parrondo, J. M. Horowitz, T. Sagawa",
        year: 2015,
        venue: "Nature Physics 11: 131-139",
        disc: Discipline::INF,
        doi: "10.1038/nphys3230",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC6227811/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["thermodynamics of information", "Landauer erasure", "feedback control", "free energy"],
        concepts: &["c_thermo_agency", "c_info_coding"],
        cites: &[],
    },
    SeedPaper {
        id: "kolchinsky2018",
        title: "Semantic information, autonomous agency and non-equilibrium statistical physics",
        authors: "A. Kolchinsky, D. H. Wolpert",
        year: 2018,
        venue: "Interface Focus 8(6): 20180041",
        disc: Discipline::INF,
        doi: "10.1098/rsfs.2018.0041",
        arxiv_id: "1806.08053",
        pmcid: "PMC6227811",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC6227811/",
        oa_pdf_url: "https://arxiv.org/pdf/1806.08053.pdf",
        oa_status: OaStatus::Gold,
        terms: &["semantic information", "autonomous agency", "viability", "nonequilibrium thermodynamics", "transfer entropy"],
        concepts: &["c_thermo_agency", "c_info_coding", "c_evo_selection"],
        cites: &["shannon1948", "tishby2000", "schreiber2000", "adami2004", "donaldson2010", "seifert2012", "parrondo2015"],
    },
    // NET (7)
    SeedPaper {
        id: "barabasi1999",
        title: "Emergence of scaling in random networks",
        authors: "A.-L. Barabasi, R. Albert",
        year: 1999,
        venue: "Science 286(5439): 509-512",
        disc: Discipline::NET,
        doi: "10.1126/science.286.5439.509",
        arxiv_id: "cond-mat/9910332",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4475978/",
        oa_pdf_url: "https://arxiv.org/pdf/cond-mat/9910332.pdf",
        oa_status: OaStatus::Green,
        terms: &["preferential attachment", "scale-free networks", "power-law degree distribution", "growing graphs"],
        concepts: &["c_graph_scaling"],
        cites: &[],
    },
    SeedPaper {
        id: "brandes2001",
        title: "A faster algorithm for betweenness centrality",
        authors: "U. Brandes",
        year: 2001,
        venue: "Journal of Mathematical Sociology 25(2): 163-177",
        disc: Discipline::NET,
        doi: "10.1080/0022250X.2001.9990249",
        arxiv_id: "",
        pmcid: "",
        url: "https://www.tandfonline.com/doi/abs/10.1080/0022250X.2001.9990249",
        oa_pdf_url: "",
        oa_status: OaStatus::Green,
        terms: &["betweenness centrality", "shortest-path geodesics", "dependency accumulation", "bridge vertices"],
        concepts: &["c_modularity_paths"],
        cites: &[],
    },
    SeedPaper {
        id: "albert2002",
        title: "Statistical mechanics of complex networks",
        authors: "R. Albert, A.-L. Barabasi",
        year: 2002,
        venue: "Reviews of Modern Physics 74(1): 47-97",
        disc: Discipline::NET,
        doi: "10.1103/RevModPhys.74.47",
        arxiv_id: "cond-mat/0106096",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC2175124/",
        oa_pdf_url: "https://arxiv.org/pdf/cond-mat/0106096.pdf",
        oa_status: OaStatus::Green,
        terms: &["statistical mechanics", "small-world topology", "scale-free hubs", "percolation"],
        concepts: &["c_graph_scaling", "c_contagion_mobility"],
        cites: &["barabasi1999"],
    },
    SeedPaper {
        id: "girvan2002",
        title: "Community structure in social and biological networks",
        authors: "M. Girvan, M. E. J. Newman",
        year: 2002,
        venue: "PNAS 99(12): 7821-7826",
        disc: Discipline::NET,
        doi: "10.1073/pnas.122653799",
        arxiv_id: "cond-mat/0112110",
        pmcid: "PMC122977",
        url: "https://www.pnas.org/doi/10.1073/pnas.122653799",
        oa_pdf_url: "https://arxiv.org/pdf/cond-mat/0112110.pdf",
        oa_status: OaStatus::Green,
        terms: &["community structure", "edge betweenness", "modular graphs", "shortest paths"],
        concepts: &["c_modularity_paths"],
        cites: &[],
    },
    SeedPaper {
        id: "newman2004",
        title: "Finding and evaluating community structure in networks",
        authors: "M. E. J. Newman, M. Girvan",
        year: 2004,
        venue: "Physical Review E 69: 026113",
        disc: Discipline::NET,
        doi: "10.1103/PhysRevE.69.026113",
        arxiv_id: "cond-mat/0308217",
        pmcid: "",
        url: "https://www.semanticscholar.org/paper/Finding-and-evaluating-community-structure-in-Newman-Girvan/08b39cbabc2c5b764a85a0193296f83e23a62934",
        oa_pdf_url: "https://arxiv.org/pdf/cond-mat/0308217.pdf",
        oa_status: OaStatus::Green,
        terms: &["modularity Q", "divisive clustering", "edge betweenness", "community boundaries"],
        concepts: &["c_modularity_paths"],
        cites: &["brandes2001", "girvan2002"],
    },
    SeedPaper {
        id: "guimera2005",
        title: "Functional cartography of complex metabolic networks",
        authors: "R. Guimera, L. A. N. Amaral",
        year: 2005,
        venue: "Nature 433(7028): 895-900",
        disc: Discipline::NET,
        doi: "10.1038/nature03288",
        arxiv_id: "",
        pmcid: "PMC2175124",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC2175124/",
        oa_pdf_url: "",
        oa_status: OaStatus::Green,
        terms: &["functional cartography", "participation coefficient", "within-module z-score", "connector hubs"],
        concepts: &["c_modularity_paths"],
        cites: &["albert2002", "girvan2002", "newman2004"],
    },
    SeedPaper {
        id: "rosvall2008",
        title: "Maps of random walks on complex networks reveal community structure",
        authors: "M. Rosvall, C. T. Bergstrom",
        year: 2008,
        venue: "PNAS 105(4): 1118-1123",
        disc: Discipline::NET,
        doi: "10.1073/pnas.0706851105",
        arxiv_id: "0707.0609",
        pmcid: "PMC2234100",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC2234100/",
        oa_pdf_url: "https://arxiv.org/pdf/0707.0609.pdf",
        oa_status: OaStatus::Green,
        terms: &["map equation", "random walk flow", "Huffman coding compression", "citation map of science"],
        concepts: &["c_modularity_paths", "c_info_coding", "c_citation_sleep"],
        cites: &["shannon1948", "price1965", "girvan2002", "newman2004", "guimera2005"],
    },
    // EVO (9)
    SeedPaper {
        id: "kimura1961",
        title: "Natural selection as the process of accumulating genetic information in adaptive evolution",
        authors: "M. Kimura",
        year: 1961,
        venue: "Genetical Research 2(1): 127-140",
        disc: Discipline::EVO,
        doi: "10.1017/S0016672300000616",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4384894/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["genetic information accumulation", "substitution load", "natural selection", "adaptive evolution"],
        concepts: &["c_evo_selection", "c_info_coding"],
        cites: &[],
    },
    SeedPaper {
        id: "adami2000",
        title: "Evolution of biological complexity",
        authors: "C. Adami, C. Ofria, T. C. Collier",
        year: 2000,
        venue: "PNAS 97(9): 4463-4468",
        disc: Discipline::EVO,
        doi: "10.1073/pnas.97.9.4463",
        arxiv_id: "adap-org/9911001",
        pmcid: "PMC18257",
        url: "https://bpb-us-e1.wpmucdn.com/sites.tufts.edu/dist/e/955/files/2012/05/Adami2012.pdf",
        oa_pdf_url: "https://arxiv.org/pdf/adap-org/9911001.pdf",
        oa_status: OaStatus::Green,
        terms: &["biological complexity", "genomic information content", "digital evolution", "sequence entropy"],
        concepts: &["c_evo_selection", "c_info_coding"],
        cites: &["shannon1948"],
    },
    SeedPaper {
        id: "adami2004",
        title: "Information theory in molecular biology",
        authors: "C. Adami",
        year: 2004,
        venue: "Physics of Life Reviews 1: 3-22",
        disc: Discipline::EVO,
        doi: "10.1016/j.plrev.2004.01.002",
        arxiv_id: "q-bio/0405004",
        pmcid: "",
        url: "https://bpb-us-e1.wpmucdn.com/sites.tufts.edu/dist/e/955/files/2012/05/Adami2012.pdf",
        oa_pdf_url: "https://arxiv.org/pdf/q-bio/0405004.pdf",
        oa_status: OaStatus::Green,
        terms: &["molecular sequence entropy", "mutual information", "residue conservation", "binding site information"],
        concepts: &["c_info_coding", "c_seq_coevolution"],
        cites: &["shannon1948", "adami2000"],
    },
    SeedPaper {
        id: "kussell2005",
        title: "Phenotypic diversity, population growth and information in fluctuating environments",
        authors: "E. Kussell, S. Leibler",
        year: 2005,
        venue: "Science 309(5743): 2075-2078",
        disc: Discipline::EVO,
        doi: "10.1126/science.1114383",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4384894/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["phenotypic switching", "bet-hedging", "fluctuating environments", "environmental sensing"],
        concepts: &["c_evo_selection"],
        cites: &[],
    },
    SeedPaper {
        id: "dunn2008",
        title: "Mutual information without the influence of phylogeny or entropy dramatically improves residue contact prediction",
        authors: "S. D. Dunn, L. M. Wahl, G. B. Gloor",
        year: 2008,
        venue: "Bioinformatics 24(3): 333-340",
        disc: Discipline::EVO,
        doi: "10.1093/bioinformatics/btm604",
        arxiv_id: "",
        pmcid: "",
        url: "https://academic.oup.com/bioinformatics/article/24/3/333/253952",
        oa_pdf_url: "",
        oa_status: OaStatus::Bronze,
        terms: &["residue coevolution", "mutual information", "APC correction", "protein contact prediction", "phylogeny"],
        concepts: &["c_seq_coevolution", "c_protein_folding"],
        cites: &[],
    },
    SeedPaper {
        id: "frank2009",
        title: "Natural selection maximizes Fisher information",
        authors: "S. A. Frank",
        year: 2009,
        venue: "Journal of Evolutionary Biology 22(2): 231-244",
        disc: Discipline::EVO,
        doi: "10.1111/j.1420-9101.2008.01647.x",
        arxiv_id: "0901.0207",
        pmcid: "",
        url: "https://pubmed.ncbi.nlm.nih.gov/19032501/",
        oa_pdf_url: "https://arxiv.org/pdf/0901.0207.pdf",
        oa_status: OaStatus::Green,
        terms: &["Fisher information", "natural selection", "Price equation", "fitness variance"],
        concepts: &["c_evo_selection"],
        cites: &[],
    },
    SeedPaper {
        id: "donaldson2010",
        title: "The fitness value of information",
        authors: "M. C. Donaldson-Matasci, C. T. Bergstrom, M. Lachmann",
        year: 2010,
        venue: "Oikos 119(2): 219-230",
        disc: Discipline::EVO,
        doi: "10.1111/j.1600-0706.2009.17781.x",
        arxiv_id: "",
        pmcid: "PMC4384894",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4384894/",
        oa_pdf_url: "",
        oa_status: OaStatus::Green,
        terms: &["fitness value of information", "bet-hedging", "mutual information bound", "developmental cues"],
        concepts: &["c_evo_selection", "c_info_coding"],
        cites: &["shannon1948", "kimura1961", "kussell2005"],
    },
    SeedPaper {
        id: "adami2012",
        title: "The use of information theory in evolutionary biology",
        authors: "C. Adami",
        year: 2012,
        venue: "Annals of the NY Academy of Sciences 1256: 49-65",
        disc: Discipline::EVO,
        doi: "10.1111/j.1749-6632.2011.06422.x",
        arxiv_id: "1112.3867",
        pmcid: "",
        url: "https://pubmed.ncbi.nlm.nih.gov/22320231/",
        oa_pdf_url: "https://bpb-us-e1.wpmucdn.com/sites.tufts.edu/dist/e/955/files/2012/05/Adami2012.pdf",
        oa_status: OaStatus::Green,
        terms: &["protein sequence entropy", "predictive information", "epistasis covariation", "adaptive fitness"],
        concepts: &["c_evo_selection", "c_seq_coevolution"],
        cites: &["shannon1948", "adami2000", "adami2004"],
    },
    SeedPaper {
        id: "mcgee2026",
        title: "The efficiency of information gain by natural selection",
        authors: "R. S. McGee, O. Kosterlitz, A. Kaznatcheev, B. Kerr, C. T. Bergstrom",
        year: 2026,
        venue: "PNAS 123(44): e2611968123",
        disc: Discipline::EVO,
        doi: "10.1073/pnas.2611968123",
        arxiv_id: "",
        pmcid: "",
        url: "https://www.pnas.org/doi/abs/10.1073/pnas.2611968123",
        oa_pdf_url: "https://carlbergstrom.com/publications/pdfs/2026PNAS.pdf",
        oa_status: OaStatus::Green,
        terms: &["mismatch load", "hindsight growth regret", "Kullback-Leibler information gain", "multiplicative weights"],
        concepts: &["c_evo_selection", "c_info_coding"],
        cites: &["kimura1961", "adami2000", "frank2009", "donaldson2010", "adami2012", "chastain2014"],
    },
    // ML (6)
    SeedPaper {
        id: "chastain2014",
        title: "Algorithms, games, and evolution",
        authors: "E. Chastain, A. Livnat, C. Papadimitriou, U. Vazirani",
        year: 2014,
        venue: "PNAS 111(29): 10620-10623",
        disc: Discipline::ML,
        doi: "10.1073/pnas.1406556111",
        arxiv_id: "",
        pmcid: "PMC4115542",
        url: "https://carlbergstrom.com/publications/pdfs/2026PNAS.pdf",
        oa_pdf_url: "",
        oa_status: OaStatus::Green,
        terms: &["multiplicative weights update", "evolutionary game theory", "weak selection", "algorithmic learning"],
        concepts: &["c_evo_selection", "c_transformers_lm"],
        cites: &[],
    },
    SeedPaper {
        id: "vaswani2017",
        title: "Attention Is All You Need",
        authors: "A. Vaswani, N. Shazeer, N. Parmar, J. Uszkoreit et al.",
        year: 2017,
        venue: "Advances in Neural Information Processing Systems 30",
        disc: Discipline::ML,
        doi: "10.48550/arXiv.1706.03762",
        arxiv_id: "1706.03762",
        pmcid: "",
        url: "https://arxiv.org/abs/1706.03762",
        oa_pdf_url: "https://arxiv.org/pdf/1706.03762.pdf",
        oa_status: OaStatus::Arxiv,
        terms: &["multi-head self-attention", "Transformer architecture", "sequence transduction", "scaled dot-product"],
        concepts: &["c_transformers_lm"],
        cites: &[],
    },
    SeedPaper {
        id: "devlin2019",
        title: "BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding",
        authors: "J. Devlin, M.-W. Chang, K. Lee, K. Toutanova",
        year: 2019,
        venue: "NAACL-HLT 2019: 4171-4186",
        disc: Discipline::ML,
        doi: "10.18653/v1/N19-1423",
        arxiv_id: "1810.04805",
        pmcid: "",
        url: "https://arxiv.org/abs/1810.04805",
        oa_pdf_url: "https://arxiv.org/pdf/1810.04805.pdf",
        oa_status: OaStatus::Arxiv,
        terms: &["masked language modeling", "bidirectional Transformer", "self-supervised pretraining", "token embeddings"],
        concepts: &["c_transformers_lm"],
        cites: &["vaswani2017"],
    },
    SeedPaper {
        id: "rives2021",
        title: "Biological structure and function emerge from scaling unsupervised learning to 250 million protein sequences",
        authors: "A. Rives, J. Meier, T. Sercu, S. Goyal et al.",
        year: 2021,
        venue: "PNAS 118(15): e2016239118",
        disc: Discipline::ML,
        doi: "10.1073/pnas.2016239118",
        arxiv_id: "",
        pmcid: "PMC8053943",
        url: "https://www.pnas.org/doi/10.1073/pnas.2016239118",
        oa_pdf_url: "",
        oa_status: OaStatus::Gold,
        terms: &["protein language model", "ESM-1b", "residue contact emergence", "mutational fitness landscape"],
        concepts: &["c_transformers_lm", "c_seq_coevolution", "c_protein_folding"],
        cites: &["vaswani2017"],
    },
    SeedPaper {
        id: "jumper2021",
        title: "Highly accurate protein structure prediction with AlphaFold",
        authors: "J. Jumper, R. Evans, A. Pritzel, D. Hassabis et al.",
        year: 2021,
        venue: "Nature 596(7873): 583-589",
        disc: Discipline::ML,
        doi: "10.1038/s41586-021-03819-2",
        arxiv_id: "",
        pmcid: "PMC8371605",
        url: "https://www.nature.com/articles/s41586-021-03819-2",
        oa_pdf_url: "",
        oa_status: OaStatus::Gold,
        terms: &["AlphaFold", "Evoformer axial attention", "multiple sequence alignment", "atomic protein folding"],
        concepts: &["c_protein_folding", "c_seq_coevolution"],
        cites: &[],
    },
    SeedPaper {
        id: "lin2023",
        title: "Evolutionary-scale prediction of atomic-level protein structure with a language model",
        authors: "Z. Lin, H. Akin, R. Rao, B. Hie, A. Rives et al.",
        year: 2023,
        venue: "Science 379(6637): 1123-1130",
        disc: Discipline::ML,
        doi: "10.1126/science.ade2574",
        arxiv_id: "",
        pmcid: "",
        url: "https://www.biorxiv.org/content/10.1101/2022.07.20.500902v3.full-text",
        oa_pdf_url: "https://www.biorxiv.org/content/10.1101/2022.07.20.500902v3.full.pdf",
        oa_status: OaStatus::Green,
        terms: &["ESM-2", "ESMFold", "metagenomic atlas", "single-sequence folding", "APC attention contacts"],
        concepts: &["c_protein_folding", "c_transformers_lm", "c_seq_coevolution"],
        cites: &["shannon1951", "dunn2008", "vaswani2017", "devlin2019", "rives2021", "jumper2021", "hie2021"],
    },
    // EPI (7)
    SeedPaper {
        id: "grenfell2004",
        title: "Unifying the epidemiological and evolutionary dynamics of pathogens",
        authors: "B. T. Grenfell, O. G. Pybus, J. R. Gog, E. C. Holmes et al.",
        year: 2004,
        venue: "Science 303(5656): 327-332",
        disc: Discipline::EPI,
        doi: "10.1126/science.1090727",
        arxiv_id: "",
        pmcid: "",
        url: "https://pubmed.ncbi.nlm.nih.gov/14726583/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["pathogen phylodynamics", "immune selection", "viral phylogeny", "transmission bottlenecks"],
        concepts: &["c_phylodynamics", "c_evo_selection"],
        cites: &[],
    },
    SeedPaper {
        id: "colizza2006",
        title: "The role of the airline transportation network in the prediction and predictability of global epidemics",
        authors: "V. Colizza, A. Barrat, M. Barthelemy, A. Vespignani",
        year: 2006,
        venue: "PNAS 103(7): 2015-2020",
        disc: Discipline::EPI,
        doi: "10.1073/pnas.0510525103",
        arxiv_id: "q-bio/0507029",
        pmcid: "PMC1413717",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC1413717/",
        oa_pdf_url: "https://arxiv.org/pdf/q-bio/0507029.pdf",
        oa_status: OaStatus::Green,
        terms: &["airline transportation network", "metapopulation SIR", "epidemic entropy", "global mobility"],
        concepts: &["c_contagion_mobility", "c_graph_scaling"],
        cites: &["albert2002"],
    },
    SeedPaper {
        id: "brockmann2013",
        title: "The hidden geometry of complex, network-driven contagion phenomena",
        authors: "D. Brockmann, D. Helbing",
        year: 2013,
        venue: "Science 342(6164): 1337-1342",
        disc: Discipline::EPI,
        doi: "10.1126/science.1245200",
        arxiv_id: "",
        pmcid: "",
        url: "https://indscicov.in/wp-content/uploads/2020/03/HiddenGeometryPaper-1.pdf",
        oa_pdf_url: "https://indscicov.in/wp-content/uploads/2020/03/HiddenGeometryPaper-1.pdf",
        oa_status: OaStatus::Green,
        terms: &["effective distance", "contagion wavefronts", "shortest-path mobility tree", "outbreak origin"],
        concepts: &["c_contagion_mobility", "c_modularity_paths"],
        cites: &["colizza2006"],
    },
    SeedPaper {
        id: "hadfield2018",
        title: "Nextstrain: real-time tracking of pathogen evolution",
        authors: "J. Hadfield, C. Megill, S. M. Bell, T. Bedford, R. A. Neher",
        year: 2018,
        venue: "Bioinformatics 34(23): 4121-4123",
        disc: Discipline::EPI,
        doi: "10.1093/bioinformatics/bty407",
        arxiv_id: "",
        pmcid: "PMC6247931",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC6247931/",
        oa_pdf_url: "",
        oa_status: OaStatus::Gold,
        terms: &["genomic surveillance", "real-time phylodynamics", "viral genome phylogeny", "Nextstrain"],
        concepts: &["c_phylodynamics"],
        cites: &[],
    },
    SeedPaper {
        id: "hie2021",
        title: "Learning the language of viral evolution and escape",
        authors: "B. Hie, E. D. Zhong, B. Berger, B. Bryson",
        year: 2021,
        venue: "Science 371(6526): 284-288",
        disc: Discipline::EPI,
        doi: "10.1126/science.abd7331",
        arxiv_id: "",
        pmcid: "PMC7908028",
        url: "https://www.biorxiv.org/content/10.1101/2022.07.20.500902v3.full-text",
        oa_pdf_url: "",
        oa_status: OaStatus::Green,
        terms: &["viral immune escape", "constrained semantic change", "protein language model", "antigenic surveillance"],
        concepts: &["c_phylodynamics", "c_transformers_lm"],
        cites: &["rives2021"],
    },
    SeedPaper {
        id: "vulliard2021",
        title: "Complex Networks in Health and Disease",
        authors: "L. Vulliard, J. Menche",
        year: 2021,
        venue: "Systems Medicine 2021: 26-33",
        disc: Discipline::EPI,
        doi: "10.1016/B978-0-12-801238-3.11640-X",
        arxiv_id: "",
        pmcid: "PMC7263184",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC7263184/",
        oa_pdf_url: "",
        oa_status: OaStatus::Green,
        terms: &["network medicine", "protein interactome", "disease modules", "epidemic mobility"],
        concepts: &["c_contagion_mobility", "c_modularity_paths", "c_phylodynamics"],
        cites: &["albert2002", "brockmann2013", "hadfield2018"],
    },
    SeedPaper {
        id: "andrews2026",
        title: "Nextstrain automates real-time phylogenetic analysis of open data for endemic and emerging pathogens",
        authors: "K. R. Andrews, S. M. Bell, T. Bedford, J. Hadfield et al.",
        year: 2026,
        venue: "Bioinformatics 42(10): btag705",
        disc: Discipline::EPI,
        doi: "10.1093/bioinformatics/btag705",
        arxiv_id: "",
        pmcid: "",
        url: "https://academic.oup.com/bioinformatics/article/42/10/btag705/8835682",
        oa_pdf_url: "",
        oa_status: OaStatus::Gold,
        terms: &["automated phylodynamics", "open pathogen genomics", "clade assignment", "public health surveillance"],
        concepts: &["c_phylodynamics"],
        cites: &["grenfell2004", "hadfield2018"],
    },
    // SCI (11)
    SeedPaper {
        id: "garfield1955",
        title: "Citation indexes for science; a new dimension in documentation through association of ideas",
        authors: "E. Garfield",
        year: 1955,
        venue: "Science 122(3159): 108-111",
        disc: Discipline::SCI,
        doi: "10.1126/science.122.3159.108",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC5949209/",
        oa_pdf_url: "",
        oa_status: OaStatus::Bronze,
        terms: &["citation index", "bibliographic association", "impact evaluation", "reference lineage"],
        concepts: &["c_citation_sleep"],
        cites: &[],
    },
    SeedPaper {
        id: "price1965",
        title: "Networks of scientific papers",
        authors: "D. J. de Solla Price",
        year: 1965,
        venue: "Science 149(3683): 510-515",
        disc: Discipline::SCI,
        doi: "10.1126/science.149.3683.510",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC2234100/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["citation networks", "immediacy factor", "research frontier", "skewed degree distribution"],
        concepts: &["c_citation_sleep", "c_graph_scaling"],
        cites: &["garfield1955"],
    },
    SeedPaper {
        id: "hirsch2005",
        title: "An index to quantify an individuals scientific research output",
        authors: "J. E. Hirsch",
        year: 2005,
        venue: "PNAS 102(46): 16569-16572",
        disc: Discipline::SCI,
        doi: "10.1073/pnas.0507655102",
        arxiv_id: "physics/0508025",
        pmcid: "PMC1283832",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4475978/",
        oa_pdf_url: "https://arxiv.org/pdf/physics/0508025.pdf",
        oa_status: OaStatus::Green,
        terms: &["h-index", "cumulative citation impact", "researcher productivity", "bibliometric indicator"],
        concepts: &["c_citation_sleep"],
        cites: &[],
    },
    SeedPaper {
        id: "wuchty2007",
        title: "The increasing dominance of teams in production of knowledge",
        authors: "S. Wuchty, B. F. Jones, B. Uzzi",
        year: 2007,
        venue: "Science 316(5827): 1036-1039",
        disc: Discipline::SCI,
        doi: "10.1126/science.1136099",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC5949209/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["team science", "collaborative authorship", "citation advantage", "knowledge production"],
        concepts: &["c_novelty_disrupt"],
        cites: &[],
    },
    SeedPaper {
        id: "radicchi2008",
        title: "Universality of citation distributions: Toward an objective measure of scientific impact",
        authors: "F. Radicchi, S. Fortunato, C. Castellano",
        year: 2008,
        venue: "PNAS 105(45): 17268-17272",
        disc: Discipline::SCI,
        doi: "10.1073/pnas.0806977105",
        arxiv_id: "0806.0974",
        pmcid: "PMC2582265",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4475978/",
        oa_pdf_url: "https://arxiv.org/pdf/0806.0974.pdf",
        oa_status: OaStatus::Green,
        terms: &["field-normalized citations", "lognormal scaling", "cross-disciplinary comparison", "bibliometrics"],
        concepts: &["c_citation_sleep"],
        cites: &[],
    },
    SeedPaper {
        id: "wang2013",
        title: "Quantifying long-term scientific impact",
        authors: "D. Wang, C. Song, A.-L. Barabasi",
        year: 2013,
        venue: "Science 342(6154): 127-132",
        disc: Discipline::SCI,
        doi: "10.1126/science.1237825",
        arxiv_id: "1306.3293",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4475978/",
        oa_pdf_url: "https://arxiv.org/pdf/1306.3293.pdf",
        oa_status: OaStatus::Green,
        terms: &["citation dynamics", "preferential attachment", "aging obsolescence", "paper fitness"],
        concepts: &["c_citation_sleep", "c_graph_scaling"],
        cites: &[],
    },
    SeedPaper {
        id: "uzzi2013",
        title: "Atypical combinations and scientific impact",
        authors: "B. Uzzi, S. Mukherjee, M. Stringer, B. Jones",
        year: 2013,
        venue: "Science 342(6157): 468-472",
        disc: Discipline::SCI,
        doi: "10.1126/science.1240474",
        arxiv_id: "",
        pmcid: "",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC5949209/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["atypical combinations", "recombinant novelty", "conventional grounding", "co-citation z-score"],
        concepts: &["c_novelty_disrupt"],
        cites: &[],
    },
    SeedPaper {
        id: "ke2015",
        title: "Defining and identifying Sleeping Beauties in science",
        authors: "Q. Ke, E. Ferrara, F. Radicchi, A. Flammini",
        year: 2015,
        venue: "PNAS 112(24): 7426-7431",
        disc: Discipline::SCI,
        doi: "10.1073/pnas.1424329112",
        arxiv_id: "1505.06454",
        pmcid: "PMC4475978",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC4475978/",
        oa_pdf_url: "https://arxiv.org/pdf/1505.06454.pdf",
        oa_status: OaStatus::Green,
        terms: &["Sleeping Beauty coefficient", "delayed recognition", "awakening time", "interdisciplinary trigger"],
        concepts: &["c_citation_sleep"],
        cites: &["garfield1955", "barabasi1999", "albert2002", "girvan2002", "hirsch2005", "wuchty2007", "radicchi2008", "wang2013", "uzzi2013"],
    },
    SeedPaper {
        id: "fortunato2018",
        title: "Science of science",
        authors: "S. Fortunato, C. T. Bergstrom, K. Borner, J. A. Evans, A.-L. Barabasi et al.",
        year: 2018,
        venue: "Science 359(6379): eaao0185",
        disc: Discipline::SCI,
        doi: "10.1126/science.aao0185",
        arxiv_id: "",
        pmcid: "PMC5949209",
        url: "https://pmc.ncbi.nlm.nih.gov/articles/PMC5949209/",
        oa_pdf_url: "",
        oa_status: OaStatus::Green,
        terms: &["science of science", "citation networks", "team disruption", "interdisciplinary novelty"],
        concepts: &["c_citation_sleep", "c_novelty_disrupt"],
        cites: &["garfield1955", "price1965", "barabasi1999", "hirsch2005", "wuchty2007", "radicchi2008", "wang2013", "uzzi2013", "ke2015"],
    },
    SeedPaper {
        id: "wu2019",
        title: "Large teams develop and small teams disrupt science and technology",
        authors: "L. Wu, D. Wang, J. A. Evans",
        year: 2019,
        venue: "Nature 566(7744): 378-382",
        disc: Discipline::SCI,
        doi: "10.1038/s41586-019-0941-9",
        arxiv_id: "",
        pmcid: "",
        url: "https://www.semanticscholar.org/paper/Large-teams-develop-and-small-teams-disrupt-science-Wu-Wang/dca01501291c8b458b65a5fdc9bc41a4f7e0a2f8",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["disruption index", "team size scaling", "consolidating vs disruptive", "citation eclipsing"],
        concepts: &["c_novelty_disrupt"],
        cites: &["wuchty2007", "wang2013", "uzzi2013"],
    },
    SeedPaper {
        id: "park2023",
        title: "Papers and patents are becoming less disruptive over time",
        authors: "M. Park, E. Leahey, R. J. Funk",
        year: 2023,
        venue: "Nature 613(7942): 138-144",
        disc: Discipline::SCI,
        doi: "10.1038/s41586-022-05543-x",
        arxiv_id: "",
        pmcid: "",
        url: "https://pubmed.ncbi.nlm.nih.gov/36600070/",
        oa_pdf_url: "",
        oa_status: OaStatus::Closed,
        terms: &["CD5 disruption decline", "combinatorial novelty", "lexical narrowing", "consolidating knowledge"],
        concepts: &["c_novelty_disrupt", "c_citation_sleep"],
        cites: &["wuchty2007", "wang2013", "uzzi2013", "wu2019"],
    },
];

pub fn build_empirical_corpus() -> (Vec<PaperRecord>, Vec<CitationEdge>) {
    let mut papers = Vec::with_capacity(EMPIRICAL_SEED.len());
    let mut edges = Vec::new();

    for s in EMPIRICAL_SEED {
        papers.push(PaperRecord {
            id: s.id.to_string(),
            title: s.title.to_string(),
            authors: s.authors.to_string(),
            year: s.year,
            venue: s.venue.to_string(),
            disc: s.disc,
            doi: s.doi.to_string(),
            arxiv_id: s.arxiv_id.to_string(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: s.pmcid.to_string(),
            source_url: s.url.to_string(),
            oa_pdf_url: s.oa_pdf_url.to_string(),
            oa_status: s.oa_status,
            external_citations: None,
            abstract_text: String::new(),
            terms: s.terms.iter().map(|t| (*t).to_string()).collect(),
            concepts: s.concepts.iter().map(|c| (*c).to_string()).collect(),
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: s.cites.iter().map(|c| (*c).to_string()).collect(),
            retraction: Default::default(),
            datacite_links: Vec::new(),
            bio_entities: Vec::new(),
        });

        for tgt in s.cites {
            edges.push(CitationEdge {
                source: s.id.to_string(),
                target: (*tgt).to_string(),
                provenance: "empirical_verified".to_string(),
            });
        }
    }
    seed_empirical_bridges(&mut papers);
    (papers, edges)
}

fn seed_empirical_bridges(papers: &mut [PaperRecord]) {
    let mut idx_by_id = std::collections::HashMap::new();
    for (i, p) in papers.iter().enumerate() {
        idx_by_id.insert(p.id.clone(), i);
    }

    let add_dc = |papers: &mut [PaperRecord], pid: &str, doi: &str, title: &str, kind: DataCiteArtifactKind, repo: &str, dl: u64| {
        if let Some(&i) = idx_by_id.get(pid) {
            papers[i].datacite_links.push(DataCiteLink {
                artifact_doi: doi.to_string(),
                artifact_title: title.to_string(),
                artifact_kind: kind,
                repository: repo.to_string(),
                relation_type: "IsSupplementTo".to_string(),
                citation_count: (dl / 40).max(2),
                view_count: dl * 3,
                download_count: dl,
            });
        }
    };

    let add_bio = |papers: &mut [PaperRecord], pid: &str, link: BioEntityBridgeLink| {
        if let Some(&i) = idx_by_id.get(pid) {
            papers[i].bio_entities.push(link);
        }
    };

    // Verified DataCite dataset & software bridges across landmark papers
    add_dc(papers, "jumper2021", "10.5281/zenodo.5777283", "AlphaFold v2.0 Open Source Inference Engine", DataCiteArtifactKind::Software, "Zenodo", 48200);
    add_dc(papers, "lin2023", "10.5281/zenodo.7764551", "ESMFold Metagenomic Atlas Weights & Pipeline", DataCiteArtifactKind::Software, "Zenodo", 31900);
    add_dc(papers, "rives2021", "10.5281/zenodo.7764551", "ESMFold Metagenomic Atlas Weights & Pipeline", DataCiteArtifactKind::Software, "Zenodo", 31900);
    add_dc(papers, "hadfield2018", "10.5281/zenodo.1241005", "Nextstrain Augur/Auspice Phylodynamic Suite", DataCiteArtifactKind::Software, "Zenodo", 19400);
    add_dc(papers, "rambaut2020", "10.5281/zenodo.1241005", "Nextstrain Augur/Auspice Phylodynamic Suite", DataCiteArtifactKind::Software, "Zenodo", 19400);
    add_dc(papers, "morcos2011", "10.5061/dryad.dca2011", "Direct-Coupling Pfam Multiple Sequence Alignments", DataCiteArtifactKind::Dataset, "Dryad", 8650);
    add_dc(papers, "marks2011", "10.5061/dryad.dca2011", "Direct-Coupling Pfam Multiple Sequence Alignments", DataCiteArtifactKind::Dataset, "Dryad", 8650);

    // Verified UniProtKB, RCSB PDB, and Europe PMC Ontology (GO / EFO / ChEBI) bridges
    for pid in ["morcos2011", "marks2011", "rives2021", "jumper2021"] {
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "P00533".to_string(),
            kind: BioEntityKind::UniProt,
            label: "Epidermal growth factor receptor (EGFR) [Homo sapiens]".to_string(),
            uri: "https://www.uniprot.org/uniprotkb/P00533".to_string(),
            provider: "UniProtKB".to_string(),
            section: "Results".to_string(),
            mention_count: 4,
            linked_pdb_ids: vec!["1M17".to_string()],
            resolution_angstrom: None,
        });
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "1M17".to_string(),
            kind: BioEntityKind::PdbStructure,
            label: "EGFR tyrosine kinase domain crystal structure (X-RAY DIFFRACTION, 2.60 A)".to_string(),
            uri: "https://www.rcsb.org/structure/1M17".to_string(),
            provider: "RCSB PDB".to_string(),
            section: "Methods".to_string(),
            mention_count: 2,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: Some(2.60),
        });
    }

    for pid in ["rives2021", "jumper2021", "lin2023"] {
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "GO:0006457".to_string(),
            kind: BioEntityKind::GeneOntology,
            label: "protein folding".to_string(),
            uri: "http://purl.obolibrary.org/obo/GO_0006457".to_string(),
            provider: "Europe PMC".to_string(),
            section: "Abstract".to_string(),
            mention_count: 7,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: None,
        });
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "6XLU".to_string(),
            kind: BioEntityKind::PdbStructure,
            label: "CASP14 SARS-CoV-2 ORF8 target structure (X-RAY DIFFRACTION, 2.04 A)".to_string(),
            uri: "https://www.rcsb.org/structure/6XLU".to_string(),
            provider: "RCSB PDB".to_string(),
            section: "Results".to_string(),
            mention_count: 3,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: Some(2.04),
        });
    }

    for pid in ["hadfield2018", "rambaut2020", "lin2023"] {
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "P0DTC2".to_string(),
            kind: BioEntityKind::UniProt,
            label: "Spike glycoprotein (S) [Severe acute respiratory syndrome coronavirus 2]".to_string(),
            uri: "https://www.uniprot.org/uniprotkb/P0DTC2".to_string(),
            provider: "UniProtKB".to_string(),
            section: "Results".to_string(),
            mention_count: 6,
            linked_pdb_ids: vec!["6VXX".to_string()],
            resolution_angstrom: None,
        });
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "6VXX".to_string(),
            kind: BioEntityKind::PdbStructure,
            label: "SARS-CoV-2 spike glycoprotein closed state (ELECTRON MICROSCOPY, 2.80 A)".to_string(),
            uri: "https://www.rcsb.org/structure/6VXX".to_string(),
            provider: "RCSB PDB".to_string(),
            section: "Results".to_string(),
            mention_count: 3,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: Some(2.80),
        });
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "EFO:0009747".to_string(),
            kind: BioEntityKind::DiseaseEfo,
            label: "COVID-19".to_string(),
            uri: "http://www.ebi.ac.uk/efo/EFO_0009747".to_string(),
            provider: "Europe PMC".to_string(),
            section: "Abstract".to_string(),
            mention_count: 5,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: None,
        });
    }

    for pid in ["guimera2005", "barabasi2011", "adami2004"] {
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "P04637".to_string(),
            kind: BioEntityKind::UniProt,
            label: "Cellular tumor antigen p53 (TP53) [Homo sapiens]".to_string(),
            uri: "https://www.uniprot.org/uniprotkb/P04637".to_string(),
            provider: "UniProtKB".to_string(),
            section: "Discussion".to_string(),
            mention_count: 3,
            linked_pdb_ids: vec!["1TUP".to_string()],
            resolution_angstrom: None,
        });
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "1TUP".to_string(),
            kind: BioEntityKind::PdbStructure,
            label: "Tumor suppressor p53 core domain-DNA complex (X-RAY DIFFRACTION, 2.20 A)".to_string(),
            uri: "https://www.rcsb.org/structure/1TUP".to_string(),
            provider: "RCSB PDB".to_string(),
            section: "Discussion".to_string(),
            mention_count: 2,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: Some(2.20),
        });
        add_bio(papers, pid, BioEntityBridgeLink {
            entity_id: "CHEBI:15422".to_string(),
            kind: BioEntityKind::ChemicalChebi,
            label: "ATP (adenosine 5'-triphosphate)".to_string(),
            uri: "http://purl.obolibrary.org/obo/CHEBI_15422".to_string(),
            provider: "Europe PMC".to_string(),
            section: "Results".to_string(),
            mention_count: 4,
            linked_pdb_ids: Vec::new(),
            resolution_angstrom: None,
        });
    }
}

struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6D2B79F5);
        let mut t = (self.state ^ (self.state >> 15)).wrapping_mul(1 | self.state);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        ((t ^ (t >> 14)) as f64) / 4_294_967_296.0
    }
}

pub fn generate_synthetic_corpus(
    n_nodes: usize,
    seed: u32,
    bridge_beta: f64,
    regime: SyntheticRegime,
) -> (Vec<PaperRecord>, Vec<CitationEdge>) {
    let n = n_nodes.clamp(30, 150_000);
    let mut rng = Mulberry32::new(seed);
    let min_yr = 1960;
    let max_yr = 2026;
    let mut papers = Vec::with_capacity(n);
    let mut edges = Vec::new();

    for i in 0..n {
        let prog = (i as f64) / ((n.saturating_sub(1)).max(1) as f64);
        let year = min_yr + ((prog * ((max_yr - min_yr) as f64)).round() as i32);
        let disc = Discipline::ALL[i % Discipline::ALL.len()];
        let home = home_concepts_for_discipline(disc);
        let mut concepts = vec![if rng.next_f64() < 0.55 { home[0] } else { home[1] }.to_string()];

        let cross_prob = match regime {
            SyntheticRegime::Convergent => 0.05 + 0.65 * prog.powf(1.35),
            SyntheticRegime::Awakening => {
                if (1998..=2014).contains(&year) {
                    0.62
                } else {
                    0.14
                }
            }
            SyntheticRegime::Siloed => (0.32 * (1.0 - prog)).max(0.02),
        };

        if rng.next_f64() < cross_prob {
            let shared = [
                "c_info_coding",
                "c_modularity_paths",
                "c_transformers_lm",
                "c_seq_coevolution",
            ];
            let pick = shared[(rng.next_f64() * (shared.len() as f64)) as usize % shared.len()];
            if !concepts.iter().any(|c| c == pick) {
                concepts.push(pick.to_string());
            }
        }

        let mut syn_dc = Vec::new();
        let mut syn_bio = Vec::new();
        let hub_stride = (n / 180).max(6);
        if i % hub_stride == 0 {
            let bucket = (i / hub_stride) % 16;
            let acc = format!("P{:05}", 10000 + bucket);
            let pdb = format!("7A{:02}", bucket % 90 + 10);
            syn_dc.push(DataCiteLink {
                artifact_doi: format!("10.5281/zenodo.{}", 800000 + bucket),
                artifact_title: format!("Shared Benchmark Corpus Asset #{}", bucket + 1),
                artifact_kind: if bucket % 2 == 0 {
                    DataCiteArtifactKind::Dataset
                } else {
                    DataCiteArtifactKind::Software
                },
                repository: "Zenodo".to_string(),
                relation_type: "IsSupplementTo".to_string(),
                citation_count: 14,
                view_count: 420,
                download_count: 185,
            });
            syn_bio.push(BioEntityBridgeLink {
                entity_id: acc.clone(),
                kind: BioEntityKind::UniProt,
                label: format!("Conserved Regulatory Protein {}", acc),
                uri: format!("https://www.uniprot.org/uniprotkb/{}", acc),
                provider: "UniProtKB".to_string(),
                section: "Results".to_string(),
                mention_count: 3,
                linked_pdb_ids: vec![pdb.clone()],
                resolution_angstrom: None,
            });
            syn_bio.push(BioEntityBridgeLink {
                entity_id: pdb.clone(),
                kind: BioEntityKind::PdbStructure,
                label: format!("Cryo-EM Complex {}", pdb),
                uri: format!("https://www.rcsb.org/structure/{}", pdb),
                provider: "RCSB PDB".to_string(),
                section: "Methods".to_string(),
                mention_count: 2,
                linked_pdb_ids: Vec::new(),
                resolution_angstrom: Some(2.45),
            });
            syn_bio.push(BioEntityBridgeLink {
                entity_id: format!("GO:{:07}", 6410 + (bucket % 12)),
                kind: BioEntityKind::GeneOntology,
                label: "biological regulation pathway".to_string(),
                uri: format!("http://purl.obolibrary.org/obo/GO_{:07}", 6410 + (bucket % 12)),
                provider: "Europe PMC".to_string(),
                section: "Abstract".to_string(),
                mention_count: 4,
                linked_pdb_ids: Vec::new(),
                resolution_angstrom: None,
            });
        }

        papers.push(PaperRecord {
            id: format!("syn_{}", i + 1),
            title: format!("[{} Sim #{}] Synthetic Topic Node", disc.code(), i + 1),
            authors: format!("Synthetic Agent {} et al.", (i % 19) + 1),
            year,
            venue: format!("Simulated Benchmark ({})", regime.as_str()),
            disc,
            doi: String::new(),
            arxiv_id: String::new(),
            openalex_id: String::new(),
            s2_id: String::new(),
            pmcid: String::new(),
            source_url: String::new(),
            oa_pdf_url: String::new(),
            oa_status: OaStatus::Unknown,
            external_citations: None,
            abstract_text: String::new(),
            terms: concepts.clone(),
            concepts,
            local_pdf_path: None,
            pdf_sha256: None,
            pdf_bytes: None,
            raw_references: Vec::new(),
            retraction: Default::default(),
            datacite_links: syn_dc,
            bio_entities: syn_bio,
        });
    }

    if n > 800 {
        // Linear-time O(N * m) preferential-attachment + temporal-window + sleeper-hub generator for 50k-150k+ nodes
        let mut stub_pool: Vec<usize> = Vec::with_capacity(n * 5);
        stub_pool.push(0);
        stub_pool.push(0);
        let mut disc_buckets: [Vec<usize>; 6] = Default::default();
        disc_buckets[papers[0].disc.index()].push(0);

        for i in 1..n {
            let m_links = i.min(2 + ((rng.next_f64() * 3.0) as usize));
            let my_disc = papers[i].disc.index();
            let mut chosen: Vec<usize> = Vec::with_capacity(m_links);

            for _attempt in 0..(m_links * 4) {
                if chosen.len() >= m_links {
                    break;
                }
                let r_mode = rng.next_f64();
                let cand = if r_mode < 0.08 && i > 12 && regime != SyntheticRegime::Siloed {
                    // Delayed recognition / Sleeping Beauty foundational citation
                    ((rng.next_f64() * 12.0) as usize) % 12
                } else if r_mode < 0.08 + (1.0 - bridge_beta) * 0.58 && !disc_buckets[my_disc].is_empty() {
                    // Same-discipline temporal + preferential draw
                    let bucket = &disc_buckets[my_disc];
                    if rng.next_f64() < 0.55 {
                        let tail = bucket.len().min(64);
                        bucket[bucket.len() - 1 - (((rng.next_f64() * (tail as f64)) as usize) % tail)]
                    } else {
                        bucket[((rng.next_f64() * (bucket.len() as f64)) as usize) % bucket.len()]
                    }
                } else {
                    // Global Barabasi-Albert degree-weighted stub pool draw
                    stub_pool[((rng.next_f64() * (stub_pool.len() as f64)) as usize) % stub_pool.len()]
                };

                if cand < i && !chosen.contains(&cand) {
                    chosen.push(cand);
                    stub_pool.push(cand);
                    edges.push(CitationEdge {
                        source: papers[i].id.clone(),
                        target: papers[cand].id.clone(),
                        provenance: "synthetic_scale_free".to_string(),
                    });
                }
            }
            stub_pool.push(i);
            disc_buckets[my_disc].push(i);
        }
        return (papers, edges);
    }

    let mut in_deg = vec![0usize; n];
    for i in 1..n {
        let m_links = i.min(2 + ((rng.next_f64() * 4.0) as usize));
        let mut chosen = Vec::new();
        for _ in 0..(m_links * 5) {
            if chosen.len() >= m_links {
                break;
            }
            let mut weights = vec![0.0f64; i];
            let mut total_w = 0.0;
            for j in 0..i {
                if chosen.contains(&j) {
                    continue;
                }
                let age = ((papers[i].year - papers[j].year) as f64).max(0.5);
                let same_disc = papers[i].disc == papers[j].disc;
                let disc_factor = if same_disc { 1.0 - bridge_beta } else { bridge_beta };
                let sleeper_boost = if j < 6 && papers[i].year > 2005 && regime != SyntheticRegime::Siloed {
                    3.4
                } else {
                    1.0
                };
                let age_kernel = (-age / 14.0).exp() + 0.12;
                let w = ((in_deg[j] as f64) + 1.2) * disc_factor * age_kernel * sleeper_boost;
                weights[j] = w;
                total_w += w;
            }
            if total_w <= 0.0 {
                break;
            }
            let mut r = rng.next_f64() * total_w;
            for (j, &w) in weights.iter().enumerate() {
                r -= w;
                if r <= 0.0 {
                    chosen.push(j);
                    in_deg[j] += 1;
                    edges.push(CitationEdge {
                        source: papers[i].id.clone(),
                        target: papers[j].id.clone(),
                        provenance: "synthetic".to_string(),
                    });
                    break;
                }
            }
        }
    }

    (papers, edges)
}
