/**
 * Random, valid-by-construction mutating actions for the `undo-redo-fuzz`
 * scenario. Every generator draws ONLY from `ctx.rng()`/`randInt`/`pick` and
 * targets ids read back out of the live project, so a (seed, iterations) pair
 * replays exactly.
 *
 * Arg shapes mirror the generated IPC types in `src/types/ipc/` (SectionData,
 * ShotData, ShotUpdates, SectionUpdates, StationRef, GpsCoordinates,
 * Coordinates, ClosureStrategy) and the `projectStore` action signatures. They
 * are restated structurally rather than imported: nothing else under `e2e/`
 * reaches into `src/`, and `tsconfig.agent.json` deliberately roots only the
 * harness tree.
 *
 * `ShotUpdates` uses serde's double-option for the nullable fields: an ABSENT
 * key means "leave it alone", an explicit `null` means "clear it". Patches here
 * are built by adding only the keys being updated, so the distinction survives
 * the JSON round trip through WebDriver by construction - we never write
 * `undefined` and rely on `JSON.stringify` dropping it.
 */
import { mulberry32, type ScenarioContext } from './framework';
import { IMPORT_ACTION_GENERATORS } from './undoRedoFuzzImportActions';

/**
 * Generator STREAM VERSION.
 *
 * v2 (default) is the full action space: the delete/split family, batches,
 * comment moves, synthetic imports, boundary-biased values and per-run swarm
 * weights. It necessarily draws differently from v1, so v1 repro lines do not
 * replay under it - the scenario header carries a v2 repro for every documented
 * bug.
 *
 * v3 is v2 PLUS the import group (`undoRedoFuzzImportActions.ts`), opt-in via
 * `STALAGMIGHT_FUZZ_STREAM=v3`. See `streamV3` below.
 *
 * `STALAGMIGHT_FUZZ_STREAM=v1` restores the ORIGINAL generator set, weights and
 * value ranges exactly: the v2-only helpers below all short-circuit without
 * consuming a draw, so a v1 run reproduces the historical stream byte for byte.
 * It is the pinned escape for any repro that cannot be re-found under v2.
 *
 * Read at CALL time, never cached: `pnpm agent:fuzz-shrink` restores the env the
 * recording ran under from inside its own `main()`, which is after this module
 * was imported, and a frozen const would label a v1 replay's trace and repro
 * line as v2. Same rule as the knobs in `undoRedoFuzz.ts`.
 */
export function streamV2(): boolean {
  return process.env.STALAGMIGHT_FUZZ_STREAM !== 'v1';
}

/**
 * v3 = v2 PLUS the import group in `undoRedoFuzzImportActions.ts`.
 *
 * It is a strict SUPERSET of v2 rather than a replacement, and that is a choice
 * worth stating because the v1/v2 relationship is not the same shape: v1 is a
 * SUBSET of v2 (v2 added generators to it and moved every trajectory), while
 * `STALAGMIGHT_FUZZ_STREAM=v3` leaves `streamV2()` true - so every v2 generator,
 * weight and value range is present and unaltered under v3. What changes is only
 * that fifteen more names join the weighted array, at the END, so the array a
 * v1 or v2 run builds is byte-identical to what it was before v3 existed.
 *
 * v3 is OPT-IN, not the default. A v3 run necessarily draws differently from a
 * v2 run of the same seed (a longer weighted array re-partitions every `pick`),
 * so making it the default would invalidate every repro line in this scenario's
 * header at a stroke - the same reason v2 kept a v1 escape hatch instead of
 * rewriting the lines.
 *
 * Read at CALL time, never cached, for exactly the reason `streamV2` is.
 */
export function streamV3(): boolean {
  return process.env.STALAGMIGHT_FUZZ_STREAM === 'v3';
}

/** Share of generated values that take an extreme instead of a nominal value. */
const P_BOUNDARY = 0.15;

/**
 * Largest magnitude a boundary value that FEEDS COORDINATE ARITHMETIC may take.
 *
 * Not arbitrary - measured. A station move of 1e6 m puts coordinates in a regime
 * where re-deriving `section_offsets` loses about 4.6e-12 RELATIVE precision
 * (roughly 2e4 ULPs through the summation and trig chain), which is 4.6e-6 m
 * ABSOLUTE. That is 3 orders above the oracle's 1e-9 floor, so it reads as a
 * roundtrip failure when it is really catastrophic cancellation in the injected
 * input. The scaling was verified at the same step of the same seed
 * (fractal seed 12 x40, `undo` at depth 3, `project.section_offsets.8[0]`):
 *
 *     injected z    drift
 *     1e6           4.56e-6 m
 *     1e5           1.59e-7 m
 *     1e4           1.7e-9 m
 *     1e3           below the 1e-9 tolerance
 *
 * i.e. absolute error tracks the injected magnitude, the signature of float
 * cancellation rather than of a logic error - and the shot fields themselves
 * roundtrip EXACTLY (the failure had six differing paths, every one of them
 * derived geometry).
 *
 * So the bound goes on the INPUT, not on the oracle: a kilometre-scale
 * displacement is already far beyond any real survey and is a genuine boundary,
 * while keeping derived geometry inside the existing tolerance. Loosening the
 * oracle instead would have blinded it to real sub-centimetre drift everywhere.
 */
const GEOMETRY_EXTREME = 1e3;

/** Mirrors `src/types/ipc/StationRef.ts` - the arg shape the store actions take. */
export interface StationRef {
  section_id: number;
  station_index: number;
}

/**
 * Generators read a `TargetingDigest` (`e2e/agent/lib/fuzzOracleCore.ts`), not
 * the raw project state: the digest is built IN THE WEBVIEW and is ~20x smaller
 * than the payload it summarizes, which is what makes one round trip per step
 * affordable.
 *
 * It carries exactly the fields read below, each list in payload order, so every
 * sort and every `ctx.pick` sees the same values in the same sequence it saw
 * when this read the raw state - the rng draw sequence, and therefore every
 * documented repro seed, is unchanged.
 */
