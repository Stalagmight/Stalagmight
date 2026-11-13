import { describe, expect, it } from 'vitest';
import { DATA_COLUMNS } from '../../columnsMeta';
import { buildDataRows } from '../rows';
import { coerceCell, f32, parseShotKind, validationError } from '../columns';
import { COLUMNS, col, makeCtx } from '../../__tests__/dataFixtures';
import type { DataEntityRow, DataRow } from '../../types';

const ctx = makeCtx();
const rows = buildDataRows(ctx.data, { showSectionHeaders: false });
const row = (key: string): DataEntityRow => {
  const r = rows.find((x: DataRow) => x.key === key);
  if (!r || r.kind === 'section') throw new Error(`no row ${key}`);
  return r;
};

describe('buildColumnDefs', () => {
  it('implements every column of columnsMeta, in order', () => {
    expect(COLUMNS.map((c) => c.key)).toEqual(DATA_COLUMNS.map((c) => c.key));
    for (const c of COLUMNS) {
      expect(typeof c.value).toBe('function');
      expect(c.editable).toBe(typeof c.parse === 'function');
    }
  });

  it('formats shot numbers with the per-category precision', () => {
    const shot = row('sh:1:101');
    expect(col('length').text(shot, ctx)).toBe('8.25');
    expect(col('length').value(shot, ctx)).toBeCloseTo(8.25);
    expect(col('azimuth_in').text(shot, ctx)).toBe('45.0');
    expect(col('azimuth').text(shot, ctx)).toBe('46.0');
    expect(col('depth_in').text(shot, ctx)).toBe('1.0');
    expect(col('left').text(shot, ctx)).toBe('0.50');
    expect(col('up').text(shot, ctx)).toBe('');
    expect(col('up').value(shot, ctx)).toBeNull();
    expect(col('inclination').text(shot, ctx)).toBe('');
    expect(col('inclination').text(row('sh:1:103'), ctx)).toBe('3.0');
  });

  it('uses the locale for display and parsing', () => {
    const de = makeCtx({ locale: 'de-DE' });
    expect(col('length').text(row('sh:1:101'), de)).toBe('8,25');
    expect(col('length').parse!('7,5', row('sh:1:101'), de)).toBe(7.5);
  });

  it('shows "not applicable" as empty text and null value', () => {
    const st = row('st:1:1');
    expect(col('length').text(st, ctx)).toBe('');
    expect(col('length').value(st, ctx)).toBeNull();
    expect(col('length').lockReason(st, ctx)).toBe('na');
    expect(col('depth').lockReason(row('sh:1:101'), ctx)).toBe('na');
  });

  it('reads station columns from the active geometry', () => {
    const st = row('st:1:3');
    expect(col('x').text(st, ctx)).toBe('20.00');
    expect(col('y').value(st, ctx)).toBe(8);
    expect(col('depth').text(st, ctx)).toBe('2.0');
    expect(col('lat').text(row('st:1:0'), ctx)).toBe('20.500000');
    // Derived from the first zero datum's GPS for stations without their own.
    expect(Number(col('long').value(st, ctx))).toBeGreaterThan(-87.25);
  });

  it('identifies rows', () => {
    expect(col('type').text(row('st:1:1'), ctx)).toBe('Station');
    expect(col('type').text(row('sh:1:101'), ctx)).toBe('Shot');
    expect(col('section').text(row('sh:3:301'), ctx)).toBe('Downstream sump 1A');
    expect(col('name').text(row('st:1:1'), ctx)).toBe('A1');
    expect(col('fromTo').text(row('sh:1:101'), ctx)).toBe('0 → A1');
    expect(col('fromTo').text(row('sh:1:103'), ctx)).toBe('2 → 3');
  });

  it('shows the kind glyph and the raw kind as value', () => {
    expect(col('kind').text(row('sh:2:202'), ctx)).toBe('J');
    expect(col('kind').text(row('sh:2:201'), ctx)).toBe('');
    expect(col('kind').value(row('sh:2:202'), ctx)).toBe('Jump');
  });

  it('lists links: zero datum, merges, branch start and closures with misclosure', () => {
    expect(col('links').text(row('st:1:0'), ctx)).toBe('Zero datum');
    expect(col('links').text(row('st:1:2'), ctx)).toBe('⇄ Branch B:0');
    expect(col('links').text(row('st:2:0'), ctx)).toBe('Continues from Main:2');
    expect(col('links').text(row('st:1:4'), ctx)).toBe(
      'Closes to Downstre…sump 1A:3 · Δ 0.70 m · 1.3 %',
    );
    expect(col('links').text(row('st:1:3'), ctx)).toBe('');
  });

  it('lists unresolved warnings as searchable text, errors weigh most', () => {
    const errShot = row('sh:3:303');
    expect(col('issues').text(errShot, ctx)).toBe('Depth jump too large [ZERO_DEPTH]');
    expect(col('issues').value(errShot, ctx)).toBe(1000);
    expect(col('issues').value(row('sh:3:302'), ctx)).toBe(1);
    expect(col('issues').value(row('sh:1:101'), ctx)).toBeNull();
  });

  it('reads comments for both row kinds', () => {
    expect(col('comment').text(row('sh:1:101'), ctx)).toBe('Entrance squeeze');
    expect(col('comment').text(row('st:1:2'), ctx)).toBe('Junction');
    expect(col('comment').lockReason(row('st:1:2'), ctx)).toBeNull();
  });

  it('locks geometry of frozen shots but not LRUD, kind or comment', () => {
    const frozen = row('sh:3:301');
    for (const key of ['length', 'azimuth', 'azimuth_in', 'inclination', 'pitch_out', 'depth_in', 'depth_out']) {
      expect(col(key).lockReason(frozen, ctx)).toBe('frozen');
    }
    for (const key of ['left', 'down', 'kind', 'comment', 'temperature']) {
      expect(col(key).lockReason(frozen, ctx)).toBeNull();
    }
    expect(col('length').lockReason(row('sh:3:302'), ctx)).toBeNull();
  });

  it('locks computed columns and everything when editing is blocked', () => {
    expect(col('x').lockReason(row('st:1:1'), ctx)).toBe('computed');
    expect(col('links').lockReason(row('st:1:1'), ctx)).toBe('computed');
    const blocked = makeCtx({ editingBlocked: true });
    expect(col('comment').lockReason(row('st:1:1'), blocked)).toBe('blocked');
    expect(col('length').lockReason(row('sh:1:101'), blocked)).toBe('blocked');
    expect(col('name').lockReason(row('st:1:1'), blocked)).toBe('blocked');
  });

  it('parses and validates like the DataTable', () => {
    const shot = row('sh:1:101');
    expect(col('length').parse!('0', shot, ctx)).toBeUndefined();
    expect(col('length').parse!('', shot, ctx)).toBeUndefined();
    expect(col('length').parse!('3.5', shot, ctx)).toBe(3.5);
    expect(col('azimuth_in').parse!('360', shot, ctx)).toBeUndefined();
    expect(col('azimuth_in').parse!('359.9', shot, ctx)).toBe(359.9);
    expect(col('pitch_in').parse!('-91', shot, ctx)).toBeUndefined();
    expect(col('pitch_in').parse!('', shot, ctx)).toBeNull();
    expect(col('left').parse!('-1', shot, ctx)).toBeUndefined();
    expect(col('left').parse!('', shot, ctx)).toBeNull();
    expect(col('depth_in').parse!('', shot, ctx)).toBeUndefined();
    expect(col('depth_in').parse!('-3', shot, ctx)).toBe(-3);
    expect(col('comment').parse!('   ', shot, ctx)).toBeNull();
    expect(col('name').parse!('  B7 ', row('st:1:3'), ctx)).toBe('B7');
    expect(col('kind').parse!('r', shot, ctx)).toBe('Reference');
    expect(col('kind').parse!('maybe', shot, ctx)).toBeUndefined();
    expect(col('date').parse!('2026-02-30', shot, ctx)).toBeUndefined();
    expect(col('date').parse!('2026-02-03', shot, ctx)).toBe('2026-02-03');
    expect(col('surveyors').parse!('Ann <ann@example.com>; Bob', shot, ctx)).toEqual([
      { name: 'Ann', email: 'ann@example.com' },
      { name: 'Bob' },
    ]);
  });

  it('rejects an averaged inclination that would push a leg past 90', () => {
    const tilted = row('sh:1:103'); // legs 2 and 4
    expect(validationError('inclination', 89.5, tilted, ctx)).toBe('dataTable.validation.pitchRange');
    expect(validationError('inclination', 80, tilted, ctx)).toBeNull();
    expect(coerceCell('inclination', '', tilted, ctx)).toBeNull();
  });

  it('accepts the kind spellings of the DataTable', () => {
    expect(parseShotKind('J')).toBe('Jump');
    expect(parseShotKind('true')).toBe('Jump');
    expect(parseShotKind('normal')).toBe('Normal');
  });

  it('reads the survey credit of the shot', () => {
    const credited = makeCtx();
    credited.project.sections[0]!.shots[0]!.credit = {
      surveyors: [{ name: 'Ann', email: 'ann@example.com' }],
      explorers: [],
      date: '2026-03-01',
    } as never;
    expect(col('surveyors').text(row('sh:1:101'), credited)).toBe('Ann <ann@example.com>');
    expect(col('date').text(row('sh:1:101'), credited)).toBe('2026-03-01');
    expect(col('explorers').text(row('sh:1:102'), credited)).toBe('');
    delete credited.project.sections[0]!.shots[0]!.credit;
  });

  it('reads float32 slots back as the decimals that were stored', () => {
    expect(f32(Math.fround(8.3))).toBe(8.3);
    expect(f32(Math.fround(120.1))).toBe(120.1);
    expect(f32(Math.fround(-0.25))).toBe(-0.25);
    expect(Number.isNaN(f32(NaN))).toBe(true);
    // 8.25 is exact in float32; 8.3 is not, and still reads back as 8.3.
    const c = makeCtx();
    const shot = row('sh:1:102');
    if (shot.kind !== 'shot') throw new Error('shot');
    c.data.shotLength[shot.g] = 8.3;
    expect(c.data.shotLength[shot.g]).not.toBe(8.3);
    expect(col('length').value(shot, c)).toBe(8.3);
  });
});
