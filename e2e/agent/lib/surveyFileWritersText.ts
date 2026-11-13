/**
 * Pure text emitters for the three line-oriented survey formats the app imports:
 * Compass `.dat` / `.mak`, Walls `.srv` / `.wpj`, and GPX.
 *
 * Wave 2 of `surveyFileWriters.ts` (MNemo `.dmp` + Ariane `.tml`/`.tmlu`), and it
 * carries the same three promises, for the same reasons:
 *
 * ## Every function is a total, deterministic function of its spec
 *
 * The same spec renders byte-identical output on any machine, in any order. That
 * is what lets a fuzz trace (`fuzzTrace.ts`, step kind `genFile`) store the SPEC
 * and have the replayer rebuild the file - a trace stays a few hundred bytes of
 * JSON instead of an embedded fixture, and a shrunk trace is still runnable.
 * Nothing here may consult the clock, the filesystem, `Math.random`, or a `Map`
 * iteration order that depends on insertion history.
 *
 * ## Numbers are `number | string` and are emitted with `String(value)`
 *
 * Never coerced, never rounded, never range-checked. A corruption catalog needs
 * to carry `nan`, `-inf`, `1e999`, `99999999999` and impossible calendar dates
 * through the SAME field a nominal spec uses; a second "raw" mechanism per field
 * would double the surface and drift. `String(0.1 + 0.2)` is deterministic in
 * JS, so a numeric spec is as replayable as a string one.
 *
 * ## `undefined` and `null` are the same thing: omit
 *
 * A trace is JSON, and `JSON.stringify` DROPS `undefined` properties while it
 * keeps `null`. Making the two equivalent is what makes a spec survive the
 * round-trip with its meaning intact. The one exception is a field with a
 * documented default (`GpxSpec.declaration`, `rootTag`, `rootAttrs`), where
 * `undefined` takes the default and `null`/`''` is the explicit omission - the
 * shape `surveyFileWriters.ts` already uses for `TmlSpec.declaration`.
 *
 * ## No fs, no ctx
 *
 * This module emits text and nothing else. `surveyFileSpecs.ts` owns generation
 * and the one function that touches disk, so the byte layout can be unit-tested
 * with no session, no temp dir and no app. It also means this module supplies NO
 * nominal defaults for content (a `.mak` with no `@`/`&` does not parse, and the
 * writer will happily emit one) - picking sensible content is the generator's
 * job, and every field here is either emitted verbatim or omitted.
 *
 * ## What the framing is transcribed from
 *
 * The parsers, not the format docs:
 * `src-tauri/vendor/compass_data/src/survey/parser/mod.rs`,
 * `.../survey/parser/survey_parameters.rs`, `.../parser_utils.rs`,
 * `.../project/{lexer,parser}.rs`, `src-tauri/src/parsers/compass.rs`,
 * `src-tauri/src/parsers/walls/{srv,units,wpj}.rs`, and
 * `src-tauri/src/parsers/gpx.rs`. Where a format doc and the parser disagree the
 * parser wins - see `compassHasBacksights` for the one place that matters most.
 */

import { escapeXmlText } from './surveyFileWriters';

/**
 * Join rendered lines with `eol`, terminating the LAST one too.
 *
 * Compass `recognize_line` (`parser_utils.rs:38-42`) is `not_line_ending` +
 * `line_ending`, so a final line with no terminator is not a line at all and the
 * row is silently dropped. Walls and GPX do not care, but emitting the trailing
 * terminator everywhere keeps `omitFinalNewline` the only way to lose it.
 */
function joinLines(lines: readonly string[], eol: string, omitFinalNewline: boolean): string {
  if (lines.length === 0) return '';
  const text = lines.join(eol);
  return omitFinalNewline ? text : `${text}${eol}`;
}

// ============================================================================
// Compass .DAT
// ============================================================================

/**
 * One shot row. LRUD is spelled L, U, D, R here because that is the order
 * `parse_shot` reads the columns in (`survey/parser/mod.rs:102-108`) - the
 * FORMAT string's `UDRL` characters are parsed and stored but never consulted
 * when the data row is split, so a writer that reorders by the format string
 * silently transposes every passage dimension.
 */
export interface CompassShotSpec {
  from: string;
  to: string;
  /** Numbers or raw tokens, emitted verbatim - a spec may carry `nan`, `-inf`, `1e999`. */
  length: number | string;
  bearing: number | string;
  inclination: number | string;
  /** Data-row LRUD order is L, U, D, R - not the FORMAT string's order. */
  left: number | string;
  up: number | string;
  down: number | string;
  right: number | string;
  /** Emitted only when the survey's format declares backsights. */
  backBearing?: number | string;
  backInclination?: number | string;
  /** Rendered as `#|<flags>#`. `X` excludes the shot. */
  flags?: string;
  comment?: string;
  /** Escape hatch: this whole line, verbatim, instead of every field above. */
  rawLine?: string;
}

