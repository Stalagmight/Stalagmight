//! The one routine that copies a whole project's survey into another (#674).
//!
//! Shared by "Merge projects" (a NEW empty project, then this once per source)
//! and "Add .stalx to current project" (this once, into the OPEN project). One
//! code path, so the two features cannot drift apart on what a copy carries.

use std::collections::{HashMap, HashSet};

use crate::models::{
    gps_to_local, GpsCoordinates, HeldGauge, LineId, LoopClosure, Project, SectionId,
    ShotUuidRemap, StationKey, StationMerge, StationRef,
};

/// What [`append_project`] did beyond the copy itself.
pub(super) struct AppendOutcome {
    /// Source line id -> the id it was given in the target, one entry per
    /// copied line.
    pub(super) line_id_map: HashMap<LineId, LineId>,
    /// The target ids in the order they were assigned (the source's line
    /// order), so callers report and record them deterministically.
    pub(super) new_line_ids: Vec<LineId>,
    /// `(incoming, kept)` when the source's entrance elevation disagreed with
    /// the one the target already held by more than the import tolerance. The
    /// target keeps its own; the caller decides how to tell the user.
    pub(super) elevation_conflict: Option<(f64, f64)>,
    /// (target line, SOURCE shot uuid) -> the uuid that shot holds in the
    /// target, for the copied shots whose id changed on the way in. Anything
    /// keyed by a source uuid (the Add's carried ignores) is translated through
    /// [`Self::target_uuid`].
    pub(super) shot_uuid_map: ShotUuidRemap,
}

impl AppendOutcome {
    /// The uuid the shot that held `source_uuid` in the source holds in
    /// `target_line`.
    pub(super) fn target_uuid(&self, target_line: LineId, source_uuid: &str) -> String {
        self.shot_uuid_map
            .get(target_line, source_uuid)
            .unwrap_or(source_uuid)
            .to_string()
    }
}

/// Append every line of `source` to `target` with fresh ids, and carry
/// everything that hangs off those lines: their Sections (appended, names
/// deduped), line and station offsets,
/// station comments, zero datums, station merges, loop closures (id 0, for the
/// caller's `assign_closure_ids`), each line's resolved north reference and
/// least-squares gauge (reverse parity and gauge anchors), the entrance
/// elevation (first one wins) and the location - and place a georeferenced
/// source at its GPS position in the target's frame (#682; [`place_by_datum`]).
///
/// Cross-references are copied as TRANSITIONAL positional keys
/// (`StationKey::positional`); the caller canonicalizes them (merge through
/// `redrive_offsets_after_load`, add through `reanchor_station_refs`).
///
/// Only references between this source's own lines are copied - the source
/// cannot name a target station. A reference to a line the source does not
/// hold, or to a station that does not resolve, is skipped with a log line.
///
/// # Errors
///
/// Only when a zero datum cannot be stored. By then `target` already holds the
/// lines copied so far: merge discards its fresh project, the add removes
/// what was appended. Nothing here rolls back on its own.
pub(super) fn append_project(
    target: &mut Project,
    source: &Project,
) -> Result<AppendOutcome, String> {
    let mut line_id_map: HashMap<LineId, LineId> = HashMap::new();
    let mut new_line_ids: Vec<LineId> = Vec::with_capacity(source.lines.len());
    // The target's georeference BEFORE this copy: a source datum is placed in
    // it, or, when there is none yet, becomes it.
    let frame_before = target.frame_datum().map(|d| d.gps);
    // Station IDs: the source's own, offset past every id the target has
    // handed out, so both keep theirs and none collides.
    let station_id_base = target.next_station_id;

    for line in &source.lines {
        let old_line_id = line.id;
        let new_line_id = target.next_line_id;

        // Shot identity rides along in `LineData` (`ShotData::uuid`),
        // so `add_line` reproduces each source shot's UUID by itself.
        // Ids the target already holds - two projects that imported the same
        // Ariane file, or two copies of one project - are renamed once the
        // whole source is in (`reassign_repeated_shot_uuids` below): shot
        // UUIDs are unique across the project, and the target keeps its own.
        target.add_line(line.to_line_data());

        // #438: `LineData` cannot carry the north reference, so the
        // copy would land on the target's default - reverting a
        // corrected line to identity and rendering (and exporting) it
        // rotated by its own declination.
        //
        // The RESOLVED reference, from the line's OWN project: a
        // line that inherited its source project's default has to
        // keep that value here, where the default is a different one.
        // Merging the defaults themselves would be the wrong question -
        // two projects can hold different ones legitimately, and only
        // the per-line answer is unambiguous - so every copied
        // line carries its own and the target's default is untouched.
        let resolved = source.azimuth_reference_for(line);
        if let Some(copied) = target.get_line_mut(new_line_id) {
            copied.azimuth_reference = Some(resolved);
            // Nor can it carry where the line's least-squares gauge sits
            // (#24): `Line::new` resets the reverse parity and both gauge
            // anchors, which would pin a datum-less component to a different
            // station than the source's and translate its adjusted geometry.
            // The anchors are shot uuids, carried over by POSITION below, once
            // the copy's own ids are settled.
            copied.reverse_parity = line.reverse_parity;
        }

        line_id_map.insert(old_line_id, new_line_id);
        new_line_ids.push(new_line_id);
    }

    target.reassign_repeated_shot_uuids(&new_line_ids);
    let shot_uuid_map = carry_shot_identity(target, source, &line_id_map);
    copy_sections(target, source, &line_id_map);

    copy_offsets_and_comments(target, source, &line_id_map);
    copy_station_ids(target, source, &line_id_map, station_id_base);
    copy_zero_datums(target, source, &line_id_map)?;
    copy_merges_and_closures(target, source, &line_id_map);
    // Last, so the copy is complete when its datum station is measured.
    place_by_datum(target, &new_line_ids, frame_before);
    let elevation_conflict = combine_surface_elevation(target, source);
    combine_location(target, source);

    Ok(AppendOutcome {
        line_id_map,
        new_line_ids,
        elevation_conflict,
        shot_uuid_map,
    })
}

