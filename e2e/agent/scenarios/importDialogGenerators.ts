/**
 * `import-dialog-fuzz`'s move generators: a weighted catalog, one entry per
 * thing a user can do on a screen of the DMP import flow.
 *
 * ## The contract every entry keeps
 *
 * `build` returns `null` when the move is IMPOSSIBLE right now - the control is
 * not there, the select has no options, there is no frozen row to aim at. That
 * is the harness's standing rule (`undoRedoFuzzActions.ts` has the same one):
 * an impossible move is skipped and counted, never asserted away, because a
 * ddmin-cut trace runs against a state the original run never had and every
 * removed step makes more moves impossible. A generator that threw instead
 * would make shrinking impossible.
 *
 * Weights are the ones the wave-2 plan fixed. Tune them only against a smoke run
 * that shows a screen starving; a weight changed for taste silently rewrites
 * every seed's trajectory and invalidates every repro line in the header of
 * `importDialogFuzz.ts`.
 *
 * ## Five formats, three input sources
 *
 * A flow starts by drawing a FORMAT FAMILY (`dmp`, `tml`, `compass`, `walls`,
 * `gpx` - restricted by `STALAGMIGHT_FUZZ_IMPORT_FORMAT`), then a concrete
 * extension inside it, then where the bytes come from: a generated spec
 * (valid-biased, or corrupted with probability
 * `STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P`), a tracked fixture, or an entry from
 * the replayer's own PRIVATE corpus when one is configured.
 *
 * The family matters to the flow and not only to the parser: `.dmp` is the ONLY
 * additive import. Every other extension routes to `new-project-for-import`
 * whether or not a project is open (`src/utils/openOrImport.ts` - with a
 * project open it goes through `import-as-new-project` first, whose confirm
 * closes the project and re-enters the router). So a non-DMP flow always has a
 * project-creation screen in the middle of it.
 *
 * ## Nothing here ever names a corpus file
 *
 * A corpus source holds its corpus-relative path only IN MEMORY, to materialize
 * the file with, and is LABELLED by `corpusFileId`. Labels reach the move
 * history, the failure payload and the metric keys, and the id is also the only
 * thing a recorded trace carries - all of which are things that get attached to
 * an issue. See `lib/fixtureCorpus.ts` for why the real name cannot be one of
 * them.
 */
import type { ScenarioContext } from './framework';
import type { ImportScreenDigest } from '../lib/importScreenDigest';
import {
  FORMATS_IN_FAMILY,
  HOSTILE_COMMENTS,
  SURVEY_FORMAT_FAMILIES,
  randomSurveyFileSpec,
  suggestiveDmpSpec,
  type GeneratedFileSpec,
  type Rng,
  type SurveyFileFormat,
  type SurveyFormatFamily,
} from '../lib/surveyFileSpecs';
import { corruptSpec } from '../lib/surveyFileCorrupt';
import { corpusFileId, discoverCorpus, type CorpusEntry } from '../lib/fixtureCorpus';
import { ORPHAN_OVERLAP_CHOICES, OVERLAP_MODES, VIRTUAL_SHOT_MODES, type ScreenId } from './importDialogModel';

// ---------------------------------------------------------------------------
// Move vocabulary
// ---------------------------------------------------------------------------

/** Where an input file comes from. */
export type FileSource =
  /**
   * Built here from a spec; the SPEC travels in the trace, never the bytes.
   *
   * `shape` names the non-default generator the spec came out of, so a run can
   * COUNT them (`file:shape:*`) and a label can say which one a flow used. Not
   * part of the recorded step: a replay re-renders from the spec itself, which
   * already carries everything the shape decided.
   */
  | { origin: 'gen'; spec: GeneratedFileSpec; rel: string; shape?: 'suggestive' }
  /** A tracked fixture, materialized by `ctx.fixturePath` under `fixtures/`. */
  | { origin: 'fixture'; name: string; rel: string }
  /**
   * An entry from the replayer's own private corpus.
   *
   * TWO rels, because the unit of materialization is a DIRECTORY: a Compass
   * `.mak` names sibling `.dat` files and a Walls `.wpj` names a tree of
   * `.srv`, so `materializeCorpusEntry` copies the entry's whole folder into
   * `dirRel` and the file to open is `rel` inside it.
   *
   * `corpusRel` is resolved against whatever `STALAGMIGHT_FUZZ_CORPUS_DIR` is
   * set to at the time, and never leaves this process: `id` is the publishable
   * stand-in that the labels, the metrics and the rendered trace all carry, and
   * `hash` is the resolution key the `corpusFile` trace step also carries but
   * nothing renders (`fixtureCorpus.corpusPathHash` - 20 bits does not resolve a
   * corpus of a few thousand files without sometimes resolving it wrong). A
   * replayer resolves the hash against its own corpus, and one that does not
   * have that file skips the move and counts it.
   */
  | { origin: 'corpus'; corpusRel: string; id: string; hash: string; dirRel: string; rel: string }
  /**
   * REPLAY ONLY: a file a preceding `genFile` step already wrote. The spec does
   * not travel twice, so the reference is just the rel - and if ddmin cut the
   * `genFile` step away, the file is simply not there and the move is skipped
   * and counted like any other impossible one.
   */
  | { origin: 'disk'; rel: string };

/**
 * One move. The first two mirror the `ui` / `input` trace steps exactly; the
 * rest are shell-level moves that need Node (disk, a native dialog answer, the
 * map projection) and are translated into their own trace steps there.
 */
export type FuzzMove =
  | { kind: 'ui'; testid: string; double?: boolean; label: string }
  | { kind: 'input'; testid: string; value: string; mode: 'type' | 'select' | 'check'; label: string }
  /** Materialize a file and hand it to the app, by picker or by router. */
  | { kind: 'openFile'; source: FileSource; via: 'dialog' | 'route'; trigger: string; label: string }
  /** Answer a picker with a CANCEL, which is its own code path. */
  | { kind: 'cancelDialog'; trigger: string; label: string }
  /**
   * A map click. `relative` means `lngLat` is an OFFSET from the map's current
   * centre, resolved by the shell at dispatch and recorded ABSOLUTE - a
   * generator cannot know where the survey is, and `lib/controls.ts`'s
   * `mapClick` refuses a point outside the visible viewport, so an absolute
   * draw would be skipped on essentially every attempt.
   */
  | { kind: 'mapClick'; lngLat: [number, number]; relative?: boolean; label: string }
  | { kind: 'undo'; label: string }
  | { kind: 'redo'; label: string }
  | { kind: 'save'; label: string }
  /** Close the project. Only ever drawn at `idle` - see `buildAuxInterjection`. */
  | { kind: 'closeProject'; label: string }
  /** Arm a picker answer, then press the chord that raises it. */
  | { kind: 'keyFile'; chord: string; source: FileSource; label: string };

/** What a generator gets to look at. */
export interface GenEnv {
  ctx: ScenarioContext;
  digest: ImportScreenDigest;
  /** Session-relative directory generated files are written under. */
  filesRel: string;
  /** Draw counter, so generated file names are unique within a run. */
  fileSeq: number;
  /**
   * Corpus entries this run may draw, by format. Empty for every format when no
   * corpus is configured, which is a supported configuration.
   *
   * Passed IN rather than discovered here: discovery walks a directory, and a
   * generator runs once per step.
   */
  corpus: Partial<Record<SurveyFileFormat, readonly CorpusEntry[]>>;
}

export interface Generator {
  name: string;
  weight: number;
  screens: readonly ScreenId[];
  build(env: GenEnv): FuzzMove | null;
}

// ---------------------------------------------------------------------------
// Draw helpers
// ---------------------------------------------------------------------------

function actionable(digest: ImportScreenDigest, testid: string): boolean {
  return digest.live[testid] === 2;
}

function presentButOff(digest: ImportScreenDigest, testid: string): boolean {
  return digest.live[testid] === 1;
}

/** Live ids matching `prefix<index>` (and nothing longer), sorted by index. */
function indexedIds(digest: ImportScreenDigest, prefix: string, onlyActionable = true): number[] {
  const out: number[] = [];
  for (const [testid, state] of Object.entries(digest.live)) {
    if (!testid.startsWith(prefix)) continue;
    if (onlyActionable && state !== 2) continue;
    const rest = testid.slice(prefix.length);
    if (!/^\d+$/.test(rest)) continue;
    out.push(Number(rest));
  }
  return out.sort((a, b) => a - b);
}

/** Rendered cells, as `[row, columnKey]`. */
function gridCells(digest: ImportScreenDigest): [number, string][] {
  const out: [number, string][] = [];
  for (const [testid, state] of Object.entries(digest.live)) {
    if (state !== 2 || !testid.startsWith('shot-editor-cell-')) continue;
    const match = /^shot-editor-cell-(\d{1,3})-([a-z_]{1,20})$/.exec(testid);
    if (!match) continue;
    out.push([Number(match[1]), match[2] as string]);
  }
  return out.sort((a, b) => a[0] - b[0] || a[1].localeCompare(b[1]));
}

function click(testid: string, label: string, double = false): FuzzMove {
  return double ? { kind: 'ui', testid, double: true, label } : { kind: 'ui', testid, label };
}

function type(testid: string, value: string, label: string): FuzzMove {
  return { kind: 'input', testid, value, mode: 'type', label };
}

function select(testid: string, value: string, label: string): FuzzMove {
  return { kind: 'input', testid, value, mode: 'select', label };
}

function check(testid: string, value: boolean, label: string): FuzzMove {
  return { kind: 'input', testid, value: value ? 'true' : 'false', mode: 'check', label };
}

// ---------------------------------------------------------------------------
// Hostile corpora
// ---------------------------------------------------------------------------

