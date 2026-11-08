/**
 * The judgments `export-roundtrip-fuzz` makes, separated from the driving.
 *
 * Same split `importFlowOracles.ts` makes and for the same reason: the scenario
 * COLLECTS (bytes, results, snapshots, toast keys) and these functions JUDGE.
 * A judgment that needs no live app is a pure function that a vitest can pin,
 * and `exportOracles.test.ts` pins every one of them - which matters most for
 * the two MIRRORS below, because a mirror that silently drifts from the code it
 * mirrors turns an oracle into a coin toss.
 *
 * ## The two mirrors, and why they are copies rather than imports
 *
 * `ensureExtensionMirror` and `escapeCsvMirror` reimplement app code:
 * `ensureExtension` in `src/utils/fileDialogs.ts` (with `lastSeparatorIndex` in
 * `src/utils/path.ts`) and `src-tauri/src/export/csv.rs:66-79`. The
 * Rust one has no choice. The TypeScript one does, and is still a copy, for the
 * reason every other cross-tier constant in this harness is copied (see
 * `importDialogFuzz.ts`'s `RECEIPT_TOAST_KEY`): `tsconfig.agent.json` compiles
 * with `types: ["node"]` and no `jsx`, so following an import into `src/` drags
 * in `import.meta.env` and TSX and breaks `pnpm typecheck:agent`. The rule is
 * therefore: the mirror carries the source's file:line in a comment, the vitest
 * pins the mirror against a table transcribed from that source, and a drift
 * shows up as a failing unit test rather than as a fuzzer that quietly checks
 * the wrong path.
 *
 * ## Every oracle THROWS, and every message carries a reproduce line
 *
 * `ExportOracleError` is the marker the scenario catches to attach its trace
 * path. Nothing here classifies, tolerates or downgrades: a red is the app's.
 */
import { foldThenCut } from './fixtureCorpus';

export class ExportOracleError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ExportOracleError';
  }
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/**
 * What one export move produces.
 *
 * `preview` is the odd one out and deliberately in the same enum: it is the
 * only export path that renders the survey and writes NOTHING
 * (`preview_svg_export` returns the markup, `commands/export.rs:475-495`), so a
 * failure there is a rendering failure with no file to look at. Keeping it in
 * the format axis is what lets the differential below compare it against the
 * SVG that does hit disk.
 */
export type ExportFormat = 'csv' | 'json' | 'tml' | 'tmlu' | 'png' | 'svg' | 'pdf' | 'preview' | 'diveProfile';

/** Formats that end in a file on disk. */
export const WRITING_FORMATS: readonly ExportFormat[] = [
  'csv',
  'json',
  'tml',
  'tmlu',
  'png',
  'svg',
  'pdf',
  'diveProfile',
];

/** Formats the `data` half of `ExportControls` offers. */
export const DATA_FORMATS: readonly ExportFormat[] = ['csv', 'json', 'tml', 'tmlu'];
/** Formats the `image` half offers. */
export const IMAGE_FORMATS: readonly ExportFormat[] = ['png', 'svg', 'pdf'];

/**
 * The three ways a user (or the IPC boundary) reaches an export.
 *
 * `modal` and `panel` render THE SAME COMPONENT (`ExportControls`, variant
 * `dialog` / `panel`) over two different option sets - the dialog's local
 * `useState`, the panel's `exportStore` - which is exactly why both are walked:
 * the component is shared, the state is not, and #X8 is about that seam.
 */
export type ExportRoute = 'modal' | 'panel' | 'store';

/**
 * Destination-path shapes. Each one aims at a rule somewhere in the stack:
 *
 *   normal              the control
 *   upperTmlu           `names_for_path` lowercases before comparing to "tml"
 *                       (`export/tml.rs:254-264`), so `.TMLU` must take the
 *                       SHORT element names and the plain-XML writer
 *   doubleExtension     `a.tml.txt` - `.txt` is not a KNOWN extension, so
 *                       `ensureExtension` APPENDS (#576 D35): the DOM routes
 *                       write `a.tml.txt.tml`, and the store route writes the
 *                       literal `.txt`, which `write_output`'s `_` arm accepts
 *   noExtension         `names_for_path` falls to SHORT; `write_output` falls
 *                       to plain `fs::write` even for a "tml" export
 *   trailingDot         `out.` - `path.extension()` is None, and
 *                       `ensureExtension` rewrites the empty extension
 *   unicodeRtl          a name carrying U+202E and non-Latin script
 *   longName            ~200 chars, well inside NAME_MAX but past every buffer
 *                       a formatter might assume
 *   percentQuoteSpace   `%`, `"` and a space - the characters a path that
 *                       reaches a shell, a URL or an XML attribute breaks on
 *   openProject         THE OPEN `.stalx` ITSELF; every writing path must
 *                       refuse it (`commands/export.rs`'s `is_project_owned_file`)
 *   plnxSibling         `<project>.plnx`, the dive-plan store beside it
 */
export type PathShape =
  | 'normal'
  | 'upperTmlu'
  | 'doubleExtension'
  | 'noExtension'
  | 'trailingDot'
  | 'unicodeRtl'
  | 'longName'
  | 'percentQuoteSpace'
  | 'openProject'
  | 'plnxSibling';

export const PATH_SHAPES: readonly PathShape[] = [
  'normal',
  'upperTmlu',
  'doubleExtension',
  'noExtension',
  'trailingDot',
  'unicodeRtl',
  'longName',
  'percentQuoteSpace',
  'openProject',
  'plnxSibling',
];

/** Shapes that name a file the OPEN PROJECT owns rather than a fresh target. */
export const PROJECT_OWNED_SHAPES: ReadonlySet<PathShape> = new Set(['openProject', 'plnxSibling']);

/**
 * The BASENAME a shape produces, given the format's natural extension and a
 * per-move sequence number.
 *
 * Pure and total: `openProject` / `plnxSibling` have no basename of their own
 * (the scenario substitutes the live project's path), and asking for one is a
 * programming error rather than a skip, so it throws.
 *
 * Every returned value is a plain basename - no separator, no `..`, never
 * starting with a dot. `materializeSurveyFile` states the same rule for
 * generated inputs; it applies just as much to a destination this run creates.
 */
export function exportBaseName(shape: PathShape, extension: string, seq: number): string {
  const stem = `export-${seq}`;
  switch (shape) {
    case 'normal':
      return `${stem}.${extension}`;
    case 'upperTmlu':
      // Upper case on BOTH halves: the point is that the app lowercases before
      // it compares, and a lower-case stem would leave that half untested.
      return `${stem.toUpperCase()}.TMLU`;
    case 'doubleExtension':
      return `${stem}.tml.txt`;
    case 'noExtension':
      return stem;
    case 'trailingDot':
      return `${stem}.`;
    case 'unicodeRtl':
      // U+202E RIGHT-TO-LEFT OVERRIDE inside the stem, plus a non-Latin script
      // and an astral-plane pair. The override is the interesting one: it makes
      // any UI that echoes the path render the tail reversed. Spelled as
      // escapes, as `surveyFileSpecs.ts`'s own boundary strings are, so the
      // source stays readable in an editor that would otherwise honour it.
      return `${stem}-\u202e\u6d1e\u7a74\ud83d\udd26.${extension}`;
    case 'longName': {
      // ~200 chars total. NAME_MAX is 255 on ext4/APFS, so this is a legal name
      // that no dialog, log line or error message was sized for.
      const pad = 'n'.repeat(Math.max(0, 200 - stem.length - extension.length - 2));
      return `${stem}-${pad}.${extension}`;
    }
    case 'percentQuoteSpace':
      return `${stem} 100% "quoted".${extension}`;
    case 'openProject':
    case 'plnxSibling':
      throw new Error(
        `path shape "${shape}" names a file the open project owns; the scenario supplies that path, ` +
          `it is not generated`,
      );
  }
}

/** The extension a format writes by default (what `ensureExtension` forces). */
export function naturalExtension(format: ExportFormat): string {
  switch (format) {
    case 'tmlu':
      return 'tmlu';
    case 'diveProfile':
      // `ProfileChart.onExport` does `resolveSavePath(path, 'xml')`
      // (src/components/SingleSystemView/ProfileChart.tsx), which forces the
      // extension through `ensureExtension`.
      return 'xml';
    case 'preview':
      return 'svg';
    default:
      return format;
  }
}

// ---------------------------------------------------------------------------
// Mirror 1: the app's extension forcing
// ---------------------------------------------------------------------------

/** `KNOWN_FILE_EXTENSIONS` in `src/utils/fileDialogs.ts`, transcribed. */
const KNOWN_FILE_EXTENSIONS_MIRROR: readonly string[] = [
  'stalx', 'plnx', 'gpx', 'tml', 'tmlu', 'mak', 'dat', 'wpj', 'srv', 'dmp',
  'csv', 'json', 'png', 'svg', 'pdf', 'xml',
];

