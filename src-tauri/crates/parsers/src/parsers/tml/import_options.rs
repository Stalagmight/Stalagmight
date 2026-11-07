//! What the Ariane import dialog can decide, and how a section ends up placed.
//!
//! Two things live here rather than in `mod.rs`:
//!
//! * [`TmlImportOptions`] - the knobs `convert_tml_with_options` reads. It grew
//!   past the two booleans the old positional signature carried once the dialog
//!   gained a per-section filter (#425) and an explicit placement step.
//! * [`determine_placement`] - the rule that turns a section's topology, its GPS
//!   and the user's choices into an [`ImportPlacement`]. It moved out of
//!   `mod.rs` with the options it now consults; that file is at the file-size
//!   ratchet's warn threshold and cannot absorb both.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{normalize_tml_shot, resolve_tml_depth_in, survives_import, TmlSection, TmlShotType};
use crate::models::{AzimuthReference, GpsCoordinates, ImportPlacement};

/// Where the user asked the FIRST imported section to land.
///
/// Only the root section takes an override: every other section is positioned
/// by the file's own topology (`FromImporting`), and moving one of those would
/// silently break the connection the file states. A root section is the one
/// thing the file cannot place - before #425 it went to the origin in silence
/// whenever no shot carried GPS.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, export_to = "../../bindings/", rename_all = "snake_case")]
pub enum TmlRootPlacement {
    /// Local coordinates, first station at (0, 0) - the historical behaviour,
    /// now chosen rather than defaulted into.
    Origin,
    /// Anchor the first station at these real-world coordinates. The numbers
    /// reach here either from the file's own georeference or typed by the user,
    /// and are validated identically in both cases.
    ZeroDatum { latitude: f64, longitude: f64 },
}

impl TmlRootPlacement {
    /// The placement this choice makes for the root section's station 0.
    ///
    /// Fallible because the coordinates are untrusted at this point: they cross
    /// IPC as bare `f64`s, so a NaN or an out-of-range latitude has to be
    /// refused here rather than written into a project. `GpsCoordinates::new`
    /// is the app's one validator for that (it also wraps longitude), so this
    /// door and the station-editing door cannot drift apart.
    pub fn to_placement(self) -> Result<ImportPlacement, String> {
        match self {
            Self::Origin => Ok(ImportPlacement::Origin),
            Self::ZeroDatum {
                latitude,
                longitude,
            } => Ok(ImportPlacement::ZeroDatum {
                // Depth reference 0.0, matching the file-derived anchor below:
                // Ariane states a surface position, not a datum depth.
                coordinates: GpsCoordinates::new(latitude, longitude, 0.0)?,
                station_index: 0,
            }),
        }
    }
}

/// Everything `convert_tml_with_options` needs beyond the parsed file.
#[derive(Debug, Clone)]
pub struct TmlImportOptions {
    /// Import shots Ariane marked `Excluded`.
    pub include_excluded: bool,
    /// Route the unmarked half of each co-encoded comment to the "to" station
    /// rather than leaving every line on the shot.
    pub comments_on_stations: bool,
    /// Section indices (into [`super::TmlFile::sections`], which the import
    /// dialog lists 1:1) the user chose to import. `None` imports every
    /// section; `Some(set)` imports only the listed ones, and an empty set
    /// imports nothing.
    ///
    /// Filtering by the ORIGINAL index is what keeps this cheap: `section_index`
    /// on the emitted configs, the `FromImporting` targets and the closure
    /// endpoints all address that same numbering, and the executor looks them
    /// up in a map rather than indexing an array - so a sparse set of indices
    /// needs no renumbering anywhere.
    pub selected_sections: Option<HashSet<u32>>,
    /// The user's answer to "where does this survey go?", applied to the first
    /// section the selection admits. `None` keeps the pre-#425 behaviour: the
    /// file's own GPS if it has any, the origin otherwise.
    pub root_placement: Option<TmlRootPlacement>,
}

impl Default for TmlImportOptions {
    fn default() -> Self {
        Self {
            include_excluded: false,
            // Matches the import dialog's default, and the SpeleoDB adoption
            // path - the co-encoded station half belongs on the station.
            comments_on_stations: true,
            selected_sections: None,
            root_placement: None,
        }
    }
}

impl TmlImportOptions {
    /// The option shape every caller had before the section filter existed.
    pub fn basic(include_excluded: bool, comments_on_stations: bool) -> Self {
        Self {
            include_excluded,
            comments_on_stations,
            ..Self::default()
        }
    }

    /// Whether section index `si` (into `TmlFile::sections`) is imported.
    /// `None` selection means every section.
    pub(super) fn includes_section(&self, si: usize) -> bool {
        match &self.selected_sections {
            Some(sel) => sel.contains(&(si as u32)),
            None => true,
        }
    }

    /// Selection entries that name no section in this file, so a caller passing
    /// junk indices over IPC gets told rather than silently importing nothing.
    pub(super) fn out_of_range_selection(&self, section_count: usize) -> usize {
        match &self.selected_sections {
            Some(sel) => sel.iter().filter(|i| **i as usize >= section_count).count(),
            None => 0,
        }
    }
}