/**
 * Dates that are not dates, plus the two ends of the four-digit year range.
 * `<input type="date">` rejects most of them outright - which is the point: the
 * oracle asserts the app stays coherent, not that the value stuck.
 */
export const DATE_BOUNDARIES: readonly string[] = [
  '',
  '0000-01-01',
  '9999-12-31',
  '2024-02-30',
  'not-a-date',
];

/**
 * GPS strings the zero-datum parser has to survive.
 *
 * `'1e400'` is `Infinity` once parsed, `'-0'` is the negative zero that
 * `section_offsets`' dust rule turns on, `'91,181'` is out of range on both
 * axes, and the RLO/PDF pair is a bidi override that reorders whatever a table
 * cell renders after it. The 512-character entry looks for a length clamp.
 *
 * Exactly ONE entry here parses (`'0,0'`), which is the whole reason
 * `GPS_PASSABLE` exists - see its header.
 */
export const GPS_HOSTILE: readonly string[] = [
  '',
  '0,0',
  '91,181',
  '12 34 56 N',
  'NaN',
  '1e400',
  '-0',
  '48.8583N,2.2944E',
  '‮abc مرحبا‬',
  'x'.repeat(512),
];

/**
 * GPS strings that PARSE, so the zero-datum screen can actually be confirmed.
 *
 * `zero-datum-config-confirm` is `disabled={!gpsCoords}`
 * (`ZeroDatumConfigDialog.tsx`), and `gpsCoords` is set only by a `parseGps`
 * that succeeded inside `handleGpsInputChange`. With a purely hostile corpus the
 * confirm is disabled forever: the 130-cell corpus campaign spent 395 steps on
 * that screen and fired `zdc:confirm` ZERO times, so the DMP wizard's whole
 * zero-datum entry - the anchor import, the snap-fit, and every suggestion that
 * follows it - was never once reached. The advancing move
 * (`ADVANCE_MOVE['zero-datum-config']`) names the same disabled button, so the
 * screen could not be force-advanced either and burned to the escape budget
 * instead.
 *
 * Every entry below was verified against `src/utils/gps.ts`'s `parseGps` and is
 * still a boundary rather than a nominal value: `'0,0'` is null island and the
 * dust-rule zero, `'90,180'` is both axes at their limit, `'-0.0001,-0.0001'` is
 * just past zero on the negative side, and the DMS entry takes a different
 * parser branch from the DD ones.
 */
export const GPS_PASSABLE: readonly string[] = [
  '0,0',
  '90,180',
  '-0.0001,-0.0001',
  '20.4567,-87.0231',
  '48.8583, 2.2944',
  '-33.8688 151.2093',
  '19.5876,-88.0412',
  '20 12 34 N 87 27 24 W',
];

/**
 * Share of zero-datum GPS draws taken from `GPS_PASSABLE`.
 *
 * A floor, not a target: the hostile half still contains `'0,0'`, so the
 * effective share of draws that leave the confirm enabled is higher than this.
 * Read at CALL time like every other knob, so a replay's restored environment
 * reaches it, and clamped to `[0.5, 1]` - below a half the screen stops being
 * reliably completable, which is the exact failure this constant was added to
 * end.
 */
export function gpsPassableProbability(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P);
  if (!Number.isFinite(raw)) return 0.6;
  return Math.min(1, Math.max(0.5, raw));
}

/** Numeric strings a shot cell has to accept, clamp or reject without crashing. */
export const CELL_BOUNDARIES: readonly string[] = [
  '0',
  '-0',
  '1e3',
  '-1e3',
  '1e-9',
  'NaN',
  'Infinity',
  '1,5',
  '  7  ',
  '360',
  '-1',
  '',
];

/** Names/surveyors: half plain, half from the shared hostile-text corpus. */
export function biasedText(rng: Rng, seq: number): string {
  if (rng() < 0.5) return HOSTILE_COMMENTS[Math.floor(rng() * HOSTILE_COMMENTS.length)] as string;
  return `fuzz-${seq}-${Math.floor(rng() * 1000)}`;
}

function pickFrom<T>(rng: Rng, items: readonly T[]): T | null {
  if (items.length === 0) return null;
  return items[Math.floor(rng() * items.length)] as T;
}

// ---------------------------------------------------------------------------
// File sources
// ---------------------------------------------------------------------------

/** Tracked `.dmp` fixtures the flow starters may draw. */
export const DMP_FIXTURES: readonly string[] = [
  'kb5.dmp',
  'test_v2.dmp',
  'test_v5.dmp',
  'test_v5_buggy_EOS.dmp',
  'test_corrupted.dmp',
];

/**
 * Stand-in for the session's `fuzz-projects/` directory in a save-path move.
 *
 * The shell substitutes the real absolute path at dispatch time. A trace
 * therefore carries the PLACEHOLDER, never a path - so replaying one on another
 * machine cannot choose where this process creates a `.stalx`, the same rule
 * `fuzzTrace.ts` enforces for every `rel`.
 */
export const PROJECT_DIR_PLACEHOLDER = '<fuzz-projects>';

/** Share of generated files drawn from the corruption catalog. */
export function malformedProbability(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P);
  return Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : 0.2;
}

/**
 * Share of well-formed `.dmp` draws built as a SUGGESTION-BEARING file.
 *
 * `suggestiveDmpSpec` is the only generated shape whose sections overlap each
 * other closely enough for `suggest_all_import_placements` to return anything,
 * and therefore the only one that reaches `wiz-suggest` at all without a corpus
 * or a tracked fixture - see that function's header for the measurement.
 * Applies to the well-formed branch only: a corrupted file is drawn to break the
 * parser, and a corrupted overlap is just a file that does not parse.
 */
export function suggestiveProbability(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_SUGGESTIVE_P);
  return Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : 0.5;
}

/** Default share of drawn files taken from the corpus, when one is configured. */
export const CORPUS_DRAW_P = 0.25;

/**
 * Share of drawn files taken from the corpus, from
 * `STALAGMIGHT_FUZZ_CORPUS_DRAW_P`.
 *
 * At 1.0 a run imports NOTHING but real files, which is the only way to fuzz
 * against survey data whose sections genuinely overlap without generating the
 * overlap first. It is still a share and not a switch: with no corpus
 * configured, or with none of this run's enabled formats present in it, the draw
 * falls through to a generated file and counts `corpus:absent`, which is a
 * narrower run and not a failed one.
 *
 * Read at CALL time like every other knob, clamped to `[0, 1]`, and carried in a
 * trace header by the `STALAGMIGHT_FUZZ_` prefix rule (`fuzzTrace.ts`) - unlike
 * `STALAGMIGHT_FUZZ_CORPUS_DIR`, which names a directory tree and is on the
 * never-restored list for exactly that reason. A replay therefore reproduces the
 * SHARE while its own operator still chooses the corpus.
 */
export function corpusDrawProbability(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P);
  return Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : CORPUS_DRAW_P;
}
/** Share of DMP draws taken from a tracked fixture. */
const FIXTURE_DRAW_P = 0.25;

/**
 * Format families this run may draw, from `STALAGMIGHT_FUZZ_IMPORT_FORMAT`.
 *
 * A csv of family names; unknown entries are dropped, and an empty or unset
 * value means all five. Read at CALL time, like every other knob, so a replay's
 * restored environment reaches it.
 */
export function enabledFamilies(): readonly SurveyFormatFamily[] {
  const raw = (process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT ?? '').trim();
  if (raw === '') return SURVEY_FORMAT_FAMILIES;
  const wanted = raw
    .split(',')
    .map((entry) => entry.trim().toLowerCase())
    .filter((entry) => entry !== '');
  const chosen = SURVEY_FORMAT_FAMILIES.filter((family) => wanted.includes(family));
  return chosen.length > 0 ? chosen : SURVEY_FORMAT_FAMILIES;
}

/** Every concrete extension the enabled families cover. */
export function enabledFormats(): SurveyFileFormat[] {
  const out: SurveyFileFormat[] = [];
  for (const family of enabledFamilies()) out.push(...FORMATS_IN_FAMILY[family]);
  return out;
}

/**
 * Draw one input file: a generated spec (valid-biased or corrupted), a tracked
 * fixture, or a private-corpus entry.
 *
 * The corpus is drawn FIRST, and the format is chosen from the INTERSECTION of
 * "formats this run may use" and "formats the corpus actually has entries for".
 * Both halves matter. The enabled set is what stops a
 * `--axis STALAGMIGHT_FUZZ_IMPORT_FORMAT=gpx` cell quietly importing somebody's
 * `.srv` collection; the corpus-bearing set is what stops a corpus-only campaign
 * spending draws on synthetic input. Picking the format FIRST and only then
 * looking for a corpus - which is what this did - meant an enabled format the
 * corpus has nothing for fell straight through to a generated spec even at
 * `STALAGMIGHT_FUZZ_CORPUS_DRAW_P=1`. Measured: a `FORMAT=dmp,tml` run over a
 * corpus of 11 `.dmp` and one `.tml` enables `dmp,tml,tmlu`, and every `tmlu`
 * draw - a third of them - took the synthetic path silently.
 *
 * When the intersection is EMPTY the draw falls through to a generated file,
 * which is a narrower run and not a failed one; the shell counts `corpus:absent`
 * for it once at startup and says so loudly if the run asked for corpus-only.
 */
