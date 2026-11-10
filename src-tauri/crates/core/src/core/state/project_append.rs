//! The one routine that copies a whole project's survey into another (#674).
//!
//! Shared by "Merge projects" (a NEW empty project, then this once per source)
//! and "Add .stalx to current project" (this once, into the OPEN project). One
//! code path, so the two features cannot drift apart on what a copy carries.

use std::collections::{HashMap, HashSet};

use crate::models::{
    LoopClosure, Project, SectionData, SectionId, StationKey, StationMerge, StationRef,
};

/// `(target section id, source shot uuid)` -> the uuid that shot was given in
/// the target, for every copied shot whose identity was already taken there.
pub(super) type RemintedUuids = HashMap<(SectionId, String), String>;

/// What [`append_project`] did beyond the copy itself.
pub(super) struct AppendOutcome {
    /// Source section id -> the id it was given in the target, one entry per
    /// copied section.
    pub(super) section_id_map: HashMap<SectionId, SectionId>,
    /// The target ids in the order they were assigned (the source's section
    /// order), so callers report and record them deterministically.
    pub(super) new_section_ids: Vec<SectionId>,
    /// The copied shots that had to take a fresh uuid because the target
    /// already held theirs. A caller that carries anything keyed by a SOURCE
    /// shot uuid (the add's ignored warnings) translates it through this.
    pub(super) reminted_uuids: RemintedUuids,
    /// `(incoming, kept)` when the source's entrance elevation disagreed with
    /// the one the target already held by more than the import tolerance. The
    /// target keeps its own; the caller decides how to tell the user.
    pub(super) elevation_conflict: Option<(f64, f64)>,
}

/// Append every section of `source` to `target` with fresh ids, and carry
/// everything that hangs off those sections: section and station offsets,
/// station comments, zero datums, station merges, loop closures (id 0, for the
/// caller's `assign_closure_ids`), each section's resolved north reference and
/// least-squares gauge (reverse parity and gauge anchors), the entrance
/// elevation (first one wins) and the location.
///
/// Cross-references are copied as TRANSITIONAL positional keys
/// (`StationKey::positional`); the caller canonicalizes them (merge through
/// `redrive_offsets_after_load`, add through `reanchor_station_refs`).
///
/// Every copied shot keeps its uuid unless the target already holds it (or an
/// earlier copied shot took it): then the copy gets a fresh one, so no shot
/// uuid the copy brings in appears twice in the project. The copy is the side
/// that loses its undo history, so nothing refers to its old ids; the
/// target's own shots, which its undo history names, are never changed - nor
/// are any repeats the target already held.
///
/// Only references between this source's own sections are copied - the source
/// cannot name a target station. A reference to a section the source does not
/// hold, or to a station that does not resolve, is skipped with a log line.
///
/// # Errors
///
/// Only when a zero datum cannot be stored. By then `target` already holds the
/// sections copied so far: merge discards its fresh project, the add removes
/// what was appended. Nothing here rolls back on its own.
pub(super) fn append_project(
    target: &mut Project,
    source: &Project,
) -> Result<AppendOutcome, String> {
    let mut section_id_map: HashMap<SectionId, SectionId> = HashMap::new();
    let mut new_section_ids: Vec<SectionId> = Vec::with_capacity(source.sections.len());
    let mut reminted_uuids = RemintedUuids::new();
    let mut taken: HashSet<String> = target
        .sections
        .iter()
        .flat_map(|s| s.shots.iter())
        .filter_map(|shot| shot.uuid.clone())
        .collect();

    for section in &source.sections {
        let old_section_id = section.id;
        let new_section_id = target.next_section_id;

        // Shot identity rides along in `SectionData` (`ShotData::uuid`), so
        // `add_section` reproduces each source shot's uuid by itself - except
        // where the target already holds it, which `claim_unique_uuids`
        // re-mints first. Settled BEFORE anything is keyed on these shots:
        // `copy_zero_datums` canonicalizes to `AfterShot(uuid)` at once, so a
        // later re-mint would leave the datum naming a uuid the section no
        // longer has. Positions are untouched (source shot i is target shot
        // i), which is what the positional cross-references below rely on.
        let mut data = section.to_section_data();
        let changed = claim_unique_uuids(&mut data, &mut taken);
        target.add_section(data);

        // #438: `SectionData` cannot carry the north reference, so the
        // copy would land on the target's default - reverting a
        // corrected section to identity and rendering (and exporting) it
        // rotated by its own declination.
        //
        // The RESOLVED reference, from the section's OWN project: a
        // section that inherited its source project's default has to
        // keep that value here, where the default is a different one.
        // Merging the defaults themselves would be the wrong question -
        // two projects can hold different ones legitimately, and only
        // the per-section answer is unambiguous - so every copied
        // section carries its own and the target's default is untouched.
        let resolved = source.azimuth_reference_for(section);
        if let Some(copied) = target.get_section_mut(new_section_id) {
            copied.azimuth_reference = Some(resolved);
            // Nor can it carry where the section's least-squares gauge sits
            // (#24): `Section::new` resets the reverse parity and both gauge
            // anchors, which would pin a datum-less component to a different
            // station than the source's and translate its adjusted geometry.
            // The anchors are shot uuids, so a re-minted shot's anchor follows
            // it: a stale one would not resolve, and the gauge would fall back
            // to the index rule for good.
            let follow = |uuid: &Option<String>| -> Option<String> {
                uuid.as_ref()
                    .map(|u| changed.get(u).cloned().unwrap_or_else(|| u.clone()))
            };
            copied.reverse_parity = section.reverse_parity;
            copied.gauge_shot_uuid = follow(&section.gauge_shot_uuid);
            copied.gauge_neighbour_uuid = follow(&section.gauge_neighbour_uuid);
        }

        reminted_uuids.extend(
            changed
                .into_iter()
                .map(|(old, new)| ((new_section_id, old), new)),
        );
        section_id_map.insert(old_section_id, new_section_id);
        new_section_ids.push(new_section_id);
    }
    if !reminted_uuids.is_empty() {
        tracing::info!(
            project = %source.name,
            reminted = reminted_uuids.len(),
            purpose = "keep copied shot uuids unique across the project",
            "Copied shots whose uuid the project already held were given fresh ones"
        );
    }

    copy_offsets_and_comments(target, source, &section_id_map);
    copy_zero_datums(target, source, &section_id_map)?;
    copy_merges_and_closures(target, source, &section_id_map);
    let elevation_conflict = combine_surface_elevation(target, source);
    combine_location(target, source);

    Ok(AppendOutcome {
        section_id_map,
        new_section_ids,
        reminted_uuids,
        elevation_conflict,
    })
}

