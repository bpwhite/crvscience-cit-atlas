# CitAtlas Scientific Methods & Numerical Architecture (`cit-atlas` v1.8.0)

This reference documents the bibliometric estimators, graph clustering algorithms, and GPU simulation/rendering equations implemented in `crvscience-cit-atlas`.

---

## 1. Stratified Horvitz–Thompson Brandes Betweenness (`src/analytics.rs`)

For networks with $n \le 1{,}500$, `run_parallel_brandes` evaluates exact Brandes (2001) shortest-path accumulation from all $n$ sources in parallel via `rayon`.

For larger graphs ($n > 1{,}500$, sample budget $K = 192$), the solver constructs a stratified Horvitz–Thompson pivot sample:
1. Sort vertices by descending degree $(d(v), -v)$ and take the top $H = \min(64, n)$ hubs deterministically with inclusion probability $\pi_s = 1.0$, assigning weight:
   $$w_s = 1.0 \quad (s \in S_{\text{hub}}).$$
2. Sample $m = \min(K - H,\, n - H)$ non-hub pivots by centered systematic stratification over the remaining $n - H$ vertices in degree-ordered strata, assigning exact inverse-probability Horvitz–Thompson weight:
   $$w_s = \frac{1}{\pi_s} = \frac{n - H}{K - H} \quad (s \in S_{\text{non-hub}}).$$
3. Vertex dependencies $\delta_{s\bullet}(v)$ and edge dependencies $\delta_{s\bullet}(e)$ are weighted by $w_s$ before normalizing undirected vertex betweenness by $\binom{n - 1}{2}$ (or $(n - 1)(n - 2)$ for directed DAGs). This eliminates the systemic upward bias on hub-dominated paths caused by uniform $n/K$ scaling across deterministic hubs.

---

## 2. Directed Infomap Flow Compression (`src/community_detection.rs`)

To preserve directional research-front flow without collapsing stationary probability onto terminal reference roots, `build_infomap_rw_transitions` builds the random-walk transition graph strictly from `out_adj` and its exact transpose `in_adj`:
- Forward citation edge $(u \to v) \in E$: weight $+1.0$.
- Reverse back-flow edge $(v \to u)$ for $(u \to v) \in E$: weight $+0.25$.

This guarantees an exact $1.0 : 0.25$ ($4 : 1$) forward-to-backward transition ratio along directed chains $A \to B \to C$ (eliminating undirected double-counting on forward arcs). Stationary visit frequencies $p_i$ are solved via damped PageRank power iteration (teleportation rate $\tau \in [0.01, 0.85]$, tolerance $\|p^{(t+1)} - p^{(t)}\|_1 < 10^{-13}$), and modules $M$ minimize the two-level Rosvall–Bergstrom Map Equation:
$$L(M) = q_{\curvearrowright} H(\mathcal{Q}) + \sum_{m=1}^{|M|} p_{\circlearrowright}^m H(\mathcal{P}^m),$$
with an incremental $O(\deg(u))$ accumulator solver for $n > 500$ seeded from Louvain super-clusters.

---

## 3. Longitudinal Citation Dynamics, Burt Constraint, and Disruption

### Ke et al. (2015) Sleeping Beauty Coefficient $B$ & Awakening Year $t_a$
For paper $i$ published in year $t_{\text{pub}}$, annual citation counts $c_t$ at age $t = t_{\text{citer}} - t_{\text{pub}}$ strictly exclude unknown-year records ($t_{\text{citer}} \le 0$), pre-publication citations ($t_{\text{citer}} < t_{\text{pub}}$), and post-horizon citations ($t_{\text{citer}} > t_{\text{horizon}}$), ensuring $c_0$ reflects only publication-year citations. Given peak count $c_{t_m}$ at age $t_m$:
$$B = \max\!\left(0,\; \sum_{t=0}^{t_m} \frac{\ell(t) - c_t}{\max(c_t, 1)}\right), \qquad \ell(t) = c_0 + \frac{c_{t_m} - c_0}{t_m} t,$$
and $t_a = t_{\text{pub}} + \arg\max_{0 \le t < t_m} d_{\perp}(t, c_t; \ell)$.

### Exact Burt (1992) Structural Hole Constraint $C_i$
On the undirected citation neighborhood $N(i)$ of degree $k_i$ ($p_{ij} = 1/k_i$):
$$C_i = \sum_{j \in N(i)} \left(p_{ij} + \sum_{q \in N(i) \setminus \{j\},\; (q, j) \in E} \frac{1}{k_i k_q}\right)^2.$$
Indirect sums are accumulated in $O\!\left(\sum_{q \in N(i)} \min(k_i, k_q)\right)$ without any artificial $1.25$ upper clamp or heuristic hub approximation (falling back to deterministic stratified neighbor sampling only when both $k_i > 512$ and work exceeds $250{,}000$ checks).

### Disruption Index ($CD, CD_5$), HITS, and Topic Drift
- **Disruption Index**: $CD_i = \frac{n_i - n_j}{n_i + n_j + n_k}$ partitioned over post-publication citers (all-time $CD$, and 5-year window $(t_{\text{pub}}, t_{\text{pub}} + 5]$ for $CD_5$), where $n_i$ cites $i$ only, $n_j$ cites $i$ and at least one reference of $i$, and $n_k$ cites references of $i$ without citing $i$.
- **DAG HITS**: Power-iteration hub $h$ and authority $a$ vectors ($h = A a, a = A^{\top} h$, $L_2$-normalized).
- **Topic Drift**: Sliding time-window Laplace-smoothed Mutual Information $I_w(D; T)$, NMI, PMI, and pairwise Jensen–Shannon divergence $\mathrm{JSD}(P \,\|\, Q) \in [0, 1]$ bits.

---

## 4. `wgpu` WGSL Physics & Vector Rendering (`src/wgpu_graph.rs`)

- **Disjoint Spatial-Hash + Multipole Partition**: `cs_bin_nodes` bins nodes into a $128 \times 128$ fine grid and $16 \times 16$ fixed-point charge/center-of-mass supercells (`fine_per_super = 8`). `cs_forces_and_integrate` computes exact pairwise ForceAtlas2 repulsion over all fine cells in the $3 \times 3$ supercell neighborhood $(|\Delta s_x| \le 1 \land |\Delta s_y| \le 1)$ and switches strictly to supercell monopole repulsion for $|\Delta s_x| > 1 \lor |\Delta s_y| > 1$, guaranteeing zero double-counting and zero skipped boundary cells.
- **Isotropic Aspect Projection**: `world_to_ndc` and `world_to_px` scale both axes uniformly by `min(viewport_w, viewport_h) * 0.44 * zoom`, preventing widescreen distortion and matching CPU `SpatialPickGrid` hit-testing.
- **Edge Thickness & Curved Bézier Arrowheads**: `vs_edge` passes per-edge `stroke_half_w` derived from `raw_eb` (and special geodesic/zombie codes) to `fs_edge`, clips edge endpoints at target node perimeter `tgt_r_px + 2.0` px, and orients the fragment arrowhead SDF along the exact quadratic Bézier terminal tangent $\vec{\tau} = \text{normalize}(1,\, -4\,\text{bow\_amp} / L_{\text{seg}})$.