import type { TargetingDigest } from '../lib/fuzzOracleCore';
import type { GeneratedFileSpec } from '../lib/surveyFileSpecs';
export type { TargetingDigest };

/** `[sectionId, [shotId, ...]]`, one entry of the digest's section list. */
type DigestSection = [number, number[]];

/** One drawn mutation: a `projectStore` action plus its args. */
export interface FuzzAction {
  action: string;
  args: unknown[];
  /** Short label for the action-history tail in a failure message. */
  label: string;
  /**
   * An input FILE this action needs on disk (v3 import generators only).
   *
   * A generator is PURE - the shrinker re-derives steps from a trace and the
   * draw loop discards candidates - so it never writes anything. It hands over
   * the spec and the session-relative path it put in `args[0]`, and
   * `undoRedoFuzz.ts` materializes it immediately before dispatch, records a
   * `genFile` trace step carrying the SPEC (never the bytes, never an absolute
   * path), and swaps `args[0]` for the absolute path for this process only.
   * See the header of `undoRedoFuzzImportActions.ts`.
   */
  file?: { spec: GeneratedFileSpec; rel: string };
}

export interface ActionGenerator {
  name: string;
  /** Relative draw weight. */
  weight: number;
  /**
   * Stream version this generator belongs to. A v1 run draws only from 'v1'; a
   * v2 run from 'v1' + 'v2'; a v3 run from all three.
   */
  since?: 'v1' | 'v2' | 'v3';
  /** Returns null when the live project has no valid target for this action. */
  build(ctx: ScenarioContext, state: TargetingDigest): FuzzAction | null;
}

export const COLORS = ['#3388FF', '#FF8833', '#33CC66', '#CC33AA', '#FFCC00', '#6666FF'] as const;
export const SHOT_KINDS = ['Normal', 'Jump', 'Reference'] as const;
export const SURVEY_DIRECTIONS = ['in', 'out'] as const;
/**
 * `Batch` is the deferred-distribution marker and `ProportionalByLength` /
 * `EqualDistribution` are rejected outright by `create_loop_closure`
 * (models/project/mutations.rs), so they are drawn only occasionally - a
 * rejected command is a case the model checks, just not an interesting one to
 * spend most draws on.
 */
const CLOSURE_STRATEGIES = [
  'Move',
  'Move',
  'LeastSquaresWeighted',
  'LeastSquaresWeighted',
  'ProportionalByLength',
  'EqualDistribution',
  'Batch',
] as const;

export function chance(ctx: ScenarioContext, probability: number): boolean {
  return ctx.rng() < probability;
}

export function roundedFloat(ctx: ScenarioContext, min: number, max: number, decimals: number): number {
  return Number((min + ctx.rng() * (max - min)).toFixed(decimals));
}

/**
 * Values that have historically been where the bugs are: exact zero, negatives,
 * very large and very small magnitudes. Bug 3 was literally "append the first
 * shot into an EMPTY section", so the shapes at the edges of a range get
 * deliberate weight rather than being left to a uniform draw to stumble on.
 *
 * In v1 this is `roundedFloat` and consumes exactly one draw, so the legacy
 * stream is unchanged.
 */
function biasedFloat(
  ctx: ScenarioContext,
  min: number,
  max: number,
  decimals: number,
  extremes: readonly number[] = [0, -0.0001, GEOMETRY_EXTREME, -GEOMETRY_EXTREME, 1e-9],
): number {
  if (!streamV2()) return roundedFloat(ctx, min, max, decimals);
  if (ctx.rng() < P_BOUNDARY) return ctx.pick(extremes);
  return roundedFloat(ctx, min, max, decimals);
}

/** Strings that exercise encoding and emptiness, not just ASCII words. */
const BOUNDARY_STRINGS = [
  '',
  ' ',
  'Cueva del Rio \u00d1andu\u0301',
  '\u6d1e\u7a74\u6e2c\u91cf',
  '\ud83d\udd26\ud83e\udd3f',
  'x'.repeat(512),
  '"quoted", \\backslash\\ and \nnewline',
] as const;

export function biasedText(ctx: ScenarioContext, nominal: string): string {
  if (!streamV2()) return nominal;
  if (ctx.rng() < P_BOUNDARY) return ctx.pick(BOUNDARY_STRINGS);
  return nominal;
}

/** `count` distinct members of `items`, drawn deterministically without replacement. */
function pickDistinct<T>(ctx: ScenarioContext, items: readonly T[], count: number): T[] {
  const pool = [...items];
  const out: T[] = [];
  while (out.length < count && pool.length > 0) {
    const [taken] = pool.splice(ctx.randInt(0, pool.length - 1), 1);
    if (taken !== undefined) out.push(taken);
  }
  return out;
}

/** Sections by ascending id, so the draw does not depend on Vec order. */
export function sortedSections(state: TargetingDigest): DigestSection[] {
  return [...state.s].sort((a, b) => a[0] - b[0]);
}

export function pickSection(ctx: ScenarioContext, state: TargetingDigest): DigestSection | null {
  const sections = sortedSections(state);
  return sections.length > 0 ? ctx.pick(sections) : null;
}

/**
 * Like `pickSection`, but biased toward EMPTY and single-shot sections. Bug 3 is
 * "append the first shot into an empty section", and a uniform pick over 30+
 * sections reaches that shape rarely. v2 only.
 */
function pickSmallSection(ctx: ScenarioContext, state: TargetingDigest): DigestSection | null {
  if (!streamV2()) return pickSection(ctx, state);
  const sections = sortedSections(state);
  if (sections.length === 0) return null;
  const small = sections.filter((section) => section[1].length <= 1);
  if (small.length > 0 && ctx.rng() < 0.5) return ctx.pick(small);
  return ctx.pick(sections);
}

