# CitAtlas User Guide (`cit-atlas` v1.8.0)

CitAtlas is a standalone native Rust scientometrics and citation-network workstation (`#![forbid(unsafe_code)]` library core, `eframe`/`egui` UI, `wgpu` 24.0 GPU graph engine, and `clap` CLI).

---

## 1. Workbench Tabs (`1` – `9`)

Press keys `1` through `9` (whenever no text input field is focused) or click a tab in the top navigation ribbon:

1. **Literature Search (Publish or Perish)** — Multi-provider query across OpenAlex, Semantic Scholar, Crossref, arXiv, OpenAIRE Graph, INSPIRE-HEP, NASA ADS, DataCite, PubMed, Europe PMC, DBLP, bioRxiv/medRxiv, and polite Google Scholar HTML parsing. Computes Publish-or-Perish citation tables (`h`, `g`, `hc`, `hI,norm`, `hA`), single-DOI lookups, and batch Open-Access PDF downloads.
2. **Citation Graph Viewer** — Interactive GPU (`wgpu` WGSL) or CPU fallback network canvas supporting 50,000+ nodes, 5 layout presets (`ForceAtlas2 Physics`, `Chronological DAG`, `Community Galaxies`, `Radial Ego Rings`, `Cartographic (P, z)`), node color/size encodings, ego spotlights (`All`, `1-Hop`, `2-Hop`, `References`, `Citers`), bipartite DataCite dataset/software diamonds, and UniProt/PDB/Europe PMC bio-entity nodes.
3. **Observatory** — Corpus-level overview cards, discipline breakdown, paper ranking table, and focal paper inspection.
4. **Velocity & Awakening** — Multi-paper comparison plots for exponential decay velocity $v_{\text{exp}}$, sliding-window velocity $v_{\text{win}}$, acceleration, cumulative citations, and Ke et al. (2015) Sleeping Beauty geometry ($B$ and awakening year $t_a$).
5. **Bridges (Cartography)** — Guimerà–Amaral $(z_i, P_i)$ role cartography (`R1`–`R7`), Burt structural hole constraint $C_i$, Girvan–Newman edge-betweenness conduits, and shortest epistemic geodesic path finder.
6. **Louvain & Infomap** — Multi-level Louvain modularity $Q(\gamma)$ vs. two-level Infomap flow compression $L(M)$, confusion/contingency matrices against fixed disciplines, NMI/VI/ARI alignment, and reclassified bridge-paper lists.
7. **MI & JSD (Topic Drift)** — Sliding time-window Mutual Information $I_w(D; T)$, normalized MI, discipline-concept PMI, and pairwise Jensen–Shannon divergence drift curves.
8. **SQLite & CSV** — Persistent `LedgerStore` (`.db`/`.sqlite`) load/save, multi-format corpus import (`.json`, `.csl`, `.bib`, `.ris`, `.nbib`, `.csv`, `.html`, `.graphml`, `.gexf`), synthetic benchmark generator (30–150,000 nodes), Retraction Watch screening, and 10-table RFC 4180 CSV + SVG/GraphML/GEXF export.
9. **Rate Audit** — Live editor and telemetry table for all 24 pre-configured API hosts (`refill_rps`, `burst_capacity`, `min_spacing_ms`, pause/circuit-breaker status, and audit log filtering).

---

## 2. Global Controls, Hotkeys & Empty-State Recovery

### Top Ribbon Parameters (Debounced)
- **`W (yr)`** (`3..=25`), **`Stride S`** (`1..=10`), **`Half-life tau`** (`2.0..=20.0`), and **`Horizon`** (`1965..=2026`): Recompute corpus analytics strictly upon `drag_stopped()` or `lost_focus()` so dragging sliders over large networks never stalls the UI thread.
- **`Hide Retracted`**: Immediately excludes retracted or withdrawn papers when toggled.
- **`Filter:`** (`Ctrl+F` / `Cmd+F`) and discipline pills (`ALL`, `INF`, `NET`, `EVO`, `ML`, `EPI`, `SCI`): Filter tables and views dynamically.

### Graph Viewport Controls (Tab 2)
- **`Space`**: Start/pause the ForceAtlas2 physics kernel (`Step` advances one frame manually).
- **`0` / `R`** or **`Fit View`**: Computes the bounding box of active paper coordinates and fits camera `pan` and `zoom` uniformly to the viewport.
- **Mouse Wheel**: Cursor-centered zoom (`0.18x` to `12.0x`).
- **Drag Node / Drag Background**: Reposition and temporarily pin an individual node during simulation, or pan the camera.
- **Click Node**: Opens the right-hand **Paper Dossier** (`Escape` closes the dossier or About dialog).
- **Empty-State Diagnostic (`n == 0`)**: If `Horizon` or `Hide Retracted` leaves zero active papers, the graph viewport displays an explanatory count card with a 1-click **`Reset Filters`** button that restores default `Horizon`, clears filters, and fits the restored network.

---

## 3. Common CLI Workflows

### Search, Screen, and Export 10 CSV Tables
```bash
cit-atlas search "protein language model" \
  --providers openalex,s2,crossref,arxiv \
  --limit 40 \
  --snowball-hops 1 \
  --db ./corpus.db \
  --export-csv-dir ./cit_atlas_csv \
  --export-graphml ./network.graphml \
  --export-gexf ./network.gexf
```

### Offline Scientometric Analysis & Community Partitioning
```bash
cit-atlas analyze --from-db ./corpus.db --window 10 --stride 5 --half-life 6.0 --horizon 2026 --export-json ./analysis.json --export-svg ./synthesis.svg
cit-atlas communities --from-db ./corpus.db --resolution 1.0 --teleport 0.15 --export-csv ./communities.csv
```

### 50,000-Node Synthetic Benchmark & Rate-Limit Dry Run
```bash
cit-atlas benchmark --nodes 50000 --seed 2026 --bridge-beta 0.18 --regime convergent --export-json ./bench50k.json
cit-atlas rate-limits --rate-scale 0.8 --simulate-burst --export-policies-csv ./policies.csv --export-audit-csv ./audit.csv
```

### Offscreen Headless Screenshot
```bash
cit-atlas --screenshot ./preview.png --screenshot-view 2
```