/**
 * `lastSeparatorIndex` (with `isWindowsPath`) in `src/utils/path.ts`,
 * transcribed: `\` separates only in a Windows-shaped path (a drive letter and
 * a separator, or a leading `\\`); on Linux and macOS it is an ordinary
 * file-name character.
 */
function lastSeparatorIndexMirror(filePath: string): number {
  const slash = filePath.lastIndexOf('/');
  const windows = /^[a-zA-Z]:[/\\]/.test(filePath) || filePath.startsWith('\\\\');
  return windows ? Math.max(slash, filePath.lastIndexOf('\\')) : slash;
}

/**
 * `ensureExtension` in `src/utils/fileDialogs.ts`, transcribed.
 *
 * The behaviour that matters and is easy to misremember (#576 D35): a trailing
 * extension that names a KNOWN file type is REPLACED (`/a/b.svg` becomes
 * `/a/b.csv`), anything else is kept and APPENDED to (`/a/b.txt` ->
 * `/a/b.txt.csv`, `CaveA 1.5` -> `CaveA 1.5.csv`), and only the last dot of the
 * FILE NAME counts - not a directory's (`/a.b/c` gains `.csv`) and not a leading
 * one (`/x/.hidden` -> `/x/.hidden.csv`). A bare trailing dot is dropped. A
 * harness that guessed wrong would check a path the app never wrote to and call
 * every export a silent failure.
 */
export function ensureExtensionMirror(filePath: string, extension: string): string {
  const normalizedExt = extension.startsWith('.') ? extension.slice(1) : extension;
  const lowerPath = filePath.toLowerCase();
  const lowerExt = normalizedExt.toLowerCase();
  if (lowerPath.endsWith(`.${lowerExt}`)) return filePath;
  const lastSeparator = lastSeparatorIndexMirror(filePath);
  const lastDot = filePath.lastIndexOf('.');
  if (lastDot > lastSeparator + 1) {
    const current = filePath.slice(lastDot + 1).toLowerCase();
    if (current === '' || KNOWN_FILE_EXTENSIONS_MIRROR.includes(current)) {
      return `${filePath.slice(0, lastDot + 1)}${normalizedExt}`;
    }
  }
  return `${filePath}.${normalizedExt}`;
}

/**
 * Where a DOM-route export ACTUALLY writes, given the path answered into the
 * save dialog.
 *
 * `ExportControls.handleExport` finalizes in three shapes
 * (`src/components/Dialogs/ExportControls.tsx`): TML keeps a `.tml` or `.tmlu`
 * the user already typed and otherwise forces `.tml`; every other data format
 * forces its own; an image forces `imageOptions.format`.
 *
 * THIS IS WHY THE `openProject` SHAPE BEHAVES DIFFERENTLY PER ROUTE. Answering
 * the save dialog with `cave.stalx` and a CSV export produces `cave.csv`: the
 * app's own extension forcing redirects the write, and the backend refusal is
 * never reached. The store route hands the path to the command verbatim and IS
 * refused. Both are correct; the oracle has to know which it is looking at.
 */
export function finalizeExportPath(selected: string, format: ExportFormat): string {
  if (format === 'tml' || format === 'tmlu') {
    // ONE branch for both, because the app has one: the Format select offers
    // `tml` and nothing else, and `.tmlu` is reached only by an answered path
    // that ALREADY ends in it. Forcing `.tmlu` here for a `tmlu` draw predicted
    // a file the app never writes - it writes `.tml` - and X1 then reported
    // every such move as "reported SUCCESS and wrote no file". Measured on
    // seed 1, move 16.
    const lower = selected.toLowerCase();
    if (lower.endsWith('.tmlu') || lower.endsWith('.tml')) return selected;
    return ensureExtensionMirror(selected, 'tml');
  }
  return ensureExtensionMirror(selected, naturalExtension(format));
}

/**
 * Does a DOM-route export write somewhere OTHER than the answered path? Only
 * then can the app's Replace File alert appear (`resolveSavePath` in
 * src/utils/overwriteGuard.ts checks a rewritten name, never the answer the OS
 * dialog already confirmed). `plnxSibling` and `openProject` are always
 * rewritten (`.plnx` / `.stalx` are known extensions, replaced by the
 * format's), so the scenario must expect, and answer, that alert there
 * (#576 NF-W2FE-2).
 */
export function domRouteRewritesPath(selected: string, format: ExportFormat): boolean {
  return finalizeExportPath(selected, format) !== selected;
}

// ---------------------------------------------------------------------------
// Mirror 2: the CSV escaping rule
// ---------------------------------------------------------------------------

/** Leading characters `escape_csv` neutralises with a `'` prefix. */
const CSV_FORMULA_LEADERS = ['=', '+', '-', '@', '\t', '\r'];
/** Characters that force RFC-4180 quoting. */
const CSV_QUOTE_TRIGGERS = [',', '"', '\n', '\r'];

/**
 * `src-tauri/src/export/csv.rs:66-79`, transcribed.
 *
 * ```rust
 * let neutralised = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
 *     format!("'{s}")
 * } else { s.to_string() };
 * if neutralised.contains([',', '"', '\n', '\r']) {
 *     format!("\"{}\"", neutralised.replace('"', "\"\""))
 * } else { neutralised }
 * ```
 *
 * Order matters and the Rust says so: neutralise FIRST, so the apostrophe ends
 * up inside the quotes rather than stranded outside them. Two consequences the
 * oracle depends on and a from-memory version would get wrong:
 *
 *  - a LEADING `\n` is not in the neutralisation set but is in the quoting set;
 *  - a leading `\t` IS neutralised and is NOT quoted, because tab is in neither
 *    the quote-trigger set nor anything else - so `"\tfoo"` renders as the
 *    unquoted five characters `'` TAB `foo`.
 */
export function escapeCsvMirror(value: string): string {
  const neutralised = CSV_FORMULA_LEADERS.some((c) => value.startsWith(c)) ? `'${value}` : value;
  if (CSV_QUOTE_TRIGGERS.some((c) => neutralised.includes(c))) {
    return `"${neutralised.replace(/"/g, '""')}"`;
  }
  return neutralised;
}

/**
 * RFC-4180 reader over the bytes the exporter wrote.
 *
 * Deliberately its own parser rather than a split on commas: the whole point of
 * the check is that a hostile comment can contain commas, quotes and newlines,
 * and a naive split would agree with a broken exporter.
 *
 * Row separators are `\r\n` or `\n` OUTSIDE quotes only. A bare `\r` outside
 * quotes is NOT a row separator here, because `writeln!` emits `\n`
 * (`export/csv.rs`) and treating a stray `\r` as one would split a row the
 * exporter meant to keep whole.
 */
export function parseCsvRows(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = '';
  let quoted = false;
  let index = 0;
  const pushField = (): void => {
    row.push(field);
    field = '';
  };
  const pushRow = (): void => {
    pushField();
    rows.push(row);
    row = [];
  };
  while (index < text.length) {
    const ch = text[index] as string;
    if (quoted) {
      if (ch === '"') {
        if (text[index + 1] === '"') {
          field += '"';
          index += 2;
          continue;
        }
        quoted = false;
        index += 1;
        continue;
      }
      field += ch;
      index += 1;
      continue;
    }
    if (ch === '"' && field === '') {
      quoted = true;
      index += 1;
      continue;
    }
    if (ch === ',') {
      pushField();
      index += 1;
      continue;
    }
    if (ch === '\r' && text[index + 1] === '\n') {
      pushRow();
      index += 2;
      continue;
    }
    if (ch === '\n') {
      pushRow();
      index += 1;
      continue;
    }
    field += ch;
    index += 1;
  }
  // A trailing newline closes the last row; anything after it is a real row.
  if (field !== '' || row.length > 0) pushRow();
  return rows;
}

/** The header `export/csv.rs:12-16` writes, in order. */
export const CSV_HEADER: readonly string[] = [
  'section_id',
  'section_name',
  'shot_id',
  'length',
  'azimuth_in',
  'azimuth_out',
  'depth_in',
  'depth_out',
  'left',
  'right',
  'up',
  'down',
  'kind',
  'comment',
];

/** Index of the `comment` column, which is the only hostile-text column. */
export const CSV_COMMENT_INDEX = CSV_HEADER.length - 1;
/** Index of the `section_name` column, the other `escape_csv`-processed field. */
export const CSV_SECTION_NAME_INDEX = 1;

export interface CsvViolation {
  row: number;
  column: string;
  detail: string;
}

/**
 * Does this CSV honour `escape_csv` for every text cell, and did every seeded
 * comment survive?
 *
 * TWO checks, and they fail differently:
 *
 *  1. SHAPE - re-encoding each parsed cell with `escapeCsvMirror` must give
 *     back exactly what the file contains at that position. This catches an
 *     un-neutralised leading `=` (a formula the next spreadsheet executes) and
 *     an unquoted embedded comma (a row that silently gains a column).
 *  2. SURVIVAL - every comment the project holds must appear, ONCE, as a parsed
 *     cell. A neutralised cell parses back to `'=cmd...` rather than `=cmd...`,
 *     which is the correct and intended difference, so the expectation is
 *     compared after applying the same neutralisation.
 *
 * The comparison is against the RAW LINE rather than the whole file so a
 * violation names a row a human can find.
 */
