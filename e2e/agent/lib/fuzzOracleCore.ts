/**
 * The undo/redo fuzzer's oracle CORE: the two functions that run in BOTH worlds.
 *
 * Shipping a whole `get_project_state` to Node on every step is the fuzzer's
 * dominant cost (~60 KB on the fractal fixture, ~277 KB on naharon, twice per
 * iteration). So the snapshot is taken and compared INSIDE the webview, and only
 * a verdict plus a small targeting digest crosses the WebDriver bridge.
 *
 * That means the same code has to run in Node (unit tests, and the Node-side
 * diff path) and in the page. It is not duplicated: the functions below are
 * SELF-CONTAINED - no imports, no module-scope references, every helper nested -
 * and `oracleInjectionSource()` serializes them with `Function.prototype
 * .toString()` for injection. tsx/esbuild strips the type annotations and
 * otherwise leaves a plain function declaration, so what the page evaluates is
 * literally the code Node runs. `fuzzOracleCore.test.ts` proves that by feeding
 * the same state to the direct call and to the injected-and-evaled copy and
 * asserting byte-identical output; a stray outer reference would surface there
 * as a ReferenceError, because `new Function` closes over nothing.
 *
 * Do NOT let these two grow a dependency, and do NOT hand-copy them anywhere.
 *
 * ## What the snapshot normalizes, and why so little
 *
 * The fuzzer's claim is "undo/redo roundtrips the project EXACTLY", so the
 * comparison covers the full IPC payload - `project` + `stations` + `stats` +
 * `frozen_shots`. `project` alone is not enough: `ProjectIpc` omits
 * `station_offsets` / `station_comments`, which are only observable through
 * `stations[].coordinates` and `stations[].comment`.
 *
 * Every field dropped is a place a real bug could hide, so the list is short and
 * each entry is a thing that CANNOT match by construction:
 *
 *   - `project.modified_at`  - every undo/redo path calls `Project::touch()`
 *     (`src-tauri/src/core/state/undo.rs`).
 *   - `project.file_path`    - `restore_snapshot_into` carries the LIVE path
 *     across a snapshot restore (`core/state/app_state.rs`).
 *   - `project.next_line_id`, `project.next_closure_id`,
 *     `project.lines.*.next_shot_id` - monotonic id counters, and an
 *     ASYMMETRY rather than a bug: a snapshot-pair undo restores them verbatim
 *     while a targeted restore (`restore_line`, a re-created shot) only ever
 *     raises them, so comparing them would fire on every mixed history.
 *
 * Everything else is compared: shot uuids, `reverse_parity`, the gauge anchors,
 * `StationMerge.merged_at`, every `LoopClosure` metric, both imported-hash
 * vectors, and all geometry.
 *
 * Two other classes of noise are handled without dropping anything. Object keys
 * are sorted recursively, which canonicalizes `Project::line_offsets` (a
 * `HashMap`, so its JSON key order is not stable across a snapshot restore)
 * without hiding a value. And numbers are rounded to 12 significant digits,
 * because geometry is RE-DERIVED after every undo/redo rather than copied. The
 * rounding is only the fast path: when two canonical strings still disagree,
 * Node re-compares with an absolute tolerance too (see `projectSnapshot.ts`),
 * which is what catches values either side of a true zero.
 */

/** Documentation mirror of the list inlined in `canonicalProjectSnapshot`. */
export const NON_ROLLBACK_PATHS: readonly string[] = [
  'project.modified_at',
  'project.file_path',
  'project.next_line_id',
  'project.next_closure_id',
  'project.lines.*.next_shot_id',
];

/** Documentation mirror of the constant inlined in `canonicalProjectSnapshot`. */
export const SIGNIFICANT_DIGITS = 12;

/**
 * SELF-CONTAINED. Canonical, comparison-ready JSON for a `get_project_state`
 * payload: non-rolling-back fields dropped, every object's keys sorted, every
 * finite number rounded to 12 significant digits. Two states that should be
 * equal produce byte-identical strings.
 *
 * Keep this dependency-free - it is injected into the webview verbatim.
 */