export interface CompassSurveySpec {
  caveName: string;
  name: string;
  /** Numbers or raw tokens, so a spec can carry `-7` or an impossible calendar date. */
  month: number | string;
  day: number | string;
  year: number | string;
  dateComment?: string;
  team: string;
  declination: number | string;
  /** null omits the whole `FORMAT: ` clause. */
  format: string | null;
  /** Emit `FORMAT:` with NO trailing space - the parser then reads no format at all. */
  formatNoSpace?: boolean;
  corrections?: readonly [number | string, number | string, number | string] | null;
  corrections2?: readonly [number | string, number | string] | null;
  /** Defaults to the standard `FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT ...` row. */
  columnHeader?: string;
  shots: readonly CompassShotSpec[];
  /** Drop the newline after the last shot row, so that row is silently lost. */
  omitFinalNewline?: boolean;
}

export interface CompassDatSpec {
  surveys: readonly CompassSurveySpec[];
  /** Between surveys. Defaults to `'\x0c'`. */
  separator?: string;
  /** Defaults to `'\r\n'`. */
  eol?: string;
  /** Emit the separator after the LAST survey too. Defaults to false. */
  trailingSeparator?: boolean;
  /** Prepended verbatim (a BOM, junk, a leading form feed). */
  leading?: string;
  /** Replace the whole document. Every other field is ignored. */
  rawText?: string;
}

/**
 * The survey terminator: ONE literal form feed, 0x0C.
 *
 * `survey/parser/mod.rs:147` is `opt(ws(tag("\x0c")))`, written with a raw 0x0C
 * byte inside the string literal that every editor renders as nothing. `ws` is
 * `multispace0`-delimited and nom 8's `multispace0` matches only space, tab, CR
 * and LF - NOT 0x0C - so exactly one form feed is consumed and a second one
 * lands in the next survey's cave name.
 */
const COMPASS_SEPARATOR = '\x0c';

/**
 * The default column-header row.
 *
 * `gobble_labels` (`survey/parser/mod.rs:60-64`) is `ws(tag("FROM"))` followed by
 * `ws(take_till1(|c| c != '\n'))`: the first non-whitespace token must be exactly
 * `FROM`, and at least one more character must follow before the newline. The
 * rest is discarded, so the text is decorative - but a row of only `FROM` fails
 * `take_till1` and kills the survey.
 */
const COMPASS_DEFAULT_COLUMN_HEADER =
  'FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS';

/**
 * Whether a FORMAT string declares redundant backsights, i.e. whether every data
 * row carries 11 columns instead of 9 (`survey/parser/mod.rs:109-115`).
 *
 * The backsight character is NOT always the 12th. `parse_survey_format`
 * (`survey_parameters.rs:108-200`) consumes 4 unit characters, then 4 passage
 * dimension characters, then the shot-item order - THREE items for an 11/12/13
 * char format but FIVE for a 15 char one. So the backsight slot is index 11 for
 * lengths 12 and 13, and index 13 for length 15. A "12th character" rule reads
 * `DDDDLUDRLADadBF` as having no backsights and truncates its whole shot list.
 *
 * Any other length is not a legal format at all (`survey_parameters.rs:196-199`
 * returns `ErrorKind::LengthValue`), which kills the survey outright; there is
 * no backsight question to answer, so this returns false.
 */
export function compassHasBacksights(format: string | null | undefined): boolean {
  if (format === null || format === undefined) return false;
  if (format.length === 12 || format.length === 13) return format[11] === 'B';
  if (format.length === 15) return format[13] === 'B';
  return false;
}

/**
 * The `DECLINATION:` line, which carries FORMAT and both CORRECTIONS clauses.
 *
 * `parse_survey_parameters` (`survey_parameters.rs:231-257`) reads ONE line and
 * applies `tag("DECLINATION:")` to it, so everything below lives on that line.
 * `tag("FORMAT: ")` at `survey_parameters.rs:110` REQUIRES the trailing space:
 * without it the format silently reads as `SurveyFormat::None` (feet, degrees,
 * no backsights) AND the `CORRECTIONS:` tag then misses too, with no warning.
 * That is what `formatNoSpace` reproduces.
 */
