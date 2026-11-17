import { describe, it, expect } from 'vitest';
import type { ShotData } from '../../../types/project';
import { type ShotDraft, isBlankDraft } from '../../SingleSystemView/dataTableDrafts';
import { AZIMUTH_AVG_KEY, PITCH_AVG_KEY } from '../../SingleSystemView/dataTableEdits';
import {
  applyEditToData,
  applyEdits,
  collectShots,
  blankRow,
  canSubmit,
  deleteRows,
  deriveAzimuthMode,
  insertBlankRows,
  MAX_PASTE_ROWS,
  moveRows,
  normalizeRows,
  summarizeSubmit,
  normalizeAngleFamilies,
  reorderForAngleMode,
} from '../newLineRows';

/** A row carrying `data`, keyed so assertions can follow identity across ops. */
const row = (key: number, data: Partial<ShotData> = {}): ShotDraft => ({
  key,
  anchor: 'end',
  data,
});

/** A complete row: length > 0, both depths, at least one azimuth. */
const filled = (key: number, over: Partial<ShotData> = {}): ShotDraft =>
  row(key, { length: 10, azimuth_in: 90, azimuth_out: 90, depth_in: 0, depth_out: 1, ...over });

/** Key allocator matching the dialog's: monotonic, never reused. */
const allocator = (start = 100) => {
  let n = start;
  return () => n++;
};

// The blankness rule itself is `isBlankDraft`, and lives (with its own tests)
// beside the drafts it judges in dataTableDrafts. Here it is only the yardstick
// for what these mutators leave behind.
describe('normalizeRows', () => {
  it('appends a blank row when the last one holds data', () => {
    const next = normalizeRows([filled(1)], allocator());
    expect(next).toHaveLength(2);
    expect(isBlankDraft(next[1]!)).toBe(true);
    expect(next[1]!.key).toBe(100);
  });

  it('returns the same array when the last row is already blank', () => {
    const rows = [filled(1), blankRow(2)];
    expect(normalizeRows(rows, allocator())).toBe(rows);
  });

  it('appends to an empty list', () => {
    expect(normalizeRows([], allocator())).toHaveLength(1);
  });
});

describe('insertBlankRows', () => {
  it('inserts at the boundary and keeps a trailing blank', () => {
    const next = insertBlankRows([filled(1), filled(2), blankRow(3)], 1, 2, allocator());
    expect(next.map((r) => r.key)).toEqual([1, 100, 101, 2, 3]);
  });

  it('inserts at the top', () => {
    const next = insertBlankRows([filled(1), blankRow(2)], 0, 1, allocator());
    expect(next.map((r) => r.key)).toEqual([100, 1, 2]);
  });

  it('clamps a boundary past the end and re-establishes the trailing blank', () => {
    const next = insertBlankRows([filled(1)], 99, 1, allocator());
    expect(next.map((r) => r.key)).toEqual([1, 100]);
  });

  it('is a no-op for a non-positive count', () => {
    const rows = [filled(1), blankRow(2)];
    expect(insertBlankRows(rows, 1, 0, allocator())).toBe(rows);
  });
});

describe('deleteRows', () => {
  it('removes an inclusive range', () => {
    const next = deleteRows([filled(1), filled(2), filled(3), blankRow(4)], 1, 2, allocator());
    expect(next.map((r) => r.key)).toEqual([1, 4]);
  });

  it('accepts a reversed range (drag upwards)', () => {
    const next = deleteRows([filled(1), filled(2), blankRow(3)], 1, 0, allocator());
    expect(next.map((r) => r.key)).toEqual([3]);
  });

  it('leaves one fresh blank row rather than an empty grid', () => {
    const next = deleteRows([filled(1), filled(2)], 0, 1, allocator());
    expect(next).toHaveLength(1);
    expect(isBlankDraft(next[0]!)).toBe(true);
    expect(next[0]!.key).toBe(100);
  });

  it('re-adds the trailing blank when it was the one deleted', () => {
    const next = deleteRows([filled(1), blankRow(2)], 1, 1, allocator());
    expect(next.map((r) => r.key)).toEqual([1, 100]);
  });
});