/// Translate the copied lines so a georeferenced source lands at its GPS
/// position: its first resolvable datum's station (the source's frame, and a
/// placement datum in the target too: the copy's datums are appended after the
/// target's and its merges stay inside the copy, #741) goes to that datum's local
/// position in the target's frame (`frame_before`, the target's frame datum
/// before this copy), or to the origin when the target had no frame (the copied
/// datum then BECOMES the frame).
///
/// The whole copy moves rigidly, unanchored components included, so the
/// source's own layout is preserved. Nothing pre-existing moves. A source with
/// no datum is left where it is - placing it is the orphan-placement dialog's
/// job, not a guess made here.
fn place_by_datum(project: &mut Project, new_ids: &[LineId], frame_before: Option<GpsCoordinates>) {
    let new: HashSet<LineId> = new_ids.iter().copied().collect();
    let Some((station, gps)) = project
        .zero_datums
        .iter()
        .filter(|zd| new.contains(&zd.station_ref.line_id))
        .find_map(|zd| project.resolve_key(&zd.station_ref).map(|sr| (sr, zd.gps)))
    else {
        return;
    };
    let Some(current) = project.compute_station_raw_coords(station) else {
        return;
    };
    let (target_x, target_y) = frame_before.map_or((0.0, 0.0), |frame| gps_to_local(&gps, &frame));
    let (dx, dy) = (target_x - current.x, target_y - current.y);
    // Non-finite is refused at the read; this keeps a NaN (which passes the dust
    // test below) from ever reaching an offset should another route produce one.
    if !dx.is_finite() || !dy.is_finite() {
        tracing::warn!(
            purpose = "place a copied source by its datum",
            "Datum placement is not finite; leaving the copy where it is"
        );
        return;
    }
    if dx.abs() <= Project::OFFSET_DUST_M && dy.abs() <= Project::OFFSET_DUST_M {
        return;
    }
    for id in new_ids {
        project.adjust_line_offset(*id, dx, dy);
    }
}