function compassDeclinationLine(survey: CompassSurveySpec): string {
  let line = `DECLINATION: ${String(survey.declination)}`;
  const format = survey.format ?? null;
  if (format !== null) {
    line += `  FORMAT:${survey.formatNoSpace === true ? '' : ' '}${format}`;
  }
  const corrections = survey.corrections ?? null;
  if (corrections !== null) {
    line += `  CORRECTIONS: ${String(corrections[0])} ${String(corrections[1])} ${String(corrections[2])}`;
  }
  const corrections2 = survey.corrections2 ?? null;
  if (corrections2 !== null) {
    line += `  CORRECTIONS2: ${String(corrections2[0])} ${String(corrections2[1])}`;
  }
  return line;
}

/**
 * One data row: two station names then 7 floats, plus 2 more when the survey's
 * format declares backsights.
 *
 * When it does, the two columns are ALWAYS emitted - falling back to `0.00` for
 * a shot that omits them. A declared-but-absent backsight pair makes
 * `parse_double` fail, `many0(parse_shot)` stop, and every remaining row in the
 * survey vanish with no warning (`survey/parser/mod.rs:109-115`); a writer must
 * not be able to trigger that by accident. Conversely, when the format does not
 * declare backsights the columns are never emitted - two extra tokens would fall
 * into the tail and silently become the shot's comment.
 */
function compassShotLine(shot: CompassShotSpec, hasBacksights: boolean): string {
  if (shot.rawLine !== undefined) return shot.rawLine;
  const columns: (number | string)[] = [
    shot.from,
    shot.to,
    shot.length,
    shot.bearing,
    shot.inclination,
    shot.left,
    shot.up,
    shot.down,
    shot.right,
  ];
  if (hasBacksights) {
    columns.push(shot.backBearing ?? '0.00', shot.backInclination ?? '0.00');
  }
  let line = columns.map(String).join(' ');
  // `split_flags_and_comment` (`survey/parser/mod.rs:70-96`) takes the text
  // between `#|` and the next `#` as flags and joins whatever is on either side
  // as the comment, so flags-then-comment is the round-tripping order.
  if (shot.flags !== undefined) line += ` #|${shot.flags}#`;
  if (shot.comment !== undefined) line += ` ${shot.comment}`;
  return line;
}

/**
 * The lines of one survey, in the fixed order `parse_survey` reads them
 * (`survey/parser/mod.rs:135-160`). Every keyword is a `nom::tag`: exact,
 * case-sensitive, anchored at column 0 of its line.
 */
function compassSurveyLines(survey: CompassSurveySpec): string[] {
  const dateComment = survey.dateComment === undefined ? '' : `  COMMENT:${survey.dateComment}`;
  const hasBacksights = compassHasBacksights(survey.format);
  return [
    survey.caveName,
    `SURVEY NAME: ${survey.name}`,
    `SURVEY DATE: ${String(survey.month)} ${String(survey.day)} ${String(survey.year)}${dateComment}`,
    'SURVEY TEAM:',
    // Emitted VERBATIM, including an empty string. `parse_survey_team`
    // (`survey/parser/mod.rs:55`) is `tag("SURVEY TEAM:")` then `multispace1`,
    // and `multispace1` is greedy across newlines - so a blank team line is
    // swallowed, the `DECLINATION:` line is consumed as the team, and the whole
    // survey then fails. An empty `team` is therefore a deliberate corruption
    // spec, not an oversight the writer should paper over with a space.
    survey.team,
    compassDeclinationLine(survey),
    '',
    survey.columnHeader ?? COMPASS_DEFAULT_COLUMN_HEADER,
    '',
    ...survey.shots.map((shot) => compassShotLine(shot, hasBacksights)),
  ];
}

export function writeCompassDat(spec: CompassDatSpec): string {
  if (spec.rawText !== undefined) return spec.rawText;
  const eol = spec.eol ?? '\r\n';
  const separator = spec.separator ?? COMPASS_SEPARATOR;
  const blocks = spec.surveys.map((survey) =>
    joinLines(compassSurveyLines(survey), eol, survey.omitFinalNewline === true),
  );
  const trailing = spec.trailingSeparator === true ? separator : '';
  return `${spec.leading ?? ''}${blocks.join(separator)}${trailing}`;
}

// ============================================================================
// Compass .MAK
// ============================================================================

export interface CompassMakStationSpec {
  name: string;
  /** `[<unit>,<east>,<north>,<elevation>]`; unit is one of f F m M. */
  fix?: { unit: string; east: number | string; north: number | string; elevation: number | string };
}

export interface CompassMakFileSpec {
  /** A path relative to the .mak, normally a sibling basename. */
  path: string;
  stations?: readonly CompassMakStationSpec[];
  /** Wrap this entry in `[<folder>;` ... `];`. */
  folder?: string;
  /** Drop the `;` terminator. */
  omitSemicolon?: boolean;
}