export function pickShot(
  ctx: ScenarioContext,
  state: TargetingDigest,
): { sectionId: number; shotId: number } | null {
  const populated = sortedSections(state).filter((section) => section[1].length > 0);
  if (populated.length === 0) return null;
  const section = ctx.pick(populated);
  const shotIds = [...section[1]].sort((a, b) => a - b);
  return { sectionId: section[0], shotId: ctx.pick(shotIds) };
}

export function sortedStationRefs(state: TargetingDigest): [number, number][] {
  return [...state.st].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
}

export function pickStationRef(ctx: ScenarioContext, state: TargetingDigest): [number, number] | null {
  const refs = sortedStationRefs(state);
  return refs.length > 0 ? ctx.pick(refs) : null;
}

/**
 * Two stations in DIFFERENT sections. Merging or closing a loop within one
 * section is rejected up front (`command_station.rs`), so cross-section pairs
 * are the ones that can actually exercise the undo path.
 */
function pickCrossSectionPair(
  ctx: ScenarioContext,
  state: TargetingDigest,
): [[number, number], [number, number]] | null {
  const bySection = new Map<number, [number, number][]>();
  for (const ref of sortedStationRefs(state)) {
    const bucket = bySection.get(ref[0]);
    if (bucket) bucket.push(ref);
    else bySection.set(ref[0], [ref]);
  }
  const sectionIds = [...bySection.keys()].sort((a, b) => a - b);
  if (sectionIds.length < 2) return null;
  const [idA, idB] = pickDistinct(ctx, sectionIds, 2);
  if (idA === undefined || idB === undefined) return null;
  const a = ctx.pick(bySection.get(idA) ?? []);
  const b = ctx.pick(bySection.get(idB) ?? []);
  return a && b ? [a, b] : null;
}

/** Digest tuple -> the `StationRef` shape the store actions and IPC expect. */
export function toRef(ref: [number, number]): StationRef {
  return { section_id: ref[0], station_index: ref[1] };
}

export function refLabel(ref: [number, number]): string {
  return `${ref[0]}/${ref[1]}`;
}

/** A valid `ShotData` (every field present; nullable ones explicitly null-or-value). */
export function randomShotData(ctx: ScenarioContext): Record<string, unknown> {
  const azimuth = roundedFloat(ctx, 0, 359.9, 1);
  const depthIn = roundedFloat(ctx, 0, 40, 2);
  const depthOut = Math.max(0, depthIn + roundedFloat(ctx, -3, 3, 2));
  return {
    azimuth_in: azimuth,
    azimuth_out: Number(((azimuth + roundedFloat(ctx, -1.5, 1.5, 1) + 360) % 360).toFixed(1)),
    length: biasedFloat(ctx, 1, 12, 2, [0, 1e-9, GEOMETRY_EXTREME, -5, 0.0001]),
    depth_in: depthIn,
    depth_out: Number(depthOut.toFixed(2)),
    pitch_in: chance(ctx, 0.5) ? roundedFloat(ctx, -60, 60, 1) : null,
    pitch_out: chance(ctx, 0.5) ? roundedFloat(ctx, -60, 60, 1) : null,
    left: chance(ctx, 0.7) ? roundedFloat(ctx, 0, 5, 2) : null,
    right: chance(ctx, 0.7) ? roundedFloat(ctx, 0, 5, 2) : null,
    up: chance(ctx, 0.7) ? roundedFloat(ctx, 0, 5, 2) : null,
    down: chance(ctx, 0.7) ? roundedFloat(ctx, 0, 5, 2) : null,
    comment: chance(ctx, 0.25) ? biasedText(ctx, `fuzz-shot-${ctx.randInt(0, 9999)}`) : null,
    kind: ctx.pick(SHOT_KINDS),
    marker_index: chance(ctx, 0.15) ? ctx.randInt(0, 9) : null,
    temperature: chance(ctx, 0.3) ? roundedFloat(ctx, 4, 30, 1) : null,
  };
}

/** A valid `SectionData` with 1-4 shots. */
export function randomSectionData(ctx: ScenarioContext): Record<string, unknown> {
  const shotCount = ctx.randInt(1, 4);
  const shots: Record<string, unknown>[] = [];
  for (let i = 0; i < shotCount; i++) shots.push(randomShotData(ctx));
  return {
    name: biasedText(ctx, `fuzz-${ctx.randInt(1000, 9999)}`),
    surveyor: chance(ctx, 0.7) ? biasedText(ctx, `fuzz-surveyor-${ctx.randInt(1, 9)}`) : null,
    date: `20${ctx.randInt(10, 26)}-0${ctx.randInt(1, 9)}-1${ctx.randInt(0, 9)}`,
    color: ctx.pick(COLORS),
    direction: ctx.pick(SURVEY_DIRECTIONS),
    quality: ctx.randInt(1, 5),
    shots,
  };
}

interface PatchField {
  key: string;
  /** May be set to explicit null ("clear this field"). */
  nullable: boolean;
  gen(ctx: ScenarioContext): unknown;
}