describe('moveRows', () => {
  // toIndex is an insertion boundary in the ORIGINAL indexing, so the boundary
  // has to be shifted back by however much of the block sat in front of it.
  it('moves a single row down', () => {
    const rows = [row(1), row(2), row(3), row(4)];
    expect(moveRows(rows, 1, 1, 3, allocator()).map((r) => r.key)).toEqual([1, 3, 2, 4]);
  });

  it('moves a single row to the top', () => {
    const rows = [row(1), row(2), row(3)];
    expect(moveRows(rows, 2, 2, 0, allocator()).map((r) => r.key)).toEqual([3, 1, 2]);
  });

  it('moves a block to the end', () => {
    const rows = [row(1), row(2), row(3), row(4)];
    expect(moveRows(rows, 0, 1, 4, allocator()).map((r) => r.key)).toEqual([3, 4, 1, 2]);
  });

  it('moves a block upwards', () => {
    const rows = [row(1), row(2), row(3), row(4)];
    expect(moveRows(rows, 2, 3, 1, allocator()).map((r) => r.key)).toEqual([1, 3, 4, 2]);
  });

  it('is a no-op when the block lands where it started', () => {
    const rows = [row(1), row(2), row(3)];
    expect(moveRows(rows, 1, 1, 1, allocator()).map((r) => r.key)).toEqual([1, 2, 3]);
  });

  it('re-establishes the trailing blank when the drag moves it away from the end', () => {
    // Dragging the bottom blank row above a filled one left the last row
    // occupied, so there was nowhere to type the next shot.
    const next = moveRows([filled(1), blankRow(2)], 1, 1, 0, allocator());
    expect(next.map((r) => r.key)).toEqual([2, 1, 100]);
    expect(isBlankDraft(next[2]!)).toBe(true);
  });
});

describe('applyEditToData', () => {
  it('assigns a plain column', () => {
    expect(applyEditToData({}, 'length', 12)).toEqual({ length: 12 });
  });

  it('deletes the field when the value is undefined (cleared to missing)', () => {
    expect(applyEditToData({ length: 12, depth_in: 3 }, 'length', undefined)).toEqual({
      depth_in: 3,
    });
  });

  it('keeps an explicit null (a cleared optional) rather than dropping it', () => {
    expect(applyEditToData({ left: 2 }, 'left', null)).toEqual({ left: null });
  });

  it('writes the typed mean to BOTH azimuth legs while they are unset', () => {
    // buildShotUpdates would rotate by a delta against NaN here; on an unset row
    // the mean simply is both legs.
    expect(applyEditToData({}, AZIMUTH_AVG_KEY, 45)).toEqual({
      azimuth_in: 45,
      azimuth_out: 45,
    });
  });

  it('normalizes an out-of-range mean typed onto an unset row', () => {
    // The unset-legs case is `buildShotUpdates`' own branch; a second copy here
    // wrote the raw typed value straight to both legs, skipping normalizeAngle.
    expect(applyEditToData({}, AZIMUTH_AVG_KEY, 400)).toEqual({
      azimuth_in: 40,
      azimuth_out: 40,
    });
  });

  it('offsets from the single measured leg rather than averaging against NaN', () => {
    expect(applyEditToData({ azimuth_in: 10 }, AZIMUTH_AVG_KEY, 25)).toEqual({
      azimuth_in: 25,
      azimuth_out: 25,
    });
  });

  it('rotates both azimuth legs by the delta once both are set', () => {
    const out = applyEditToData({ azimuth_in: 10, azimuth_out: 20 }, AZIMUTH_AVG_KEY, 25);
    // mean was 15, so the delta is +10 and the 10-degree split is preserved.
    expect(out).toEqual({ azimuth_in: 20, azimuth_out: 30 });
  });

  it('normalizes a rotation that wraps past 360', () => {
    // Circular mean of 350/358 is 354, so a target of 2 rotates by +8 forward
    // through north; the 8-degree split survives the wrap.
    const out = applyEditToData({ azimuth_in: 350, azimuth_out: 358 }, AZIMUTH_AVG_KEY, 2);
    expect(out.azimuth_in).toBeCloseTo(358, 6);
    expect(out.azimuth_out).toBeCloseTo(6, 6);
  });

  it('clears both azimuth legs when the average cell is cleared', () => {
    expect(applyEditToData({ azimuth_in: 10, azimuth_out: 20, length: 5 }, AZIMUTH_AVG_KEY, undefined)).toEqual({
      length: 5,
    });
  });

  it('sets both pitch legs from the average while they are unset', () => {
    expect(applyEditToData({}, PITCH_AVG_KEY, -5)).toEqual({ pitch_in: -5, pitch_out: -5 });
  });

  it('shifts both pitch legs by the delta once both are set', () => {
    expect(applyEditToData({ pitch_in: -10, pitch_out: 0 }, PITCH_AVG_KEY, 0)).toEqual({
      pitch_in: -5,
      pitch_out: 5,
    });
  });

  it('clears both pitch legs when the average cell is nulled', () => {
    expect(applyEditToData({ pitch_in: -10, pitch_out: 0 }, PITCH_AVG_KEY, null)).toEqual({
      pitch_in: null,
      pitch_out: null,
    });
  });

  it('does not mutate the input', () => {
    const data: Partial<ShotData> = { length: 3 };
    applyEditToData(data, 'length', 9);
    expect(data).toEqual({ length: 3 });
  });
});

