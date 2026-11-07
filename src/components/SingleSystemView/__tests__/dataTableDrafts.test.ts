import { describe, it, expect } from 'vitest';
import {
  type ShotDraft,
  END_DRAFT_KEY,
  endDraft,
  interleaveRows,
  pruneDrafts,
  insertDraftsAt,
  materializeDraft,
  draftMissing,
  completeDraftData,
  lateDraftEdits,
  commitAnchorArgs,
  overflowDraft,
  respawnsBlank,
  isBlankDraft,
  isFiniteNumber,
} from '../dataTableDrafts';
import { AZIMUTH_AVG_KEY } from '../dataTableEdits';
import type { Shot } from '../../../types/project';
import { makeShot } from '../../../test/factories/survey';

const shot = (id: number, over: Partial<Shot> = {}): Shot =>
  makeShot({ id, azimuth_in: 90, azimuth_out: 90, depth_out: 1, ...over });

const draft = (key: number, anchor: ShotDraft['anchor'], data: ShotDraft['data'] = {}): ShotDraft => ({
  key,
  anchor,
  data,
});

/** Compact signature of an entries list: shot ids and (negative) draft keys. */
const ids = (entries: ReturnType<typeof interleaveRows>) =>
  entries.map((e) => (e.kind === 'shot' ? e.shot.id : e.draft.key));

describe('interleaveRows', () => {
  const shots = [shot(1), shot(2), shot(3)];

  it('places after-anchored drafts below their shot, before-anchored above, end last', () => {
    const entries = interleaveRows(shots, [
      endDraft(),
      draft(-2, { afterShotId: 1 }),
      draft(-3, { beforeShotId: 3 }),
    ]);
    expect(ids(entries)).toEqual([1, -2, 2, -3, 3, END_DRAFT_KEY]);
  });

  it('keeps same-anchor drafts in array order', () => {
    const entries = interleaveRows(shots, [
      draft(-2, { afterShotId: 2 }),
      draft(-3, { afterShotId: 2 }),
    ]);
    expect(ids(entries)).toEqual([1, 2, -2, -3, 3]);
  });

  it('carries the shots-array index on shot entries', () => {
    const entries = interleaveRows(shots, [draft(-2, { beforeShotId: 1 })]);
    const shotEntries = entries.flatMap((e) => (e.kind === 'shot' ? [e] : []));
    expect(shotEntries.map((e) => e.shotIndex)).toEqual([0, 1, 2]);
  });

  it('prunes drafts whose anchor shot is gone', () => {
    const entries = interleaveRows(shots, [
      draft(-2, { afterShotId: 99 }),
      draft(-3, { beforeShotId: 99 }),
      endDraft(),
    ]);
    expect(ids(entries)).toEqual([1, 2, 3, END_DRAFT_KEY]);
  });

  it('shows only end drafts for an empty section', () => {
    expect(ids(interleaveRows([], [endDraft()]))).toEqual([END_DRAFT_KEY]);
  });
});

describe('pruneDrafts', () => {
  it('drops dead anchors but always keeps the end draft', () => {
    const drafts = [endDraft(), draft(-2, { afterShotId: 1 }), draft(-3, { beforeShotId: 9 })];
    const pruned = pruneDrafts(drafts, [shot(1)]);
    expect(pruned.map((d) => d.key)).toEqual([END_DRAFT_KEY, -2]);
  });

  it('returns the same array when nothing is pruned', () => {
    const drafts = [endDraft(), draft(-2, { afterShotId: 1 })];
    expect(pruneDrafts(drafts, [shot(1)])).toBe(drafts);
  });
});