/// Give each shot of `data` a uuid no shot in `taken` holds, claiming it.
///
/// A shot whose uuid is already taken - by the target, by a section copied
/// earlier, or by an earlier shot of this same section - gets a fresh v4. The
/// returned map (source uuid -> fresh uuid) holds only a uuid's FIRST
/// occurrence in the section: that is the shot a source reference to that
/// uuid names (`resolve_key` takes the first match), so it is the one a
/// translation must follow. A later in-section repeat is re-minted too (as
/// `Section::new` would), but nothing can name it.
///
/// A missing or empty uuid is minted here too (as `Section::new` would), so
/// every id the section ends up with is in `taken` when this returns.
fn claim_unique_uuids(
    data: &mut SectionData,
    taken: &mut HashSet<String>,
) -> HashMap<String, String> {
    let mut changed: HashMap<String, String> = HashMap::new();
    let mut seen_here: HashSet<String> = HashSet::new();
    for shot in &mut data.shots {
        let old = shot.uuid.clone().filter(|u| !u.is_empty());
        let first_here = match &old {
            Some(id) => {
                let first_here = seen_here.insert(id.clone());
                if taken.insert(id.clone()) {
                    continue; // nobody holds it yet: the shot keeps it
                }
                first_here
            }
            None => false,
        };
        let fresh = uuid::Uuid::new_v4().to_string();
        taken.insert(fresh.clone());
        if let (Some(old), true) = (old, first_here) {
            changed.insert(old, fresh.clone());
        }
        shot.uuid = Some(fresh);
    }
    changed
}