export function csvTextViolations(
  csvText: string,
  expectedComments: readonly string[],
): { violations: CsvViolation[]; missing: string[]; rows: number } {
  const rows = parseCsvRows(csvText);
  const violations: CsvViolation[] = [];
  const rawLines = splitCsvRecords(csvText);

  rows.forEach((row, rowIndex) => {
    if (rowIndex === 0) return; // header
    const raw = rawLines[rowIndex];
    if (raw === undefined) return;
    // The exporter writes exactly one `format!` per shot with a fixed number of
    // `{}` holes, so the field count is not a heuristic - any other count means
    // a separator got out of a cell. Checked FIRST and independently of the
    // per-cell shape check below, because a torn row can leave every surviving
    // cell individually well-formed: `...,Permanent,a,b` parses as a benign
    // `a` in the comment column and a stray `b` after it.
    if (row.length !== CSV_HEADER.length) {
      violations.push({
        row: rowIndex,
        column: '(record)',
        detail:
          `row has ${row.length} field(s), not the ${CSV_HEADER.length} the header declares - ` +
          `a comma, quote or newline escaped its cell`,
      });
    }
    for (const [column, index] of [
      ['section_name', CSV_SECTION_NAME_INDEX],
      ['comment', CSV_COMMENT_INDEX],
    ] as const) {
      const cell = row[index];
      if (cell === undefined) {
        violations.push({
          row: rowIndex,
          column,
          detail: `row has ${row.length} field(s), fewer than the ${CSV_HEADER.length} the header declares - ` +
            `an unquoted separator split it`,
        });
        continue;
      }
      const reEncoded = escapeCsvMirror(cell);
      if (!raw.includes(reEncoded)) {
        violations.push({
          row: rowIndex,
          column,
          detail:
            `cell ${JSON.stringify(cell)} would be written as ${JSON.stringify(reEncoded)} by ` +
            `escape_csv, and that text is not in the record`,
        });
      }
    }
  });

  // Every parsed comment cell, as written (i.e. post-neutralisation).
  const present = new Set<string>();
  rows.forEach((row, rowIndex) => {
    if (rowIndex === 0) return;
    const cell = row[CSV_COMMENT_INDEX];
    if (cell !== undefined) present.add(cell);
  });
  const missing = expectedComments.filter((comment) => !present.has(neutralisedForm(comment)));
  return { violations, missing, rows: rows.length };
}

/**
 * What a comment looks like once `escape_csv` has run, with the RFC-4180
 * quoting removed again by the parser.
 *
 * Only the neutralisation survives a parse: the quotes are transport.
 */
export function neutralisedForm(value: string): string {
  return CSV_FORMULA_LEADERS.some((c) => value.startsWith(c)) ? `'${value}` : value;
}

/** Physical records, so a violation can quote the line it is in. */
function splitCsvRecords(text: string): string[] {
  const records: string[] = [];
  let current = '';
  let quoted = false;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i] as string;
    if (ch === '"') quoted = !quoted;
    if (!quoted && ch === '\n') {
      records.push(current.endsWith('\r') ? current.slice(0, -1) : current);
      current = '';
      continue;
    }
    current += ch;
  }
  if (current !== '') records.push(current);
  return records;
}

// ---------------------------------------------------------------------------
// Canonicalization for the convergence chain
// ---------------------------------------------------------------------------

