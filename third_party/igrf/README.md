# Geomagnetic model data

`src-tauri/vendor/igrf/data/igrf14coeffs.txt` is IAGA's IGRF-14 coefficient
table, committed byte for byte as NOAA distributes it. The vendored `igrf`
crate's build script turns it into the model table the importers use to compute
magnetic declination (#811).

| Data | Origin | Terms | File |
|------|--------|-------|------|
| IGRF-14 coefficients, 1900-2030 | IAGA Division V, Working Group V-MOD; distributed by NOAA NCEI | No license statement in the file | `NOTICE-igrf14` |

Where it was retrieved, its checksum, the proof that the build script reads it
the way the upstream crate read IGRF-13, and how to update it for the next
generation are in `src-tauri/vendor/igrf/MODIFICATIONS.md`.

`scripts/gen-third-party-licenses.mjs` reproduces `NOTICE-igrf14` in the
"Vendored data" section of `THIRD-PARTY-LICENSES.md`. After changing anything
here, run `pnpm gen:third-party`.
