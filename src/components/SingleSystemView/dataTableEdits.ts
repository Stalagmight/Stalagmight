/**
 * Pure edit-dispatch helpers for the data-table grid integration.
 *
 * The grid emits edits as `{ row, colKey, value }` with values already coerced by
 * each column's parser. These helpers turn those into shot `ShotUpdates` patches
 * and `update_shot` batch commands, expanding the virtual "average azimuth"
 * column into an in/out pair via the same delta-offset rule the old inline editor
 * used (so editing the displayed mean rotates both legs by the same amount).
 *
 * Kept free of React/store imports so it is unit-testable in the node vitest env.
 */
import type { CellEdit } from '../common/SpreadsheetGrid/types';
import type { Shot, ShotUpdates, SectionId } from '../../types/project';
import type { BatchCommand } from '../../types/commands';
import { circularMeanDegrees, normalizeAngle } from '../../utils/angles';

/** Virtual column key for the displayed average azimuth (not a real shot field). */
export const AZIMUTH_AVG_KEY = '_azimuth_avg';
/** Virtual column key for the displayed average inclination (pitch). */
export const PITCH_AVG_KEY = '_pitch_avg';

/** Build a ShotUpdates patch for one shot from its edits. The azimuth-average
 *  column rotates both legs by (newMean - currentMean); the pitch-average column
 *  shifts both legs the same way when both are present, otherwise it sets the
 *  present leg (or both, when neither is) to the typed value - and a `null`
 *  (cleared Avg cell) clears both legs. A non-finite new mean is skipped. All
 *  other column keys are real shot fields, assigned directly. */
export function buildShotUpdates(
  // Only the four angle legs are read, so this takes them rather than a whole
  // `Shot`: that lets the shared shot editor's `EditableShot` rows through the
  // same function instead of keeping a second copy of the averaging rule.
  shot: Pick<Shot, 'azimuth_in' | 'azimuth_out' | 'pitch_in' | 'pitch_out'>,
  edits: { colKey: string; value: unknown }[],
): ShotUpdates {
  const updates: ShotUpdates = {};
  for (const { colKey, value } of edits) {
    if (colKey === AZIMUTH_AVG_KEY) {
      const newMean = typeof value === 'number' ? value : Number(value);
      if (!Number.isFinite(newMean)) continue;
      // Both legs unset (a row added but not yet measured): there is no mean to
      // rotate away from, so the typed value IS both legs. Without this the
      // delta is NaN, and while `normalizeAngle` still folded a non-finite angle
      // to 0 the shot silently recorded due north - a bearing nobody measured,
      // which then passed the range validation and saved. The pitch branch
      // below has always had this case; azimuth did not, and only got here when
      // the shared shot editor stopped using its own averaging rule.
      const inFinite = Number.isFinite(shot.azimuth_in);
      const outFinite = Number.isFinite(shot.azimuth_out);
      if (!inFinite && !outFinite) {
        updates.azimuth_in = normalizeAngle(newMean);
        updates.azimuth_out = normalizeAngle(newMean);
        continue;
      }
      if (!inFinite || !outFinite) {
        // ONE leg measured, which is a valid shot. The mean of a number and a
        // NaN is NaN (which normalizeAngle used to fold to 0) - so averaging here
        // used to replace a real bearing with due north. The single reading IS
        // the current mean, so offset from it and give the missing leg the same
        // value; nothing is invented that the user did not type.
        const present = inFinite ? shot.azimuth_in : shot.azimuth_out;
        const delta = newMean - present;
        updates.azimuth_in = normalizeAngle((inFinite ? shot.azimuth_in : present) + delta);
        updates.azimuth_out = normalizeAngle((outFinite ? shot.azimuth_out : present) + delta);
        continue;
      }
      const delta = newMean - circularMeanDegrees(shot.azimuth_in, shot.azimuth_out);
      updates.azimuth_in = normalizeAngle(shot.azimuth_in + delta);
      updates.azimuth_out = normalizeAngle(shot.azimuth_out + delta);
    } else if (colKey === PITCH_AVG_KEY) {
      // A cleared Avg cell (number-nullable → null) clears BOTH legs; without
      // this, `Number(null)` is a finite 0 and the clear would zero the pitch.
      if (value === null) {
        updates.pitch_in = null;
        updates.pitch_out = null;
        continue;
      }
      const newMean = typeof value === 'number' ? value : Number(value);
      if (!Number.isFinite(newMean)) continue;
      const pitchIn = shot.pitch_in;
      const pitchOut = shot.pitch_out;
      if (typeof pitchIn === 'number' && typeof pitchOut === 'number') {
        // Pitch doesn't wrap, so no normalization; the column's validate
        // rejects means that would push a leg outside ±90.
        const delta = newMean - (pitchIn + pitchOut) / 2;
        updates.pitch_in = pitchIn + delta;
        updates.pitch_out = pitchOut + delta;
      } else {
        if (typeof pitchIn === 'number' || typeof pitchOut !== 'number') updates.pitch_in = newMean;
        if (typeof pitchOut === 'number' || typeof pitchIn !== 'number') updates.pitch_out = newMean;
      }
    } else {
      (updates as Record<string, unknown>)[colKey] = value;
    }
  }
  return updates;
}

/** Group a flat list of grid edits into one `update_shot` command per shot, so a
 *  multi-cell paste/fill commits as a single undo unit via execute_batch. Edits
 *  on rows outside `shots` are dropped; shots with no net change are skipped. */
export function buildShotUpdateBatch(
  edits: CellEdit[],
  shots: Shot[],
  sectionId: SectionId,
): BatchCommand[] {
  const byRow = new Map<number, CellEdit[]>();
  for (const e of edits) {
    const arr = byRow.get(e.row);
    if (arr) arr.push(e);
    else byRow.set(e.row, [e]);
  }
  const commands: BatchCommand[] = [];
  for (const [row, rowEdits] of byRow) {
    const shot = shots[row];
    if (!shot) continue;
    const updates = buildShotUpdates(shot, rowEdits);
    if (Object.keys(updates).length === 0) continue;
    commands.push({ type: 'update_shot', section_id: sectionId, shot_id: shot.id, updates });
  }
  return commands;
}
