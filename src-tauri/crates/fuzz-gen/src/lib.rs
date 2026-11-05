//! Test-only support for fuzzing every file Stalagmight reads: the Ariane,
//! mNemo, Walls, Compass and GPX importers and the `.stalx` open path.
//!
//! - [`source`]: the decision stream every generator draws from.
//! - [`gen`]: structure-aware generators and the semantic mutators they share
//!   (units and unit-scaled sentinels, number spellings, encodings, damage).
//! - [`fileset`]: multi-file imports and the scratch directory they run in.
//! - [`oracle`]: the app's import pipeline plus the invariants it must keep.
//! - [`stalx`]: synthetic projects, SQL-level corruption, the load oracle.
//! - [`targets`]: the fuzz targets by name and their synthetic seeds.
//!
//! Consumers: the cargo-fuzz crate at `src-tauri/fuzz` (nightly, outside the
//! workspace) and this crate's own fixed-seed tests (stable, in the normal
//! `cargo test --workspace` run). Nothing in the shipped app depends on it.
//! See docs/testing-recipes.md, "Parser fuzzing".

pub mod fileset;
pub mod gen;
pub mod oracle;
pub mod source;
pub mod stalx;
pub mod targets;
