/**
 * The values a fuzz trace step's trigger, export format and oracle name print
 * as in `fuzz-shrink`'s minimal action list (#576 W58 NF2).
 *
 * A trace is attached to issues and shrunk from wherever someone got it, and
 * `readTrace` checks these fields only as text (`requireText`), so
 * `describeSteps` prints one only when it is a value the harness defines -
 * listed here - and any other as its length (`valueShape`). The other fields it
 * checks only as text print through their own helpers: a test id through
 * `shareableTestId`, a path through `shareableTracePath` below, a reload's
 * fixture through `shareableFixtureName`, a corpus id only in its 5-hex form.
 *
 * The lists copy the scenarios' own tables: the triggers are literals of the
 * generators, the oracle names live inside `exportRoundtripFuzz`'s run, where
 * nothing can import them, and the formats are private to
 * `exportFuzzActions.ts` (the compiler holds this copy to `ExportFormat`).
 * `traceStepFields.test.ts` holds each copy to its table, so a value a
 * scenario starts recording cannot print as a length unnoticed.
 */
import { shortId } from './crc32';
import type { ExportFormat } from './exportOracles';
import { isTrackedFixture, isTrackedPath } from './privateFixtures';
import { foldLocalPaths, valueShape } from './shareableFailure';

/**
 * The triggers the import-dialog fuzzer records in a `fileDialog` step
 * (`importDialogGenerators.ts` through `importDialogTrace.moveTraceStep`): the
 * two picker buttons, and the armed-keyboard picker as `key:<chord>`.
 */
export const TRACE_TRIGGERS: ReadonlySet<string> = new Set(['toolbar-import-button', 'global-open-import-button', 'key:Ctrl+O']);

/** Every `ExportFormat`, which the compiler holds to the type: the formats the export fuzzer draws (`exportFuzzActions.ts` ALL_FORMATS). */
const FORMATS: Readonly<Record<ExportFormat, true>> = {
  csv: true,
  json: true,
  tml: true,
  tmlu: true,
  png: true,
  svg: true,
  pdf: true,
  preview: true,
  diveProfile: true,
};
export const TRACE_EXPORT_FORMATS: ReadonlySet<string> = new Set(Object.keys(FORMATS));

/** The structural oracles the export fuzzer runs (`exportRoundtripFuzz.ts` ORACLE_NAMES). */
export const TRACE_ORACLE_NAMES: ReadonlySet<string> = new Set([
  'x3-differential',
  'x4-byte-identity',
  'x5x6-roundtrip',
  'x7-hostile-comments',
  'x8-option-independence',
  'x9-mode-invariants',
  'x9b-pre-plan-leak',
]);

/** A trace field's value as `describeSteps` prints it: itself when `known` has it, else `valueShape` (its length). */
export function shareableTraceWord(value: unknown, known: ReadonlySet<string>): string {
  return typeof value === 'string' && known.has(value) ? value : valueShape(value);
}

/**
 * A trace step's path as `describeSteps` prints it (#576 W74-NF2). A relative
 * path - the only kind `readTrace` lets through - prints as itself only when it
 * names what the repo tracks, as a fixture name does (#576 W58 Q1): a path the
 * checkout tracks (`isTrackedPath`), or a session's copy of a tracked fixture
 * (`fixtures/<name>`). Any other prints as its tag, `<private-path#xxxxx>`
 * (`crc32.shortId` of it), so the steps of one trace that name one file still
 * agree: folding replaces only the local roots, so a forged trace's
 * `qzCave/x.dmp` printed its names. An absolute path, which only a step built
 * in memory can hold, prints folded as before.
 */
export function shareableTracePath(rel: string): string {
  if (/^(?:[/\\]|[A-Za-z]:)/.test(rel)) return foldLocalPaths(rel);
  const [dir, name, ...deeper] = rel.split('/');
  const fixtureCopy = dir === 'fixtures' && name !== undefined && deeper.length === 0 && isTrackedFixture(name);
  return fixtureCopy || isTrackedPath(rel) ? rel : `<private-path#${shortId(rel)}>`;
}
