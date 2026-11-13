/**
 * Build the Data workspace's rows: per section (declaration order) an optional
 * header, then station 0, shot 1, station 1, ..., shot n, station n.
 * Reads the struct-of-arrays SurveyData only (never materializes Shot objects).
 *
 * CONTRACT STUB - owned by the workspace-shell work; refine freely, keep the signature.
 */
import type { SurveyData } from '../../../types/survey-data';
import type { DataRow } from '../types';

export function buildDataRows(data: SurveyData, opts: { showSectionHeaders: boolean }): DataRow[] {
  // global station index by (sectionId, stationIndex)
  const stationG = new Map<number, Map<number, number>>();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g] ?? 0;
    let m = stationG.get(sid);
    if (!m) stationG.set(sid, (m = new Map()));
    m.set(data.stationIndices[g] ?? 0, g);
  }
  const rows: DataRow[] = [];
  data.sections.forEach((sec, sectionOrder) => {
    const sectionId = sec.id;
    if (opts.showSectionHeaders) {
      rows.push({ kind: 'section', key: `sec:${sectionId}`, sectionId, sectionOrder, order: rows.length });
    }
    const stations = stationG.get(sectionId);
    for (let i = 0; i <= sec.shotCount; i++) {
      const sg = stations?.get(i);
      if (sg !== undefined) {
        rows.push({ kind: 'station', key: `st:${sectionId}:${i}`, sectionId, sectionOrder, order: rows.length, stationIndex: i, g: sg });
      }
      if (i < sec.shotCount) {
        const g = sec.shotOffset + i;
        const shotId = data.shotIds[g] ?? 0;
        rows.push({ kind: 'shot', key: `sh:${sectionId}:${shotId}`, sectionId, sectionOrder, order: rows.length, shotIndex: i, position: i + 1, shotId, g });
      }
    }
  });
  return rows;
}