export interface CompassMakSpec {
  projectUuid?: string | null;
  comments?: readonly string[];
  baseLocation?: {
    east: number | string;
    north: number | string;
    elevation: number | string;
    zone: number | string;
    convergence: number | string;
  } | null;
  /** Must be one of COMPASS_DATUMS to parse. */
  datum?: string | null;
  utmZone?: number | string | null;
  fileConvergence?: { value: number | string; enabled: boolean } | null;
  flags?: string | null;
  files: readonly CompassMakFileSpec[];
  /** Emit the `#` entries BEFORE `@`/`&`, which the parser refuses. */
  filesFirst?: boolean;
  eol?: string;
  leading?: string;
  rawText?: string;
}

/**
 * The datum names `parse_datum_name` accepts, transcribed from
 * `vendor/compass_data/src/project/parser.rs:213-244`.
 *
 * The match is a plain `match name { "Adindan" => ... }` on the raw text between
 * `&` and `;`, so it is EXACT and case-sensitive: `&wgs 1984;` fails the whole
 * `.mak` with `Unknown datum 'wgs 1984'`. Order here is the source's order, so a
 * generator that indexes into this array is stable across edits to the enum.
 */
export const COMPASS_DATUMS: readonly string[] = [
  'Adindan',
  'Arc 1950',
  'Arc 1960',
  'Australian 1966',
  'Australian 1984',
  'Camp Area Astro',
  'Cape',
  'European 1950',
  'European 1979',
  'Geodetic 1949',
  'HongKong 1963',
  'HuTzuShan',
  'Indian',
  'North American 1927',
  'North American 1983',
  'Oman',
  'Ordinance Survey 1936',
  'Pulkovo 1942',
  'South American 1956',
  'South American 1969',
  'Tokyo',
  'WGS 1972',
  'WGS 1984',
];

/**
 * A `,<name>` station, optionally carrying a `[unit,e,n,elev]` fix.
 *
 * No spaces anywhere inside the bracket. `lex_station_fix`
 * (`project/lexer.rs:419-470`) allows `multispace0` after `[` and around the
 * unit character but NOWHERE else, so `[m, 1, 2, 3]` is `Invalid station fix
 * format` and takes the whole file with it.
 */
function makStationText(station: CompassMakStationSpec): string {
  const fix = station.fix;
  if (fix === undefined) return `,${station.name}`;
  return `,${station.name}[${fix.unit},${String(fix.east)},${String(fix.north)},${String(fix.elevation)}]`;
}

/**
 * A `#` survey-file entry, optionally wrapped in a `[folder;` ... `];` pair.
 *
 * `lex_survey_file` (`project/lexer.rs:552-569`) finds the FIRST `;` at bracket
 * depth zero, which is why a `;` inside a station fix is safe and why dropping
 * the terminator is `MissingSemicolon` rather than a truncated path.
 */
function makFileLines(file: CompassMakFileSpec): string[] {
  const stations = (file.stations ?? []).map(makStationText).join('');
  const entry = `#${file.path}${stations}${file.omitSemicolon === true ? '' : ';'}`;
  if (file.folder === undefined) return [entry];
  return [`[${file.folder};`, entry, '];'];
}

/**
 * A `.mak` is a flat directive stream, not a line-oriented format: the lexer
 * dispatches on the first non-whitespace character (`project/lexer.rs:179-197`)
 * and any character outside `/ @ & $ % * ! # [ ]` is `UnexpectedChar` and kills
 * the file. One directive per line is a convention, not a requirement.
 *
 * Order matters in exactly one way: `to_file_state` (`project/parser.rs:95-103`)
 * requires both a datum and a base location, so every `#` entry must be preceded
 * by an `@` and an `&`. `filesFirst` inverts that to produce `Survey file
 * encountered before required state was set`.
 *
 * Comments are emitted as `/<text>;`. A comment runs to the next `/`, `\n` or
 * `\r` (`project/lexer.rs:236-241`), so the `;` is just comment text - it is
 * there to keep every emitted directive `;`-terminated. Two consequences a spec
 * author owns: a comment containing `/` or a newline ends early and leaves the
 * remainder to be dispatched as a directive, and a comment whose text before the
 * `;` is >= 32 characters of only hex digits and hyphens is lexed as a project
 * id instead (`project/lexer.rs:220-231`) and then fails `Uuid::parse_str`.
 */
