//! The wire projection of the station-graph model (plan 9.2): what the IPC
//! handlers send the frontend, in the shapes it reads today.
//!
//! The frontend keeps its positional view address, the slot
//! `StationRef { line_id, station_index }` ("the i-th station along line L"),
//! and its generated TypeScript types. This module turns a
//! [`Project`](super::Project) (and the derived views of stream M3) into
//! exactly those payloads:
//!
//! - [`ProjectWire`], [`LineWire`] and [`ProjectStateWire`]: the TS `Project`,
//!   `Line` and `ProjectState`, field for field, except `Project.line_offsets`
//!   (dropped, plan 9.2). `station_merges` are derived (each junction's first
//!   slot paired with each of its other slots), datum and closure ends are
//!   slots, closure metrics and `is_aligned` are derived.
//! - The per-slot [`crate::Station`] list (`Project::stations_wire`, raw or
//!   Computed View): a junction's slots share one Station ID and comment,
//!   and each carries the name its line shows it by (plan 5.3 per line);
//!   `merged_with` lists the station's other slots.
//! - Every preview and query that names stations, mapped from the M1/M2/M3
//!   result structs to today's DTOs (`previews.rs`, `combine.rs`).
//! - Slot resolution for the command boundary ([`SlotTarget`]).
//! - The input of the binary load envelope ([`LoadEnvelopeInput`]).
//!
//! The DTOs this module returns are today's types wherever their shape does
//! not change (`Station`, `StationMergeWire`, `ZeroDatumWire`,
//! `LoopClosureWire`, `ClosureRef`, `ConnectivityResult`, `DeleteShotImpact`,
//! `CombinePlan`, ...), so their TypeScript cannot drift. Only the three
//! types whose source struct is the legacy model itself are new here; they
//! derive `TS` with the legacy TS names but without `export`, so nothing
//! writes them until the generator is switched to them (docs/survey-model.md,
//! "Wire API", has the switch procedure).
//!
//! Every function is a pure function of the project state it is called on.
//! A preview mapping takes the state the preview ran on (before the edit),
//! so the slots it names are the ones the user sees.

mod combine;
mod envelope;
mod identity;
mod previews;
mod project;
mod refs;
mod stations;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod identity_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod ts_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and bit-exact float equality is what it checks"
)]
mod wire_tests;

pub use envelope::LoadEnvelopeInput;
pub use identity::lettered_wire;
pub use project::{LineWire, ProjectStateWire, ProjectWire};
pub use refs::{wire_strategy, SlotTarget};