export function canonicalProjectSnapshot(state: unknown, idAgnostic?: boolean): string {
  // Inlined rather than imported: see the module header. `NON_ROLLBACK_PATHS`
  // and `SIGNIFICANT_DIGITS` mirror these, and a unit test pins them together.
  const dropped: Record<string, true> = {
    'project.modified_at': true,
    'project.file_path': true,
    'project.next_line_id': true,
    'project.next_closure_id': true,
    'project.lines.*.next_shot_id': true,
  };
  const digits = 12;

  const isPlainObject = (value: unknown): value is Record<string, unknown> =>
    typeof value === 'object' && value !== null && !Array.isArray(value);

  const round = (value: number): number => {
    if (!Number.isFinite(value) || value === 0) return value;
    return Number(value.toPrecision(digits));
  };

  const walk = (value: unknown, path: string): unknown => {
    if (Array.isArray(value)) {
      const childPath = path ? `${path}.*` : '*';
      const out: unknown[] = [];
      for (let i = 0; i < value.length; i++) out.push(walk(value[i], childPath));
      return out;
    }
    if (isPlainObject(value)) {
      const out: Record<string, unknown> = {};
      const keys = Object.keys(value).sort();
      for (let i = 0; i < keys.length; i++) {
        const key = keys[i] as string;
        const childPath = path ? `${path}.${key}` : key;
        if (dropped[childPath] === true) continue;
        out[key] = walk(value[key], childPath);
      }
      return out;
    }
    if (typeof value === 'number') return round(value);
    // u64 ids can arrive as BigInt through some bridges; compare them as text.
    if (typeof value === 'bigint') return value.toString();
    return value;
  };

  const normalized = walk(state, '');
  // ID-AGNOSTIC mode, used ONLY for comparisons that cross a save/reload.
  // `Shot::id` is not persisted: the loader renumbers every shot to
  // `line_id * FEATURE_ID_STRIDE + sequence_order`
  // (src-tauri/src/persistence/sqlite/read.rs:372), so a session that deleted or
  // inserted a shot mid-line comes back with different ids for the same
  // shots. Replacing each id with its position makes the comparison independent
  // of that renumbering while leaving everything else - uuids, geometry,
  // ordering, every other field - compared exactly. Line ids ARE persisted
  // and are NOT touched here.
  if (idAgnostic === true && normalized !== null && typeof normalized === 'object') {
    const project = (normalized as Record<string, unknown>).project;
    if (isPlainObject(project) && Array.isArray(project.lines)) {
      for (let i = 0; i < project.lines.length; i++) {
        const line = project.lines[i];
        if (!isPlainObject(line) || !Array.isArray(line.shots)) continue;
        for (let j = 0; j < line.shots.length; j++) {
          const shot = line.shots[j];
          if (isPlainObject(shot) && 'id' in shot) shot.id = j;
        }
      }
    }
  }
  return JSON.stringify(normalized === undefined ? null : normalized);
}

/**
 * SELF-CONTAINED. Re-express an ALREADY-canonical snapshot string in id-agnostic
 * form, so snapshots recorded before a save/reload can still be compared against
 * states observed after it. Same rule as the `idAgnostic` flag above; this is the
 * version for strings the model already holds.
 *
 * Keep this dependency-free - it is injected into the webview verbatim.
 */
export function idAgnosticSnapshot(canonicalJsonText: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(canonicalJsonText);
  } catch {
    return canonicalJsonText;
  }
  const isObj = (value: unknown): value is Record<string, unknown> =>
    typeof value === 'object' && value !== null && !Array.isArray(value);
  if (!isObj(parsed)) return canonicalJsonText;
  const project = parsed.project;
  if (isObj(project) && Array.isArray(project.lines)) {
    for (let i = 0; i < project.lines.length; i++) {
      const line = project.lines[i];
      if (!isObj(line) || !Array.isArray(line.shots)) continue;
      for (let j = 0; j < line.shots.length; j++) {
        const shot = line.shots[j];
        if (isObj(shot) && 'id' in shot) shot.id = j;
      }
    }
  }
  // Key order is preserved from the already-sorted input, so this stays canonical.
  return JSON.stringify(parsed);
}

/**
 * The compact view of live state the action generators target.
 *
 * Every field here is one the generators in `undoRedoFuzzActions.ts` actually
 * read, and each list is in the SAME ORDER as the payload it came from. Order
 * matters even though every generator re-sorts what it picks from: a sort is
 * only a total order while its keys are unique, and preserving payload order
 * keeps the tie-breaking (and therefore the rng draw sequence, and therefore
 * every documented repro seed) identical to when the generators read the raw
 * project state.
 *
 * Tuples rather than objects because this crosses the bridge on every step:
 * naharon's full payload is ~277 KB, its digest ~15 KB.
 */
