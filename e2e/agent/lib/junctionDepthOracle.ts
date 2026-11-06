/**
 * X6c - depths, tapes and plan across a TML round trip, under the junction
 * rule (#576 TML-DEPTH-D1, Q1-D1, Q2-D1, Q3-D1; docs/ariane-export.md,
 * "Junction depths").
 *
 * A station has ONE depth in Ariane's chain and ours can have several. The
 * export settles that, and this oracle asserts exactly what it settles to,
 * matching every shot to its re-imported self BY UUID:
 *
 * - EVERY STATION KEEPS ITS TRUE DEPTH: each end of a shot comes back at one of
 *   the readings its own section has of that station. A second reading inside
 *   a section (Q3-D1) is refitted or kept, so either of its two readings is
 *   true; a shot whose drop exceeds its tape (Q2-D1) keeps both.
 * - EXCEPT AN AVERAGED JUNCTION: a station the export wrote as one Ariane node
 *   for several sections whose readings disagree comes back at ONE depth, the
 *   mean of their readings with ONE reading per section (Q1-D1), however many
 *   pieces a section has there.
 * - THE LENGTH BOUND HOLDS: `|depth change| <= length`, against the shot's own
 *   start, or against the depth of the node it hangs off where it kept an
 *   explicit second reading.
 * - THE PLAN IS UNCHANGED: each shot keeps its heading and its horizontal run
 *   `sqrt(L^2 - dz^2)` (none for a broken shot), within what the file's six
 *   decimals and the micrometre round-up can move a run by. Plan positions are
 *   sums of those runs along the same tree, so they stay put too.
 *
 * WHICH STATIONS ARE ONE NODE comes from the RE-IMPORTED project: its station
 * merges are the file's `FromID` links (the importer's `FromImporting`
 * placement), and the uuid match ties each of its stations back to the
 * original's. The original's own merges are NOT used: a merge the export does
 * not take as a parent link (a second one closing a loop) leaves two nodes,
 * each at its own depth, and grouping by it would expect a mean that is not
 * there.
 *
 * WHICH READING A SECTION GIVES when its own readings at a junction disagree
 * depends on the direction the export walks it (the depth it arrives with), so
 * the oracle accepts the mean for any one-per-section choice rather than
 * re-deriving the walk. With every section single-valued, the usual case, the
 * mean is exact.
 *
 * This module only imports the error type, so it stays pinnable from vitest.
 */

import { ExportOracleError } from './exportOracles';

/** One shot, with where it sits: its section's id and its index in that section. */
export interface StationedShot {
  uuid?: string;
  section: number;
  index: number;
  length: number;
  /** The drawn bearing (degrees), as the export writes it. */
  azimuthIn: number;
  depthIn: number;
  depthOut: number;
}

/** A station merge: `[sectionA, stationA, sectionB, stationB]`. */
export type StationMergeWire = readonly [number, number, number, number];

export interface JunctionDepthReport {
  /** Original shots matched to a re-imported shot by a unique uuid. */
  matched: number;
  /** Original shots with no uuid, or none (or several) after: not checked here. */
  unmatched: number;
  /** Nodes whose readings disagreed and came back at their mean. */
  averagedJunctions: number;
  /** Matched shots whose tape changed (a refit). */
  refittedShots: number;
  /** Original shots whose own drop exceeds their tape. */
  brokenShots: number;
  /** Every way the round trip broke the rule, one line each. */
  failures: string[];
}

/** The file writes six decimals; ten micrometres is far below any reading. */
const DEPTH_TOLERANCE_M = 1e-5;
/** The bound as written: both depths and the tape at six decimals. */
const BOUND_TOLERANCE_M = 2e-6;
/** A drawn bearing comes back through six decimals. */
const HEADING_TOLERANCE_DEG = 1e-3;
/**
 * Six decimals on the tape and on both depths, plus a tape rounded up by up to
 * two micrometres more: a run can move by `sqrt(h^2 + slack) - h` (the fuzz-gen
 * `tml_roundtrip` oracle's allowance).
 */
const RUN_SLACK_PER_M = 8e-6;
/** Above this many one-per-section choices the mean is checked by its range. */
const MAX_MEAN_CHOICES = 4096;

const close = (a: number, b: number): boolean => Math.abs(a - b) <= DEPTH_TOLERANCE_M;

function horizontalRun(length: number, change: number): number {
  return Math.sqrt(Math.max(length * length - change * change, 0));
}

function headingGap(a: number, b: number): number {
  const d = (((a - b) % 360) + 360) % 360;
  return Math.min(d, 360 - d);
}

function distinct(values: readonly number[]): number[] {
  const out: number[] = [];
  for (const v of values) if (!out.some((o) => close(o, v))) out.push(v);
  return out;
}

