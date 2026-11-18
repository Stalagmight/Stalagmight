/**
 * Station, shot and line identity as the user sees it (SPEC 4.1/4.2).
 *
 * A station is named by its Name (unique within its line), a shot by its two
 * station names (`4 → 5`), a line by its project-unique name. The positional
 * handles the IPC addresses things with - `StationRef{line_id, station_index}`,
 * shot ids, line ids - are session handles that are never shown. Everything
 * user-visible that names a station, shot or line goes through here (or the
 * pure formatters it wraps), so no caller re-derives a label from the handles.
 *
 * Names are read from the active `SurveyData` (its dense `stationNames` and
 * `stationIds`); when a caller has no SurveyData (a unit test that seeds only
 * `stations`), the `Station[]` list is scanned instead.
 */
import i18n from '../i18n';
import type { Station, StationRef } from '../types/project';
import type { SurveyData } from '../types/survey-data';
import { displayStationName, formatShotLabel, formatStationLabel } from './formatters';

/** The SurveyData columns identity needs. */
export type IdentityData = Pick<
  SurveyData,
  'stationCount' | 'stationLineIds' | 'stationIndices' | 'stationNames' | 'stationIds' | 'lines' | 'shotIds'
>;

/** A line as identity needs it: the store's `project.lines` or `SurveyData.lines`. */
export interface IdentityLine {
  id: number;
  name: string | null;
}

export interface IdentitySource {
  surveyData?: IdentityData | null;
  /** Fallback when there is no SurveyData. */
  stations?: ReadonlyArray<Station> | null;
  lines?: ReadonlyArray<IdentityLine & { shots?: ReadonlyArray<{ id: number }> }> | null;
}

interface RefIndex {
  indices: Uint32Array;
  byLine: Map<number, number[]>;
}

/** `(line_id, station_index)` -> global station index, built once per SurveyData. */
const refIndexCache = new WeakMap<Uint32Array, RefIndex>();

function refIndexOf(data: IdentityData): RefIndex {
  const cached = refIndexCache.get(data.stationLineIds);
  if (cached && cached.indices === data.stationIndices) return cached;
  const byLine = new Map<number, number[]>();
  for (let g = 0; g < data.stationCount; g++) {
    const lineId = data.stationLineIds[g]!;
    let slots = byLine.get(lineId);
    if (!slots) {
      slots = [];
      byLine.set(lineId, slots);
    }
    slots[data.stationIndices[g]!] = g;
  }
  const built = { indices: data.stationIndices, byLine };
  refIndexCache.set(data.stationLineIds, built);
  return built;
}

/** The global station index of `ref` in `data`, or null when it has no such station. */
export function stationGlobalIndexOf(data: IdentityData, ref: StationRef): number | null {
  return refIndexOf(data).byLine.get(ref.line_id)?.[ref.station_index] ?? null;
}

function findStationIn(stations: ReadonlyArray<Station> | null | undefined, ref: StationRef): Station | undefined {
  return stations?.find(
    (s) => s.station_ref.line_id === ref.line_id && s.station_ref.station_index === ref.station_index,
  );
}

/**
 * A station's stored name, unsanitized (for an edit field). Every station of a
 * loaded project has one; an unknown station falls back to its index as text,
 * which is also the name the backend seeds for an unnamed station.
 */
export function stationNameOf(src: IdentitySource, ref: StationRef): string {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const g = stationGlobalIndexOf(data, ref);
    const name = g === null ? undefined : data.stationNames.get(g);
    if (name !== undefined && name !== '') return name;
    if (g !== null) return String(ref.station_index);
  }
  const station = findStationIn(src.stations, ref);
  return station?.name || String(ref.station_index);
}

/** The project-unique Station ID of `ref`, or null when the station is unknown. */
export function stationIdOf(src: IdentitySource, ref: StationRef): number | null {
  const data = src.surveyData;
  if (data && data.stationCount > 0) {
    const g = stationGlobalIndexOf(data, ref);
    if (g !== null) return data.stationIds[g] ?? null;
  }
  return findStationIn(src.stations, ref)?.id ?? null;
}

/** A line's name for display (sanitized), never its id: "Unnamed line" when it has none. */
export function lineDisplayName(name: string | null | undefined): string {
  const shown = displayStationName(name).trim();
  return shown || i18n.t('detailPanel.section.unnamed');
}

/** The display name of line `lineId` (see {@link lineDisplayName}). */
export function lineNameOf(src: IdentitySource, lineId: number): string {
  const line =
    src.lines?.find((l) => l.id === lineId) ?? src.surveyData?.lines.find((l) => l.id === lineId);
  return lineDisplayName(line?.name);
}

/** `17`, or `Main Line / 17` with `withLine`. */
export function stationLabelOf(src: IdentitySource, ref: StationRef, withLine = false): string {
  const name = stationNameOf(src, ref);
  return formatStationLabel(name, withLine ? lineNameOf(src, ref.line_id) : null);
}

/**
 * `4 → 5` (or `Main Line: 4 → 5` with `withLine`) for the shot at 0-based
 * position `shotIndex` of line `lineId`: it runs from station `shotIndex` to
 * station `shotIndex + 1`.
 */
export function shotLabelAt(src: IdentitySource, lineId: number, shotIndex: number, withLine = false): string {
  const from = stationNameOf(src, { line_id: lineId, station_index: shotIndex });
  const to = stationNameOf(src, { line_id: lineId, station_index: shotIndex + 1 });
  return formatShotLabel(from, to, withLine ? lineNameOf(src, lineId) : null);
}

/** The 0-based position of shot `shotId` in line `lineId`, or null when it is not there. */
export function shotIndexOf(src: IdentitySource, lineId: number, shotId: number): number | null {
  const data = src.surveyData;
  const meta = data?.lines.find((l) => l.id === lineId);
  if (data && meta) {
    for (let k = 0; k < meta.shotCount; k++) {
      if (data.shotIds[meta.shotOffset + k] === shotId) return k;
    }
    return null;
  }
  const shots = src.lines?.find((l) => l.id === lineId)?.shots;
  const k = shots?.findIndex((s) => s.id === shotId) ?? -1;
  return k < 0 ? null : k;
}

/** {@link shotLabelAt} for a shot addressed by its id; null when the shot is unknown. */
export function shotLabelOf(src: IdentitySource, lineId: number, shotId: number, withLine = false): string | null {
  const k = shotIndexOf(src, lineId, shotId);
  return k === null ? null : shotLabelAt(src, lineId, k, withLine);
}

/** The identity labels bound to one source; what {@link useIdentityLabels} returns. */
export interface IdentityLabels {
  stationName: (ref: StationRef) => string;
  stationId: (ref: StationRef) => number | null;
  lineName: (lineId: number) => string;
  station: (ref: StationRef, withLine?: boolean) => string;
  shotAt: (lineId: number, shotIndex: number, withLine?: boolean) => string;
  shot: (lineId: number, shotId: number, withLine?: boolean) => string | null;
}

export function identityLabels(src: IdentitySource): IdentityLabels {
  return {
    stationName: (ref) => stationNameOf(src, ref),
    stationId: (ref) => stationIdOf(src, ref),
    lineName: (lineId) => lineNameOf(src, lineId),
    station: (ref, withLine) => stationLabelOf(src, ref, withLine),
    shotAt: (lineId, shotIndex, withLine) => shotLabelAt(src, lineId, shotIndex, withLine),
    shot: (lineId, shotId, withLine) => shotLabelOf(src, lineId, shotId, withLine),
  };
}