describe('insertDraftsAt', () => {
  const shots = [shot(1), shot(2), shot(3)];
  const base = [endDraft()];
  // Keys are caller-allocated and never reused; tests mint them from `start`
  // down (use a lower start when the fixture already holds lower keys).
  const counter = (start = -2) => {
    let k = start;
    return () => k--;
  };

  it('anchors before the first shot at the top boundary', () => {
    const next = insertDraftsAt(shots, base, 0, 1, counter());
    expect(next).not.toBeNull();
    expect(ids(interleaveRows(shots, next!))).toEqual([-2, 1, 2, 3, END_DRAFT_KEY]);
    expect(next!.find((d) => d.key === -2)?.anchor).toEqual({ beforeShotId: 1 });
  });

  it('anchors after the shot above for an interior boundary', () => {
    const next = insertDraftsAt(shots, base, 2, 1, counter());
    expect(ids(interleaveRows(shots, next!))).toEqual([1, 2, -2, 3, END_DRAFT_KEY]);
    expect(next!.find((d) => d.key === -2)?.anchor).toEqual({ afterShotId: 2 });
  });

  it('creates `count` drafts with fresh unique keys', () => {
    const next = insertDraftsAt(shots, base, 1, 3, counter());
    expect(ids(interleaveRows(shots, next!))).toEqual([1, -2, -3, -4, 2, 3, END_DRAFT_KEY]);
  });

  it('inserting below the end draft lands after the last shot', () => {
    // entries = [1, 2, 3, end] → boundary 4 is past the end draft.
    const next = insertDraftsAt(shots, base, 4, 1, counter());
    expect(ids(interleaveRows(shots, next!))).toEqual([1, 2, 3, -2, END_DRAFT_KEY]);
    expect(next!.find((d) => d.key === -2)?.anchor).toEqual({ afterShotId: 3 });
  });

  it('inserting between two same-anchor drafts keeps the clicked position', () => {
    const drafts = [endDraft(), draft(-2, { afterShotId: 1 }), draft(-3, { afterShotId: 1 })];
    // entries = [1, -2, -3, 2, 3, end] → boundary 2 sits between -2 and -3.
    const next = insertDraftsAt(shots, drafts, 2, 1, counter(-4));
    expect(ids(interleaveRows(shots, next!))).toEqual([1, -2, -4, -3, 2, 3, END_DRAFT_KEY]);
  });

  it('is a no-op for an empty section', () => {
    expect(insertDraftsAt([], base, 0, 1, counter())).toBeNull();
    expect(insertDraftsAt([], base, 1, 2, counter())).toBeNull();
  });

  it('clamps an out-of-range boundary', () => {
    const next = insertDraftsAt(shots, base, 99, 1, counter());
    expect(ids(interleaveRows(shots, next!))).toEqual([1, 2, 3, -2, END_DRAFT_KEY]);
  });
});

describe('materializeDraft', () => {
  it('uses the draft key as the row id and NaN for unset required numbers', () => {
    const row = materializeDraft(draft(-4, { afterShotId: 1 }, { length: 5 }));
    expect(row.id).toBe(-4);
    expect(row.length).toBe(5);
    expect(Number.isNaN(row.depth_in)).toBe(true);
    expect(row.pitch_in).toBeNull();
    expect(row.kind).toBe('Normal');
  });
});

/**
 * THE blankness rule, shared by the end draft, the new-section dialog's rows and
 * the submit gate. Judged by value rather than by key presence: a cleared
 * optional WRITES its key (`comment: null`, and `pitch_in: null` out of
 * `buildShotUpdates`), so counting keys read an emptied row as half-filled.
 */
describe('isBlankDraft', () => {
  it('is true for a row with no fields at all', () => {
    expect(isBlankDraft(endDraft())).toBe(true);
  });

  it('is false once a cell holds a real value', () => {
    expect(isBlankDraft(draft(-2, 'end', { comment: 'x' }))).toBe(false);
    expect(isBlankDraft(draft(-2, 'end', { length: 5 }))).toBe(false);
  });

  it('stays blank when a cell is written back to its default', () => {
    expect(isBlankDraft(draft(-2, 'end', { pitch_in: null, pitch_out: null }))).toBe(true);
    expect(isBlankDraft(draft(-2, 'end', { comment: null }))).toBe(true);
    expect(isBlankDraft(draft(-2, 'end', { comment: '' }))).toBe(true);
    expect(isBlankDraft(draft(-2, 'end', { kind: 'Normal' }))).toBe(true);
    expect(isBlankDraft(draft(-2, 'end', { length: NaN }))).toBe(true);
  });

  it('is not blank when the kind was cycled AWAY from the default', () => {
    expect(isBlankDraft(draft(-2, 'end', { kind: 'Jump' }))).toBe(false);
  });

  it('is blank when every written cell is at its default, together', () => {
    expect(isBlankDraft(draft(-2, 'end', { kind: 'Normal', comment: '', pitch_in: null }))).toBe(
      true
    );
  });
});