/// The inputs `determine_placement` weighs, gathered by the conversion loop.
pub(super) struct PlacementInput<'a> {
    /// This section's index in `TmlFile::sections`.
    pub section_index: u32,
    /// The raw first shot's `from_id` (the file's original topology).
    pub start_ariane_id: i64,
    /// `from_id` of the first non-excluded shot, when it differs from
    /// `start_ariane_id` - the fallback for a section whose leading shots were
    /// filtered out.
    pub effective_start_id: Option<i64>,
    /// Ariane shot id -> (section index, station index), built over the
    /// SELECTED sections only. A parent in a deselected section is therefore
    /// absent here, which is what stops a `FromImporting` placement pointing at
    /// a section the executor never creates (that is a hard, rolling-back
    /// failure in `execute_import_sections`, not a skip).
    pub id_to_import_loc: &'a HashMap<i64, (u32, u32)>,
    /// Shot ids that WOULD have resolved above had their section been selected.
    /// Only used to say why a section lost its anchor.
    pub deselected_ids: &'a HashSet<i64>,
    /// Whether this is the first section the import will CREATE.
    ///
    /// The one definition of "root", and it is deliberately a fact the caller
    /// observes (`configs.is_empty()` at the moment of the call) rather than a
    /// number anyone recomputes. Three parties needed the answer - this
    /// function, the dialog's placement step and the preview layout - and while
    /// two of them derived it from the selection alone ("lowest selected
    /// index") they disagreed with the loop that actually builds the configs:
    /// a lowest-selected section whose every shot is excluded is dropped BEFORE
    /// placement runs, so the user's typed datum was attached to a section that
    /// was never created and silently vanished.
    pub is_root: bool,
    /// The user's explicit choice for the root section.
    pub root_placement: Option<TmlRootPlacement>,
    /// Whether excluded shots are imported. When they are not, the shots in
    /// front of a section's first imported one are not built, and a GPS
    /// anchor written on the first of them belongs to a station the section
    /// no longer has (see [`gps_placement`]).
    pub include_excluded: bool,
    /// The north reference the section's bearings are walked in - the one the
    /// import records on it.
    pub azimuth_reference: AzimuthReference,
}

/// Determine placement for a section based on the user's choice, topology and
/// GPS data - in that order.
///
/// Returns the placement and, when the section had to fall back to the origin,
/// the warning that says so.
pub(super) fn determine_placement(
    section: &TmlSection,
    input: &PlacementInput<'_>,
) -> Result<(ImportPlacement, Option<String>), String> {
    // The root section: the user's answer wins, then the file's own GPS, then
    // the origin. Before #425 only the last two existed and neither was said
    // out loud.
    if input.is_root {
        if let Some(choice) = input.root_placement {
            // The dialog's "use the position in the file" sends the root's own
            // first-shot coordinates verbatim. That IS the file's anchor, so it
            // lands where the file measured it - the same rule as with no
            // choice at all - rather than on whatever station the section now
            // starts at.
            if is_file_anchor(section, choice) {
                if let Some(placement) = gps_placement(section, input) {
                    return Ok((placement, None));
                }
            }
            return Ok((choice.to_placement()?, None));
        }
        if let Some(placement) = gps_placement(section, input) {
            return Ok((placement, None));
        }
        return Ok((ImportPlacement::Origin, None));
    }

    // Try to connect to an already-imported section. First the original
    // start_ariane_id, then effective_start_id (from the first non-excluded
    // shot) if the original was excluded and cannot be resolved.
    let candidates: Vec<i64> = std::iter::once(Some(input.start_ariane_id))
        .chain(std::iter::once(input.effective_start_id))
        .flatten()
        .filter(|&id| id >= 0)
        .collect();

    for &from_id in &candidates {
        if let Some(&(target_section, target_station)) = input.id_to_import_loc.get(&from_id) {
            // Forward references are fine - configs are topologically sorted
            // afterward so the target section is created first.
            if target_section != input.section_index {
                return Ok((
                    ImportPlacement::FromImporting {
                        import_station: 0, // First station of this section
                        target_section_index: target_section,
                        target_station_index: target_station,
                    },
                    None,
                ));
            }
        }
    }

    if let Some(placement) = gps_placement(section, input) {
        return Ok((placement, None));
    }

    // Nothing left to attach to. Say WHICH of the two reasons it was: a section
    // the user deselected is a consequence of their own choice and reversible
    // by re-selecting it, while a genuinely disconnected section is a property
    // of the file.
    let lost_parent = candidates
        .iter()
        .any(|id| input.deselected_ids.contains(id));
    tracing::warn!(
        section = %section.name,
        lost_parent,
        "Section has no topology link or GPS data, placing at origin"
    );
    let warning = if lost_parent {
        format!(
            "Section \"{}\" attached to a section you did not select, so it was placed at the origin",
            section.name
        )
    } else {
        format!(
            "Section \"{}\" was disconnected from the rest of the survey and was placed at the origin",
            section.name
        )
    };
    Ok((ImportPlacement::Origin, Some(warning)))
}