export function writeCompassMak(spec: CompassMakSpec): string {
  if (spec.rawText !== undefined) return spec.rawText;
  const eol = spec.eol ?? '\r\n';

  const head: string[] = [];
  const uuid = spec.projectUuid ?? null;
  if (uuid !== null) head.push(`/${uuid};`);
  for (const comment of spec.comments ?? []) head.push(`/${comment};`);

  const state: string[] = [];
  const base = spec.baseLocation ?? null;
  if (base !== null) {
    // `@east,north,elevation,zone,convergence;` (`project/lexer.rs:274-318`).
    // The lexer's own `parse_double` admits an optional sign, digits and an
    // optional `.` fraction - no exponent, no `nan`, no bare `.5` - and the zone
    // is a `u8`, so `1e2` or `300` here fails the whole file.
    state.push(
      `@${String(base.east)},${String(base.north)},${String(base.elevation)},${String(base.zone)},${String(base.convergence)};`,
    );
  }
  const datum = spec.datum ?? null;
  if (datum !== null) state.push(`&${datum};`);
  const utmZone = spec.utmZone ?? null;
  if (utmZone !== null) state.push(`$${String(utmZone)};`);
  const convergence = spec.fileConvergence ?? null;
  if (convergence !== null) {
    // `%` is convergence-enabled, `*` disabled (`project/lexer.rs:364-394`).
    state.push(`${convergence.enabled ? '%' : '*'}${String(convergence.value)};`);
  }
  const flags = spec.flags ?? null;
  if (flags !== null) state.push(`!${flags};`);

  const files = spec.files.flatMap(makFileLines);
  const lines =
    spec.filesFirst === true ? [...head, ...files, ...state] : [...head, ...state, ...files];
  return `${spec.leading ?? ''}${joinLines(lines, eol, false)}`;
}

// ============================================================================
// Walls .SRV
// ============================================================================

export interface WallsShotSpec {
  from: string;
  /** `-` or `--` makes this a splay. */
  to: string;
  /** Measurement columns in the survey's active order, emitted verbatim. */
  columns: readonly (number | string)[];
  /** Rendered as `<a,b,c,d>` (or `*a,b,c,d*` when `lrudStar`). */
  lrud?: readonly (number | string)[] | null;
  lrudStar?: boolean;
  /** Trailing `;` comment. */
  comment?: string;
  rawLine?: string;
}

export interface WallsFixSpec {
  station: string;
  /** Three coordinate columns, verbatim - plain lengths or `W97:43:52.5` forms. */
  coords: readonly (number | string)[];
  comment?: string;
}

export interface WallsSrvSpec {
  /** `;`-prefixed lines emitted first; one may carry `Team: ...`. */
  header?: readonly string[];
  /** `#Date <value>`; null omits it. */
  date?: string | null;
  /** The ARGUMENT text of `#Units ...`; null omits the directive. */
  units?: string | null;
  /** Extra whole directive lines emitted after `#Units` (`#Segment /a`, `#Frobnicate x`). */
  directives?: readonly string[];
  fixes?: readonly WallsFixSpec[];
  shots: readonly WallsShotSpec[];
  /** Lines wrapped in `#[` ... `#]`. */
  blockComment?: readonly string[];
  /** Emit the opening `#[` without its `#]`, swallowing everything after it. */
  unterminatedBlockComment?: boolean;
  eol?: string;
  leading?: string;
  rawText?: string;
}

/**
 * A vector line: FROM, TO, the measurement columns, then an optional LRUD span
 * and an optional `;` comment.
 *
 * The LRUD delimiter is emitted after a space and, when a comment follows, with
 * a space after the closer. `find_span_end` (`walls/srv.rs:461-464`) only treats
 * `<`/`*` as a span when it opens at a token boundary AND its closer sits at
 * end-of-body or immediately before whitespace - otherwise the delimiter is a
 * literal station-name character and the "LRUD" is swallowed into a token.
 */
function wallsShotLine(shot: WallsShotSpec): string {
  if (shot.rawLine !== undefined) return shot.rawLine;
  let line = [shot.from, shot.to, ...shot.columns.map(String)].join(' ');
  const lrud = shot.lrud ?? null;
  if (lrud !== null) {
    const delimiters = shot.lrudStar === true ? ['*', '*'] : ['<', '>'];
    line += ` ${delimiters[0]}${lrud.map(String).join(',')}${delimiters[1]}`;
  }
  if (shot.comment !== undefined) line += ` ;${shot.comment}`;
  return line;
}

/**
 * `#Fix <station> <c1> <c2> <c3> [;comment]`.
 *
 * The three coordinate columns are consumed in `units.rect_order` (default E, N,
 * U) and NOT in the vector order (`walls/srv.rs:628`), so a dive survey's
 * `Order=DA` cannot truncate the elevation column. An `N`/`S`/`E`/`W`-prefixed
 * token like `W97:43:52.5` is a geographic angle placed by its prefix letter
 * rather than by its column (`walls/srv.rs:635-666`).
 */
