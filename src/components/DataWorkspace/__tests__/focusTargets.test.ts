/**
 * Where a focus request or an app selection for a SECTION lands in the Data
 * grid: never on a header the view does not show (a header is never revealed,
 * so such a request would stay pending and block re-anchoring).
 */
import { describe, expect, it } from 'vitest';
import { buildDataRows } from '../model/rows';
import { focusTargetKey, initialActiveCell } from '../DataGrid';
import { DATA_COLUMNS, DATA_DEFAULT_HIDDEN } from '../columnsMeta';
import { rowKeyForSelection } from '../useDataNavigation';
import type { DataModel } from '../DataModelContext';
import type { DataRow, DataView } from '../types';
import { makeCtx } from './dataFixtures';

const ctx = makeCtx();
const all = buildDataRows(ctx.data, { showSectionHeaders: true });
const indexOf = (rows: readonly DataRow[]) => new Map(rows.map((r, i) => [r.key, i]));
const model = { rowByKey: new Map(all.map((r) => [r.key, r])) } as unknown as DataModel;
const viewOf = (rows: DataRow[]) => ({ rows }) as unknown as DataView;
const section = { type: 'section' as const, sectionId: 2 };

describe('focusTargetKey', () => {
  it('keeps a displayed header and any non-section key', () => {
    expect(focusTargetKey('sec:2', all, indexOf(all))).toBe('sec:2');
    expect(focusTargetKey('st:9:9', all, indexOf(all))).toBe('st:9:9');
  });

  it('falls back to the section first displayed row, in display order', () => {
    const sorted = all.filter((r) => r.kind !== 'section').reverse();
    const expected = sorted.find((r) => r.sectionId === 2)!.key;
    expect(focusTargetKey('sec:2', sorted, indexOf(sorted))).toBe(expected);
  });

  it('is null when nothing of the section is displayed', () => {
    const only1 = all.filter((r) => r.sectionId === 1);
    expect(focusTargetKey('sec:2', only1, indexOf(only1))).toBeNull();
  });
});

describe('rowKeyForSelection (section)', () => {
  it('maps to the header only when the view shows it', () => {
    expect(rowKeyForSelection(section, model, viewOf(all))).toBe('sec:2');
    const sorted = all.filter((r) => r.kind !== 'section').reverse();
    expect(rowKeyForSelection(section, model, viewOf(sorted))).toBe(sorted.find((r) => r.sectionId === 2)!.key);
  });

  it('falls back to station 0 (revealable) when nothing of the section is displayed', () => {
    expect(rowKeyForSelection(section, model, viewOf(all.filter((r) => r.sectionId === 1)))).toBe('st:2:0');
  });

  it('without a view keeps the model rule', () => {
    expect(rowKeyForSelection(section, model)).toBe('sec:2');
    expect(rowKeyForSelection(section, { rowByKey: new Map() } as unknown as DataModel)).toBe('st:2:0');
  });
});

describe('initialActiveCell', () => {
  it('starts on the first entity row, in the first shown column that applies to it', () => {
    const start = initialActiveCell(all, DATA_COLUMNS, DATA_DEFAULT_HIDDEN);
    expect(start).not.toBeNull();
    const row = all[start!.row]!;
    expect(row.kind).toBe('station');
    // A station row skips the shot-only Length and lands on the first shown
    // column that applies to it: Comment (Depth is hidden by default), never
    // a blank cell.
    expect(start!.colKey).toBe('comment');
    // With Depth shown, Depth.
    expect(initialActiveCell(all, DATA_COLUMNS, DATA_DEFAULT_HIDDEN.filter((k) => k !== 'depth'))!.colKey).toBe('depth');
  });

  it('a shots-only view starts on Length', () => {
    const shots = all.filter((r) => r.kind === 'shot');
    expect(initialActiveCell(shots, DATA_COLUMNS, DATA_DEFAULT_HIDDEN)?.colKey).toBe('length');
  });

  it('no shown column applies to the first row: the first shot instead, never a blank cell', () => {
    const start = initialActiveCell(all, DATA_COLUMNS, [...DATA_DEFAULT_HIDDEN, 'comment']);
    expect(all[start!.row]!.kind).toBe('shot');
    expect(start!.colKey).toBe('length');
  });

  it('null when there is no entity row', () => {
    expect(initialActiveCell(all.filter((r) => r.kind === 'section'), DATA_COLUMNS, [])).toBeNull();
  });
});
