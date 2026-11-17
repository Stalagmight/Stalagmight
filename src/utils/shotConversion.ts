import type { DmpSection, DmpShot } from '../types/commands';
import type { ShotData } from '../types/project';
import type { EditableShot } from '../components/common/ShotTableEditor';
import { isEmittedShot } from '../components/common/emittedShots';

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
    marker_index: null,
    temperature: null,
  };
}

/**
 * Re-render a DMP line from the shot table's current rows.
 *
 * The wizard previews are fed `DmpSection`s but the user edits `EditableShot`s,
 * so every preview that must reflect pending edits converts back. Doing that in
 * one place matters for more than duplication: `LinePreview` draws
 * `shots.filter(s => s.shot_type === 'Standard')` and numbers the stations it
 * draws from 0, so a preview built off the RAW line addresses a different
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

/**
 * Calculate station positions for a line based on shots.
 * Returns positions in local coordinates (station 0 at origin).
 */
export function calculateStationPositions(shots: EditableShot[]): Array<{ x: number; y: number }> {
  const positions: Array<{ x: number; y: number }> = [{ x: 0, y: 0 }];

  let x = 0;
  let y = 0;

  for (const shot of shots) {
    // Calculate heading from azimuth_in and azimuth_out average
    const heading = (shot.azimuth_in + shot.azimuth_out) / 2;
    // Convert heading to radians (heading is in degrees, 0 = north)
    const radians = ((90 - heading) * Math.PI) / 180;
    // Calculate dx, dy
    const dx = shot.length * Math.cos(radians);
    const dy = shot.length * Math.sin(radians);
    x += dx;
    y += dy;
    positions.push({ x, y });
  }

  return positions;
}