function wallsFixLine(fix: WallsFixSpec): string {
  let line = `#Fix ${fix.station} ${fix.coords.map(String).join(' ')}`;
  if (fix.comment !== undefined) line += ` ;${fix.comment}`;
  return line;
}

/**
 * A `.srv` is one survey, line-oriented and indentation-insensitive: every line
 * is `trim_end_matches(['\r','\n'])` then `.trim()` (`walls/srv.rs:49-51`), so
 * `\r\n` versus `\n` and any leading whitespace are irrelevant.
 *
 * The block comment is emitted BEFORE the shots on purpose: `#[` raises a depth
 * counter that skips every following line until `#]`
 * (`walls/srv.rs:56-66`), so `unterminatedBlockComment` swallowing the shot list
 * is only reachable when the opener precedes it.
 */
export function writeWallsSrv(spec: WallsSrvSpec): string {
  if (spec.rawText !== undefined) return spec.rawText;
  const eol = spec.eol ?? '\r\n';
  const lines: string[] = [];
  for (const header of spec.header ?? []) lines.push(`;${header}`);
  const date = spec.date ?? null;
  if (date !== null) lines.push(`#Date ${date}`);
  const units = spec.units ?? null;
  if (units !== null) lines.push(`#Units ${units}`);
  // Emitted verbatim: `handle_directive` (`walls/srv.rs:184-219`) lowercases the
  // key and SILENTLY ignores anything it does not know, so an invented directive
  // is a no-op rather than a warning - which is exactly the corruption shape.
  for (const directive of spec.directives ?? []) lines.push(directive);
  for (const fix of spec.fixes ?? []) lines.push(wallsFixLine(fix));
  const blockComment = spec.blockComment ?? [];
  if (blockComment.length > 0 || spec.unterminatedBlockComment === true) {
    lines.push('#[');
    for (const line of blockComment) lines.push(line);
    if (spec.unterminatedBlockComment !== true) lines.push('#]');
  }
  for (const shot of spec.shots) lines.push(wallsShotLine(shot));
  return `${spec.leading ?? ''}${joinLines(lines, eol, false)}`;
}

// ============================================================================
// Walls .WPJ
// ============================================================================

export interface WallsWpjEntrySpec {
  kind: 'book' | 'survey';
  title: string;
  name?: string | null;
  status?: number | string | null;
  options?: string | null;
  path?: string | null;
  /** The whole `.REF` argument text. */
  ref?: string | null;
  /** Books only. */
  children?: readonly WallsWpjEntrySpec[];
}

export interface WallsWpjSpec {
  header?: readonly string[];
  root: WallsWpjEntrySpec;
  eol?: string;
  rawText?: string;
}

/**
 * One `.BOOK`/`.SURVEY` entry and its properties.
 *
 * Property directives attach to the most recent `.BOOK`/`.SURVEY` - the
 * `pending` entry in `parse_wpj` (`walls/wpj.rs:220-284`) - so they must appear
 * after it and before the next `.BOOK`, `.SURVEY` or `.ENDBOOK`. Children are
 * emitted after the properties, and `.ENDBOOK` closes a book (it flushes the
 * pending entry and pops the stack, `walls/wpj.rs:244-247`).
 *
 * `children` on a `survey` entry is ignored: a leaf has no `.ENDBOOK` to close,
 * so nesting under it would silently reparent the children onto the enclosing
 * book instead of producing the structure the spec describes.
 */
function wpjEntryLines(entry: WallsWpjEntrySpec): string[] {
  const lines: string[] = [`${entry.kind === 'book' ? '.BOOK' : '.SURVEY'} ${entry.title}`];
  const name = entry.name ?? null;
  // A `.NAME` is a bare filename that gets `.SRV` appended; `resolve_survey`
  // (`walls/wpj.rs:362-364`) rejects one containing `/`, `\` or `:` as a
  // traversal attempt and reports the survey missing. Emitted verbatim so a spec
  // can produce exactly that.
  if (name !== null) lines.push(`.NAME ${name}`);
  const status = entry.status ?? null;
  if (status !== null) lines.push(`.STATUS ${String(status)}`);
  const options = entry.options ?? null;
  if (options !== null) lines.push(`.OPTIONS ${options}`);
  const path = entry.path ?? null;
  if (path !== null) lines.push(`.PATH ${path}`);
  const ref = entry.ref ?? null;
  // `.REF` needs >= 12 finite whitespace-separated numbers then a quoted datum
  // (`walls/wpj.rs:417-460`); anything else is one warning and no georeference.
  if (ref !== null) lines.push(`.REF ${ref}`);
  if (entry.kind === 'book') {
    for (const child of entry.children ?? []) lines.push(...wpjEntryLines(child));
    lines.push('.ENDBOOK');
  }
  return lines;
}

