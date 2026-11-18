/**
 * The two questions asked before a line leaves for the backend: is any row
 * still half-filled (ask the user to finish it), and can the payload be
 * serialized at all (refuse, naming the row).
 */
import { describe, it, expect } from 'vitest';
import {
  NonFiniteShotError,
  assertSendableShots,
  findIncompleteLines,
  findNonFiniteShot,
} from '../importPayloadGuard';
import type { LineEditState } from '../batchImportHelpers';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import type { ShotData } from '../../types/project';

const shot = (overrides: Partial<ShotData> = {}): ShotData => ({
  azimuth_in: 10,
  azimuth_out: 12,
  length: 5,
  depth_in: 1,
  depth_out: 2,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  comment: null,
  kind: 'Normal',
  marker_index: null,
  temperature: null,
  ...overrides,
});

const editable = (overrides: Partial<EditableShot> = {}): EditableShot =>
  ({
    id: 1,
    azimuth_in: 10,
    azimuth_out: 12,
    length: 5,
    depth_in: 1,
    depth_out: 2,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    ...overrides,
  }) as EditableShot;

const edits = (name: string, shots: EditableShot[]): LineEditState => ({
  name,
  date: '',
  color: '',
  direction: 'in',
  quality: 1,
  reverse: false,
  shots,
});

describe('findNonFiniteShot', () => {
  it('passes a payload of real measurements', () => {
    expect(findNonFiniteShot([shot(), shot(), shot()])).toBeNull();
  });

  it('finds a cleared required value, 1-based as the grid numbers rows', () => {
    expect(findNonFiniteShot([shot(), shot({ azimuth_in: NaN })])).toEqual({
      row: 2,
      field: 'azimuth_in',
    });
    expect(findNonFiniteShot([shot({ length: NaN })])).toEqual({ row: 1, field: 'length' });
  });

  it('allows an absent optional value but not a non-finite one', () => {
    expect(findNonFiniteShot([shot({ pitch_in: null, left: null })])).toBeNull();
    expect(findNonFiniteShot([shot({ up: NaN })])).toEqual({ row: 1, field: 'up' });
    expect(findNonFiniteShot([shot({ pitch_out: Infinity })])).toEqual({
      row: 1,
      field: 'pitch_out',
    });
  });
});

describe('assertSendableShots', () => {
  it('says nothing about a sendable payload', () => {
    expect(() => assertSendableShots([shot()], 3, 'Tunnel')).not.toThrow();
  });

  it('throws a typed error naming the line and the row', () => {
    let caught: unknown;
    try {
      assertSendableShots([shot(), shot({ depth_out: NaN })], 3, 'Tunnel');
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(NonFiniteShotError);
    const err = caught as NonFiniteShotError;
    expect(err.lineIndex).toBe(3);
    expect(err.location).toEqual({ row: 2, field: 'depth_out' });
    expect(err.message).toContain('Tunnel');
    expect(err.message).toContain('row 2');
  });
});

describe('findIncompleteLines', () => {
  const complete = editable();
  const half = editable({ id: 2, azimuth_in: NaN, azimuth_out: NaN });
  const blank = editable({
    id: -1,
    isNew: true,
    length: NaN,
    azimuth_in: NaN,
    azimuth_out: NaN,
    depth_in: NaN,
    depth_out: NaN,
  });

  it('reports a line the user is no longer looking at', () => {
    const map = new Map([
      [0, edits('A', [complete])],
      [1, edits('B', [complete, half])],
    ]);
    expect(findIncompleteLines([0, 1], map)).toEqual([
      { lineIndex: 1, name: 'B', count: 1 },
    ]);
  });

  it('ignores lines that are not selected', () => {
    const map = new Map([[1, edits('B', [half])]]);
    expect(findIncompleteLines([0], map)).toEqual([]);
  });

  it('does not count a wholly blank added row - it is an invitation', () => {
    const map = new Map([[0, edits('A', [complete, blank])]]);
    expect(findIncompleteLines([0], map)).toEqual([]);
  });

  it('does not count a deleted half-filled row', () => {
    const map = new Map([[0, edits('A', [complete, { ...half, isDeleted: true }])]]);
    expect(findIncompleteLines([0], map)).toEqual([]);
  });

  it('returns every blocking line in line order', () => {
    const map = new Map([
      [2, edits('C', [half])],
      [0, edits('A', [half, half])],
    ]);
    expect(findIncompleteLines([2, 0], map).map((s) => s.lineIndex)).toEqual([0, 2]);
    expect(findIncompleteLines([2, 0], map)[0]?.count).toBe(2);
  });
});
