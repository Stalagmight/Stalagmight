/**
 * The Survey Issues score scale and its per-error Levels.
 *
 * Every error (azimuth in vs out, inclination in vs out, inclination vs depth,
 * length vs depth and inclination, loop closure) has its own Min / Warn / Max
 * levels: at or below Min it scores 0 (the palette's start), at Warn 1 (the
 * palette's middle, the "warn colour"), at or above Max 3 (the palette's end).
 * The defaults put Warn on the project's warning threshold and Max at three
 * times it, which is exactly the original scoring for the three angle checks.
 * The length check is new to the colour (it has no project threshold): 1 m
 * warn, 3 m max. Loop closures default to their line's own colour bands.
 *
 * These levels only change what the map draws (colours, label colours, which
 * label wins a collision). The warnings themselves come from the backend
 * validator and its Project Settings thresholds, which this never touches.
 *
 * Kept free of i18n and metric code: the colour ramp imports it, and some
 * Playwright specs load the ramp in Node.
 */

/** Score at a check's warning level (the palette's middle). */
export const SCORE_WARNING = 1;
/** Highest score: an error, or the Max level (the palette's end). */
export const SCORE_MAX = 3;

/** The errors, in display order. */
export const ISSUE_METRICS = ['azimuth', 'inclination', 'inclinationDepth', 'length', 'closure'] as const;
export type IssueMetricId = (typeof ISSUE_METRICS)[number];

/** One bit per error, for ignore / hide masks. */
export const METRIC_AZ = 1;
export const METRIC_PITCH = 2;
export const METRIC_INCL_DEPTH = 4;
export const METRIC_LENGTH = 8;
export const METRIC_CLOSURE = 16;

export const METRIC_BIT: Readonly<Record<IssueMetricId, number>> = {
  azimuth: METRIC_AZ,
  inclination: METRIC_PITCH,
  inclinationDepth: METRIC_INCL_DEPTH,
  length: METRIC_LENGTH,
  closure: METRIC_CLOSURE,
};

/** Value at the palette's start, middle (warn colour) and end. The unit is the
 *  error's own: degrees, metres, or percent for a loop closure. */
export interface IssueLevels {
  readonly min: number;
  readonly warn: number;
  readonly max: number;
}

export type IssueCombineMode = 'worst' | 'average';

/** Everything that decides a Survey Issues colour besides the palette. */
export interface IssueScale {
  readonly levels: Readonly<Record<IssueMetricId, IssueLevels>>;
  /** OR of the {@link METRIC_BIT}s of the errors that are switched off. */
  readonly hiddenMask: number;
  readonly combine: IssueCombineMode;
}

/** The project thresholds the default levels sit on, in degrees. */
export interface IssueThresholds {
  readonly azimuthDiff: number;
  readonly pitchDiff: number;
  readonly inclinationDepthDiff: number;
}

/** Default warn level for the length delta, in metres (no project threshold). */
export const LENGTH_WARN_DEFAULT_M = 1;
/** Default warn level for a loop closure, in percent: the closure line's own
 *  first colour band. Its max (5%) is that band scale's red. */
export const CLOSURE_WARN_DEFAULT_PCT = 1;
export const CLOSURE_MAX_DEFAULT_PCT = 5;

function onThreshold(threshold: number): IssueLevels {
  const t = Number.isFinite(threshold) && threshold > 0 ? threshold : 0;
  return { min: 0, warn: t, max: 3 * t };
}

/** The levels every error starts from: warn on its threshold, max at 3x. */
export function defaultIssueLevels(t: IssueThresholds): Record<IssueMetricId, IssueLevels> {
  return {
    azimuth: onThreshold(t.azimuthDiff),
    inclination: onThreshold(t.pitchDiff),
    inclinationDepth: onThreshold(t.inclinationDepthDiff),
    length: onThreshold(LENGTH_WARN_DEFAULT_M),
    closure: { min: 0, warn: CLOSURE_WARN_DEFAULT_PCT, max: CLOSURE_MAX_DEFAULT_PCT },
  };
}

/**
 * Score one error's size against its levels: 0 at or below Min, 1 at Warn, 3
 * at or above Max, linear in between. The sign does not matter. NaN scores 0.
 * Degenerate levels (a zero threshold) make any disagreement score 3.
 */