/// A zero-datum placement from the section's own first shot, when it carries a
/// USABLE one. Ariane writes the survey's GPS anchor onto a shot, and `0.0/0.0`
/// is its no-data value (Null Island is not a cave).
///
/// The file is untrusted input, so the numbers go through
/// `GpsCoordinates::new` - the app's one coordinate validator - exactly like
/// the ones a user types into the placement step. This is not theoretical: the
/// null-island guard below is a pair of `<=` comparisons, and EVERY comparison
/// against NaN is false, so a crafted `<Latitude>NaN</Latitude>` sailed past it
/// and was written into the project verbatim as a datum. A refusal falls
/// through to the caller's next option (topology, then the origin with the
/// disconnected warning) rather than failing the import: the rest of the
/// section's measurements are still perfectly good.
///
/// The anchor is where the file MEASURED it: on the station the section's raw
/// first shot starts from (Ariane writes it on the START, and the parser
/// copies it onto the shots leaving that START). When that shot is not
/// imported - excluded, with the include-excluded toggle off - the section's
/// station 0 is the start of its first IMPORTED shot instead, so the datum is
/// carried forward along the shots that were dropped, by the same horizontal
/// geometry the import draws (#576 D39(j), NF-AN4-4). Anchoring station 0 at
/// the raw coordinates instead displaced the whole section by one dropped
/// shot. CLOSURE rows are skipped in that walk: the import never draws them.
fn gps_placement(section: &TmlSection, input: &PlacementInput<'_>) -> Option<ImportPlacement> {
    let first_shot = section.shots.first()?;
    if first_shot.latitude.abs() <= f64::EPSILON && first_shot.longitude.abs() <= f64::EPSILON {
        return None;
    }
    let measured = match GpsCoordinates::new(first_shot.latitude, first_shot.longitude, 0.0) {
        Ok(coordinates) => coordinates,
        Err(reason) => {
            tracing::warn!(
                section = %section.name,
                %reason,
                purpose = "import Ariane TML/TMLU",
                "Ignoring an unusable GPS anchor in the file"
            );
            return None;
        }
    };
    let (east, north) =
        dropped_lead_offset(section, input.include_excluded, input.azimuth_reference);
    // `local_to_gps` returns a storable fix (longitude in [-180, 180]); it is
    // `None` only beyond the horizon, which the same fallback covers.
    let coordinates = if east == 0.0 && north == 0.0 {
        measured
    } else {
        crate::models::local_to_gps(east, north, &measured)
            .ok_or_else(|| format!("({east}, {north}) m is beyond the horizon"))
            .and_then(|(latitude, longitude)| GpsCoordinates::new(latitude, longitude, 0.0))
            .unwrap_or_else(|reason| {
                tracing::warn!(
                    section = %section.name,
                    %reason,
                    purpose = "import Ariane TML/TMLU",
                    "Could not carry the GPS anchor past the excluded shots; anchoring the \
                     first imported station at the file's coordinates"
                );
                measured
            })
    };
    Some(ImportPlacement::ZeroDatum {
        coordinates,
        station_index: 0,
    })
}

/// Whether `choice` is exactly the root section's own first-shot anchor - the
/// coordinates the dialog offers as "the position in the file" and sends back
/// verbatim (bit-equal through the JSON round trip).
fn is_file_anchor(section: &TmlSection, choice: TmlRootPlacement) -> bool {
    let (
        TmlRootPlacement::ZeroDatum {
            latitude,
            longitude,
        },
        Some(first),
    ) = (choice, section.shots.first())
    else {
        return false;
    };
    latitude.to_bits() == first.latitude.to_bits()
        && longitude.to_bits() == first.longitude.to_bits()
}

/// East/north metres from the station the section's raw first shot starts at
/// to the station its first IMPORTED shot starts at: the sum of the dropped
/// leading shots' horizontal legs, each walked exactly as the import walks a
/// shot (horizontal length from slope and depth change, the circular-mean
/// bearing turned true by `reference`). `(0, 0)` when nothing in front is
/// dropped.
fn dropped_lead_offset(
    section: &TmlSection,
    include_excluded: bool,
    reference: AzimuthReference,
) -> (f64, f64) {
    let (mut east, mut north) = (0.0, 0.0);
    let mut prev_depth: Option<f64> = None;
    for shot in &section.shots {
        if survives_import(shot, include_excluded) {
            break;
        }
        if shot.shot_type == TmlShotType::Closure {
            continue;
        }
        let depth_in = resolve_tml_depth_in(shot, prev_depth, section.start_depth);
        let data = normalize_tml_shot(shot, depth_in);
        let horizontal = data.horizontal_length();
        let bearing = data.average_azimuth_true_rad(reference);
        east += horizontal * bearing.sin();
        north += horizontal * bearing.cos();
        prev_depth = Some(shot.depth);
    }
    (east, north)
}

/// Ariane shot ids that the current selection keeps, and the ones it drops.
///
/// Both maps apply the SAME shot filter the import applies (`CLOSURE` rows
/// never enter a section, excluded rows only with `include_excluded`), so an
/// id is in exactly one of them or in neither.
pub(super) struct SelectionIndex {
    /// Kept shot id -> (section index, station index). Station index is the
    /// station at the END of the shot: shot `i` of a section ends at station
    /// `i + 1`.
    pub id_to_import_loc: HashMap<i64, (u32, u32)>,
    /// Shot ids dropped only because their section was deselected.
    pub deselected_ids: HashSet<i64>,
}

