/**
 * A minimal `SurveyData` for the survey-quality tests: only the fields the
 * metrics, the scores and the label builder read. Stations are listed in
 * global order; every shot defaults to pristine readings (azimuths equal, no
 * depth change, no inclination).
 */
import { emptySurveyData, type SurveyData } from '../../../../types/survey-data';
import type { ProjectWarning } from '../../../../types/commands';
import type { WarningSeverity } from '../../../../types/ipc/WarningSeverity';

export interface ShotReadings {
  azimuthIn?: number;
  azimuthOut?: number;
  length?: number;
  depthIn?: number;
  depthOut?: number;
  /** null = not recorded. */
  pitchIn?: number | null;
  pitchOut?: number | null;
}

export function buildSurveyData(spec: {
  lines: { id: number; shotIds: number[] }[];
  stations: { lineId: number; stationIndex: number; x?: number; y?: number }[];
  /** Readings per GLOBAL shot index; missing entries are pristine. */
  shots?: Record<number, ShotReadings>;
  /** Groups of GLOBAL station indices that are merged (fully transitive). */
  merged?: number[][];
}): SurveyData {
  const data = emptySurveyData();

  let offset = 0;
  for (const s of spec.lines) {
    data.lines.push({
      id: s.id,
      name: null,
      color: '#ffffff',
      direction: 'in',
      quality: 1,
      next_shot_id: 1,
      shotOffset: offset,
      shotCount: s.shotIds.length,
    });
    offset += s.shotIds.length;
  }

  const n = offset;
  data.shotCount = n;
  data.shotIds = new Uint32Array(n);
  data.shotLineIds = new Uint32Array(n);
  data.shotAzimuthIn = new Float32Array(n);
  data.shotAzimuthOut = new Float32Array(n);
  data.shotLength = new Float32Array(n);
  data.shotDepthIn = new Float32Array(n);
  data.shotDepthOut = new Float32Array(n);
  data.shotPitchIn = new Float32Array(n);
  data.shotPitchOut = new Float32Array(n);
  data.shotFlags = new Uint8Array(n);
  let c = 0;
  for (const s of spec.lines) {
    for (const id of s.shotIds) {
      const r = spec.shots?.[c] ?? {};
      data.shotIds[c] = id;
      data.shotLineIds[c] = s.id;
      data.shotAzimuthIn[c] = r.azimuthIn ?? 90;
      data.shotAzimuthOut[c] = r.azimuthOut ?? r.azimuthIn ?? 90;
      data.shotLength[c] = r.length ?? 10;
      data.shotDepthIn[c] = r.depthIn ?? 0;
      data.shotDepthOut[c] = r.depthOut ?? r.depthIn ?? 0;
      data.shotPitchIn[c] = r.pitchIn ?? NaN;
      data.shotPitchOut[c] = r.pitchOut ?? NaN;
      c++;
    }
  }

  data.stationCount = spec.stations.length;
  data.stationLineIds = new Uint32Array(spec.stations.length);
  data.stationIndices = new Uint32Array(spec.stations.length);
  data.stationCoords = new Float64Array(spec.stations.length * 3);
  data.stationFlags = new Uint8Array(spec.stations.length);
  // Every station has a project-unique Station ID; offset from the global
  // index so a test can never mistake one for the other.
  data.stationIds = new Uint32Array(spec.stations.length);
  spec.stations.forEach((st, i) => {
    data.stationLineIds[i] = st.lineId;
    data.stationIndices[i] = st.stationIndex;
    data.stationIds[i] = 500 + i;
    data.stationCoords[i * 3] = st.x ?? i * 10;
    data.stationCoords[i * 3 + 1] = st.y ?? 0;
  });

  for (const group of spec.merged ?? []) {
    for (const g of group) {
      // Mirror the backend: every member lists every OTHER member.
      const others = group
        .filter((x) => x !== g)
        .map((x) => ({ line_id: spec.stations[x]!.lineId, station_index: spec.stations[x]!.stationIndex }));
      data.stationMergedWith.set(g, others);
    }
  }

  return data;
}

/** One-shot survey with the given readings (shot 1 of line 1). */
export function oneShot(readings: ShotReadings): SurveyData {
  return buildSurveyData({
    lines: [{ id: 1, shotIds: [101] }],
    stations: [
      { lineId: 1, stationIndex: 0 },
      { lineId: 1, stationIndex: 1 },
    ],
    shots: { 0: readings },
  });
}

export function warn(
  lineId: number,
  shotId: number,
  severity: WarningSeverity,
  opts: { isResolved?: boolean; code?: string } = {},
): ProjectWarning {
  return {
    lineId,
    shotId,
    shotIndex: 0,
    code: opts.code ?? 'TEST',
    message: 'm',
    severity,
    isResolved: opts.isResolved ?? false,
  };
}