describe('isFiniteNumber', () => {
  it('accepts only finite numbers', () => {
    expect(isFiniteNumber(0)).toBe(true);
    expect(isFiniteNumber(-1.5)).toBe(true);
    expect(isFiniteNumber(NaN)).toBe(false);
    expect(isFiniteNumber(Infinity)).toBe(false);
    expect(isFiniteNumber(null)).toBe(false);
    expect(isFiniteNumber('5')).toBe(false);
  });
});

describe('draftMissing', () => {
  const noHidden = new Set<string>();

  it('keeps an EMPTY end draft quiet (lazy)', () => {
    const m = draftMissing(endDraft(), noHidden, 'avg');
    expect(m.cells.size).toBe(0);
    expect(m.hiddenProblems.size).toBe(0);
  });

  /**
   * Clearing an optional cell on the trailing row writes `comment: null` /
   * `pitch_in: null` rather than deleting the key. Judging the row by key count
   * painted four red required cells on a visibly empty row, with no blocker text
   * to explain them and Create disabled.
   */
  it('stays quiet when the end draft holds only cleared optionals', () => {
    const cleared = draft(END_DRAFT_KEY, 'end', {
      comment: null,
      pitch_in: null,
      pitch_out: null,
      kind: 'Normal',
    });
    const m = draftMissing(cleared, noHidden, 'avg');
    expect(m.cells.size).toBe(0);
    expect(m.hiddenProblems.size).toBe(0);
  });

  it('surfaces the end draft once the kind is cycled away from Normal', () => {
    const m = draftMissing(draft(END_DRAFT_KEY, 'end', { kind: 'Jump' }), noHidden, 'avg');
    expect(m.cells).toEqual(new Set([AZIMUTH_AVG_KEY, 'length', 'depth_in', 'depth_out']));
  });

  it('surfaces an end draft once it has any value', () => {
    const m = draftMissing(draft(END_DRAFT_KEY, 'end', { length: 5 }), noHidden, 'avg');
    expect(m.cells).toEqual(new Set([AZIMUTH_AVG_KEY, 'depth_in', 'depth_out']));
  });

  it('marks an EMPTY inserted draft immediately', () => {
    const m = draftMissing(draft(-2, { afterShotId: 1 }), noHidden, 'avg');
    expect(m.cells).toEqual(new Set(['length', 'depth_in', 'depth_out', AZIMUTH_AVG_KEY]));
  });

  it('routes hidden required columns to the menu problems instead', () => {
    const m = draftMissing(draft(-2, { beforeShotId: 1 }), new Set(['length', 'depth_in']), 'avg');
    expect(m.cells).toEqual(new Set(['depth_out', AZIMUTH_AVG_KEY]));
    expect(m.hiddenProblems).toEqual(new Set(['length', 'depthIn']));
  });

  it('follows the azimuth column mode (in/out cells, or a menu problem when off)', () => {
    const inout = draftMissing(draft(-2, { afterShotId: 1 }), noHidden, 'inout');
    expect(inout.cells.has('azimuth_in')).toBe(true);
    expect(inout.cells.has('azimuth_out')).toBe(true);
    const off = draftMissing(draft(-2, { afterShotId: 1 }), noHidden, 'off');
    expect(off.hiddenProblems.has('azimuth')).toBe(true);
  });

  it('clears the azimuth mark when either leg is present', () => {
    const m = draftMissing(draft(-2, { afterShotId: 1 }, { azimuth_out: 10 }), noHidden, 'avg');
    expect(m.cells.has(AZIMUTH_AVG_KEY)).toBe(false);
  });
});