export function levelScore(value: number, levels: IssueLevels): number {
  const a = Math.abs(value);
  if (!(a > levels.min)) return 0;
  if (a <= levels.warn) return (a - levels.min) / (levels.warn - levels.min);
  if (a < levels.max) return SCORE_WARNING + ((SCORE_MAX - SCORE_WARNING) * (a - levels.warn)) / (levels.max - levels.warn);
  return SCORE_MAX;
}

/** Palette position (0..1) of a score: warn sits at the middle. */
export function scoreToPosition(score: number): number {
  const s = Number.isFinite(score) ? Math.min(Math.max(score, 0), SCORE_MAX) : 0;
  return s <= SCORE_WARNING
    ? (0.5 * s) / SCORE_WARNING
    : 0.5 + (0.5 * (s - SCORE_WARNING)) / (SCORE_MAX - SCORE_WARNING);
}

/** The score at a palette position (the inverse of {@link scoreToPosition}). */
export function positionToScore(position: number): number {
  const p = Number.isFinite(position) ? Math.min(Math.max(position, 0), 1) : 0;
  return p <= 0.5 ? 2 * p * SCORE_WARNING : SCORE_WARNING + 2 * (p - 0.5) * (SCORE_MAX - SCORE_WARNING);
}

/**
 * Combine several scores into one: the worst, or the average of their palette
 * positions (so one error at Max and one at Min give the warn colour). An empty
 * set is 0.
 */
export class ScoreCombiner {
  private worst = 0;
  private sum = 0;
  private n = 0;
  constructor(private readonly mode: IssueCombineMode) {}
  add(score: number): void {
    if (score > this.worst) this.worst = score;
    this.sum += scoreToPosition(score);
    this.n += 1;
  }
  result(): number {
    if (this.n === 0) return 0;
    return this.mode === 'worst' ? this.worst : positionToScore(this.sum / this.n);
  }
  reset(): void {
    this.worst = 0;
    this.sum = 0;
    this.n = 0;
  }
}

export function sameLevels(a: IssueLevels, b: IssueLevels): boolean {
  return a.min === b.min && a.warn === b.warn && a.max === b.max;
}

/** Whether two scales colour every shot identically. */
export function sameIssueScale(a: IssueScale | null | undefined, b: IssueScale | null | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.hiddenMask === b.hiddenMask &&
    a.combine === b.combine &&
    ISSUE_METRICS.every((id) => sameLevels(a.levels[id], b.levels[id]))
  );
}

/** Largest level accepted, in the error's own unit (far past any real
 *  degrees, metres or percent; keeps the slider's span finite). */
export const MAX_LEVEL = 1e6;

/** A valid levels triple: 0 <= min < warn < max <= {@link MAX_LEVEL}. */
export function validLevels(l: IssueLevels): boolean {
  return (
    Number.isFinite(l.min) && Number.isFinite(l.warn) && Number.isFinite(l.max) &&
    l.min >= 0 && l.min < l.warn && l.warn < l.max && l.max <= MAX_LEVEL
  );
}

/** Per-error levels the user set, by error; an absent error keeps its default. */
export type IssueLevelOverrides = Readonly<Partial<Record<IssueMetricId, IssueLevels>>>;

/** The hidden-error mask of a list of error ids (unknown ids are ignored). */
export function hiddenMaskOf(ids: readonly string[]): number {
  let mask = 0;
  for (const id of ids) {
    if (Object.prototype.hasOwnProperty.call(METRIC_BIT, id)) mask |= METRIC_BIT[id as IssueMetricId];
  }
  return mask;
}

/** The scale from the project thresholds, the user's levels, the switched-off
 *  errors and the combine mode. */
export function issueScaleFrom(
  thresholds: IssueThresholds,
  overrides: IssueLevelOverrides,
  hidden: readonly string[],
  combine: IssueCombineMode,
): IssueScale {
  const levels = defaultIssueLevels(thresholds);
  for (const id of ISSUE_METRICS) {
    const o = overrides[id];
    if (o && validLevels(o)) levels[id] = o;
  }
  return { levels, hiddenMask: hiddenMaskOf(hidden), combine };
}

/** `scale` with one error's levels replaced (a live drag preview). */
export function withMetricLevels(scale: IssueScale, id: IssueMetricId, levels: IssueLevels): IssueScale {
  return validLevels(levels) ? { ...scale, levels: { ...scale.levels, [id]: levels } } : scale;
}
