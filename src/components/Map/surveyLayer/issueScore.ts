/**
 * Graded issue scores for the map's Survey Issues colouring and Errors labels.
 *
 * Every error of a shot scores 0 to 3 against its own Min / Warn / Max levels
 * (`issueScale.ts`); the shot's score combines them - the worst, or the
 * average, per the user's choice - leaving out errors the user switched off,
 * errors that cannot be computed and errors whose warning was ignored. On top:
 *
 *   - an unresolved ERROR on the shot (zero length, length shorter than the
 *     depth change, ...) forces 3 (the palette's end);
 *   - any other unresolved warning (short / long shot, large depth change, or a
 *     code this module does not know) floors the colour at 1 (the warn colour),
 *     as the old three-colour mode did. A warning about a graded metric does
 *     not: that metric already scores the shot on the user's levels;
 *   - an IGNORED (resolved) AZIMUTH_DEVIATION / PITCH_DEVIATION /
 *     INCLINATION_DEPTH_MISMATCH drops that metric from the shot.
 *
 * `labelScore` is the score without the warn floor: it ranks and colours the
 * Errors labels, which show the metric values. A station combines the shots
 * touching it the same way (worst or average), is raised to the severity of a
 * datum warning naming it (`datum_gps_gap`), and shares its score across its merge
 * group (`stationMergedWith` is transitive, so one sweep per group is exact).
 *
 * Pure: aligned to the global shot / station indices `SurveyData` uses.
 */
import type { SectionMetadata, SurveyData } from '../../../types/survey-data';
import type { ProjectWarning } from '../../../types/commands';
import type { DecimalPrecision, LengthUnit } from '../../../types/config';
import type { LoopClosure } from '../../../types/project';
import type { ResolvedPalette } from '../../../utils/palettes/sample';
import { buildStationLookup } from './customRender/tessellate';
import { computeShotQualityMetrics, shotMetricScore, WARNING_CODE_METRICS, type ShotQualityMetrics } from './shotQuality';
import {
  positionToScore,
  sameIssueScale,
  SCORE_MAX,
  SCORE_WARNING,
  ScoreCombiner,
  scoreToPosition,
  type IssueScale,
} from './issueScale';

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
  scale: IssueScale,
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
  //     A warning about a graded metric (azimuth / inclination in vs out,
  //     inclination vs depth) sets no floor: its metric already scores it on
  //     the user's levels, and switching that metric off or raising its levels
  //     must be able to take the shot below the warn colour.
  const lookupShotGlobal = shotGlobalLookup(data);
  for (const w of warnings) {
    const g = lookupShotGlobal(w.sectionId, w.shotId);
    if (g === null) continue; // a warning for a shot not in this SoA (stale)
    if (w.isResolved) {
      ignored[g] = ignored[g]! | (WARNING_CODE_METRICS[w.code] ?? 0);
    } else if (w.severity === 'error') {
      hardError[g] = 1;
    } else if (!Object.prototype.hasOwnProperty.call(WARNING_CODE_METRICS, w.code)) {
      warned[g] = 1;
    }
  }

  // (2) Per-shot scores.
  const combiner = new ScoreCombiner(scale.combine);
  for (let i = 0; i < n; i++) {
    const metric = shotMetricScore(metrics, i, scale, ignored[i]! | scale.hiddenMask, combiner);
    labelScore[i] = hardError[i] ? SCORE_MAX : metric;
    shotScore[i] = hardError[i] ? SCORE_MAX : Math.max(metric, warned[i] ? SCORE_WARNING : 0);
  }

  // (3) (sectionId, stationIndex) -> global station index, the renderer's own map.
  const stationLookup = buildStationLookup(data);
  const average = scale.combine === 'average';
  // Average mode: palette-position sums and shot counts per station.
  const posSum = average ? new Float64Array(data.stationCount) : null;
  const posCount = average ? new Uint32Array(data.stationCount) : null;
  const addTo = (g: number | undefined, s: number) => {
    if (g === undefined) return;
    if (posSum && posCount) {
      posSum[g] = posSum[g]! + scoreToPosition(s);
      posCount[g] = posCount[g]! + 1;
    } else {
      raise(stationScore, g, s);
    }
  };

  // (4) Each shot's score into its endpoint stations. Shot i of a section
  //     connects station_index i and i + 1.
  for (const sec of data.sections) {
    const inner = stationLookup.get(sec.id);
    if (!inner) continue;
    for (let i = 0; i < sec.shotCount; i++) {
      const s = shotScore[sec.shotOffset + i]!;
      if (s === 0 && !average) continue;
      addTo(inner.get(i), s);
      addTo(inner.get(i + 1), s);
    }
  }

  // (5) Share the score across each transitive merge group: the highest, or
  //     the average over every shot touching any member.
  const visited = new Uint8Array(data.stationCount);
  for (let g = 0; g < data.stationCount; g++) {
    if (visited[g]) continue;
    visited[g] = 1;
    const group = data.stationMergedWith.get(g);
    const members = [g];
    for (const ref of group ?? []) {
      const mg = stationLookup.get(ref.section_id)?.get(ref.station_index);
      if (mg !== undefined) members.push(mg);
    }
    let shared = 0;
    if (posSum && posCount) {
      let sum = 0;
      let count = 0;
      for (const mg of members) {
        sum += posSum[mg]!;
        count += posCount[mg]!;
      }
      shared = count > 0 ? positionToScore(sum / count) : 0;
    } else {
      for (const mg of members) shared = Math.max(shared, stationScore[mg]!);
    }
    for (const mg of members) {
      stationScore[mg] = shared;
      visited[mg] = 1;
    }
  }

  // (6) A datum warning (`datum_gps_gap`, #741) is attached to a shot next to
  //     its station but names the station itself in `datumStationIndex`, so
  //     colour that station directly. After the sharing: a datum's own warning
  //     is not averaged away.
  for (const w of warnings) {
    if (w.isResolved || w.datumStationIndex === undefined) continue;
    const s = w.severity === 'error' ? SCORE_MAX : SCORE_WARNING;
    raise(stationScore, stationLookup.get(w.sectionId)?.get(w.datumStationIndex), s);
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
  /** The localized part prefixes, e.g. "Δaz", and the closure label's "Loop". */
  abbrev: { az: string; inc: string; dep: string; len: string; loop: string };
}