export interface TargetingDigest {
  /** `[lineId, [shotId, ...]]`, in `project.lines` order. */
  s: [number, number[]][];
  /** `[line_id, station_index]`, in `stations` order. */
  st: [number, number][];
  /** `[a.line_id, a.station_index, b.line_id, b.station_index]`, in `project.station_merges` order. */
  m: [number, number, number, number][];
  /** `[line_id, station_index]`, in `project.zero_datums` order. */
  z: [number, number][];
  /** Closure ids, in `project.loop_closures` order. */
  c: number[];
}

/**
 * SELF-CONTAINED. Reduce a `get_project_state` payload to the targeting digest.
 *
 * Keep this dependency-free - it is injected into the webview verbatim.
 */
export function buildTargetingDigest(state: unknown): TargetingDigest {
  const root = (state ?? {}) as Record<string, unknown>;
  const project = (root.project ?? {}) as Record<string, unknown>;
  const lines = Array.isArray(project.lines) ? project.lines : [];
  const stations = Array.isArray(root.stations) ? root.stations : [];
  const merges = Array.isArray(project.station_merges) ? project.station_merges : [];
  const datums = Array.isArray(project.zero_datums) ? project.zero_datums : [];
  const closures = Array.isArray(project.loop_closures) ? project.loop_closures : [];

  const refPair = (value: unknown): [number, number] => {
    const ref = (value ?? {}) as Record<string, unknown>;
    return [Number(ref.line_id), Number(ref.station_index)];
  };

  const s: [number, number[]][] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = (lines[i] ?? {}) as Record<string, unknown>;
    const shots = Array.isArray(line.shots) ? line.shots : [];
    const shotIds: number[] = [];
    for (let j = 0; j < shots.length; j++) {
      shotIds.push(Number((shots[j] as Record<string, unknown>).id));
    }
    s.push([Number(line.id), shotIds]);
  }

  const st: [number, number][] = [];
  for (let i = 0; i < stations.length; i++) {
    st.push(refPair((stations[i] as Record<string, unknown>).station_ref));
  }

  const m: [number, number, number, number][] = [];
  for (let i = 0; i < merges.length; i++) {
    const merge = (merges[i] ?? {}) as Record<string, unknown>;
    const a = refPair(merge.station_a);
    const b = refPair(merge.station_b);
    m.push([a[0], a[1], b[0], b[1]]);
  }

  const z: [number, number][] = [];
  for (let i = 0; i < datums.length; i++) {
    z.push(refPair((datums[i] as Record<string, unknown>).station_ref));
  }

  const c: number[] = [];
  for (let i = 0; i < closures.length; i++) {
    c.push(Number((closures[i] as Record<string, unknown>).id));
  }

  return { s, st, m, z, c };
}

/** Global the injected oracle installs. `globalThis` is `window` in the webview. */
export const ORACLE_GLOBAL = '__FUZZ_ORACLE__';

/**
 * JS source that installs the oracle at `globalThis.__FUZZ_ORACLE__`:
 * the two functions above, verbatim, plus the per-run model state (the
 * canonical snapshot for every undo depth, and the last mismatching pair so
 * Node can fetch it only when it needs it). Nothing else, and nothing that
 * writes to app state.
 *
 * Re-running it resets the model, which is what a scenario does at start.
 */
export function oracleInjectionSource(): string {
  const parts = [
    { name: 'canonicalProjectSnapshot', source: canonicalProjectSnapshot.toString() },
    { name: 'buildTargetingDigest', source: buildTargetingDigest.toString() },
    { name: 'checkProjectInvariants', source: checkProjectInvariants.toString() },
    { name: 'idAgnosticSnapshot', source: idAgnosticSnapshot.toString() },
  ];
  for (const part of parts) {
    // A toolchain that renames or wraps these (a minifier, a coverage
    // instrumenter) would otherwise inject something subtly different from what
    // Node runs, which is the one thing this design must never do.
    if (!part.source.startsWith(`function ${part.name}(`)) {
      throw new Error(
        `oracle source for ${part.name} is not a plain function declaration; ` +
          `refusing to inject: ${part.source.slice(0, 80)}`,
      );
    }
  }
  return [
    '(function () {',
    ...parts.map((part) => part.source),
    `globalThis.${ORACLE_GLOBAL} = {`,
    '  canonical: canonicalProjectSnapshot,',
    '  digest: buildTargetingDigest,',
    '  invariants: checkProjectInvariants,',
    '  idAgnostic: idAgnosticSnapshot,',
    '  idAgnosticMode: false,',
    '  epoch: 0,',
    '  epochs: [],',
    '  snapshots: [],',
    '  integrity: [],',
    '  lastExpected: null,',
    '  lastActual: null,',
    '  mismatchIndex: -1,',
    '};',
    '})();',
  ].join('\n');
}

