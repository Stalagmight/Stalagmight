//! The exporters on the station-graph model (`models::survey`, plan
//! `docs/plans/2026-11-20_station-graph-model.md` 10.5): CSV, JSON, Ariane
//! TML/TMLU, the map image (SVG/PNG/PDF), its Levels histogram and the dive
//! profile.
//!
//! They live beside the legacy exporters (the parent module, on
//! `models::project`) and share their writers: the CSV escaping and number
//! formats, the JSON records, the Ariane row writer and every pass after the
//! rows are built (GPS stamping, the geometry repair, the SpeleoDB
//! hardening), and the whole image renderer. Only the step that reads the
//! model is new. The core switch flips its callers to these functions and
//! deletes the legacy ones.
//!
//! What the station graph changes in the files (declared, plan 12.2):
//! - **CSV / JSON:** `from` / `to` are station names and `from_id` / `to_id`
//!   Station IDs, read through the chain, so a junction has ONE name and ONE
//!   id on every line through it (a merged junction used to print each line's
//!   own station).
//! - **Ariane:** a row's `ID` is the Station ID of the station the row ends at
//!   (a START's, the station it stands on), so ids are stable across exports;
//!   CLOSURE rows and the GPS-fix STARTs take ids above every station. A
//!   junction is one node, so a line hangs off its parent's row for that
//!   station without any alias, and a station comment is folded once.
//! - **Image:** one label and one comment per station, one star per datum
//!   station; the Levels depth histogram counts each station once.
//! - **Dive profile:** a route has no merge hops, so no zero-length point at a
//!   junction.
//!
//! Every exporter is deterministic: the same project exports to the same
//! bytes (the JSON export's `exported_at` is an argument of [`write_json`]).

mod dive_profile;
mod image;
mod tabular;

pub use dive_profile::render_dive_profile;
pub use image::{
    export_image, export_image_with_positions, export_value_histogram, generate_svg_string,
    generate_svg_string_with_positions,
};
pub(crate) use tabular::shot_credit;
pub use tabular::{export_csv, export_json, json_export, write_csv, write_json};

#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test helpers: a refused synthetic build is a broken test setup"
)]
pub(crate) mod test_helpers;

pub use super::tml::survey::{export_tml, export_tml_for_speleodb};