export function drawFileSource(env: GenEnv): FileSource {
  const rng = () => env.ctx.rng();
  const enabled = enabledFormats();
  // The intersection, computed BEFORE the dice: a format the corpus cannot serve
  // must not be able to win a corpus draw and then quietly fall through.
  const corpusFormats = enabled.filter((candidate) => (env.corpus[candidate]?.length ?? 0) > 0);

  if (corpusFormats.length > 0 && rng() < corpusDrawProbability()) {
    const format = pickFrom(rng, corpusFormats) as SurveyFileFormat;
    const corpus = env.corpus[format] as readonly CorpusEntry[];
    const entry = pickFrom(rng, corpus) as CorpusEntry;
    const dirRel = `${env.filesRel}/corpus-${env.fileSeq}`;
    // `relPath` is POSIX by construction (`discoverCorpus` joins on '/').
    const base = entry.relPath.slice(entry.relPath.lastIndexOf('/') + 1);
    return {
      origin: 'corpus',
      corpusRel: entry.relPath,
      id: entry.id,
      hash: entry.pathHash,
      dirRel,
      rel: `${dirRel}/${base}`,
    };
  }

  const format = pickFrom(rng, enabled) as SurveyFileFormat;

  // Tracked fixtures are all `.dmp`; there are none for the other formats, and
  // the repo forbids adding any.
  if (format === 'dmp' && rng() < FIXTURE_DRAW_P) {
    const name = pickFrom(rng, DMP_FIXTURES) as string;
    return { origin: 'fixture', name, rel: `fixtures/${name}` };
  }

  const malformed = rng() < malformedProbability();
  const profile = malformed ? 'malformed' : 'valid';
  // Drawn BEFORE the ordinary spec so the suggestive branch does not consume a
  // whole `randomSurveyFileSpec` it then throws away.
  if (!malformed && format === 'dmp' && rng() < suggestiveProbability()) {
    const dmp = suggestiveDmpSpec(rng);
    const spec: GeneratedFileSpec = { format: 'dmp', name: `fuzz-${env.fileSeq}-suggestive.dmp`, profile: 'valid', dmp };
    return { origin: 'gen', spec, rel: `${env.filesRel}/${spec.name}`, shape: 'suggestive' };
  }
  const clean = randomSurveyFileSpec(rng, profile, format);
  let spec: GeneratedFileSpec = { ...clean, name: `fuzz-${env.fileSeq}.${format}` };
  if (malformed) {
    const corrupted = corruptSpec(rng, clean);
    // The catalog id is `<format>:<entry>`; a colon is a legal Linux filename
    // byte and an illegal Windows one, and a trace is meant to be portable.
    const tag = corrupted.corruption.replace(/[^a-z0-9]+/gi, '-');
    spec = { ...corrupted.spec, name: `fuzz-${env.fileSeq}-${tag}.${format}` };
  }
  return { origin: 'gen', spec, rel: `${env.filesRel}/${spec.name}` };
}

/** How a source is named in a log line, a metric key or a failure payload. */
export function describeSource(source: FileSource): string {
  if (source.origin === 'gen') {
    const shape = source.shape === undefined ? '' : ` (${source.shape})`;
    return `${source.spec.profile} ${source.spec.format}${shape}`;
  }
  if (source.origin === 'fixture') return `fixture ${source.name}`;
  if (source.origin === 'corpus') return `corpus #${source.id}`;
  return `disk ${source.rel}`;
}

/** The publishable id of a corpus-relative path. Re-exported for the shell. */
export { corpusFileId };

// ---------------------------------------------------------------------------
// The catalog
// ---------------------------------------------------------------------------

/**
 * Flow starters. Only legal on `idle`.
 *
 * With NO project open a `.dmp` routes to `new-project-for-import` rather than
 * to the selection dialog, so the same starters exercise
 * `importContext='new-project'` and its deferred initial save. That is not a
 * separate generator: it falls out of whether a project happens to be open.
 */
const STARTERS: Generator[] = [
  {
    name: 'start:picker',
    weight: 10,
    screens: ['idle'],
    build: (env) => {
      const trigger = actionable(env.digest, 'toolbar-import-button')
        ? 'toolbar-import-button'
        : actionable(env.digest, 'global-open-import-button')
          ? 'global-open-import-button'
          : null;
      if (trigger === null) return null;
      const source = drawFileSource(env);
      return { kind: 'openFile', source, via: 'dialog', trigger, label: `import via ${trigger}` };
    },
  },
  {
    name: 'start:route',
    weight: 6,
    screens: ['idle'],
    build: (env) => {
      const source = drawFileSource(env);
      return { kind: 'openFile', source, via: 'route', trigger: 'routeOpenedFile', label: 'import via router' };
    },
  },
  {
    name: 'start:cancel-picker',
    weight: 2,
    screens: ['idle'],
    build: (env) => {
      const trigger = actionable(env.digest, 'toolbar-import-button')
        ? 'toolbar-import-button'
        : actionable(env.digest, 'global-open-import-button')
          ? 'global-open-import-button'
          : null;
      if (trigger === null) return null;
      return { kind: 'cancelDialog', trigger, label: `cancel the picker on ${trigger}` };
    },
  },
];

const DMP_SELECT: Generator[] = [
  {
    name: 'dmp:toggle-section',
    weight: 12,
    screens: ['dmp-select'],
    build: (env) => {
      const index = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'section-checkbox-'));
      if (index === null) return null;
      const testid = `section-checkbox-${index}`;
      const want = env.ctx.rng() < 0.5;
      return check(testid, want, `set ${testid}=${want}`);
    },
  },
  {
    name: 'dmp:select-all',
    weight: 3,
    screens: ['dmp-select'],
    build: (env) => (actionable(env.digest, 'select-all-button') ? click('select-all-button', 'select all') : null),
  },
  {
    name: 'dmp:select-none',
    weight: 3,
    screens: ['dmp-select'],
    build: (env) => (actionable(env.digest, 'select-none-button') ? click('select-none-button', 'select none') : null),
  },
  {
    name: 'dmp:invert',
    weight: 3,
    screens: ['dmp-select'],
    build: (env) =>
      actionable(env.digest, 'invert-selection-button') ? click('invert-selection-button', 'invert selection') : null,
  },
  {
    name: 'dmp:surveyor',
    weight: 5,
    screens: ['dmp-select'],
    build: (env) => {
      if (!actionable(env.digest, 'default-surveyor-input')) return null;
      const value = biasedText(() => env.ctx.rng(), env.fileSeq);
      return type('default-surveyor-input', value, `default surveyor=${JSON.stringify(value.slice(0, 24))}`);
    },
  },
  {
    name: 'dmp:date',
    weight: 5,
    screens: ['dmp-select'],
    build: (env) => {
      if (!actionable(env.digest, 'default-date-input')) return null;
      const value = pickFrom(() => env.ctx.rng(), DATE_BOUNDARIES) as string;
      return type('default-date-input', value, `default date=${JSON.stringify(value)}`);
    },
  },
  {
    name: 'dmp:continue',
    weight: 14,
    screens: ['dmp-select'],
    build: (env) => (actionable(env.digest, 'continue-button') ? click('continue-button', 'continue') : null),
  },
  {
    name: 'dmp:cancel',
    weight: 2,
    screens: ['dmp-select'],
    build: (env) => (actionable(env.digest, 'cancel-button') ? click('cancel-button', 'cancel selection') : null),
  },
  {
    /**
     * The ONLY generator that deliberately targets a control the digest says is
     * NOT actionable.
     *
     * `continue-button` is disabled while the duplicate-hash and section-hash
     * probes are still in flight, and that window is short and real. Clicking
     * into it asserts the disabled state is HONEST: a disabled control must not
     * advance the flow. The shell checks that inline (the screen must not have
     * changed); everything else about the move is an ordinary click.
     */
    name: 'dmp:race-continue',
    weight: 6,
    screens: ['dmp-select'],
    build: (env) =>
      presentButOff(env.digest, 'continue-button')
        ? click('continue-button', 'RACE: continue while disabled')
        : null,
  },
];

/**
 * The four non-DMP options screens, plus the GPX placement screen and the TML
 * residue choice.
 *
 * Each of these is ONE screen with a handful of controls, so the catalog is
 * flat rather than layered the way the DMP wizard's is. What they have in
 * common with `dmp-select` is a confirm that is DISABLED while a duplicate-hash
 * probe is in flight, which is why each has a `race-*` entry: `RACE_TARGETS` in
 * `importDialogFuzz.ts` asserts that none of them advances while disabled.
 */
