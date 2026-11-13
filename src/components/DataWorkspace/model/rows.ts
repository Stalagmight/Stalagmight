/**
 * Build the Data workspace's rows: per section (declaration order) an optional
 * header, then station 0, shot 1, station 1, ..., shot n, station n.
 * Reads the struct-of-arrays SurveyData only (never materializes Shot objects):
 * one pass over the station arrays to index them, one pass over the sections.
 */
import type { SurveyData } from '../../../types/survey-data';
import type { DataEntityRow, DataRow, ShotRow, StationRow } from '../types';

export const sectionRowKey = (sectionId: number) => `sec:${sectionId}`;
export const stationRowKey = (sectionId: number, stationIndex: number) => `st:${sectionId}:${stationIndex}`;
export const shotRowKey = (sectionId: number, shotId: number) => `sh:${sectionId}:${shotId}`;

/** (sectionId, stationIndex) -> global station index, built once per SurveyData. */
const stationIndexCache = new WeakMap<SurveyData, Map<number, Map<number, number>>>();

export function stationGlobalIndex(data: SurveyData): Map<number, Map<number, number>> {
  let cached = stationIndexCache.get(data);
  if (cached) return cached;
  cached = new Map();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g]!;
    let inner = cached.get(sid);
    if (!inner) cached.set(sid, (inner = new Map()));
    inner.set(data.stationIndices[g]!, g);
  }
  stationIndexCache.set(data, cached);
  return cached;
}

export function buildDataRows(data: SurveyData, opts: { showSectionHeaders: boolean }): DataRow[] {
  const stationG = stationGlobalIndex(data);
  const rows: DataRow[] = [];
  data.sections.forEach((sec, sectionOrder) => {
    const sectionId = sec.id;
    if (opts.showSectionHeaders) {
      rows.push({ kind: 'section', key: sectionRowKey(sectionId), sectionId, sectionOrder, order: rows.length });
    }
    const stations = stationG.get(sectionId);
    for (let i = 0; i <= sec.shotCount; i++) {
      const sg = stations?.get(i);
      if (sg !== undefined) {
        const station: StationRow = {
          kind: 'station',
          key: stationRowKey(sectionId, i),
          sectionId,
          sectionOrder,
          order: rows.length,
          stationIndex: i,
          g: sg,
        };
        rows.push(station);
      }
      if (i < sec.shotCount) {
        const g = sec.shotOffset + i;
        const shotId = data.shotIds[g]!;
        const shot: ShotRow = {
          kind: 'shot',
          key: shotRowKey(sectionId, shotId),
          sectionId,
          sectionOrder,
          order: rows.length,
          shotIndex: i,
          position: i + 1,
          shotId,
          g,
        };
        rows.push(shot);
      }
    }
  });
  return rows;
}

export function isEntityRow(row: DataRow | undefined): row is DataEntityRow {
  return row !== undefined && row.kind !== 'section';
}

/** Position of an entity inside its section's interleaved run: station i -> 2i,
 *  shot i -> 2i + 1. Two rows of one section are adjacent iff these differ by 1. */
export function sectionRunPosition(row: DataEntityRow): number {
  return row.kind === 'station' ? row.stationIndex * 2 : row.shotIndex * 2 + 1;
}

/** Count stations / shots / sections in a row list (headers count sections). */
export function countRows(rows: readonly DataRow[]): { stations: number; shots: number; sections: number } {
  let stations = 0;
  let shots = 0;
  const sections = new Set<number>();
  for (const r of rows) {
    if (r.kind === 'station') stations++;
    else if (r.kind === 'shot') shots++;
    sections.add(r.sectionId);
  }
  return { stations, shots, sections: sections.size };
}

export type ParsedRowKey =
  | { kind: 'section'; sectionId: number }
  | { kind: 'station'; sectionId: number; stationIndex: number }
  | { kind: 'shot'; sectionId: number; shotId: number };

/** Inverse of the row-key builders; null for anything else. */
export function parseRowKey(key: string): ParsedRowKey | null {
  const parts = key.split(':');
  const nums = parts.slice(1).map(Number);
  if (nums.some((n) => !Number.isInteger(n) || n < 0)) return null;
  if (parts[0] === 'sec' && nums.length === 1) return { kind: 'section', sectionId: nums[0]! };
  if (parts[0] === 'st' && nums.length === 2) return { kind: 'station', sectionId: nums[0]!, stationIndex: nums[1]! };
  if (parts[0] === 'sh' && nums.length === 2) return { kind: 'shot', sectionId: nums[0]!, shotId: nums[1]! };
  return null;
}