const SHOT_PATCH_FIELDS: readonly PatchField[] = [
  { key: 'azimuth_in', nullable: false, gen: (c) => roundedFloat(c, 0, 359.9, 1) },
  { key: 'azimuth_out', nullable: false, gen: (c) => roundedFloat(c, 0, 359.9, 1) },
  { key: 'length', nullable: false, gen: (c) => biasedFloat(c, 0.5, 20, 2, [0, 1e-9, GEOMETRY_EXTREME, -3]) },
  { key: 'depth_in', nullable: false, gen: (c) => roundedFloat(c, 0, 40, 2) },
  { key: 'depth_out', nullable: false, gen: (c) => roundedFloat(c, 0, 40, 2) },
  { key: 'pitch_in', nullable: true, gen: (c) => roundedFloat(c, -60, 60, 1) },
  { key: 'pitch_out', nullable: true, gen: (c) => roundedFloat(c, -60, 60, 1) },
  { key: 'left', nullable: true, gen: (c) => roundedFloat(c, 0, 5, 2) },
  { key: 'right', nullable: true, gen: (c) => roundedFloat(c, 0, 5, 2) },
  { key: 'up', nullable: true, gen: (c) => roundedFloat(c, 0, 5, 2) },
  { key: 'down', nullable: true, gen: (c) => roundedFloat(c, 0, 5, 2) },
  { key: 'comment', nullable: true, gen: (c) => biasedText(c, `fuzz-comment-${c.randInt(0, 9999)}`) },
  { key: 'kind', nullable: false, gen: (c) => c.pick(SHOT_KINDS) },
  { key: 'marker_index', nullable: true, gen: (c) => c.randInt(0, 9) },
  { key: 'temperature', nullable: true, gen: (c) => roundedFloat(c, 4, 30, 1) },
];

const SECTION_PATCH_FIELDS: readonly PatchField[] = [
  { key: 'name', nullable: true, gen: (c) => biasedText(c, `fuzz-renamed-${c.randInt(0, 9999)}`) },
  { key: 'surveyor', nullable: true, gen: (c) => biasedText(c, `fuzz-surveyor-${c.randInt(1, 9)}`) },
  {
    key: 'date',
    nullable: true,
    gen: (c) => `20${c.randInt(10, 26)}-0${c.randInt(1, 9)}-1${c.randInt(0, 9)}`,
  },
  { key: 'color', nullable: false, gen: (c) => c.pick(COLORS) },
  { key: 'direction', nullable: false, gen: (c) => c.pick(SURVEY_DIRECTIONS) },
  { key: 'quality', nullable: false, gen: (c) => c.randInt(1, 5) },
];

/**
 * Build a partial update. Only the drawn keys are present; a nullable key is
 * set to explicit `null` ~30% of the time to exercise serde's "clear" case
 * alongside the "absent = no change" one.
 */
function buildPatch(
  ctx: ScenarioContext,
  fields: readonly PatchField[],
  count: number,
): { patch: Record<string, unknown>; label: string } {
  const patch: Record<string, unknown> = {};
  const parts: string[] = [];
  for (const field of pickDistinct(ctx, fields, count).sort((a, b) => a.key.localeCompare(b.key))) {
    if (field.nullable && chance(ctx, 0.3)) {
      patch[field.key] = null;
      parts.push(`${field.key}=null`);
    } else {
      patch[field.key] = field.gen(ctx);
      parts.push(field.key);
    }
  }
  return { patch, label: parts.join(',') };
}

function randomOffset(ctx: ScenarioContext): Record<string, number> {
  return {
    x: biasedFloat(ctx, -3, 3, 3, [0, GEOMETRY_EXTREME, -GEOMETRY_EXTREME, 1e-9]),
    y: biasedFloat(ctx, -3, 3, 3, [0, GEOMETRY_EXTREME, -GEOMETRY_EXTREME, 1e-9]),
    z: biasedFloat(ctx, -1.5, 1.5, 3, [0, GEOMETRY_EXTREME, -GEOMETRY_EXTREME, 1e-9]),
  };
}


/** A contiguous shot range within one section, for the range-delete family. */
function pickShotRange(
  ctx: ScenarioContext,
  state: TargetingDigest,
): { sectionId: number; startShotId: number; endShotId: number } | null {
  const populated = sortedSections(state).filter((section) => section[1].length >= 2);
  if (populated.length === 0) return null;
  const section = ctx.pick(populated);
  const shotIds = [...section[1]].sort((a, b) => a - b);
  const first = ctx.randInt(0, shotIds.length - 1);
  const second = ctx.randInt(0, shotIds.length - 1);
  const lo = Math.min(first, second);
  const hi = Math.max(first, second);
  return {
    sectionId: section[0],
    startShotId: shotIds[lo] as number,
    endShotId: shotIds[hi] as number,
  };
}

/** A `CommentHolder`: either a station or a shot (src/types/ipc/CommentHolder.ts). */
function pickCommentHolder(
  ctx: ScenarioContext,
  state: TargetingDigest,
): { holder: Record<string, unknown>; label: string } | null {
  if (chance(ctx, 0.5)) {
    const ref = pickStationRef(ctx, state);
    if (!ref) return null;
    return { holder: { type: 'station', station_ref: toRef(ref) }, label: `st:${refLabel(ref)}` };
  }
  const target = pickShot(ctx, state);
  if (!target) return null;
  return {
    holder: { type: 'shot', section_id: target.sectionId, shot_id: target.shotId },
    label: `sh:${target.sectionId}/${target.shotId}`,
  };
}

/**
 * 2-4 nested `BatchCommand`s (src/types/commands.ts). A batch is ONE undo entry
 * covering all of them, with all-or-nothing rollback, so it is the only way to
 * reach `execute_batch`'s snapshot-pair path and its rollback guard.
 *
 * With `P_BATCH_POISON` the LAST sub-command is deliberately made invalid, which
 * is the case the rollback exists for: the batch must fail, leave state
 * untouched, and push nothing.
 */
const P_BATCH_POISON = 0.25;

export function buildBatch(
  ctx: ScenarioContext,
  state: TargetingDigest,
): { commands: Record<string, unknown>[]; label: string } | null {
  const count = ctx.randInt(2, 4);
  const commands: Record<string, unknown>[] = [];
  const parts: string[] = [];
  for (let i = 0; i < count; i++) {
    const built = buildBatchCommand(ctx, state);
    if (!built) continue;
    commands.push(built.command);
    parts.push(built.label);
  }
  if (commands.length === 0) return null;
  if (chance(ctx, P_BATCH_POISON)) {
    // A move_section against a section id that cannot exist: the sub-command
    // fails, so execute_batch must roll the whole batch back.
    commands.push({ type: 'move_section', section_id: 999999, offset: { x: 1, y: 1, z: 1 } });
    parts.push('POISON');
  }
  return { commands, label: parts.join('+') };
}

