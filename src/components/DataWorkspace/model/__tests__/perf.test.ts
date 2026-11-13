/**
 * Scale sanity: a synthetic 50k-shot survey (500 sections x 100 shots, merges
 * and closures sprinkled in) builds its rows, labels, every column's text and
 * the rail within a generous budget, without materializing a Shot.
 */
import { describe, expect, it } from 'vitest';
import { emptySurveyData, shotMaterialization, type SurveyData } from '../../../../types/survey-data';
import type { Project, StationRef } from '../../../../types/project';
import { buildDataRows } from '../rows';
import { buildSectionLabels } from '../sectionLabels';
import { buildColumnDefs } from '../columns';
import { buildRail } from '../links';
import type { DataModelCtx } from '../../types';

const SECTIONS = 500;
const SHOTS_PER = 100;

function bigData(): SurveyData {
  const d = emptySurveyData();
  const shots = SECTIONS * SHOTS_PER;
  const stations = SECTIONS * (SHOTS_PER + 1);
  const f32 = () => new Float32Array(shots);
  Object.assign(d, {
    shotCount: shots,
    shotIds: new Uint32Array(shots),
    shotSectionIds: new Uint32Array(shots),
    shotAzimuthIn: f32(),
    shotAzimuthOut: f32(),
    shotLength: f32(),
    shotDepthIn: f32(),
    shotDepthOut: f32(),
    shotPitchIn: f32().fill(NaN),
    shotPitchOut: f32().fill(NaN),
    shotLeft: f32(),
    shotRight: f32(),
    shotUp: f32().fill(NaN),
    shotDown: f32(),
    shotTemperature: f32().fill(NaN),
    shotFlags: new Uint8Array(shots),
    shotMarkerIndex: new Int32Array(shots).fill(-1),
    stationCount: stations,
    stationSectionIds: new Uint32Array(stations),
    stationIndices: new Uint32Array(stations),
    stationCoords: new Float64Array(stations * 3),
    stationFlags: new Uint8Array(stations),
  });
  let g = 0;
  let sg = 0;
  for (let s = 0; s < SECTIONS; s++) {
    const id = s + 1;
    d.sections.push({
      id,
      name: `Long synthetic passage number ${id} towards the sump`,
      color: '#336699',
      direction: 'Out',
      quality: 0,
      next_shot_id: 0,
      shotOffset: g,
      shotCount: SHOTS_PER,
    } as SurveyData['sections'][number]);
    for (let i = 0; i < SHOTS_PER; i++, g++) {
      d.shotIds[g] = id * 100_000 + i + 1;
      d.shotSectionIds[g] = id;
      d.shotAzimuthIn[g] = (i * 7) % 360;
      d.shotAzimuthOut[g] = (i * 7 + 1) % 360;
      d.shotLength[g] = 1 + (i % 9);
      d.shotDepthIn[g] = i * 0.1;
      d.shotDepthOut[g] = i * 0.1 + 0.1;
      d.shotLeft[g] = 1;
      d.shotRight[g] = 1;
      d.shotDown[g] = 0.5;
      d.shotFlags[g] = i % 25 === 0 ? 1 : 0;
      if (i % 10 === 0) d.shotComments.set(g, `note ${i}`);
    }
    for (let k = 0; k <= SHOTS_PER; k++, sg++) {
      d.stationSectionIds[sg] = id;
      d.stationIndices[sg] = k;
      d.stationCoords[sg * 3] = k;
      d.stationCoords[sg * 3 + 1] = s;
      d.stationCoords[sg * 3 + 2] = k * 0.1;
    }
    if (s > 0) {
      // Each section branches from station 50 of the previous one.
      const parent: StationRef = { section_id: id - 1, station_index: 50 };
      d.stationMergedWith.set(sg - SHOTS_PER - 1, [parent]);
    }
  }
  return d;
}

describe('Data workspace model at scale', () => {
  it('builds rows, labels, every cell text and the rail for 50k shots quickly', () => {
    const data = bigData();
    const before = shotMaterialization.count;
    const t0 = performance.now();
    const rows = buildDataRows(data, { showSectionHeaders: true });
    const labels = buildSectionLabels(data.sections, (id) => `Section ${id}`);
    const ctx: DataModelCtx = {
      data,
      project: { sections: [], loop_closures: [], zero_datums: [] } as unknown as Project,
      sectionsById: new Map(data.sections.map((s) => [s.id, s])),
      sectionLabels: labels,
      sectionNames: new Map(data.sections.map((s) => [s.id, s.name ?? ''])),
      warningsByShot: new Map(),
      frozenShots: new Set(),
      precision: 1,
      locale: 'en-US',
      editingBlocked: false,
    };
    const columns = buildColumnDefs().filter((c) => !['surveyors', 'explorers', 'date'].includes(c.key));
    let chars = 0;
    for (const row of rows) {
      if (row.kind === 'section') continue;
      for (const c of columns) {
        chars += c.text(row, ctx).length;
        c.value(row, ctx);
      }
    }
    const rail = buildRail(rows, ctx, true);
    const elapsed = performance.now() - t0;

    expect(rows).toHaveLength(SECTIONS * (2 * SHOTS_PER + 2));
    expect(new Set(labels.values()).size).toBe(SECTIONS);
    expect(rail).toHaveLength(rows.length);
    expect(chars).toBeGreaterThan(0);
    expect(shotMaterialization.count).toBe(before);
    // Generous: a few seconds on a slow CI box (typically well under one).
    expect(elapsed).toBeLessThan(15_000);
  });
});
