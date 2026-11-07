/**
 * These writers are the fuzzer's contact surface with three more real parsers,
 * and every one of them fails SILENTLY on a framing mistake: Compass `many0`
 * stops at the first bad row and returns what it has, Walls skips a vector line
 * with fewer than two tokens without a warning, GPX drops a `<trkseg>` with one
 * point while still burning its section index. A writer bug therefore shows up
 * as "the import preview says 0 surveys" hours later, inside the app, with no
 * error to grep for.
 *
 * So the tests below are TS MIRRORS of the parsers' framing rules - the survey
 * splitter, the `.mak` first-character dispatch, the `.srv` line classifier and
 * `extract_spans`, the `.wpj` directive walk, an XML element scanner - and the
 * writers' output is round-tripped through them. A framing mistake fails here.
 *
 * Determinism gets its own case per format: a trace stores the SPEC, so a spec
 * that renders two ways is a trace that does not replay.
 */
import { describe, expect, it } from 'vitest';

import {
  COMPASS_DATUMS,
  type CompassDatSpec,
  type CompassMakSpec,
  type CompassShotSpec,
  type CompassSurveySpec,
  type GpxSpec,
  type WallsSrvSpec,
  type WallsWpjSpec,
  compassHasBacksights,
  writeCompassDat,
  writeCompassMak,
  writeGpxXml,
  writeWallsSrv,
  writeWallsWpj,
} from './surveyFileWritersText';

/**
 * `items[index]`, throwing exactly where reading a property of a missing entry
 * would have thrown anyway, so noUncheckedIndexedAccess needs no fallback.
 */
function at<T>(items: readonly T[], index: number): T {
  const item = items[index];
  if (item === undefined) throw new Error(`no entry ${index} (of ${items.length})`);
  return item;
}

// ---------------------------------------------------------------------------
// Mirror: Compass .DAT framing
// ---------------------------------------------------------------------------

/**
 * `many0(parse_survey)` framing (`survey/parser/mod.rs:162-164`).
 *
 * There is no explicit separator token: the parser simply re-enters
 * `parse_survey` on what is left, and the optional trailer at
 * `survey/parser/mod.rs:147` eats ONE literal `\x0c`. Splitting on `\x0c` is
 * therefore the framing, and a trailing separator leaves an empty tail the
 * parser would just fail to start a survey on.
 */
function splitCompassSurveys(text: string): [string, ...string[]] {
  const blocks = text.split('\x0c');
  if (blocks.length > 1 && blocks[blocks.length - 1] === '') blocks.pop();
  // Never empty: split() yields at least one block and pop() needs two.
  return blocks as [string, ...string[]];
}

/**
 * `recognize_line` (`parser_utils.rs:38-42`) = `not_line_ending` + `line_ending`.
 *
 * A line only exists if it is terminated, so an unterminated tail is dropped
 * rather than yielded; and `not_line_ending` rejects a `\r` that is not followed
 * by `\n`, which is why bare-CR files parse to nothing.
 */
function compassLines(block: string): string[] {
  const lines: string[] = [];
  let rest = block;
  while (rest.length > 0) {
    const at = rest.indexOf('\n');
    if (at === -1) return lines; // no terminator: the tail is not a line
    const line = rest.slice(0, at);
    const cr = line.indexOf('\r');
    if (cr !== -1 && cr !== line.length - 1) throw new Error('bare CR: not_line_ending fails');
    lines.push(line.endsWith('\r') ? line.slice(0, -1) : line);
    rest = rest.slice(at + 1);
  }
  return lines;
}

/**
 * The data rows of one survey block: everything after the `FROM` column header
 * (`gobble_labels`, `survey/parser/mod.rs:60-64`), whitespace-split the way
 * `parse_station_name`/`parse_double` consume it (any run of spaces or tabs).
 */
function compassDataRows(block: string): string[][] {
  const lines = compassLines(block);
  const headerAt = lines.findIndex((line) => line.trimStart().startsWith('FROM'));
  if (headerAt === -1) return [];
  return lines
    .slice(headerAt + 1)
    .filter((line) => line.trim() !== '')
    .map((line) => line.trim().split(/\s+/));
}

// ---------------------------------------------------------------------------
// Mirror: Compass .MAK lexer
// ---------------------------------------------------------------------------

/** The only characters `tokenize` dispatches on (`project/lexer.rs:179-197`). */
const MAK_DISPATCH_CHARS = ['/', '@', '&', '$', '%', '*', '!', '#', '[', ']'];

interface MakDirective {
  char: string;
  /** The directive text including its `;`. */
  text: string;
}

/**
 * `tokenize` (`project/lexer.rs:157-207`): skip whitespace and `\x1a`, dispatch
 * on the first non-whitespace character, consume to the terminating `;`.
 *
 * The `#` entry's terminator is the first `;` at bracket depth zero
 * (`project/lexer.rs:552-569`), so a `;` inside a `[unit,e,n,elev]` station fix
 * does not end the entry. Every other directive here is `;`-terminated too,
 * which is why the writer gives even a comment a `;`.
 */
function lexMak(text: string): MakDirective[] {
  const directives: MakDirective[] = [];
  let index = 0;
  while (index < text.length) {
    while (index < text.length && (/\s/.test(text[index]!) || text[index] === '\x1a')) index += 1;
    if (index >= text.length) break;
    const char = text[index]!; // index < text.length: the break above
    if (!MAK_DISPATCH_CHARS.includes(char)) throw new Error(`Unexpected character '${char}'`);
    let depth = 0;
    let end = -1;
    for (let scan = index + 1; scan < text.length; scan += 1) {
      const c = text[scan];
      if (c === '[') depth += 1;
      else if (c === ']') depth -= 1;
      else if (c === ';' && depth === 0) {
        end = scan;
        break;
      }
    }
    if (end === -1) throw new Error('Missing semicolon terminator');
    directives.push({ char, text: text.slice(index, end + 1) });
    index = end + 1;
  }
  return directives;
}

// ---------------------------------------------------------------------------
// Mirror: Walls .SRV line classifier and extract_spans
// ---------------------------------------------------------------------------

type WallsLineKind = 'blank' | 'blockOpen' | 'blockClose' | 'inBlock' | 'comment' | 'directive' | 'vector';