describe('applyEdits', () => {
  it('applies several cells on one row and appends a trailing blank', () => {
    const { rows: next } = applyEdits(
      [blankRow(1)],
      [
        { row: 0, colKey: 'length', value: 10 },
        { row: 0, colKey: 'depth_in', value: 0 },
      ],
      allocator()
    );
    expect(next[0]!.data).toEqual({ length: 10, depth_in: 0 });
    expect(next).toHaveLength(2);
    expect(isBlankDraft(next[1]!)).toBe(true);
  });

  it('grows the row list for edits landing past the end (a pasted block)', () => {
    // The grid is given `pasteGrowsRows`, so a block pasted into the single
    // blank row the dialog opens with must land in full, not clip to one row.
    const { rows: next } = applyEdits(
      [blankRow(1)],
      [
        { row: 0, colKey: 'length', value: 10 },
        { row: 1, colKey: 'length', value: 20 },
        { row: 2, colKey: 'length', value: 30 },
      ],
      allocator()
    );
    expect(next.map((r) => r.data.length)).toEqual([10, 20, 30, undefined]);
    expect(isBlankDraft(next[3]!)).toBe(true);
  });

  it('refuses to conjure unbounded rows from a stray clipboard, and says so', () => {
    const { rows: next, droppedPastCap } = applyEdits(
      [blankRow(1)],
      [{ row: MAX_PASTE_ROWS + 10, colKey: 'length', value: 10 }],
      allocator()
    );
    expect(next).toHaveLength(1);
    expect(droppedPastCap).toBe(1);
  });

  it('still edits rows that already exist past the cap', () => {
    // The cap bounds GROWTH, not editing: bounding by raw row index meant that
    // once the list legitimately passed it, typing into those rows was dropped.
    const big = Array.from({ length: MAX_PASTE_ROWS + 5 }, (_, i) => row(i, { length: 1 }));
    const { rows: next, droppedPastCap } = applyEdits(
      big,
      [{ row: MAX_PASTE_ROWS + 2, colKey: 'length', value: 42 }],
      allocator()
    );
    expect(next[MAX_PASTE_ROWS + 2]!.data.length).toBe(42);
    expect(droppedPastCap).toBe(0);
  });

  it('returns the same array when there is nothing to apply', () => {
    const rows = [blankRow(1)];
    expect(applyEdits(rows, [], allocator()).rows).toBe(rows);
  });

  it('seeds Depth In from the row above when a blank row is first typed into', () => {
    // What the old dialog's "+ Add Row" did: depth in IS the previous shot's
    // depth out on a continuous survey.
    const { rows: next } = applyEdits(
      [filled(1, { depth_out: -7 }), blankRow(2)],
      [{ row: 1, colKey: 'length', value: 12 }],
      allocator()
    );
    expect(next[1]!.data).toEqual({ length: 12, depth_in: -7 });
  });

  it('does not override a Depth In the edit itself supplies', () => {
    const { rows: next } = applyEdits(
      [filled(1, { depth_out: -7 }), blankRow(2)],
      [
        { row: 1, colKey: 'length', value: 12 },
        { row: 1, colKey: 'depth_in', value: -3 },
      ],
      allocator()
    );
    expect(next[1]!.data.depth_in).toBe(-3);
  });

  it('does not seed when the row above is being written in the same batch', () => {
    // A pasted block carries its own depths; seeding from a row that is itself
    // mid-write would race the paste.
    const { rows: next } = applyEdits(
      [blankRow(1)],
      [
        { row: 0, colKey: 'depth_out', value: -1 },
        { row: 1, colKey: 'length', value: 12 },
      ],
      allocator()
    );
    expect(next[1]!.data).toEqual({ length: 12 });
  });

  it('does not seed a row that already held data', () => {
    const { rows: next } = applyEdits(
      [filled(1, { depth_out: -7 }), row(2, { length: 3 })],
      [{ row: 1, colKey: 'azimuth_in', value: 10 }],
      allocator()
    );
    expect(next[1]!.data.depth_in).toBeUndefined();
  });

  it('does not seed Depth In when the edit only clears cells', () => {
    // Pressing Delete over the trailing blank row: every edit is a clear, so the
    // row has to come back empty rather than materializing a depth from above
    // that then counts as data and blocks Create.
    const { rows: next } = applyEdits(
      [filled(1, { depth_out: -7 }), blankRow(2)],
      [
        { row: 1, colKey: 'length', value: undefined },
        { row: 1, colKey: 'comment', value: null },
      ],
      allocator()
    );
    expect(next[1]!.data.depth_in).toBeUndefined();
    expect(isBlankDraft(next[1]!)).toBe(true);
  });

  it('leaves the trailing row empty after a cell-by-cell whole-row Delete', () => {
    // The grid reports one cleared cell per call, so the rule ran several times
    // over the same row and the order of the clears decided whether a seeded
    // depth survived.
    const nextKey = allocator();
    let rows = [filled(1, { depth_out: -7 }), blankRow(2)];
    for (const colKey of ['length', 'depth_in', 'depth_out', 'comment']) {
      rows = applyEdits(rows, [{ row: 1, colKey, value: undefined }], nextKey).rows;
    }
    expect(rows).toHaveLength(2);
    expect(isBlankDraft(rows[1]!)).toBe(true);
  });

  it('re-establishes the invitation row when the trailing row is tagged J/R', () => {
    // The J/R cell writes through this funnel precisely for this: setting `kind`
    // straight into the array skipped normalizeRows and consumed the blank row.
    const { rows: next } = applyEdits(
      [blankRow(1)],
      [{ row: 0, colKey: 'kind', value: 'Jump' }],
      allocator()
    );
    expect(next.map((r) => r.key)).toEqual([1, 100]);
    expect(isBlankDraft(next[1]!)).toBe(true);
  });

  it('does not seed Depth In for a tag alone, but does on the first measurement', () => {
    // A kind is a label, not a reading: seeding on it would strand a depth on a
    // row whose kind can be cycled straight back to Normal.
    const tagged = applyEdits(
      [filled(1, { depth_out: -7 }), blankRow(2)],
      [{ row: 1, colKey: 'kind', value: 'Jump' }],
      allocator()
    ).rows;
    expect(tagged[1]!.data).toEqual({ kind: 'Jump' });
    const measured = applyEdits(
      tagged,
      [{ row: 1, colKey: 'length', value: 12 }],
      allocator(200)
    ).rows;
    expect(measured[1]!.data).toEqual({ kind: 'Jump', length: 12, depth_in: -7 });
  });

  it('returns a row cycled back to Normal to blank, so Create is not stuck on it', () => {
    const tagged = applyEdits(
      [filled(1, { depth_out: -7 }), blankRow(2)],
      [{ row: 1, colKey: 'kind', value: 'Jump' }],
      allocator()
    ).rows;
    const back = applyEdits(tagged, [{ row: 1, colKey: 'kind', value: 'Normal' }], allocator(200))
      .rows;
    expect(isBlankDraft(back[1]!)).toBe(true);
    expect(summarizeSubmit(back).incompleteRows).toEqual([]);
  });

  it('leaves untouched rows referentially identical', () => {
    const rows = [filled(1), blankRow(2)];
    const { rows: next } = applyEdits(rows, [{ row: 1, colKey: 'length', value: 4 }], allocator());
    expect(next[0]).toBe(rows[0]);
  });
});