/** One nested command for a batch. Shapes mirror the `BatchCommand` union. */
function buildBatchCommand(
  ctx: ScenarioContext,
  state: TargetingDigest,
): { command: Record<string, unknown>; label: string } | null {
  const kind = ctx.pick([
    'update_shot',
    'move_station',
    'move_section',
    'reverse_section',
    'create_section',
    'merge_stations',
  ] as const);
  if (kind === 'update_shot') {
    const target = pickShot(ctx, state);
    if (!target) return null;
    const { patch } = buildPatch(ctx, SHOT_PATCH_FIELDS, ctx.randInt(1, 3));
    return {
      command: { type: 'update_shot', section_id: target.sectionId, shot_id: target.shotId, updates: patch },
      label: 'updShot',
    };
  }
  if (kind === 'move_station') {
    const ref = pickStationRef(ctx, state);
    if (!ref) return null;
    return { command: { type: 'move_station', station_ref: toRef(ref), offset: randomOffset(ctx) }, label: 'mvSt' };
  }
  if (kind === 'move_section') {
    const section = pickSection(ctx, state);
    if (!section) return null;
    return { command: { type: 'move_section', section_id: section[0], offset: randomOffset(ctx) }, label: 'mvSec' };
  }
  if (kind === 'reverse_section') {
    const section = pickSection(ctx, state);
    if (!section) return null;
    return {
      command: { type: 'reverse_section', section_id: section[0], swap_left_right: chance(ctx, 0.5) },
      label: 'revSec',
    };
  }
  if (kind === 'merge_stations') {
    const pair = pickCrossSectionPair(ctx, state);
    if (!pair) return null;
    return {
      command: { type: 'merge_stations', station_a: toRef(pair[0]), station_b: toRef(pair[1]) },
      label: 'merge',
    };
  }
  const data = randomSectionData(ctx);
  return {
    command: {
      type: 'create_section',
      name: data.name,
      surveyor: data.surveyor,
      date: data.date,
      color: data.color,
      direction: data.direction,
      shots: data.shots,
    },
    label: 'crtSec',
  };
}

/**
 * Per-run SWARM PROFILE: a deterministic weight multiplier per generator, drawn
 * from its own stream so it cannot disturb the primary sequence.
 *
 * A single uniform weighting explores one shape of history. Skewing each run -
 * merge-heavy, delete-heavy, batch-heavy - reaches states a balanced run needs
 * far more iterations to stumble into, while staying reproducible from the seed.
 */
const SWARM_STREAM_SALT = 0xc2b2ae35;

export interface SwarmProfile {
  name: string;
  multipliers: Record<string, number>;
}

const SWARM_PROFILES: readonly { name: string; boost: Record<string, number> }[] = [
  { name: 'balanced', boost: {} },
  {
    name: 'merge-heavy',
    boost: { mergeStations: 5, unmergeStationPair: 4, createLoopClosure: 4, removeLoopClosure: 3, setZeroDatum: 3 },
  },
  {
    name: 'delete-heavy',
    boost: {
      deleteShot: 4,
      deleteSection: 4,
      deleteShotAndSplit: 5,
      deleteShotRangeSplit: 5,
      deleteShotRangeShift: 5,
      deleteLastShotAndSection: 5,
    },
  },
  { name: 'batch-heavy', boost: { executeBatch: 8, importSections: 4, createSectionAndMerge: 4 } },
  { name: 'split-heavy', boost: { splitShotAtDistance: 8, deleteShotAndSplit: 4, createShot: 3 } },
  { name: 'move-heavy', boost: { moveStation: 5, moveSection: 5, moveSegment: 5, moveComment: 4, reverseSection: 3 } },
  { name: 'metadata-heavy', boost: { updateShot: 4, updateSection: 4, updateStationComment: 5, setShotKind: 4, moveComment: 3 } },
];

/**
 * The v3-only profile, appended rather than merged into the list above.
 *
 * Merging it would change `SWARM_PROFILES.length`, and the profile a seed
 * selects is `floor(rng() * length)` - so every v2 seed would have silently
 * changed profile and every v2 repro line with it. Appending under the v3 gate
 * leaves the v2 selection arithmetic untouched and gives a v3 campaign one run
 * in eight where the import group dominates.
 */
const IMPORT_HEAVY_PROFILE: { name: string; boost: Record<string, number> } = {
  name: 'import-heavy',
  boost: {
    importSectionsSynthetic: 4,
    importSectionsWithOverlap: 4,
    importSectionsNonFinite: 3,
    importTmlFile: 2,
    importCompassFile: 2,
    importWallsFile: 2,
    importGpxFile: 2,
  },
};

/** The profile a seed selects. Stable for a seed, independent of every other stream. */
export function swarmProfileFor(seed: number): SwarmProfile {
  if (!streamV2()) return { name: 'v1-legacy', multipliers: {} };
  const profiles = streamV3() ? [...SWARM_PROFILES, IMPORT_HEAVY_PROFILE] : SWARM_PROFILES;
  const rng = mulberry32((seed ^ SWARM_STREAM_SALT) >>> 0);
  const chosen = profiles[Math.floor(rng() * profiles.length)] ?? profiles[0];
  return { name: chosen?.name ?? 'balanced', multipliers: { ...(chosen?.boost ?? {}) } };
}

/**
 * The generator set. Weights keep the cheap structural edits dominant while
 * still reaching the graph operations (merge / closure / datum), which is where
 * the interesting undo bugs live. Deletes are held down so a long run does not
 * empty the project out.
 */
