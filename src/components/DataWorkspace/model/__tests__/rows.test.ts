import { describe, expect, it } from 'vitest';
import { buildDataRows, countRows, sectionRunPosition, shotRowKey, stationRowKey } from '../rows';
import { makeData } from '../../__tests__/dataFixtures';
import { shotMaterialization } from '../../../../types/survey-data';

describe('buildDataRows', () => {
  it('interleaves station 0, shot 1, station 1, ... per section in declaration order', () => {
    const rows = buildDataRows(makeData(), { showSectionHeaders: false });
    expect(rows.slice(0, 9).map((r) => r.key)).toEqual([
      'st:1:0',
      'sh:1:101',
      'st:1:1',
      'sh:1:102',
      'st:1:2',
      'sh:1:103',
      'st:1:3',
      'sh:1:104',
      'st:1:4',
    ]);
    expect(rows[9]!.key).toBe('st:2:0');
    // 3 sections: 5 + 3 + 4 stations, 4 + 2 + 3 shots
    expect(rows).toHaveLength(21);
    expect(rows.map((r) => r.order)).toEqual(rows.map((_, i) => i));
  });

  it('adds a header row per section when enabled', () => {
    const rows = buildDataRows(makeData(), { showSectionHeaders: true });
    expect(rows.filter((r) => r.kind === 'section').map((r) => r.key)).toEqual(['sec:1', 'sec:2', 'sec:3']);
    expect(rows[0]!.key).toBe('sec:1');
    expect(rows[1]!.key).toBe('st:1:0');
    expect(rows).toHaveLength(24);
  });

  it('carries the global indices, positions and section order', () => {
    const data = makeData();
    const rows = buildDataRows(data, { showSectionHeaders: false });
    const shot = rows.find((r) => r.key === shotRowKey(2, 202));
    expect(shot).toMatchObject({ kind: 'shot', sectionId: 2, sectionOrder: 1, shotIndex: 1, position: 2, shotId: 202 });
    if (shot?.kind !== 'shot') throw new Error('shot row');
    expect(data.shotIds[shot.g]).toBe(202);
    const station = rows.find((r) => r.key === stationRowKey(3, 2));
    if (station?.kind !== 'station') throw new Error('station row');
    expect(data.stationSectionIds[station.g]).toBe(3);
    expect(data.stationIndices[station.g]).toBe(2);
  });

  it('never materializes Shot objects', () => {
    const before = shotMaterialization.count;
    buildDataRows(makeData(), { showSectionHeaders: true });
    expect(shotMaterialization.count).toBe(before);
  });

  it('counts and positions rows', () => {
    const rows = buildDataRows(makeData(), { showSectionHeaders: true });
    expect(countRows(rows)).toEqual({ stations: 12, shots: 9, sections: 3 });
    const st = rows.find((r) => r.key === 'st:1:2');
    const sh = rows.find((r) => r.key === 'sh:1:103');
    if (st?.kind !== 'station' || sh?.kind !== 'shot') throw new Error('rows');
    expect(sectionRunPosition(st)).toBe(4);
    expect(sectionRunPosition(sh)).toBe(5);
  });
});