/**
 * A `.wpj` is flat and indentation-insensitive (`line = raw.trim()`,
 * `walls/wpj.rs:213`) and its first token is uppercased before matching
 * (`walls/wpj.rs:217-219`), so `.book` and `.BOOK` are the same directive and
 * any unknown one is silently ignored.
 */
export function writeWallsWpj(spec: WallsWpjSpec): string {
  if (spec.rawText !== undefined) return spec.rawText;
  const eol = spec.eol ?? '\r\n';
  const lines = [
    ...(spec.header ?? []).map((header) => `;${header}`),
    ...wpjEntryLines(spec.root),
  ];
  return joinLines(lines, eol, false);
}

// ============================================================================
// GPX
// ============================================================================

export interface GpxPointSpec {
  /** Verbatim attribute text, so a spec can carry `91`, `nan`, `abc`, or omit one. */
  lat?: number | string;
  lon?: number | string;
  name?: string;
  ele?: number | string;
  time?: string;
  cmt?: string;
  desc?: string;
  /** Emit `<tag .../>` with no children. */
  selfClosing?: boolean;
  /** Wrap `ele` in a CDATA section, which the reader silently drops. */
  eleCdata?: boolean;
}

export interface GpxTrackSpec {
  name?: string;
  cmt?: string;
  desc?: string;
  segments: readonly (readonly GpxPointSpec[])[];
  selfClosing?: boolean;
}

export interface GpxRouteSpec {
  name?: string;
  cmt?: string;
  desc?: string;
  points: readonly GpxPointSpec[];
}

export interface GpxSpec {
  /** null omits the XML declaration. */
  declaration?: string | null;
  /** Defaults to `gpx`; a value like `g:gpx` exercises the prefix-stripping path. */
  rootTag?: string;
  /** Verbatim attribute text on the root element. Defaults to a GPX 1.1 xmlns. */
  rootAttrs?: string;
  metadataName?: string | null;
  waypoints?: readonly GpxPointSpec[];
  tracks?: readonly GpxTrackSpec[];
  routes?: readonly GpxRouteSpec[];
  omitClosingRoot?: boolean;
  trailing?: string;
  rawText?: string;
}

const GPX_DEFAULT_DECLARATION = '<?xml version="1.0" encoding="UTF-8"?>';

/**
 * The default root attributes.
 *
 * Neither is required: the reader scans for a `Start` element whose LOCAL name
 * is `gpx` and never reads `version` or any namespace (`gpx.rs:137-150`,
 * `strip_ns` at `gpx.rs:105-116`). They are here because a real file has them,
 * and `rootAttrs: ''` is how a spec drops them.
 */
const GPX_DEFAULT_ROOT_ATTRS =
  'version="1.1" creator="stalagmight-fuzz" xmlns="http://www.topografix.com/GPX/1/1"';

/** Element text is DATA: escaped through the wave-1 escaper, never re-implemented. */
function gpxTextElement(tag: string, value: string | number): string {
  return `<${tag}>${escapeXmlText(String(value))}</${tag}>`;
}

/**
 * `lat`/`lon` attribute text, emitted VERBATIM and unescaped.
 *
 * Deliberate: `parse_lat_lon` (`gpx.rs:222-259`) requires both attributes and
 * runs each through `str::parse::<f64>()` then a range check, so a spec must be
 * able to write `lat="nan"`, `lat="1e999"`, `lat="abc"` or no `lat` at all. A
 * single out-of-range value aborts the WHOLE file - there is no per-point skip.
 */
function gpxPointAttrs(point: GpxPointSpec): string {
  let attrs = '';
  if (point.lat !== undefined) attrs += ` lat="${String(point.lat)}"`;
  if (point.lon !== undefined) attrs += ` lon="${String(point.lon)}"`;
  return attrs;
}

/**
 * One `<wpt>` / `<trkpt>` / `<rtept>`, children inline on a single line.
 *
 * Inline because the reader's guards are on ELEMENT depth, not on layout: a
 * `<name>`/`<cmt>`/`<ele>` is only read at depth 2 inside its point
 * (`gpx.rs:340-415`, `gpx.rs:655-715`), and one line per point makes that
 * relationship impossible to break with an indentation edit.
 */
