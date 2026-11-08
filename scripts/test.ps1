# Run the Rust test suite on Windows (PowerShell).
#
# Sets STALAGMIGHT_TEST_MANIFEST so `cargo test` harness binaries delay-load
# comctl32 (see src-tauri/build.rs). Without it those binaries abort at startup
# with STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139) - the loader binds to comctl32
# v5, which lacks TaskDialogIndirect (imported via rfd / tauri-plugin-dialog),
# and no test gets to run. Mirrors the CI command (telemetry feature + endpoint).
#
# Extra args are forwarded to cargo, e.g.:  ./scripts/test.ps1 save_project
$ErrorActionPreference = 'Stop'

$env:STALAGMIGHT_TEST_MANIFEST = '1'
# Strip SQLite modules a hostile .stalx could reach (#608, docs/toolchain.md).
$env:LIBSQLITE3_FLAGS = '-USQLITE_ENABLE_FTS3 -USQLITE_ENABLE_FTS3_PARENTHESIS -USQLITE_ENABLE_FTS5 -USQLITE_ENABLE_RTREE -USQLITE_ENABLE_DBSTAT_VTAB -DSQLITE_OMIT_LOAD_EXTENSION'
if (-not $env:TELEMETRY_ENDPOINT) { $env:TELEMETRY_ENDPOINT = 'http://localhost/dummy' }

cargo test --workspace --manifest-path src-tauri/Cargo.toml --features telemetry @args
# $ErrorActionPreference does not stop on native-command failure (that needs
# $PSNativeCommandUseErrorActionPreference, PS 7.3+), so propagate explicitly -
# otherwise a later success would mask a red workspace run.
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Vendored compass_data is [workspace]-excluded, so the run above never tests it
# (CI gates it separately, #494). Skipped when a filter is given - extra args
# target the workspace run.
if (-not $args) {
    cargo test --locked --manifest-path src-tauri/vendor/compass_data/Cargo.toml
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
