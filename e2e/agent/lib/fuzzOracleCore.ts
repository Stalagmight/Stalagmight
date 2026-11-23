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
 * Everything else is compared: shot uuids, every `LoopClosure` metric, both
 * imported-hash vectors, the junction identity (shared Station IDs, names,
 * comments, `merged_with`), and all geometry. On the station-graph wire a few
 * fields are CONSTANT and so compare trivially: `reverse_parity` is `false`,
 * the gauge anchors are `null` (plan W5), and every derived
 * `StationMerge.merged_at` is the project's `created_at` (W4). They are kept
 * rather than dropped, so a backend that started filling them again would be
 * seen.
 *
 * Two other classes of noise are handled without dropping anything. Object keys
 * are sorted recursively, so no map's key order can leak in. And numbers are
 * rounded to 12 significant digits, because geometry is RE-DERIVED from the
 * model after every undo/redo rather than copied. The station graph derives it
 * deterministically, so an exact restore gives the same bits; the rounding is
 * only the fast path, and when two canonical strings still disagree Node
 * re-compares with an absolute tolerance too (see `projectSnapshot.ts`).
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
  // `Shot::id` is a session handle, never persisted (plan 1.1): the load
  // numbers every shot `line_id * FEATURE_ID_STRIDE + index`
  // (`Line::assign_session_shot_ids`, models/src/survey/types.rs), so a session
  // that deleted or inserted a shot mid-line comes back with different ids for
  // the same shots. Replacing each id with its position makes the comparison independent
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
 * ## What lives here vs in `Project::validate`
 *
 * The app already has a structural-integrity checker, `Project::validate`
 * (plan section 2), which it runs after every edit, import, load and
 * file-decoded history step, rolling back (with an error log) whatever would
 * break it. Duplicating it would be noise, so this suite deliberately does NOT
 * re-check what it owns:
 *
 *   - non-finite (NaN/inf) shot geometry
 *   - two shots in ONE line sharing a uuid (note: the app's contract is
 *     per-line, not global - `ensure_unique_shot_uuids` dedupes per line -
 *     so global uniqueness is NOT asserted here either)
 *   - dangling merge / loop-closure / zero-datum refs, stale hand moves
 *
 * Those are covered by the app's own validation (its rollback logs an error,
 * which the sweeps report) and by the strict roundtrip, which sees any state
 * an undo or redo fails to restore. Two further reasons not to duplicate them: several are `Warning`, i.e. states the app
 * considers repairable rather than corrupt, and the ref checks are largely
 * unobservable from this payload anyway - `ProjectIpc` emits merges, closures
 * and datums through `filter_map(..._to_wire)`, so a dangling ref is DROPPED on
 * the way out rather than shown.
 *
 * What is left, and what this suite asserts, is well-formedness nothing else
 * checks: id uniqueness, id-allocator dominance, agreement between the model
 * and the derived arrays computed from it, and the station graph's identity
 * on the wire (plan 9.2, kept and archived stations RS 3, 5 and 6): a line
 * passes a node (`id`) once, and two slots of a line that show different
 * identities have different names (N8; a same-line closure shows one
 * identity twice); the slots of one node show one identity (its name,
 * comment, records and shown Station ID) and list exactly each other in
 * `merged_with`; its records hold A1 to A4; a loop closure archives a node
 * at most once and never in a cycle (C1, C2); every slot shows what its
 * closures resolve it to, by that station's name and comment, and carries
 * `shown` exactly when it is foreign or a loop end, with the right flags,
 * home line and closure; and every `station_merges` pair joins two slots of
 * one node. The model enforces most of it (`validate`); the wire
 * projection's half is a contract nothing else checks from the outside.
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

  // Every station ref ON THE WIRE must address a real station. The model's
  // validation checks its own references BEFORE the wire conversion drops the
  // ones that do not resolve; this validates what consumers actually index into.
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

  // The station graph's identity on the wire (plan 9.2, kept and archived
  // stations RS 3, 5 and 6). Checked only on slots that carry an `id` (every
  // real payload does); a slot is named by its line and index. `id` is the
  // NODE (every grouping keys on it); the slot shows the node's shown
  // identity, `shown.id` when present else `id`, by `name` and `comment`;
  // `archived` lists the node's records; `shown` is present exactly on a
  // foreign slot (its line is not a native line of the node holding the
  // shown identity) and on a loop end (a closure archives the node).
  const slotName = (lineId: number, index: number): string => `${lineId}/${index}`;
  const lineOrder: Record<string, number> = {};
  for (let i = 0; i < lines.length; i++) {
    lineOrder[String(Number(((lines[i] ?? {}) as Record<string, unknown>).id))] = i;
  }
  const shownKeyOf = (slot: Record<string, unknown>): string => {
    const shown = slot.shown as Record<string, unknown> | undefined | null;
    return String(Number(shown && shown.id !== undefined && shown.id !== null ? shown.id : slot.id));
  };
  const recordsOf = (slot: Record<string, unknown>): Array<Record<string, unknown>> =>
    (Array.isArray(slot.archived) ? slot.archived : []) as Array<Record<string, unknown>>;
  const byStationId: Record<string, Array<Record<string, unknown>>> = {};
  const linesOfNode: Record<string, Record<string, true>> = {};
  const idAt: Record<string, string> = {};
  const perLineIds: Record<string, Record<string, true>> = {};
  const perLineNames: Record<string, Record<string, string>> = {};
  for (let i = 0; i < stations.length; i++) {
    const station = (stations[i] ?? {}) as Record<string, unknown>;
    if (station.id === undefined || station.id === null) continue;
    const ref = (station.station_ref ?? {}) as Record<string, unknown>;
    const lineKey = String(Number(ref.line_id));
    const idKey = String(Number(station.id));
    const here = slotName(Number(ref.line_id), Number(ref.station_index));
    const ids = (perLineIds[lineKey] = perLineIds[lineKey] ?? {});
    if (ids[idKey] === true) violations.push(`line ${lineKey} passes Station ID ${idKey} twice (at ${here})`);
    ids[idKey] = true;
    // N8: slots that show different identities have different names; two
    // slots of one shown identity (a same-line closure) may share one.
    if (typeof station.name === 'string') {
      const names = (perLineNames[lineKey] = perLineNames[lineKey] ?? {});
      const shownKey = shownKeyOf(station);
      const prior = names[station.name];
      if (prior !== undefined && prior !== shownKey) {
        violations.push(
          `line ${lineKey} shows two stations named ${JSON.stringify(station.name)} ` +
            `(Station IDs ${prior} and ${shownKey}, at ${here})`,
        );
      }
      names[station.name] = shownKey;
    }
    (byStationId[idKey] = byStationId[idKey] ?? []).push(station);
    (linesOfNode[idKey] = linesOfNode[idKey] ?? {})[lineKey] = true;
    idAt[here] = idKey;
  }
  // Each node: its slots agree, list each other, and its records hold A1 to
  // A4 (RS 3). `nativeOf` is its native lines in the model's line order.
  const nativeOf: Record<string, string[]> = {};
  const recordHolder: Record<string, string> = {};
  const idKeys = Object.keys(byStationId);
  for (let k = 0; k < idKeys.length; k++) {
    const idKey = idKeys[k] as string;
    const slots = byStationId[idKey] as Array<Record<string, unknown>>;
    const first = slots[0] as Record<string, unknown>;
    const members: string[] = [];
    for (let j = 0; j < slots.length; j++) {
      const ref = ((slots[j] as Record<string, unknown>).station_ref ?? {}) as Record<string, unknown>;
      members.push(slotName(Number(ref.line_id), Number(ref.station_index)));
    }
    const records = recordsOf(first);
    const named: Record<string, true> = {};
    for (let r = 0; r < records.length; r++) {
      const record = records[r] as Record<string, unknown>;
      const recordKey = String(Number(record.id));
      if (byStationId[recordKey] !== undefined) {
        violations.push(`record ${recordKey} of Station ID ${idKey} is also a live Station ID (A1)`);
      }
      if (recordHolder[recordKey] !== undefined) {
        violations.push(
          `record ${recordKey} is archived in both Station ID ${recordHolder[recordKey] as string} and ${idKey} (A1)`,
        );
      }
      recordHolder[recordKey] = idKey;
      const recordLines = Array.isArray(record.lines) ? record.lines : [];
      for (let l = 0; l < recordLines.length; l++) {
        const lineKey = String(Number(recordLines[l]));
        if ((linesOfNode[idKey] as Record<string, true>)[lineKey] !== true) {
          violations.push(`record ${recordKey} of Station ID ${idKey} names line ${lineKey}, which does not pass it (A2)`);
        }
        if (named[lineKey] === true) {
          violations.push(`Station ID ${idKey} has two records naming line ${lineKey} (A3)`);
        }
        named[lineKey] = true;
      }
    }
    const native = Object.keys(linesOfNode[idKey] as Record<string, true>)
      .filter((lineKey) => named[lineKey] !== true)
      .sort((x, y) => (lineOrder[x] ?? 0) - (lineOrder[y] ?? 0));
    nativeOf[idKey] = native;
    if (native.length === 0) violations.push(`Station ID ${idKey} has no native line (A4)`);
    const facts = (slot: Record<string, unknown>): string =>
      JSON.stringify([slot.name ?? null, slot.comment ?? null, recordsOf(slot), shownKeyOf(slot)]);
    for (let j = 0; j < slots.length; j++) {
      const slot = slots[j] as Record<string, unknown>;
      const here = members[j] as string;
      // RS 4.1: a junction shows one identity, by one name, on every line.
      if (facts(slot) !== facts(first)) {
        violations.push(
          `Station ID ${idKey} has a different name, comment, records or shown station at ${here} than at ${members[0] as string}`,
        );
      }
      const listed: string[] = [];
      const mergedWith = Array.isArray(slot.merged_with) ? slot.merged_with : [];
      for (let m = 0; m < mergedWith.length; m++) {
        const ref = (mergedWith[m] ?? {}) as Record<string, unknown>;
        listed.push(slotName(Number(ref.line_id), Number(ref.station_index)));
      }
      const expected = members.filter((name) => name !== here);
      if (listed.slice().sort().join(',') !== expected.slice().sort().join(',')) {
        violations.push(
          `${here} (Station ID ${idKey}) lists merged_with [${listed.join(', ')}] ` +
            `but the slots sharing its Station ID are [${expected.join(', ')}]`,
        );
      }
    }
  }
  // Loop closures that archive an end (RS 5): at most one per node (C1), and
  // following archived end -> kept end never cycles (C2).
  const keptOf: Record<string, string> = {};
  const archivingClosure: Record<string, number> = {};
  for (let i = 0; i < closures.length; i++) {
    const closure = (closures[i] ?? {}) as Record<string, unknown>;
    const archive = closure.archived as Record<string, unknown> | undefined | null;
    if (!archive) continue;
    const a = (closure.station_a ?? {}) as Record<string, unknown>;
    const b = (closure.station_b ?? {}) as Record<string, unknown>;
    const [end, other] = archive.end === 'a' ? [a, b] : [b, a];
    const node = idAt[slotName(Number(end.line_id), Number(end.station_index))];
    const kept = idAt[slotName(Number(other.line_id), Number(other.station_index))];
    if (node === undefined || kept === undefined) continue;
    const own = (archive.station ?? {}) as Record<string, unknown>;
    if (String(Number(own.id)) !== node) {
      violations.push(`loop_closures[${i}] archives Station ID ${node} but names it ${String(own.id)}`);
    }
    if (keptOf[node] !== undefined) {
      violations.push(`Station ID ${node} is archived by two loop closures (C1)`);
      continue;
    }
    keptOf[node] = kept;
    archivingClosure[node] = Number(closure.id);
  }
  const resolve = (node: string): string | null => {
    let current = node;
    for (let step = 0; step <= idKeys.length; step++) {
      const next = keptOf[current];
      if (next === undefined) return current;
      current = next;
    }
    return null;
  };
  for (let k = 0; k < idKeys.length; k++) {
    const idKey = idKeys[k] as string;
    const target = resolve(idKey);
    if (target === null) {
      violations.push(`the loop closures archiving Station ID ${idKey} cycle (C2)`);
      continue;
    }
    const slots = byStationId[idKey] as Array<Record<string, unknown>>;
    const holder = ((byStationId[target] ?? [])[0] ?? {}) as Record<string, unknown>;
    const loopEnd = keptOf[idKey] !== undefined;
    for (let j = 0; j < slots.length; j++) {
      const slot = slots[j] as Record<string, unknown>;
      const ref = (slot.station_ref ?? {}) as Record<string, unknown>;
      const lineKey = String(Number(ref.line_id));
      const here = slotName(Number(ref.line_id), Number(ref.station_index));
      if (shownKeyOf(slot) !== target) {
        violations.push(
          `${here} (Station ID ${idKey}) shows Station ID ${shownKeyOf(slot)}, but its loop closures make it ${target}`,
        );
        continue;
      }
      if (target !== idKey && ((slot.name ?? null) !== (holder.name ?? null) || (slot.comment ?? null) !== (holder.comment ?? null))) {
        violations.push(
          `${here} (Station ID ${idKey}) shows ${JSON.stringify(slot.name)}, but Station ID ${target} (which it shows) is ${JSON.stringify(holder.name)}`,
        );
      }
      const native = nativeOf[target] ?? [];
      const foreign = native.indexOf(lineKey) < 0;
      const shown = slot.shown as Record<string, unknown> | undefined | null;
      if (!shown) {
        if (foreign || loopEnd) {
          violations.push(
            `${here} (Station ID ${idKey}) is ${loopEnd ? 'a loop end' : 'a foreign slot'} but carries no shown station`,
          );
        }
        continue;
      }
      const home = native.length > 0 ? Number(native[0]) : undefined;
      const homeLine = shown.home_line === undefined || shown.home_line === null ? undefined : Number(shown.home_line);
      const closureId = shown.loop_closure === undefined || shown.loop_closure === null ? undefined : Number(shown.loop_closure);
      if (
        shown.foreign !== foreign ||
        shown.loop_end !== loopEnd ||
        homeLine !== home ||
        closureId !== (loopEnd ? archivingClosure[idKey] : undefined) ||
        (!foreign && !loopEnd)
      ) {
        violations.push(
          `${here} (Station ID ${idKey}) carries shown ${JSON.stringify(shown)}, but it is ` +
            `foreign=${String(foreign)} loop_end=${String(loopEnd)} with home line ${String(home)}`,
        );
      }
    }
  }
  for (let i = 0; i < merges.length; i++) {
    const merge = (merges[i] ?? {}) as Record<string, unknown>;
    const a = (merge.station_a ?? {}) as Record<string, unknown>;
    const b = (merge.station_b ?? {}) as Record<string, unknown>;
    const nameA = slotName(Number(a.line_id), Number(a.station_index));
    const nameB = slotName(Number(b.line_id), Number(b.station_index));
    const idA = idAt[nameA];
    const idB = idAt[nameB];
    if (idA === undefined || idB === undefined) continue;
    if (nameA === nameB || idA !== idB) {
      violations.push(`station_merges[${i}] joins ${nameA} and ${nameB}, which are not two slots of one station`);
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
  // `station_count` counts stations, not slots: a junction once, a loop
  // end (a closure-archived node) as a station of its own (RS 6, U-P3-11).
  // Counted only when the slots carry their `id` (every real payload does).
  const distinctStations = idKeys.length;
  if (
    distinctStations > 0 &&
    Number.isFinite(Number(stats.station_count)) &&
    Number(stats.station_count) !== distinctStations
  ) {
    violations.push(`stats.station_count ${Number(stats.station_count)} != ${distinctStations} station(s)`);
  }

  return violations;
}