/**
 * SELF-CONTAINED. ABSOLUTE well-formedness checks on one observed state,
 * returning a violation message per broken invariant (empty = sound).
 *
 * ## Why this exists next to the roundtrip oracle
 *
 * The snapshot comparison only catches drift BETWEEN states. If a forward
 * command corrupts the project and undo faithfully restores the corruption,
 * every snapshot matches and nothing fires. These are the checks that do not
 * care what the previous state was.
 *
 * ## What lives here vs in `check_project_integrity`
 *
 * The app already has a structural-integrity checker
 * (`AppState::integrity_check` -> `Project::check_structural_integrity`,
 * `src-tauri/src/models/project/integrity.rs`). Duplicating it would be noise,
 * so this suite deliberately does NOT re-check what it owns:
 *
 *   - non-finite (NaN/inf) shot geometry
 *   - two shots in ONE line sharing a uuid (note: the app's contract is
 *     per-line, not global - `ensure_unique_shot_uuids` dedupes per line -
 *     so global uniqueness is NOT asserted here either)
 *   - dangling merge / loop-closure / zero-datum refs, orphan station offsets
 *     and comments, orphan `line_offsets` keys
 *
 * Those are covered instead by the integrity-DELTA oracle, which records the
 * checker's own verdict per depth and asserts undo/redo restores it. Two further
 * reasons not to duplicate them: several are `Warning`, i.e. states the app
 * considers repairable rather than corrupt, and the ref checks are largely
 * unobservable from this payload anyway - `ProjectIpc` emits merges, closures
 * and datums through `filter_map(..._to_wire)`, so a dangling ref is DROPPED on
 * the way out rather than shown.
 *
 * What is left, and what this suite asserts, is well-formedness nothing else
 * checks: id uniqueness, id-allocator dominance, and agreement between the
 * model and the derived arrays computed from it.
 *
 * Keep this dependency-free - it is injected into the webview verbatim.
 */