/// Map every copied shot's SOURCE uuid to the uuid it holds in the target, by
/// position (a copy keeps shot order), keeping only the ones that differ: ids
/// renamed because the target already held them, and in-line repeats
/// `Line::new` re-minted. Then carry each line's gauge pair over through
/// that map, so it names the same two shots in the copy as in the source.
///
/// Each copy's record parity comes from the source too: its entry in the
/// source's `counter_parity_shots`, set or cleared on the copy's own uuid.
/// Never from the target's set, whose entry for a shared old id belongs to the
/// target's shot (`reassign_repeated_shot_uuids` does not follow it).
fn carry_shot_identity(
    target: &mut Project,
    source: &Project,
    line_id_map: &HashMap<LineId, LineId>,
) -> ShotUuidRemap {
    let mut renamed = ShotUuidRemap::default();
    let mut counter_parity: Vec<(String, bool)> = Vec::new();
    for line in &source.lines {
        let Some(&new_line_id) = line_id_map.get(&line.id) else {
            continue;
        };
        let Some(copied) = target.get_line_mut(new_line_id) else {
            continue;
        };
        let mut by_source: HashMap<&str, String> = HashMap::new();
        for (from, to) in line.shots.iter().zip(&copied.shots) {
            let (Some(from), Some(to)) = (from.uuid.as_deref(), to.uuid.as_deref()) else {
                continue;
            };
            by_source.entry(from).or_insert_with(|| to.to_string());
            counter_parity.push((to.to_string(), source.counter_parity_shots.contains(from)));
            if from != to && renamed.get(new_line_id, from).is_none() {
                renamed.insert(new_line_id, from.to_string(), to.to_string());
            }
        }
        let carry = |anchor: &Option<String>| {
            anchor.as_deref().map(|uuid| {
                by_source
                    .get(uuid)
                    .cloned()
                    .unwrap_or_else(|| uuid.to_string())
            })
        };
        copied.gauge_shot_uuid = carry(&line.gauge_shot_uuid);
        copied.gauge_neighbour_uuid = carry(&line.gauge_neighbour_uuid);
    }
    for (uuid, counter) in counter_parity {
        if counter {
            target.counter_parity_shots.insert(uuid);
        } else {
            target.counter_parity_shots.remove(&uuid);
        }
    }
    // The source's held gauges (`Project::held_gauges`) keep its components'
    // adjustment on the same stations in the copy, through the copy's uuids.
    for held in &source.held_gauges {
        let Some(&to) = line_id_map.get(&held.line_id) else {
            continue;
        };
        let carry = |uuid: &str| {
            renamed
                .get(to, uuid)
                .map_or_else(|| uuid.to_string(), str::to_string)
        };
        target.held_gauges.push(HeldGauge {
            line_id: to,
            shot: carry(&held.shot),
            neighbour: held.neighbour.as_deref().map(carry),
            ..held.clone()
        });
    }
    // One per component, whatever the source file held.
    target.normalize_held_gauges();
    renamed
}

/// Sections (#327): every source Section is appended to the target's with a
/// fresh id and a name made unique among the target's (`Upper level` beside
/// the target's own `Upper level` becomes `Upper level (2)`), and every copied
/// line keeps its membership through the remap. A Section with no copied
/// line is appended too: it is part of the source's survey.
fn copy_sections(target: &mut Project, source: &Project, line_id_map: &HashMap<LineId, LineId>) {
    let mut section_map: HashMap<SectionId, SectionId> = HashMap::new();
    for section in &source.sections {
        let name = target.unique_section_name(&section.name);
        let id = target.add_section(name, section.color.clone());
        section_map.insert(section.id, id);
    }
    for line in &source.lines {
        let (Some(&copied), Some(section)) = (line_id_map.get(&line.id), line.section_id) else {
            continue;
        };
        target.set_line_section(copied, section_map.get(&section).copied());
    }
}

