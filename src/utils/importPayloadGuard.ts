/**
 * The last stop before a line payload becomes IPC.
 *
 * A cleared cell in the shot grid writes `NaN` - the grid will never put a 0
 * into a survey - and every UI gate that refuses to proceed while a row is
 * half-filled is a gate on ONE line's rows. The batch builders serialize
 * every SELECTED line, so a route that reaches them past the gate (edit
 * line B, switch the active line, place) sends the `NaN` on: it
 * serializes to `null`, and Rust's serde rejects the whole batch with an error
 * that names no line, no row and no field.
 *
 * So the payload itself is checked, once, on the way out. The guard is about
 * what CANNOT be sent rather than what the user still has to fill in - the two
 * differ (a wholly blank added row is dropped, not refused) and only this one
 * has to hold for every route.
 */
import { countIncompleteShots } from '../components/common/incompleteShots';
import type { ShotData } from '../types/project';
import type { LineEditState } from './batchImportHelpers';

/** Fields that must carry a real measurement (the nullable ones are allowed to
 *  be absent, but never `NaN`/`Infinity` when present). */
const REQUIRED_NUMBERS = [
  'length',
  'azimuth_in',
  'azimuth_out',
  'depth_in',
  'depth_out',
] as const;

const OPTIONAL_NUMBERS = [
  'pitch_in',
  'pitch_out',
  'left',
  'right',
  'up',
  'down',
] as const;

/** A shot that cannot be serialized, and where it is. `row` is 1-based - it is
 *  a number the user reads off the grid, not an array index. */
export interface NonFiniteShotLocation {
  row: number;
  field: string;
}

/**
 * Refused because a value would reach the backend as `null`. Typed so a caller
 * can tell it apart from an IPC failure; `message` is already user-facing.
 */
export class NonFiniteShotError extends Error {
  readonly lineIndex: number;
  readonly lineName: string;
  readonly location: NonFiniteShotLocation;

  constructor(lineIndex: number, lineName: string, location: NonFiniteShotLocation) {
    super(
      `Line "${lineName}" row ${location.row}: ${location.field} has no value. ` +
        `Fill it in or delete the row before importing.`,
    );
    this.name = 'NonFiniteShotError';
    this.lineIndex = lineIndex;
    this.lineName = lineName;
    this.location = location;
  }
}

/** The first unsendable value in these shots, or null when the payload is safe. */
export function findNonFiniteShot(shots: readonly ShotData[]): NonFiniteShotLocation | null {
  for (let i = 0; i < shots.length; i++) {
    const shot = shots[i];
    if (!shot) continue;
    for (const field of REQUIRED_NUMBERS) {
      if (!Number.isFinite(shot[field])) return { row: i + 1, field };
    }
    for (const field of OPTIONAL_NUMBERS) {
      const value = shot[field];
      if (value !== null && value !== undefined && !Number.isFinite(value)) {
        return { row: i + 1, field };
      }
    }
  }
  return null;
}

/** A line the user still has to finish, named so a toast can say which. */
export interface IncompleteLine {
  lineIndex: number;
  name: string;
  count: number;
}

/**
 * Every line that will be serialized and still holds a half-filled row, in
 * line order.
 *
 * A gate on the line currently on screen is not a gate on the payload: the
 * batch builders walk every selected line's edits, so clearing a cell in one
 * line and switching to another passed the check and died at Rust's
 * deserializer instead.
 */
export function findIncompleteLines(
  selectedIndices: Iterable<number>,
  lineEdits: ReadonlyMap<number, LineEditState>,
): IncompleteLine[] {
  const out: IncompleteLine[] = [];
  for (const lineIndex of selectedIndices) {
    const edits = lineEdits.get(lineIndex);
    if (!edits) continue;
    const count = countIncompleteShots(edits.shots);
    if (count > 0) out.push({ lineIndex, name: edits.name, count });
  }
  return out.sort((a, b) => a.lineIndex - b.lineIndex);
}

/** Throw {@link NonFiniteShotError} if any of these shots cannot be serialized. */
export function assertSendableShots(
  shots: readonly ShotData[],
  lineIndex: number,
  lineName: string,
): void {
  const found = findNonFiniteShot(shots);
  if (found) throw new NonFiniteShotError(lineIndex, lineName, found);
}