pub(super) fn build_selection_index(
    sections: &[TmlSection],
    opts: &TmlImportOptions,
) -> SelectionIndex {
    let mut id_to_import_loc: HashMap<i64, (u32, u32)> = HashMap::new();
    let mut deselected_ids: HashSet<i64> = HashSet::new();
    for (si, section) in sections.iter().enumerate() {
        let selected = opts.includes_section(si);
        let mut import_shot_idx: u32 = 0;
        for shot in &section.shots {
            if !survives_import(shot, opts.include_excluded) {
                continue;
            }
            if selected {
                id_to_import_loc.insert(shot.id, (si as u32, import_shot_idx + 1));
                import_shot_idx += 1;
            } else {
                deselected_ids.insert(shot.id);
            }
        }
    }
    SelectionIndex {
        id_to_import_loc,
        deselected_ids,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::{convert_tml_with_options, TmlFile, TmlShot, TmlUnit};
    use super::*;

    pub(super) fn shot(id: i64, from_id: i64) -> TmlShot {
        TmlShot {
            id,
            uuid: None,
            from_id,
            shot_type: TmlShotType::Real,
            azimuth: 90.0,
            length: 10.0,
            depth: 0.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#ff0000".to_string(),
            excluded: false,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: String::new(),
        }
    }

    pub(super) fn section(name: &str, start_ariane_id: i64, shots: Vec<TmlShot>) -> TmlSection {
        let end_ariane_id = shots.last().map_or(-1, |s| s.id);
        TmlSection {
            name: name.to_string(),
            shots,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            is_branch_split: false,
            start_ariane_id,
            end_ariane_id,
            start_depth: 0.0,
        }
    }

    pub(super) fn file(sections: Vec<TmlSection>) -> TmlFile {
        TmlFile {
            cave_name: "T".to_string(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            geo_coding: None,
            sections,
            closures: Vec::new(),
        }
    }

    /// A → B → C, each hanging off the previous section's last shot.
    pub(super) fn chain_of_three() -> TmlFile {
        file(vec![
            section("A", 0, vec![shot(1, 0), shot(2, 1)]),
            section("B", 2, vec![shot(3, 2), shot(4, 3)]),
            section("C", 4, vec![shot(5, 4), shot(6, 5)]),
        ])
    }

    fn convert(
        tml: &TmlFile,
        opts: &TmlImportOptions,
    ) -> (Vec<crate::models::ImportSectionConfig>, Vec<String>) {
        let (configs, _, warnings) = convert_tml_with_options(tml, opts).unwrap();
        (configs, warnings)
    }

    fn selection(indices: &[u32]) -> TmlImportOptions {
        TmlImportOptions {
            selected_sections: Some(indices.iter().copied().collect()),
            ..Default::default()
        }
    }

    /// The premise the rest of these tests rest on: with no selection the chain
    /// places section 0 at the origin and hangs 1 and 2 off their parents.
    #[test]
    fn no_selection_places_the_chain_exactly_as_before() {
        let (configs, warnings) = convert(&chain_of_three(), &TmlImportOptions::default());
        assert_eq!(configs.len(), 3);
        assert!(matches!(configs[0].placement, ImportPlacement::Origin));
        for c in &configs[1..] {
            assert!(
                matches!(c.placement, ImportPlacement::FromImporting { .. }),
                "expected a topology link, got {:?}",
                c.placement
            );
        }
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    /// Deselecting the section everything hangs off must NOT leave a
    /// `FromImporting` pointing at a section that is never created: that is a
    /// hard, whole-import rollback in `execute_import_sections`, not a skip.
    /// The first surviving section re-anchors to the origin instead.
    #[test]
    fn deselecting_the_root_reanchors_its_child_instead_of_dangling() {
        let tml = chain_of_three();
        let (configs, warnings) = convert(&tml, &selection(&[1, 2]));

        assert_eq!(configs.len(), 2, "only the selected sections are imported");
        let created: HashSet<u32> = configs.iter().map(|c| c.section_index).collect();
        assert_eq!(created, [1, 2].into_iter().collect::<HashSet<u32>>());

        let b = configs
            .iter()
            .find(|c| c.section_index == 1)
            .expect("B is imported");
        assert!(
            matches!(b.placement, ImportPlacement::Origin),
            "B lost its parent and must become the root, got {:?}",
            b.placement
        );
        let c = configs
            .iter()
            .find(|c| c.section_index == 2)
            .expect("C is imported");
        let ImportPlacement::FromImporting {
            target_section_index,
            ..
        } = c.placement
        else {
            panic!("C still attaches to B, got {:?}", c.placement);
        };
        assert_eq!(target_section_index, 1);

        // Every FromImporting target must be a section this batch creates.
        for cfg in &configs {
            if let ImportPlacement::FromImporting {
                target_section_index,
                ..
            } = cfg.placement
            {
                assert!(
                    created.contains(&target_section_index),
                    "dangling placement onto uncreated section {target_section_index}"
                );
            }
        }
        assert!(
            warnings.is_empty(),
            "the new root is not a fallback and must not warn: {warnings:?}"
        );
    }

    /// A section whose parent was deselected but which is NOT the new root is a
    /// genuine fallback, and says which of the two reasons put it at the origin.
    #[test]
    fn a_child_of_a_deselected_middle_section_says_the_selection_moved_it() {
        let tml = chain_of_three();
        let (configs, warnings) = convert(&tml, &selection(&[0, 2]));

        let c = configs
            .iter()
            .find(|c| c.section_index == 2)
            .expect("C is imported");
        assert!(
            matches!(c.placement, ImportPlacement::Origin),
            "C's parent B is gone, got {:?}",
            c.placement
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("attached to a section you did not select")),
            "expected the deselection wording, got: {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("was disconnected")),
            "a deselected parent is not the file being disconnected: {warnings:?}"
        );
    }

    /// The generic "disconnected" wording is still what a genuinely orphaned
    /// section gets - the selection-specific message must not swallow it.
    #[test]
    fn a_file_orphan_still_reports_as_disconnected() {
        let tml = file(vec![
            section("A", 0, vec![shot(1, 0)]),
            section("Orphan", 999, vec![shot(2, 999)]),
        ]);
        let (_, warnings) = convert(&tml, &TmlImportOptions::default());
        assert!(
            warnings.iter().any(|w| w.contains("was disconnected")),
            "expected the disconnected wording, got: {warnings:?}"
        );
    }

    /// Deselecting the section that carried the file's GPS anchor drops that
    /// anchor with it: the new root falls back to the origin rather than
    /// inheriting coordinates it has no shot for.
    #[test]
    fn deselecting_the_gps_anchor_section_leaves_the_new_root_at_the_origin() {
        let mut tml = chain_of_three();
        tml.sections[0].shots[0].latitude = 20.5;
        tml.sections[0].shots[0].longitude = -87.1;

        let (all, _) = convert(&tml, &TmlImportOptions::default());
        assert!(
            matches!(all[0].placement, ImportPlacement::ZeroDatum { .. }),
            "premise: the unfiltered import anchors on the file's GPS"
        );

        let (configs, _) = convert(&tml, &selection(&[1, 2]));
        let b = configs
            .iter()
            .find(|c| c.section_index == 1)
            .expect("B is imported");
        assert!(
            matches!(b.placement, ImportPlacement::Origin),
            "the anchor left with section A, got {:?}",
            b.placement
        );
    }

    /// A section keeps its OWN GPS anchor when the sections before it are
    /// deselected - the anchor is per-section, not per-file.
    #[test]
    fn a_selected_section_keeps_its_own_gps_anchor() {
        let mut tml = chain_of_three();
        tml.sections[1].shots[0].latitude = 20.5;
        tml.sections[1].shots[0].longitude = -87.1;

        let (configs, _) = convert(&tml, &selection(&[1, 2]));
        let b = configs
            .iter()
            .find(|c| c.section_index == 1)
            .expect("B is imported");
        assert!(
            matches!(b.placement, ImportPlacement::ZeroDatum { .. }),
            "B's own anchor survives, got {:?}",
            b.placement
        );
    }

    #[test]
    fn an_empty_selection_imports_nothing_and_reports_nothing_dropped() {
        let (configs, warnings) = convert(&chain_of_three(), &selection(&[]));
        assert!(configs.is_empty());
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("no shots after filtering")),
            "a deselected section was not 'dropped by filtering': {warnings:?}"
        );
    }

    /// Indices that name no section select nothing, and say so rather than
    /// producing a silent empty import. Not reachable from the dialog, which
    /// builds its indices from the file it is previewing.
    #[test]
    fn out_of_range_selection_indices_are_reported() {
        let (configs, warnings) = convert(&chain_of_three(), &selection(&[7, 9]));
        assert!(configs.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("did not exist in this file")),
            "expected an out-of-range warning, got: {warnings:?}"
        );

        // A partly-valid selection still imports the valid part.
        let (configs, warnings) = convert(&chain_of_three(), &selection(&[0, 42]));
        assert_eq!(configs.len(), 1);
        assert!(warnings
            .iter()
            .any(|w| w.contains("did not exist in this file")));
    }

    /// A loop closure that lands in a deselected section is skipped, and the
    /// receipt blames the selection rather than the exclusion filter (which is
    /// a different toggle in a different place).
    #[test]
    fn a_closure_into_a_deselected_section_names_the_selection() {
        let mut closure = shot(7, 2);
        closure.shot_type = TmlShotType::Closure;
        closure.closure_to_id = 5;
        let tml = file(vec![
            section("A", 0, vec![shot(1, 0), shot(2, 1), closure]),
            section("B", 2, vec![shot(5, 2), shot(6, 5)]),
        ]);

        let (_, closures, warnings) =
            convert_tml_with_options(&tml, &TmlImportOptions::default()).unwrap();
        assert_eq!(
            closures.len(),
            1,
            "premise: unfiltered, the closure resolves"
        );
        assert!(warnings.is_empty(), "premise: and warns about nothing");

        let (_, closures, warnings) = convert_tml_with_options(&tml, &selection(&[0])).unwrap();
        assert!(closures.is_empty(), "the far endpoint is not imported");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("referenced a section you did not select")),
            "expected the deselection wording, got: {warnings:?}"
        );
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("referenced excluded stations")),
            "the exclusion filter did not do this: {warnings:?}"
        );
    }

    /// A CLOSURE row inside a deselected section is silent: neither end of it
    /// is being imported, so there is nothing to tell the user about.
    #[test]
    fn a_closure_inside_a_deselected_section_is_silent() {
        let mut closure = shot(7, 5);
        closure.shot_type = TmlShotType::Closure;
        closure.closure_to_id = 1;
        let tml = file(vec![
            section("A", 0, vec![shot(1, 0), shot(2, 1)]),
            section("B", 2, vec![shot(5, 2), closure]),
        ]);

        let (_, closures, warnings) = convert_tml_with_options(&tml, &selection(&[0])).unwrap();
        assert!(closures.is_empty());
        // "Silent" means SILENT, not "does not happen to contain this phrase":
        // a substring check passes for any wording that misses it, including a
        // future one. Deselecting B removes B entirely, so the conversion has
        // nothing at all to report here.
        assert!(
            warnings.is_empty(),
            "a closure in a section the user removed is not news: {warnings:?}"
        );
    }

    // -----------------------------------------------------------------
    // Root placement (#425)
    // -----------------------------------------------------------------

    #[test]
    fn an_explicit_origin_choice_overrides_the_files_gps_anchor() {
        let mut tml = chain_of_three();
        tml.sections[0].shots[0].latitude = 20.5;
        tml.sections[0].shots[0].longitude = -87.1;

        let (configs, _) = convert(
            &tml,
            &TmlImportOptions {
                root_placement: Some(TmlRootPlacement::Origin),
                ..Default::default()
            },
        );
        assert!(
            matches!(configs[0].placement, ImportPlacement::Origin),
            "the user's choice wins over the file, got {:?}",
            configs[0].placement
        );
    }

    #[test]
    fn a_typed_datum_anchors_the_root_and_leaves_the_children_on_topology() {
        let (configs, _) = convert(
            &chain_of_three(),
            &TmlImportOptions {
                root_placement: Some(TmlRootPlacement::ZeroDatum {
                    latitude: 20.5,
                    longitude: -87.1,
                }),
                ..Default::default()
            },
        );
        let ImportPlacement::ZeroDatum {
            coordinates,
            station_index,
        } = configs[0].placement
        else {
            panic!("expected a zero datum, got {:?}", configs[0].placement);
        };
        assert_eq!(station_index, 0);
        assert!((coordinates.latitude - 20.5).abs() < 1e-9);
        assert!((coordinates.longitude - -87.1).abs() < 1e-9);
        assert!(
            matches!(configs[1].placement, ImportPlacement::FromImporting { .. }),
            "only the root takes the override"
        );
    }

    /// The override applies to whichever section the SELECTION makes root, not
    /// to file index 0 - otherwise choosing coordinates while section 0 is
    /// deselected would silently place nothing.
    #[test]
    fn the_override_follows_the_selection_root() {
        let (configs, _) = convert(
            &chain_of_three(),
            &TmlImportOptions {
                selected_sections: Some([1, 2].into_iter().collect()),
                root_placement: Some(TmlRootPlacement::ZeroDatum {
                    latitude: 20.5,
                    longitude: -87.1,
                }),
                ..Default::default()
            },
        );
        let b = configs
            .iter()
            .find(|c| c.section_index == 1)
            .expect("B is imported");
        assert!(
            matches!(b.placement, ImportPlacement::ZeroDatum { .. }),
            "got {:?}",
            b.placement
        );
    }

    /// Coordinates cross IPC as bare f64s, so the conversion is the door that
    /// refuses junk - a NaN or an out-of-range latitude must fail the import
    /// rather than land in a project.
    #[test]
    fn an_invalid_root_datum_fails_the_conversion() {
        for (latitude, longitude, what) in [
            (f64::NAN, 0.0, "NaN latitude"),
            (0.0, f64::INFINITY, "infinite longitude"),
            (91.0, 0.0, "latitude past the pole"),
            (0.0, 181.0, "longitude past the antimeridian"),
        ] {
            let result = convert_tml_with_options(
                &chain_of_three(),
                &TmlImportOptions {
                    root_placement: Some(TmlRootPlacement::ZeroDatum {
                        latitude,
                        longitude,
                    }),
                    ..Default::default()
                },
            );
            assert!(result.is_err(), "{what} must be refused");
        }
    }
}