describe('deriveAzimuthMode', () => {
  it('is avg while the average column is visible', () => {
    expect(deriveAzimuthMode(new Set(['azimuth_in', 'azimuth_out']))).toBe('avg');
  });

  it('is inout when the average is hidden but a leg is visible', () => {
    expect(deriveAzimuthMode(new Set([AZIMUTH_AVG_KEY, 'azimuth_out']))).toBe('inout');
  });

  it('is off when every azimuth column is hidden', () => {
    expect(deriveAzimuthMode(new Set([AZIMUTH_AVG_KEY, 'azimuth_in', 'azimuth_out']))).toBe('off');
  });
});

describe('summarizeSubmit', () => {
  it('drops blank rows and reports nothing missing', () => {
    const { completeCount, incompleteRows } = summarizeSubmit([filled(1), blankRow(2)]);
    expect(completeCount).toBe(1);
    expect(incompleteRows).toEqual([]);
  });

  it('reports a partially-filled row by its 1-based display number', () => {
    const { completeCount, incompleteRows } = summarizeSubmit([
      filled(1),
      row(2, { length: 5 }), // no depths, no azimuth
      blankRow(3),
    ]);
    expect(completeCount).toBe(1);
    expect(incompleteRows).toEqual([2]);
  });

  it('treats a non-positive length as incomplete rather than silently dropping it', () => {
    const { completeCount, incompleteRows } = summarizeSubmit([filled(1, { length: 0 }), blankRow(2)]);
    expect(completeCount).toBe(0);
    expect(incompleteRows).toEqual([1]);
  });

  it('duplicates the present azimuth leg onto the missing one', () => {
    const shots = collectShots([row(1, { length: 10, azimuth_in: 90, depth_in: 0, depth_out: 1 })]);
    expect(shots[0]).toMatchObject({ azimuth_in: 90, azimuth_out: 90 });
  });

  it('carries the optional fields through', () => {
    const shots = collectShots([filled(1, { left: 2, comment: 'sump', kind: 'Reference' })]);
    expect(shots[0]).toMatchObject({ left: 2, comment: 'sump', kind: 'Reference' });
  });
});