export const ACTION_GENERATORS: readonly ActionGenerator[] = [
  {
    name: 'createSection',
    weight: 6,
    build(ctx) {
      const data = randomSectionData(ctx);
      const shots = data.shots as unknown[];
      return { action: 'createSection', args: [data], label: `createSection(shots=${shots.length})` };
    },
  },
  {
    name: 'createShot',
    weight: 12,
    build(ctx, state) {
      const section = pickSection(ctx, state);
      if (!section) return null;
      const shot = randomShotData(ctx);
      const shotIds = [...section[1]].sort((a, b) => a - b);
      // The PRE-insert shot count is in every label: appending into a section
      // that currently has none takes a different backend path from appending
      // onto a populated one, and that distinction has already been the
      // difference between a clean roundtrip and a displaced section.
      const where = `s=${section[0]},n=${shotIds.length}`;
      const mode = shotIds.length === 0 ? 'append' : ctx.pick(['append', 'after', 'before'] as const);
      if (mode === 'after') {
        const afterId = ctx.pick(shotIds);
        return {
          action: 'createShot',
          args: [section[0], shot, afterId],
          label: `createShot(${where},after=${afterId})`,
        };
      }
      if (mode === 'before') {
        const beforeId = ctx.pick(shotIds);
        return {
          action: 'createShot',
          args: [section[0], shot, null, beforeId],
          label: `createShot(${where},before=${beforeId})`,
        };
      }
      return { action: 'createShot', args: [section[0], shot], label: `createShot(${where},append)` };
    },
  },
  {
    name: 'updateShot',
    weight: 14,
    build(ctx, state) {
      const target = pickShot(ctx, state);
      if (!target) return null;
      const { patch, label } = buildPatch(ctx, SHOT_PATCH_FIELDS, ctx.randInt(1, 4));
      return {
        action: 'updateShot',
        args: [target.sectionId, target.shotId, patch],
        label: `updateShot(s=${target.sectionId},shot=${target.shotId},{${label}})`,
      };
    },
  },
  {
    name: 'deleteShot',
    weight: 8,
    build(ctx, state) {
      const target = pickShot(ctx, state);
      if (!target) return null;
      return {
        action: 'deleteShot',
        args: [target.sectionId, target.shotId],
        label: `deleteShot(s=${target.sectionId},shot=${target.shotId})`,
      };
    },
  },
  {
    name: 'deleteSection',
    weight: 3,
    build(ctx, state) {
      // Keep at least two sections so cross-section merges stay reachable.
      if (state.s.length <= 2) return null;
      const section = pickSection(ctx, state);
      if (!section) return null;
      return { action: 'deleteSection', args: [section[0]], label: `deleteSection(s=${section[0]})` };
    },
  },
  {
    name: 'updateSection',
    weight: 8,
    build(ctx, state) {
      const section = pickSection(ctx, state);
      if (!section) return null;
      const { patch, label } = buildPatch(ctx, SECTION_PATCH_FIELDS, ctx.randInt(1, 3));
      return {
        action: 'updateSection',
        args: [section[0], patch],
        label: `updateSection(s=${section[0]},{${label}})`,
      };
    },
  },
  {
    name: 'reverseSection',
    weight: 4,
    build(ctx, state) {
      const section = pickSection(ctx, state);
      if (!section) return null;
      const swap = chance(ctx, 0.5);
      return {
        action: 'reverseSection',
        args: [section[0], swap],
        label: `reverseSection(s=${section[0]},swapLR=${swap})`,
      };
    },
  },
  {
    name: 'setShotKind',
    weight: 8,
    build(ctx, state) {
      const target = pickShot(ctx, state);
      if (!target) return null;
      const kind = ctx.pick(SHOT_KINDS);
      return {
        action: 'setShotKind',
        args: [target.sectionId, target.shotId, kind],
        label: `setShotKind(s=${target.sectionId},shot=${target.shotId},${kind})`,
      };
    },
  },
  {
    name: 'updateStationComment',
    weight: 8,
    build(ctx, state) {
      const ref = pickStationRef(ctx, state);
      if (!ref) return null;
      const comment = chance(ctx, 0.3) ? null : `fuzz-station-${ctx.randInt(0, 9999)}`;
      return {
        action: 'updateStationComment',
        args: [toRef(ref), comment],
        label: `updateStationComment(${refLabel(ref)},${comment === null ? 'null' : 'text'})`,
      };
    },
  },
  {
    name: 'moveStation',
    weight: 8,
    build(ctx, state) {
      const ref = pickStationRef(ctx, state);
      if (!ref) return null;
      // The store types this parameter `newPosition`, but every caller (and the
      // backend `move_station`) treats it as an OFFSET.
      const offset = randomOffset(ctx);
      return {
        action: 'moveStation',
        args: [toRef(ref), offset],
        label: `moveStation(${refLabel(ref)},d=${offset.x}/${offset.y}/${offset.z})`,
      };
    },
  },
  {
    name: 'moveSection',
    weight: 5,
    build(ctx, state) {
      const section = pickSection(ctx, state);
      if (!section) return null;
      const offset = randomOffset(ctx);
      return {
        action: 'moveSection',
        args: [section[0], offset],
        label: `moveSection(s=${section[0]},d=${offset.x}/${offset.y}/${offset.z})`,
      };
    },
  },
  {
    name: 'moveSegment',
    weight: 5,
    build(ctx, state) {
      const section = pickSection(ctx, state);
      if (!section) return null;
      const offset = randomOffset(ctx);
      return {
        action: 'moveSegment',
        args: [section[0], offset],
        label: `moveSegment(s=${section[0]},d=${offset.x}/${offset.y}/${offset.z})`,
      };
    },
  },
  {
    name: 'mergeStations',
    weight: 6,
    build(ctx, state) {
      const pair = pickCrossSectionPair(ctx, state);
      if (!pair) return null;
      return {
        action: 'mergeStations',
        args: [toRef(pair[0]), toRef(pair[1])],
        label: `mergeStations(${refLabel(pair[0])},${refLabel(pair[1])})`,
      };
    },
  },
  {
    name: 'unmergeStationPair',
    weight: 4,
    build(ctx, state) {
      const merges = [...state.m].sort(
        (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2] || a[3] - b[3],
      );
      if (merges.length === 0) return null;
      const merge = ctx.pick(merges);
      const a: [number, number] = [merge[0], merge[1]];
      const b: [number, number] = [merge[2], merge[3]];
      return {
        action: 'unmergeStationPair',
        args: [toRef(a), toRef(b)],
        label: `unmergeStationPair(${refLabel(a)},${refLabel(b)})`,
      };
    },
  },
  {
    name: 'setZeroDatum',
    weight: 3,
    build(ctx, state) {
      const ref = pickStationRef(ctx, state);
      if (!ref) return null;
      const gps = {
        latitude: roundedFloat(ctx, 20.2, 20.5, 6),
        longitude: roundedFloat(ctx, -87.5, -87.2, 6),
        depth_reference: roundedFloat(ctx, 0, 5, 2),
      };
      // fitToBounds:false - the camera fit is UI churn the oracle does not want.
      return {
        action: 'setZeroDatum',
        args: [toRef(ref), gps, { fitToBounds: false }],
        label: `setZeroDatum(${refLabel(ref)})`,
      };
    },
  },
  {
    name: 'removeZeroDatum',
    weight: 2,
    build(ctx, state) {
      const datums = [...state.z].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
      if (datums.length === 0) return null;
      const datum = ctx.pick(datums);
      return {
        action: 'removeZeroDatum',
        args: [toRef(datum)],
        label: `removeZeroDatum(${refLabel(datum)})`,
      };
    },
  },
  {
    name: 'createLoopClosure',
    weight: 3,
    build(ctx, state) {
      const pair = pickCrossSectionPair(ctx, state);
      if (!pair) return null;
      const strategy = ctx.pick(CLOSURE_STRATEGIES);
      return {
        action: 'createLoopClosure',
        args: [toRef(pair[0]), toRef(pair[1]), strategy],
        label: `createLoopClosure(${refLabel(pair[0])},${refLabel(pair[1])},${strategy})`,
      };
    },
  },
  // ---------------------------------------------------------------------
  // v2 additions. These reach the command families with the hardest undo code:
  // the delete/split group and the import path push ProjectSnapshotPair entries,
  // and a batch is one undo entry covering several sub-commands with all-or-
  // nothing rollback.
  // ---------------------------------------------------------------------
  {
    name: 'splitShotAtDistance',
    weight: 5,
    since: 'v2',
    build(ctx, state) {
      const target = pickShot(ctx, state);
      if (!target) return null;
      // Distances AT the boundaries (0 and the full shot length) are the
      // interesting ones - a split there degenerates into a zero-length shot.
      const distance = biasedFloat(ctx, 0.1, 8, 3, [0, 1e-9, GEOMETRY_EXTREME, -1]);
      return {
        action: 'splitShotAtDistance',
        args: [target.sectionId, target.shotId, distance],
        label: `splitShotAtDistance(s=${target.sectionId},shot=${target.shotId},d=${distance})`,
      };
    },
  },
  {
    name: 'deleteShotAndSplit',
    weight: 3,
    since: 'v2',
    build(ctx, state) {
      const target = pickShot(ctx, state);
      if (!target) return null;
      const nameA = biasedText(ctx, `split-a-${ctx.randInt(0, 999)}`);
      const nameB = biasedText(ctx, `split-b-${ctx.randInt(0, 999)}`);
      return {
        action: 'deleteShotAndSplit',
        args: [target.sectionId, target.shotId, nameA, nameB],
        label: `deleteShotAndSplit(s=${target.sectionId},shot=${target.shotId})`,
      };
    },
  },
  {
    name: 'deleteLastShotAndSection',
    weight: 3,
    since: 'v2',
    build(ctx, state) {
      // Meaningful only on a single-shot section, which is exactly the shape
      // pickSmallSection is biased toward.
      const section = pickSmallSection(ctx, state);
      if (!section || section[1].length === 0) return null;
      const shotIds = [...section[1]].sort((a, b) => a - b);
      const shotId = ctx.pick(shotIds);
      return {
        action: 'deleteLastShotAndSection',
        args: [section[0], shotId],
        label: `deleteLastShotAndSection(s=${section[0]},n=${shotIds.length},shot=${shotId})`,
      };
    },
  },
  {
    name: 'deleteShotRangeSplit',
    weight: 3,
    since: 'v2',
    build(ctx, state) {
      const range = pickShotRange(ctx, state);
      if (!range) return null;
      const nameA = biasedText(ctx, `range-a-${ctx.randInt(0, 999)}`);
      const nameB = biasedText(ctx, `range-b-${ctx.randInt(0, 999)}`);
      return {
        action: 'deleteShotRangeSplit',
        args: [range.sectionId, range.startShotId, range.endShotId, nameA, nameB],
        label: `deleteShotRangeSplit(s=${range.sectionId},${range.startShotId}..${range.endShotId})`,
      };
    },
  },
  {
    name: 'deleteShotRangeShift',
    weight: 3,
    since: 'v2',
    build(ctx, state) {
      const range = pickShotRange(ctx, state);
      if (!range) return null;
      return {
        action: 'deleteShotRangeShift',
        args: [range.sectionId, range.startShotId, range.endShotId],
        label: `deleteShotRangeShift(s=${range.sectionId},${range.startShotId}..${range.endShotId})`,
      };
    },
  },
  {
    name: 'moveComment',
    weight: 4,
    since: 'v2',
    build(ctx, state) {
      const source = pickCommentHolder(ctx, state);
      const target = pickCommentHolder(ctx, state);
      if (!source || !target) return null;
      const swap = chance(ctx, 0.5);
      return {
        action: 'moveComment',
        args: [source.holder, target.holder, swap],
        label: `moveComment(${source.label}->${target.label},swap=${swap})`,
      };
    },
  },
  {
    name: 'createSectionAndMerge',
    weight: 3,
    since: 'v2',
    build(ctx, state) {
      const ref = pickStationRef(ctx, state);
      if (!ref) return null;
      const data = randomSectionData(ctx);
      const shots = data.shots as unknown[];
      // An out-of-range import station index is a deliberate rejection case.
      const importStationIndex = streamV2() && chance(ctx, 0.15) ? ctx.randInt(50, 99) : ctx.randInt(0, shots.length);
      const strategy = ctx.pick(CLOSURE_STRATEGIES);
      return {
        action: 'createSectionAndMerge',
        args: [data, importStationIndex, toRef(ref), strategy],
        label: `createSectionAndMerge(shots=${shots.length},i=${importStationIndex},->${refLabel(ref)})`,
      };
    },
  },
  {
    name: 'importSections',
    weight: 2,
    since: 'v2',
    build(ctx) {
      // FULLY SYNTHETIC: shots are generated here, never taken from a fixture.
      const data = randomSectionData(ctx);
      const shots = data.shots as unknown[];
      const config = {
        section_index: 0,
        name: data.name,
        surveyor: data.surveyor,
        date: data.date,
        color: data.color,
        direction: data.direction,
        quality: data.quality,
        shots,
        placement: { type: 'origin' },
        station_comments: chance(ctx, 0.4) ? [[0, biasedText(ctx, `imported-${ctx.randInt(0, 999)}`)]] : [],
        reverse: chance(ctx, 0.3),
      };
      return {
        action: 'importSections',
        args: [[config], null, null, ctx.pick(['Auto', 'AllJumps', 'AllReferences'] as const), null],
        label: `importSections(shots=${shots.length})`,
      };
    },
  },
  {
    name: 'executeBatch',
    weight: 5,
    since: 'v2',
    build(ctx, state) {
      const batch = buildBatch(ctx, state);
      if (!batch) return null;
      return {
        action: 'executeBatch',
        args: [batch.commands],
        label: `executeBatch(${batch.label})`,
      };
    },
  },
  {
    name: 'removeLoopClosure',
    weight: 2,
    build(ctx, state) {
      const ids = [...state.c].sort((a, b) => a - b);
      if (ids.length === 0) return null;
      const id = ctx.pick(ids);
      // A plain number: the store action's declared type (#576 C6), the only
      // thing WebDriver can carry, and what the IPC boundary sees.
      return { action: 'removeLoopClosure', args: [id], label: `removeLoopClosure(${id})` };
    },
  },
];