/// Line offsets, station offsets and station comments, remapped.
fn copy_offsets_and_comments(
    target: &mut Project,
    source: &Project,
    line_id_map: &HashMap<LineId, LineId>,
) {
    // Through `write_line_offset` so an all-zero entry a source project is
    // carrying (a file written before #413 finding 2 was fixed) is not copied
    // forward and re-serialized: the map means "lines that are actually
    // displaced", and (0, 0) is the same geometry as no entry at all.
    for (old_line_id, offset) in &source.line_offsets {
        if let Some(&new_line_id) = line_id_map.get(old_line_id) {
            Project::write_line_offset(&mut target.line_offsets, new_line_id, *offset);
        } else {
            tracing::warn!(
                project = %source.name,
                line_id = old_line_id,
                purpose = "append line offsets",
                "Line offset references unknown line ID; skipping"
            );
        }
    }

    // Resolve the source key to a positional index against the source, then
    // store as a transitional Index anchor under the new line; the caller's
    // re-anchor pass canonicalizes it once the copied shots are present.
    for (old_key, offset) in &source.station_offsets {
        if let Some(new_key) = remap_key(source, line_id_map, old_key, "append station offsets") {
            target.station_offsets.insert(new_key, *offset);
        }
    }

    // Same transitional Index-anchor approach as station_offsets above.
    // (Without this, every per-station comment was silently dropped on merge
    // while every sibling collection was copied.)
    for (old_key, comment) in &source.station_comments {
        if let Some(new_key) = remap_key(source, line_id_map, old_key, "append station comments") {
            target.station_comments.insert(new_key, comment.clone());
        }
    }
    // Station names ride the same way.
    for (old_key, name) in &source.station_names {
        if let Some(new_key) = remap_key(source, line_id_map, old_key, "append station names") {
            target.station_names.insert(new_key, name.clone());
        }
    }
}

/// Station IDs, remapped onto the copied lines and offset by `base` (the
/// target's `next_station_id` before the copy); the target's counter then
/// moves past every id the source could have handed out. Repeated line names
/// are left to the normalization that follows every append (the incoming
/// line is the later holder, so it gets the ` (2)`).
fn copy_station_ids(
    target: &mut Project,
    source: &Project,
    line_id_map: &HashMap<LineId, LineId>,
    base: crate::models::StationId,
) {
    for (old_key, id) in &source.station_ids {
        if let Some(new_key) = remap_key(source, line_id_map, old_key, "append station ids") {
            target.station_ids.insert(new_key, id.saturating_add(base));
        }
    }
    let span = source
        .station_ids
        .values()
        .map(|id| id.saturating_add(1))
        .max()
        .unwrap_or(0)
        .max(source.next_station_id);
    target.next_station_id = target.next_station_id.max(base.saturating_add(span));
}

/// A source station key as a transitional positional key in the target, or
/// `None` when its line was not copied (logged) or it does not resolve.
fn remap_key(
    source: &Project,
    line_id_map: &HashMap<LineId, LineId>,
    old_key: &StationKey,
    purpose: &'static str,
) -> Option<StationKey> {
    let Some(&new_line_id) = line_id_map.get(&old_key.line_id) else {
        tracing::warn!(
            project = %source.name,
            line_id = old_key.line_id,
            purpose,
            "Station reference names an unknown line ID; skipping"
        );
        return None;
    };
    let src_sr = source.resolve_key(old_key)?;
    Some(StationKey::positional(new_line_id, src_sr.station_index))
}

/// Zero datums, remapped, stored WITHOUT repositioning: the line offsets
/// were already copied, and a datum placed through `set_zero_datum` would move
/// the line it anchors.
fn copy_zero_datums(
    target: &mut Project,
    source: &Project,
    line_id_map: &HashMap<LineId, LineId>,
) -> Result<(), String> {
    for zd in &source.zero_datums {
        if let Some(&new_line_id) = line_id_map.get(&zd.station_ref.line_id) {
            let Some(src_sr) = source.resolve_key(&zd.station_ref) else {
                continue;
            };
            let new_station_ref = StationRef::new(new_line_id, src_sr.station_index);
            target
                .store_zero_datum_without_repositioning(new_station_ref, zd.gps, zd.accuracy_m)
                .map_err(|e| {
                    format!(
                        "Failed to copy zero datum from source project {}: {}",
                        source.name, e
                    )
                })?;
        } else {
            tracing::warn!(
                project = %source.name,
                line_id = zd.station_ref.line_id,
                purpose = "append zero datums",
                "Zero datum references unknown line ID; skipping"
            );
        }
    }
    Ok(())
}