describe('canSubmit', () => {
  it('is false while any row is incomplete', () => {
    expect(canSubmit({ completeCount: 1, incompleteRows: [2] })).toBe(false);
  });

  it('is false with nothing to write', () => {
    expect(canSubmit({ completeCount: 0, incompleteRows: [] })).toBe(false);
  });

  it('is true with at least one complete row and no incomplete ones', () => {
    expect(canSubmit({ completeCount: 1, incompleteRows: [] })).toBe(true);
  });
});

describe('normalizeAngleFamilies', () => {
  it('drops the legs when an average and its legs are both visible', () => {
    // What a set stored before the legs existed looks like: nothing hidden.
    const out = normalizeAngleFamilies(new Set());
    expect(out.has('azimuth_in')).toBe(true);
    expect(out.has('azimuth_out')).toBe(true);
    expect(out.has(AZIMUTH_AVG_KEY)).toBe(false);
    expect(out.has('pitch_in')).toBe(true);
    expect(out.has(PITCH_AVG_KEY)).toBe(false);
  });

  it('leaves a consistent in-out set alone', () => {
    const inout = new Set([AZIMUTH_AVG_KEY, PITCH_AVG_KEY, 'pitch_in', 'pitch_out']);
    expect([...normalizeAngleFamilies(inout)].sort()).toEqual([...inout].sort());
  });

  it('leaves an off family off', () => {
    const off = new Set([AZIMUTH_AVG_KEY, 'azimuth_in', 'azimuth_out']);
    const out = normalizeAngleFamilies(off);
    for (const k of off) expect(out.has(k)).toBe(true);
  });

  it('is idempotent', () => {
    const once = normalizeAngleFamilies(new Set());
    expect([...normalizeAngleFamilies(once)].sort()).toEqual([...once].sort());
  });

  it('does not touch unrelated columns', () => {
    expect(normalizeAngleFamilies(new Set(['comment'])).has('comment')).toBe(true);
  });
});

describe('reorderForAngleMode', () => {
  const ALL = ['length', AZIMUTH_AVG_KEY, 'azimuth_in', 'azimuth_out', 'depth_in'];

  it('puts the legs where the average was', () => {
    const out = reorderForAngleMode(
      ['length', AZIMUTH_AVG_KEY, 'depth_in', 'azimuth_in', 'azimuth_out'],
      ALL,
      [AZIMUTH_AVG_KEY],
      ['azimuth_in', 'azimuth_out']
    );
    expect(out).toEqual(['length', 'azimuth_in', 'azimuth_out', AZIMUTH_AVG_KEY, 'depth_in']);
  });

  it('puts the average where the legs were', () => {
    const out = reorderForAngleMode(
      ['length', 'azimuth_in', 'azimuth_out', 'depth_in', AZIMUTH_AVG_KEY],
      ALL,
      ['azimuth_in', 'azimuth_out'],
      [AZIMUTH_AVG_KEY]
    );
    expect(out.indexOf(AZIMUTH_AVG_KEY)).toBe(1);
  });

  it('falls back to declaration order when nothing was customized', () => {
    const out = reorderForAngleMode([], ALL, [AZIMUTH_AVG_KEY], ['azimuth_in', 'azimuth_out']);
    expect(out[1]).toBe('azimuth_in');
    expect(out[2]).toBe('azimuth_out');
  });

  it('appends columns the stored order never knew about', () => {
    const out = reorderForAngleMode(['length'], ALL, [AZIMUTH_AVG_KEY], ['azimuth_in']);
    expect(out).toContain('depth_in');
    expect(new Set(out).size).toBe(out.length);
  });

  it('appends at the end when the outgoing slot cannot be found', () => {
    const out = reorderForAngleMode(['length'], ['length', 'azimuth_in'], ['_gone'], ['azimuth_in']);
    expect(out[out.length - 1]).toBe('azimuth_in');
  });
});
