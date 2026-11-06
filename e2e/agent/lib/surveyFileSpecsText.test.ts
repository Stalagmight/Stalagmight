/**
 * What a VALID spec has to be, for the five formats whose parsers say nothing
 * when it is not.
 *
 * `surveyFileSpecs.test.ts` covers the wave-1 generators and asks one big
 * question of them: does a `valid` DMP spec yield exactly as many sections as it
 * declares. These do the same job for Compass, Walls and GPX, and they need more
 * cases because there are more ways to lose data in silence. A Compass shot row
 * one column short truncates the rest of the survey; a Walls `#Units Order=` out
 * of step with the columns written reads a bearing as a distance; a GPX
 * coordinate one step past the range aborts the ENTIRE file. None of those is an
 * error the app can show, so none of them would show up as a red test - they
 * would show up as a fuzz run that passes green having imported nothing.
 *
 * The checks below are TS MIRRORS of the parsers' framing rules, transcribed
 * from the same sources `surveyFileWritersText.test.ts` mirrors (kept local
 * rather than imported from that file, because importing a module that calls
 * `describe` at the top level would re-register its suites here). They are
 * deliberately the minimum each assertion needs.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

import {
  type GeneratedFileSpec,
  type Rng,
  type SurveyFileFormat,
  FORMATS_IN_FAMILY,
  SURVEY_FORMAT_FAMILIES,
  familyOf,
  materializeSurveyFile,
  randomCompassDatSpec,
  randomCompassProjectSpec,
  randomGpxSpec,
  randomSurveyFileSpec,
  randomWallsProjectSpec,
  randomWallsSrvSpec,
  renderSurveyFiles,
  selfCheckClaim,
} from './surveyFileSpecs';
import {
  type CompassDatSpec,
  type WallsSrvSpec,
  compassHasBacksights,
  writeCompassDat,
  writeCompassMak,
  writeGpxXml,
  writeWallsSrv,
  writeWallsWpj,
} from './surveyFileWritersText';
import { seedWindow } from './seedWindow';

/** `scenarios/framework.ts`'s `mulberry32`, copied for the same reason it is in `surveyFileSpecs.test.ts`. */
function mulberry32(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** 1..220, or the window the STALAGMIGHT_VITEST_SEED_* knobs ask for (seedWindow.ts). */
const SEEDS = seedWindow(1, 220);

// ---------------------------------------------------------------------------
// Mirrors
// ---------------------------------------------------------------------------

/** `many0(parse_survey)` framing: `\x0c` is the only separator, and one is eaten. */
function splitCompassSurveys(text: string): string[] {
  const blocks = text.split('\x0c');
  if (blocks.length > 1 && blocks[blocks.length - 1] === '') blocks.pop();
  return blocks;
}

/** `recognize_line` = `not_line_ending` + `line_ending` (`parser_utils.rs:38-42`). */
function compassLines(block: string): string[] {
  const lines: string[] = [];
  let rest = block;
  while (rest.length > 0) {
    const at = rest.indexOf('\n');
    if (at === -1) return lines;
    const line = rest.slice(0, at);
    const cr = line.indexOf('\r');
    if (cr !== -1 && cr !== line.length - 1) throw new Error('bare CR: not_line_ending fails');
    lines.push(line.endsWith('\r') ? line.slice(0, -1) : line);
    rest = rest.slice(at + 1);
  }
  return lines;
}

/** Rows after the `FROM` column header, whitespace-split (`survey/parser/mod.rs:60-64`). */
function compassDataRows(block: string): string[][] {
  const lines = compassLines(block);
  const headerAt = lines.findIndex((line) => line.trimStart().startsWith('FROM'));
  if (headerAt === -1) return [];
  return lines
    .slice(headerAt + 1)
    .filter((line) => line.trim() !== '')
    .map((line) => line.trim().split(/\s+/));
}

/** The only characters the `.mak` lexer dispatches on (`project/lexer.rs:179-197`). */
const MAK_DISPATCH_CHARS = ['/', '@', '&', '$', '%', '*', '!', '#', '[', ']'];

/** `tokenize`: skip whitespace, dispatch on the first character, run to the depth-0 `;`. */
function lexMak(text: string): { char: string; text: string }[] {
  const directives: { char: string; text: string }[] = [];
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

type WallsLineKind = 'blank' | 'blockOpen' | 'blockClose' | 'inBlock' | 'comment' | 'directive' | 'vector';

/** `parse_srv_contents`' line loop, in its exact precedence (`walls/srv.rs:48-118`). */
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

/** `find_span_end` (`walls/srv.rs:461-464`): the closer counts only at EOL or before whitespace. */
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
      flush();
      break;
    } else if ((c === '<' || c === '*') && current === '') {
      const end = findSpanEnd(chars, i + 1, c === '<' ? '>' : '*');
      if (end !== -1) {
        lrud = chars.slice(i + 1, end).join('');
        i = end + 1;
        continue;
      }
      current += c;
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
  if (tokens.length < 2) return null;
  return { from: tokens[0]!, to: tokens[1]!, columns: tokens.slice(2), lrud };
}

/** `is_anonymous` (`walls/units.rs:441-443`): exactly `-` or `--`, never `---`. */
function isAnonymousStation(token: string): boolean {
  return token === '-' || token === '--';
}

/** `parse_wpj`'s loop (`walls/wpj.rs:212-219`): trim, skip blanks and `;`, uppercase token 0. */
function wpjDirectives(text: string): { name: string; rest: string }[] {
  const out: { name: string; rest: string }[] = [];
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
    fields: arg.slice(0, start).trim().split(/\s+/).map(Number),
    datum: arg.slice(start + 1, end),
  };
}