// ============================================================================
// Review round 2: one definition of "root", and validated coordinates
// ============================================================================

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod root_and_coordinate_tests {
    use super::super::{convert_tml_with_options, TmlFile};
    use super::tests::{chain_of_three, file, section, shot};
    use super::*;

    fn convert(
        tml: &TmlFile,
        opts: &TmlImportOptions,
    ) -> (Vec<crate::models::ImportSectionConfig>, Vec<String>) {
        let (configs, _, warnings) = convert_tml_with_options(tml, opts).unwrap();
        (configs, warnings)
    }

    const TYPED_DATUM: TmlRootPlacement = TmlRootPlacement::ZeroDatum {
        latitude: 20.5,
        longitude: -87.1,
    };

    fn with_datum() -> TmlImportOptions {
        TmlImportOptions {
            root_placement: Some(TYPED_DATUM),
            ..Default::default()
        }
    }

    /// A file whose FIRST section is emptied by the exclusion filter.
    ///
    /// The lowest index and the first section that actually gets created are
    /// then different sections, which is the whole point: the two used to be
    /// separate definitions of "root" and the placement went to the one that
    /// was never built.
    fn first_section_all_excluded() -> TmlFile {
        let mut excluded = shot(1, 0);
        excluded.excluded = true;
        file(vec![
            section("Dropped", 0, vec![excluded]),
            section("Survivor", 1, vec![shot(2, 1), shot(3, 2)]),
        ])
    }

    /// The user typed coordinates, the section they were shown as belonging to
    /// was emptied by a filter they also set, and the coordinates went nowhere:
    /// the survivor took the topology path, found its parent missing, and landed
    /// at the origin with a warning blaming the FILE.
    #[test]
    fn a_typed_datum_survives_the_first_section_being_filtered_away() {
        let tml = first_section_all_excluded();

        // Premise: the first section really is dropped, so "lowest index" and
        // "first created" disagree here.
        let (configs, warnings) = convert_tml_with_options(&tml, &TmlImportOptions::default())
            .map(|(c, _, w)| (c, w))
            .unwrap();
        assert_eq!(configs.len(), 1, "premise: only the survivor is created");
        assert_eq!(configs[0].section_index, 1);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("had no shots after filtering")),
            "premise: the drop is reported, got {warnings:?}"
        );

        let (configs, _, _) = convert_tml_with_options(&tml, &with_datum()).unwrap();
        assert_eq!(configs.len(), 1);
        let ImportPlacement::ZeroDatum {
            coordinates,
            station_index,
        } = configs[0].placement
        else {
            panic!(
                "the typed datum must reach the section that is actually created, got {:?}",
                configs[0].placement
            );
        };
        assert_eq!(station_index, 0);
        assert!((coordinates.latitude - 20.5).abs() < 1e-9);
    }

    /// The same disagreement, reached through the section FILTER rather than the
    /// shot filter: index 1 is selected but empty, so index 2 is the root.
    #[test]
    fn the_root_is_the_first_created_section_not_the_lowest_selected() {
        let mut tml = chain_of_three();
        for s in &mut tml.sections[1].shots {
            s.excluded = true;
        }

        let opts = TmlImportOptions {
            selected_sections: Some([1, 2].into_iter().collect()),
            root_placement: Some(TYPED_DATUM),
            ..Default::default()
        };
        let (configs, _, _) = convert_tml_with_options(&tml, &opts).unwrap();
        assert_eq!(configs.len(), 1, "only C survives the two filters");
        assert_eq!(configs[0].section_index, 2);
        assert!(
            matches!(configs[0].placement, ImportPlacement::ZeroDatum { .. }),
            "got {:?}",
            configs[0].placement
        );
    }

    /// Defense in depth for the same class of bug: if the placement is ever
    /// dropped on the floor again, the receipt says so instead of the survey
    /// quietly appearing at (0, 0).
    #[test]
    fn an_unconsumed_root_placement_is_reported() {
        // Nothing at all is imported, so nothing can carry the placement.
        let (configs, _, warnings) = convert_tml_with_options(
            &chain_of_three(),
            &TmlImportOptions {
                selected_sections: Some(HashSet::new()),
                root_placement: Some(TYPED_DATUM),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(configs.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("placement you chose was not applied")),
            "expected an unconsumed-placement warning, got: {warnings:?}"
        );
    }

    /// And it stays quiet when the placement DID land, so the warning keeps
    /// meaning something.
    #[test]
    fn a_consumed_root_placement_is_not_reported() {
        let (_, _, warnings) = convert_tml_with_options(&chain_of_three(), &with_datum()).unwrap();
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("placement you chose was not applied")),
            "the placement landed; nothing to report: {warnings:?}"
        );
    }

    // -----------------------------------------------------------------
    // File-supplied coordinates are untrusted too
    // -----------------------------------------------------------------

    fn with_gps(latitude: f64, longitude: f64) -> TmlFile {
        let mut first = shot(1, 0);
        first.latitude = latitude;
        first.longitude = longitude;
        file(vec![section("A", 0, vec![first, shot(2, 1)])])
    }

    /// A crafted `.tml` must not be able to write a junk datum into a project.
    /// NaN is the sharp case: every comparison against it is false, so it slips
    /// straight through the `abs() <= EPSILON` null-island guard.
    #[test]
    fn a_file_supplied_coordinate_that_fails_validation_is_refused() {
        assert_eq!(
            f64::NAN.partial_cmp(&f64::EPSILON),
            None,
            "premise: NaN is incomparable, so the null-island guard's `<=` is false \
             for it and only validation can stop it"
        );

        for (latitude, longitude, what) in [
            (f64::NAN, 10.0, "NaN latitude"),
            (10.0, f64::NAN, "NaN longitude"),
            (1e300, 10.0, "latitude past the pole"),
            (10.0, 1e300, "longitude past the antimeridian"),
            (f64::INFINITY, 10.0, "infinite latitude"),
        ] {
            let (configs, _, _) = convert_tml_with_options(
                &with_gps(latitude, longitude),
                &TmlImportOptions::default(),
            )
            .unwrap();
            assert!(
                matches!(configs[0].placement, ImportPlacement::Origin),
                "{what} must fall back to the origin, got {:?}",
                configs[0].placement
            );
        }
    }

    /// The valid case still anchors, so the refusal above is not just "GPS never
    /// works any more".
    #[test]
    fn a_valid_file_supplied_coordinate_still_anchors() {
        let (configs, _, _) =
            convert_tml_with_options(&with_gps(20.5, -87.1), &TmlImportOptions::default()).unwrap();
        assert!(
            matches!(configs[0].placement, ImportPlacement::ZeroDatum { .. }),
            "got {:?}",
            configs[0].placement
        );
    }

    /// A non-root section with junk coordinates falls back the same way, and
    /// says the file disconnected it rather than silently anchoring on NaN.
    #[test]
    fn a_non_root_section_with_junk_coordinates_falls_back_with_a_warning() {
        let mut orphan = shot(9, 999);
        orphan.latitude = f64::NAN;
        orphan.longitude = f64::NAN;
        let tml = file(vec![
            section("A", 0, vec![shot(1, 0)]),
            section("Orphan", 999, vec![orphan]),
        ]);

        let (configs, _, warnings) =
            convert_tml_with_options(&tml, &TmlImportOptions::default()).unwrap();
        let orphan_config = configs
            .iter()
            .find(|c| c.section_index == 1)
            .expect("the orphan is imported");
        assert!(
            matches!(orphan_config.placement, ImportPlacement::Origin),
            "got {:?}",
            orphan_config.placement
        );
        assert!(
            warnings.iter().any(|w| w.contains("was disconnected")),
            "expected the disconnected warning, got: {warnings:?}"
        );
    }

    // ---- #576 D39(j) / NF-AN4-4: the GPS lands on the station it was measured at

    /// The file's GPS (written on the START, copied onto the shots leaving it)
    /// sits on the raw first shot. Here that shot is EXCLUDED: a 10 m leg due
    /// east. Without the include-excluded toggle the section is built from
    /// its second shot, so its station 0 is 10 m east of where the GPS was
    /// taken.
    fn gps_on_an_excluded_first_shot(root: bool) -> TmlFile {
        let mut lead = shot(1, 0);
        lead.excluded = true;
        lead.latitude = 20.5;
        lead.longitude = -87.3;
        let anchored = section("Anchored", 0, vec![lead, shot(2, 1), shot(3, 2)]);
        if root {
            file(vec![anchored])
        } else {
            // Not connected to the root by topology: placed by its own GPS.
            file(vec![section("Root", 10, vec![shot(11, 10)]), anchored])
        }
    }

    /// The coordinates a section was anchored at, from its placement.
    fn datum_of(config: &crate::models::ImportSectionConfig) -> (f64, f64) {
        match &config.placement {
            ImportPlacement::ZeroDatum {
                coordinates,
                station_index: 0,
            } => (coordinates.latitude, coordinates.longitude),
            other => panic!("expected a station-0 zero datum, got {other:?}"),
        }
    }

    /// Where 10 m due east of the file's GPS is.
    fn ten_metres_east() -> (f64, f64) {
        let measured = GpsCoordinates::new(20.5, -87.3, 0.0).unwrap();
        crate::models::local_to_gps(10.0, 0.0, &measured).unwrap()
    }

    fn assert_close(actual: (f64, f64), expected: (f64, f64)) {
        assert!(
            (actual.0 - expected.0).abs() < 1e-12 && (actual.1 - expected.1).abs() < 1e-12,
            "anchored at {actual:?}, expected {expected:?}"
        );
    }

    #[test]
    fn a_dropped_first_shot_carries_the_root_gps_to_the_first_imported_station() {
        let tml = gps_on_an_excluded_first_shot(true);
        let (configs, warnings) = convert(&tml, &TmlImportOptions::default());
        assert_eq!(configs[0].shots.len(), 2, "the excluded shot is not built");
        assert_close(datum_of(&configs[0]), ten_metres_east());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_dropped_first_shot_carries_a_detached_sections_gps_too() {
        let tml = gps_on_an_excluded_first_shot(false);
        let (configs, warnings) = convert(&tml, &TmlImportOptions::default());
        let anchored = configs
            .iter()
            .find(|c| c.name.as_deref() == Some("Anchored"))
            .unwrap();
        assert_close(datum_of(anchored), ten_metres_east());
        assert!(
            warnings.is_empty(),
            "placed by its GPS, not re-anchored: {warnings:?}"
        );
    }

    /// The dialog's "use the position in the file" sends the root's own
    /// first-shot coordinates back verbatim; that choice follows the same rule.
    #[test]
    fn choosing_the_files_own_position_follows_the_same_rule() {
        let tml = gps_on_an_excluded_first_shot(true);
        let opts = TmlImportOptions {
            root_placement: Some(TmlRootPlacement::ZeroDatum {
                latitude: 20.5,
                longitude: -87.3,
            }),
            ..TmlImportOptions::default()
        };
        let (configs, _) = convert(&tml, &opts);
        assert_close(datum_of(&configs[0]), ten_metres_east());

        // Typed coordinates are the user's station 0, exactly as typed.
        let typed = TmlImportOptions {
            root_placement: Some(TmlRootPlacement::ZeroDatum {
                latitude: 20.6,
                longitude: -87.3,
            }),
            ..TmlImportOptions::default()
        };
        let (configs, _) = convert(&tml, &typed);
        assert_close(datum_of(&configs[0]), (20.6, -87.3));
    }

    /// With the toggle on nothing is dropped, so the GPS stays on station 0.
    #[test]
    fn an_imported_first_shot_keeps_the_gps_on_station_zero() {
        let tml = gps_on_an_excluded_first_shot(true);
        let opts = TmlImportOptions {
            include_excluded: true,
            ..TmlImportOptions::default()
        };
        let (configs, _) = convert(&tml, &opts);
        assert_eq!(configs[0].shots.len(), 3);
        assert_close(datum_of(&configs[0]), (20.5, -87.3));
    }

    /// GEO-D1: carried across the antimeridian, the datum is still the fix 10 m
    /// east of the file's. From 179.99995 E that is -179.9999541; the #576
    /// `local_to_gps` returned it as the continuous 180.0000459, which
    /// `GpsCoordinates::new` refuses, so the import anchored the first imported
    /// station at the file's own fix instead, 10 m short of where it is.
    #[test]
    fn a_dropped_first_shot_carries_the_gps_across_the_antimeridian() {
        let mut tml = gps_on_an_excluded_first_shot(true);
        tml.sections[0].shots[0].longitude = 179.999_95;
        let (configs, warnings) = convert(&tml, &TmlImportOptions::default());
        let (lat, lon) = datum_of(&configs[0]);
        let measured = GpsCoordinates::new(20.5, 179.999_95, 0.0).unwrap();
        let datum = GpsCoordinates::new(lat, lon, 0.0).unwrap();
        let (east, north) = crate::models::gps_to_local(&datum, &measured);
        assert!(
            (east - 10.0).hypot(north) < 1e-6,
            "datum at ({lat}, {lon}): ({east:.3}, {north:.3}) m from the file's fix, \
             expected (10, 0)"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

// ============================================================================
// #576 RV3-R-8: the carried datum turns with a magnetic north reference
// ============================================================================

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]
mod magnetic_datum_tests {
    use chrono::NaiveDate;

    use super::super::{convert_tml_with_options, TmlGeoCoding};
    use super::tests::{file, section, shot};
    use super::*;

    /// The D39(j) layout - a 10 m leg at a MEASURED bearing of 90, excluded,
    /// in front of the shots the section is built from, with the file's GPS on
    /// it - in a dated, georeferenced file whose azimuths are magnetic. The
    /// carried datum has to follow the leg's TRUE bearing, 90 plus the
    /// declination, exactly as the import draws the shots after it.
    #[test]
    fn a_dropped_first_shot_carries_the_gps_along_its_true_bearing() {
        let (lat, lon) = (45.0, -70.0);
        let mut lead = shot(1, 0);
        lead.excluded = true;
        lead.latitude = lat;
        lead.longitude = lon;
        // The declination is evaluated at the survey date; one trip, so the
        // shots carry the section's date.
        let date = NaiveDate::from_ymd_opt(2020, 6, 15);
        let mut shots = vec![lead, shot(2, 1), shot(3, 2)];
        for s in &mut shots {
            s.date = date;
        }
        let mut anchored = section("Anchored", 0, shots);
        anchored.date = date;
        let mut tml = file(vec![anchored]);
        tml.use_magnetic_azimuth = true;
        tml.geo_coding = Some(TmlGeoCoding {
            latitude: lat,
            longitude: lon,
            altitude: 0.0,
        });

        let (configs, _, warnings) =
            convert_tml_with_options(&tml, &TmlImportOptions::default()).unwrap();
        let reference = configs[0].azimuth_reference.expect("a recorded reference");
        let declination = reference.applied_correction();
        let measured = GpsCoordinates::new(lat, lon, 0.0).unwrap();
        let bearing = reference.to_true(90.0).to_radians();
        let expected =
            crate::models::local_to_gps(10.0 * bearing.sin(), 10.0 * bearing.cos(), &measured)
                .unwrap();
        let unrotated = crate::models::local_to_gps(10.0, 0.0, &measured).unwrap();
        let actual = match &configs[0].placement {
            ImportPlacement::ZeroDatum {
                coordinates,
                station_index: 0,
            } => (coordinates.latitude, coordinates.longitude),
            other => panic!("expected a station-0 zero datum, got {other:?}"),
        };
        println!(
            "declination {declination:.3} deg; datum {actual:?}, expected {expected:?} \
             (10 m at {:.3} deg true), not {unrotated:?} (10 m due east)",
            reference.to_true(90.0)
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            declination.abs() > 5.0,
            "premise: a declination worth rotating by, got {declination}"
        );
        assert!(
            (actual.0 - expected.0).abs() < 1e-12 && (actual.1 - expected.1).abs() < 1e-12,
            "anchored at {actual:?}, expected {expected:?}"
        );
    }
}