/// Both endpoints of a source merge/closure remapped, or `None` (logged when a
/// line was not copied; silent when a station does not resolve).
fn remap_pair(
    source: &Project,
    line_id_map: &HashMap<LineId, LineId>,
    a: &StationKey,
    b: &StationKey,
    purpose: &'static str,
) -> Option<(StationKey, StationKey)> {
    let (Some(&new_a_line), Some(&new_b_line)) =
        (line_id_map.get(&a.line_id), line_id_map.get(&b.line_id))
    else {
        tracing::warn!(
            project = %source.name,
            station_a_line = a.line_id,
            station_b_line = b.line_id,
            purpose,
            "Station pair references unknown line ID; skipping"
        );
        return None;
    };
    let (Some(a_sr), Some(b_sr)) = (source.resolve_key(a), source.resolve_key(b)) else {
        return None;
    };
    Some((
        StationKey::positional(new_a_line, a_sr.station_index),
        StationKey::positional(new_b_line, b_sr.station_index),
    ))
}

/// Station merges and loop closures, remapped (only within the same source).
fn copy_merges_and_closures(
    target: &mut Project,
    source: &Project,
    line_id_map: &HashMap<LineId, LineId>,
) {
    for merge in &source.station_merges {
        if let Some((station_a, station_b)) = remap_pair(
            source,
            line_id_map,
            &merge.station_a,
            &merge.station_b,
            "append station merges",
        ) {
            target.record_station_merge(StationMerge {
                station_a,
                station_b,
                merged_at: merge.merged_at,
            });
        }
    }

    for closure in &source.loop_closures {
        if let Some((station_a, station_b)) = remap_pair(
            source,
            line_id_map,
            &closure.station_a,
            &closure.station_b,
            "append loop closures",
        ) {
            target.loop_closures.push(LoopClosure {
                id: 0, // Reset to 0; the caller's assign_closure_ids() reassigns unique IDs
                station_a,
                station_b,
                path_length: closure.path_length,
                misclosure_distance: closure.misclosure_distance,
                error_percentage: closure.error_percentage,
                horizontal_misclosure: closure.horizontal_misclosure,
                vertical_misclosure: closure.vertical_misclosure,
                strategy: closure.strategy,
                is_aligned: closure.is_aligned,
            });
        }
    }
}

/// Entrance elevation (#431, #448): the first `Some` wins, and a disagreeing
/// source is reported rather than overwriting or averaging. A NaN/infinite
/// incoming value (never produced by a real Ariane import, but not provably
/// impossible upstream) is skipped outright - adopting it would poison every
/// later comparison and export, and comparing against it would always read as
/// "differs" without ever explaining why.
fn combine_surface_elevation(target: &mut Project, source: &Project) -> Option<(f64, f64)> {
    let incoming = source.surface_elevation.filter(|v| v.is_finite())?;
    match target.surface_elevation {
        None => {
            target.surface_elevation = Some(incoming);
            None
        }
        // Strictly greater than 0.005 m (exclusive): a difference AT the
        // boundary is agreement, matching the message's own `{:.2}` rounding
        // (see command_import.rs's identical comparison, which this mirrors).
        Some(existing) if (existing - incoming).abs() > 0.005 => Some((incoming, existing)),
        Some(_) => None,
    }
}

/// Location: adopt the source's when the target has none, otherwise append it
/// unless the target already mentions it.
fn combine_location(target: &mut Project, source: &Project) {
    let Some(ref loc) = source.location else {
        return;
    };
    match target.location {
        None => target.location = Some(loc.clone()),
        Some(ref mut merged_loc) => {
            if !merged_loc.contains(loc.as_str()) {
                merged_loc.push_str(", ");
                merged_loc.push_str(loc);
            }
        }
    }
}