describe('lateDraftEdits (#576 D39(f))', () => {
  const required = { length: 5, depth_in: 0, depth_out: 1, azimuth_in: 90 };
  const sentFrom = (data: ShotDraft['data']) => completeDraftData(draft(-2, 'end', data))!;

  it('returns the fields typed after the create payload was taken', () => {
    const sent = sentFrom(required);
    expect(lateDraftEdits(sent, draft(-2, 'end', { ...required, left: 2.5, comment: 'sump' }))).toEqual({
      left: 2.5,
      comment: 'sump',
    });
  });

  it('is empty when nothing changed, including the azimuth leg that was sent as a duplicate', () => {
    const sent = sentFrom(required);
    expect(lateDraftEdits(sent, draft(-2, 'end', required))).toEqual({});
  });

  it('carries a back reading typed after the front one was duplicated into it', () => {
    const sent = sentFrom(required);
    expect(lateDraftEdits(sent, draft(-2, 'end', { ...required, azimuth_out: 271 }))).toEqual({ azimuth_out: 271 });
  });

  it('leaves out a required field cleared since - a shot cannot hold a blank one - but keeps the rest', () => {
    const sent = sentFrom(required);
    const { length: _cleared, ...withoutLength } = required;
    expect(lateDraftEdits(sent, draft(-2, 'end', { ...withoutLength, up: 3 }))).toEqual({ up: 3 });
  });
});

describe('completeDraftData', () => {
  const required = { length: 5, depth_in: 0, depth_out: 1, azimuth_in: 90, azimuth_out: 90 };

  it('returns null while required fields are missing', () => {
    expect(completeDraftData(endDraft())).toBeNull();
    expect(completeDraftData(draft(-2, 'end', { length: 5, depth_in: 0, depth_out: 1 }))).toBeNull();
  });

  it('rejects a non-positive length', () => {
    expect(completeDraftData(draft(-2, 'end', { ...required, length: 0 }))).toBeNull();
  });

  it('duplicates a missing azimuth leg from the other', () => {
    const data = completeDraftData(
      draft(-2, 'end', { length: 5, depth_in: 0, depth_out: 1, azimuth_out: 123 })
    );
    expect(data).not.toBeNull();
    expect(data!.azimuth_in).toBe(123);
    expect(data!.azimuth_out).toBe(123);
  });

  it('carries optional fields and defaults through', () => {
    const data = completeDraftData(draft(-2, 'end', { ...required, left: 2, comment: 'x' }));
    expect(data).toMatchObject({ ...required, left: 2, comment: 'x', pitch_in: null, kind: 'Normal' as const });
  });
});

describe('commitAnchorArgs', () => {
  const shots = [shot(1), shot(2)];

  it('resolves the end draft to "after the last shot"', () => {
    expect(commitAnchorArgs(endDraft(), shots)).toEqual({ afterShotId: 2 });
  });

  it('appends when the section has no shots yet', () => {
    expect(commitAnchorArgs(endDraft(), [])).toEqual({});
  });

  it('passes id anchors through', () => {
    expect(commitAnchorArgs(draft(-2, { afterShotId: 1 }), shots)).toEqual({ afterShotId: 1 });
    expect(commitAnchorArgs(draft(-3, { beforeShotId: 2 }), shots)).toEqual({ beforeShotId: 2 });
  });
});

/**
 * A multi-row paste that runs past the trailing blank creates one end row per
 * pasted row. Every end row respawning a fresh blank on commit is what left a
 * 3-row paste showing 3 empty rows: only the canonical one may respawn.
 */
describe('overflow drafts from a grown paste', () => {
  it('marks an overflow row as not the canonical trailing blank', () => {
    expect(respawnsBlank(endDraft(-1))).toBe(true);
    expect(respawnsBlank(overflowDraft(-2))).toBe(false);
  });

  it('still renders overflow rows at the end, in the order they were made', () => {
    // End-anchored is what keeps pasted row 2 below pasted row 1; the flag only
    // changes what happens when the row commits, never where it sits.
    const shots = [{ id: 1 } as Shot];
    const rows = interleaveRows(shots, [
      endDraft(-1),
      overflowDraft(-2),
      overflowDraft(-3),
    ]);
    expect(rows.map((r) => (r.kind === 'draft' ? r.draft.key : r.shot.id))).toEqual([
      1, -1, -2, -3,
    ]);
  });
});
