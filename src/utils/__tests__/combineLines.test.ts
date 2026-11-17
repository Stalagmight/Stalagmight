import { describe, it, expect } from 'vitest';
import {
  combineMoveIsSmall,
  combineMoveShown,
  combinePreview,
  combinePreviewAll,
  defaultCombinedName,
  formatRotation,
} from '../combineLines';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { CombinePlan } from '../../types/ipc/CombinePlan';

describe('defaultCombinedName', () => {
  it('gives the two halves of a split their original name back, whichever is kept', () => {
    expect(defaultCombinedName('Main-A', 'Main-B')).toBe('Main');
    expect(defaultCombinedName('Main-B', 'Main-A')).toBe('Main');
  });

  it('keeps the kept line name otherwise', () => {
    expect(defaultCombinedName('Main', 'Side')).toBe('Main');
    expect(defaultCombinedName('Main-A', 'Other-B')).toBe('Main-A');
    expect(defaultCombinedName('Main-A', 'Main-A')).toBe('Main-A');
    expect(defaultCombinedName('-A', '-B')).toBe('-A');
  });
});

describe('combinePreview', () => {
  it('highlights both lines and rings both join stations', () => {
    const candidate = {
      line_a: 1,
      line_b: 4,
      station_a: { line_id: 1, station_index: 3 },
      station_b: { line_id: 4, station_index: 0 },
    } as CombineCandidate;
    expect(combinePreview(candidate)).toEqual({
      lines: [1, 4],
      stations: [
        { line_id: 1, station_index: 3 },
        { line_id: 4, station_index: 0 },
      ],
    });
  });
});

describe('combinePreviewAll', () => {
  it('also rings the stations of every tie the combine turns into a loop closure', () => {
    const plan = (kept: number, a: [number, number], b: [number, number]) => ({
      kept,
      torn_ties: [
        {
          station_a: { line_id: a[0], station_index: a[1] },
          station_b: { line_id: b[0], station_index: b[1] },
          kind: 'merge',
        },
      ],
    });
    const candidate = {
      line_a: 1,
      line_b: 4,
      station_a: { line_id: 1, station_index: 3 },
      station_b: { line_id: 4, station_index: 0 },
      plan_keeping_a: plan(1, [4, 2], [7, 1]),
      plan_keeping_b: plan(4, [1, 0], [7, 0]),
    } as unknown as CombineCandidate;
    expect(combinePreviewAll([candidate]).stations).toEqual([
      { line_id: 1, station_index: 3 },
      { line_id: 4, station_index: 0 },
      { line_id: 4, station_index: 2 },
      { line_id: 7, station_index: 1 },
      { line_id: 1, station_index: 0 },
      { line_id: 7, station_index: 0 },
    ]);
  });

  it('rings the stations of every tie the combine removes across one shot too (#746)', () => {
    const at = (line_id: number, station_index: number) => ({ line_id, station_index });
    const candidate = {
      line_a: 1,
      line_b: 4,
      station_a: at(1, 3),
      station_b: at(4, 0),
      plan_keeping_a: {
        kept: 1,
        torn_ties: [],
        closures_removed_across_one_shot: [{ station_a: at(7, 1), station_b: at(4, 0), kind: 'merge' }],
      },
      plan_keeping_b: {
        kept: 4,
        torn_ties: [],
        closures_removed_across_one_shot: [{ station_a: at(1, 2), station_b: at(1, 0), kind: 'loop_closure' }],
      },
    } as unknown as CombineCandidate;
    expect(combinePreviewAll([candidate]).stations).toEqual([at(1, 3), at(4, 0), at(7, 1), at(1, 2), at(1, 0)]);
  });

  it('rings the stations of every closure the combine replaces, and of what takes its place (#749)', () => {
    const at = (line_id: number, station_index: number) => ({ line_id, station_index });
    const candidate = {
      line_a: 1,
      line_b: 4,
      station_a: at(1, 1),
      station_b: at(4, 0),
      plan_keeping_a: {
        kept: 1,
        torn_ties: [],
        closures_replaced: [
          {
            station_a: at(4, 2),
            station_b: at(4, 0),
            kind: 'legacy_closure',
            by_station_a: at(1, 1),
            by_station_b: at(4, 2),
            by_kind: 'merge',
          },
        ],
      },
      plan_keeping_b: {
        kept: 4,
        torn_ties: [],
        closures_replaced: [
          {
            station_a: at(4, 3),
            station_b: at(1, 0),
            kind: 'snapped_closure',
            by_station_a: at(4, 3),
            by_station_b: at(1, 1),
            by_kind: 'loop_closure',
          },
        ],
      },
    } as unknown as CombineCandidate;
    expect(combinePreviewAll([candidate]).stations).toEqual([at(1, 1), at(4, 0), at(4, 2), at(4, 3), at(1, 0)]);
  });
});

describe('formatRotation', () => {
  it('signs the angle and keeps one decimal', () => {
    expect(formatRotation(5)).toBe('+5.0°');
    expect(formatRotation(-0.34)).toBe('-0.3°');
    expect(formatRotation(0)).toBe('0.000°');
    expect(formatRotation(0.04)).toBe('+0.040°');
    expect(formatRotation(-0.04)).toBe('-0.040°');
  });
});

describe('combineMoveShown', () => {
  const tie = { station_a: { line_id: 4, station_index: 0 }, station_b: { line_id: 9, station_index: 1 } };
  const plan = (moving_line: number | null, shift_m: number, torn = 0) =>
    ({
      kept: 1,
      moving_line,
      moved_lines: moving_line === null ? [] : [moving_line],
      shift_m,
      torn_ties: Array.from({ length: torn }, () => ({ ...tie, kind: 'snapped_closure', reason: 'loop' })),
    }) as unknown as CombinePlan;

  it('lists a move past the side-by-side tolerance, or a smaller one that tears a tie (#740)', () => {
    expect(combineMoveShown(plan(null, 0))).toBe(false);
    expect(combineMoveShown(plan(null, 0, 1))).toBe(false);
    expect(combineMoveShown(plan(4, 0.0006))).toBe(false);
    expect(combineMoveShown(plan(4, 0.001))).toBe(false);
    expect(combineMoveShown(plan(4, 0.0006, 1))).toBe(true);
    expect(combineMoveShown(plan(4, 0.0011))).toBe(true);
    expect(combineMoveShown(plan(4, 0.4))).toBe(true);
  });

  it('words a listed move within the tolerance as side by side (#740)', () => {
    expect(combineMoveIsSmall(plan(4, 0.0006, 1))).toBe(true);
    expect(combineMoveIsSmall(plan(4, 0.001, 1))).toBe(true);
    expect(combineMoveIsSmall(plan(4, 0.0011))).toBe(false);
  });
});