class Nodes {
  private readonly parent = new Map<string, string>();

  has(key: string): boolean {
    return this.parent.has(key);
  }

  find(key: string): string {
    if (!this.parent.has(key)) this.parent.set(key, key);
    let root = key;
    while (this.parent.get(root) !== root) root = this.parent.get(root) as string;
    let at = key;
    while (at !== root) {
      const up = this.parent.get(at) as string;
      this.parent.set(at, root);
      at = up;
    }
    return root;
  }

  union(a: string, b: string): void {
    const ra = this.find(a);
    const rb = this.find(b);
    if (ra !== rb) this.parent.set(ra, rb);
  }
}

const beforeStation = (section: number, index: number): string => `b:${section}:${index}`;
const afterStation = (section: number, index: number): string => `a:${section}:${index}`;

/** One end of a matched shot: what came back, and whose reading it is. */
interface Endpoint {
  section: number;
  value: number;
  shot: string;
}

/** Whether `x` is the mean of some one-per-section choice from `options`. */
function isOnePerSectionMean(options: readonly (readonly number[])[], x: number): boolean {
  const choices = options.reduce((n, o) => n * o.length, 1);
  if (choices > MAX_MEAN_CHOICES) {
    const low = options.reduce((s, o) => s + Math.min(...o), 0) / options.length;
    const high = options.reduce((s, o) => s + Math.max(...o), 0) / options.length;
    return x >= low - DEPTH_TOLERANCE_M && x <= high + DEPTH_TOLERANCE_M;
  }
  const walk = (at: number, sum: number): boolean => {
    if (at === options.length) return close(sum / options.length, x);
    return (options[at] as readonly number[]).some((r) => walk(at + 1, sum + r));
  };
  return walk(0, 0);
}

function describeShot(shot: StationedShot): string {
  return `shot ${shot.uuid ?? '(no uuid)'} (section ${shot.section} #${shot.index})`;
}

/**
 * Check one node: every end at its own section's reading (and the readings of
 * the sections there can agree), or every end at one mean of one reading per
 * section. Returns whether it was averaged, or the failure.
 */
function checkNode(
  readings: ReadonlyMap<number, number[]>,
  ends: readonly Endpoint[],
): { averaged: boolean } | { failure: string } {
  const own = new Map<number, number[]>();
  for (const [section, values] of readings) own.set(section, distinct(values));
  const options = [...own.values()];
  const first = options[0] ?? [];
  const canAgree = first.some((r) => options.every((o) => o.some((v) => close(v, r))));
  const allTrue = ends.every((e) => (own.get(e.section) ?? []).some((r) => close(r, e.value)));
  if (allTrue && (own.size === 1 || canAgree)) return { averaged: false };
  const x = (ends[0] as Endpoint).value;
  if (own.size >= 2 && ends.every((e) => close(e.value, x)) && isOnePerSectionMean(options, x)) {
    return { averaged: true };
  }
  const shown = [...own].map(([section, values]) => `section ${section}: ${values.join('/')}`).join(', ');
  const back = ends.map((e) => `${e.shot}=${e.value}`).join(', ');
  return {
    failure:
      `a station came back at ${back}; its readings are ${shown}. Only a junction ` +
      `whose readings disagree may move, to their mean with one reading per section`,
  };
}

/**
 * Compare the original shots with the re-imported project under the junction
 * rule. `before` are the original shots AS EXPORTED (bearings already rotated
 * where the export rotates them, `asExportedToTml`); `after` the re-imported
 * shots and the re-imported project's station merges.
 */
