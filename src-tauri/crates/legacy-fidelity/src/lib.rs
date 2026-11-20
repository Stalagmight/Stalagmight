//! Test-only crate: `tests/fidelity.rs` proves that the frozen
//! `stalagmight-legacy` reader opens a `.stalx` exactly as the live 0.6.0 app
//! does (`AppState::do_load_project_full`, `get_adjusted_stations`), bit for
//! bit. It depends on the live crates on purpose, which the legacy crate must
//! never do; delete this crate when the live model moves on.
