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
  sections: { id: number; shotIds: number[] }[];
  stations: { sectionId: number; stationIndex: number; x?: number; y?: number }[];
  /** Readings per GLOBAL shot index; missing entries are pristine. */
  shots?: Record<number, ShotReadings>;
  /** Groups of GLOBAL station indices that are merged (fully transitive). */
  merged?: number[][];
}): SurveyData {
  const data = emptySurveyData();

  let offset = 0;
  for (const s of spec.sections) {
    data.sections.push({
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
  data.shotSectionIds = new Uint32Array(n);
  data.shotAzimuthIn = new Float32Array(n);
  data.shotAzimuthOut = new Float32Array(n);
  data.shotLength = new Float32Array(n);
  data.shotDepthIn = new Float32Array(n);
  data.shotDepthOut = new Float32Array(n);
  data.shotPitchIn = new Float32Array(n);
  data.shotPitchOut = new Float32Array(n);
  data.shotFlags = new Uint8Array(n);
  let c = 0;
  for (const s of spec.sections) {
    for (const id of s.shotIds) {
      const r = spec.shots?.[c] ?? {};
      data.shotIds[c] = id;
      data.shotSectionIds[c] = s.id;
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
  data.stationSectionIds = new Uint32Array(spec.stations.length);
  data.stationIndices = new Uint32Array(spec.stations.length);
  data.stationCoords = new Float64Array(spec.stations.length * 3);
  data.stationFlags = new Uint8Array(spec.stations.length);
  spec.stations.forEach((st, i) => {
    data.stationSectionIds[i] = st.sectionId;
    data.stationIndices[i] = st.stationIndex;
    data.stationCoords[i * 3] = st.x ?? i * 10;
    data.stationCoords[i * 3 + 1] = st.y ?? 0;
  });

  for (const group of spec.merged ?? []) {
    for (const g of group) {
      // Mirror the backend: every member lists every OTHER member.
      const others = group
        .filter((x) => x !== g)
        .map((x) => ({ section_id: spec.stations[x]!.sectionId, station_index: spec.stations[x]!.stationIndex }));
      data.stationMergedWith.set(g, others);
    }
  }

  return data;
}

/** One-shot survey with the given readings (shot 1 of section 1). */
export function oneShot(readings: ShotReadings): SurveyData {
  return buildSurveyData({
    sections: [{ id: 1, shotIds: [101] }],
    stations: [
      { sectionId: 1, stationIndex: 0 },
      { sectionId: 1, stationIndex: 1 },
    ],
    shots: { 0: readings },
  });
}

export function warn(
  sectionId: number,
  shotId: number,
  severity: WarningSeverity,
  opts: { isResolved?: boolean; code?: string } = {},
): ProjectWarning {
  return {
    sectionId,
    shotId,
    shotIndex: 0,
    code: opts.code ?? 'TEST',
    message: 'm',
    severity,
    isResolved: opts.isResolved ?? false,
  };
}