/** What the map asks the survey layer to draw for Survey Issues. */
export interface IssueRenderInput {
  /** The graded Survey Issues colouring is on. */
  color: boolean;
  /** The Errors labels are on (and how to format them), else null. */
  labels: ErrorLabelFormat | null;
  warnings: readonly ProjectWarning[];
  /** Per-error levels, the switched-off errors and the combine mode. */
  scale: IssueScale;
  /** The Survey Issues palette, or null for the default green -> yellow -> red
   *  (which then takes its original code path, byte for byte). */
  palette: ResolvedPalette | null;
}

/** Everything the label builder needs to draw the Errors labels. */
export interface ErrorLabelInput {
  metrics: ShotQualityMetrics;
  scores: IssueScores;
  format: ErrorLabelFormat;
  closures: readonly LoopClosure[];
  scale: IssueScale;
  palette: ResolvedPalette | null;
  /** Survey Issues paints the map: loop closures are coloured on the same
   *  scale (else they keep their own colour bands). */
  graded: boolean;
}

export interface IssueRender {
  scores: IssueScores;
  labels: ErrorLabelInput | null;
}

interface RenderMemo {
  warnings: readonly ProjectWarning[];
  scale: IssueScale;
  scores: IssueScores;
  input: IssueRenderInput;
  closures: readonly LoopClosure[];
  render: IssueRender;
}

const renderMemo = new WeakMap<SurveyData, RenderMemo>();

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
  const sameScores = !!prev && prev.warnings === input.warnings && sameIssueScale(prev.scale, input.scale);
  if (
    prev &&
    sameScores &&
    prev.input.labels === input.labels &&
    prev.input.color === input.color &&
    prev.input.palette?.key === input.palette?.key &&
    prev.closures === closureList
  ) {
    return prev.render;
  }
  const metrics = computeShotQualityMetrics(data);
  const scores = prev && sameScores ? prev.scores : computeIssueScores(data, metrics, input.warnings, input.scale);
  const render: IssueRender = {
    scores,
    labels: input.labels
      ? {
          metrics,
          scores,
          format: input.labels,
          closures: closureList,
          scale: input.scale,
          palette: input.palette,
          graded: input.color,
        }
      : null,
  };
  renderMemo.set(data, { warnings: input.warnings, scale: input.scale, scores, input, closures: closureList, render });
  return render;
}

const EMPTY_CLOSURES: readonly LoopClosure[] = [];

/** The committed input with a live preview's scale and palette in place, or
 *  the committed input itself when there is no preview. */
export function withIssueOverride(
  committed: IssueRenderInput | null,
  preview: { scale: IssueScale; palette: ResolvedPalette | null } | null,
): IssueRenderInput | null {
  if (!preview || !committed) return committed;
  return { ...committed, scale: preview.scale, palette: preview.palette };
}