export function checkProjectInvariants(state: unknown): string[] {
  const violations: string[] = [];
  const root = (state ?? {}) as Record<string, unknown>;
  const project = (root.project ?? {}) as Record<string, unknown>;
  const lines = Array.isArray(project.lines) ? project.lines : [];
  const stations = Array.isArray(root.stations) ? root.stations : [];
  const closures = Array.isArray(project.loop_closures) ? project.loop_closures : [];
  const frozen = Array.isArray(root.frozen_shots) ? root.frozen_shots : [];
  const stats = (root.stats ?? {}) as Record<string, unknown>;

  const nextLineId = Number(project.next_line_id);
  const nextClosureId = Number(project.next_closure_id);

  // Line id uniqueness + allocator dominance. `get_line` takes the FIRST
  // match, so a duplicate id silently shadows a line; an id at or above the
  // allocator means the next create collides with a live line.
  const seenLineIds: Record<string, true> = {};
  const shotCounts: Record<string, number> = {};
  let totalShots = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = (lines[i] ?? {}) as Record<string, unknown>;
    const id = Number(line.id);
    const key = String(id);
    if (seenLineIds[key] === true) {
      violations.push(`duplicate line id ${id}`);
    }
    seenLineIds[key] = true;
    if (Number.isFinite(nextLineId) && id >= nextLineId) {
      violations.push(`line id ${id} is at or above next_line_id ${nextLineId}`);
    }

    const shots = Array.isArray(line.shots) ? line.shots : [];
    shotCounts[key] = shots.length;
    totalShots += shots.length;
    const nextShotId = Number(line.next_shot_id);
    const seenShotIds: Record<string, true> = {};
    for (let j = 0; j < shots.length; j++) {
      const shot = (shots[j] ?? {}) as Record<string, unknown>;
      const shotId = Number(shot.id);
      const shotKey = String(shotId);
      if (seenShotIds[shotKey] === true) {
        violations.push(`line ${id} has two shots with id ${shotId}`);
      }
      seenShotIds[shotKey] = true;
      if (Number.isFinite(nextShotId) && shotId >= nextShotId) {
        violations.push(`line ${id} shot id ${shotId} is at or above next_shot_id ${nextShotId}`);
      }
    }
  }

  // Loop-closure id uniqueness + allocator dominance. Undo/redo of a closure
  // restores it BY ID (`UndoData::ClosureInfo`), so a duplicate id makes those
  // paths ambiguous.
  const seenClosureIds: Record<string, true> = {};
  for (let i = 0; i < closures.length; i++) {
    const closure = (closures[i] ?? {}) as Record<string, unknown>;
    const id = Number(closure.id);
    const key = String(id);
    if (seenClosureIds[key] === true) violations.push(`duplicate loop closure id ${id}`);
    seenClosureIds[key] = true;
    if (Number.isFinite(nextClosureId) && id >= nextClosureId) {
      violations.push(`loop closure id ${id} is at or above next_closure_id ${nextClosureId}`);
    }
  }

  // Every station ref ON THE WIRE must address a real station. The integrity
  // checker validates StationKeys BEFORE the wire conversion drops the ones that
  // do not resolve; this validates what consumers actually index into.
  const checkRef = (label: string, value: unknown): void => {
    const ref = (value ?? {}) as Record<string, unknown>;
    const lineId = Number(ref.line_id);
    const index = Number(ref.station_index);
    const count = shotCounts[String(lineId)];
    if (count === undefined) {
      violations.push(`${label} references missing line ${lineId}`);
      return;
    }
    // A line with N shots has N+1 stations, indices 0..N.
    if (!Number.isInteger(index) || index < 0 || index > count) {
      violations.push(`${label} station index ${index} out of range for line ${lineId} (0..${count})`);
    }
  };

  const merges = Array.isArray(project.station_merges) ? project.station_merges : [];
  for (let i = 0; i < merges.length; i++) {
    const merge = (merges[i] ?? {}) as Record<string, unknown>;
    checkRef(`station_merges[${i}].station_a`, merge.station_a);
    checkRef(`station_merges[${i}].station_b`, merge.station_b);
  }
  const datums = Array.isArray(project.zero_datums) ? project.zero_datums : [];
  for (let i = 0; i < datums.length; i++) {
    checkRef(`zero_datums[${i}]`, (datums[i] as Record<string, unknown>).station_ref);
  }
  for (let i = 0; i < closures.length; i++) {
    const closure = (closures[i] ?? {}) as Record<string, unknown>;
    checkRef(`loop_closures[${i}].station_a`, closure.station_a);
    checkRef(`loop_closures[${i}].station_b`, closure.station_b);
  }
  for (let i = 0; i < stations.length; i++) {
    checkRef(`stations[${i}]`, (stations[i] as Record<string, unknown>).station_ref);
  }

  // The derived station list must agree with the model it was computed from:
  // one station per shot plus a terminal one, per line.
  const expectedStations = lines.length + totalShots;
  if (stations.length !== expectedStations) {
    violations.push(
      `stations has ${stations.length} entries but the lines imply ${expectedStations} ` +
        `(${lines.length} line(s) + ${totalShots} shot(s))`,
    );
  }

  // frozen_shots indexes into the same model.
  for (let i = 0; i < frozen.length; i++) {
    const entry = (frozen[i] ?? {}) as Record<string, unknown>;
    const lineId = Number(entry.line_id);
    const count = shotCounts[String(lineId)];
    const shotIndex = Number(entry.shot_index);
    if (count === undefined) {
      violations.push(`frozen_shots[${i}] references missing line ${lineId}`);
    } else if (!Number.isInteger(shotIndex) || shotIndex < 0 || shotIndex >= count) {
      violations.push(
        `frozen_shots[${i}] shot index ${shotIndex} out of range for line ${lineId} (0..${count - 1})`,
      );
    }
  }

  // Derived stats vs the model. Cheap, and it catches a stats computation that
  // has drifted from the project it summarizes.
  if (Number.isFinite(Number(stats.line_count)) && Number(stats.line_count) !== lines.length) {
    violations.push(`stats.line_count ${Number(stats.line_count)} != ${lines.length} line(s)`);
  }
  if (Number.isFinite(Number(stats.shot_count)) && Number(stats.shot_count) !== totalShots) {
    violations.push(`stats.shot_count ${Number(stats.shot_count)} != ${totalShots} shot(s)`);
  }
  if (Number.isFinite(Number(stats.station_count)) && Number(stats.station_count) !== stations.length) {
    violations.push(`stats.station_count ${Number(stats.station_count)} != ${stations.length} station(s)`);
  }

  return violations;
}