const TML_CONFIG: Generator[] = [
  {
    name: 'tml:include-excluded',
    weight: 5,
    screens: ['tml-config'],
    build: (env) => {
      if (!actionable(env.digest, 'tml-include-excluded')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('tml-include-excluded', want, `tml include excluded=${want}`);
    },
  },
  {
    name: 'tml:comments-on-shots',
    weight: 5,
    screens: ['tml-config'],
    build: (env) => {
      if (!actionable(env.digest, 'tml-comments-on-shots')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('tml-comments-on-shots', want, `tml comments on shots=${want}`);
    },
  },
  {
    name: 'tml:virtual-mode',
    weight: 5,
    screens: ['tml-config'],
    build: (env) => {
      if (!actionable(env.digest, 'tml-virtual-shot-mode')) return null;
      const mode = pickFrom(() => env.ctx.rng(), VIRTUAL_SHOT_MODES) as string;
      return select('tml-virtual-shot-mode', mode, `tml virtual shot mode ${mode}`);
    },
  },
  {
    name: 'tml:confirm',
    weight: 14,
    screens: ['tml-config'],
    build: (env) => (actionable(env.digest, 'tml-import-confirm') ? click('tml-import-confirm', 'tml import') : null),
  },
  {
    name: 'tml:cancel',
    weight: 3,
    screens: ['tml-config'],
    build: (env) => (actionable(env.digest, 'tml-import-cancel') ? click('tml-import-cancel', 'tml cancel') : null),
  },
  {
    name: 'tml:race-confirm',
    weight: 4,
    screens: ['tml-config'],
    build: (env) =>
      presentButOff(env.digest, 'tml-import-confirm')
        ? click('tml-import-confirm', 'RACE: tml import while disabled')
        : null,
  },
  {
    /**
     * KEEP is what an escape plays, so the generator weights DISCARD higher:
     * discarding is the branch that re-checks the project's `created_at`
     * against the one the residue captured and then deletes the project the
     * import created, and it is only reachable from here.
     */
    name: 'residue:discard',
    weight: 10,
    screens: ['tml-residue'],
    build: (env) =>
      actionable(env.digest, 'tml-import-residue-discard')
        ? click('tml-import-residue-discard', 'discard the import project')
        : null,
  },
  {
    name: 'residue:keep',
    weight: 6,
    screens: ['tml-residue'],
    build: (env) =>
      actionable(env.digest, 'tml-import-residue-keep')
        ? click('tml-import-residue-keep', 'keep the import project')
        : null,
  },
];

const COMPASS: Generator[] = [
  {
    name: 'compass:apply-corrections',
    weight: 5,
    screens: ['compass'],
    build: (env) => {
      if (!actionable(env.digest, 'compass-apply-corrections')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('compass-apply-corrections', want, `compass apply corrections=${want}`);
    },
  },
  {
    name: 'compass:comments-on-stations',
    weight: 5,
    screens: ['compass'],
    build: (env) => {
      if (!actionable(env.digest, 'compass-comments-on-stations')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('compass-comments-on-stations', want, `compass comments on stations=${want}`);
    },
  },
  {
    name: 'compass:include-excluded',
    weight: 5,
    screens: ['compass'],
    build: (env) => {
      if (!actionable(env.digest, 'compass-include-excluded')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('compass-include-excluded', want, `compass include excluded=${want}`);
    },
  },
  {
    name: 'compass:confirm',
    weight: 14,
    screens: ['compass'],
    build: (env) =>
      actionable(env.digest, 'compass-import-confirm') ? click('compass-import-confirm', 'compass import') : null,
  },
  {
    name: 'compass:cancel',
    weight: 3,
    screens: ['compass'],
    build: (env) =>
      actionable(env.digest, 'compass-import-cancel') ? click('compass-import-cancel', 'compass cancel') : null,
  },
  {
    name: 'compass:race-confirm',
    weight: 5,
    screens: ['compass'],
    build: (env) =>
      presentButOff(env.digest, 'compass-import-confirm')
        ? click('compass-import-confirm', 'RACE: compass import while disabled')
        : null,
  },
];

const WALLS: Generator[] = [
  {
    name: 'walls:toggle-survey',
    weight: 12,
    screens: ['walls'],
    build: (env) => {
      const index = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'walls-survey-'));
      if (index === null) return null;
      const testid = `walls-survey-${index}`;
      const want = env.ctx.rng() < 0.5;
      return check(testid, want, `set ${testid}=${want}`);
    },
  },
  {
    name: 'walls:select-all',
    weight: 5,
    screens: ['walls'],
    build: (env) => {
      if (!actionable(env.digest, 'walls-select-all')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('walls-select-all', want, `walls select all=${want}`);
    },
  },
  {
    name: 'walls:comments-on-stations',
    weight: 4,
    screens: ['walls'],
    build: (env) => {
      if (!actionable(env.digest, 'walls-comments-on-stations')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('walls-comments-on-stations', want, `walls comments on stations=${want}`);
    },
  },
  {
    /**
     * The one Walls control with a GATE behind it: a selection whose only
     * importable content is splays leaves the confirm disabled until this is
     * on (`WallsImportDialog.tsx`), and the valid-biased spec generator emits
     * splay-only surveys on purpose so that gate is reachable.
     */
    name: 'walls:include-splays',
    weight: 8,
    screens: ['walls'],
    build: (env) => {
      if (!actionable(env.digest, 'walls-include-splays')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('walls-include-splays', want, `walls include splays=${want}`);
    },
  },
  {
    name: 'walls:confirm',
    weight: 14,
    screens: ['walls'],
    build: (env) =>
      actionable(env.digest, 'walls-import-confirm') ? click('walls-import-confirm', 'walls import') : null,
  },
  {
    name: 'walls:cancel',
    weight: 3,
    screens: ['walls'],
    build: (env) =>
      actionable(env.digest, 'walls-import-cancel') ? click('walls-import-cancel', 'walls cancel') : null,
  },
  {
    name: 'walls:race-confirm',
    weight: 5,
    screens: ['walls'],
    build: (env) =>
      presentButOff(env.digest, 'walls-import-confirm')
        ? click('walls-import-confirm', 'RACE: walls import while disabled')
        : null,
  },
];

const GPX: Generator[] = [
  {
    name: 'gpx:toggle-collection',
    weight: 14,
    screens: ['gpx-select'],
    build: (env) => {
      const candidates = ['gpx-import-waypoints', 'gpx-import-tracks', 'gpx-import-routes'].filter((testid) =>
        actionable(env.digest, testid),
      );
      const testid = pickFrom(() => env.ctx.rng(), candidates);
      if (testid === null) return null;
      const want = env.ctx.rng() < 0.5;
      return check(testid, want, `set ${testid}=${want}`);
    },
  },
  {
    name: 'gpx:depth-inverted',
    weight: 5,
    screens: ['gpx-select'],
    build: (env) => {
      if (!actionable(env.digest, 'gpx-depth-inverted')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('gpx-depth-inverted', want, `gpx depth inverted=${want}`);
    },
  },
  {
    name: 'gpx:expand',
    weight: 3,
    screens: ['gpx-select'],
    build: (env) => {
      const candidates = ['gpx-waypoints-toggle', 'gpx-tracks-toggle', 'gpx-routes-toggle'].filter((testid) =>
        actionable(env.digest, testid),
      );
      const testid = pickFrom(() => env.ctx.rng(), candidates);
      return testid === null ? null : click(testid, `expand ${testid}`);
    },
  },
  {
    /**
     * The footer's primary is `gpx-import-confirm` when ONLY waypoints are
     * selected (there is nothing to place, so the flow finishes here) and
     * `gpx-continue-button` otherwise. They are opposite branches of one
     * ternary, so exactly one of them exists at a time.
     */
    name: 'gpx:advance',
    weight: 14,
    screens: ['gpx-select'],
    build: (env) => {
      const testid = actionable(env.digest, 'gpx-continue-button')
        ? 'gpx-continue-button'
        : actionable(env.digest, 'gpx-import-confirm')
          ? 'gpx-import-confirm'
          : null;
      return testid === null ? null : click(testid, `gpx advance via ${testid}`);
    },
  },
  {
    name: 'gpx:cancel',
    weight: 3,
    screens: ['gpx-select'],
    build: (env) =>
      actionable(env.digest, 'gpx-selection-cancel') ? click('gpx-selection-cancel', 'gpx cancel') : null,
  },
  {
    name: 'gpx:race-advance',
    weight: 5,
    screens: ['gpx-select'],
    build: (env) => {
      const testid = presentButOff(env.digest, 'gpx-continue-button')
        ? 'gpx-continue-button'
        : presentButOff(env.digest, 'gpx-import-confirm')
          ? 'gpx-import-confirm'
          : null;
      return testid === null ? null : click(testid, `RACE: gpx advance on a disabled ${testid}`);
    },
  },
  {
    /**
     * The zero-datum picker for one track/route. Its options are the point
     * indices of THAT section, which the digest cannot report for a dynamic id
     * (it reads options only for the statically named selects), so the index is
     * drawn blind and an option the section does not have is skipped-and-counted
     * like any other impossible move.
     */
    name: 'gpxp:zero-datum',
    weight: 10,
    screens: ['gpx-place'],
    build: (env) => {
      const section = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'gpx-placement-zero-datum-'));
      if (section === null) return null;
      const point = env.ctx.randInt(0, 7);
      return select(`gpx-placement-zero-datum-${section}`, String(point), `gpx section ${section} zero datum ${point}`);
    },
  },
  {
    name: 'gpxp:confirm',
    weight: 12,
    screens: ['gpx-place'],
    build: (env) =>
      actionable(env.digest, 'gpx-import-confirm') ? click('gpx-import-confirm', 'gpx placement import') : null,
  },
  {
    name: 'gpxp:back',
    weight: 5,
    screens: ['gpx-place'],
    build: (env) =>
      actionable(env.digest, 'gpx-placement-back') ? click('gpx-placement-back', 'gpx placement back') : null,
  },
  {
    name: 'gpxp:cancel',
    weight: 3,
    screens: ['gpx-place'],
    build: (env) =>
      actionable(env.digest, 'gpx-placement-cancel') ? click('gpx-placement-cancel', 'gpx placement cancel') : null,
  },
];

/**
 * Orphan placement, reachable after ANY import that lands more than one
 * component.
 *
 * `OrphanPlacementDialog` renders the same suggestion controls the DMP wizard
 * does, and the generators are separate rather than shared so their weights can
 * differ from the wizard's without rewriting that screen's trajectory.
 */
const ORPHAN: Generator[] = [
  {
    name: 'orphan:merge-station',
    weight: 5,
    screens: ['orphan-placement'],
    build: (env) => {
      if (!actionable(env.digest, 'merge-station-select')) return null;
      const options = env.digest.opts['merge-station-select'];
      const value =
        options && options.length > 0 ? pickFrom(() => env.ctx.rng(), options) : String(env.ctx.randInt(-2, 2));
      if (value === null) return null;
      return select('merge-station-select', value, `orphan merge station offset ${value}`);
    },
  },
  {
    // One repeated shot's choice (#681). A disabled option (doubly-anchored
    // target) is refused by the bridge as `option-absent`, which is the guard.
    name: 'orphan:overlap-choice',
    weight: 4,
    screens: ['orphan-placement'],
    build: (env) => {
      const index = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'overlap-choice-'));
      if (index === null) return null;
      const choice = pickFrom(() => env.ctx.rng(), ORPHAN_OVERLAP_CHOICES) as string;
      return select(`overlap-choice-${index}`, choice, `orphan overlap shot ${index} ${choice}`);
    },
  },
  {
    name: 'orphan:overlap-set-all',
    weight: 3,
    screens: ['orphan-placement'],
    build: (env) => {
      const live = [
        'overlap-set-all-keep-existing',
        'overlap-set-all-use-new',
        'overlap-set-all-average',
      ].filter((testid) => actionable(env.digest, testid));
      const testid = pickFrom(() => env.ctx.rng(), live);
      if (testid === null) return null;
      return click(testid, `orphan overlap ${testid}`);
    },
  },
  {
    name: 'orphan:toggle-alternatives',
    weight: 4,
    screens: ['orphan-placement'],
    build: (env) =>
      actionable(env.digest, 'toggle-alternatives') ? click('toggle-alternatives', 'orphan alternatives') : null,
  },
  {
    name: 'orphan:alternative',
    weight: 5,
    screens: ['orphan-placement'],
    build: (env) => {
      const index = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'alternative-'));
      if (index === null) return null;
      return click(`alternative-${index}`, `orphan alternative ${index}`);
    },
  },
];

/** The export modal, opened by a Ctrl+E interjection. Its contents are not fuzzed. */
const EXPORT_MODAL: Generator[] = [
  {
    name: 'export:close',
    weight: 10,
    screens: ['export-modal'],
    build: (env) =>
      actionable(env.digest, 'export-dialog-close') ? click('export-dialog-close', 'close the export modal') : null,
  },
  {
    /**
     * The unsaved-changes prompt, cancelled and nothing else.
     *
     * A non-DMP import raises it on the way in: `import-as-new-project`'s
     * confirm closes the current project first, and `handleCloseProject`
     * prompts when that project is dirty. `unsaved-changes-discard` is
     * forbidden (it quits the app on the quit path and throws away work with no
     * undo entry on the others), so cancelling - which abandons whatever raised
     * the prompt - is the only move here.
     */
    name: 'unsaved:cancel',
    weight: 10,
    screens: ['unsaved-changes'],
    build: (env) =>
      actionable(env.digest, 'unsaved-changes-cancel')
        ? click('unsaved-changes-cancel', 'cancel the unsaved-changes prompt')
        : null,
  },
];

const ZERO_DATUM: Generator[] = [
  {
    name: 'zd:pick-section',
    weight: 8,
    screens: ['zero-datum-select'],
    build: (env) => {
      const index = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'zero-datum-section-card-'));
      if (index === null) return null;
      return click(`zero-datum-section-card-${index}`, `zero datum on section ${index}`);
    },
  },
  {
    name: 'zd:skip',
    weight: 6,
    screens: ['zero-datum-select'],
    build: (env) =>
      actionable(env.digest, 'zero-datum-selection-skip') ? click('zero-datum-selection-skip', 'skip zero datum') : null,
  },
  {
    name: 'zd:back',
    weight: 3,
    screens: ['zero-datum-select'],
    build: (env) =>
      actionable(env.digest, 'zero-datum-selection-back') ? click('zero-datum-selection-back', 'back') : null,
  },
  {
    name: 'zd:cancel',
    weight: 2,
    screens: ['zero-datum-select'],
    build: (env) =>
      actionable(env.digest, 'zero-datum-selection-cancel') ? click('zero-datum-selection-cancel', 'cancel') : null,
  },
];