/**
 * `parse_srv_contents`' line loop (`walls/srv.rs:48-118`), in its exact
 * precedence: `#[`, `#]`, in-block, `;`, `#`, otherwise a vector line.
 *
 * Rust's `str::lines()` splits on `\n` and strips one trailing `\r`, and never
 * yields a final empty line for a `\n`-terminated file.
 */
function classifyWallsSrv(text: string): WallsLineKind[] {
  const raw = text.split('\n');
  if (raw.length > 0 && raw[raw.length - 1] === '') raw.pop();
  let depth = 0;
  return raw.map((line) => {
    const trimmed = line.replace(/\r$/, '').trim();
    if (trimmed === '') return 'blank';
    if (trimmed.startsWith('#[')) {
      depth += 1;
      return 'blockOpen';
    }
    if (trimmed.startsWith('#]')) {
      depth = Math.max(0, depth - 1);
      return 'blockClose';
    }
    if (depth > 0) return 'inBlock';
    if (trimmed.startsWith(';')) return 'comment';
    if (trimmed.startsWith('#')) return 'directive';
    return 'vector';
  });
}

/**
 * `find_span_end` (`walls/srv.rs:461-464`): the closer counts only when it sits
 * at end-of-body or immediately before whitespace. Otherwise the delimiter is a
 * literal station-name character.
 */
function findSpanEnd(chars: readonly string[], start: number, close: string): number {
  for (let j = start; j < chars.length; j += 1) {
    const next = chars[j + 1];
    if (chars[j] === close && (next === undefined || /\s/.test(next))) return j;
  }
  return -1;
}

/** `extract_spans` (`walls/srv.rs:468-526`). */
function extractSpans(body: string): { tokens: string[]; lrud: string | null } {
  const chars = [...body];
  const tokens: string[] = [];
  let current = '';
  let lrud: string | null = null;
  const flush = (): void => {
    if (current !== '') {
      tokens.push(current);
      current = '';
    }
  };
  let i = 0;
  while (i < chars.length) {
    const c = chars[i]!;
    if (/\s/.test(c)) {
      flush();
    } else if (c === '#') {
      // Inline segment directive: stop collecting measurement tokens.
      flush();
      break;
    } else if ((c === '<' || c === '*') && current === '') {
      const end = findSpanEnd(chars, i + 1, c === '<' ? '>' : '*');
      if (end !== -1) {
        lrud = chars.slice(i + 1, end).join('');
        i = end + 1;
        continue;
      }
      current += c; // unterminated -> literal name character
    } else if (c === '(' && current === '') {
      const close = chars.indexOf(')', i + 1);
      i = close === -1 ? chars.length : close + 1;
      continue;
    } else {
      current += c;
    }
    i += 1;
  }
  flush();
  return { tokens, lrud };
}

/** `parse_vector_line`'s framing only (`walls/srv.rs:234-256`). */
function parseWallsVector(
  line: string,
): { from: string; to: string; columns: string[]; lrud: string | null } | null {
  const at = line.indexOf(';');
  const body = at === -1 ? line : line.slice(0, at);
  const { tokens, lrud } = extractSpans(body);
  if (tokens.length < 2) return null; // skipped silently (`walls/srv.rs:246-248`)
  return { from: tokens[0]!, to: tokens[1]!, columns: tokens.slice(2), lrud };
}

/** `is_anonymous` (`walls/units.rs:441-443`): exactly `-` or `--`, never `---`. */
function isAnonymousStation(token: string): boolean {
  return token === '-' || token === '--';
}

// ---------------------------------------------------------------------------
// Mirror: Walls .WPJ directive walk
// ---------------------------------------------------------------------------

interface WpjDirective {
  name: string;
  rest: string;
}

/** `parse_wpj`'s loop (`walls/wpj.rs:212-219`): trim, skip blanks and `;`, uppercase token 0. */
function wpjDirectives(text: string): WpjDirective[] {
  const out: WpjDirective[] = [];
  for (const raw of text.split('\n')) {
    const line = raw.replace(/\r$/, '').trim();
    if (line === '' || line.startsWith(';')) continue;
    const at = line.search(/\s/);
    out.push(
      at === -1
        ? { name: line.toUpperCase(), rest: '' }
        : { name: line.slice(0, at).toUpperCase(), rest: line.slice(at).trim() },
    );
  }
  return out;
}

/** `parse_ref` (`walls/wpj.rs:417-433`): pull the quoted datum, then split the head. */
function parseWpjRef(arg: string): { fields: number[]; datum: string } {
  const end = arg.lastIndexOf('"');
  if (end === -1) return { fields: arg.trim().split(/\s+/).map(Number), datum: '' };
  const start = arg.slice(0, end).lastIndexOf('"');
  if (start === -1) throw new Error('unterminated datum quote');
  return {
    fields: arg
      .slice(0, start)
      .trim()
      .split(/\s+/)
      .map((token) => Number(token)),
    datum: arg.slice(start + 1, end),
  };
}

// ---------------------------------------------------------------------------
// Mirror: a GPX element scanner
// ---------------------------------------------------------------------------

interface ScannedElement {
  /** Local name, namespace prefix stripped exactly as `strip_ns` does (`gpx.rs:105-116`). */
  tag: string;
  attrs: string;
  selfClosing: boolean;
}

/**
 * Enough of an XML scanner to check framing: start tags, their attribute text
 * and whether they self-close. It deliberately does NOT understand CDATA or
 * comments, because the writer only emits those inside `<ele>` where a `<` can
 * never appear.
 */
function scanXmlElements(xml: string): ScannedElement[] {
  const out: ScannedElement[] = [];
  const pattern = /<(\/?)([A-Za-z_][\w.:-]*)((?:"[^"]*"|[^<>"])*?)(\/?)>/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(xml)) !== null) {
    // All four groups are non-optional, so each is a string on every match.
    if (match[1] === '/') continue;
    out.push({ tag: match[2]!.split(':').pop() ?? '', attrs: match[3]!, selfClosing: match[4] === '/' });
  }
  return out;
}

function hasAttribute(attrs: string, name: string): boolean {
  return new RegExp(`\\s${name}="`).test(attrs);
}

// ---------------------------------------------------------------------------
// Nominal specs
// ---------------------------------------------------------------------------

