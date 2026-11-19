# Local modifications

This is a vendored fork of [`igrf`](https://github.com/rkusa/igrf) v0.2.0 by
Markus Ast (upstream commit `63590fc1b`, the commit the crates.io release was
cut from), used under its MIT license. Upstream declares MIT in `Cargo.toml`
and ships no license file. `LICENSE-MIT` here is the standard MIT text with the
author `Cargo.toml` names. Upstream has had no commits since 2021.

It is embedded in Stalagmight as a path dependency because upstream bundles
IGRF-13, which is only defined up to 2025-12-31. From 2026 on, every magnetic
survey Stalagmight imported got no declination correction (#811).

## Changes from upstream v0.2.0

- **Model data: IGRF-14 instead of IGRF-13.** `data/IGRF13.COF` (NOAA's
  geomag70 format) is replaced by `data/igrf14coeffs.txt`, IAGA's published
  coefficient table. It is committed byte for byte as NOAA distributes it at
  <https://www.ngdc.noaa.gov/IAGA/vmod/coeffs/igrf14coeffs.txt> (retrieved
  2026-11-19, sha256
  `8f8d88403028fc4ee92c4f38d97b46e0a87e2cfc496045b43c9e26c1d6b0903c`). IGRF-14
  covers 1900.0 up to 2030.0. It revises the 2020 model (now definitive, DGRF
  2020) and adds IGRF 2025 with secular variation to 2030. Earlier models are
  unchanged.
- **`build.rs` reads the IAGA table** in place of the COF file. NOAA's geomag70
  package, the only source of COF files, was not updated for IGRF-14. The
  generated `MODELS` table has the same shape and names. **Proof of
  equivalence:** fed `igrf13coeffs.txt` (sha256
  `460b8d8beb9b4df84febe4f0b639f0dd54dccfe8ff0970616287b015fa721425`, from the
  same NOAA directory), the new `build.rs` writes a `data.rs` byte-identical to
  the one upstream's COF parser writes from `IGRF13.COF`. That was checked on
  2026-11-19 against upstream's generated output.
- **`lib.rs`**:
  - the `DateOutOfRange` message names the new end date (2029-12-31);
  - `#[allow(dead_code)]` on `FieldModel`, whose unread fields warned on every
    build once this became a path dependency (cargo hides warnings from
    crates.io dependencies);
  - the tests follow the data. `test_gh2_max2_gt_zero` checks IGRF-14's
    secular variation, and `test_declination_year_out_of_range` checks that
    2026 is in range and 2030 is not. `test_interpsh` and `test_declination`
    (1999) are unchanged and still pass, since those models did not change.
- **`Cargo.toml`** gains a `repository` link and a fork note; `Cargo.lock` is
  committed and pinned to the versions the app's own lock resolves.

Everything else (the field evaluation itself) is upstream's code, unchanged.

## Updating to the next generation (IGRF-15, expected late 2029)

1. Download `igrfNNcoeffs.txt` from NOAA's IAGA V-MOD page into `data/`, delete
   the old table, and point `COEFFS` and the `rerun-if-changed` line in
   `build.rs` at the new file.
2. Update the end date in `lib.rs`'s `DateOutOfRange` message and in
   `IGRF_MODEL_SPAN` (`crates/parsers/src/parsers/geo.rs`).
3. Re-run the equivalence check above against the previous generation's
   table, and refresh the golden values in `geo.rs` from a calculator that
   serves the new model.
4. Update the provenance here and in `third_party/igrf/`, then run
   `pnpm gen:third-party`.

`geo.rs`'s `the_bundled_model_covers_a_survey_a_year_from_today` goes red a
year before the bundled model ends, which is the reminder.