const ZERO_DATUM_CONFIG: Generator[] = [
  {
    name: 'zdc:station',
    weight: 6,
    screens: ['zero-datum-config'],
    build: (env) => {
      if (!actionable(env.digest, 'zero-datum-station-select')) return null;
      // Station count is shots+1; `rows` is the rendered grid window, which is
      // the honest upper bound available without opening the listbox. An index
      // the select does not offer is skipped-and-counted by the bridge.
      const bound = Math.max(1, env.digest.rows);
      const index = Math.floor(env.ctx.rng() * bound);
      return select('zero-datum-station-select', String(index), `zero-datum station ${index}`);
    },
  },
  {
    /**
     * UNBLOCK the confirm: type a GPS that parses, but only while the confirm is
     * present and DISABLED.
     *
     * The passable share alone is not enough on its own. `zero-datum-config` has
     * a 125-weight pool (the shot table and the metadata form are both mounted
     * on it), so `zdc:gps` is about 6% of draws - and a visit is 8 steps before
     * the walk force-advances and 16 before it escapes. Measured on a pre-flight
     * cell: 17 steps on the screen, no GPS typed, `forcedAdvanceUnavailable`
     * eight times. The screen's ONLY exit forward is gated on a field, so the
     * walk needs a move that aims at the gate rather than at the screen.
     *
     * Aimed with `presentButOff`, so it cannot fire once the confirm is enabled
     * and cannot dilute anything else. `zdc:gps` keeps the hostile corpus and
     * keeps its own weight: the point is to reach what is BEHIND the screen as
     * well as to hammer the screen.
     *
     * The weight is high for the same structural reason, not for taste: a visit
     * is 8 steps before the walk force-advances, and every step spent before the
     * gate opens is a step the forced advance cannot use. At 20 of a ~145 pool
     * the gate opens inside about four steps; the generator then stops building
     * and costs nothing but a redraw.
     */
    name: 'zdc:gps-unblock',
    weight: 20,
    screens: ['zero-datum-config'],
    build: (env) => {
      if (!presentButOff(env.digest, 'zero-datum-config-confirm')) return null;
      if (!actionable(env.digest, 'zero-datum-gps-input')) return null;
      const value = pickFrom(() => env.ctx.rng(), GPS_PASSABLE) as string;
      return type('zero-datum-gps-input', value, `gps (unblock)=${JSON.stringify(value.slice(0, 24))}`);
    },
  },
  {
    /**
     * Type a GPS position. At least half the draws PARSE, so the confirm below
     * is reachable - see `GPS_PASSABLE`.
     *
     * Two rng calls, always in the same order, so the share can be retuned
     * without shifting which string a given draw lands on relative to the rest
     * of the stream.
     */
    name: 'zdc:gps',
    weight: 8,
    screens: ['zero-datum-config'],
    build: (env) => {
      if (!actionable(env.digest, 'zero-datum-gps-input')) return null;
      const passable = env.ctx.rng() < gpsPassableProbability();
      const value = pickFrom(() => env.ctx.rng(), passable ? GPS_PASSABLE : GPS_HOSTILE) as string;
      return type('zero-datum-gps-input', value, `gps=${JSON.stringify(value.slice(0, 24))}`);
    },
  },
  {
    name: 'zdc:name',
    weight: 5,
    screens: ['zero-datum-config'],
    build: (env) => {
      if (!actionable(env.digest, 'section-name-input')) return null;
      const value = biasedText(() => env.ctx.rng(), env.fileSeq);
      return type('section-name-input', value, `section name=${JSON.stringify(value.slice(0, 24))}`);
    },
  },
  {
    name: 'zdc:confirm',
    weight: 12,
    screens: ['zero-datum-config'],
    build: (env) =>
      actionable(env.digest, 'zero-datum-config-confirm') ? click('zero-datum-config-confirm', 'confirm zero datum') : null,
  },
  {
    name: 'zdc:back',
    weight: 4,
    screens: ['zero-datum-config'],
    build: (env) => (actionable(env.digest, 'zero-datum-config-back') ? click('zero-datum-config-back', 'back') : null),
  },
  {
    name: 'zdc:cancel',
    weight: 2,
    screens: ['zero-datum-config'],
    build: (env) =>
      actionable(env.digest, 'zero-datum-config-cancel') ? click('zero-datum-config-cancel', 'cancel') : null,
  },
  {
    // Only actionable once `zdc:back` has raised the dialog's own Back-discard
    // confirm (dirty state) - accepts it, actually leaving for zero-datum-select.
    name: 'zdc:back-confirm-discard',
    weight: 3,
    screens: ['zero-datum-config'],
    build: (env) =>
      actionable(env.digest, 'zero-datum-config-back-confirm-discard')
        ? click('zero-datum-config-back-confirm-discard', 'back (discard)')
        : null,
  },
  {
    // The other half: dismiss the Back-discard confirm and stay put.
    name: 'zdc:back-confirm-cancel',
    weight: 3,
    screens: ['zero-datum-config'],
    build: (env) =>
      actionable(env.digest, 'zero-datum-config-back-confirm-cancel')
        ? click('zero-datum-config-back-confirm-cancel', 'back (keep editing)')
        : null,
  },
];

