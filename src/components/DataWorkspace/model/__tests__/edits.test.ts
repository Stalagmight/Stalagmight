import { describe, expect, it } from 'vitest';
import { buildDataRows } from '../rows';
import { buildEditCommands } from '../edits';
import { makeCtx } from '../../__tests__/dataFixtures';
import type { DataCellEdit, DataColumnKey, DataEntityRow } from '../../types';

const ctx = makeCtx();
const rows = buildDataRows(ctx.data, { showSectionHeaders: true });
const row = (key: string): DataEntityRow => {
  const r = rows.find((x) => x.key === key);
  if (!r || r.kind === 'section') throw new Error(key);
  return r;
};
const edit = (key: string, colKey: DataColumnKey, value: unknown): DataCellEdit => ({ row: row(key), colKey, value });

describe('buildEditCommands', () => {
  it('groups shot fields into one update_shot per shot', () => {
    const plan = buildEditCommands(
      [edit('sh:1:101', 'length', 9), edit('sh:1:101', 'left', null), edit('sh:1:102', 'comment', 'Low')],
      ctx,
    );
    expect(plan.skipped).toEqual([]);
    expect(plan.commands).toEqual([
      { type: 'update_shot', section_id: 1, shot_id: 101, updates: { length: 9, left: null } },
      { type: 'update_shot', section_id: 1, shot_id: 102, updates: { comment: 'Low' } },
    ]);
  });

  it('expands the averaged azimuth / inclination like the DataTable', () => {
    const plan = buildEditCommands([edit('sh:1:101', 'azimuth', 50), edit('sh:1:103', 'inclination', 5)], ctx);
    const [az, inc] = plan.commands;
    expect(az).toMatchObject({ type: 'update_shot', shot_id: 101 });
    // legs 45 / 47 (mean 46) rotate by +4
    if (az?.type !== 'update_shot') throw new Error('update_shot');
    expect(az.updates.azimuth_in).toBeCloseTo(49);
    expect(az.updates.azimuth_out).toBeCloseTo(51);
    // legs 2 / 4 (mean 3) shift by +2
    expect(inc).toMatchObject({ type: 'update_shot', shot_id: 103, updates: { pitch_in: 4, pitch_out: 6 } });
  });

  it('lets explicit in/out legs win over the averaged angle of the same shot, in any order', () => {
    const avgAz = edit('sh:1:101', 'azimuth', 50);
    const legAz = edit('sh:1:101', 'azimuth_out', 200);
    const avgInc = edit('sh:1:103', 'inclination', 5);
    const legInc = edit('sh:1:103', 'pitch_in', -10);
    for (const order of [
      [avgAz, legAz, avgInc, legInc],
      [legInc, legAz, avgInc, avgAz],
    ]) {
      const plan = buildEditCommands(order, ctx);
      expect(plan.commands).toHaveLength(2);
      const byShot = new Map(plan.commands.map((c) => [c.type === 'update_shot' ? c.shot_id : -1, c]));
      // Only the leg edits survive; the untouched leg keeps its value.
      expect(byShot.get(101)).toEqual({ type: 'update_shot', section_id: 1, shot_id: 101, updates: { azimuth_out: 200 } });
      expect(byShot.get(103)).toEqual({ type: 'update_shot', section_id: 1, shot_id: 103, updates: { pitch_in: -10 } });
      expect(plan.skipped).toEqual(
        expect.arrayContaining([
          { edit: avgAz, reason: 'conflict' },
          { edit: avgInc, reason: 'conflict' },
        ]),
      );
      expect(plan.skipped).toHaveLength(2);
    }
  });

  it('keeps an averaged edit when only another shot has its legs edited', () => {
    const plan = buildEditCommands([edit('sh:1:101', 'azimuth', 50), edit('sh:1:102', 'azimuth_in', 10)], ctx);
    expect(plan.skipped).toEqual([]);
    expect(plan.commands).toHaveLength(2);
  });

  it('emits station comment and name commands', () => {
    const plan = buildEditCommands([edit('st:1:3', 'comment', 'Dome'), edit('st:1:3', 'name', 'B7')], ctx);
    expect(plan.commands).toEqual([
      { type: 'set_station_comment', station_ref: { section_id: 1, station_index: 3 }, comment: 'Dome' },
      { type: 'set_station_name', station_ref: { section_id: 1, station_index: 3 }, name: 'B7' },
    ]);
  });

  it('clears a station comment / name with null and skips unchanged values', () => {
    const plan = buildEditCommands(
      [edit('st:1:2', 'comment', null), edit('st:1:1', 'name', 'A1'), edit('sh:1:101', 'comment', 'Entrance squeeze')],
      ctx,
    );
    expect(plan.commands).toEqual([
      { type: 'set_station_comment', station_ref: { section_id: 1, station_index: 2 }, comment: null },
    ]);
    expect(plan.skipped).toEqual([]);
  });

  it('routes credit cells through edit_shot_credit in the same batch', () => {
    const plan = buildEditCommands(
      [edit('sh:1:102', 'surveyors', [{ name: 'Ann' }]), edit('sh:1:102', 'date', '2026-01-02'), edit('sh:1:102', 'length', 7)],
      ctx,
    );
    expect(plan.commands).toEqual([
      {
        type: 'edit_shot_credit',
        targets: { type: 'shots', shots: [[1, 102]] },
        edit: { set_surveyors: [{ name: 'Ann' }], set_date: '2026-01-02' },
      },
      { type: 'update_shot', section_id: 1, shot_id: 102, updates: { length: 7 } },
    ]);
  });

  it('reports frozen, computed, not-applicable and invalid edits as skipped', () => {
    const frozen = edit('sh:3:301', 'length', 4);
    const computed = edit('st:1:1', 'x', 4);
    const na = edit('st:1:1', 'length', 4);
    const invalid = edit('sh:1:101', 'length', undefined);
    const ok = edit('sh:3:301', 'left', 1);
    const plan = buildEditCommands([frozen, computed, na, invalid, ok], ctx);
    expect(plan.skipped).toEqual([
      { edit: frozen, reason: 'frozen' },
      { edit: computed, reason: 'computed' },
      { edit: na, reason: 'na' },
      { edit: invalid, reason: 'invalid' },
    ]);
    expect(plan.commands).toEqual([{ type: 'update_shot', section_id: 3, shot_id: 301, updates: { left: 1 } }]);
  });

  it('skips everything while editing is blocked', () => {
    const blocked = makeCtx({ editingBlocked: true });
    const plan = buildEditCommands([{ row: row('st:1:3'), colKey: 'comment', value: 'x' }], blocked);
    expect(plan.commands).toEqual([]);
    expect(plan.skipped[0]!.reason).toBe('blocked');
  });

  it('drops an unchanged kind but keeps a changed one', () => {
    const plan = buildEditCommands([edit('sh:2:202', 'kind', 'Jump'), edit('sh:2:201', 'kind', 'Reference')], ctx);
    expect(plan.commands).toEqual([{ type: 'update_shot', section_id: 2, shot_id: 201, updates: { kind: 'Reference' } }]);
  });

  it('reads people given as text (a moved display value) instead of clearing them', () => {
    const plan = buildEditCommands([edit('sh:1:102', 'surveyors', 'Ann <ann@example.com>; Bob')], ctx);
    expect(plan.commands).toEqual([
      {
        type: 'edit_shot_credit',
        targets: { type: 'shots', shots: [[1, 102]] },
        edit: { set_surveyors: [{ name: 'Ann', email: 'ann@example.com' }, { name: 'Bob' }] },
      },
    ]);
  });

  it('rotates float32 legs from their stored decimals, without float32 noise', () => {
    const c = makeCtx();
    const shot = rows.find((r) => r.key === 'sh:1:102');
    if (shot?.kind !== 'shot') throw new Error('shot');
    c.data.shotAzimuthIn[shot.g] = 120.1;
    c.data.shotAzimuthOut[shot.g] = 120.3;
    const plan = buildEditCommands([{ row: shot, colKey: 'azimuth', value: 130 }], c);
    const cmd = plan.commands[0];
    if (cmd?.type !== 'update_shot') throw new Error('update_shot');
    expect(cmd.updates.azimuth_in).toBeCloseTo(129.9, 9);
    expect(cmd.updates.azimuth_out).toBeCloseTo(130.1, 9);
  });
});
