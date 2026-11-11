/**
 * Graded issue scores for the map's Survey Issues colouring and Errors labels.
 *
 * Every shot gets a score from 0 to 3 (see `shotQuality.ts`): 0 when its
 * readings agree, 1 at a warning threshold, 3 at three times the threshold.
 * On top of the metric score:
 *
 *   - an unresolved ERROR on the shot (zero length, length shorter than the
 *     depth change, ...) forces 3 (red);
 *   - any other unresolved warning (short / long shot, large depth change, or a
 *     code this module does not know) floors the colour at 1 (yellow), as the
 *     old three-colour mode did;
 *   - an IGNORED (resolved) AZIMUTH_DEVIATION / PITCH_DEVIATION /
 *     INCLINATION_DEPTH_MISMATCH drops that metric from the shot.
 *
 * `labelScore` is the score without the yellow floor: it ranks and colours the
 * Errors labels, which show the metric values. A station takes the highest
 * score of the shots touching it, raised to 3 when it is a datum of an
 * over-anchored merge group, and the score is shared across its merge group
 * (`stationMergedWith` is transitive, so one sweep per group is exact).
 *
 * Pure: aligned to the global shot / station indices `SurveyData` uses.
 */
import type { SectionMetadata, SurveyData } from '../../../types/survey-data';
import type { ProjectWarning } from '../../../types/commands';
import type { DecimalPrecision, LengthUnit } from '../../../types/config';
import type { LoopClosure } from '../../../types/project';
import {
  computeShotQualityMetrics,
  SCORE_MAX,
  SCORE_WARNING,
  shotMetricScore,
  WARNING_CODE_METRICS,
  type ShotQualityMetrics,
  type ShotQualityThresholds,
} from './shotQuality';

export interface IssueScores {
  /** Colour score per GLOBAL shot index. */
  shotScore: Float32Array;
  /** Colour score per GLOBAL station index. */
  stationScore: Float32Array;
  /** Metric score per shot (3 for a hard error), without the warning floor. */
  labelScore: Float32Array;
  /** Per shot: the metric bits (`METRIC_*`) whose warning was ignored. */
  ignored: Uint8Array;
}

export function computeIssueScores(
  data: SurveyData,
  metrics: ShotQualityMetrics,
  warnings: readonly ProjectWarning[],
  thresholds: ShotQualityThresholds,
): IssueScores {
  const n = data.shotCount;
  const shotScore = new Float32Array(n);
  const stationScore = new Float32Array(data.stationCount);
  const labelScore = new Float32Array(n);
  const ignored = new Uint8Array(n);
  const hardError = new Uint8Array(n);
  const warned = new Uint8Array(n);

  // (1) Unresolved warnings and ignored metrics per shot, keyed by
  //     (sectionId, shotId) and resolved to a global index through a
  //     per-section map built lazily, so untouched sections cost nothing.
  const lookupShotGlobal = shotGlobalLookup(data);
  for (const w of warnings) {
    const g = lookupShotGlobal(w.sectionId, w.shotId);
    if (g === null) continue; // a warning for a shot not in this SoA (stale)
    if (w.isResolved) {
      ignored[g] = ignored[g]! | (WARNING_CODE_METRICS[w.code] ?? 0);
    } else if (w.severity === 'error') {
      hardError[g] = 1;
    } else {
      warned[g] = 1;
    }
  }

  // (2) Per-shot scores.
  for (let i = 0; i < n; i++) {
    const metric = shotMetricScore(metrics, i, thresholds, ignored[i]!);
    labelScore[i] = hardError[i] ? SCORE_MAX : metric;
    shotScore[i] = hardError[i] ? SCORE_MAX : Math.max(metric, warned[i] ? SCORE_WARNING : 0);
  }

  // (3) (sectionId, stationIndex) -> global station index, as features.ts maps it.
  const stationLookup = new Map<number, Map<number, number>>();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g]!;
    let inner = stationLookup.get(sid);
    if (!inner) {
      inner = new Map();
      stationLookup.set(sid, inner);
    }
    inner.set(data.stationIndices[g]!, g);
  }

  // (4) Each shot's score into its endpoint stations. Shot i of a section
  //     connects station_index i and i + 1.
  for (const sec of data.sections) {
    const inner = stationLookup.get(sec.id);
    if (!inner) continue;
    for (let i = 0; i < sec.shotCount; i++) {
      const s = shotScore[sec.shotOffset + i]!;
      if (s === 0) continue;
      raise(stationScore, inner.get(i), s);
      raise(stationScore, inner.get(i + 1), s);
    }
  }

  // (4b) An over-anchoring error is attached to one representative shot but
  //      names every zero datum of the group in `datumRefs`; a datum can sit in
  //      a different merge group from that shot, so colour each one directly.
  for (const w of warnings) {
    if (w.isResolved || !w.datumRefs) continue;
    const s = w.severity === 'error' ? SCORE_MAX : SCORE_WARNING;
    for (const ref of w.datumRefs) raise(stationScore, stationLookup.get(ref.section_id)?.get(ref.station_index), s);
  }

  // (5) Share the highest score across each transitive merge group.
  const visited = new Uint8Array(data.stationCount);
  for (let g = 0; g < data.stationCount; g++) {
    if (visited[g]) continue;
    visited[g] = 1;
    const group = data.stationMergedWith.get(g);
    if (!group || group.length === 0) continue;
    const members = [g];
    for (const ref of group) {
      const mg = stationLookup.get(ref.section_id)?.get(ref.station_index);
      if (mg !== undefined) members.push(mg);
    }
    let groupMax = 0;
    for (const mg of members) groupMax = Math.max(groupMax, stationScore[mg]!);
    for (const mg of members) {
      stationScore[mg] = groupMax;
      visited[mg] = 1;
    }
  }

  return { shotScore, stationScore, labelScore, ignored };
}