const WIZARD_SUGGEST: Generator[] = [
  {
    name: 'wiz:confirm',
    weight: 14,
    screens: ['wiz-suggest'],
    build: (env) => (actionable(env.digest, 'confirm-button') ? click('confirm-button', 'accept suggestion') : null),
  },
  {
    name: 'wiz:skip',
    weight: 6,
    screens: ['wiz-suggest'],
    build: (env) => (actionable(env.digest, 'skip-button') ? click('skip-button', 'skip suggestion') : null),
  },
  {
    name: 'wiz:toggle-alternatives',
    weight: 4,
    screens: ['wiz-suggest'],
    build: (env) =>
      actionable(env.digest, 'toggle-alternatives') ? click('toggle-alternatives', 'toggle alternatives') : null,
  },
  {
    name: 'wiz:alternative',
    weight: 5,
    screens: ['wiz-suggest'],
    build: (env) => {
      const index = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'alternative-'));
      if (index === null) return null;
      return click(`alternative-${index}`, `alternative ${index}`);
    },
  },
  {
    name: 'wiz:back',
    weight: 3,
    // `wiz-no-suggest` is deliberately NOT here - the bridge's settle predicate
    // now waits through that frame, because it is a sub-paint transition into
    // `wiz-manual` and not a screen. See `busy()` in importDialogBridge.ts.
    screens: ['wiz-suggest', 'wiz-manual'],
    build: (env) => (actionable(env.digest, 'back-button') ? click('back-button', 'wizard back') : null),
  },
  {
    name: 'wiz:overlap-mode',
    weight: 5,
    screens: ['wiz-suggest'],
    build: (env) => {
      if (!actionable(env.digest, 'overlap-mode-select')) return null;
      const mode = pickFrom(() => env.ctx.rng(), OVERLAP_MODES) as string;
      return select('overlap-mode-select', mode, `overlap mode ${mode}`);
    },
  },
  {
    name: 'wiz:merge-station',
    weight: 4,
    screens: ['wiz-suggest'],
    build: (env) => {
      if (!actionable(env.digest, 'merge-station-select')) return null;
      // The offsets are computed per suggestion, so the readable list is the
      // only source of truth; blind draws would be skips almost every time.
      const options = env.digest.opts['merge-station-select'];
      const value = options && options.length > 0 ? pickFrom(() => env.ctx.rng(), options) : String(env.ctx.randInt(-2, 2));
      if (value === null) return null;
      return select('merge-station-select', value, `merge station offset ${value}`);
    },
  },
  {
    name: 'wiz:cancel',
    weight: 2,
    screens: ['wiz-suggest', 'wiz-manual', 'wiz-done', 'wiz-error'],
    build: (env) => (actionable(env.digest, 'cancel-button') ? click('cancel-button', 'wizard cancel') : null),
  },
];

/** Mid-import metadata edits, legal wherever a SectionMetadataForm is mounted. */
const METADATA_SCREENS: readonly ScreenId[] = ['zero-datum-config', 'wiz-suggest', 'wiz-manual'];

const METADATA: Generator[] = [
  {
    name: 'meta:name',
    weight: 4,
    screens: ['wiz-suggest', 'wiz-manual'],
    build: (env) => {
      if (!actionable(env.digest, 'section-name-input')) return null;
      const value = biasedText(() => env.ctx.rng(), env.fileSeq);
      return type('section-name-input', value, `name=${JSON.stringify(value.slice(0, 24))}`);
    },
  },
  {
    name: 'meta:surveyor',
    weight: 3,
    screens: METADATA_SCREENS,
    build: (env) => {
      if (!actionable(env.digest, 'section-surveyor-input')) return null;
      const value = biasedText(() => env.ctx.rng(), env.fileSeq);
      return type('section-surveyor-input', value, `surveyor=${JSON.stringify(value.slice(0, 24))}`);
    },
  },
  {
    name: 'meta:date',
    weight: 3,
    screens: METADATA_SCREENS,
    build: (env) => {
      if (!actionable(env.digest, 'section-date-input')) return null;
      const value = pickFrom(() => env.ctx.rng(), DATE_BOUNDARIES) as string;
      return type('section-date-input', value, `date=${JSON.stringify(value)}`);
    },
  },
  {
    name: 'meta:direction',
    weight: 3,
    screens: METADATA_SCREENS,
    build: (env) => {
      if (!actionable(env.digest, 'section-direction-select')) return null;
      const value = env.ctx.rng() < 0.5 ? 'in' : 'out';
      return select('section-direction-select', value, `direction ${value}`);
    },
  },
  {
    name: 'meta:quality',
    weight: 2,
    screens: METADATA_SCREENS,
    build: (env) => {
      if (!actionable(env.digest, 'section-quality-input')) return null;
      const value = String(env.ctx.randInt(-1, 6));
      return type('section-quality-input', value, `quality=${value}`);
    },
  },
  {
    name: 'meta:color',
    weight: 2,
    screens: METADATA_SCREENS,
    build: (env) => {
      if (!actionable(env.digest, 'section-color-hex-input')) return null;
      const value = pickFrom(() => env.ctx.rng(), ['#000000', '#ffffff', 'not-a-colour', '#12345', '#ABCDEF']) as string;
      return type('section-color-hex-input', value, `colour=${value}`);
    },
  },
  {
    name: 'meta:reverse',
    weight: 5,
    screens: METADATA_SCREENS,
    build: (env) => {
      if (!actionable(env.digest, 'section-reverse-checkbox')) return null;
      const want = env.ctx.rng() < 0.5;
      return check('section-reverse-checkbox', want, `reverse=${want}`);
    },
  },
];

const WIZARD_MANUAL: Generator[] = [
  {
    name: 'man:mode-suggestion',
    weight: 4,
    screens: ['wiz-manual'],
    build: (env) =>
      actionable(env.digest, 'segment-mode-suggestion-input')
        ? check('segment-mode-suggestion-input', true, 'segment mode: suggestion')
        : null,
  },
  {
    name: 'man:mode-from-existing',
    weight: 5,
    screens: ['wiz-manual'],
    build: (env) =>
      actionable(env.digest, 'segment-mode-from-existing-input')
        ? check('segment-mode-from-existing-input', true, 'segment mode: from existing')
        : null,
  },
  {
    name: 'man:from-existing-selects',
    weight: 12,
    screens: ['wiz-manual'],
    build: (env) => {
      const candidates = [
        'from-existing-import-section-select',
        'from-existing-import-station-select',
        'from-existing-section-select',
        'from-existing-station-select',
      ].filter((testid) => actionable(env.digest, testid));
      const testid = pickFrom(() => env.ctx.rng(), candidates);
      if (testid === null) return null;
      const options = env.digest.opts[testid];
      const value =
        options && options.length > 0
          ? (pickFrom(() => env.ctx.rng(), options) as string)
          : String(env.ctx.randInt(0, Math.max(1, env.digest.rows)));
      return select(testid, value, `${testid}=${value}`);
    },
  },
  {
    name: 'man:mode-manual',
    weight: 5,
    screens: ['wiz-manual'],
    build: (env) =>
      actionable(env.digest, 'segment-mode-manual-input')
        ? check('segment-mode-manual-input', true, 'segment mode: manual')
        : null,
  },
  {
    name: 'man:manual-selects',
    weight: 6,
    screens: ['wiz-manual'],
    build: (env) => {
      const candidates = ['manual-import-section-select', 'manual-import-station-select'].filter((testid) =>
        actionable(env.digest, testid),
      );
      const testid = pickFrom(() => env.ctx.rng(), candidates);
      if (testid === null) return null;
      const options = env.digest.opts[testid];
      const value =
        options && options.length > 0
          ? (pickFrom(() => env.ctx.rng(), options) as string)
          : String(env.ctx.randInt(0, Math.max(1, env.digest.rows)));
      return select(testid, value, `${testid}=${value}`);
    },
  },
  {
    name: 'man:place-on-map',
    weight: 8,
    screens: ['wiz-manual'],
    build: (env) =>
      actionable(env.digest, 'segment-place-on-map-button')
        ? click('segment-place-on-map-button', 'place segment on map')
        : null,
  },
  {
    name: 'man:mode-discard',
    weight: 3,
    screens: ['wiz-manual'],
    build: (env) =>
      actionable(env.digest, 'segment-mode-discard-input')
        ? check('segment-mode-discard-input', true, 'segment mode: discard')
        : null,
  },
  {
    name: 'man:place',
    weight: 10,
    screens: ['wiz-manual'],
    build: (env) => (actionable(env.digest, 'segment-place-button') ? click('segment-place-button', 'place segment') : null),
  },
  {
    name: 'man:skip',
    weight: 5,
    screens: ['wiz-manual'],
    build: (env) =>
      actionable(env.digest, 'segment-skip-button') ? click('segment-skip-button', 'skip segment') : null,
  },
];

// ---------------------------------------------------------------------------
// Shot table
// ---------------------------------------------------------------------------

const SHOT_TABLE_SCREENS: readonly ScreenId[] = ['zero-datum-config', 'wiz-suggest', 'wiz-manual'];

