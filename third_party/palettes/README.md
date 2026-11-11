# Colormap data licences

`src/utils/palettes/catalog.generated.ts` holds stop tables sampled from the
named colormaps that ship with matplotlib 3.10.7, written by
`scripts/gen-palettes.py`. The tables are derived data: each map is sampled at
256 points and simplified to the fewest stops that stay within one 8-bit step
of matplotlib's own colours.

Where the data comes from, and the licence each part carries:

| Maps | Origin | Licence | File |
|------|--------|---------|------|
| Everything in the table | matplotlib 3.10.7 | Matplotlib license (PSF-style) | `LICENSE-matplotlib` |
| viridis, plasma, inferno, magma | Nathaniel J. Smith, Stefan van der Walt, Eric Firing | CC0 (no conditions) | - |
| The ColorBrewer maps (Blues ... YlOrRd, the diverging Br/Pi/PR/Pu/Rd maps, Spectral, and the Pastel/Paired/Accent/Dark2/Set qualitative sets) | Cynthia Brewer, Mark Harrower, The Pennsylvania State University | Apache-2.0 | `LICENSE-colorbrewer` |
| gist_earth, gist_gray, gist_heat, gist_ncar, gist_rainbow, gist_stern, gist_yarg | The Yorick scientific visualisation package | BSD-like | `LICENSE-yorick` |
| Wistia | Wistia heatmap-palette | MIT | `LICENSE-wistia` |
| turbo | Google LLC | Apache-2.0 | `NOTICE-turbo` |
| berlin, managua, vanimo | Fabio Crameri, Scientific colour maps 8.0.1 (doi:10.5281/zenodo.1243862), as distributed with matplotlib | MIT | `LICENSE-crameri` |

`scripts/gen-third-party-licenses.mjs` reproduces every file in this directory
in the "Vendored data" section of `THIRD-PARTY-LICENSES.md`, which ships with
the app. After changing anything here, run `pnpm gen:third-party`.

The hand-authored gradients in `src/utils/palettes/catalog.curated.ts` are
original to Stalagmight and carry no third-party licence.