export function junctionDepthReport(
  before: readonly StationedShot[],
  after: { shots: readonly StationedShot[]; merges: readonly StationMergeWire[] },
): JunctionDepthReport {
  const failures: string[] = [];
  const byUuid = new Map<string, StationedShot | null>();
  for (const shot of after.shots) {
    if (!shot.uuid) continue;
    byUuid.set(shot.uuid, byUuid.has(shot.uuid) ? null : shot);
  }
  const nodes = new Nodes();
  for (const [sa, ia, sb, ib] of after.merges) nodes.union(afterStation(sa, ia), afterStation(sb, ib));

  interface Match {
    shot: StationedShot;
    back: StationedShot;
    /** The re-imported depths, turned to the original's direction. */
    depthIn: number;
    depthOut: number;
    reversed: boolean;
  }
  const matches: Match[] = [];
  let unmatched = 0;
  for (const shot of before) {
    const back = shot.uuid ? byUuid.get(shot.uuid) : undefined;
    if (!back) {
      unmatched += 1;
      continue;
    }
    // The export writes some legs backwards (heading through 180, depths
    // swapped), and the import reads them as written.
    const gap = headingGap(back.azimuthIn, shot.azimuthIn);
    const reversed = gap > 90;
    const off = reversed ? 180 - gap : gap;
    if (off > HEADING_TOLERANCE_DEG) {
      failures.push(`${describeShot(shot)}: heading ${shot.azimuthIn} came back as ${back.azimuthIn}`);
    }
    const startAfter = afterStation(back.section, back.index + (reversed ? 1 : 0));
    const endAfter = afterStation(back.section, back.index + (reversed ? 0 : 1));
    nodes.union(beforeStation(shot.section, shot.index), startAfter);
    nodes.union(beforeStation(shot.section, shot.index + 1), endAfter);
    matches.push({
      shot,
      back,
      depthIn: reversed ? back.depthOut : back.depthIn,
      depthOut: reversed ? back.depthIn : back.depthOut,
      reversed,
    });
  }

  // Every reading of every station a matched shot touches, by node and by
  // section - unmatched shots' readings included, they are still readings.
  const readings = new Map<string, Map<number, number[]>>();
  const addReading = (station: string, section: number, value: number): void => {
    if (!nodes.has(station)) return;
    const node = nodes.find(station);
    const bySection = readings.get(node) ?? new Map<number, number[]>();
    readings.set(node, bySection);
    bySection.set(section, [...(bySection.get(section) ?? []), value]);
  };
  for (const shot of before) {
    addReading(beforeStation(shot.section, shot.index), shot.section, shot.depthIn);
    addReading(beforeStation(shot.section, shot.index + 1), shot.section, shot.depthOut);
  }
  const ends = new Map<string, Endpoint[]>();
  for (const m of matches) {
    const label = m.shot.uuid ?? '';
    for (const [station, value] of [
      [beforeStation(m.shot.section, m.shot.index), m.depthIn],
      [beforeStation(m.shot.section, m.shot.index + 1), m.depthOut],
    ] as const) {
      const node = nodes.find(station);
      ends.set(node, [...(ends.get(node) ?? []), { section: m.shot.section, value, shot: label }]);
    }
  }

  let averagedJunctions = 0;
  for (const [node, nodeEnds] of ends) {
    const verdict = checkNode(readings.get(node) ?? new Map(), nodeEnds);
    if ('failure' in verdict) failures.push(verdict.failure);
    else if (verdict.averaged) averagedJunctions += 1;
  }

  let refittedShots = 0;
  let brokenShots = 0;
  for (const shot of before) {
    if (Math.abs(shot.depthOut - shot.depthIn) > shot.length + 1e-9) brokenShots += 1;
  }
  for (const m of matches) {
    const { shot, back } = m;
    if (Math.abs(back.length - shot.length) > BOUND_TOLERANCE_M) refittedShots += 1;
    const change = m.depthOut - m.depthIn;
    const bound = back.length + BOUND_TOLERANCE_M;
    if (Math.abs(change) > bound) {
      // A kept explicit second reading is bound against the node it hangs off
      // (the file's parent `Depth`), not against its own start.
      const hangsOff = nodes.find(beforeStation(shot.section, shot.index + (m.reversed ? 1 : 0)));
      const fileEnd = m.reversed ? m.depthIn : m.depthOut;
      const parents = (ends.get(hangsOff) ?? []).map((e) => e.value);
      if (!parents.some((p) => Math.abs(fileEnd - p) <= bound)) {
        failures.push(
          `${describeShot(shot)}: drops ${change} m over a length of ${back.length} m - ` +
            `the length bound does not hold`,
        );
      }
    }
    const run = horizontalRun(shot.length, shot.depthOut - shot.depthIn);
    const runBack = horizontalRun(back.length, change);
    const slack = (back.length + Math.abs(change)) * RUN_SLACK_PER_M;
    const allowed = Math.sqrt(run * run + slack) - run + 1e-6;
    if (Math.abs(runBack - run) > allowed) {
      failures.push(
        `${describeShot(shot)}: horizontal run ${run} m came back as ${runBack} m ` +
          `(allowed ${allowed}) - the plan moved`,
      );
    }
  }

  return { matched: matches.length, unmatched, averagedJunctions, refittedShots, brokenShots, failures };
}

/** Throw the report's failures, if any, as one oracle verdict. */
export function checkJunctionDepths(report: JunctionDepthReport, reproduce: string): void {
  if (report.failures.length === 0) return;
  const lines = [
    `X6c the depths, tapes or plan changed beyond the junction rule (docs/ariane-export.md, ` +
      `"Junction depths")`,
    `    ${report.matched} shot(s) matched by uuid, ${report.failures.length} failure(s):`,
    ...report.failures.slice(0, 8).map((f) => `      ${f}`),
  ];
  if (report.failures.length > 8) lines.push(`      ... and ${report.failures.length - 8} more`);
  lines.push(`  ${reproduce}`);
  throw new ExportOracleError(lines.join('\n'));
}
