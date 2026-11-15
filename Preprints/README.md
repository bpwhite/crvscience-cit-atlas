# Preprints for Cit-Atlas (`crvscience-cit-atlas`)

**Author:** Bryan P. White (ORCID: [0000-0003-4191-3511](https://orcid.org/0000-0003-4191-3511))  
**Hub:** https://crvscience.com/apps

This directory contains the self-contained LaTeX source (`.tex`) and single-column PDF (`.pdf`) editions for both:

1. **Companion Application Preprint (`APP-PREPRINT-05`)**
   - **Title:** Cit-Atlas: Mapping Citation Networks and Research Impact Across Scientific Papers
   - **Files:** `cit-atlas-bibliometric-graph-engine.tex`, `cit-atlas-bibliometric-graph-engine.pdf`
   - **Canonical Zenodo DOI:** https://doi.org/10.5281/zenodo.24222380
   - **Figshare Mirror:** https://figshare.com/articles/preprint/Cit-Atlas_Mapping_Citation_Networks_and_Research_Impact_Across_Scientific_Papers/35013754
   - **Web Preprint:** https://crvscience.com/apps/preprints/cit-atlas-bibliometric-graph-engine

2. **CRV Science Public Apps Network Overview (`APP-PREPRINT-01`)**
   - **Title:** The CRV Science App Network: Seventeen Open-Source Rust Programs for Science, Data, and Simulation
   - **Files:** `crv-science-public-apps-network.tex`, `crv-science-public-apps-network.pdf`
   - **Canonical Zenodo DOI:** https://doi.org/10.5281/zenodo.24222398
   - **Figshare Mirror:** https://figshare.com/articles/preprint/The_CRV_Science_App_Network_Seventeen_Open-Source_Rust_Programs_for_Science_Data_and_Simulation/35013736
   - **Web Preprint:** https://crvscience.com/apps/preprints/crv-science-public-apps-network

## Compiling Standalone LaTeX

Both `.tex` files inline their bibliography so each compiles standalone with standard `pdflatex`:

```bash
pdflatex cit-atlas-bibliometric-graph-engine.tex
pdflatex crv-science-public-apps-network.tex
```