/** A v4-shaped UUID anywhere in a document. */
export const UUID_RE = /[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/g;

/**
 * The same document with every UUID-shaped token replaced by a positional
 * placeholder, and trailing whitespace normalised.
 *
 * Used ONLY as the fallback comparison in the convergence oracle, and the
 * scenario records which comparison actually decided the verdict - because the
 * two answer different questions. Raw bytes ask "is the exporter a function of
 * the project"; canonical bytes ask "is anything but identity moving". Reaching
 * for the second when the first would have passed would hide a real
 * instability, so the oracle tries raw first and says so.
 *
 * Positional (`<uuid-0>`, `<uuid-1>`, ...) rather than ONE constant token, so
 * the REPEAT STRUCTURE survives the canonicalization: a document where one uuid
 * sits on two shots must not compare equal to one where those shots have
 * distinct uuids, and a single token would call those two the same. That is
 * exactly the difference `remint_duplicate_uuids` makes, so flattening it would
 * canonicalize away the thing most worth noticing.
 *
 * What it does NOT distinguish is a pure permutation of two distinct uuids in
 * otherwise identical surroundings - `A B` and `B A` both become
 * `<uuid-0> <uuid-1>`. That is fine and stated rather than hidden: shots carry
 * their geometry with them, so a real reordering moves every Length, Azimuth
 * and Depth around it and the comparison sees THOSE.
 */
export function canonicalizeUuids(text: string): string {
  let next = 0;
  const seen = new Map<string, string>();
  return text.replace(UUID_RE, (match) => {
    const existing = seen.get(match);
    if (existing !== undefined) return existing;
    const token = `<uuid-${next}>`;
    next += 1;
    seen.set(match, token);
    return token;
  });
}

// ---------------------------------------------------------------------------
// The oracles
// ---------------------------------------------------------------------------

/** What one export attempt produced, as the scenario observed it. */
export interface ExportAttempt {
  format: ExportFormat;
  route: ExportRoute;
  shape: PathShape;
  /** The path answered into the dialog / handed to the command. */
  requested: string;
  /** Where the app should have written, after its own extension forcing. */
  effective: string;
  /** Did the app report success? */
  ok: boolean;
  /** The failure text, when it failed. */
  error: string | null;
  /** `fs.statSync(effective).size`, or null when the file is absent. */
  size: number | null;
  /**
   * Toast keys THIS MOVE produced - `newToastKeys(before, after)`, not every
   * key that happens to be on screen. See that function for why the difference
   * is the whole point.
   */
  toastKeys: readonly string[];
  /** Files present in the export directory that the move did not intend. */
  strays: readonly string[];
}

/** `TOAST_KEYS.EXPORT_FAILED` (`src/utils/toastKeys.ts`). */
export const EXPORT_FAILED_TOAST_KEY = 'export-failed';

/**
 * The toast stack as one move sees it, taken before and after.
 *
 * `marks` identifies each toast the STORE holds by `key#id@revision`, which is
 * exactly the identity `uiStore` mints and refreshes them under; `keys` is the
 * key set the DOM and the store together show, which is what a failure message
 * prints. Two fields because they answer different questions: what CHANGED, and
 * what is on screen.
 */
export interface ToastSnapshot {
  marks: readonly string[];
  keys: readonly string[];
}

/**
 * Toast keys that are NEW between two snapshots: a toast that was not there
 * before, or one that was refreshed in place.
 *
 * WHY THIS IS NOT "IS THE KEY ON SCREEN". An `export-failed` toast is
 * `kind: 'error'`, and ToastStack gives an error toast a 9000 ms dwell plus a
 * 1500 ms fade (`ToastStack.tsx`), so it stays readable for ten and a half
 * seconds - far longer than a CSV or JSON export takes. A move that read the
 * live key set therefore inherited the PREVIOUS move's failure: the later
 * export succeeded, was classified as failed, and X1 then reported "an export
 * reported FAILURE and left a N-byte file behind" against an app that had done
 * nothing wrong. The classification has to be a DIFF over the move, and the
 * move's own bracket is the only thing that defines it.
 *
 * A refresh counts as new, and has to. `showToast` collapses a repeat of the
 * same key within `TOAST_DEDUP_WINDOW_MS` (1000 ms) into the toast already on
 * screen, bumping its `revision` instead of pushing a second one - so two
 * failures a few hundred milliseconds apart produce ONE toast, and a key-set
 * diff would see no change and call the second one a success. That is the same
 * bug in the other direction, and X1 would then demand a file the failed export
 * correctly did not write. Keying on `id@revision` catches both.
 *
 * DOM-only keys (a rendered toast the store no longer lists) are compared as a
 * plain key set, because there is nothing else to compare them by. That is
 * belt and braces: ToastStack renders from the store and a leaving toast stays
 * in it until `removeToast`, so the store is a superset in practice.
 */
export function newToastKeys(before: ToastSnapshot, after: ToastSnapshot): string[] {
  const seen = new Set(before.marks);
  const fresh = new Set<string>();
  for (const mark of after.marks) {
    if (seen.has(mark)) continue;
    const hash = mark.indexOf('#');
    fresh.add(hash > 0 ? mark.slice(0, hash) : mark);
  }
  const beforeKeys = new Set(before.keys);
  const afterMarkKeys = new Set(after.marks.map((mark) => (mark.indexOf('#') > 0 ? mark.slice(0, mark.indexOf('#')) : mark)));
  for (const key of after.keys) {
    if (!beforeKeys.has(key) && !afterMarkKeys.has(key)) fresh.add(key);
  }
  return [...fresh].sort();
}

/**
 * X1 - the result and the filesystem agree, and a failure is visible.
 *
 * Three claims, and the third is the one the app did not used to honour:
 *
 *  a. success => the file exists and is NON-EMPTY. A zero-byte artifact is the
 *     shape a truncating writer leaves when it fails after `File::create` and
 *     before the first row - and every export writer truncates in place
 *     (`lib/oracles.ts` records the audit), so there is no staging file to look
 *     for instead.
 *  b. failure => the file is ABSENT. Not "absent or empty": every reachable
 *     refusal in the stack happens before the destination is opened (the
 *     project-owned guard, `export_png`'s DPI and size bails, the usvg parse),
 *     so a file appearing after a reported failure means a writer got further
 *     than it said it did.
 *  c. failure on a UI route => the user was told. That is `ExportControls`'
 *     error toast; before it existed, a refused export left the dialog sitting
 *     there un-greyed, which reads as "the file was written".
 *
 * The store route is exempt from (c) BY CONSTRUCTION, and the exemption is the
 * honest one rather than a convenience: `projectStore.exportCsv` and its three
 * siblings return `Promise<void>` and throw, and the surfacing lives in the
 * component that calls them. A raw IPC caller getting a rejected promise IS the
 * report.
 */
export function checkFileResult(attempt: ExportAttempt, reproduce: string): void {
  const where =
    `format=${attempt.format} route=${attempt.route} shape=${attempt.shape}\n` +
    `    requested: ${attempt.requested}\n` +
    `    effective: ${attempt.effective}`;

  if (attempt.ok) {
    if (attempt.size === null) {
      throw new ExportOracleError(
        `X1 an export reported SUCCESS and wrote no file\n  ${where}\n  ${reproduce}`,
      );
    }
    if (attempt.size === 0) {
      throw new ExportOracleError(
        `X1 an export reported SUCCESS and left a ZERO-BYTE file\n  ${where}\n` +
          `    every writer truncates in place, so this is what a write that died after\n` +
          `    File::create looks like\n  ${reproduce}`,
      );
    }
  } else {
    if (attempt.size !== null) {
      throw new ExportOracleError(
        `X1 an export reported FAILURE and left a ${attempt.size}-byte file behind\n  ${where}\n` +
          `    error: ${attempt.error ?? '(none)'}\n  ${reproduce}`,
      );
    }
    if (attempt.route !== 'store' && !attempt.toastKeys.includes(EXPORT_FAILED_TOAST_KEY)) {
      throw new ExportOracleError(
        `X1 an export failed on a UI route and NOTHING TOLD THE USER\n  ${where}\n` +
          `    error: ${attempt.error ?? '(none)'}\n` +
          `    live toast keys: ${JSON.stringify(attempt.toastKeys)}\n` +
          `    expected ${EXPORT_FAILED_TOAST_KEY} (TOAST_KEYS.EXPORT_FAILED)\n  ${reproduce}`,
      );
    }
  }

  if (attempt.strays.length > 0) {
    throw new ExportOracleError(
      `X1 an export left ${attempt.strays.length} file(s) it did not name\n  ${where}\n` +
        `    strays: ${JSON.stringify(attempt.strays)}\n  ${reproduce}`,
    );
  }
}

/**
 * X2 - the open project's own database survives every attempt to export over it.
 *
 * `projectBytesBefore` / `After` are the `.stalx`'s own bytes. This is the
 * claim that actually matters, and it is route-independent: whether the app
 * refused (the store route, where the path reaches the command verbatim) or
 * REDIRECTED (the UI routes, where `ensureExtension` rewrites `cave.stalx` to
 * `cave.csv` before the command ever sees it), the live database must be
 * byte-for-byte what it was.
 *
 * `expectRefusal` is passed only for the store route, and it is a separate
 * claim: there the backend guard is the only thing standing between an
 * arbitrary IPC path and the open database, so it must fire.
 */
export function checkProjectFileSurvived(
  args: {
    route: ExportRoute;
    format: ExportFormat;
    before: string;
    after: string | null;
    ok: boolean;
    error: string | null;
    expectRefusal: boolean;
  },
  reproduce: string,
): void {
  const where = `format=${args.format} route=${args.route}`;
  if (args.after === null) {
    throw new ExportOracleError(
      `X2 the open project's .stalx is GONE after an export aimed at it\n  ${where}\n  ${reproduce}`,
    );
  }
  if (args.after !== args.before) {
    throw new ExportOracleError(
      `X2 an export aimed at the open project MODIFIED its .stalx\n  ${where}\n` +
        `    reported: ${args.ok ? 'success' : `failure (${args.error ?? 'no message'})`}\n` +
        `    the live database is what every one of these guards exists to protect\n  ${reproduce}`,
    );
  }
  if (args.expectRefusal && args.ok) {
    throw new ExportOracleError(
      `X2 the raw command ACCEPTED the open project's own path\n  ${where}\n` +
        `    commands/export.rs refuses it with "Cannot export over the open project file";\n` +
        `    the UI routes are protected by ensureExtension rewriting the path, so this\n` +
        `    guard is the only thing between the IPC boundary and the database\n  ${reproduce}`,
    );
  }
}

/** One arm of the svg/png/pdf differential. */
export interface DifferentialArm {
  format: ExportFormat;
  ok: boolean;
  error: string | null;
}

/**
 * X3 - if the SVG renders, the raster and the PDF must render too.
 *
 * PNG and PDF are the SAME SVG string put through `usvg`'s `Tree::from_str`
 * (`export/image.rs:434` and `:459`); SVG export writes the string straight out
 * with no parse gate (`:400`). So a document that only the SVG path accepts is
 * a document `generate_svg_string` produced and cannot itself read - malformed
 * markup that ships as a file nobody warned the user about, because the SVG
 * export cannot fail on it.
 *
 * The comparison is only sound at options that cannot fail for an unrelated
 * reason, which is why the scenario pins the DPI for this move: `dpi = 0` bails
 * before the parse, and a high DPI on a large survey trips the 16384px cap.
 * Both are legitimate refusals of the PNG path alone, and neither says anything
 * about the markup.
 */
export function checkFormatDifferential(
  svg: DifferentialArm,
  others: readonly DifferentialArm[],
  reproduce: string,
): void {
  if (!svg.ok) return;
  const broken = others.filter((arm) => !arm.ok);
  if (broken.length === 0) return;
  throw new ExportOracleError(
    `X3 the SVG export succeeded but ${broken.map((a) => a.format).join(' and ')} failed on the ` +
      `SAME options\n` +
      broken.map((a) => `    ${a.format}: ${a.error ?? '(no message)'}`).join('\n') +
      `\n    PNG and PDF re-parse the generated SVG with usvg; the SVG path does not.\n` +
      `    So this is markup generate_svg_string emitted and cannot read back, shipped\n` +
      `    as a file with no error.\n  ${reproduce}`,
  );
}

/** What the byte-identity oracle needs to know about the project it ran on. */
export interface UuidPrecondition {
  /** Shots whose `uuid` is missing or empty. */
  missing: number;
  /** Uuids that appear more than once ACROSS the whole project. */
  duplicated: number;
}

/**
 * X4 - a TML export is a function of the project, not of the clock.
 *
 * Byte identity is CONDITIONAL, and the conditions are in the exporter rather
 * than in this harness:
 *
 *  - `shot_export_uuid` (`export/tml.rs:556-561`) mints a fresh v4 for any shot
 *    whose stored uuid is missing or empty, per export. Its own doc says so.
 *  - `remint_duplicate_uuids` (`:588-596`) mints a fresh v4 for every
 *    DOCUMENT-level repeat, per export. Its doc states the consequence: "a
 *    project holding a cross-section duplicate does not re-export byte-identical".
 *    Note `Project::ensure_unique_shot_uuids` is SECTION-scoped, so a
 *    cross-section duplicate survives a save and reaches this.
 *
 * Neither is export-time healing of the PROJECT: `ensure_shot_uuids` runs on
 * save and on load (`core/state/project_io.rs:198`, `core/state/project_load.rs:138`),
 * never in `export_tml`. So the first export does NOT settle anything, and A,
 * B and C are all the same question - which is why all three are asserted
 * together when the precondition holds, rather than exempting A.
 *
 * When the precondition does NOT hold the oracle asserts only that all three
 * exports succeeded, and the scenario counts it. That is the honest shape: the
 * instability is documented behaviour of the exporter, and asserting against it
 * would be asserting the app is different from what it says it is.
 *
 * (The zip container is not a second source of drift: `write_zip` takes
 * `SimpleFileOptions::default()`, whose mtime is wall-clock only under zip's
 * `time` feature, and `src-tauri/Cargo.toml` builds zip with
 * `default-features = false, features = ["deflate"]`.)
 */
export function checkByteIdentity(
  args: {
    hashes: readonly string[];
    sizes: readonly (number | null)[];
    precondition: UuidPrecondition;
    extension: string;
  },
  reproduce: string,
): { stable: boolean; skipped: boolean } {
  if (args.hashes.length !== 3 || args.sizes.length !== 3) {
    throw new ExportOracleError(
      `X4 needs exactly three exports, got ${args.hashes.length}\n  ${reproduce}`,
    );
  }
  args.sizes.forEach((size, index) => {
    if (size === null || size === 0) {
      throw new ExportOracleError(
        `X4 export ${'ABC'[index]} of three produced ${size === null ? 'no file' : 'an empty file'}\n` +
          `  ${reproduce}`,
      );
    }
  });

  const unstableByDesign = args.precondition.missing > 0 || args.precondition.duplicated > 0;
  const [a, b, c] = args.hashes as [string, string, string];
  if (unstableByDesign) {
    return { stable: a === b && b === c, skipped: true };
  }
  if (a !== b || b !== c) {
    throw new ExportOracleError(
      `X4 three .${args.extension} exports of an UNMUTATED project are not byte-identical\n` +
        `    sha256: A=${a.slice(0, 16)} B=${b.slice(0, 16)} C=${c.slice(0, 16)}\n` +
        `    sizes:  ${args.sizes.join(' / ')}\n` +
        `    every shot has a non-empty uuid and none is duplicated, so neither\n` +
        `    shot_export_uuid's mint nor remint_duplicate_uuids can fire; the exporter\n` +
        `    is supposed to be a pure function of the project here, and\n` +
        `    export/tml.rs's own export_tml_is_byte_stable_across_calls says so.\n  ${reproduce}`,
    );
  }
  return { stable: true, skipped: false };
}

/**
 * X5 - the round trip reaches a FIXED POINT.
 *
 * T1 is exported from the fixture and imported into a fresh project as T2, T2
 * into another as T3, and so on. What the oracle asserts is that the chain
 * SETTLES: some consecutive pair is byte-identical within the exports it takes.
 * Everything after that pair is identical too, by induction - the composition is
 * a function of its input.
 *
 * IT USED TO ASSERT `T2 == T3`, AND THAT PREMISE WAS OFF BY ONE PASS. The
 * reasoning was "both sides have been through a full trip already, so a
 * remaining difference is one the trip INTRODUCES every time". On a real
 * 532-shot survey it is not: T2 and T3 differ and T3 and T4 are byte-identical,
 * so the trip introduces the difference exactly ONCE MORE and then stops.
 *
 * Measured with the files parsed rather than diffed: T2 and T3 hold the same 40
 * sections with the same names, the same membership (ZERO shots changed section,
 * matched by uuid), and - once the Ariane `<ID>`/`<FR>` node numbers, which
 * renumber with position, are set aside - an IDENTICAL block multiset. The only
 * thing that moved was the ORDER the sections are written in. The first import
 * inherits that order from the source project rather than deriving it, so the
 * composition needs one more pass to reach its own fixed point. Nothing is lost,
 * gained or re-partitioned on the way. See finding 6 in `exportRoundtripFuzz.ts`.
 *
 * Asserting the fixed POINT instead of a fixed INDEX is the same claim correctly
 * stated, and it is not weaker: a trip that really never converged - a new order
 * every pass - fails, and so does one that loses or gains anything, because a
 * loss makes every later pass differ too. `settledAt` is reported so a fixture
 * that starts needing more passes is visible before it becomes a failure.
 *
 * Uuid canonicalization is tried only when raw bytes differ, and the caller is
 * told which decided it: the TML importer PRESERVES the file's shot uuids
 * (`parsers/tml/mod.rs:619`), so raw equality is the expected outcome and a
 * fall-back to canonical means identity moved.
 */
export function checkConvergence(
  args: { exports: readonly string[] },
  reproduce: string,
): { via: 'raw' | 'canonical'; settledAt: number } {
  const chain = args.exports;
  if (chain.length < 2) {
    throw new ExportOracleError(
      `X5 needs at least two exports to compare, got ${chain.length}\n  ${reproduce}`,
    );
  }
  for (let i = 1; i < chain.length; i += 1) {
    const previous = chain[i - 1] as string;
    const current = chain[i] as string;
    if (previous === current) return { via: 'raw', settledAt: i + 1 };
    if (canonicalizeUuids(previous) === canonicalizeUuids(current)) {
      return { via: 'canonical', settledAt: i + 1 };
    }
  }
  const last = chain[chain.length - 1] as string;
  const previous = chain[chain.length - 2] as string;
  const c1 = canonicalizeUuids(previous);
  const c2 = canonicalizeUuids(last);
  throw new ExportOracleError(
    `X5 the export/import round trip never settles: ${chain.length} exports and no consecutive\n` +
      `    pair is identical, even with uuids canonicalized\n` +
      `    sizes: ${chain.map((text) => text.length).join(' -> ')}\n` +
      `    last pair first differs at offset ${firstDifference(c1, c2)}\n` +
      `${describeDifference(c1, c2)}\n` +
      `    T1 vs T2 may differ (documented lossiness, plus branch fragmentation #412), and so\n` +
      `    may one pass after it - the first import inherits its section ORDER from the source\n` +
      `    project rather than deriving it. What may not happen is a chain that keeps changing:\n` +
      `    a user who round-trips twice would then get a differently-shaped project every time.\n` +
      `  ${reproduce}`,
  );
}

function firstDifference(a: string, b: string): number {
  const limit = Math.min(a.length, b.length);
  for (let i = 0; i < limit; i += 1) if (a[i] !== b[i]) return i;
  return limit;
}

function describeDifference(a: string, b: string): string {
  const at = firstDifference(a, b);
  const from = Math.max(0, at - 60);
  return (
    `    T2: ...${JSON.stringify(a.slice(from, at + 60))}\n` +
    `    T3: ...${JSON.stringify(b.slice(from, at + 60))}`
  );
}

/** One shot, reduced to the GEOMETRY a round trip has to carry. */
export interface ShotIdentity {
  /**
   * The shot's uuid, when it has one. CARRIED BUT NEVER KEYED.
   *
   * `shotKey` reads only the geometry below, so this cannot affect a verdict.
   * It is here so a failure can be investigated shot-by-shot by IDENTITY rather
   * than by guessing which vanished key pairs with which gained one - which is
   * exactly how finding 5's families were told apart, and only possible at all
   * because 5a made uuids survive the trip.
   */
  uuid?: string;
  length: number;
  azimuthIn: number;
  depthIn: number;
  depthOut: number;
  lrud: [number | null, number | null, number | null, number | null];
}

/**
 * One LRUD field, with Ariane's absent sentinel collapsed onto absent.
 *
 * `0.0` IS the absent sentinel in the `.tml`/`.tmlu` format: the writer fills
 * `shot.left.unwrap_or(0.0)` because Ariane's `Left`/`Right`/`Up`/`Down` have no
 * null, and the reader maps `abs() < f64::EPSILON` back to `None` PER FIELD. So
 * a MEASURED zero cannot round-trip and an unmeasured one can, and both arrive
 * as the same four bytes in between.
 *
 * THIS IS THE FORMAT'S SEMANTIC, NOT A TOLERANCE, and the difference is worth
 * being precise about because the two look identical from a distance. A
 * tolerance would be a threshold this harness chose to stop a red it could not
 * explain. This is a documented equivalence with a citation:
 * `docs/ariane-export.md#the-lrud-zeronull-collision` states the collision, why
 * neither end can be "fixed" without inventing a spelling Ariane does not
 * define, and the measured scale of it; `docs/ariane-import.md` states the read
 * half at `parsers/tml/mod.rs:1908-1914`. The oracle folds exactly that
 * equivalence and nothing else - a zero becoming 3.5, or a 3.5 becoming null,
 * still fails.
 */
function lrudSentinel(value: number | null): string {
  if (value === null) return '-';
  return Math.abs(value) < 1e-12 ? '-' : value.toFixed(3);
}

/**
 * A multiset key for one shot: its heading and its LRUD.
 *
 * Numbers are rounded to the precision the TML actually carries so a formatting
 * round trip is not reported as data loss.
 *
 * LRUD goes through `lrudSentinel`, which reads Ariane's `0.0` as the absent
 * sentinel it is - see that function for why this is a documented format
 * semantic rather than a tolerance, and for the two doc sections that own the
 * claim. The heading is compared as measured.
 *
 * DEPTHS AND THE TAPE ARE NOT IN THE KEY, and that is the junction rule rather
 * than a weakening (#576 TML-DEPTH-D1, Q1-D1, Q2-D1; docs/ariane-export.md,
 * "Junction depths"). The export writes a junction whose readings disagree at
 * their mean and gives every shot touching it the length that keeps its run,
 * and it writes a shot whose drop exceeds its tape vertical, its drop as its
 * length - so a round trip legitimately changes some depths and some tapes.
 * What it may change, and what it may not, is asserted per shot BY UUID in
 * `junctionDepthOracle.ts` (X6c): every station at its true depth except an
 * averaged junction, at the mean of one reading per section; the length bound;
 * the plan. The cost: two different legs with reciprocal headings and mirrored
 * LRUD now share a key, so the multiset counts them under one key twice - a
 * shot lost or gained still changes the count, and the uuid set and X6c see
 * which.
 *
 * The key is also DIRECTION-AGNOSTIC: a leg and its reciprocal produce the same
 * key, because the exporter genuinely writes some legs backwards and a shot
 * walked the other way is the same shot. `reciprocal` states the argument.
 *
 * COMMENTS ARE DELIBERATELY NOT IN THE KEY, and that is a corrected premise
 * rather than a weakening. A comment does not belong to a SHOT across this round
 * trip: the exporter co-encodes station comments INTO the shot comment with a
 * U+200B line marker (`models::comment_split`), and the importer moves the
 * unmarked lines out onto stations when "comments on stations" is on - which is
 * the DEFAULT, since `projectStore.executeTmlImport` sends
 * `commentsOnStations ?? true`. Shot comments themselves stay on the shot since
 * #564 (every exported shot comment is marked), but a STATION comment still
 * legitimately moves - it lands in `project.station_comments`, which
 * `get_project_state` does not expose - so keying the shot on comment text
 * would report movement/invisibility as loss. Measured on seed 1.
 *
 * Text survival is therefore its own project-wide multiset
 * (`commentMultisetDelta`), which asks the question that survives the move.
 */
export function shotKey(shot: ShotIdentity): string {
  const n = (value: number | null): string => (value === null ? '-' : value.toFixed(3));
  const oriented = (s: ShotIdentity): string =>
    [
      // Normalised in BOTH orientations, not just the reciprocal: 360 and 0 are
      // one heading, and keying them apart would report a shot as lost and
      // gained for pointing due north.
      n(normalizeHeading(s.azimuthIn)),
      s.lrud.map(lrudSentinel).join('/'),
    ].join('|');
  // DIRECTION-AGNOSTIC: a leg and the same leg walked the other way key equal.
  // See `reciprocal` for why that is a correctness fix and not a fold.
  const forward = oriented(shot);
  const backward = oriented(reciprocal(shot));
  return forward < backward ? forward : backward;
}

/**
 * The same physical leg, seen from its other end.
 *
 * Heading turns through 180 (mod 360, so 0 and 360 are one value), the two
 * depths swap, and LEFT and RIGHT swap with them - up and down do not, because
 * turning round does not turn you over. This is exactly the transform
 * `export/tml.rs`'s reversed-leg constructors apply when they emit the
 * `(pre-zero-datum)` and merge legs, `reverse_pre_zero_azimuth` included.
 *
 * WHY THE KEY IS DIRECTION-AGNOSTIC, and why that is a CORRECTNESS FIX rather
 * than a fold that buys a green. A `.tml` cannot say "this passage was surveyed
 * outward from here": Ariane carries one forward azimuth per shot and the chain
 * hangs off a single rooted start, so a section surveyed away from its zero
 * datum has to be written walking BACK toward it. The shot that returns is the
 * same tape between the same two stations - it is one leg, recorded from the
 * other end. A direction-bearing key calls that a lost shot and a gained shot,
 * which is not a fact about the survey, it is a fact about the key.
 *
 * Measured before the change: on a 532-shot survey, 75 of the 99 vanished keys
 * paired exactly with a gained key under this transform - 150 of 198 residual
 * deltas, all of them reversed legs. See finding 5 in `exportRoundtripFuzz.ts`.
 *
 * What it does NOT excuse: the transform is exact. A heading off by anything
 * but 180, an LRUD that is not the mirror - both still differ, and the unit
 * tests pin each one. (A length or a depth is X6c's, which turns each re-imported
 * leg to its original's direction before it compares them.)
 */
function normalizeHeading(azimuth: number): number {
  return ((azimuth % 360) + 360) % 360;
}

function reciprocal(shot: ShotIdentity): ShotIdentity {
  const [left, right, up, down] = shot.lrud;
  return {
    length: shot.length,
    azimuthIn: normalizeHeading(shot.azimuthIn + 180),
    depthIn: shot.depthOut,
    depthOut: shot.depthIn,
    lrud: [right, left, up, down],
  };
}

export interface MultisetDelta {
  key: string;
  before: number;
  after: number;
}

/** Multiset difference, as counts per key. */
export function multisetDelta(
  before: readonly ShotIdentity[],
  after: readonly ShotIdentity[],
  keyFn: (shot: ShotIdentity) => string = shotKey,
): MultisetDelta[] {
  const count = (shots: readonly ShotIdentity[]): Map<string, number> => {
    const out = new Map<string, number>();
    for (const shot of shots) {
      const key = keyFn(shot);
      out.set(key, (out.get(key) ?? 0) + 1);
    }
    return out;
  };
  const a = count(before);
  const b = count(after);
  const deltas: MultisetDelta[] = [];
  for (const key of new Set([...a.keys(), ...b.keys()])) {
    const x = a.get(key) ?? 0;
    const y = b.get(key) ?? 0;
    if (x !== y) deltas.push({ key, before: x, after: y });
  }
  return deltas.sort((p, q) => (p.key < q.key ? -1 : 1));
}

// ---------------------------------------------------------------------------
// The one azimuth change a TML round trip is ENTITLED to make
// ---------------------------------------------------------------------------

/**
 * A section's RESOLVED azimuth reference, as `Project::azimuth_reference_for`
 * (`models/project.rs:1169`) returns it: the section's own, else the project
 * default, else the "nothing recorded" default (magnetic, 0, unknown).
 */
export interface SectionAzimuthReference {
  north: 'magnetic' | 'true' | 'grid';
  /** Degrees, east positive, as stored. */
  correction: number;
  /** False exactly when the source is `CorrectionSource::Unknown`: no correction on record. */
  onRecord: boolean;
}

/** The four answers `AzimuthExport::decide` gives, in the same order. */
export type TmlAzimuthPlan = 'magnetic' | 'true-as-stored' | 'mixed-to-true' | 'magnetic-unresolved';

/** `AzimuthReference::applied_correction`: the stored value, or 0 when it is not finite. */
function appliedCorrection(reference: SectionAzimuthReference): number {
  return Number.isFinite(reference.correction) ? reference.correction : 0;
}

/**
 * MIRROR of `AzimuthExport::decide` (`export/tml_azimuth.rs:56-108`): which
 * north the Ariane document declares, and whether its bearings are rewritten.
 *
 * `references` holds the resolved reference of every NON-EMPTY section - or,
 * equivalently, of every SHOT's section: the decision reads only which
 * references occur (any unknown? do the declared ones agree?), and a section with
 * no shots has none, which is exactly the Rust's `!section.shots.is_empty()`
 * filter. Only a section with a correction on record votes; an unknown one
 * constrains the document to magnetic and forbids the rewrite. "Agree" is the
 * same north and an applied correction within 1e-12 - the Rust's identity test,
 * not a tolerance.
 */
export function tmlAzimuthPlan(references: readonly SectionAzimuthReference[]): TmlAzimuthPlan {
  const anyUnknown = references.some((reference) => !reference.onRecord);
  const declared = references.filter((reference) => reference.onRecord);
  const first = declared[0];
  if (first === undefined) return 'magnetic';
  const uniform = declared.every(
    (reference) =>
      reference.north === first.north && Math.abs(appliedCorrection(reference) - appliedCorrection(first)) < 1e-12,
  );
  const canRewrite = !anyUnknown;
  if (!uniform) return canRewrite ? 'mixed-to-true' : 'magnetic-unresolved';
  switch (first.north) {
    case 'magnetic':
      return 'magnetic';
    case 'true':
      return anyUnknown ? 'magnetic-unresolved' : 'true-as-stored';
    case 'grid':
      return canRewrite ? 'mixed-to-true' : 'magnetic-unresolved';
  }
}

/**
 * The shots AS THE TML EXPORT WRITES THEM - what X6 compares the re-import to.
 *
 * On every plan but `mixed-to-true` that is the shots unchanged. On
 * `mixed-to-true` the exporter pushes each shot's measured mean bearing through
 * its section's reference (`AzimuthReference::to_true`,
 * `models/azimuth_reference.rs:232-240`, via `AzimuthExport::emit_reference`):
 * `normalize(bearing + correction)`, and a zero correction is the identity that
 * returns its input untouched. The re-import then reads those rotated bearings as
 * measured true-north ones, so without this fold X6 reported every shot of a
 * project whose sections carry different declinations (multi-year IGRF) as lost
 * and gained (#576 HFI-8).
 *
 * THIS IS THE FORMAT'S DOCUMENTED LOSS, NOT A TOLERANCE - the same distinction
 * `lrudSentinel` draws. `docs/ariane-export.md` ("Round-trip
 * cost of that third case") states that the per-section corrections are gone
 * after such a round trip and the azimuth multiset legitimately differs, and
 * decision HFI-8-D1 keeps the rotation (the declination review is #600). The
 * fold applies exactly that rotation, per section, and nothing else: a bearing
 * rotated by the wrong section's correction or an LRUD that changed still fails
 * X6, and a length or a depth outside the junction rule fails X6c.
 *
 * Generic so the caller's extra fields (a shot's section and index, which X6c
 * reads) ride along.
 *
 * `references[i]` is the resolved reference of `shots[i]`'s section.
 */
export function asExportedToTml<T extends ShotIdentity>(
  shots: readonly T[],
  references: readonly SectionAzimuthReference[],
): { shots: T[]; plan: TmlAzimuthPlan; rotated: number } {
  if (references.length !== shots.length) {
    // A harness bug, not an oracle verdict: the two lists come from one digest.
    throw new Error(`asExportedToTml: ${shots.length} shot(s) but ${references.length} reference(s)`);
  }
  const plan = tmlAzimuthPlan(references);
  if (plan !== 'mixed-to-true') return { shots: [...shots], plan, rotated: 0 };
  let rotated = 0;
  const out = shots.map((shot, index) => {
    const correction = appliedCorrection(references[index] as SectionAzimuthReference);
    if (correction === 0) return shot;
    rotated += 1;
    return { ...shot, azimuthIn: normalizeHeading(shot.azimuthIn + correction) };
  });
  return { shots: out, plan, rotated };
}

/**
 * Which COMMENT TEXTS the project gained or lost, wherever they live.
 *
 * Both sides are the flattened U+200B-split parts of every shot comment AND
 * every station comment. Splitting is what makes the exporter's co-encoding
 * invisible; pooling shots and stations is what makes the importer's
 * shot-to-station move invisible. What stays visible is text that is simply not
 * there any more.
 *
 * Compared TRIMMED, for the reason `docs/ariane-import.md` now records: the TML
 * reader trims every text field (which is how a pretty-printed file from
 * another tool reads correctly), so leading and trailing whitespace cannot
 * round-trip and that is a property of the format rather than a defect.
 */
export function commentMultisetDelta(
  before: readonly string[],
  after: readonly string[],
): MultisetDelta[] {
  const count = (parts: readonly string[]): Map<string, number> => {
    const out = new Map<string, number>();
    for (const part of parts) {
      const key = part.trim();
      if (key === '') continue;
      out.set(key, (out.get(key) ?? 0) + 1);
    }
    return out;
  };
  const a = count(before);
  const b = count(after);
  const deltas: MultisetDelta[] = [];
  for (const key of new Set([...a.keys(), ...b.keys()])) {
    const x = a.get(key) ?? 0;
    const y = b.get(key) ?? 0;
    if (x !== y) deltas.push({ key, before: x, after: y });
  }
  return deltas.sort((p, q) => (p.key < q.key ? -1 : 1));
}

/**
 * X6 - the shots survive the trip.
 *
 * The multiset, not the structure. Section boundaries are NOT compared: a
 * `.tml` describes a shot graph and `parse_tml` reconstructs sections as
 * connected runs of it, so one exported section can come back as several and
 * several can collapse into one (#412, by design, and `surveyFileSpecs.ts`'s
 * self-check claim makes the same point from the other side).
 *
 * What may not change is which SHOTS exist. `deltas` is the geometry+comment
 * multiset; `uuidDelta` is `shotSetDelta`'s answer for identity. Both are
 * reported together because they fail differently: identical geometry with a
 * changed uuid set is a re-mint, and a changed multiset with an intact uuid set
 * is a value being rewritten under a stable identity. Depths and tapes, which
 * the junction rule may change, are X6c's (`junctionDepthOracle.ts`).
 */
export function checkShotMultiset(
  args: {
    deltas: readonly MultisetDelta[];
    uuidsBefore: readonly string[];
    uuidsAfter: readonly string[];
    countBefore: number;
    countAfter: number;
    /**
     * `describeShotSetDelta`'s rendering of the same identity question, when
     * the caller has one.
     *
     * The caller computes it with `projectSnapshot.ts`'s `shotSetDelta`, which
     * is the harness's one answer to "which shots did this gain or lose"; it is
     * passed in already rendered rather than recomputed here so this module
     * stays free of imports and can be pinned from a `src/` test.
     */
    identityDetail?: string | null;
    /** `commentMultisetDelta`'s answer for the text, pooled over shots + stations. */
    commentDeltas?: readonly MultisetDelta[];
    /**
     * The export's azimuth plan, when the caller folded it (`asExportedToTml`).
     * Only rendered: on `mixed-to-true` the BEFORE side was already rotated, so
     * a reader knows a remaining azimuth difference is not that documented loss.
     */
    azimuthPlan?: TmlAzimuthPlan;
  },
  reproduce: string,
): void {
  const afterSet = new Set(args.uuidsAfter);
  const lostUuids = args.uuidsBefore.filter((u) => !afterSet.has(u));
  const commentDeltas = args.commentDeltas ?? [];
  if (args.deltas.length === 0 && lostUuids.length === 0 && commentDeltas.length === 0) return;
  const lines: string[] = [
    `X6 the shot set changed across an export + re-import`,
    `    shots: ${args.countBefore} before, ${args.countAfter} after`,
  ];
  if (args.deltas.length > 0) {
    lines.push(
      `    ${args.deltas.length} GEOMETRY key(s) differ ` +
        `(azimuth|LRUD, direction-agnostic, with Ariane's 0.0 absent-sentinel folded onto ` +
        `null - see docs/ariane-export.md; depths and tapes are X6c's, see \`shotKey\`):`,
    );
    if (args.azimuthPlan === 'mixed-to-true') {
      lines.push(
        `      (the BEFORE side is already rotated into true north per section correction - the ` +
          `exporter's documented mixed-declination case, docs/ariane-export.md - so this is not that)`,
      );
    }
    for (const delta of args.deltas.slice(0, 8)) {
      lines.push(`      ${delta.key}  x${delta.before} -> x${delta.after}`);
    }
    if (args.deltas.length > 8) lines.push(`      ... and ${args.deltas.length - 8} more`);
  }
  if (lostUuids.length > 0) {
    lines.push(`    ${lostUuids.length} shot uuid(s) did not come back: ${JSON.stringify(lostUuids.slice(0, 6))}`);
    lines.push(`      the TML importer PRESERVES the file's uuids (parsers/tml/mod.rs:619),`);
    lines.push(`      so a missing one is a shot that was dropped or re-minted, not a format limit.`);
  }
  if (args.identityDetail !== undefined && args.identityDetail !== null && args.identityDetail !== '') {
    lines.push(`    shotSetDelta:`);
    lines.push(args.identityDetail);
  }
  if (commentDeltas.length > 0) {
    lines.push(`    ${commentDeltas.length} COMMENT text(s) differ, pooled over shots AND stations:`);
    for (const delta of commentDeltas.slice(0, 8)) {
      // Fold, THEN cut (RV18-W23-6): a corpus name cut first leaves a stub no fold matches (#576 RV20-8).
      lines.push(`      ${JSON.stringify(foldThenCut(delta.key, 60))}  x${delta.before} -> x${delta.after}`);
    }
    if (commentDeltas.length > 8) lines.push(`      ... and ${commentDeltas.length - 8} more`);
    lines.push(`      A STATION comment legitimately moves into project.station_comments (which`);
    lines.push(`      get_project_state does not expose); shot comments stay on the shot (#564),`);
    lines.push(`      and the U+200B co-encoding is undone by splitting - with this scenario's`);
    lines.push(`      commentsOnStations: false import, a differing text really is GONE.`);
  }
  lines.push(`    Section STRUCTURE is deliberately not compared (branch fragmentation, #412).`);
  lines.push(`  ${reproduce}`);
  throw new ExportOracleError(lines.join('\n'));
}

/**
 * X6b - importing the same file twice must not put two shots in ONE SECTION on
 * the same uuid.
 *
 * PER-SECTION, not project-wide, and the difference is the app's stated
 * invariant rather than a relaxation. `Project::ensure_unique_shot_uuids`
 * (`models/project.rs:810-838`) re-mints "any shot that shares a UUID with an
 * earlier shot in the SAME section ... Restores the per-section uniqueness
 * `resolve_key` relies on", and the lookup cache it feeds is keyed
 * `(section_id, shot_uuid)` (`rebuild_shot_uuid_index`). A cross-section
 * duplicate is therefore legal, and the exporter handles it where it becomes a
 * problem - `export/tml.rs::remint_duplicate_uuids` re-mints document-level
 * repeats, and its doc names the consequence (that project does not re-export
 * byte-identical, which is exactly what X4's precondition measures).
 *
 * Importing the same file twice lands the second copy in NEW sections, so the
 * foreign uuids repeat across sections by construction and that is fine. What
 * would not be fine is a second import appending into an existing section, or
 * an importer that re-used a section: then `resolve_key` would resolve two
 * different shots to the same first match.
 */
export function checkNoDuplicateUuids(
  uuidsBySection: readonly (readonly string[])[],
  reproduce: string,
): void {
  const offenders: { section: number; duplicates: string[] }[] = [];
  uuidsBySection.forEach((uuids, index) => {
    const seen = new Set<string>();
    const duplicates = new Set<string>();
    for (const uuid of uuids) {
      if (seen.has(uuid)) duplicates.add(uuid);
      seen.add(uuid);
    }
    if (duplicates.size > 0) offenders.push({ section: index, duplicates: [...duplicates] });
  });
  if (offenders.length === 0) return;
  throw new ExportOracleError(
    `X6b ${offenders.length} section(s) hold two shots on the same uuid after importing the ` +
      `same file twice\n` +
      offenders
        .slice(0, 4)
        .map(
          (o) =>
            `    section #${o.section}: ${o.duplicates.length} duplicate(s) ` +
            `${JSON.stringify(o.duplicates.slice(0, 4))}`,
        )
        .join('\n') +
      `\n    Per-section uniqueness is what \`resolve_key\` and the (section_id, uuid) lookup\n` +
      `    cache rely on, and \`ensure_unique_shot_uuids\` exists to restore it. A survivor\n` +
      `    makes two shots in one section indistinguishable to every consumer that keys on\n` +
      `    uuid - undo anchors, warning backfill, and the import de-duplicator.\n  ${reproduce}`,
  );
}

/** The option set one surface is showing, as the scenario read it back. */
export interface OptionSnapshot {
  exportType: string;
  dataFormat: string;
  imageFormat: string;
  dpi: number;
  branchingThreshold: number;
}

/**
 * X8 - the two surfaces do not share state, and the store resets on a switch.
 *
 * The modal keeps LOCAL `useState` and the panel reads `exportStore`
 * (`ExportDialog.tsx:17-19` vs `ExportPanel.tsx`), which is a deliberate split
 * rather than an accident - so it has to hold in both directions, and a future
 * "just use the store everywhere" would break the dialog's documented
 * open-on-CSV behaviour rather than quietly unify anything.
 */
export function checkOptionIndependence(
  args: { panelBefore: OptionSnapshot; panelAfter: OptionSnapshot; modalChangedTo: OptionSnapshot },
  reproduce: string,
): void {
  const same =
    args.panelBefore.exportType === args.panelAfter.exportType &&
    args.panelBefore.dataFormat === args.panelAfter.dataFormat &&
    args.panelBefore.imageFormat === args.panelAfter.imageFormat &&
    args.panelBefore.dpi === args.panelAfter.dpi &&
    args.panelBefore.branchingThreshold === args.panelAfter.branchingThreshold;
  if (same) return;
  throw new ExportOracleError(
    `X8 changing the MODAL's options changed the PANEL's\n` +
      `    panel before: ${JSON.stringify(args.panelBefore)}\n` +
      `    panel after:  ${JSON.stringify(args.panelAfter)}\n` +
      `    modal set to: ${JSON.stringify(args.modalChangedTo)}\n` +
      `    the dialog owns local state and the panel owns exportStore; they are two\n` +
      `    renders of one component over two option sets, and neither may write the other.\n` +
      `  ${reproduce}`,
  );
}

/** X8b - a project switch drops the Export-mode options. */
export function checkExportStoreReset(
  args: { dirty: OptionSnapshot; afterSwitch: OptionSnapshot; defaults: OptionSnapshot },
  reproduce: string,
): void {
  const matchesDefaults =
    args.afterSwitch.exportType === args.defaults.exportType &&
    args.afterSwitch.dataFormat === args.defaults.dataFormat &&
    args.afterSwitch.imageFormat === args.defaults.imageFormat &&
    args.afterSwitch.dpi === args.defaults.dpi &&
    args.afterSwitch.branchingThreshold === args.defaults.branchingThreshold;
  if (matchesDefaults) return;
  throw new ExportOracleError(
    `X8b the Export-mode options survived a project switch\n` +
      `    set to:        ${JSON.stringify(args.dirty)}\n` +
      `    after loading: ${JSON.stringify(args.afterSwitch)}\n` +
      `    defaults:      ${JSON.stringify(args.defaults)}\n` +
      `    exportStore is app-global, so a format and DPI framed for one survey would\n` +
      `    otherwise render the NEXT one in the Export-mode preview at settings nobody\n` +
      `    chose for it. uiStore.applyDisplaySettings resets it on load/create and\n` +
      `    projectStore.clearOpenProjectState on close.\n  ${reproduce}`,
  );
}

/** What the UI store says about geometry and mode. */
export interface ModeSnapshot {
  appMode: string;
  geometryMode: string;
  /** `isMapReadOnly(state)` (`src/store/uiStore.ts:65-67`), evaluated in page. */
  mapReadOnly: boolean;
  /**
   * `hasEligibleClosures(project.loop_closures)` (`src/utils/closures.ts:33`).
   *
   * Carried because the geometry half of the Export-mode invariant is
   * CONDITIONAL on it - see `checkExportModeInvariants`.
   */
  hasEligibleClosures: boolean;
}

/** `AppMode` and `GeometryMode` (`src/store/uiStore.ts`): the words X9 prints as they are. */
const APP_MODES: ReadonlySet<string> = new Set(['survey', 'plan', 'export']);
const GEOMETRY_MODES: ReadonlySet<string> = new Set(['measured', 'closureAdjusted']);

/**
 * X9 - Export mode forces the read-only map, the adjusted geometry when there is
 * any to adjust, and gives the previous state back.
 *
 * THE TWO HALVES HAVE DIFFERENT STRENGTHS, and getting that wrong is what this
 * scenario's first run did. `setAppMode` sets `geometryMode: 'closureAdjusted'`
 * on the way into plan/export (`uiStore.ts:1596-1660`), but MapCanvas then
 * REVERTS it: "The adjusted view is meaningless without an eligible closure:
 * revert to measured if the last adjustable closure is removed while it is
 * active" (`MapCanvas.tsx:2344-2349`), and a project with no closures at all is
 * that case from the start. So:
 *
 *   - READ-ONLY is unconditional. `isMapReadOnly` is true whenever `appMode` is
 *     not survey, and MapCanvas's own comment says the same from the other side:
 *     the map is read-only in Plan/Export "even on a project with no closures
 *     where the display falls back to measured geometry". This is the half that
 *     protects data, and it has no exemption.
 *   - CLOSURE-ADJUSTED geometry is conditional on there being an eligible
 *     closure. Asserting it unconditionally would report the app's documented
 *     fallback as a defect - which is a false premise, not a finding.
 *
 * The exit check is the one that catches the most, because the state it
 * restores from (`prePlanGeometryMode`) is a module variable nothing can read.
 */
export function checkExportModeInvariants(
  args: { before: ModeSnapshot; during: ModeSnapshot; after: ModeSnapshot },
  reproduce: string,
): void {
  // The modes are read off the app's store: a value outside the store's own
  // words prints as its length, never as text (#576 W51).
  if (args.during.appMode !== 'export') {
    const shown = APP_MODES.has(args.during.appMode) ? args.during.appMode : `${String(args.during.appMode).length} characters`;
    throw new ExportOracleError(`X9 entering Export mode left appMode at "${shown}"\n  ${reproduce}`);
  }
  if (args.during.hasEligibleClosures && args.during.geometryMode !== 'closureAdjusted') {
    const shown = GEOMETRY_MODES.has(args.during.geometryMode)
      ? args.during.geometryMode
      : `${String(args.during.geometryMode).length} characters`;
    throw new ExportOracleError(
      `X9 Export mode did not force closure-adjusted geometry (geometryMode=` +
        `"${shown}") on a project that HAS an adjustable closure\n` +
        `    the export renders what the map shows, and in Export mode the map is the\n` +
        `    adjusted survey; exporting measured geometry from that screen would produce\n` +
        `    a file that does not match the preview beside it.\n` +
        `    (A project with no eligible closure is exempt - MapCanvas reverts to measured\n` +
        `    there by design, and the read-only check below still applies.)\n  ${reproduce}`,
    );
  }
  if (!args.during.mapReadOnly) {
    throw new ExportOracleError(
      `X9 the map was still EDITABLE in Export mode\n` +
        `    UNCONDITIONAL: isMapReadOnly is true whenever appMode is not survey, closures\n` +
        `    or no closures - MapCanvas says so explicitly at the tool-drop effect.\n  ${reproduce}`,
    );
  }
  if (args.after.appMode !== args.before.appMode || args.after.geometryMode !== args.before.geometryMode) {
    throw new ExportOracleError(
      `X9 leaving Export mode did not restore what entering it took\n` +
        `    before: ${JSON.stringify(args.before)}\n` +
        `    after:  ${JSON.stringify(args.after)}\n` +
        `    the snapshot lives in a module variable (prePlanGeometryMode), which nothing\n` +
        `    can read - so this behavioural check is the only thing that covers it.\n  ${reproduce}`,
    );
  }
}
