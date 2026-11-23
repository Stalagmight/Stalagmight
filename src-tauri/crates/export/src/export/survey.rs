//! The exporters on the station-graph model (`models::survey`, plan
//! `docs/plans/2026-11-20_station-graph-model.md` 10.5): CSV, JSON, Ariane
//! TML/TMLU, the map image (SVG/PNG/PDF), its Levels histogram and the dive
//! profile.
//!
//! They are the only exporters: the 0.6.0 ones on `models::project` were
//! deleted with the core switch. The writers they feed are the parent
//! module's: the CSV escaping and number formats, the JSON records, the
//! Ariane row writer and every pass after the rows are built (GPS stamping,
//! the geometry repair, the SpeleoDB hardening), and the whole image
//! renderer, which draws a model-neutral sketch (`image_sketch.rs`).
//!
//! What the station graph changes in the files (declared, plan 12.2, and the
//! kept and archived stations, RS 9 and 11):
//! - **CSV / JSON:** `from` / `to` are station names and `from_id` / `to_id`
//!   Station IDs, read through the chain by the **shown identity**: a
//!   junction has ONE name and ONE id on every line through it, its kept
//!   station's (a merged junction used to print each line's own station),
//!   and a loop end a closure archives is written under its kept end's name
//!   and id (these files hold readings only, so a shared id is how their
//!   readers tie a loop).
//! - **Ariane:** a row's `ID` is the Station ID of the node the row ends at
//!   (a START's, the node it stands on), so ids are stable across exports;
//!   CLOSURE rows and the GPS-fix STARTs take ids above every station. A
//!   junction is one node, so a line hangs off its parent's row for that
//!   station without any alias, and a station comment is folded once. A loop
//!   end a closure archives keeps its own id as its node number and writes no
//!   comment. Archived stations and closure archives do not survive a round
//!   trip (Ariane has no names).
//! - **Image:** one label and one comment per station, one star per datum
//!   station; a loop end a closure archives is drawn but never labelled; the
//!   Levels depth histogram counts each station once.
//! - **Dive profile:** a route has no merge hops, so no zero-length point at a
//!   junction; it names no station.
//!
//! Compass, Walls and survex are import-only (no exporter), so RS 9's rule
//! for them (shown names) has no writer to apply to yet.
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