/**
 * Every generator this build knows about: the v1/v2 set above, then the v3
 * import group, in that order and never interleaved.
 *
 * Resolved LAZILY, on first use, rather than as a module-level const. That is
 * not a micro-optimization - it breaks an import CYCLE. `undoRedoFuzzImportActions`
 * imports the draw helpers from this module, so whichever of the two a caller
 * loads first, the other is mid-evaluation when its bindings are read. Reading
 * `IMPORT_ACTION_GENERATORS` inside a function defers that read until both
 * modules have finished, which a top-level array literal cannot do (it would
 * throw a TDZ `ReferenceError` whenever the import module happened to be
 * imported first - as a unit test importing it directly does).
 *
 * The ORDER is what keeps v1/v2 byte-identical: appending means the weighted
 * name array a v1 or v2 run builds is exactly the array it built before the v3
 * group existed, index for index.
 */
let allGeneratorsCache: readonly ActionGenerator[] | null = null;

function allGenerators(): readonly ActionGenerator[] {
  allGeneratorsCache ??= [...ACTION_GENERATORS, ...IMPORT_ACTION_GENERATORS];
  return allGeneratorsCache;
}

let byNameCache: Map<string, ActionGenerator> | null = null;

function generatorsByName(): Map<string, ActionGenerator> {
  byNameCache ??= new Map(allGenerators().map((generator) => [generator.name, generator]));
  return byNameCache;
}