const SHOT_TABLE: Generator[] = [
  {
    /**
     * Edit a cell: double-click opens the grid's textarea editor, and the value
     * lands on the NEXT step (an `input` on `shot-editor-cell-editor`). Two
     * moves rather than one because both are plain trace steps, and the trace
     * schema has no compound "edit" step to record a fused one with.
     */
    name: 'grid:open-cell',
    weight: 10,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => {
      const cell = pickFrom(() => env.ctx.rng(), gridCells(env.digest));
      if (cell === null) return null;
      return click(`shot-editor-cell-${cell[0]}-${cell[1]}`, `open cell ${cell[0]}/${cell[1]}`, true);
    },
  },
  {
    name: 'grid:edit-cell',
    weight: 10,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => {
      if (!actionable(env.digest, 'shot-editor-cell-editor')) return null;
      const value = pickFrom(() => env.ctx.rng(), CELL_BOUNDARIES) as string;
      return type('shot-editor-cell-editor', value, `cell=${JSON.stringify(value)}`);
    },
  },
  {
    name: 'grid:select-row',
    weight: 6,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => {
      const index = pickFrom(() => env.ctx.rng(), indexedIds(env.digest, 'shot-editor-gutter-'));
      if (index === null) return null;
      return click(`shot-editor-gutter-${index}`, `select row ${index}`);
    },
  },
  {
    name: 'grid:insert-above',
    weight: 5,
    screens: SHOT_TABLE_SCREENS,
    build: (env) =>
      env.digest.rows > 0 ? click('shot-editor-context-insert-above', 'insert row above') : null,
  },
  {
    name: 'grid:insert-below',
    weight: 5,
    screens: SHOT_TABLE_SCREENS,
    build: (env) =>
      env.digest.rows > 0 ? click('shot-editor-context-insert-below', 'insert row below') : null,
  },
  {
    name: 'grid:delete-rows',
    weight: 6,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => (env.digest.rows > 0 ? click('shot-editor-context-delete-rows', 'delete selected rows') : null),
  },
  {
    /**
     * The frozen-row expected refusal, in two legal moves.
     *
     * The wizard freezes the shots an overlap matched and REFUSES to delete
     * them: `handleDeleteShots` filters them out, raises the
     * `dmp-wizard-frozen-delete` toast and returns false. So this generator
     * first makes a frozen row the active one, then asks for the delete; the
     * shell asserts the refusal whenever a delete's whole selection was frozen
     * (it has the pre-click selection in the digest).
     */
    name: 'grid:delete-frozen',
    weight: 4,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => {
      if (env.digest.frozen.length === 0) return null;
      const armed =
        env.digest.activeRow !== null &&
        env.digest.frozen.includes(env.digest.activeRow) &&
        env.digest.selRows.length > 0 &&
        env.digest.selRows.every((row) => env.digest.frozen.includes(row));
      if (armed) return click('shot-editor-context-delete-rows', 'delete FROZEN rows (expect refusal)');
      const row = pickFrom(() => env.ctx.rng(), env.digest.frozen);
      if (row === null) return null;
      return click(`shot-editor-gutter-${row}`, `select FROZEN row ${row}`);
    },
  },
  {
    name: 'grid:paste-block',
    weight: 3,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => (env.digest.rows > 0 ? click('shot-editor-context-paste', 'paste into grid') : null),
  },
  {
    /**
     * Blank the selected cells in place.
     *
     * The one grid mutation that can leave a row PRESENT but incomplete, which
     * is what `refuseIncompleteShots` refuses a confirm on
     * (`DmpNewImportDialog.tsx`): every other mutator either fills a cell or
     * removes the row. Allowlisted since wave 2 and, until now, never driven.
     */
    name: 'grid:clear',
    weight: 5,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => (env.digest.rows > 0 ? click('shot-editor-context-clear', 'clear grid selection') : null),
  },
  {
    /** Copy AND clear in one gesture - the other half of the clipboard pair. */
    name: 'grid:cut',
    weight: 4,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => (env.digest.rows > 0 ? click('shot-editor-context-cut', 'cut grid selection') : null),
  },
  {
    name: 'grid:fill-down',
    weight: 2,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => (env.digest.rows > 0 ? click('shot-editor-context-fill-down', 'fill down') : null),
  },
  {
    name: 'grid:copy',
    weight: 2,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => (env.digest.rows > 0 ? click('shot-editor-context-copy', 'copy grid selection') : null),
  },
  {
    /**
     * Appending is typing into the trailing PHANTOM row - the grid has no add
     * button. The phantom is the last row the grid renders, so the highest
     * rendered `length` cell is the target. (The grid virtualizes, so that is
     * only the true phantom when the grid is scrolled to the bottom; when it is
     * not, this is an ordinary edit, which is a fine move to make anyway.)
     */
    name: 'grid:append',
    weight: 5,
    screens: SHOT_TABLE_SCREENS,
    build: (env) => {
      const lengths = gridCells(env.digest).filter((cell) => cell[1] === 'length');
      if (lengths.length === 0) return null;
      const last = lengths[lengths.length - 1] as [number, string];
      return click(`shot-editor-cell-${last[0]}-length`, `open trailing row ${last[0]} (append)`, true);
    },
  },
  {
    /**
     * Grow and shrink the table band.
     *
     * `shot-editor-expand` only renders where the HOST passes `onToggleExpand`,
     * and of the import screens that is `ZeroDatumConfigDialog` alone - the DMP
     * wizard owns its own layout and does not. Scoped accordingly rather than
     * offered on all three shot-table screens, where it would be a permanent
     * skip. Allowlisted since wave 2 and never driven until now.
     */
    name: 'grid:expand',
    weight: 3,
    screens: ['zero-datum-config'],
    build: (env) => (actionable(env.digest, 'shot-editor-expand') ? click('shot-editor-expand', 'toggle table size') : null),
  },
];

/**
 * `DialogWrapper`'s discard-confirm overlay.
 *
 * An ESCAPE always plays `dirty-dialog-discard` - it is trying to leave - so the
 * other button was allowlisted and never clicked. Keeping editing is the branch
 * that puts the walk BACK into a dirty dialog it had already asked to close,
 * which is the state the overlay exists to protect and the one nothing has
 * exercised.
 */
const DIRTY_OVERLAY: Generator[] = [
  {
    name: 'dirty:keep-editing',
    weight: 5,
    screens: ['dmp-select', 'tml-config', 'new-project'],
    build: (env) =>
      actionable(env.digest, 'dirty-dialog-keep-editing')
        ? click('dirty-dialog-keep-editing', 'keep editing (discard overlay)')
        : null,
  },
];

// ---------------------------------------------------------------------------
// Map placement
// ---------------------------------------------------------------------------

const PLACEMENT: Generator[] = [
  {
    /**
     * Land the placement, NEAR the survey or well away from it.
     *
     * An OFFSET from the map centre, which is where the survey is: the shell
     * resolves it against the live centre at dispatch. An absolute draw would
     * land in the Gulf of Guinea and be refused before it reached the app.
     *
     * The two ranges are different code paths, not two flavours of the same
     * one. A click within snapping distance of a station commits a
     * `manual_snap` placement, which MERGES the segment onto what it snapped to;
     * a click far from everything commits a plain `manual` placement, which
     * merges onto nothing and leaves the segment as its own component with no
     * zero datum. That second outcome is the only thing this fuzzer can build an
     * ORPHAN out of (`Project::suggest_orphan_placements` wants a component with
     * no datum and no newly-imported member), and with the tight jitter alone
     * essentially every placement snapped: 214 campaign cells, zero orphans.
     *
     * The far range stays well inside a normal viewport, and one that lands
     * outside is refused by `lib/controls.ts` and counted as `mapSkip:*` like
     * any other impossible move.
     */
    name: 'map:click',
    weight: 12,
    screens: ['map-placement'],
    build: (env) => {
      const far = env.ctx.rng() < 0.5;
      const span = far ? 0.005 : 0.0008;
      const jitter = (): number => (env.ctx.rng() - 0.5) * span;
      return {
        kind: 'mapClick',
        lngLat: [jitter(), jitter()],
        relative: true,
        label: far ? 'place well away from the survey' : 'place near the map centre',
      };
    },
  },
  // There is deliberately NO generator for `uiStore.confirmPlacement()`.
  //
  // It reads like the map's confirm button and is not one: it clears
  // `placementMode` and RETURNS the state, leaving the caller to run
  // `onComplete`. `MapCanvas.handlePlacementConfirm` is its only caller and
  // bails when there is no `cursorPosition`, so calling it directly ABANDONS
  // the placement - wizard already closed, early-imported sections committed,
  // completion handler never run. That is a state no user can reach, and the
  // flow oracles then fairly report residue for a flow that was neither
  // completed nor cancelled. A placement is driven the way a user drives it:
  // `map:click` (which goes through MapCanvas and does call `onComplete`) or
  // Escape.
  {
    name: 'map:escape',
    weight: 3,
    screens: ['map-placement'],
    build: () => click('key:Escape', 'Escape out of placement'),
  },
  {
    /**
     * Route a SECOND file while a placement owns the screen.
     *
     * `routeOpenedFile`'s `importFlowOwnsScreen` guard must refuse it - toast,
     * no state change - and the shell asserts exactly that. This is the guard's
     * only test path (see the hook's comment in `src/main.tsx`).
     */
    name: 'map:interject-route',
    weight: 4,
    screens: ['map-placement'],
    build: (env) => {
      const source = drawFileSource(env);
      return {
        kind: 'openFile',
        source,
        via: 'route',
        trigger: 'routeOpenedFile',
        label: 'INTERJECT: route a second file mid-placement',
      };
    },
  },
];

const TERMINALS: Generator[] = [
  {
    name: 'receipt:details',
    weight: 4,
    screens: ['receipt'],
    build: (env) =>
      actionable(env.digest, 'import-receipt-details') ? click('import-receipt-details', 'receipt details') : null,
  },
  {
    name: 'receipt:continue',
    weight: 14,
    screens: ['receipt'],
    build: (env) =>
      actionable(env.digest, 'import-receipt-continue') ? click('import-receipt-continue', 'dismiss receipt') : null,
  },
  {
    name: 'confirm:continue',
    weight: 6,
    screens: ['wiz-confirm'],
    build: (env) => {
      const testid = actionable(env.digest, 'cancel-confirm-continue')
        ? 'cancel-confirm-continue'
        : actionable(env.digest, 'back-confirm-continue')
          ? 'back-confirm-continue'
          : null;
      if (testid === null) return null;
      return click(testid, 'keep going (confirm dialog)');
    },
  },
  {
    name: 'confirm:accept',
    weight: 8,
    screens: ['wiz-confirm'],
    build: (env) => {
      const testid = actionable(env.digest, 'confirm-cancel-button')
        ? 'confirm-cancel-button'
        : actionable(env.digest, 'confirm-back-button')
          ? 'confirm-back-button'
          : null;
      if (testid === null) return null;
      return click(testid, 'confirm the cancel/back');
    },
  },
  {
    /**
     * The THIRD answer #394 gave the cancel overlay: close AND withdraw the
     * sections the wizard already imported, in one batched undoable step.
     *
     * The model carries the id and the escape path names it as something "the
     * walk reaches through the generators rather than while trying to leave" -
     * this is that generator. It self-gates: the button only renders once
     * `pendingWithdrawCount > 0`, so it cannot fire on a wizard that has
     * imported nothing.
     *
     * Weighted BELOW keep-and-close (8) deliberately, and not for taste: a
     * withdrawal removes the very sections the post-import surfaces need, so a
     * walk that withdrew every time it cancelled would starve the receipt, the
     * orphan dialog and the F1 undo roundtrip of the state they measure. Four is
     * enough to exercise the path often without owning the flow's ending.
     */
    name: 'confirm:cancel-remove',
    weight: 4,
    screens: ['wiz-confirm'],
    build: (env) =>
      actionable(env.digest, 'cancel-remove-button')
        ? click('cancel-remove-button', 'cancel AND remove what was imported')
        : null,
  },
  {
    name: 'blocked:choose',
    weight: 10,
    screens: ['blocked'],
    build: (env) => {
      const testid = env.ctx.rng() < 0.5 ? 'import-blocked-switch' : 'import-blocked-stay';
      return actionable(env.digest, testid) ? click(testid, `import blocked: ${testid}`) : null;
    },
  },
  {
    name: 'as-new:choose',
    weight: 10,
    screens: ['as-new-project'],
    build: (env) => {
      const testid = env.ctx.rng() < 0.4 ? 'import-as-new-project-confirm' : 'import-as-new-project-cancel';
      return actionable(env.digest, testid) ? click(testid, `import as new project: ${testid}`) : null;
    },
  },
  {
    name: 'orphan:choose',
    weight: 10,
    screens: ['orphan-placement'],
    build: (env) => {
      const candidates = ['orphan-accept', 'orphan-skip', 'orphan-accept-all', 'orphan-close'].filter((testid) =>
        actionable(env.digest, testid),
      );
      const testid = pickFrom(() => env.ctx.rng(), candidates);
      return testid === null ? null : click(testid, `orphan placement: ${testid}`);
    },
  },
  {
    name: 'error:dismiss',
    weight: 10,
    screens: ['error'],
    build: (env) => (actionable(env.digest, 'error-dialog-close') ? click('error-dialog-close', 'dismiss error') : null),
  },
  {
    name: 'wiz:retry',
    weight: 8,
    screens: ['wiz-error', 'wiz-placing'],
    build: (env) => (actionable(env.digest, 'retry-button') ? click('retry-button', 'retry import') : null),
  },
];