/**
 * `D` bearing, `D` decimal feet, `D` passage feet, `D` degrees, `LUDR` passage
 * order, `ADL` shot items, `B` redundant backsight, `F` from-station LRUD.
 * 13 characters, backsight slot at index 11 - the legal shape the parser's own
 * fixtures use.
 */
const FORMAT_13_BACKSIGHT = 'DDDDLUDRADLBF';
/** The same layout with `N` in the backsight slot: 9 data columns, not 11. */
const FORMAT_13_NO_BACKSIGHT = 'DDDDLUDRADLNF';

function compassShot(overrides: Partial<CompassShotSpec> = {}): CompassShotSpec {
  return {
    from: 'A1',
    to: 'A2',
    length: '10.00',
    bearing: '90.00',
    inclination: '-3.00',
    left: '1.00',
    up: '2.00',
    down: '3.00',
    right: '4.00',
    backBearing: '270.00',
    backInclination: '3.00',
    ...overrides,
  };
}

function compassSurvey(overrides: Partial<CompassSurveySpec> = {}): CompassSurveySpec {
  return {
    caveName: 'SYNTH CAVE',
    name: 'A',
    month: 7,
    day: 10,
    year: 1979,
    team: 'NOBODY',
    declination: '1.00',
    format: FORMAT_13_BACKSIGHT,
    corrections: ['0.00', '0.00', '0.00'],
    shots: [compassShot(), compassShot({ from: 'A2', to: 'A3', length: '12.50' })],
    ...overrides,
  };
}

function compassDat(overrides: Partial<CompassDatSpec> = {}): CompassDatSpec {
  return { surveys: [compassSurvey()], ...overrides };
}

function compassMak(overrides: Partial<CompassMakSpec> = {}): CompassMakSpec {
  return {
    projectUuid: '2602942a-ef0c-46d5-b528-77a72c77239c',
    comments: ['synthetic project'],
    baseLocation: {
      east: '500000.000',
      north: '4000000.000',
      elevation: '100.000',
      zone: 13,
      convergence: '-0.260',
    },
    datum: 'North American 1983',
    utmZone: 13,
    fileConvergence: { value: '-0.26', enabled: true },
    flags: 'EV',
    files: [
      {
        path: 'synth.dat',
        stations: [
          { name: 'A1', fix: { unit: 'm', east: '500000.000', north: '4000000.000', elevation: '100.000' } },
        ],
      },
    ],
    ...overrides,
  };
}

function wallsSrv(overrides: Partial<WallsSrvSpec> = {}): WallsSrvSpec {
  return {
    header: [' Synthetic survey - Team: Nobody'],
    date: '2020-06-15',
    units: 'meters Order=DAV Decl=1.5',
    fixes: [{ station: 'A0', coords: ['W97:43:52.5', 'N31:16:45.0', '220'] }],
    shots: [
      { from: 'A0', to: 'A1', columns: ['10.00', '90.00', '-3.00'], lrud: [1, 2, 3, 4] },
      { from: 'A1', to: 'A2', columns: ['12.50', '95.00/275.00', '-4.00/4.00'] },
      { from: 'A1', to: '-', columns: ['3.20', '200.00', '0.00'] },
      { from: 'A2', to: '--', columns: ['1.10', '20.00', '0.00'] },
      { from: 'A2', to: '---', columns: ['8.00', '100.00', '2.00'] },
    ],
    blockComment: ['A2 A9 99 99 99   ; this whole block is ignored'],
    ...overrides,
  };
}

/** 12 numeric fields then a quoted datum, the minimum `parse_ref` accepts. */
const WPJ_REF = '2258942.000 467427.000 16 -0.109 30 6 20 25 42.950 87 18 44.073 19 "NAD83"';

function wallsWpj(overrides: Partial<WallsWpjSpec> = {}): WallsWpjSpec {
  return {
    header: ['WALLS Project File'],
    root: {
      kind: 'book',
      title: 'Synthetic Project',
      name: 'SYNTHPROJ',
      status: 8,
      ref: WPJ_REF,
      children: [{ kind: 'survey', title: 'Main Line', name: 'SYNTH', status: 8 }],
    },
    ...overrides,
  };
}

