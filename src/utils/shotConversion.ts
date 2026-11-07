import type { DmpSection, DmpShot } from '../types/commands';
import type { ShotData } from '../types/project';
import type { EditableShot } from '../components/common/ShotTableEditor';
import { isEmittedShot } from '../components/common/emittedShots';
import { normalizeAzimuth } from './azimuth';

const DEFAULT_EXCLUDED_DMP_SHOT_LENGTH = 1;

/**
 * DMP imports skip length-1 standard shots by default.
 */
export function isImportableDmpShot(shot: DmpShot): boolean {
  if (shot.shot_type !== 'Standard') {
    return false;
  }
  return Math.abs(shot.length - DEFAULT_EXCLUDED_DMP_SHOT_LENGTH) > 1e-9;
}

/**
 * Convert DMP shots to EditableShots.
 *
 * `version` is the section's DMP version (`DmpSection.version`). It decides
 * which fields are readings. A v2 record has no temperature bytes, and a v2 or
 * v3 record has no LRUD bytes (they arrive with v4); the parser fills `0` for
 * each, which must not reach the project as a measured 0 degrees or a measured
 * zero-width passage (#576 NF-AN4-3), so such a row carries none. The marker
 * index is in every version's record.
 *
 * Headings are wrapped into [0, 360) here (#576 RV2-FE-D7). A DMP heading is a
 * raw `i16 * 0.1`, so a damaged dump can carry 400 or -30; the core import gate
 * stores every azimuth wrapped (B24-D2), so the wizard shows - and validates -
 * the bearing that will be stored instead of flagging 400 as out of range while
 * the import silently keeps 40. A non-finite reading passes through untouched.
 */
export function convertToEditableShots(shots: DmpShot[], version: number): EditableShot[] {
  const recordsTemperature = version >= 3;
  const recordsLrud = version >= 4;
  return shots
    .filter(isImportableDmpShot)
    .map((shot, index) => ({
      id: index,
      azimuth_in: normalizeAzimuth(shot.heading_in),
      azimuth_out: normalizeAzimuth(shot.heading_out),
      length: shot.length,
      depth_in: shot.depth_in,
      depth_out: shot.depth_out,
      pitch_in: shot.pitch_in,
      pitch_out: shot.pitch_out,
      left: recordsLrud ? shot.left : null,
      right: recordsLrud ? shot.right : null,
      up: recordsLrud ? shot.up : null,
      down: recordsLrud ? shot.down : null,
      comment: shot.comment || null,
      kind: 'Normal' as const,
      temperature: recordsTemperature ? shot.temperature : null,
      marker_index: shot.marker_index,
    }));
}

/**
 * Convert an EditableShot to a ShotData for the backend.
 */
export function editableShotToShotData(shot: EditableShot): ShotData {
  return {
    azimuth_in: shot.azimuth_in,
    azimuth_out: shot.azimuth_out,
    length: shot.length,
    depth_in: shot.depth_in,
    depth_out: shot.depth_out,
    pitch_in: shot.pitch_in,
    pitch_out: shot.pitch_out,
    left: shot.left,
    right: shot.right,
    up: shot.up,
    down: shot.down,
    comment: shot.comment,
    kind: shot.kind ?? 'Normal',
    marker_index: shot.marker_index ?? null,
    temperature: shot.temperature ?? null,
  };
}

/**
 * Re-render a DMP section from the shot table's current rows.
 *
 * The wizard previews are fed `DmpSection`s but the user edits `EditableShot`s,
 * so every preview that must reflect pending edits converts back. Doing that in
 * one place matters for more than duplication: `SectionPreview` draws
 * `shots.filter(s => s.shot_type === 'Standard')` and numbers the stations it
 * draws from 0, so a preview built off the RAW section addresses a different
 * station space than the shot table does. `convertToEditableShots` drops
 * non-Standard shots AND length-1 Standard ones (`isImportableDmpShot`), and the
 * table then drops whatever the user deleted. Emitting every surviving row as
 * `Standard` makes that filter a no-op, so a click on the preview and a pick in
 * the station dropdown mean the same station.
 *
 * Callers that also track a name/direction edit should spread those over the
 * result; this only rewrites the geometry.
 *
 * The rows are the EMITTED ones (`isEmittedShot`), i.e. the same set the import
 * payloads serialize. Keeping a wholly blank row here put a NaN length and NaN
 * azimuths into the preview: `computeStationsFromDmpShots` carried that NaN into
 * every station after it, so the centerline vanished and `fitBounds` was handed
 * NaNs - and the preview drew one station more than the import would create, so
 * a click on it committed a station index one past the one meant.
 */
export function editableShotsToDmpSection(raw: DmpSection, shots: EditableShot[]): DmpSection {
  const syntheticShots: DmpShot[] = shots
    .filter(isEmittedShot)
    .map((s) => ({
      shot_type: 'Standard' as const,
      heading_in: s.azimuth_in,
      heading_out: s.azimuth_out,
      length: s.length,
      depth_in: s.depth_in,
      depth_out: s.depth_out,
      pitch_in: s.pitch_in ?? 0,
      pitch_out: s.pitch_out ?? 0,
      left: s.left ?? 0,
      right: s.right ?? 0,
      up: s.up ?? 0,
      down: s.down ?? 0,
      temperature: 0, hour: 0, minute: 0, second: 0, marker_index: 0,
      ...(s.comment != null ? { comment: s.comment } : {}),
    }));

  return { ...raw, shots: syntheticShots };
}
