/**
 * Building the backend `ImportSectionConfig` for an imported DMP section (#24).
 *
 * The wizard has two import paths - the zero-datum anchor, imported on mount
 * straight from `ZeroDatumConfigDialog`, and the batch built from virtual
 * connections - and both have to hand the backend placement indices that
 * address the FINAL, post-edit, post-reverse station order. Keeping that
 * rewrite here (rather than inline in DmpNewImportDialog) gives both paths one
 * implementation and makes it unit-testable without mounting the wizard.
 */
import type { AzimuthReference } from '../types/ipc/AzimuthReference';
import type { ImportPlacement, ImportSectionConfig } from '../types/commands';
import type { EditableShot } from '../components/common/editableShot';
import { isEmittedShot } from '../components/common/emittedShots';
import type { ShotData } from '../types/project';
import type { ZeroDatumConfigResult } from '../components/Dialogs/ZeroDatumConfigDialog';
import type { VirtualConnection } from './segmentContext';
import { importCreditFields, type SectionEditState } from './batchImportHelpers';
import { editableShotToShotData } from './shotConversion';
import { mirrorPlacementForReverse, targetShotCountForMirror } from './reversePlacement';

/**
 * The north reference every MNemo section gets (#438).
 *
 * The device is a compass: its headings are MAGNETIC, and the `.dmp` format
 * carries neither a declination nor a georeference, so there is no number to
 * record - `unknown` says exactly that, and leaves the geometry uncorrected
 * (correction 0), which is what the DMP importer has always produced. Declaring
 * it rather than leaving the field out is load-bearing in one place and
 * documentation in the other. It is load-bearing on the SUGGESTION payload
 * (`ImportSectionForSuggestion.azimuthReference`): the matcher scores incoming
 * bearings against existing sections', and without it the backend falls back to
 * the project default, so a DMP section matched against an existing Compass
 * section would be compared across that section's declination. On the import
 * CONFIG it currently resolves to the same value the default would, and is sent
 * anyway so the section records what it IS rather than inheriting whatever the
 * project later declares.
 */
export const DMP_AZIMUTH_REFERENCE: AzimuthReference = {
  north: 'magnetic',
  correction: 0,
  source: 'unknown',
};

/**
 * The station names (MNemo station numbers) of a section, from the rows that
 * will actually be sent, in that order.
 *
 * Station `k + 1` is the end of emitted row `k`, so it takes that row's
 * `mnemoStation`. The START of a row is the end of the row before it, so it
 * already has that row's number or, after a row the user added, none: the
 * same as an in-app edit, where deleting an interior shot removes only its END
 * station and an inserted shot adds an unnamed one. The one station no earlier
 * DMP row can name is the start of the FIRST one (the section start, or the
 * station after rows the user prepended); it is `mnemoStation - 1`. So a
 * section whose first m shots an overlap trim removed starts at MNemo `m`, not
 * 0, and the trimmed numbers are gone with their stations.
 *
 * Indices are in the emitted (pre-reverse) order: the backend mirrors them for
 * a reversed import, exactly like `station_comments`.
 */
export function stationNamesForEmittedShots(
  emitted: readonly EditableShot[],
): Array<[number, string]> {
  const names: Array<[number, string]> = [];
  emitted.forEach((shot, k) => {
    const end = shot.mnemoStation;
    if (end === undefined) return;
    if (names.length === 0 && end > 0) names.push([k, String(end - 1)]);
    names.push([k + 1, String(end)]);
  });
  return names;
}

/**
 * Rewrite a placement's indices for reverse, on both sides of a connection.
 *
 * q14: the backend reverses a section BEFORE creating it, so every index in its
 * placement addresses the post-reverse order, while the wizard collected those
 * indices against the file's original order.
 *
 * The target side is mirrored ONLY when the placement was derived from a
 * connection, so `connection` must be the connection that produced `placement`
 * or `null`. A direct placement (segment "From existing", or a manual /
 * manual_snap drop) targets a live project section, which this batch does not
 * renumber - while an unrelated auto-derived connection for the same section
 * would mirror the user's chosen station onto the wrong physical end (or out of
 * range, failing the whole batch in merge_stations).
 */
export function finalizePlacementForReverse(
  placement: ImportPlacement,
  selfReversed: boolean,
  selfShots: number,
  connection: VirtualConnection | null | undefined,
  sectionEdits: Map<number, SectionEditState> | undefined,
): ImportPlacement {
  const targetShots = targetShotCountForMirror(
    placement,
    connection?.targetType,
    connection?.targetImportIndex,
    sectionEdits,
  );
  return mirrorPlacementForReverse(placement, selfReversed, selfShots, targetShots);
}

/**
 * Build the backend config for the zero-datum anchor section.
 *
 * `stationIndex` already addresses the POST-EDIT station order: the dropdown is
 * built from the surviving shot count, the preview is drawn from the same
 * surviving shots (`editableShotsToDmpSection`), and every shot-table edit
 * re-anchors the committed pick onto its physical station. So there is nothing
 * left to translate here - it is the index into the `shots` sent below.
 *
 * It is NOT the `EditableShot.id` space `adjustStationForDeletedShots` maps
 * from: those ids are positions in the `isImportableDmpShot`-filtered file
 * order, which already omits non-Standard and length-1 shots. Running that
 * adjustment over a post-edit pick shrank it a second time and moved the GPS
 * anchor one shot down the passage for every deletion, silently - `set_zero_datum`
 * only rejects an out-of-range index.
 *
 * The clamp is the one remaining translation, and it guards the reverse mirror
 * (`shotCount - index`) rather than the pick: a stale index past the last
 * surviving station would mirror NEGATIVE, and `station_index` is `u32` on the
 * Rust side, so serde would reject the whole import with an opaque error
 * instead of a placement-level complaint.
 */
export function buildZeroDatumSectionConfig(config: ZeroDatumConfigResult): ImportSectionConfig {
  const { sectionIndex, stationIndex, gps, metadata, shots } = config;
  // Deleted rows go, and so do rows that were added and never filled in. The
  // completeness gates let a wholly blank row through on purpose - it is an
  // invitation, not an obligation - which is only safe if it is dropped HERE.
  // Otherwise it serializes as NaN length, depths and azimuths.
  const emitted = shots.filter(isEmittedShot);
  const backendShots: ShotData[] = emitted.map(editableShotToShotData);
  const reverse = metadata.reverse ?? false;

  const placement: ImportPlacement = {
    type: 'zero_datum',
    coordinates: gps,
    station_index: Math.max(0, Math.min(backendShots.length, stationIndex)),
  };

  return {
    section_index: sectionIndex,
    name: metadata.name,
    ...importCreditFields(
      { surveyors: metadata.surveyors ?? [], explorers: metadata.explorers ?? [] },
      metadata.date,
    ),
    color: metadata.color,
    direction: metadata.direction,
    quality: metadata.quality,
    shots: backendShots,
    placement: mirrorPlacementForReverse(placement, reverse, backendShots.length, null),
    station_comments: [],
    station_names: stationNamesForEmittedShots(emitted),
    reverse,
    azimuth_reference: DMP_AZIMUTH_REFERENCE,
  };
}