function gpx(overrides: Partial<GpxSpec> = {}): GpxSpec {
  return {
    metadataName: 'Synthetic Survey',
    waypoints: [{ lat: '45.500000', lon: '-90.500000', name: 'Entrance', cmt: 'sump pool', ele: '350.2' }],
    tracks: [
      {
        name: 'Approach',
        segments: [
          [
            { lat: '45.500000', lon: '-90.500000', ele: '350.2' },
            { lat: '45.500500', lon: '-90.499500', ele: '351.0' },
            { lat: '45.501000', lon: '-90.499000', ele: '348.0' },
          ],
        ],
      },
    ],
    routes: [
      {
        name: 'Planned',
        points: [
          { lat: '45.500000', lon: '-90.500000', name: 'Start', ele: '350.2' },
          { lat: '45.502000', lon: '-90.498000', name: 'End', ele: '345.0' },
        ],
      },
    ],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Compass .DAT
// ---------------------------------------------------------------------------

describe('writeCompassDat framing', () => {
  it('splits into one block per survey, each with the exact tags at column 0', () => {
    const spec = compassDat({
      surveys: [
        compassSurvey({ name: 'A' }),
        compassSurvey({ name: 'B', caveName: 'OTHER CAVE' }),
        compassSurvey({ name: 'C' }),
      ],
    });
    const blocks = splitCompassSurveys(writeCompassDat(spec));
    expect(blocks).toHaveLength(3);

    for (const [index, block] of blocks.entries()) {
      const lines = compassLines(block);
      // Every keyword is a `nom::tag` applied to the line slice, so it must sit
      // at column 0 and match case exactly (`survey/parser/mod.rs:29, 37, 55`).
      expect(lines[0]).toBe(index === 1 ? 'OTHER CAVE' : 'SYNTH CAVE');
      expect(at(lines, 1).startsWith('SURVEY NAME: ')).toBe(true);
      expect(at(lines, 2).startsWith('SURVEY DATE:')).toBe(true);
      expect(lines[3]).toBe('SURVEY TEAM:');
      // `multispace1` after the tag is greedy across newlines, so a BLANK team
      // line would swallow the parameter line and kill the survey.
      expect(at(lines, 4).trim()).not.toBe('');
      expect(at(lines, 5).startsWith('DECLINATION:')).toBe(true);
    }
    expect(blocks.map((block) => compassLines(block)[1])).toEqual([
      'SURVEY NAME: A',
      'SURVEY NAME: B',
      'SURVEY NAME: C',
    ]);
  });

  it('emits exactly one form feed between surveys, and none after the last by default', () => {
    const two = writeCompassDat(compassDat({ surveys: [compassSurvey(), compassSurvey()] }));
    // Two in a row would leave one in the next cave name; the trailer at
    // `survey/parser/mod.rs:147` consumes only ONE.
    expect(two.split('\x0c')).toHaveLength(2);
    expect(two.includes('\x0c\x0c')).toBe(false);
    expect(two.endsWith('\x0c')).toBe(false);

    const trailing = writeCompassDat(
      compassDat({ surveys: [compassSurvey(), compassSurvey()], trailingSeparator: true }),
    );
    expect(trailing.endsWith('\x0c')).toBe(true);
    expect(splitCompassSurveys(trailing)).toHaveLength(2);

    // A leading form feed has nothing before it to consume it, so it lands in
    // the first cave name - a corruption a spec must be able to write.
    expect(writeCompassDat(compassDat({ leading: '\x0c' })).startsWith('\x0c')).toBe(true);
  });

  it('writes 9 or 11 data columns exactly as compassHasBacksights predicts', () => {
    const formats = [
      'DDDDLUDRLAD', // 11: no backsight slot at all
      'DMMDLRUDLADN', // 12, slot index 11 = N
      'DMMDLRUDLADB', // 12, slot index 11 = B
      FORMAT_13_NO_BACKSIGHT, // 13, slot index 11 = N
      FORMAT_13_BACKSIGHT, // 13, slot index 11 = B
      'DDDDLUDRLADadNF', // 15: five shot items, slot index 13 = N
      'DDDDLUDRLADadBF', // 15: five shot items, slot index 13 = B
    ];
    for (const format of formats) {
      const text = writeCompassDat(compassDat({ surveys: [compassSurvey({ format })] }));
      const rows = compassDataRows(splitCompassSurveys(text)[0]);
      const expected = compassHasBacksights(format) ? 11 : 9;
      expect(rows.map((row) => row.length)).toEqual([expected, expected]);
      expect(rows[0]!.slice(2)).toHaveLength(expected - 2); // two rows: asserted above
    }
  });

  it('reads the backsight slot at index 13 for a 15-char format, not index 11', () => {
    // `parse_survey_format` consumes 4 unit chars, 4 passage chars, then FIVE
    // shot items for a 15-char format before `parse_backsight`
    // (`survey_parameters.rs:176-200`). A "12th character" rule reads the
    // backsight format below as having none and truncates its whole shot list.
    expect('DDDDLUDRLADadBF'[11]).toBe('a');
    expect('DDDDLUDRLADadBF'[13]).toBe('B');
    expect(compassHasBacksights('DDDDLUDRLADadBF')).toBe(true);
    expect(compassHasBacksights('DDDDLUDRLADadNF')).toBe(false);
    // Any other length is not a legal format at all (`ErrorKind::LengthValue`).
    expect(compassHasBacksights('DDDDLUDRLA')).toBe(false);
    expect(compassHasBacksights('DDDDLUDRLADadB')).toBe(false);
    expect(compassHasBacksights(null)).toBe(false);
    expect(compassHasBacksights(undefined)).toBe(false);
  });

  it('falls back to 0.00 rather than emitting a short backsight row', () => {
    // A declared-but-absent backsight pair fails `parse_double`, `many0` stops,
    // and every remaining row in the survey vanishes with no warning.
    const text = writeCompassDat(
      compassDat({
        surveys: [
          compassSurvey({
            shots: [
              compassShot({ backBearing: undefined, backInclination: undefined }),
              compassShot({ from: 'A2', to: 'A3' }),
            ],
          }),
        ],
      }),
    );
    const rows = compassDataRows(splitCompassSurveys(text)[0]);
    expect(rows.map((row) => row.length)).toEqual([11, 11]);
    expect(rows[0]!.slice(9)).toEqual(['0.00', '0.00']); // two rows: asserted above
    // And a non-backsight format never emits them, even when the shot carries them.
    const noBacksight = writeCompassDat(
      compassDat({ surveys: [compassSurvey({ format: FORMAT_13_NO_BACKSIGHT })] }),
    );
    expect(compassDataRows(splitCompassSurveys(noBacksight)[0])[0]).toHaveLength(9);
  });

  it('emits FORMAT with its trailing space, and formatNoSpace removes exactly that space', () => {
    const withSpace = writeCompassDat(compassDat());
    // `tag("FORMAT: ")` at `survey_parameters.rs:110` requires the space.
    expect(withSpace).toContain(`  FORMAT: ${FORMAT_13_BACKSIGHT}`);

    const withoutSpace = writeCompassDat(compassDat({ surveys: [compassSurvey({ formatNoSpace: true })] }));
    expect(withoutSpace).toContain(`  FORMAT:${FORMAT_13_BACKSIGHT}`);
    expect(withoutSpace).not.toContain('FORMAT: ');
    // Exactly one character shorter, and it is the space after the colon.
    expect(withoutSpace.length).toBe(withSpace.length - 1);
    expect(withoutSpace.replace('FORMAT:', 'FORMAT: ')).toBe(withSpace);
    // The corruption is silent: the survey still parses, as feet + degrees +
    // no backsights + no corrections, so the writer still emits 11 columns.
    expect(compassDataRows(splitCompassSurveys(withoutSpace)[0])[0]).toHaveLength(11);
  });

  it('terminates every line including the last, and omitFinalNewline drops exactly one', () => {
    for (const eol of ['\r\n', '\n']) {
      const text = writeCompassDat(compassDat({ eol }));
      expect(text.endsWith(eol)).toBe(true);
      const chunks = text.split(eol);
      expect(chunks[chunks.length - 1]).toBe('');
      // No chunk may hide another terminator: that would mean a line the parser
      // never sees as a line.
      for (const chunk of chunks.slice(0, -1)) {
        expect(chunk.includes('\n')).toBe(false);
        expect(chunk.includes('\r')).toBe(false);
      }

      const truncated = writeCompassDat(compassDat({ eol, surveys: [compassSurvey({ omitFinalNewline: true })] }));
      expect(`${truncated}${eol}`).toBe(text);
      // `recognize_line` needs the terminator, so the last shot is simply lost.
      expect(compassDataRows(splitCompassSurveys(truncated)[0])).toHaveLength(1);
      expect(compassDataRows(splitCompassSurveys(text)[0])).toHaveLength(2);
    }
  });

  it('keeps station names inside the character class take_while1 accepts', () => {
    // `is_valid_station_name_char` (`parser_utils.rs:19-21`) is exactly
    // alphanumeric plus `_ - ' *`. A `.` or `,` truncates the shot list.
    const legal = /^[A-Za-z0-9_'*-]+$/;
    const text = writeCompassDat(
      compassDat({
        surveys: [
          compassSurvey({
            shots: [
              compassShot({ from: 'A1', to: "A2'" }),
              compassShot({ from: 'B-1', to: 'B_2' }),
              compassShot({ from: '*8', to: '9' }),
            ],
          }),
        ],
      }),
    );
    const rows = compassDataRows(splitCompassSurveys(text)[0]);
    expect(rows).toHaveLength(3);
    for (const row of rows) {
      expect(row[0]).toMatch(legal);
      expect(row[1]).toMatch(legal);
    }
  });

  it('passes rawLine through verbatim and ignores every typed field beside it', () => {
    const text = writeCompassDat(
      compassDat({
        surveys: [compassSurvey({ shots: [compassShot({ from: 'IGNORED', rawLine: 'A.1 A2 1 2 3' })] })],
      }),
    );
    expect(text).toContain('A.1 A2 1 2 3');
    expect(text).not.toContain('IGNORED');
  });

  it('emits flags and comments in the order split_flags_and_comment round-trips', () => {
    const text = writeCompassDat(
      compassDat({
        surveys: [compassSurvey({ shots: [compassShot({ flags: 'PX', comment: 'side lead' })] })],
      }),
    );
    expect(text).toContain('#|PX# side lead');
  });

  it('carries impossible values verbatim, because that is the corruption catalog', () => {
    const text = writeCompassDat(
      compassDat({
        surveys: [
          compassSurvey({
            month: 2,
            day: 29,
            year: '99999999999',
            declination: 'nan',
            shots: [compassShot({ length: '-inf', bearing: '1e999', left: 'nan' })],
          }),
        ],
      }),
    );
    expect(text).toContain('SURVEY DATE: 2 29 99999999999');
    expect(text).toContain('DECLINATION: nan');
    expect(text).toContain('-inf 1e999');
  });

  it('replaces the whole document when rawText is set', () => {
    expect(writeCompassDat(compassDat({ rawText: 'not a survey at all' }))).toBe('not a survey at all');
  });
});

// ---------------------------------------------------------------------------
// Compass .MAK
// ---------------------------------------------------------------------------

describe('writeCompassMak framing', () => {
  it('emits only directives the lexer dispatches on, each ;-terminated', () => {
    const directives = lexMak(writeCompassMak(compassMak()));
    expect(directives.length).toBeGreaterThan(0);
    for (const directive of directives) {
      expect(MAK_DISPATCH_CHARS).toContain(directive.char);
      expect(directive.text.endsWith(';')).toBe(true);
    }
    expect(directives.map((directive) => directive.char)).toEqual([
      '/', // project id
      '/', // comment
      '@',
      '&',
      '$',
      '%',
      '!',
      '#',
    ]);
  });

  it('puts @ and & before every #, and filesFirst inverts exactly that', () => {
    // `to_file_state` (`project/parser.rs:95-103`) needs both a datum and a base
    // location, so a `#` entry seen first is `Survey file encountered before
    // required state was set`.
    const chars = lexMak(writeCompassMak(compassMak())).map((directive) => directive.char);
    const firstFile = chars.indexOf('#');
    expect(firstFile).toBeGreaterThan(chars.indexOf('@'));
    expect(firstFile).toBeGreaterThan(chars.indexOf('&'));

    const inverted = lexMak(writeCompassMak(compassMak({ filesFirst: true }))).map((d) => d.char);
    expect(inverted.indexOf('#')).toBeLessThan(inverted.indexOf('@'));
    expect(inverted.indexOf('#')).toBeLessThan(inverted.indexOf('&'));
  });

  it('names a datum from the 23-entry case-sensitive table', () => {
    // Transcribed from `parse_datum_name` (`project/parser.rs:213-244`), which
    // is a plain exact match: `&wgs 1984;` fails the whole file.
    expect(COMPASS_DATUMS).toHaveLength(23);
    expect(new Set(COMPASS_DATUMS).size).toBe(23);
    const datumDirective = lexMak(writeCompassMak(compassMak())).find((d) => d.char === '&');
    expect(datumDirective).toBeDefined();
    expect(COMPASS_DATUMS).toContain(datumDirective?.text.slice(1, -1));
    for (const datum of COMPASS_DATUMS) {
      const text = writeCompassMak(compassMak({ datum }));
      expect(lexMak(text).find((d) => d.char === '&')?.text).toBe(`&${datum};`);
    }
  });

  it('keeps a station fix free of the whitespace lex_station_fix rejects', () => {
    // `lex_station_fix` (`project/lexer.rs:419-470`) allows whitespace after `[`
    // and around the unit char, and NOWHERE else - not before a comma, not
    // before `]`. One stray space is `Invalid station fix format`.
    const entry = lexMak(writeCompassMak(compassMak())).find((d) => d.char === '#');
    expect(entry?.text).toBe('#synth.dat,A1[m,500000.000,4000000.000,100.000];');
    expect(entry?.text).not.toMatch(/[\s]/);
  });

  it('brackets a folder entry and finds the ; at bracket depth zero', () => {
    const text = writeCompassMak(
      compassMak({
        files: [
          { path: 'a.dat' },
          {
            path: 'b.dat',
            folder: 'sub',
            stations: [{ name: 'B1', fix: { unit: 'f', east: 1, north: 2, elevation: 3 } }],
          },
        ],
      }),
    );
    const tail = lexMak(text)
      .map((directive) => directive.text)
      .slice(-4);
    expect(tail).toEqual(['#a.dat;', '[sub;', '#b.dat,B1[f,1,2,3];', '];']);
  });

  it('produces the .mak corruptions the catalog needs', () => {
    // A missing `;` is `MissingSemicolon`, which the mirror raises too.
    expect(() => lexMak(writeCompassMak(compassMak({ files: [{ path: 'a.dat', omitSemicolon: true }] })))).toThrow(
      /Missing semicolon/,
    );
    // A leading character outside the dispatch set is `UnexpectedChar`.
    expect(() => lexMak(writeCompassMak(compassMak({ leading: '^\r\n' })))).toThrow(/Unexpected character/);
    // Exponents and `nan` are rejected by the lexer's own `parse_double`
    // (`project/lexer.rs:247-272`) - the writer must still be able to emit them.
    expect(writeCompassMak(compassMak({ baseLocation: { east: '1e2', north: 2, elevation: 3, zone: 300, convergence: 'nan' } }))).toContain(
      '@1e2,2,3,300,nan;',
    );
    // `*` is the convergence-disabled spelling.
    expect(writeCompassMak(compassMak({ fileConvergence: { value: '-0.26', enabled: false } }))).toContain('*-0.26;');
    // null omits a clause entirely.
    const bare = writeCompassMak(
      compassMak({ projectUuid: null, comments: [], baseLocation: null, datum: null, utmZone: null, fileConvergence: null, flags: null }),
    );
    expect(lexMak(bare).map((directive) => directive.char)).toEqual(['#']);
  });

  it('replaces the whole document when rawText is set', () => {
    expect(writeCompassMak(compassMak({ rawText: '^^^' }))).toBe('^^^');
  });
});

// ---------------------------------------------------------------------------
// Walls .SRV
// ---------------------------------------------------------------------------

describe('writeWallsSrv framing', () => {
  it('classifies every emitted line the way parse_srv_contents does', () => {
    const spec = wallsSrv();
    const kinds = classifyWallsSrv(writeWallsSrv(spec));
    const count = (kind: WallsLineKind): number => kinds.filter((k) => k === kind).length;

    expect(count('comment')).toBe(1); // the `;`-prefixed header line
    expect(count('directive')).toBe(3); // #Date, #Units, #Fix
    expect(count('blockOpen')).toBe(1);
    expect(count('blockClose')).toBe(1);
    expect(count('inBlock')).toBe(1);
    // Every shot, splays included, is a vector line.
    expect(count('vector')).toBe(spec.shots.length);
    expect(count('blank')).toBe(0);
    // The block comment must precede the shots or an unterminated `#[` could
    // never swallow them.
    expect(kinds.indexOf('blockOpen')).toBeLessThan(kinds.indexOf('vector'));
  });

  it('lets an unterminated #[ swallow every following line', () => {
    const spec = wallsSrv({ unterminatedBlockComment: true, blockComment: [] });
    const kinds = classifyWallsSrv(writeWallsSrv(spec));
    expect(kinds).toContain('blockOpen');
    expect(kinds).not.toContain('blockClose');
    expect(kinds).not.toContain('vector');
    expect(kinds.filter((kind) => kind === 'inBlock')).toHaveLength(spec.shots.length);
  });

  it('counts - and -- as splays but leaves --- a real station', () => {
    const spec = wallsSrv();
    const text = writeWallsSrv(spec);
    const vectors = text
      .split('\r\n')
      .filter((_, index) => classifyWallsSrv(text)[index] === 'vector')
      .map((line) => parseWallsVector(line.trim()));

    expect(vectors).toHaveLength(spec.shots.length);
    const anonymous = vectors.map((vector) => vector !== null && isAnonymousStation(vector.to));
    expect(anonymous).toEqual([false, false, true, true, false]);
    // `---` is stored as a real station name (`walls/units.rs:441-443` follows
    // McKenzie's source rather than dewalls' `-+` regex).
    expect(vectors[4]?.to).toBe('---');
  });

  it('closes an LRUD span before whitespace or at end of line', () => {
    const withComment = writeWallsSrv(
      wallsSrv({
        header: [],
        date: null,
        units: null,
        fixes: [],
        blockComment: [],
        shots: [
          { from: 'A0', to: 'A1', columns: ['10.00', '90.00', '0.00'], lrud: [1, 2, 3, 4], comment: 'note' },
          { from: 'A1', to: 'A2', columns: ['9.00', '91.00', '0.00'], lrud: [1, 2, 3, 4] },
          { from: 'A2', to: 'A3', columns: ['8.00', '92.00', '0.00'], lrud: [1, 2, 3, 4], lrudStar: true },
        ],
      }),
    );
    const lines = withComment.split('\r\n').filter((line) => line !== '');
    for (const line of lines) {
      const vector = parseWallsVector(line);
      expect(vector).not.toBeNull();
      // Three measurement columns survive: the span was recognised, not glued
      // into a token.
      expect(vector?.columns).toHaveLength(3);
      expect(vector?.lrud).toBe('1,2,3,4');
    }
    expect(at(lines, 0).endsWith(' ;note')).toBe(true);
    expect(lines[2]).toContain(' *1,2,3,4*');

    // The counter-case the rule exists for: a closer glued to more text is a
    // literal name character, so the "LRUD" is swallowed into a token.
    const glued = parseWallsVector('A0 A1 10.00 90.00 0.00 <1,2,3,4>x');
    expect(glued?.lrud).toBeNull();
    expect(glued?.columns).toEqual(['10.00', '90.00', '0.00', '<1,2,3,4>x']);
  });

  it('emits a #Fix with three coordinate columns and an optional ; comment', () => {
    const text = writeWallsSrv(wallsSrv({ fixes: [{ station: 'A0', coords: [467427, 2258942, 220], comment: 'datum tie' }] }));
    expect(text).toContain('#Fix A0 467427 2258942 220 ;datum tie');
  });

  it('emits an unknown directive verbatim, which the parser silently ignores', () => {
    const text = writeWallsSrv(wallsSrv({ directives: ['#Segment /main', '#Frobnicate x'] }));
    expect(text).toContain('#Segment /main');
    expect(text).toContain('#Frobnicate x');
    const kinds = classifyWallsSrv(text);
    expect(kinds.filter((kind) => kind === 'directive')).toHaveLength(5);
  });

  it('passes rawLine and rawText through verbatim', () => {
    const text = writeWallsSrv(wallsSrv({ shots: [{ from: 'IGNORED', to: 'X', columns: [], rawLine: 'A0 A1 $(undefinedMacro)' }] }));
    expect(text).toContain('A0 A1 $(undefinedMacro)');
    expect(text).not.toContain('IGNORED');
    expect(writeWallsSrv(wallsSrv({ rawText: '#[' }))).toBe('#[');
  });
});

// ---------------------------------------------------------------------------
// Walls .WPJ
// ---------------------------------------------------------------------------

describe('writeWallsWpj framing', () => {
  it('opens each entry before its properties and closes every book', () => {
    const directives = wpjDirectives(writeWallsWpj(wallsWpj()));
    expect(directives.map((directive) => directive.name)).toEqual([
      '.BOOK',
      '.NAME',
      '.STATUS',
      '.REF',
      '.SURVEY',
      '.NAME',
      '.STATUS',
      '.ENDBOOK',
    ]);
    // Every property must follow a `.BOOK`/`.SURVEY` and precede the next
    // container directive - `parse_wpj` attaches them to `pending`.
    const containers = new Set(['.BOOK', '.SURVEY', '.ENDBOOK']);
    let pending = false;
    for (const directive of directives) {
      if (containers.has(directive.name)) {
        pending = directive.name !== '.ENDBOOK';
        continue;
      }
      expect(pending).toBe(true);
    }
    const opens = directives.filter((directive) => directive.name === '.BOOK').length;
    expect(directives.filter((directive) => directive.name === '.ENDBOOK')).toHaveLength(opens);
  });

  it('nests books and closes them innermost first', () => {
    const text = writeWallsWpj(
      wallsWpj({
        root: {
          kind: 'book',
          title: 'Outer',
          children: [
            { kind: 'book', title: 'Inner', children: [{ kind: 'survey', title: 'Leaf', name: 'LEAF' }] },
            { kind: 'survey', title: 'Sibling', name: 'SIB' },
          ],
        },
      }),
    );
    expect(wpjDirectives(text).map((directive) => `${directive.name} ${directive.rest}`.trim())).toEqual([
      '.BOOK Outer',
      '.BOOK Inner',
      '.SURVEY Leaf',
      '.NAME LEAF',
      '.ENDBOOK',
      '.SURVEY Sibling',
      '.NAME SIB',
      '.ENDBOOK',
    ]);
  });

  it('writes a .NAME free of the separators resolve_survey rejects', () => {
    // `walls/wpj.rs:362-364` treats `/`, `\` or `:` in a `.NAME` as a traversal
    // attempt and reports the survey missing rather than reading it.
    for (const directive of wpjDirectives(writeWallsWpj(wallsWpj()))) {
      if (directive.name !== '.NAME') continue;
      expect(directive.rest).not.toMatch(/[/\\:]/);
      expect(directive.rest).not.toBe('');
    }
    // A spec can still write one deliberately.
    const traversal = writeWallsWpj(wallsWpj({ root: { kind: 'survey', title: 'Bad', name: '../../etc/passwd' } }));
    expect(traversal).toContain('.NAME ../../etc/passwd');
  });

  it('writes a .REF with >= 12 finite numeric fields and a quoted datum', () => {
    const ref = wpjDirectives(writeWallsWpj(wallsWpj())).find((directive) => directive.name === '.REF');
    expect(ref).toBeDefined();
    const parsed = parseWpjRef(ref?.rest ?? '');
    expect(parsed.fields.length).toBeGreaterThanOrEqual(12);
    expect(parsed.fields.slice(0, 12).every((value) => Number.isFinite(value))).toBe(true);
    expect(parsed.datum).toBe('NAD83');
    // Zone integral in 1..=60 and flags integral in 0..=65535 (`wpj.rs:441-449`).
    // fields.length >= 12 is asserted above.
    expect(Number.isInteger(parsed.fields[2])).toBe(true);
    expect(Math.abs(parsed.fields[2]!)).toBeGreaterThanOrEqual(1);
    expect(Math.abs(parsed.fields[2]!)).toBeLessThanOrEqual(60);
    expect(parsed.fields[5]).toBeGreaterThanOrEqual(0);
    expect(parsed.fields[5]).toBeLessThanOrEqual(65535);
  });

  it('omits a property when it is null and replaces the document with rawText', () => {
    const bare = writeWallsWpj(wallsWpj({ header: [], root: { kind: 'book', title: 'Bare', name: null, status: null, ref: null } }));
    expect(wpjDirectives(bare).map((directive) => directive.name)).toEqual(['.BOOK', '.ENDBOOK']);
    expect(writeWallsWpj(wallsWpj({ rawText: '.NOPE' }))).toBe('.NOPE');
  });
});

// ---------------------------------------------------------------------------
// GPX
// ---------------------------------------------------------------------------

describe('writeGpxXml framing', () => {
  it('emits one element per spec point, each carrying lat and lon', () => {
    const spec = gpx();
    const elements = scanXmlElements(writeGpxXml(spec));
    const tagged = (tag: string): ScannedElement[] => elements.filter((element) => element.tag === tag);

    expect(tagged('gpx')).toHaveLength(1);
    expect(tagged('wpt')).toHaveLength(1);
    expect(tagged('trkseg')).toHaveLength(1);
    expect(tagged('trkpt')).toHaveLength(3);
    expect(tagged('rtept')).toHaveLength(2);
    // A single out-of-range or missing coordinate aborts the WHOLE file
    // (`gpx.rs:222-259`), so a nominal spec may never omit one.
    for (const element of [...tagged('wpt'), ...tagged('trkpt'), ...tagged('rtept')]) {
      expect(hasAttribute(element.attrs, 'lat')).toBe(true);
      expect(hasAttribute(element.attrs, 'lon')).toBe(true);
    }
  });

  it('omits an attribute the spec omits, and writes a hostile value verbatim', () => {
    const elements = scanXmlElements(
      writeGpxXml(gpx({ waypoints: [{ lat: 'nan' }, { lon: '1e999' }, { lat: 91, lon: -181 }], tracks: [], routes: [] })),
    );
    const waypoints = elements.filter((element) => element.tag === 'wpt');
    expect(hasAttribute(at(waypoints, 0).attrs, 'lat')).toBe(true);
    expect(hasAttribute(at(waypoints, 0).attrs, 'lon')).toBe(false);
    expect(hasAttribute(at(waypoints, 1).attrs, 'lat')).toBe(false);
    expect(at(waypoints, 0).attrs).toContain('lat="nan"');
    expect(at(waypoints, 1).attrs).toContain('lon="1e999"');
    expect(at(waypoints, 2).attrs).toContain('lat="91" lon="-181"');
  });

  it('escapes text content so a payload arrives as data, not as markup', () => {
    const hostile = ']]><![CDATA[ &amp;';
    const xml = writeGpxXml(
      gpx({ metadataName: hostile, waypoints: [{ lat: 1, lon: 2, name: hostile, cmt: hostile }], tracks: [], routes: [] }),
    );
    const escaped = ']]&gt;&lt;![CDATA[ &amp;amp;';
    expect(xml).toContain(`<name>${escaped}</name>`);
    expect(xml).toContain(`<cmt>${escaped}</cmt>`);
    // The payload must not have opened a second metadata element.
    expect(xml.match(/<metadata>/g)).toHaveLength(1);
    expect(scanXmlElements(xml).map((element) => element.tag)).toEqual([
      'gpx',
      'metadata',
      'name',
      'wpt',
      'name',
      'cmt',
    ]);
  });

  it('self-closes a point when the spec asks, with no children', () => {
    const xml = writeGpxXml(
      gpx({ waypoints: [{ lat: 1, lon: 2, name: 'dropped', selfClosing: true }], tracks: [], routes: [] }),
    );
    expect(xml).toContain('<wpt lat="1" lon="2" />');
    expect(xml).not.toContain('dropped');
    const waypoint = scanXmlElements(xml).find((element) => element.tag === 'wpt');
    expect(waypoint?.selfClosing).toBe(true);
    // A self-closing `<trk/>` never produces a Start event, so the track is
    // ignored entirely (`gpx.rs:154-216`).
    const track = writeGpxXml(gpx({ waypoints: [], routes: [], tracks: [{ segments: [], selfClosing: true }] }));
    expect(track).toContain('<trk />');
  });

  it('wraps ele in CDATA when asked, which the reader silently drops', () => {
    // `read_text` (`gpx.rs:262-281`) never matches `Event::CData`, so the value
    // becomes None and the point reads as depth 0.
    const xml = writeGpxXml(gpx({ waypoints: [{ lat: 1, lon: 2, ele: '350.2', eleCdata: true }], tracks: [], routes: [] }));
    expect(xml).toContain('<ele><![CDATA[350.2]]></ele>');
  });

  it('produces the document-level corruptions the catalog needs', () => {
    expect(writeGpxXml(gpx({ declaration: null })).startsWith('<gpx ')).toBe(true);
    expect(writeGpxXml(gpx()).startsWith('<?xml ')).toBe(true);
    // A namespace prefix is stripped at the `:` by `strip_ns`, so `g:gpx` is
    // still the root element.
    const prefixed = writeGpxXml(gpx({ rootTag: 'g:gpx', rootAttrs: 'xmlns:g="http://www.topografix.com/GPX/1/1"' }));
    expect(prefixed).toContain('<g:gpx xmlns:g=');
    expect(prefixed).toContain('</g:gpx>');
    expect(at(scanXmlElements(prefixed), 0).tag).toBe('gpx');
    // No attributes at all is legal: neither `xmlns` nor `version` is read.
    expect(writeGpxXml(gpx({ rootAttrs: '' }))).toContain('<gpx>');
    expect(writeGpxXml(gpx({ omitClosingRoot: true }))).not.toContain('</gpx>');
    expect(writeGpxXml(gpx({ trailing: '<<garbage' })).endsWith('<<garbage')).toBe(true);
    // A one-point segment imports nothing but still burns a section index.
    const thin = writeGpxXml(gpx({ waypoints: [], routes: [], tracks: [{ segments: [[{ lat: 1, lon: 2 }], []] }] }));
    expect(scanXmlElements(thin).filter((element) => element.tag === 'trkseg')).toHaveLength(2);
    expect(scanXmlElements(thin).filter((element) => element.tag === 'trkpt')).toHaveLength(1);
    expect(writeGpxXml(gpx({ rawText: 'not xml' }))).toBe('not xml');
  });
});

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

describe('determinism', () => {
  it('renders each format identically twice, which is what a trace relies on', () => {
    const dat = compassDat({ surveys: [compassSurvey(), compassSurvey({ name: 'B', team: '' })] });
    expect(writeCompassDat(dat)).toBe(writeCompassDat(structuredClone(dat) as CompassDatSpec));

    const mak = compassMak({ files: [{ path: 'a.dat', folder: 'sub' }, { path: 'b.dat' }] });
    expect(writeCompassMak(mak)).toBe(writeCompassMak(structuredClone(mak) as CompassMakSpec));

    const srv = wallsSrv({ shots: [...wallsSrv().shots, { from: 'A3', to: 'A4', columns: ['x'.repeat(300)] }] });
    expect(writeWallsSrv(srv)).toBe(writeWallsSrv(structuredClone(srv) as WallsSrvSpec));

    const wpj = wallsWpj();
    expect(writeWallsWpj(wpj)).toBe(writeWallsWpj(structuredClone(wpj) as WallsWpjSpec));

    const gpxSpec = gpx({ metadataName: 'z'.repeat(300) });
    expect(writeGpxXml(gpxSpec)).toBe(writeGpxXml(structuredClone(gpxSpec) as GpxSpec));
  });

  it('treats an omitted field and an explicit null identically, so a JSON trace round-trips', () => {
    // `JSON.stringify` drops `undefined` properties and keeps `null`; if the two
    // meant different things, a spec would not survive being written to a trace.
    expect(writeCompassMak(compassMak({ datum: undefined }))).toBe(writeCompassMak(compassMak({ datum: null })));
    expect(writeWallsSrv(wallsSrv({ date: undefined }))).toBe(writeWallsSrv(wallsSrv({ date: null })));
    expect(writeGpxXml(gpx({ metadataName: undefined }))).toBe(writeGpxXml(gpx({ metadataName: null })));
  });
});