function raise(scores: Float32Array, g: number | undefined, s: number): void {
  if (g !== undefined && s > scores[g]!) scores[g] = s;
}

function shotGlobalLookup(data: SurveyData): (sectionId: number, shotId: number) => number | null {
  const sectionById = new Map<number, SectionMetadata>();
  for (const s of data.sections) sectionById.set(s.id, s);
  const bySection = new Map<number, Map<number, number>>();
  return (sectionId, shotId) => {
    let inner = bySection.get(sectionId);
    if (!inner) {
      const sec = sectionById.get(sectionId);
      if (!sec) return null;
      inner = new Map();
      for (let i = sec.shotOffset; i < sec.shotOffset + sec.shotCount; i++) inner.set(data.shotIds[i]!, i);
      bySection.set(sectionId, inner);
    }
    return inner.get(shotId) ?? null;
  };
}

/** How the Errors labels format their values. */
export interface ErrorLabelFormat {
  precision: DecimalPrecision | undefined;
  lengthUnit: LengthUnit;
  /** The localized part prefixes, e.g. "Δaz". */
  abbrev: { az: string; inc: string; dep: string; len: string };
}

/** What the map asks the survey layer to draw for Survey Issues. */
export interface IssueRenderInput {
  /** The graded Survey Issues colouring is on. */
  color: boolean;
  /** The Errors labels are on (and how to format them), else null. */
  labels: ErrorLabelFormat | null;
  warnings: readonly ProjectWarning[];
  thresholds: ShotQualityThresholds;
}

/** Everything the label builder needs to draw the Errors labels. */
export interface ErrorLabelInput {
  metrics: ShotQualityMetrics;
  scores: IssueScores;
  format: ErrorLabelFormat;
  closures: readonly LoopClosure[];
}

export interface IssueRender {
  scores: IssueScores;
  labels: ErrorLabelInput | null;
}

interface RenderMemo {
  warnings: readonly ProjectWarning[];
  thresholds: ShotQualityThresholds;
  scores: IssueScores;
  format: ErrorLabelFormat | null;
  closures: readonly LoopClosure[];
  render: IssueRender;
}

const renderMemo = new WeakMap<SurveyData, RenderMemo>();

function sameThresholds(a: ShotQualityThresholds, b: ShotQualityThresholds): boolean {
  return a.azimuthDiff === b.azimuthDiff && a.pitchDiff === b.pitchDiff && a.inclinationDepthDiff === b.inclinationDepthDiff;
}

/**
 * The scores (and label input) for one flush, or null when neither the
 * colouring nor the labels are on. Memoized per `SurveyData`: a flush that
 * changes nothing these depend on (a theme change, a toggle elsewhere) gets the
 * SAME objects back, so the renderer's identity checks can keep their cheap
 * incremental paths.
 */
export function resolveIssueRender(
  data: SurveyData,
  input: IssueRenderInput | null,
  closures: readonly LoopClosure[] | undefined,
): IssueRender | null {
  if (!input || (!input.color && !input.labels)) return null;
  const closureList = closures ?? EMPTY_CLOSURES;
  const prev = renderMemo.get(data);
  if (
    prev &&
    prev.warnings === input.warnings &&
    sameThresholds(prev.thresholds, input.thresholds) &&
    prev.format === input.labels &&
    prev.closures === closureList
  ) {
    return prev.render;
  }
  const metrics = computeShotQualityMetrics(data);
  const scores =
    prev && prev.warnings === input.warnings && sameThresholds(prev.thresholds, input.thresholds)
      ? prev.scores
      : computeIssueScores(data, metrics, input.warnings, input.thresholds);
  const render: IssueRender = {
    scores,
    labels: input.labels ? { metrics, scores, format: input.labels, closures: closureList } : null,
  };
  renderMemo.set(data, {
    warnings: input.warnings,
    thresholds: input.thresholds,
    scores,
    format: input.labels,
    closures: closureList,
    render,
  });
  return render;
}

const EMPTY_CLOSURES: readonly LoopClosure[] = [];