/**
 * The new-project-for-import screen. It needs a NAME and a SAVE PATH, and the
 * path has to be one the app may write: the shell substitutes the session's
 * `fuzz-projects/` directory for the placeholder below, which is what keeps the
 * save path out of a replayed trace's control.
 */
const NEW_PROJECT: Generator[] = [
  {
    name: 'newproj:name',
    weight: 8,
    screens: ['new-project'],
    build: (env) => {
      if (!actionable(env.digest, 'project-name-input')) return null;
      const value = env.ctx.rng() < 0.3 ? '' : `fuzz-project-${env.fileSeq}`;
      return type('project-name-input', value, `project name=${JSON.stringify(value)}`);
    },
  },
  {
    name: 'newproj:path',
    weight: 8,
    screens: ['new-project'],
    build: (env) =>
      actionable(env.digest, 'project-save-path-input')
        ? type('project-save-path-input', PROJECT_DIR_PLACEHOLDER, 'project save path (session dir)')
        : null,
  },
  {
    name: 'newproj:create',
    weight: 12,
    screens: ['new-project'],
    build: (env) =>
      actionable(env.digest, 'create-and-continue-button')
        ? click('create-and-continue-button', 'create project and continue')
        : null,
  },
  {
    name: 'newproj:cancel',
    weight: 3,
    screens: ['new-project'],
    build: (env) =>
      actionable(env.digest, 'new-project-for-import-cancel') ? click('new-project-for-import-cancel', 'cancel') : null,
  },
];

/** Every generator, in catalog order. */
export const GENERATORS: readonly Generator[] = [
  ...STARTERS,
  ...DMP_SELECT,
  ...TML_CONFIG,
  ...COMPASS,
  ...WALLS,
  ...GPX,
  ...ORPHAN,
  ...EXPORT_MODAL,
  ...ZERO_DATUM,
  ...ZERO_DATUM_CONFIG,
  ...WIZARD_SUGGEST,
  ...METADATA,
  ...WIZARD_MANUAL,
  ...SHOT_TABLE,
  ...DIRTY_OVERLAY,
  ...PLACEMENT,
  ...TERMINALS,
  ...NEW_PROJECT,
];

/** Generators legal on `screen`, expanded by weight. */
export function weightedFor(screen: ScreenId): readonly Generator[] {
  const out: Generator[] = [];
  for (const generator of GENERATORS) {
    if (!generator.screens.includes(screen)) continue;
    for (let i = 0; i < generator.weight; i += 1) out.push(generator);
  }
  return out;
}

// ---------------------------------------------------------------------------
// Interjections
// ---------------------------------------------------------------------------

/**
 * Moves legal on EVERY screen, drawn from their own PRNG stream so adding one
 * does not shift the trajectory every existing repro line depends on (the
 * convention `framework.ts`'s `mulberry32` header states).
 */
export type Interjection = 'escape' | 'route' | 'undo' | 'redo' | 'save';

export function interjectProbability(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_INTERJECT_P);
  return Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : 0.08;
}

/** Salt for the interjection stream (`mulberry32(seed ^ SALT)`). */
export const INTERJECT_STREAM_SALT = 0x27d4eb2f;

export function drawInterjection(rng: Rng): Interjection {
  const roll = rng();
  if (roll < 0.3) return 'escape';
  if (roll < 0.5) return 'route';
  if (roll < 0.7) return 'undo';
  if (roll < 0.85) return 'redo';
  return 'save';
}

export function buildInterjection(kind: Interjection, env: GenEnv): FuzzMove | null {
  if (kind === 'escape') return click('key:Escape', 'INTERJECT: Escape');
  if (kind === 'undo') return { kind: 'undo', label: 'INTERJECT: undo mid-flow' };
  if (kind === 'redo') return { kind: 'redo', label: 'INTERJECT: redo mid-flow' };
  if (kind === 'save') {
    return env.digest.hasProject ? { kind: 'save', label: 'INTERJECT: saveProject mid-flow' } : null;
  }
  const source = drawFileSource(env);
  return {
    kind: 'openFile',
    source,
    via: 'route',
    trigger: 'routeOpenedFile',
    label: 'INTERJECT: route a second file mid-flow',
  };
}

// ---------------------------------------------------------------------------
// Auxiliary interjections (wave 3)
// ---------------------------------------------------------------------------

/**
 * The moves wave 3 added, on their OWN stream with their OWN probability.
 *
 * Deliberately not folded into `drawInterjection`: that function's thresholds
 * and its stream are what every repro line in `importDialogFuzz.ts`'s header
 * was recorded against, and re-partitioning them would silently rewrite the
 * draw sequence of a trajectory those lines describe. A second stream leaves
 * the first one's draws bit-identical and only inserts moves between them.
 */
export type AuxInterjection = 'export' | 'mode' | 'close' | 'openKey';

export function auxInterjectProbability(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_IMPORT_AUX_INTERJECT_P);
  return Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : 0.04;
}

/** Salt for the auxiliary stream (`mulberry32(seed ^ SALT)`). */
export const AUX_INTERJECT_STREAM_SALT = 0x9e3779b9;

export function drawAuxInterjection(rng: Rng): AuxInterjection {
  const roll = rng();
  if (roll < 0.4) return 'export';
  if (roll < 0.7) return 'mode';
  if (roll < 0.85) return 'close';
  return 'openKey';
}

/**
 * Build one auxiliary interjection, or `null` when it is not legal right now.
 *
 * THREE OF THE FOUR ARE `idle`-ONLY, and the reason is the same for each: they
 * would otherwise reach a state no user can reach, and a bug found in one of
 * those is a fabricated bug. This file already makes that argument once, for
 * `uiStore.confirmPlacement()`; these are the same argument applied to the
 * shell's other privileged handles.
 *
 *   `mode`   - the mode carousel lives in the toolbar, and every import screen
 *              is a modal whose backdrop covers it. This bridge dispatches
 *              synthetic events straight at an element, so it can "click"
 *              through a backdrop that a mouse cannot. The carousel is also
 *              `disabled` while a placement is running, which the digest
 *              already reports as not-actionable.
 *   `close`  - nothing in the UI binds `handleCloseProject` while a dialog is
 *              open, and both keyboard dispatchers return early for every
 *              non-Escape key in that state. Driving the raw store action
 *              mid-flow leaves a dialog mounted with no project behind it -
 *              which is a real observation about `closeProject`, and NOT a bug
 *              this scenario may report, because no user can get there.
 *   `openKey`- an armed answer that the dispatcher then swallows has to be
 *              un-armed again; keeping the chord to the state where it actually
 *              runs keeps that window as small as it can be.
 *
 * `export` is the exception and is legal EVERYWHERE, because Ctrl+E is a real
 * keystroke a user can make at any moment and the app's own dispatcher decides
 * what it does. That is exactly what makes it worth pressing mid-flow: the
 * scenario asserts the modal does NOT open on top of an import screen.
 */
export function buildAuxInterjection(kind: AuxInterjection, env: GenEnv): FuzzMove | null {
  if (kind === 'export') return click('key:Ctrl+E', 'INTERJECT: Ctrl+E (export modal)');
  if (env.digest.screen !== 'idle') return null;
  if (kind === 'mode') {
    const candidates = ['mode-survey', 'mode-plan', 'mode-export'].filter((testid) => actionable(env.digest, testid));
    const testid = pickFrom(() => env.ctx.rng(), candidates);
    return testid === null ? null : click(testid, `INTERJECT: switch app mode via ${testid}`);
  }
  if (kind === 'close') {
    return env.digest.hasProject ? { kind: 'closeProject', label: 'INTERJECT: closeProject at idle' } : null;
  }
  const source = drawFileSource(env);
  return { kind: 'keyFile', chord: 'Ctrl+O', source, label: 'INTERJECT: Ctrl+O with an armed picker' };
}