interface ScannedElement {
  tag: string;
  attrs: string;
  selfClosing: boolean;
}

/** Enough of an XML scanner to check framing; `strip_ns` at `gpx.rs:105-116`. */
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

function attributeValue(attrs: string, name: string): string | null {
  const match = new RegExp(`\\s${name}="([^"]*)"`).exec(attrs);
  return match === null ? null : match[1]!; // group 1 is non-optional
}

// ---------------------------------------------------------------------------
// Compass .DAT
// ---------------------------------------------------------------------------

/** `is_valid_station_name_char` (`parser_utils.rs:19-21`). */
const COMPASS_STATION = /^[A-Za-z0-9_'*-]+$/;

function datRowsOf(spec: CompassDatSpec): { block: string; rows: string[][] }[] {
  return splitCompassSurveys(writeCompassDat(spec)).map((block) => ({
    block,
    rows: compassDataRows(block),
  }));
}

describe('randomCompassDatSpec (valid)', () => {
  it('renders exactly one block per survey, each with its keywords at column 0', () => {
    for (const seed of SEEDS) {
      const spec = randomCompassDatSpec(mulberry32(seed), 'valid');
      const blocks = splitCompassSurveys(writeCompassDat(spec));
      expect(blocks).toHaveLength(spec.surveys.length);
      for (const [index, block] of blocks.entries()) {
        const lines = compassLines(block);
        // Throws exactly where `lines[n].trim()` / `.startsWith()` would have on a
        // block too short to have line n.
        const lineAt = (n: number): string => {
          const line = lines[n];
          if (line === undefined) throw new Error(`survey block ${index} has no line ${n}`);
          return line;
        };
        // NON-BLANK cave name. The terminator is `opt(ws(tag("\x0c")))` and
        // `ws` runs `multispace0` AFTER the form feed, so an all-whitespace
        // cave-name line is eaten by it: the next survey's `SURVEY NAME:` is
        // then read as ITS cave name and `many0` stops with no error at all.
        // Measured - it turned a 3-survey `.dat` into a 1-survey one.
        expect(lineAt(0).trim()).not.toBe('');
        expect(lines[1]).toBe(`SURVEY NAME: ${spec.surveys[index]!.name}`); // one block per survey: asserted above
        expect(lineAt(2).startsWith('SURVEY DATE:')).toBe(true);
        expect(lines[3]).toBe('SURVEY TEAM:');
        // `multispace1` after the tag is greedy ACROSS newlines, so a blank team
        // line swallows the parameter line and kills the survey.
        expect(lineAt(4).trim()).not.toBe('');
        expect(lineAt(5).startsWith('DECLINATION:')).toBe(true);
        // `tag("FORMAT: ")` requires the trailing space, or the whole survey
        // silently reads as feet + degrees + no backsights + no corrections.
        expect(lines[5]).toContain('  FORMAT: ');
      }
    }
  });

  it('writes 9 or 11 measurement columns, exactly as compassHasBacksights predicts', () => {
    for (const seed of SEEDS) {
      const spec = randomCompassDatSpec(mulberry32(seed), 'valid');
      for (const [index, entry] of datRowsOf(spec).entries()) {
        const survey = spec.surveys[index];
        if (survey === undefined) throw new Error(`seed ${seed}: no survey ${index} for a rendered block`);
        expect(entry.rows).toHaveLength(survey.shots.length);
        const expected = compassHasBacksights(survey.format) ? 11 : 9;
        for (const row of entry.rows) {
          // `>=` and not `===`: a flag string and a comment become extra tail
          // tokens, which is legal (`split_flags_and_comment`). What must hold
          // is that the first `expected` tokens are the two stations and then
          // exactly the floats `parse_shot` demands - one short and `many0`
          // stops, dropping every remaining row in the survey.
          expect(row.length).toBeGreaterThanOrEqual(expected);
          for (const token of row.slice(2, expected)) {
            expect(Number.isFinite(Number(token))).toBe(true);
          }
        }
      }
    }
  });

  it('keeps every station name inside the class take_while1 accepts', () => {
    for (const seed of SEEDS) {
      const spec = randomCompassDatSpec(mulberry32(seed), 'valid');
      for (const entry of datRowsOf(spec)) {
        for (const row of entry.rows) {
          expect(row[0]).toMatch(COMPASS_STATION);
          expect(row[1]).toMatch(COMPASS_STATION);
        }
      }
    }
  });

  it('never carries an impossible calendar date, and never a FORMAT of an illegal length', () => {
    for (const seed of SEEDS) {
      const spec = randomCompassDatSpec(mulberry32(seed), 'valid');
      for (const survey of spec.surveys) {
        const month = Number(survey.month);
        const day = Number(survey.day);
        const year = Number(survey.year);
        // `NaiveDate::from_ymd_opt` rejects the survey outright since #479, so a
        // day that only exists in some months is not a valid spec.
        const date = new Date(Date.UTC(year, month - 1, day));
        expect(date.getUTCFullYear()).toBe(year);
        expect(date.getUTCMonth()).toBe(month - 1);
        expect(date.getUTCDate()).toBe(day);
        expect(day).toBeLessThanOrEqual(28);
        expect([11, 12, 13, 15]).toContain((survey.format ?? '').length);
        // `alpha1` stops at the first non-alphabetic character, so a format with
        // one in it is short and then an illegal length.
        expect(survey.format ?? '').toMatch(/^[A-Za-z]+$/);
        expect(survey.team).not.toBe('');
        // `corrections2` without `corrections` parses to NEITHER.
        if (survey.corrections2 !== undefined) expect(survey.corrections).toBeDefined();
      }
    }
  });

  it('reaches both a shared and a disjoint station namespace across surveys', () => {
    let shared = 0;
    let disjoint = 0;
    for (const seed of SEEDS) {
      const spec = randomCompassDatSpec(mulberry32(seed), 'valid');
      if (spec.surveys.length < 2) continue;
      const stationsOf = (index: number): Set<string> =>
        // Called with 0 and 1 only, after the length < 2 skip above.
        new Set(spec.surveys[index]!.shots.flatMap((shot) => [shot.from, shot.to]));
      const first = stationsOf(0);
      const overlap = [...stationsOf(1)].some((station) => first.has(station));
      if (overlap) shared += 1;
      else disjoint += 1;
    }
    // Disjoint namespaces are the ONLY way a Compass import lands as more than
    // one component, and that is the only way the orphan-placement dialog is
    // reachable for this format. Both must actually occur.
    expect(shared).toBeGreaterThan(0);
    expect(disjoint).toBeGreaterThan(0);
  });

  it('reaches the excluded flag and a comment, the two dialog checkboxes', () => {
    const shots = SEEDS.flatMap((seed) =>
      randomCompassDatSpec(mulberry32(seed), 'valid').surveys.flatMap((survey) => survey.shots),
    );
    expect(shots.some((shot) => shot.flags === 'X')).toBe(true);
    expect(shots.some((shot) => shot.comment !== undefined)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Compass .MAK
// ---------------------------------------------------------------------------

describe('randomCompassProjectSpec (valid)', () => {
  it('emits only lexable directives, each ;-terminated, with @ and & before every #', () => {
    for (const seed of SEEDS) {
      const { mak } = randomCompassProjectSpec(mulberry32(seed), 'valid');
      const directives = lexMak(writeCompassMak(mak));
      for (const directive of directives) {
        expect(MAK_DISPATCH_CHARS).toContain(directive.char);
        expect(directive.text.endsWith(';')).toBe(true);
      }
      const chars = directives.map((directive) => directive.char);
      // `to_file_state` needs a datum AND a base location before any `#`.
      expect(chars.indexOf('#')).toBeGreaterThan(chars.indexOf('@'));
      expect(chars.indexOf('#')).toBeGreaterThan(chars.indexOf('&'));
      expect(chars).toContain('#');
    }
  });

  it('names a sibling this spec also writes, with no folder to resolve through', () => {
    for (const seed of SEEDS) {
      const { mak, dats } = randomCompassProjectSpec(mulberry32(seed), 'valid');
      const written = new Set(dats.map((sibling) => sibling.name));
      expect(mak.files.length).toBe(dats.length);
      for (const file of mak.files) {
        // A missing `.DAT` is a HARD error, not a warning
        // (`Survey file not found`, `compass.rs:200-202`).
        expect(written.has(file.path)).toBe(true);
        expect(file.folder).toBeUndefined();
        expect(file.path).not.toMatch(/[\\/]/);
      }
    }
  });

  it('writes plain decimal numbers only, which is all the .mak lexer admits', () => {
    // `project/lexer.rs:247-272` is sign, digits, optional `.` fraction. No
    // exponent, no `nan`, no bare `.5` - far stricter than the `.dat` parser.
    const plain = /^[+-]?\d+(\.\d+)?$/;
    for (const seed of SEEDS) {
      const { mak } = randomCompassProjectSpec(mulberry32(seed), 'valid');
      const base = mak.baseLocation ?? null;
      expect(base).not.toBeNull();
      if (base !== null) {
        for (const value of [base.east, base.north, base.elevation, base.zone, base.convergence]) {
          expect(String(value)).toMatch(plain);
        }
        expect(Number(base.zone)).toBeGreaterThanOrEqual(1);
        expect(Number(base.zone)).toBeLessThanOrEqual(60);
      }
      for (const file of mak.files) {
        for (const station of file.stations ?? []) {
          // `[alphanumeric*'-_]+` (`project/lexer.rs:415-417`).
          expect(station.name).toMatch(/^[A-Za-z0-9*'_-]+$/);
          const fix = station.fix;
          if (fix === undefined) continue;
          expect(['f', 'F', 'm', 'M']).toContain(fix.unit);
          for (const value of [fix.east, fix.north, fix.elevation]) {
            expect(String(value)).toMatch(plain);
          }
        }
      }
    }
  });

  it('reaches a station fix, which is what makes has_georeference true', () => {
    const withFix = SEEDS.filter((seed) =>
      randomCompassProjectSpec(mulberry32(seed), 'valid').mak.files.some((file) =>
        (file.stations ?? []).some((station) => station.fix !== undefined),
      ),
    );
    expect(withFix.length).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// Walls .SRV
// ---------------------------------------------------------------------------

/**
 * The measurement columns `active_order()` will consume, read back out of the
 * `#Units` line the spec wrote.
 *
 * `Order=` alone never switches the vector type (`walls/units.rs:149-169`), so
 * an `E/N/U` order without the `rect` key would leave the parser reading the
 * DEFAULT `D,A,V`. Dive mode is inferred, not declared: CT, station-to-station
 * taping, and an order with D and A but no V (`walls/mod.rs:333-340`), and it
 * appends two depth columns after the order's own.
 */
function expectedWallsColumns(units: string): number {
  const order = /(?:^|\s)Order=(\S+)/i.exec(units)?.[1] ?? 'DAV';
  const tape = /(?:^|\s)Tape=(\S+)/i.exec(units)?.[1] ?? 'IT';
  const rect = /(?:^|\s)rect\b/i.test(units);
  const letters = order.toUpperCase();
  const dive =
    !rect &&
    tape.toUpperCase().startsWith('SS') &&
    letters.includes('D') &&
    letters.includes('A') &&
    !letters.includes('V');
  return order.length + (dive ? 2 : 0);
}

function wallsVectorLines(spec: WallsSrvSpec): string[] {
  const text = writeWallsSrv(spec);
  const kinds = classifyWallsSrv(text);
  const lines = text.split('\n');
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  return lines.filter((_line, index) => kinds[index] === 'vector').map((line) => line.trim());
}

describe('randomWallsSrvSpec (valid)', () => {
  it("gives every shot exactly as many columns as #Units Order= asks for", () => {
    for (const seed of SEEDS) {
      const spec = randomWallsSrvSpec(mulberry32(seed), 'valid');
      const expected = expectedWallsColumns(spec.units ?? '');
      const vectors = wallsVectorLines(spec).map(parseWallsVector);
      expect(vectors).toHaveLength(spec.shots.length);
      for (const vector of vectors) {
        // `tokens.len() < 2` is skipped SILENTLY, and a short column list just
        // breaks the loop - neither is an error the app can report.
        expect(vector).not.toBeNull();
        expect(vector?.columns).toHaveLength(expected);
      }
    }
  });

  it('closes every LRUD span and keeps splays to exactly - or --', () => {
    for (const seed of SEEDS) {
      const spec = randomWallsSrvSpec(mulberry32(seed), 'valid');
      const vectors = wallsVectorLines(spec).map(parseWallsVector);
      for (const [index, vector] of vectors.entries()) {
        const shot = spec.shots[index];
        if (shot === undefined) throw new Error(`seed ${seed}: no shot ${index} for a vector line`);
        const lrud = shot.lrud ?? null;
        // An unterminated span makes the delimiter a literal name character and
        // swallows the LRUD into a measurement token.
        if (lrud === null) expect(vector?.lrud).toBeNull();
        else expect(vector?.lrud).toBe(lrud.map(String).join(','));
        const to = shot.to;
        if (isAnonymousStation(to)) expect(vector?.to).toBe(to);
      }
    }
  });

  it('reaches a splay-only survey and a fixed-station-only one', () => {
    let splayOnly = 0;
    let fixedOnly = 0;
    let ordinary = 0;
    for (const seed of SEEDS) {
      const spec = randomWallsSrvSpec(mulberry32(seed), 'valid');
      const splays = spec.shots.filter((shot) => isAnonymousStation(shot.to)).length;
      if (spec.shots.length === 0 && (spec.fixes ?? []).length > 0) fixedOnly += 1;
      else if (spec.shots.length > 0 && splays === spec.shots.length) splayOnly += 1;
      else if (spec.shots.length > 0 && splays === 0) ordinary += 1;
    }
    // These are the two shapes that make the Walls dialog's confirm button gate
    // on "nothing to import": `is_importable_shot` counts neither a splay (with
    // splays off) nor a survey with no vector lines at all.
    expect(splayOnly).toBeGreaterThan(0);
    expect(fixedOnly).toBeGreaterThan(0);
    expect(ordinary).toBeGreaterThan(0);
  });

  it('reaches a geographic #Fix, a block comment and an unknown directive', () => {
    const specs = SEEDS.map((seed) => randomWallsSrvSpec(mulberry32(seed), 'valid'));
    // An `N`/`S`/`E`/`W`-prefixed column is what drives the fixed-station and
    // georeference branch (`walls/srv.rs:635-666`).
    expect(
      specs.some((spec) =>
        (spec.fixes ?? []).some((fix) => fix.coords.some((coord) => /^[NSEW]/.test(String(coord)))),
      ),
    ).toBe(true);
    expect(specs.some((spec) => (spec.blockComment ?? []).length > 0)).toBe(true);
    expect(specs.some((spec) => (spec.directives ?? []).length > 0)).toBe(true);
    // A terminated block comment must never swallow the shot list.
    for (const spec of specs) {
      const kinds = classifyWallsSrv(writeWallsSrv(spec));
      expect(kinds.filter((kind) => kind === 'blockOpen')).toHaveLength(
        kinds.filter((kind) => kind === 'blockClose').length,
      );
      expect(kinds).not.toContain('blank');
    }
  });
});

// ---------------------------------------------------------------------------
// Walls .WPJ
// ---------------------------------------------------------------------------

describe('randomWallsProjectSpec (valid)', () => {
  it('gives every resolvable .SURVEY a sibling, and never leaves them all unresolved', () => {
    let withMissing = 0;
    for (const seed of SEEDS) {
      const { wpj, srvs } = randomWallsProjectSpec(mulberry32(seed), 'valid');
      const written = new Set(srvs.map((sibling) => sibling.name.toLowerCase()));
      const names = wpjDirectives(writeWallsWpj(wpj))
        .filter((directive) => directive.name === '.NAME')
        .map((directive) => directive.rest);
      // The root book has a `.NAME` too; only the leaves resolve to a file.
      const leafNames = names.filter((name) => !name.startsWith('FUZZPROJ'));
      expect(leafNames.length).toBeGreaterThan(0);
      const resolved = leafNames.filter((name) => written.has(`${name.toLowerCase()}.srv`));
      expect(resolved.length).toBe(srvs.length);
      expect(resolved.length).toBeGreaterThan(0);
      if (resolved.length < leafNames.length) withMissing += 1;
      for (const name of names) {
        // `resolve_survey` rejects a `.NAME` carrying `/`, `\` or `:` as a
        // traversal attempt and reports the survey missing.
        expect(name).not.toMatch(/[/\\:]/);
        expect(name).not.toBe('');
      }
      // Every sibling must be reachable: a `.PATH` would resolve the children
      // into a directory `materializeSurveyFile` never writes to.
      expect(writeWallsWpj(wpj)).not.toContain('.PATH');
    }
    // `missing_file_count` and the unresolved-survey rendering are real states.
    expect(withMissing).toBeGreaterThan(0);
  });

  it('balances every .BOOK with an .ENDBOOK and keeps properties on their entry', () => {
    for (const seed of SEEDS) {
      const { wpj } = randomWallsProjectSpec(mulberry32(seed), 'valid');
      const directives = wpjDirectives(writeWallsWpj(wpj));
      const books = directives.filter((directive) => directive.name === '.BOOK').length;
      expect(directives.filter((directive) => directive.name === '.ENDBOOK')).toHaveLength(books);
      const containers = new Set(['.BOOK', '.SURVEY', '.ENDBOOK']);
      let pending = false;
      for (const directive of directives) {
        if (containers.has(directive.name)) {
          pending = directive.name !== '.ENDBOOK';
          continue;
        }
        // A property directive attaches to the most recent `.BOOK`/`.SURVEY`
        // (`pending` in `parse_wpj`); one after an `.ENDBOOK` is silently lost.
        expect(pending).toBe(true);
      }
    }
  });

  it('writes a .REF of >= 12 finite fields and a .STATUS that keeps the entry countable', () => {
    let refs = 0;
    let statuses = 0;
    for (const seed of SEEDS) {
      const { wpj } = randomWallsProjectSpec(mulberry32(seed), 'valid');
      for (const directive of wpjDirectives(writeWallsWpj(wpj))) {
        if (directive.name === '.REF') {
          refs += 1;
          const parsed = parseWpjRef(directive.rest);
          expect(parsed.fields.length).toBeGreaterThanOrEqual(12);
          expect(parsed.fields.every((value) => Number.isFinite(value))).toBe(true);
          // fields.length >= 12 is asserted above.
          expect(Number.isInteger(parsed.fields[2])).toBe(true);
          expect(Math.abs(parsed.fields[2]!)).toBeGreaterThanOrEqual(1);
          expect(Math.abs(parsed.fields[2]!)).toBeLessThanOrEqual(60);
          expect(Number.isInteger(parsed.fields[5])).toBe(true);
          expect(parsed.fields[5]).toBeGreaterThanOrEqual(0);
          expect(parsed.fields[5]).toBeLessThanOrEqual(65535);
          expect(parsed.datum).not.toBe('');
        }
        if (directive.name === '.STATUS') {
          statuses += 1;
          const status = Number(directive.rest);
          expect(Number.isInteger(status)).toBe(true);
          // Bit 1 (DETACHED) makes the entry vanish; bit 16 (TYPE_OTHER) turns
          // it into a skipped attachment. Either would make the resolved-survey
          // claim a lie.
          expect(status & 2).toBe(0);
          expect(status & 65536).toBe(0);
        }
      }
    }
    expect(refs).toBeGreaterThan(0);
    expect(statuses).toBeGreaterThan(0);
  });

  it('reaches both a shared and a disjoint station namespace across its .srv files', () => {
    let shared = 0;
    let disjoint = 0;
    for (const seed of SEEDS) {
      const { srvs } = randomWallsProjectSpec(mulberry32(seed), 'valid');
      if (srvs.length < 2) continue;
      const stationsOf = (index: number): Set<string> =>
        // Called with 0 and 1 only, after the length < 2 skip above.
        new Set(srvs[index]!.srv.shots.flatMap((shot) => [shot.from, shot.to]));
      const first = stationsOf(0);
      if ([...stationsOf(1)].some((station) => first.has(station))) shared += 1;
      else disjoint += 1;
    }
    expect(shared).toBeGreaterThan(0);
    expect(disjoint).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// GPX
// ---------------------------------------------------------------------------

describe('randomGpxSpec (valid)', () => {
  it('always has at least one non-empty collection, and every coordinate is in range', () => {
    for (const seed of SEEDS) {
      const spec = randomGpxSpec(mulberry32(seed), 'valid');
      const total =
        (spec.waypoints?.length ?? 0) + (spec.tracks?.length ?? 0) + (spec.routes?.length ?? 0);
      // With all three empty the selection dialog has nothing to offer at all.
      expect(total).toBeGreaterThan(0);

      const elements = scanXmlElements(writeGpxXml(spec));
      const points = elements.filter((element) => ['wpt', 'trkpt', 'rtept'].includes(element.tag));
      for (const point of points) {
        const lat = attributeValue(point.attrs, 'lat');
        const lon = attributeValue(point.attrs, 'lon');
        // Both are REQUIRED and a single failure aborts the WHOLE file - there
        // is no per-point skip (`gpx.rs:222-259`).
        expect(lat).not.toBeNull();
        expect(lon).not.toBeNull();
        expect(Number.isFinite(Number(lat))).toBe(true);
        expect(Number.isFinite(Number(lon))).toBe(true);
        expect(Number(lat)).toBeGreaterThanOrEqual(-90);
        expect(Number(lat)).toBeLessThanOrEqual(90);
        expect(Number(lon)).toBeGreaterThanOrEqual(-180);
        expect(Number(lon)).toBeLessThanOrEqual(180);
      }
      expect(elements.filter((element) => element.tag === 'gpx')).toHaveLength(1);
      expect(elements.some((element) => element.tag === 'gpx' && element.selfClosing)).toBe(false);
    }
  });

  it('reaches every combination of the three collections', () => {
    const seen = new Set<string>();
    for (const seed of SEEDS) {
      const spec = randomGpxSpec(mulberry32(seed), 'valid');
      seen.add(
        `${(spec.waypoints?.length ?? 0) > 0 ? 'w' : '-'}${(spec.tracks?.length ?? 0) > 0 ? 't' : '-'}${
          (spec.routes?.length ?? 0) > 0 ? 'r' : '-'
        }`,
      );
    }
    // "waypoints only" is the one that shows `gpx-import-confirm` instead of
    // `gpx-continue-button`, so it has to actually occur.
    for (const combination of ['w--', '-t-', '--r', 'wtr', 'wt-', 'w-r', '-tr']) {
      expect([...seen]).toContain(combination);
    }
  });

  it('reaches a one-point segment, a missing <ele> and a boundary coordinate', () => {
    const specs = SEEDS.map((seed) => randomGpxSpec(mulberry32(seed), 'valid'));
    const segments = specs.flatMap((spec) => (spec.tracks ?? []).flatMap((track) => track.segments));
    // Skipped at import while its `section_index` is STILL consumed, so every
    // later placement index shifts (`gpx.rs:920-923`).
    expect(segments.some((segment) => segment.length === 1)).toBe(true);
    expect(segments.some((segment) => segment.length >= 2)).toBe(true);
    const points = [
      ...specs.flatMap((spec) => spec.waypoints ?? []),
      ...segments.flat(),
      ...specs.flatMap((spec) => (spec.routes ?? []).flatMap((route) => route.points)),
    ];
    expect(points.some((point) => point.ele === undefined)).toBe(true);
    expect(points.some((point) => Number(point.ele) < 0)).toBe(true);
    expect(points.some((point) => Math.abs(Number(point.lat)) === 90)).toBe(true);
    expect(points.some((point) => Math.abs(Number(point.lon)) === 180)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Cross-cutting
// ---------------------------------------------------------------------------

const TEXT_FORMATS: readonly SurveyFileFormat[] = ['dat', 'mak', 'srv', 'wpj', 'gpx'];
const ALL_FORMATS: readonly SurveyFileFormat[] = ['dmp', 'tml', 'tmlu', ...TEXT_FORMATS];

/** Names and bytes of every file a spec becomes, as one comparable string. */
function renderDigest(spec: GeneratedFileSpec): string {
  return renderSurveyFiles(spec)
    .map((file) => `${file.name}:${[...file.bytes].join(',')}`)
    .join('|');
}

describe('format families', () => {
  it('partitions the eight formats, and familyOf agrees with the table', () => {
    const listed = SURVEY_FORMAT_FAMILIES.flatMap((family) => FORMATS_IN_FAMILY[family]);
    expect([...listed].sort()).toEqual([...ALL_FORMATS].sort());
    expect(new Set(listed).size).toBe(listed.length);
    for (const family of SURVEY_FORMAT_FAMILIES) {
      for (const format of FORMATS_IN_FAMILY[family]) expect(familyOf(format)).toBe(family);
    }
  });
});

describe('determinism', () => {
  it('reproduces the same spec, the same names and the same bytes from one seed', () => {
    for (const seed of SEEDS.slice(0, 40)) {
      for (const format of ALL_FORMATS) {
        for (const profile of ['valid', 'malformed'] as const) {
          const a = randomSurveyFileSpec(mulberry32(seed), profile, format);
          const b = randomSurveyFileSpec(mulberry32(seed), profile, format);
          expect(b).toEqual(a);
          expect(b.name).toBe(a.name);
          // A trace stores the SPEC, so a spec that renders two ways is a trace
          // that does not replay.
          expect(renderDigest(b)).toBe(renderDigest(a));
        }
      }
    }
  });

  it('gives a project the sibling files it names, all as plain basenames', () => {
    for (const seed of SEEDS.slice(0, 60)) {
      for (const format of ['mak', 'wpj'] as const) {
        const spec = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const files = renderSurveyFiles(spec);
        expect(files.length).toBeGreaterThan(1);
        expect(files[0].name).toBe(spec.name);
        for (const file of files) {
          expect(file.name).toBe(path.basename(file.name));
          expect(file.bytes.length).toBeGreaterThan(0);
        }
        expect(new Set(files.map((file) => file.name)).size).toBe(files.length);
      }
    }
  });
});

describe('selfCheckClaim', () => {
  it('brackets what each valid spec actually declares', () => {
    for (const seed of SEEDS.slice(0, 60)) {
      for (const format of ALL_FORMATS) {
        const spec = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const claim = selfCheckClaim(spec);
        expect(claim.command).not.toBe('');
        expect(claim.what).not.toBe('');
        expect(claim.min).toBeGreaterThanOrEqual(0);
        if (claim.max !== null) expect(claim.max).toBeGreaterThanOrEqual(claim.min);

        if (spec.format === 'dat') expect(claim.min).toBe(spec.dat.surveys.length);
        if (spec.format === 'mak') {
          expect(claim.min).toBe(
            spec.dats.reduce((total, sibling) => total + sibling.dat.surveys.length, 0),
          );
        }
        if (spec.format === 'srv') expect([claim.min, claim.max]).toEqual([1, 1]);
        if (spec.format === 'wpj') {
          // Exactly the leaves whose `.NAME` names a sibling this spec writes -
          // which, in the valid profile, is every sibling it writes.
          expect(claim.min).toBe(spec.srvs.length);
        }
        if (spec.format === 'gpx') {
          expect(claim.min).toBe(
            (spec.gpx.waypoints?.length ?? 0) +
              (spec.gpx.tracks?.length ?? 0) +
              (spec.gpx.routes?.length ?? 0),
          );
        }
        if (spec.format === 'dmp') expect(claim.min).toBe(spec.dmp.sections.length);
        // Topology reconstruction means a TML's section count is NOT the spec's
        // section-name count, so the only honest claim is "at least one" - and
        // only when the file has a leg at all. Shot 0 is the `START`, a station
        // rather than a leg, so a file with no `REAL` shot legitimately parses
        // to zero sections.
        if (spec.format === 'tml' || spec.format === 'tmlu') {
          const hasLeg = spec.tml.shots.some((shot) => shot.type === 'REAL');
          expect([claim.min, claim.max]).toEqual([hasLeg ? 1 : 0, null]);
        }
      }
    }
  });

  it('returns -1 rather than throwing on a shape it did not expect', () => {
    for (const format of ALL_FORMATS) {
      const claim = selfCheckClaim(randomSurveyFileSpec(mulberry32(9), 'valid', format));
      for (const garbage of [null, undefined, 7, 'text', [], {}, { sections: 'nope' }, { survey_count: 'x' }]) {
        expect(claim.read(garbage)).toBe(-1);
      }
    }
  });
});

describe('materializeSurveyFile', () => {
  it('writes every sibling a project names, next to the project', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fuzz-text-'));
    try {
      for (const format of ['mak', 'wpj'] as const) {
        const target = path.join(dir, format);
        const spec = randomSurveyFileSpec(mulberry32(4), 'valid', format);
        const written = materializeSurveyFile(target, spec);
        expect(written.path).toBe(path.join(target, spec.name));
        const files = renderSurveyFiles(spec);
        expect(fs.readdirSync(target).sort()).toEqual(files.map((file) => file.name).sort());
        for (const file of files) {
          expect(fs.readFileSync(path.join(target, file.name))).toEqual(Buffer.from(file.bytes));
        }
      }
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it('refuses a hostile SIBLING name before it writes anything', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fuzz-text-'));
    try {
      const spec = randomSurveyFileSpec(mulberry32(4), 'valid', 'mak');
      if (spec.format !== 'mak') throw new Error(`asked for mak, got ${spec.format}`);
      const hostile: GeneratedFileSpec = {
        ...spec,
        dats: spec.dats.map((sibling, index) =>
          index === 0 ? { ...sibling, name: '../escape.dat' } : sibling,
        ),
      };
      expect(() => materializeSurveyFile(dir, hostile)).toThrow(/plain basename/);
      // The check runs over the WHOLE set before the first write, so a bad
      // sibling cannot leave the primary on disk.
      expect(fs.readdirSync(dir)).toEqual([]);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