/**
 * Weight-expanded generator names for a run.
 *
 * v1 uses the ORIGINAL generators at their original weights, so the legacy draw
 * stream is reproduced exactly. v2 adds the delete/split, batch and comment
 * families and applies the seed's swarm multipliers. v3 adds the import group on
 * top of v2, at the end of the array.
 */
export function weightedNamesFor(profile: SwarmProfile): string[] {
  const names: string[] = [];
  for (const generator of allGenerators()) {
    if (!streamV2() && generator.since === 'v2') continue;
    if (!streamV3() && generator.since === 'v3') continue;
    const weight = Math.max(1, Math.round(generator.weight * (profile.multipliers[generator.name] ?? 1)));
    for (let i = 0; i < weight; i++) names.push(generator.name);
  }
  return names;
}

/**
 * Draw a mutation the current project actually has a target for. Retries a
 * bounded number of times (a generator returns null when, say, there are no
 * loop closures to remove), then falls back to `createSection`, which is always
 * buildable.
 */
export function drawAction(
  ctx: ScenarioContext,
  state: TargetingDigest,
  weightedNames: readonly string[],
  attempts = 8,
): FuzzAction {
  for (let i = 0; i < attempts; i++) {
    const generator = generatorsByName().get(ctx.pick(weightedNames));
    const action = generator?.build(ctx, state);
    if (action) return action;
  }
  const data = randomSectionData(ctx);
  const shots = data.shots as unknown[];
  return { action: 'createSection', args: [data], label: `createSection(shots=${shots.length})` };
}