function gpxPointLines(tag: string, point: GpxPointSpec, indent: string): string[] {
  const attrs = gpxPointAttrs(point);
  if (point.selfClosing === true) return [`${indent}<${tag}${attrs} />`];
  const children: string[] = [];
  if (point.name !== undefined) children.push(gpxTextElement('name', point.name));
  if (point.cmt !== undefined) children.push(gpxTextElement('cmt', point.cmt));
  if (point.desc !== undefined) children.push(gpxTextElement('desc', point.desc));
  if (point.ele !== undefined) {
    // `read_text` (`gpx.rs:262-281`) matches `Event::Text` and skips nested
    // elements; `Event::CData` is NOT handled, so CDATA content is silently
    // dropped and `<ele>` reads as None - depth 0 relative to the first point.
    children.push(
      point.eleCdata === true
        ? `<ele><![CDATA[${String(point.ele)}]]></ele>`
        : gpxTextElement('ele', point.ele),
    );
  }
  if (point.time !== undefined) children.push(gpxTextElement('time', point.time));
  return [`${indent}<${tag}${attrs}>${children.join('')}</${tag}>`];
}

/** `<trk>`: name/cmt/desc at depth 2, then zero or more `<trkseg>` (`gpx.rs:418-476`). */
function gpxTrackLines(track: GpxTrackSpec): string[] {
  // A self-closing `<trk/>` never produces a `Start` event, so `parse_track` is
  // never called and the whole track is ignored (`gpx.rs:154-216`).
  if (track.selfClosing === true) return ['  <trk />'];
  const lines = ['  <trk>'];
  if (track.name !== undefined) lines.push(`    ${gpxTextElement('name', track.name)}`);
  if (track.cmt !== undefined) lines.push(`    ${gpxTextElement('cmt', track.cmt)}`);
  if (track.desc !== undefined) lines.push(`    ${gpxTextElement('desc', track.desc)}`);
  for (const segment of track.segments) {
    // A segment with fewer than 2 points is skipped at import but still consumes
    // a `section_index` (`gpx.rs:920-923`), shifting every later placement.
    lines.push('    <trkseg>');
    for (const point of segment) lines.push(...gpxPointLines('trkpt', point, '      '));
    lines.push('    </trkseg>');
  }
  lines.push('  </trk>');
  return lines;
}

/** `<rte>`: name/cmt/desc at depth 2, then `<rtept>` children (`gpx.rs:579-652`). */
function gpxRouteLines(route: GpxRouteSpec): string[] {
  const lines = ['  <rte>'];
  if (route.name !== undefined) lines.push(`    ${gpxTextElement('name', route.name)}`);
  if (route.cmt !== undefined) lines.push(`    ${gpxTextElement('cmt', route.cmt)}`);
  if (route.desc !== undefined) lines.push(`    ${gpxTextElement('desc', route.desc)}`);
  for (const point of route.points) lines.push(...gpxPointLines('rtept', point, '    '));
  lines.push('  </rte>');
  return lines;
}

/**
 * The GPX document.
 *
 * `\n` line endings, not `\r\n`: this is the one format here read by an XML
 * parser rather than by a hand-rolled line scanner, and `\n` is what every GPX
 * writer in the wild emits.
 */
export function writeGpxXml(spec: GpxSpec): string {
  if (spec.rawText !== undefined) return spec.rawText;
  const declaration =
    spec.declaration === null ? null : (spec.declaration ?? GPX_DEFAULT_DECLARATION);
  const rootTag = spec.rootTag ?? 'gpx';
  const rootAttrs = spec.rootAttrs ?? GPX_DEFAULT_ROOT_ATTRS;

  const lines: string[] = [];
  if (declaration !== null) lines.push(declaration);
  lines.push(`<${rootTag}${rootAttrs === '' ? '' : ` ${rootAttrs}`}>`);
  const metadataName = spec.metadataName ?? null;
  // Only a direct `<name>` child of `<metadata>` (depth 2) becomes the file name
  // (`gpx.rs:305-337`), so it stays on one line with its parent.
  if (metadataName !== null) {
    lines.push(`  <metadata>${gpxTextElement('name', metadataName)}</metadata>`);
  }
  for (const waypoint of spec.waypoints ?? []) lines.push(...gpxPointLines('wpt', waypoint, '  '));
  for (const track of spec.tracks ?? []) lines.push(...gpxTrackLines(track));
  for (const route of spec.routes ?? []) lines.push(...gpxRouteLines(route));
  // Dropping the closing tag leaves the child loop's depth counter above zero,
  // so it exits on `Event::Eof` with whatever it has (`gpx.rs:207`) rather than
  // erroring - a truncated file that still imports.
  if (spec.omitClosingRoot !== true) lines.push(`</${rootTag}>`);
  return `${joinLines(lines, '\n', false)}${spec.trailing ?? ''}`;
}