/// Section offsets, station offsets and station comments, remapped.
fn copy_offsets_and_comments(
    target: &mut Project,
    source: &Project,
    section_id_map: &HashMap<SectionId, SectionId>,
) {
    // Through `write_section_offset` so an all-zero entry a source project is
    // carrying (a file written before #413 finding 2 was fixed) is not copied
    // forward and re-serialized: the map means "sections that are actually
    // displaced", and (0, 0) is the same geometry as no entry at all.
    for (old_section_id, offset) in &source.section_offsets {
        if let Some(&new_section_id) = section_id_map.get(old_section_id) {
            Project::write_section_offset(&mut target.section_offsets, new_section_id, *offset);
        } else {
            tracing::warn!(
                project = %source.name,
                section_id = old_section_id,
                purpose = "append section offsets",
                "Section offset references unknown section ID; skipping"
            );
        }
    }

    // Resolve the source key to a positional index against the source, then
    // store as a transitional Index anchor under the new section; the caller's
    // re-anchor pass canonicalizes it once the copied shots are present.
    for (old_key, offset) in &source.station_offsets {
        if let Some(new_key) = remap_key(source, section_id_map, old_key, "append station offsets")
        {
            target.station_offsets.insert(new_key, *offset);
        }
    }

    // Same transitional Index-anchor approach as station_offsets above.
    // (Without this, every per-station comment was silently dropped on merge
    // while every sibling collection was copied.)
    for (old_key, comment) in &source.station_comments {
        if let Some(new_key) = remap_key(source, section_id_map, old_key, "append station comments")
        {
            target.station_comments.insert(new_key, comment.clone());
        }
    }
    // Station names ride the same way.
    for (old_key, name) in &source.station_names {
        if let Some(new_key) = remap_key(source, section_id_map, old_key, "append station names") {
            target.station_names.insert(new_key, name.clone());
        }
    }
}

/// A source station key as a transitional positional key in the target, or
/// `None` when its section was not copied (logged) or it does not resolve.
fn remap_key(
    source: &Project,
    section_id_map: &HashMap<SectionId, SectionId>,
    old_key: &StationKey,
    purpose: &'static str,
) -> Option<StationKey> {
    let Some(&new_section_id) = section_id_map.get(&old_key.section_id) else {
        tracing::warn!(
            project = %source.name,
            section_id = old_key.section_id,
            purpose,
            "Station reference names an unknown section ID; skipping"
        );
        return None;
    };
    let src_sr = source.resolve_key(old_key)?;
    Some(StationKey::positional(new_section_id, src_sr.station_index))
}

/// Zero datums, remapped, stored WITHOUT repositioning: the section offsets
/// were already copied, and a datum placed through `set_zero_datum` would move
/// the section it anchors.
fn copy_zero_datums(
    target: &mut Project,
    source: &Project,
    section_id_map: &HashMap<SectionId, SectionId>,
) -> Result<(), String> {
    for zd in &source.zero_datums {
        if let Some(&new_section_id) = section_id_map.get(&zd.station_ref.section_id) {
            let Some(src_sr) = source.resolve_key(&zd.station_ref) else {
                continue;
            };
            let new_station_ref = StationRef::new(new_section_id, src_sr.station_index);
            target
                .store_zero_datum_without_repositioning(new_station_ref, zd.gps)
                .map_err(|e| {
                    format!(
                        "Failed to copy zero datum from source project {}: {}",
                        source.name, e
                    )
                })?;
        } else {
            tracing::warn!(
                project = %source.name,
                section_id = zd.station_ref.section_id,
                purpose = "append zero datums",
                "Zero datum references unknown section ID; skipping"
            );
        }
    }
    Ok(())
}

/// Both endpoints of a source merge/closure remapped, or `None` (logged when a
/// section was not copied; silent when a station does not resolve).
fn remap_pair(
    source: &Project,
    section_id_map: &HashMap<SectionId, SectionId>,
    a: &StationKey,
    b: &StationKey,
    purpose: &'static str,
) -> Option<(StationKey, StationKey)> {
    let (Some(&new_a_section), Some(&new_b_section)) = (
        section_id_map.get(&a.section_id),
        section_id_map.get(&b.section_id),
    ) else {
        tracing::warn!(
            project = %source.name,
            station_a_section = a.section_id,
            station_b_section = b.section_id,
            purpose,
            "Station pair references unknown section ID; skipping"
        );
        return None;
    };
    let (Some(a_sr), Some(b_sr)) = (source.resolve_key(a), source.resolve_key(b)) else {
        return None;
    };
    Some((
        StationKey::positional(new_a_section, a_sr.station_index),
        StationKey::positional(new_b_section, b_sr.station_index),
    ))
}

/// Station merges and loop closures, remapped (only within the same source).
fn copy_merges_and_closures(
    target: &mut Project,
    source: &Project,
    section_id_map: &HashMap<SectionId, SectionId>,
) {
    for merge in &source.station_merges {
        if let Some((station_a, station_b)) = remap_pair(
            source,
            section_id_map,
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
            section_id_map,
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
