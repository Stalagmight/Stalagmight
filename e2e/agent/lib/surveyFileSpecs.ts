/**
 * Seeded generators for the survey-file specs `surveyFileWriters.ts` renders,
 * plus the one function in the import-fuzz stack that touches disk.
 *
 * ## Draws, not values
 *
 * A generator takes the framework's `rng` (`() => number`, see `mulberry32` in
 * `scenarios/framework.ts`) and nothing else, so a seed reproduces a file set
 * exactly. The boundary-biasing convention is the one
 * `scenarios/undoRedoFuzzActions.ts` established - `P_BOUNDARY` of every value
 * takes an extreme instead of a nominal draw, because the bugs live at zero, at
 * the sentinel, and just past the end of the range.
 *
 * Those helpers (`chance`, `biasedFloat`, `biasedText`, `P_BOUNDARY`) are
 * module-private over there and take a `ScenarioContext` rather than an `rng`.
 * Importing them would drag the whole targeting-digest surface into a module
 * that has no app to target, so the shapes are MIRRORED here against a bare
 * `Rng`. `P_BOUNDARY` is deliberately the same number: a reader comparing the
 * two files should not have to wonder whether the difference is meaningful.
 *
 * ## The one invariant a 'valid' DMP spec must keep
 *
 * `split_into_sections` cuts the file wherever it sees the end-of-section
 * pattern - for v2/v3/v4 that is a `3` byte followed by 15/20/28 ZEROS. A shot
 * record full of zeros can therefore terminate a section in the middle of
 * itself. So the valid profile never draws `length` or `markerIndex` as exactly
 * zero, which bounds the longest zero run inside a record at 21 bytes (v4's
 * worst case) against a 28-byte pattern. `surveyFileSpecs.test.ts` checks the
 * consequence directly - a valid spec yields exactly as many sections as it
 * declares - over enough seeds that the argument is not taken on trust.
 *
 * ## What 'valid' has to mean for the five text formats
 *
 * Compass, Walls and GPX each fail SILENTLY on a framing mistake:
 * `many0(parse_survey)` stops at the first survey it cannot read and returns
 * what it has, Walls skips a vector line with fewer than two tokens without a
 * warning, and a Compass row with one column too few truncates the whole
 * remaining shot list. A generator that gets one of those wrong does not produce
 * a test failure - it produces a fuzz run that spends its entire budget on the
 * error dialog and passes green. So each generator below states, per field, the
 * parser rule that bounds it, and `selfCheckClaim` turns "this spec declares N
 * surveys" into a claim the scenario can push through the REAL parse command
 * before it starts fuzzing.
 *
 * ## Why some specs are more than one file
 *
 * A Compass `.mak` is a project that names sibling `.dat` files by relative
 * path, and a missing one is a hard error (`Survey file not found:`,
 * `parsers/compass.rs:200-202`). A Walls `.wpj` is a tree of `.SURVEY` entries
 * whose `.NAME` resolves to `<name>.SRV` next to the project
 * (`walls/wpj.rs:351-382`). Neither can be exercised by writing one file, so
 * `renderSurveyFiles` returns a LIST and `materializeSurveyFile` writes all of
 * it. Sibling names are content-addressed for the same reason the primary's is:
 * two different projects that both wanted `part-0.dat` in one directory would
 * silently read each other's data.
 */
import * as fs from 'fs';
import * as path from 'path';

import { crc32Text } from './crc32';
import {
  type DmpSectionSpec,
  type DmpShotSpec,
  type DmpSpec,
  type TmlShotSpec,
  type TmlSpec,
  writeDmpBytes,
  writeTmlZip,
  writeTmluXml,
} from './surveyFileWriters';
import {
  COMPASS_DATUMS,
  type CompassDatSpec,
  type CompassMakSpec,
  type CompassMakStationSpec,
  type CompassShotSpec,
  type CompassSurveySpec,
  type GpxPointSpec,
  type GpxRouteSpec,
  type GpxSpec,
  type GpxTrackSpec,
  type WallsFixSpec,
  type WallsShotSpec,
  type WallsSrvSpec,
  type WallsWpjEntrySpec,
  type WallsWpjSpec,
  compassHasBacksights,
  writeCompassDat,
  writeCompassMak,
  writeGpxXml,
  writeWallsSrv,
  writeWallsWpj,
} from './surveyFileWritersText';

/** The framework's PRNG shape (`ScenarioContext.rng`). */
export type Rng = () => number;

/** `valid` aims to parse and import; `malformed` aims to be rejected cleanly. */
export type SpecProfile = 'valid' | 'malformed';

export type SurveyFileFormat = 'dmp' | 'tml' | 'tmlu' | 'dat' | 'mak' | 'srv' | 'wpj' | 'gpx';

/**
 * A sibling a multi-file spec carries. `name` is a plain basename.
 *
 * Content-addressed by the generator (`surveyFileName`), never sequential: a
 * `.mak` names its `.dat` files by relative path and both land in ONE directory
 * alongside every other file the run generates, so `part-0.dat` from two
 * different projects would collide and each project would then read the other's
 * surveys. Two byte-identical siblings colliding is harmless by construction.
 */
export interface SiblingDatSpec {
  name: string;
  dat: CompassDatSpec;
}

export interface SiblingSrvSpec {
  name: string;
  srv: WallsSrvSpec;
}

/** A generated file: what to write, and under what name. */
export type GeneratedFileSpec =
  | { format: 'dmp'; name: string; profile: SpecProfile; dmp: DmpSpec }
  | { format: 'tml'; name: string; profile: SpecProfile; tml: TmlSpec }
  | { format: 'tmlu'; name: string; profile: SpecProfile; tml: TmlSpec }
  | { format: 'dat'; name: string; profile: SpecProfile; dat: CompassDatSpec }
  | {
      format: 'mak';
      name: string;
      profile: SpecProfile;
      mak: CompassMakSpec;
      dats: readonly SiblingDatSpec[];
    }
  | { format: 'srv'; name: string; profile: SpecProfile; srv: WallsSrvSpec }
  | {
      format: 'wpj';
      name: string;
      profile: SpecProfile;
      wpj: WallsWpjSpec;
      srvs: readonly SiblingSrvSpec[];
    }
  | { format: 'gpx'; name: string; profile: SpecProfile; gpx: GpxSpec };

/**
 * The five families the fuzzer's format axis draws from.
 *
 * A family is "one parser and one import dialog", not "one extension": a `.dat`
 * and a `.mak` both go through `parse_compass` and the same Compass dialog with
 * the same two checkboxes, and `.srv`/`.wpj` both through `parse_walls`. Drawing
 * a family and then an extension inside it keeps the four one-file formats from
 * being starved by the two project formats, which are strictly rarer in the wild
 * and strictly more expensive to generate.
 */
export type SurveyFormatFamily = 'dmp' | 'tml' | 'compass' | 'walls' | 'gpx';

export const SURVEY_FORMAT_FAMILIES: readonly SurveyFormatFamily[] = [
  'dmp',
  'tml',
  'compass',
  'walls',
  'gpx',
];

export const FORMATS_IN_FAMILY: Readonly<Record<SurveyFormatFamily, readonly SurveyFileFormat[]>> = {
  dmp: ['dmp'],
  tml: ['tml', 'tmlu'],
  compass: ['dat', 'mak'],
  walls: ['srv', 'wpj'],
  gpx: ['gpx'],
};

export function familyOf(format: SurveyFileFormat): SurveyFormatFamily {
  if (format === 'dmp') return 'dmp';
  if (format === 'tml' || format === 'tmlu') return 'tml';
  if (format === 'dat' || format === 'mak') return 'compass';
  if (format === 'srv' || format === 'wpj') return 'walls';
  return 'gpx';
}

// ---------------------------------------------------------------------------
// Draw helpers (mirrors of undoRedoFuzzActions.ts, against a bare Rng)
// ---------------------------------------------------------------------------

/** Share of generated values that take an extreme instead of a nominal one. */
export const P_BOUNDARY = 0.15;

export function chance(rng: Rng, probability: number): boolean {
  return rng() < probability;
}

export function randInt(rng: Rng, min: number, max: number): number {
  return min + Math.floor(rng() * (max - min + 1));
}

export function pick<T>(rng: Rng, items: readonly T[]): T {
  // rng() is in [0, 1), so the index is in range for any non-empty list (an
  // empty one still yields undefined, exactly as before the flag).
  return items[Math.floor(rng() * items.length)]!;
}

/** An integer draw that lands on `extremes` `P_BOUNDARY` of the time. */
function biasedInt(rng: Rng, min: number, max: number, extremes: readonly number[]): number {
  if (rng() < P_BOUNDARY) return pick(rng, extremes);
  return randInt(rng, min, max);
}

// ---------------------------------------------------------------------------
// Hostile text corpus
// ---------------------------------------------------------------------------

/**
 * Comment text that has historically broken something downstream of an import.
 *
 * The first four are spreadsheet-formula injection: a comment that reaches a CSV
 * or clipboard export unquoted is executed by Excel and LibreOffice on paste.
 * The rest are encoding and markup hazards - a leading control character, XML
 * fragments that must arrive as data, an entity that must not be decoded twice,
 * a zero-width joiner that makes two comments look like one, a bidi override
 * that reorders whatever follows it in a table cell, and a payload large enough
 * to find a length clamp.
 *
 * LONE SURROGATES ARE ABSENT, deliberately. An unpaired UTF-16 code unit cannot
 * survive a JS string on its way to bytes: `TextEncoder` substitutes U+FFFD, so
 * a spec carrying `'\uD83D'` would render the same bytes as one carrying
 * U+FFFD, and the trace would claim to reproduce a case it never produced. The
 * stand-in is U+FFFD itself, spelled out below - it exercises the same "the
 * decoder already gave up on this" path in the receipt and label code without
 * pretending the writer can emit something it cannot.
 */
export const HOSTILE_COMMENTS: readonly string[] = [
  "=cmd|' /C calc'!A0",
  '+1+1',
  '-2+3',
  '@SUM(1)',
  '\tleading tab',
  '\rleading carriage return',
  ']]>',
  '<![CDATA[',
  '&amp;',
  'raw & ampersand',
  'part one\u200bpart two\u200bpart three',
  '\u202eoverridden',
  '\ufffd replacement, standing in for a lone surrogate',
  'x'.repeat(8 * 1024),
];

/** Section/explorer/cave names that are not just ASCII words. */
const BOUNDARY_NAMES: readonly string[] = [
  '',
  ' ',
  'Cueva del Rio \u00d1andu\u0301',
  '\u6d1e\u7a74\u6e2c\u91cf',
  '\ud83d\udd26\ud83e\udd3f',
  'x'.repeat(512),
  '"quoted", \\backslash\\ and \nnewline',
];

function biasedName(rng: Rng, nominal: string): string {
  if (rng() < P_BOUNDARY) return pick(rng, BOUNDARY_NAMES);
  return nominal;
}

function maybeHostileComment(rng: Rng, nominal: string): string {
  if (rng() < P_BOUNDARY) return pick(rng, HOSTILE_COMMENTS);
  return nominal;
}

/**
 * The same corpus, made safe to put on a LINE of a line-oriented format.
 *
 * CR and LF are removed, and only those. Compass reads a line with
 * `recognize_line` = `not_line_ending` + `line_ending` (`parser_utils.rs:38-42`),
 * so an embedded `\n` ends the row early and an embedded bare `\r` fails
 * `not_line_ending` outright - either one truncates the survey's whole shot list
 * with no warning, which is a corruption, not a valid file. `\x0c` goes too: it
 * is the Compass survey terminator (`survey/parser/mod.rs:147`) and would split
 * a survey in the middle of a comment. Everything else in the corpus - the
 * formula-injection prefixes, the bidi override, the 8 KB payload - survives,
 * because in every one of these formats a comment is free text to end of line.
 */
function lineSafe(text: string): string {
  return text.replace(/[\r\n\f]/g, ' ');
}

function maybeHostileLine(rng: Rng, nominal: string): string {
  return lineSafe(maybeHostileComment(rng, nominal));
}

// ---------------------------------------------------------------------------
// DMP
// ---------------------------------------------------------------------------

/** Standard dominates; the two cross-section types are drawn occasionally. */
const VALID_SHOT_TYPES: readonly number[] = [2, 2, 2, 2, 2, 0, 1];

function randomDmpShot(rng: Rng, profile: SpecProfile): DmpShotSpec {
  const malformed = profile === 'malformed';
  return {
    shotType: malformed ? randInt(rng, 0, 255) : pick(rng, VALID_SHOT_TYPES),
    headingIn: biasedInt(rng, 0, 3599, [0, 3599, 3600, -1, 32767, -32768]),
    headingOut: biasedInt(rng, 0, 3599, [0, 3599, 3600, -1, 32767, -32768]),
    // Never exactly zero in either profile: see the zero-run invariant at the
    // top of this file. A NEGATIVE length is still a boundary the parser has to
    // handle (`.max(0.0)` clamps it) and its bytes are not zero.
    length: biasedInt(rng, 1, 2000, [1, -1, 32767, -32768, 20000]),
    depthIn: biasedInt(rng, 0, 4000, [0, -1, 32767, -32768]),
    depthOut: biasedInt(rng, 0, 4000, [0, -1, 32767, -32768]),
    pitchIn: biasedInt(rng, -900, 900, [0, 900, -900, 32767, -32768]),
    pitchOut: biasedInt(rng, -900, 900, [0, 900, -900, 32767, -32768]),
    left: biasedInt(rng, 0, 500, [0, -1, 32767]),
    right: biasedInt(rng, 0, 500, [0, -1, 32767]),
    up: biasedInt(rng, 0, 500, [0, -1, 32767]),
    down: biasedInt(rng, 0, 500, [0, -1, 32767]),
    temperature: biasedInt(rng, 40, 300, [0, -400, 32767]),
    hour: randInt(rng, 0, 23),
    minute: randInt(rng, 0, 59),
    second: randInt(rng, 0, 59),
    markerIndex: malformed ? randInt(rng, 0, 255) : biasedInt(rng, 1, 9, [1, 9, 255]),
  };
}

const DMP_NAME_ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';

function randomDmpSectionName(rng: Rng, profile: SpecProfile): DmpSectionSpec['name'] {
  if (profile === 'malformed' && chance(rng, 0.3)) {
    // Bytes that are not valid UTF-8 on their own: `parse_section` falls back to
    // a numeric spelling rather than failing, and that fallback name then flows
    // into the preview, the content hash and the imported section title.
    return [randInt(rng, 128, 255), randInt(rng, 128, 255), randInt(rng, 128, 255)];
  }
  const letter = (): string => DMP_NAME_ALPHABET[randInt(rng, 0, DMP_NAME_ALPHABET.length - 1)]!;
  return `${letter()}${letter()}${letter()}`;
}

/**
 * A DMP file spec.
 *
 * The `valid` profile keeps every field inside what `parse_section` accepts -
 * year 2000..2099, month 1..12, day 1..28 (never a day that only exists in some
 * months, so the file parses in February too), hour < 24, minute < 60 - and
 * relies on the corruption catalog for everything past those edges.
 */
export function randomDmpSpec(rng: Rng, profile: SpecProfile = 'valid'): DmpSpec {
  const malformed = profile === 'malformed';
  const version = pick(rng, [5, 5, 5, 4, 4, 3, 2]);
  const sectionCount = randInt(rng, 1, 4);
  const sections: DmpSectionSpec[] = [];

  for (let index = 0; index < sectionCount; index++) {
    const section: DmpSectionSpec = {
      name: randomDmpSectionName(rng, profile),
      direction: malformed ? randInt(rng, 0, 255) : pick(rng, [0, 1]),
      year: biasedInt(rng, 20, 26, [0, 99]),
      month: biasedInt(rng, 1, 12, [1, 12]),
      day: biasedInt(rng, 1, 28, [1, 28]),
      hour: biasedInt(rng, 0, 23, [0, 23]),
      minute: biasedInt(rng, 0, 59, [0, 59]),
      shots: Array.from({ length: randInt(rng, 1, 8) }, () => randomDmpShot(rng, profile)),
    };
    // A clone of an earlier section collides on `content_hash` while carrying
    // its own date, which is the shape re-import deduplication has to notice.
    if (index > 0 && chance(rng, 0.2)) section.sameContentAs = randInt(rng, 0, index - 1);
    sections.push(section);
  }

  const spec: DmpSpec = { version, sections };
  if (chance(rng, 0.5)) spec.signedTokens = true;
  return spec;
}

// ---------------------------------------------------------------------------
// DMP, shaped so the placement suggester finds something
// ---------------------------------------------------------------------------

/**
 * A DMP whose sections OVERLAP each other, so `suggest_all_import_placements`
 * returns a non-empty pool.
 *
 * ## Why this exists
 *
 * `randomDmpSpec` draws every shot independently, and the suggester
 * (`models/src/placement_suggest.rs`) matches shot-for-shot inside hard cutoffs:
 * 25 degrees of azimuth, `max(3% of length, 0.4 m)` of length, 0.30 m of depth
 * per measurement point, with a 0.45 aggregate-score floor. Two independently
 * random sections clear none of that, so the pool comes back empty,
 * `pickFromPool` returns false, and the wizard falls straight into
 * `manual_placing`. Measured over a 130-cell
 * campaign: 47 steps on `wiz-suggest` against 13,781 total, ZERO clicks on an
 * alternative, and not one confirmed overlap - so the merge-station picker, the
 * overlap modes, `applyAllOverlapTreatments` and the `ExistingShotUpdate` path
 * behind them were never exercised at all.
 *
 * ## The shape
 *
 * One ANCHOR section carries a MOTIF - a short run of shots - and carries it
 * TWICE, separated by filler. Every other section replays that motif with jitter
 * that stays well inside the cutoffs above:
 *
 *   anchor  [lead] MOTIF [middle] MOTIF' [tail]
 *   head    MOTIF(jitter) [tail turning away]      <- matches from station 0
 *   tail    [lead turning in] MOTIF(jitter)        <- matches from station N
 *   tie     MOTIF(coarser jitter) [tail]           <- a second, lower-scoring
 *                                                     candidate for the same run
 *
 * Each of those is a leaf the suggester actually tries: it builds reference
 * shots from the first `max_reference_shots` in natural order AND from the last
 * `max_reference_shots` reversed, then sweeps every start index in the target
 * both forward and reverse. The `head` section exercises the first leaf, `tail`
 * the second (which arrives as a `Reverse` match), and the anchor's REPEATED
 * motif is what makes the same import section match in two places - which is
 * the only way `alternatives` is ever non-empty, since the wizard filters it to
 * suggestions sharing `importSectionIndex`.
 *
 * ## Field choices that are not arbitrary
 *
 * `direction` is pinned to 0 (In) rather than drawn: it is section metadata that
 * the wizard's own reverse checkbox can still flip, and pinning it keeps the
 * motif's meaning stable across the four sections. Every shot is `Standard`
 * (type 2), because that is the only type the wizard counts when it sizes the
 * merge-offset list. Lengths stay in 3..9 m and depth steps under a metre, so
 * `|depth_out - depth_in| < length` always holds and the horizontal projection
 * is real. Filler and tails turn 90 degrees away and descend, so a replayed
 * section leaves the anchor's corridor instead of running back across it and
 * losing the suggestion to the crossing-clearance guard.
 *
 * The zero-run invariant in this module's header is kept the same way
 * `randomDmpShot` keeps it: `length` and `markerIndex` are never zero, and
 * depths start above zero too.
 */
const SUGGESTIVE_SECTION_KINDS = ['head', 'tail', 'tie'] as const;

/** Jitter, in the DMP's own pre-scaled integer units, per replay kind. */
const SUGGESTIVE_JITTER: Readonly<
  Record<(typeof SUGGESTIVE_SECTION_KINDS)[number], { heading: number; length: number; depth: number }>
> = {
  // 1.5 deg / 6 cm / 4 cm - a near-exact replay, which scores close to 1.0.
  head: { heading: 15, length: 6, depth: 4 },
  tail: { heading: 15, length: 6, depth: 4 },
  // 3.0 deg / 14 cm / 8 cm - still inside every cutoff, but a visibly worse
  // score, so the tie section ranks BELOW the head one instead of alongside it.
  tie: { heading: 30, length: 14, depth: 8 },
};

/** A motif shot, in the units `DmpShotSpec` stores. */
interface MotifShot {
  heading: number;
  length: number;
  depthIn: number;
  depthOut: number;
}

/**
 * A run of shots.
 *
 * With `length0` the run is DETERMINISTIC in every field the suggester compares
 * - azimuth, length and the two depths - so two files that drew the same shape
 * produce the same motif and can match each other. Without it the run is drawn,
 * which is what the filler wants: filler must not look like anything.
 */
function motifRun(
  rng: Rng,
  count: number,
  heading0: number,
  headingStep: number,
  depth0: number,
  length0?: number,
): MotifShot[] {
  const out: MotifShot[] = [];
  let heading = heading0;
  let depth = depth0;
  for (let i = 0; i < count; i++) {
    const length = length0 === undefined ? randInt(rng, 300, 900) : length0 + i * 70;
    // Under a metre of depth change per shot, and never more than a third of
    // the slope length, so the horizontal projection stays comfortably real.
    const step = length0 === undefined ? randInt(rng, -60, 90) : 45 + (i % 3) * 15;
    const next = Math.max(20, Math.min(3800, depth + step));
    out.push({ heading, length, depthIn: depth, depthOut: next });
    depth = next;
    heading = (heading + headingStep + 3600) % 3600;
  }
  return out;
}

/** Turn a motif shot into a `DmpShotSpec`, optionally jittered. */
function motifShotSpec(rng: Rng, shot: MotifShot, jitter: { heading: number; length: number; depth: number }): DmpShotSpec {
  const wobble = (amp: number): number => (amp === 0 ? 0 : randInt(rng, -amp, amp));
  const heading = ((shot.heading + wobble(jitter.heading)) % 3600 + 3600) % 3600;
  const length = Math.max(50, shot.length + wobble(jitter.length));
  const depthIn = Math.max(0, shot.depthIn + wobble(jitter.depth));
  const depthOut = Math.max(0, shot.depthOut + wobble(jitter.depth));
  // Pitch is not part of the match (the suggester reads azimuth, length and the
  // two depths), but an inconsistent one would make the rendered geometry
  // disagree with the numbers the match was made on.
  const dz = (depthOut - depthIn) / 100;
  const slope = Math.max(0.01, length / 100);
  const ratio = Math.max(-1, Math.min(1, dz / slope));
  const pitch = Math.round((Math.asin(ratio) * 180) / Math.PI * 10);
  return {
    shotType: 2,
    headingIn: heading,
    headingOut: heading,
    length,
    depthIn,
    depthOut,
    pitchIn: pitch,
    pitchOut: pitch,
    left: randInt(rng, 0, 300),
    right: randInt(rng, 0, 300),
    up: randInt(rng, 0, 300),
    down: randInt(rng, 0, 300),
    temperature: randInt(rng, 40, 300),
    hour: randInt(rng, 0, 23),
    minute: randInt(rng, 0, 59),
    second: randInt(rng, 0, 59),
    markerIndex: randInt(rng, 1, 9),
  };
}

/**
 * Filler that leaves the anchor's corridor: 90 degrees off and descending, so a
 * replayed section's non-overlapping part does not run back across the anchor
 * and lose the suggestion to `placement_crossing_clearance_ok`.
 */
function turnAwayRun(rng: Rng, count: number, fromHeading: number, depth0: number): MotifShot[] {
  return motifRun(rng, count, (fromHeading + 900) % 3600, randInt(rng, -40, 40), depth0 + randInt(rng, 100, 400));
}

/**
 * Filler that turns away and comes BACK to `toDepth`.
 *
 * The suggester compares ABSOLUTE `depth_in`/`depth_out` inside 0.30 m, so the
 * anchor's second copy of the motif has to sit at the same depths as the first
 * or it is not a repeat at all. This is the loop that gets it back there.
 */
function bridgeRun(rng: Rng, count: number, fromHeading: number, fromDepth: number, toDepth: number): MotifShot[] {
  const out: MotifShot[] = [];
  let heading = (fromHeading + 900) % 3600;
  let depth = fromDepth;
  for (let i = 0; i < count; i++) {
    const next = Math.round(fromDepth + ((toDepth - fromDepth) * (i + 1)) / count);
    // Long enough that the depth change is a small fraction of the slope, and
    // long enough to carry the trace clear of the corridor it just left.
    const length = Math.max(400, Math.abs(next - depth) * 3 + randInt(rng, 400, 900));
    out.push({ heading, length, depthIn: depth, depthOut: next });
    depth = next;
    heading = (heading + randInt(rng, -300, 300) + 3600) % 3600;
  }
  return out;
}

function suggestiveSection(
  rng: Rng,
  name: DmpSectionSpec['name'],
  shots: readonly MotifShot[],
  jitter: { heading: number; length: number; depth: number },
): DmpSectionSpec {
  return {
    name,
    direction: 0,
    year: randInt(rng, 20, 26),
    month: randInt(rng, 1, 12),
    day: randInt(rng, 1, 28),
    hour: randInt(rng, 0, 23),
    minute: randInt(rng, 0, 59),
    shots: shots.map((shot) => motifShotSpec(rng, shot, jitter)),
  };
}

/**
 * Motif shapes two DIFFERENT files can share.
 *
 * Within one file the anchor's repeat is what produces alternatives. ACROSS
 * files nothing matched at all, and that is what kept the orphan surface dark:
 * an orphan component is a set of EXISTING sections with no datum and no
 * newly-imported member (`Project::suggest_orphan_placements`), so the only way
 * this fuzzer makes one is to place a segment manually in one flow and then
 * import something that MATCHES it in a later flow. With every file drawing a
 * fresh random motif, the later import never matched.
 *
 * Four shapes, drawn per file: two files in the same episode collide often
 * enough to make the cross-file match routine, while the files stay visibly
 * different (section count, filler, jitter and every date/LRUD field still come
 * off the stream).
 */
const SUGGESTIVE_MOTIF_SHAPES: readonly { heading0: number; headingStep: number; depth0: number; length0: number }[] = [
  { heading0: 200, headingStep: 60, depth0: 320, length0: 480 },
  { heading0: 1100, headingStep: -45, depth0: 780, length0: 640 },
  { heading0: 2000, headingStep: 95, depth0: 150, length0: 550 },
  { heading0: 2900, headingStep: -80, depth0: 610, length0: 720 },
];

export function suggestiveDmpSpec(rng: Rng): DmpSpec {
  const version = pick(rng, [5, 5, 5, 4, 4, 3]);
  const motifLength = randInt(rng, 3, 5);
  const shape = pick(rng, SUGGESTIVE_MOTIF_SHAPES);
  const heading0 = shape.heading0;
  const headingStep = shape.headingStep;
  const depth0 = shape.depth0;
  const motif = motifRun(rng, motifLength, heading0, headingStep, depth0, shape.length0);
  const zero = { heading: 0, length: 0, depth: 0 };
  const lastHeading = motif[motif.length - 1]!.heading;
  const lastDepth = motif[motif.length - 1]!.depthOut;

  // The anchor: motif, a loop that comes back to the motif's own starting depth,
  // then the SAME motif again. The repeat is exact on purpose - it is what gives
  // a replaying section two equally good candidate placements, which is the only
  // way the wizard's alternatives list is ever non-empty (it filters to
  // suggestions sharing `importSectionIndex`) and the only way its tie-break is
  // ever exercised.
  const anchorShots: MotifShot[] = [
    ...motif,
    ...bridgeRun(rng, randInt(rng, 2, 3), lastHeading, lastDepth, depth0),
    ...motif,
  ];

  const letter = (index: number): string => `A${String.fromCharCode(65 + (index % 26))}${index % 10}`;
  const sections: DmpSectionSpec[] = [suggestiveSection(rng, letter(0), anchorShots, zero)];

  // Two or three replays, always including `head` so the station-0 leaf is
  // always exercised; the rest are drawn so a seed does not always produce the
  // same four-section file.
  const kinds: (typeof SUGGESTIVE_SECTION_KINDS)[number][] = ['head'];
  if (chance(rng, 0.75)) kinds.push('tail');
  if (chance(rng, 0.6)) kinds.push('tie');

  for (const kind of kinds) {
    const jitter = SUGGESTIVE_JITTER[kind];
    const shots =
      kind === 'tail'
        ? [...turnAwayRun(rng, randInt(rng, 1, 2), heading0, depth0), ...motif]
        : [...motif, ...turnAwayRun(rng, randInt(rng, 1, 2), lastHeading, lastDepth)];
    sections.push(suggestiveSection(rng, letter(sections.length), shots, jitter));
  }

  const spec: DmpSpec = { version, sections };
  if (chance(rng, 0.5)) spec.signedTokens = true;
  return spec;
}

// ---------------------------------------------------------------------------
// TML / TMLU
// ---------------------------------------------------------------------------

const TML_UNITS: readonly string[] = ['m', 'm', 'm', 'ft', 'metric', 'imperial'];
const TML_EXPLORERS: readonly string[] = ['', 'Tester', 'A. Surveyor', 'team-2'];

function hex2(rng: Rng): string {
  return randInt(rng, 0, 255).toString(16).padStart(2, '0');
}

function randomTmlColor(rng: Rng, profile: SpecProfile): string {
  if (profile === 'malformed' && chance(rng, 0.4)) {
    return pick(rng, ['', 'not-a-color', '0x', '#ff00', '0xZZZZZZZZ', '0x8f6112']);
  }
  return `0x${hex2(rng)}${hex2(rng)}${hex2(rng)}ff`;
}

function randomTmlDate(rng: Rng): string {
  // `2000-01-01` is Ariane's "no date" sentinel, so it gets deliberate weight
  // rather than being one date in seven thousand.
  if (chance(rng, P_BOUNDARY)) return pick(rng, ['2000-01-01', '', '0000-00-00', '2099-12-31']);
  return `20${String(randInt(rng, 10, 26)).padStart(2, '0')}-${String(randInt(rng, 1, 12)).padStart(2, '0')}-${String(
    randInt(rng, 1, 28),
  ).padStart(2, '0')}`;
}

function roundedFloat(rng: Rng, min: number, max: number, decimals: number): number {
  return Number((min + rng() * (max - min)).toFixed(decimals));
}

function biasedFloat(rng: Rng, min: number, max: number, decimals: number, extremes: readonly number[]): number {
  if (rng() < P_BOUNDARY) return pick(rng, extremes);
  return roundedFloat(rng, min, max, decimals);
}

/**
 * A TML/TMLU spec.
 *
 * The `valid` profile builds a proper rooted tree: shot 0 is the `START` with no
 * parent, every later shot names an EARLIER id as its `FromID`, and ids are
 * unique and non-negative. Cycles, self-parents, dangling parents, duplicate ids
 * and negative ids are all reachable - through `malformed`, or through the
 * schema-level entries of the corruption catalog.
 */
export function randomTmlSpec(rng: Rng, profile: SpecProfile = 'valid'): TmlSpec {
  const malformed = profile === 'malformed';
  // At least TWO in the valid profile: shot 0 is the `START`, which is a
  // station rather than a leg, so a one-shot file is a well-formed document
  // describing an empty survey - it parses, imports nothing, and spends a whole
  // fuzz flow on a dialog with no content. `malformed` keeps the one-shot case,
  // where an empty import IS the thing being tested.
  const shotCount = malformed ? randInt(rng, 1, 12) : randInt(rng, 2, 12);
  const sectionNames = Array.from({ length: randInt(rng, 1, 3) }, (_unused, index) =>
    biasedName(rng, `Section ${index + 1}`),
  );

  const shots: TmlShotSpec[] = [];
  for (let index = 0; index < shotCount; index++) {
    const isStart = index === 0;
    const parent = isStart ? -1 : randInt(rng, 0, index - 1);
    shots.push({
      id: malformed && chance(rng, 0.25) ? randInt(rng, -5, shotCount) : index,
      fromId: malformed && chance(rng, 0.25) ? randInt(rng, -5, shotCount + 5) : parent,
      closureToId: !isStart && chance(rng, 0.1) ? randInt(rng, 0, index - 1) : -1,
      type: isStart ? 'START' : pick(rng, ['REAL', 'REAL', 'REAL', 'REAL', 'VIRTUAL']),
      section: pick(rng, sectionNames),
      azimuth: biasedFloat(rng, 0, 359.9, 1, [0, 359.9, 360, -1, 1e6]),
      length: biasedFloat(rng, 0.1, 30, 2, [0, -1, 1e-9, 1e6]),
      depth: biasedFloat(rng, 0, 60, 2, [0, -1, 1e6]),
      // -1.0 is the file's "not recorded" sentinel, read by `resolve_tml_depth_in`.
      depthIn: chance(rng, 0.3) ? -1 : biasedFloat(rng, 0, 60, 2, [0, -1, 1e6]),
      inclination: biasedFloat(rng, -90, 90, 1, [0, 90, -90, 1e6]),
      left: biasedFloat(rng, 0, 5, 2, [0, -1]),
      right: biasedFloat(rng, 0, 5, 2, [0, -1]),
      up: biasedFloat(rng, 0, 5, 2, [0, -1]),
      down: biasedFloat(rng, 0, 5, 2, [0, -1]),
      comment: chance(rng, 0.3) ? maybeHostileComment(rng, `note-${randInt(rng, 0, 9999)}`) : '',
      explorer: pick(rng, TML_EXPLORERS),
      date: randomTmlDate(rng),
      color: randomTmlColor(rng, profile),
      excluded: chance(rng, 0.1),
      latitude: chance(rng, 0.2) ? roundedFloat(rng, -90, 90, 6) : 0,
      longitude: chance(rng, 0.2) ? roundedFloat(rng, -180, 180, 6) : 0,
    });
  }

  const spec: TmlSpec = {
    caveName: biasedName(rng, `fuzz-cave-${randInt(rng, 1000, 9999)}`),
    unit: malformed && chance(rng, 0.3) ? pick(rng, ['', 'furlongs', 'M ', '\u00b5m']) : pick(rng, TML_UNITS),
    useMagneticAzimuth: chance(rng, 0.8),
    firstStartAbsoluteElevation:
      malformed && chance(rng, 0.3)
        ? pick(rng, ['', 'NaN', 'abc', '1e999', 'Infinity'])
        : String(biasedFloat(rng, -500, 2000, 2, [0, -1, 1e6])),
    shots,
  };
  if (chance(rng, 0.5)) spec.dialect = 'short';
  return spec;
}

// ---------------------------------------------------------------------------
// Compass .DAT
// ---------------------------------------------------------------------------

/**
 * FORMAT strings that are all genuinely legal, covering all four legal lengths.
 *
 * `parse_survey_format` (`survey_parameters.rs:108-200`) accepts a length of
 * exactly 11, 12, 13 or 15 and NOTHING else - any other length is
 * `ErrorKind::LengthValue`, which kills the whole survey silently. The layout is
 * 1 bearing unit (`D`/`Q`/`R`), 1 length unit (`D`/`I`/`M`), 1 passage unit, 1
 * inclination unit (`D`/`G`/`M`/`R`/`W`), 4 passage dimensions (a permutation of
 * `U D R L`), then THREE shot items (`L A D a d`) for lengths 11/12/13 but FIVE
 * for length 15, then the backsight char (`B`/`N`, lengths >= 12) and the LRUD
 * association (`F`/`T`, lengths >= 13).
 *
 * Both length units (`D` decimal feet, `M` meters) and both backsight chars are
 * represented, because those are the only two format facts that change what the
 * data rows must contain: `length_units` (`compass.rs:709-712`) and
 * `has_backsights` (`survey/mod.rs:288-296`). Which of the two is the backsight
 * slot is NOT a fixed index - see `compassHasBacksights`, which is imported
 * rather than re-derived here.
 */
const COMPASS_FORMATS: readonly string[] = [
  'DDDDLUDRLAD', // 11, decimal feet, no backsight slot at all
  'DMMDLUDRLAD', // 11, meters
  'DDDDLUDRLADN', // 12, feet, no backsights
  'DDDDLUDRLADB', // 12, feet, backsights
  'DMMDLUDRLADN', // 12, meters, no backsights
  'DMMDLUDRLADB', // 12, meters, backsights
  'DDDMLUDRLADNF', // 13, feet, DDD.MM packed inclination
  'DDDDLUDRLADBF', // 13, feet, backsights, from-station LRUD
  'DMMDLUDRLADBT', // 13, meters, backsights, to-station LRUD
  'DMMGLUDRLADNF', // 13, meters, percent-grade inclination
  'DDDDLUDRLADadNF', // 15, feet, five shot items, backsight slot at index 13
  'DDDDLUDRLADadBF', // 15, feet, backsights
  'DMMDLUDRLADadBF', // 15, meters, backsights
  'DMMRLUDRLADadNT', // 15, meters, grads inclination
];

/**
 * Station-name decorations, all inside `is_valid_station_name_char`.
 *
 * `parse_station_name` is `take_while1` over `[A-Za-z0-9_\-'*]`
 * (`parser_utils.rs:19-21, 27-30`). A `.`, `,`, `#` or `/` makes the row fail and
 * `many0(parse_shot)` then drops that row AND every row after it in the survey,
 * with no warning at all - so a valid spec may never emit one.
 */
const COMPASS_NAME_SUFFIXES: readonly string[] = ['', '', '', "'", '*', '_a', '-1'];

const COMPASS_TEAMS: readonly string[] = [
  'NOBODY',
  'A. Surveyor, B. Notes',
  'team-2',
  'E Xample / J Doe',
];

function compassNumber(rng: Rng, min: number, max: number, decimals: number): string {
  return (min + rng() * (max - min)).toFixed(decimals);
}

/**
 * The shots of one survey, as a chain `s0 -> s1 -> ... -> sN`.
 *
 * A chain rather than a star because that is what a real trip is, and because
 * `import_compass` reconstructs components from shared station names - a survey
 * whose shots do not touch would land as N components and turn EVERY generated
 * file into an orphan-placement case, which is the opposite of the coverage
 * spread this file exists to produce.
 */
function compassShots(
  rng: Rng,
  stations: readonly string[],
  hasBacksights: boolean,
): CompassShotSpec[] {
  const shots: CompassShotSpec[] = [];
  for (let index = 0; index + 1 < stations.length; index++) {
    const shot: CompassShotSpec = {
      from: stations[index]!,
      to: stations[index + 1]!,
      // Never 0 and never >= 990: both make `length_missing` true and the shot
      // is DROPPED with a warning (`compass.rs:472-474, 1161-1177`). A valid
      // spec that declares shots it does not get is not a valid spec.
      length: compassNumber(rng, 0.5, 60, 2),
      bearing: compassNumber(rng, 0, 359.99, 2),
      inclination: compassNumber(rng, -89, 89, 2),
      // `lrud_meters` reads `<= -1.0`, `>= 990` and a converted magnitude below
      // `f64::EPSILON` as "no LRUD" (`compass.rs:476-486`), so 0.00 and -9.90
      // are the same thing. Drawn above zero so the dimension actually lands.
      left: compassNumber(rng, 0.1, 8, 2),
      up: compassNumber(rng, 0.1, 8, 2),
      down: compassNumber(rng, 0.1, 8, 2),
      right: compassNumber(rng, 0.1, 8, 2),
    };
    if (hasBacksights) {
      // ALWAYS both, never one: a declared-but-absent backsight pair fails
      // `parse_double`, `many0` stops, and every remaining row vanishes
      // (`survey/parser/mod.rs:109-115`).
      shot.backBearing = compassNumber(rng, 0, 359.99, 2);
      shot.backInclination = compassNumber(rng, -89, 89, 2);
    }
    // The two controls the Compass import dialog exists for: `X` is the only
    // flag character `parse_flags` acts on (`compass.rs:446-456`) and drives
    // "include excluded", and a comment drives "comments on stations".
    if (chance(rng, 0.12)) shot.flags = 'X';
    if (chance(rng, 0.3)) shot.comment = maybeHostileLine(rng, `lead-${randInt(rng, 0, 999)}`);
    shots.push(shot);
  }
  return shots;
}

/**
 * A Compass `.dat` spec: 1..3 surveys of 1..8 shots.
 *
 * The `valid` profile keeps every rule that decides whether a survey exists at
 * all, each cited at its field. The two that are easiest to get wrong and
 * impossible to notice afterwards:
 *
 *   - the date is a REAL calendar day (`from_ymd_opt`, `survey/parser/mod.rs:141`
 *     - since #479 an impossible one rejects the survey rather than panicking,
 *     but it still loses it);
 *   - the team line is never blank, because `tag("SURVEY TEAM:")` is followed by
 *     `multispace1` (`survey/parser/mod.rs:55`) which is greedy ACROSS newlines,
 *     so a blank team line eats the `DECLINATION:` line and kills the survey.
 *
 * MULTI-COMPONENT COVERAGE. Half the time the surveys share no station name at
 * all (`A1..`, `B1..`), which lands as two disconnected components and is what
 * raises the orphan-placement dialog; the other half survey N+1 starts at survey
 * N's last station, which lands as one. Compass has no explicit topology, so
 * station-name overlap is the ONLY thing that decides this - which makes it the
 * only way the orphan screen is reachable for this format.
 */
export function randomCompassDatSpec(rng: Rng, profile: SpecProfile = 'valid'): CompassDatSpec {
  const malformed = profile === 'malformed';
  const surveyCount = randInt(rng, 1, 3);
  const disjoint = chance(rng, 0.5);
  // NON-BLANK, always, and the reason is worth stating because the failure it
  // causes is silent and total. A survey terminator is
  // `opt(ws(tag("\x0c")))` (`survey/parser/mod.rs:147`), and `ws` runs
  // `multispace0` AFTER the form feed - so a cave-name line that is empty or
  // all spaces is EATEN by the terminator, the next survey's `SURVEY NAME:`
  // line is read as its cave name, and `many0` stops there with no error at
  // all. Measured: a spec whose cave name was a single space rendered 3 surveys
  // into a `.dat` the parser read as 1, and the whole `.mak` self-check came
  // back 4 instead of 6.
  const nominalCaveName = `FUZZ CAVE ${randInt(rng, 100, 999)}`;
  const drawnCaveName = lineSafe(biasedName(rng, nominalCaveName));
  const caveName = drawnCaveName.trim() === '' ? nominalCaveName : drawnCaveName;

  const surveys: CompassSurveySpec[] = [];
  let previousLast: string | null = null;
  for (let index = 0; index < surveyCount; index++) {
    // The `malformed` alternatives are plausible-but-wrong LENGTHS (10 and 14),
    // which `survey_parameters.rs:196-199` rejects with `ErrorKind::LengthValue`.
    const format =
      malformed && chance(rng, 0.3)
        ? pick(rng, ['DDDDLUDRLA', 'DMMDLUDRLADadB'])
        : pick(rng, COMPASS_FORMATS);
    const hasBacksights = compassHasBacksights(format);
    const prefix = String.fromCharCode(65 + index);
    const shotCount = randInt(rng, 1, 8);
    const stations: string[] = [];
    for (let station = 0; station <= shotCount; station++) {
      if (station === 0 && !disjoint && previousLast !== null) {
        stations.push(previousLast);
        continue;
      }
      stations.push(`${prefix}${station}${pick(rng, COMPASS_NAME_SUFFIXES)}`);
    }
    previousLast = stations[stations.length - 1]!; // the loop above pushed shotCount + 1 >= 2 names

    const survey: CompassSurveySpec = {
      caveName,
      name: `${prefix}${randInt(rng, 1, 99)}`,
      // Real calendar day, always: 1..12 and 1..28 exist in every month and
      // every year, so the file parses in February too.
      month: malformed && chance(rng, 0.25) ? randInt(rng, 0, 20) : randInt(rng, 1, 12),
      day: malformed && chance(rng, 0.25) ? randInt(rng, 0, 40) : randInt(rng, 1, 28),
      year: randInt(rng, 1970, 2030),
      team: pick(rng, COMPASS_TEAMS),
      declination: compassNumber(rng, -20, 20, 2),
      format,
      shots: compassShots(rng, stations, hasBacksights),
    };
    if (chance(rng, 0.3)) survey.dateComment = maybeHostileLine(rng, `trip ${index + 1}`);
    // `CORRECTIONS2:` without `CORRECTIONS:` parses but yields neither
    // (`survey_parameters.rs:237-246`), so the second is only drawn under the
    // first - a spec that meant to set corrections and set none is a spec whose
    // claim about the file is wrong.
    if (chance(rng, 0.35)) {
      survey.corrections = [
        compassNumber(rng, -5, 5, 2),
        compassNumber(rng, -5, 5, 2),
        compassNumber(rng, -5, 5, 2),
      ];
      if (chance(rng, 0.4)) {
        survey.corrections2 = [compassNumber(rng, -5, 5, 2), compassNumber(rng, -5, 5, 2)];
      }
    }
    surveys.push(survey);
  }

  return { surveys };
}

// ---------------------------------------------------------------------------
// Compass .MAK
// ---------------------------------------------------------------------------

/** Flag strings with no repeated KEY, which is what `DuplicateFlag` counts. */
const COMPASS_FLAGS: readonly string[] = ['G', 'E', 'I', 'A', 'V', 'EV', 'GEV', 'GIV', 'OT', 'SXP'];

function hexDigit(rng: Rng): string {
  return '0123456789abcdef'[randInt(rng, 0, 15)]!;
}

/** A hyphenated 32-hex-digit id; `Uuid::parse_str` checks the shape, not the version. */
function randomUuid(rng: Rng): string {
  const group = (length: number): string =>
    Array.from({ length }, () => hexDigit(rng)).join('');
  return `${group(8)}-${group(4)}-${group(4)}-${group(4)}-${group(12)}`;
}

/**
 * A Compass project: one `.mak` plus the 1..3 `.dat` files it names.
 *
 * Ordering is the one hard rule: `to_file_state` (`project/parser.rs:95-103`)
 * needs BOTH a datum and a base location before any `#` entry, so `@` and `&`
 * are emitted first and `filesFirst` is left to the corruption catalog.
 *
 * Every number here is a plain decimal. The `.mak` lexer has its OWN
 * `parse_double` (`project/lexer.rs:247-272`) - sign, digits, optional `.`
 * fraction, and nothing else - so an exponent, a `nan` or a bare `.5` anywhere
 * fails the whole file. The datum comes from the 23-entry exact, case-sensitive
 * table (`project/parser.rs:213-244`); the zone is a `u8` in the lexer, so it is
 * drawn inside the real 1..60 UTM range rather than at the type's edge.
 *
 * NO `[folder;` entries in the valid profile: a folder prefixes the sibling path
 * with a subdirectory (`project/parser.rs:180-184`) and `materializeSurveyFile`
 * writes every sibling flat into one directory, so a folder is a guaranteed
 * `Survey file not found`.
 */
export function randomCompassProjectSpec(
  rng: Rng,
  profile: SpecProfile = 'valid',
): { mak: CompassMakSpec; dats: SiblingDatSpec[] } {
  const malformed = profile === 'malformed';
  const dats: SiblingDatSpec[] = Array.from({ length: randInt(rng, 1, 3) }, () => {
    const dat = randomCompassDatSpec(rng, profile);
    return { name: surveyFileName('dat', dat), dat };
  });

  const mak: CompassMakSpec = {
    baseLocation: {
      east: compassNumber(rng, 200000, 800000, 3),
      north: compassNumber(rng, 0, 9000000, 3),
      elevation: compassNumber(rng, -100, 3000, 3),
      zone: randInt(rng, 1, 60),
      convergence: compassNumber(rng, -3, 3, 3),
    },
    datum: malformed && chance(rng, 0.3) ? pick(rng, ['wgs 1984', 'NAD83', '']) : pick(rng, COMPASS_DATUMS),
    files: dats.map((sibling) => ({ path: sibling.name })),
  };
  if (chance(rng, 0.5)) mak.projectUuid = randomUuid(rng);
  // A comment runs to the next `/`, `\n` or `\r` (`project/lexer.rs:236-241`),
  // and a >= 32-character run of hex digits and hyphens before the `;` is lexed
  // as a project id instead and then fails `Uuid::parse_str` - so the text is
  // fixed prose rather than a draw from the hostile corpus.
  if (chance(rng, 0.4)) mak.comments = [`fuzz project ${randInt(rng, 1000, 9999)}`];
  if (chance(rng, 0.4)) mak.utmZone = randInt(rng, 1, 60);
  if (chance(rng, 0.35)) {
    mak.fileConvergence = { value: compassNumber(rng, -3, 3, 3), enabled: chance(rng, 0.7) };
  }
  if (chance(rng, 0.4)) mak.flags = pick(rng, COMPASS_FLAGS);

  // A `[unit,e,n,elev]` station fix on the first file is what makes
  // `has_georeference` true (`compass.rs:211-305`), and only the FIRST fixed
  // station becomes the GPS anchor. No whitespace anywhere inside the bracket:
  // `lex_station_fix` (`project/lexer.rs:419-470`) allows it after `[` and
  // around the unit char and NOWHERE else.
  if (chance(rng, 0.5) && dats.length > 0) {
    const first = dats[0]!.dat.surveys[0];
    if (first === undefined) throw new Error('the first .dat has no survey to anchor');
    const station: CompassMakStationSpec = {
      // `[alphanumeric*'-_]+` (`project/lexer.rs:415-417`); a station the file
      // actually contains, so the anchor lands on a real station.
      name: first.shots.length > 0 ? first.shots[0]!.from : `${first.name}0`,
      fix: {
        unit: pick(rng, ['m', 'M', 'f', 'F']),
        east: compassNumber(rng, 200000, 800000, 3),
        north: compassNumber(rng, 0, 9000000, 3),
        elevation: compassNumber(rng, -100, 3000, 3),
      },
    };
    mak.files = mak.files.map((file, index) => (index === 0 ? { ...file, stations: [station] } : file));
  }

  return { mak, dats };
}

// ---------------------------------------------------------------------------
// Walls .SRV
// ---------------------------------------------------------------------------

/**
 * `#Units Order=` values and the column count each one obliges every shot to
 * write. Keeping the two in sync is the single easiest way to generate garbage.
 *
 * `parse_vector_line` (`walls/srv.rs:258-286`) consumes `tokens[2..]` in
 * `units.active_order()` order and simply BREAKS when it runs out - too few
 * columns is not an error, it is a silently shorter shot.
 *
 * `Order=` is routed to `ct_order` or `rect_order` by whether its letters are
 * `E/N/U` (`walls/units.rs:149-169`) but it does NOT switch the vector type, and
 * `active_order()` returns `ct_order` while the type is CT (`walls/mod.rs:323-328`).
 * So `Order=ENU` alone leaves the parser reading the DEFAULT `D,A,V` and
 * silently interpreting an easting as a tape length - the `rect` key must be
 * written too, which is why it is a field here and not a comment. A rect vector
 * additionally needs a non-zero horizontal delta or `parse_vector_line` returns
 * None (`walls/srv.rs:303-306`).
 *
 * The `DA` + `Tape=SS` row is dive mode, which is INFERRED and not declared:
 * CT vectors, station-to-station taping, and an order with D and A but no V
 * (`walls/mod.rs:333-340`). It appends two depth columns AFTER the order's own,
 * so its shots carry four.
 */
interface WallsOrderSpec {
  order: string;
  /** Measurement columns each shot must write, depth columns included. */
  columns: number;
  rect?: boolean;
  tape?: string;
}

const WALLS_ORDERS: readonly WallsOrderSpec[] = [
  { order: 'DAV', columns: 3 },
  { order: 'DAV', columns: 3 },
  { order: 'ADV', columns: 3 },
  { order: 'DVA', columns: 3 },
  { order: 'ENU', columns: 3, rect: true },
  { order: 'NEU', columns: 3, rect: true },
  { order: 'DA', columns: 4, tape: 'SS' },
];

/** One measurement column, in the position `order[index]` names. */
function wallsColumn(rng: Rng, element: string, spec: WallsOrderSpec, index: number): string {
  if (spec.tape !== undefined && index >= spec.order.length) {
    // Dive depth column: scaled by the S unit, and only a real depth CHANGE
    // makes a zero-tape dive leg importable (`walls/convert.rs:99-113`).
    return (index === spec.order.length ? rng() * 5 : 6 + rng() * 30).toFixed(1);
  }
  switch (element) {
    case 'D':
      // Never <= 0: a zero-length leg with no depth and no bearing is dropped
      // outright (`walls/srv.rs:396-407`).
      return (0.5 + rng() * 40).toFixed(2);
    case 'A':
      // `parse_fsbs` splits on `/` into foresight and backsight
      // (`walls/srv.rs:718-741`); an omitted side is legal and common.
      return chance(rng, 0.25)
        ? `${(rng() * 359.9).toFixed(2)}/${(rng() * 359.9).toFixed(2)}`
        : (rng() * 359.9).toFixed(2);
    case 'V':
      return chance(rng, 0.2)
        ? `${(rng() * 60 - 30).toFixed(2)}/${(rng() * 60 - 30).toFixed(2)}`
        : (rng() * 160 - 80).toFixed(2);
    // A rectangular vector's length is sqrt(E^2 + N^2 + U^2) and a zero-length
    // one is skipped outright (`walls/srv.rs:303-306`), so neither horizontal
    // column ever draws near zero.
    case 'E':
    case 'N': {
      const magnitude = 1 + rng() * 30;
      return (chance(rng, 0.5) ? -magnitude : magnitude).toFixed(2);
    }
    default:
      return (rng() * 20 - 10).toFixed(2);
  }
}

function wallsShot(rng: Rng, from: string, to: string, spec: WallsOrderSpec): WallsShotSpec {
  const columns: string[] = [];
  for (let index = 0; index < spec.columns; index++) {
    columns.push(wallsColumn(rng, spec.order[index] ?? '', spec, index));
  }
  const shot: WallsShotSpec = { from, to, columns };
  // An LRUD span must be CLOSED at end-of-body or immediately before whitespace
  // (`find_span_end`, `walls/srv.rs:461-464`); the writer guarantees that, and
  // every value is positive because `parse_lrud` drops a negative one.
  if (chance(rng, 0.35)) {
    shot.lrud = [
      (rng() * 4).toFixed(1),
      (rng() * 4).toFixed(1),
      (rng() * 4).toFixed(1),
      (rng() * 4).toFixed(1),
    ];
    if (chance(rng, 0.3)) shot.lrudStar = true;
  }
  if (chance(rng, 0.25)) shot.comment = maybeHostileLine(rng, `note ${randInt(rng, 0, 999)}`);
  return shot;
}

/** Directives `handle_directive` (`walls/srv.rs:217-219`) does not know and ignores. */
const WALLS_UNKNOWN_DIRECTIVES: readonly string[] = [
  '#Frobnicate whatever',
  '#Note A1 a note',
  '#Flag A1 /CAVE',
  '#Symbol 3',
];

/**
 * A Walls `.srv` spec: one file, one survey, 1..10 shots.
 *
 * TWO SHAPES THE DIALOG NEEDS. `is_importable_shot` (`walls/convert.rs:99-113`)
 * is the single source of truth for what the preview counts, and the Walls
 * dialog gates its confirm button on "nothing to import". The two ways to reach
 * that gate are a SPLAY-ONLY survey (every leg has a `-`/`--` endpoint, so
 * `shot_count` is 0 and `splay_count` is not) and a FIXED-STATION-ONLY survey
 * (no vector lines at all), and both are drawn here on purpose.
 */
export function randomWallsSrvSpec(rng: Rng, profile: SpecProfile = 'valid'): WallsSrvSpec {
  const malformed = profile === 'malformed';
  const order = pick(rng, WALLS_ORDERS);
  const roll = rng();
  const splayOnly = roll < 0.1;
  const fixedOnly = roll >= 0.1 && roll < 0.18;

  const prefix = pick(rng, ['A', 'B', 'C', 'D']);
  const shotCount = fixedOnly ? 0 : randInt(rng, 1, 10);
  const shots: WallsShotSpec[] = [];
  for (let index = 0; index < shotCount; index++) {
    const from = `${prefix}${index}`;
    // `is_anonymous` is EXACTLY `-` or `--` (`walls/units.rs:441-443`); `---`
    // and longer runs are real station names, which is why the splay draw does
    // not simply repeat a hyphen.
    const to = splayOnly || chance(rng, 0.15) ? pick(rng, ['-', '--']) : `${prefix}${index + 1}`;
    shots.push(wallsShot(rng, from, to, order));
  }

  let units = pick(rng, ['meters', 'meters', 'feet']);
  if (order.rect === true) units += ' rect';
  units += ` Order=${order.order}`;
  if (order.tape !== undefined) units += ` Tape=${order.tape}`;
  if (chance(rng, 0.4)) units += ` Decl=${(rng() * 20 - 10).toFixed(2)}`;
  if (malformed && chance(rng, 0.3)) units += ` ${pick(rng, ['Order=DAX', 'Tape=ZZ', 'Decl=oops'])}`;

  const spec: WallsSrvSpec = { units, shots };

  // `extract_team` scrapes a `;` comment line for `team:`, `surveyors:`,
  // `surveyor:` or `survey:` case-insensitively (`walls/srv.rs:153-164`); that
  // scrape is the only place a bare `.srv` gets a surveyor from.
  const header: string[] = [` fuzz survey ${randInt(rng, 100, 999)}`];
  if (chance(rng, 0.5)) header.push(` Team: ${maybeHostileLine(rng, 'A Surveyor, B Notes')}`);
  spec.header = header;

  if (chance(rng, 0.6)) {
    spec.date = `${randInt(rng, 1980, 2030)}-${String(randInt(rng, 1, 12)).padStart(2, '0')}-${String(
      randInt(rng, 1, 28),
    ).padStart(2, '0')}`;
  }

  const fixCount = fixedOnly ? randInt(rng, 1, 2) : randInt(rng, 0, 2);
  if (fixCount > 0) {
    const fixes: WallsFixSpec[] = [];
    for (let index = 0; index < fixCount; index++) {
      // Coordinate columns are consumed in `units.rect_order` (default E, N, U)
      // and NOT in the vector order (`walls/srv.rs:628`), so a dive survey's
      // `Order=DA` cannot truncate the elevation column. A geographic column is
      // placed by its `N`/`S`/`E`/`W` prefix letter rather than by its position,
      // and it is what drives the fixed-station / georeference branch.
      const geographic = chance(rng, 0.5);
      const coords = geographic
        ? [
            `${pick(rng, ['W', 'E'])}${randInt(rng, 0, 179)}:${randInt(rng, 0, 59)}:${(rng() * 59).toFixed(1)}`,
            `${pick(rng, ['N', 'S'])}${randInt(rng, 0, 89)}:${randInt(rng, 0, 59)}:${(rng() * 59).toFixed(1)}`,
            (rng() * 2000 - 100).toFixed(1),
          ]
        : [
            (200000 + rng() * 600000).toFixed(2),
            (rng() * 9000000).toFixed(2),
            (rng() * 2000 - 100).toFixed(1),
          ];
      const fix: WallsFixSpec = { station: `${prefix}${index}`, coords };
      if (chance(rng, 0.3)) fix.comment = maybeHostileLine(rng, 'datum tie');
      fixes.push(fix);
    }
    spec.fixes = fixes;
  }

  if (chance(rng, 0.3)) spec.directives = [pick(rng, WALLS_UNKNOWN_DIRECTIVES)];
  // `#[` raises a depth counter that skips every line until `#]`
  // (`walls/srv.rs:56-66`); the writer emits it before the shots, so a
  // TERMINATED block is inert and only the corruption catalog's unterminated
  // one swallows the survey.
  if (chance(rng, 0.25)) spec.blockComment = [`${prefix}9 ${prefix}8 99 99 99`, '; ignored'];

  return spec;
}

// ---------------------------------------------------------------------------
// Walls .WPJ
// ---------------------------------------------------------------------------

/** `.REF` datums `convert.rs:55-89` recognizes, plus one it does not. */
const WALLS_REF_DATUMS: readonly string[] = ['WGS84', 'NAD83', 'NAD27', 'WGS72', 'Unknown Datum'];

/**
 * A `.REF` argument: 12 finite numbers then a quoted datum.
 *
 * `parse_ref` (`walls/wpj.rs:417-460`) pulls the quoted datum off the tail with
 * `rfind('"')`, splits the head on whitespace, and requires at least 12 finite
 * fields. Field 2 is the UTM zone and must be integral with `|zone|` in 1..=60;
 * field 5 is a flags bitfield and must be integral in 0..=65535. Anything else
 * is one warning and no georeference at all.
 */
function wallsRefText(rng: Rng): string {
  const fields = [
    (rng() * 9000000).toFixed(3), // northing
    (200000 + rng() * 600000).toFixed(3), // easting
    String(randInt(rng, 1, 60)), // zone
    (rng() * 4 - 2).toFixed(3), // convergence
    (rng() * 2000).toFixed(1), // elevation
    String(randInt(rng, 0, 15)), // flags
    String(randInt(rng, 0, 89)), // lat d
    String(randInt(rng, 0, 59)), // lat m
    (rng() * 59).toFixed(3), // lat s
    String(randInt(rng, 0, 179)), // lon d
    String(randInt(rng, 0, 59)), // lon m
    (rng() * 59).toFixed(3), // lon s
  ];
  return `${fields.join(' ')} ${randInt(rng, 0, 25)} "${pick(rng, WALLS_REF_DATUMS)}"`;
}

/**
 * `.STATUS` values that keep a `.SURVEY` countable.
 *
 * Bit 1 (value 2, DETACHED) makes the entry vanish from compilation entirely and
 * bit 16 (value 65536, TYPE_OTHER) turns it into a skipped attachment
 * (`walls/wpj.rs:135-138`), so neither may appear in the valid profile: both
 * would make `selfCheckClaim`'s resolved-survey count a lie. The remaining bits
 * only steer segments, declination and grid-relativity.
 */
const WALLS_SAFE_STATUS: readonly number[] = [0, 8, 8 + 64, 8 + 512, 8 + 2048, 256, 1024];

/**
 * A Walls project: one `.wpj` plus the `.srv` files its `.SURVEY` entries name.
 *
 * `.NAME` is a BARE filename that gets `.SRV` and then `.srv` appended
 * (`resolve_survey`, `walls/wpj.rs:351-382`), and one containing `/`, `\` or `:`
 * is rejected as a traversal attempt and reported missing. So each `.NAME` here
 * is a sibling's content-addressed basename with its extension stripped, and the
 * sibling is written under the lowercase `.srv` spelling the second candidate
 * finds.
 *
 * Sometimes exactly ONE `.SURVEY` names a sibling that is not written, because
 * `missing_file_count` and the unresolved-survey rendering are real states worth
 * reaching - but never all of them, or the file has nothing to import and the
 * generator has produced a rejection case while claiming to produce a valid one.
 *
 * There is deliberately NO `.PATH`: siblings are written flat into one directory
 * by `materializeSurveyFile`, so any `.PATH` at all makes every child unresolved.
 *
 * MULTI-COMPONENT COVERAGE. Half the time the sibling `.srv` files are given
 * disjoint station namespaces, which lands as one component per file and raises
 * the orphan-placement dialog; otherwise they share a station and land as one.
 */
export function randomWallsProjectSpec(
  rng: Rng,
  profile: SpecProfile = 'valid',
): { wpj: WallsWpjSpec; srvs: SiblingSrvSpec[] } {
  const surveyCount = randInt(rng, 1, 3);
  const disjoint = chance(rng, 0.5);
  const srvs: SiblingSrvSpec[] = [];
  const leaves: WallsWpjEntrySpec[] = [];

  // One unresolvable entry at most, and never the only one.
  const missingAt = surveyCount > 1 && chance(rng, 0.25) ? randInt(rng, 0, surveyCount - 1) : -1;

  for (let index = 0; index < surveyCount; index++) {
    const srv = randomWallsSrvSpec(rng, profile);
    if (!disjoint && index > 0 && srv.shots.length > 0 && srvs.length > 0) {
      // Tie this file to the previous one by re-using its first station name,
      // which is the only thing that joins two Walls surveys into one component.
      const anchor = srvs[0]!.srv.shots[0]?.from;
      if (anchor !== undefined) {
        srv.shots = [{ ...srv.shots[0]!, from: anchor }, ...srv.shots.slice(1)];
      }
    }
    const name = surveyFileName('srv', srv);
    const leaf: WallsWpjEntrySpec = {
      kind: 'survey',
      title: `Trip ${index + 1}`,
      name: index === missingAt ? `absent-${randInt(rng, 1000, 9999)}` : name.replace(/\.srv$/, ''),
    };
    if (chance(rng, 0.5)) leaf.status = pick(rng, WALLS_SAFE_STATUS);
    leaves.push(leaf);
    if (index !== missingAt) srvs.push({ name, srv });
  }

  // Sometimes one book deep, so `derive_context` really walks a stack rather
  // than always reading the root (`walls/wpj.rs:113-131`).
  const nested = leaves.length > 1 && chance(rng, 0.35);
  const children: WallsWpjEntrySpec[] = nested
    ? [{ kind: 'book', title: 'Inner Book', children: leaves.slice(1) }, leaves[0]!]
    : leaves;

  const root: WallsWpjEntrySpec = {
    kind: 'book',
    title: `Fuzz Project ${randInt(rng, 100, 999)}`,
    name: `FUZZPROJ${randInt(rng, 10, 99)}`,
    children,
  };
  if (chance(rng, 0.5)) root.status = pick(rng, WALLS_SAFE_STATUS);
  if (chance(rng, 0.5)) root.ref = wallsRefText(rng);
  // `.OPTIONS` is inherited by every child, so it stays to macro definitions:
  // a units option here would fight the `#Units Order=` each `.srv` writes and
  // could silently change how many columns those shots are read as.
  if (chance(rng, 0.25)) root.options = `$cave=FUZZ${randInt(rng, 10, 99)}`;

  return { wpj: { header: ['WALLS Project File'], root }, srvs };
}

// ---------------------------------------------------------------------------
// GPX
// ---------------------------------------------------------------------------

function gpxLatitude(rng: Rng): string {
  if (chance(rng, P_BOUNDARY)) return pick(rng, ['90', '-90', '0', '89.9999999', '-0']);
  return (rng() * 178 - 89).toFixed(6);
}

function gpxLongitude(rng: Rng): string {
  if (chance(rng, P_BOUNDARY)) return pick(rng, ['180', '-180', '0', '179.9999999', '-0']);
  return (rng() * 358 - 179).toFixed(6);
}

/**
 * One `<wpt>`/`<trkpt>`/`<rtept>`.
 *
 * `lat` and `lon` are the only hard requirements and a single out-of-range one
 * aborts the WHOLE file (`gpx.rs:222-259`) - there is no per-point skip - so the
 * valid profile never omits either and never leaves the range. `<ele>` is the
 * opposite: a bad or non-finite one silently becomes `None` (`gpx.rs:388`), so
 * it is drawn absent, negative and at a boundary as well as nominal, because
 * depth is derived from it relative to the FIRST point of each segment.
 */
function gpxPoint(rng: Rng, named: boolean): GpxPointSpec {
  const point: GpxPointSpec = { lat: gpxLatitude(rng), lon: gpxLongitude(rng) };
  if (named && chance(rng, 0.6)) point.name = maybeHostileComment(rng, `pt-${randInt(rng, 0, 9999)}`);
  if (chance(rng, 0.75)) {
    point.ele = chance(rng, P_BOUNDARY)
      ? pick(rng, ['0', '-0', '-500.5', '8848', '-11034'])
      : (rng() * 2000 - 200).toFixed(1);
  }
  if (chance(rng, 0.3)) point.cmt = maybeHostileComment(rng, `comment ${randInt(rng, 0, 999)}`);
  if (chance(rng, 0.2)) {
    point.time = `20${String(randInt(rng, 10, 30)).padStart(2, '0')}-01-15T10:${String(
      randInt(rng, 0, 59),
    ).padStart(2, '0')}:00Z`;
  }
  return point;
}

/**
 * A GPX spec.
 *
 * The three collections are drawn INDEPENDENTLY so that every combination
 * occurs, including "waypoints only", "tracks only" and all three: the GPX
 * selection dialog branches on exactly that, and a waypoints-only file is the
 * one that shows `gpx-import-confirm` instead of `gpx-continue-button`. In the
 * valid profile at least one of the three is non-empty, or the dialog has
 * nothing to offer at all.
 *
 * A one-point track segment is drawn occasionally on purpose: `shots_from_points`
 * skips a segment with fewer than 2 points while its `section_index` is STILL
 * consumed (`gpx.rs:920-923`), so every later placement index shifts. That is a
 * documented real shape, not a corruption.
 */
export function randomGpxSpec(rng: Rng, profile: SpecProfile = 'valid'): GpxSpec {
  const malformed = profile === 'malformed';
  let waypointCount = randInt(rng, 0, 5);
  let trackCount = randInt(rng, 0, 3);
  let routeCount = randInt(rng, 0, 3);
  if (!malformed && waypointCount + trackCount + routeCount === 0) {
    // Re-draw ONE collection rather than all three, so "waypoints only" and
    // "tracks only" stay as likely as they were before the guard.
    const which = randInt(rng, 0, 2);
    if (which === 0) waypointCount = randInt(rng, 1, 5);
    else if (which === 1) trackCount = randInt(rng, 1, 3);
    else routeCount = randInt(rng, 1, 3);
  }

  const spec: GpxSpec = {};
  if (chance(rng, 0.6)) spec.metadataName = maybeHostileComment(rng, `fuzz-gpx-${randInt(rng, 100, 999)}`);

  if (waypointCount > 0) {
    spec.waypoints = Array.from({ length: waypointCount }, () => gpxPoint(rng, true));
  }
  if (trackCount > 0) {
    spec.tracks = Array.from({ length: trackCount }, (): GpxTrackSpec => {
      const segments = Array.from({ length: randInt(rng, 1, 2) }, () => {
        // A trackpoint has no `<name>` child at all (`gpx.rs:479-521`), so
        // naming one would emit an element the reader never looks at.
        const points = chance(rng, 0.12) ? 1 : randInt(rng, 2, 8);
        return Array.from({ length: points }, () => gpxPoint(rng, false));
      });
      const track: GpxTrackSpec = { segments };
      if (chance(rng, 0.7)) track.name = maybeHostileComment(rng, `trk-${randInt(rng, 0, 999)}`);
      if (chance(rng, 0.3)) track.cmt = maybeHostileComment(rng, 'walk-in');
      return track;
    });
  }
  if (routeCount > 0) {
    spec.routes = Array.from({ length: routeCount }, (): GpxRouteSpec => {
      const route: GpxRouteSpec = {
        points: Array.from({ length: randInt(rng, 2, 6) }, () => gpxPoint(rng, true)),
      };
      if (chance(rng, 0.7)) route.name = maybeHostileComment(rng, `rte-${randInt(rng, 0, 999)}`);
      if (chance(rng, 0.3)) route.cmt = maybeHostileComment(rng, 'planned');
      return route;
    });
  }

  // Neither `xmlns` nor `version` is read: the reader scans for a Start element
  // whose LOCAL name is `gpx` (`gpx.rs:137-150`, `strip_ns` at `gpx.rs:105-116`).
  // Both alternatives below are therefore valid, and both are real files.
  if (chance(rng, 0.15)) {
    spec.rootTag = 'g:gpx';
    spec.rootAttrs = 'version="1.1" xmlns:g="http://www.topografix.com/GPX/1/1"';
  } else if (chance(rng, 0.15)) {
    spec.rootAttrs = '';
  }
  if (chance(rng, 0.15)) spec.declaration = null;

  return spec;
}

// ---------------------------------------------------------------------------
// Naming, rendering, materialization
// ---------------------------------------------------------------------------

/**
 * A basename derived from the spec's own content.
 *
 * Content-addressed on purpose: two identical specs get the same name and the
 * same bytes, which is how the fuzzer produces a "the user imported the SAME
 * file twice" case without a separate `sameFileAs` mechanism. Two different
 * specs practically never collide, and a collision would only mean one
 * materialization overwrote an identical-length sibling.
 */
export type SurveyFileSpecBody =
  | DmpSpec
  | TmlSpec
  | CompassDatSpec
  | WallsSrvSpec
  | GpxSpec
  | { mak: CompassMakSpec; dats: readonly SiblingDatSpec[] }
  | { wpj: WallsWpjSpec; srvs: readonly SiblingSrvSpec[] };

export function surveyFileName(format: SurveyFileFormat, spec: SurveyFileSpecBody): string {
  const digest = crc32Text(JSON.stringify(spec)).toString(16).padStart(8, '0');
  return `fuzz-${digest}.${format}`;
}

/**
 * Draw a whole file: a format, a spec for it, and its content-addressed name.
 *
 * The DEFAULT draw is deliberately unchanged from wave 1 (`dmp` twice, `tml`,
 * `tmlu`). The header of `scenarios/importDialogFuzz.ts` carries repro lines
 * whose trajectories depend on the rng draw sequence, and widening the default
 * would silently invalidate every one of them. A caller that wants the new
 * formats names one explicitly - the format axis draws a family from
 * `SURVEY_FORMAT_FAMILIES` and then an extension from `FORMATS_IN_FAMILY`, which
 * is a separate stream and cannot disturb an existing trajectory.
 */
export function randomSurveyFileSpec(
  rng: Rng,
  profile: SpecProfile = 'valid',
  format?: SurveyFileFormat,
): GeneratedFileSpec {
  const chosen = format ?? pick(rng, ['dmp', 'dmp', 'tml', 'tmlu'] as const);
  if (chosen === 'dmp') {
    const dmp = randomDmpSpec(rng, profile);
    return { format: 'dmp', name: surveyFileName('dmp', dmp), profile, dmp };
  }
  if (chosen === 'tml' || chosen === 'tmlu') {
    const tml = randomTmlSpec(rng, profile);
    return { format: chosen, name: surveyFileName(chosen, tml), profile, tml };
  }
  if (chosen === 'dat') {
    const dat = randomCompassDatSpec(rng, profile);
    return { format: 'dat', name: surveyFileName('dat', dat), profile, dat };
  }
  if (chosen === 'mak') {
    const { mak, dats } = randomCompassProjectSpec(rng, profile);
    return { format: 'mak', name: surveyFileName('mak', { mak, dats }), profile, mak, dats };
  }
  if (chosen === 'srv') {
    const srv = randomWallsSrvSpec(rng, profile);
    return { format: 'srv', name: surveyFileName('srv', srv), profile, srv };
  }
  if (chosen === 'wpj') {
    const { wpj, srvs } = randomWallsProjectSpec(rng, profile);
    return { format: 'wpj', name: surveyFileName('wpj', { wpj, srvs }), profile, wpj, srvs };
  }
  const gpx = randomGpxSpec(rng, profile);
  return { format: 'gpx', name: surveyFileName('gpx', gpx), profile, gpx };
}

/**
 * Text bytes for a format read as Latin-1: `.dat`, `.srv`, `.wpj`.
 *
 * `read_latin1` (`parsers/encoding.rs:94-97`) maps each BYTE to the code point of
 * the same value, so a code point in 0..255 must arrive as exactly that byte.
 * That is the only way a corruption can write a UTF-16 byte-order mark (`FF FE`),
 * a `\x1a` DOS EOF or a raw `\x0c` survey terminator, and `TextEncoder` cannot:
 * it would spell `U+00FF` as the two bytes `C3 BF`.
 *
 * A code point ABOVE 255 has no Latin-1 byte at all, so it is written as UTF-8 -
 * which is what a real UTF-8-authored `.dat` contains and what the parser reads
 * back as mojibake. That is the honest outcome for a hostile comment carrying a
 * bidi override or a replacement character, and it never affects framing: in all
 * three formats a comment is free text to end of line.
 */
function encodeLatin1First(text: string): Uint8Array {
  const encoder = new TextEncoder();
  const out: number[] = [];
  for (const character of text) {
    const code = character.codePointAt(0) ?? 0;
    if (code <= 0xff) out.push(code);
    else for (const byte of encoder.encode(character)) out.push(byte);
  }
  return Uint8Array.from(out);
}

/** One file a spec becomes on disk. */
export interface RenderedFile {
  name: string;
  bytes: Uint8Array;
}

/**
 * Every file a spec becomes. Entry 0 is the one the app is pointed at.
 *
 * `.mak` and `.wpj` are projects, so they render more than one file - see the
 * module header. `.mak` and `.gpx` are encoded as UTF-8 because their readers
 * demand it (`Project::read` is `fs::read_to_string`, `project/mod.rs:239`;
 * `read_utf8` at `encoding.rs:100-104`); `.dat`, `.srv` and `.wpj` go through
 * `encodeLatin1First` because their reader is `read_latin1`.
 */
export function renderSurveyFiles(spec: GeneratedFileSpec): [RenderedFile, ...RenderedFile[]] {
  const utf8 = (text: string): Uint8Array => new TextEncoder().encode(text);
  switch (spec.format) {
    case 'dmp':
      return [{ name: spec.name, bytes: writeDmpBytes(spec.dmp) }];
    case 'tml':
      return [{ name: spec.name, bytes: writeTmlZip(spec.tml) }];
    case 'tmlu':
      return [{ name: spec.name, bytes: utf8(writeTmluXml(spec.tml)) }];
    case 'dat':
      return [{ name: spec.name, bytes: encodeLatin1First(writeCompassDat(spec.dat)) }];
    case 'mak':
      return [
        { name: spec.name, bytes: utf8(writeCompassMak(spec.mak)) },
        ...spec.dats.map((sibling) => ({
          name: sibling.name,
          bytes: encodeLatin1First(writeCompassDat(sibling.dat)),
        })),
      ];
    case 'srv':
      return [{ name: spec.name, bytes: encodeLatin1First(writeWallsSrv(spec.srv)) }];
    case 'wpj':
      return [
        { name: spec.name, bytes: encodeLatin1First(writeWallsWpj(spec.wpj)) },
        ...spec.srvs.map((sibling) => ({
          name: sibling.name,
          bytes: encodeLatin1First(writeWallsSrv(sibling.srv)),
        })),
      ];
    default:
      return [{ name: spec.name, bytes: utf8(writeGpxXml(spec.gpx)) }];
  }
}

/** The bytes of the PRIMARY file. Pure - `materializeSurveyFile` writes them. */
export function renderSurveyFile(spec: GeneratedFileSpec): Uint8Array {
  return renderSurveyFiles(spec)[0].bytes;
}

/**
 * What a VALID spec claims the real parse command will report.
 *
 * The one oracle that catches writer/parser drift before it costs a whole run.
 * Every format here fails silently on a framing mistake, so a drifted writer
 * does not produce a red test - it produces a fuzz run that exercises nothing
 * but the error dialog and passes green. Pushing a handful of valid-biased specs
 * through the REAL command and checking this claim is what makes that failure
 * loud.
 */
export interface SelfCheckClaim {
  /** The Tauri command that parses this format. */
  command: string;
  /** The unit being counted, for the failure message ("survey", "waypoint"). */
  what: string;
  /** Pull the count out of the command's return value. */
  read(parsed: unknown): number;
  min: number;
  /** null = unbounded above. */
  max: number | null;
}

/** A record field, or -1 when the shape is not what the claim expects. */
function fieldOf(parsed: unknown, key: string): unknown {
  if (typeof parsed !== 'object' || parsed === null) return undefined;
  return (parsed as Record<string, unknown>)[key];
}

function readArrayLength(parsed: unknown, key: string): number {
  const value = fieldOf(parsed, key);
  return Array.isArray(value) ? value.length : -1;
}

function readCount(parsed: unknown, key: string): number {
  const value = fieldOf(parsed, key);
  return typeof value === 'number' && Number.isFinite(value) ? value : -1;
}

export function selfCheckClaim(spec: GeneratedFileSpec): SelfCheckClaim {
  switch (spec.format) {
    case 'dmp': {
      const declared = spec.dmp.sections.length;
      return {
        command: 'import_dmp',
        what: 'section',
        read: (parsed) => readArrayLength(parsed, 'sections'),
        min: declared,
        max: declared,
      };
    }
    case 'tml':
    case 'tmlu':
      // NOT the spec's section-NAME count. `parse_tml` reconstructs topology
      // from the `FromID` graph and a `TmlSection` is a connected run of shots,
      // so one `<Section>` name can become several sections and several names
      // can collapse into one. The only honest claim is "at least one".
      //
      // And only when there IS one to make: shot 0 is the `START`, which is a
      // station rather than a leg, so a one-shot file is a well-formed document
      // describing an empty survey and `parse_tml` correctly reports zero
      // sections for it. Claiming 1 there would report writer drift for a file
      // the writer got exactly right - measured, on the second `.tml` the
      // seed-1 self-check draws.
      return {
        command: 'parse_tml',
        what: 'section',
        read: (parsed) => readArrayLength(parsed, 'sections'),
        min: spec.tml.shots.some((shot) => shot.type === 'REAL') ? 1 : 0,
        max: null,
      };
    case 'dat': {
      const declared = spec.dat.surveys.length;
      return {
        command: 'parse_compass',
        what: 'survey',
        read: (parsed) => readCount(parsed, 'survey_count'),
        min: declared,
        max: declared,
      };
    }
    case 'mak': {
      // `parse_mak` loads every referenced `.dat` and reports one count over all
      // of them (`compass.rs:211-305`), so the claim sums the siblings.
      const declared = spec.dats.reduce((total, sibling) => total + sibling.dat.surveys.length, 0);
      return {
        command: 'parse_compass',
        what: 'survey',
        read: (parsed) => readCount(parsed, 'survey_count'),
        min: declared,
        max: declared,
      };
    }
    case 'srv':
      // `parse_single_srv` (`walls/wpj.rs:14-45`) wraps the file in a one-survey
      // project unconditionally - even a file with no importable shot at all.
      return {
        command: 'parse_walls',
        what: 'survey',
        read: (parsed) => readCount(parsed, 'survey_count'),
        min: 1,
        max: 1,
      };
    case 'wpj': {
      // `resolved_survey_count` counts the `.SURVEY` leaves whose file was found
      // and read (`walls/convert.rs:134`), which is exactly the leaves whose
      // `.NAME` names a sibling this spec also writes.
      const written = new Set(spec.srvs.map((sibling) => sibling.name.toLowerCase()));
      const declared = countResolvableSurveys(spec.wpj.root, written);
      return {
        command: 'parse_walls',
        what: 'resolved survey',
        read: (parsed) => readCount(parsed, 'resolved_survey_count'),
        min: declared,
        max: declared,
      };
    }
    default: {
      // Section indices are allocated waypoints first, then track segments, then
      // routes (`gpx.rs:898-985`); the dialog lists one row per waypoint, track
      // and route, which is what this counts.
      const declared =
        (spec.gpx.waypoints?.length ?? 0) +
        (spec.gpx.tracks?.length ?? 0) +
        (spec.gpx.routes?.length ?? 0);
      return {
        command: 'parse_gpx',
        what: 'waypoint, track and route',
        read: (parsed) => {
          const waypoints = readArrayLength(parsed, 'waypoints');
          const tracks = readArrayLength(parsed, 'tracks');
          const routes = readArrayLength(parsed, 'routes');
          if (waypoints < 0 || tracks < 0 || routes < 0) return -1;
          return waypoints + tracks + routes;
        },
        min: declared,
        max: declared,
      };
    }
  }
}

/**
 * `.SURVEY` leaves whose `.NAME` resolves to a sibling in `written`.
 *
 * Mirrors `resolve_survey` (`walls/wpj.rs:351-382`): a name is rejected outright
 * when it is empty or carries `/`, `\` or `:`; otherwise `<name>.SRV` then
 * `<name>.srv` are tried, and a name that already ends in `.srv` (case
 * insensitively) is used verbatim. Books are not surveys and are not counted.
 */
function countResolvableSurveys(entry: WallsWpjEntrySpec, written: ReadonlySet<string>): number {
  if (entry.kind === 'book') {
    return (entry.children ?? []).reduce(
      (total, child) => total + countResolvableSurveys(child, written),
      0,
    );
  }
  const name = (entry.name ?? '').trim();
  if (name === '' || /[/\\:]/.test(name)) return 0;
  const candidate = name.toLowerCase().endsWith('.srv') ? name : `${name}.srv`;
  return written.has(candidate.toLowerCase()) ? 1 : 0;
}

/**
 * Write a generated file - and every sibling it needs - into a caller-supplied
 * directory, returning the PRIMARY file's path and bytes.
 *
 * More than one file for a `.mak` or a `.wpj`, because neither is self
 * contained. A Compass project names its sibling `.dat` files by relative path
 * and a missing one is a hard error, not a warning
 * (`Survey file not found: ...`, `parsers/compass.rs:200-202`); a Walls project
 * is a tree of `.SURVEY` entries whose `.NAME` resolves to `<name>.SRV` beside
 * the project (`walls/wpj.rs:351-382`). Writing only the project file would make
 * every generated Compass project fail at the door and every generated Walls
 * project import zero surveys - both silently, both indistinguishable from a
 * generator bug.
 *
 * `dir` MUST be a session-owned path - a subdirectory of the run's artifact
 * directory (`<sessionDir>/fuzz-files/`) or an OS temp dir. Nothing here writes
 * to `e2e/fixtures/`, and nothing here is ever committed. Two independent
 * mechanisms keep it that way, and both are load-bearing because the repo
 * forbids fixture files outright:
 *
 *   1. `.gitignore` ignores `*.dmp`, `*.tml`, `*.tmlu` (and the other survey
 *      extensions) REPO-WIDE, so a generated file is untracked wherever it
 *      lands - including inside the repo tree.
 *   2. Session artifact directories live under `.agent-artifacts/`, which
 *      `.gitignore` also ignores wholesale, so the containing directory is
 *      invisible to git even before the extension rule applies.
 *
 * Every name is a basename by construction; ALL of them are re-checked here, the
 * siblings included, so neither a hand-built spec nor a corruption that rewrote
 * a sibling name can make this write outside `dir`.
 */
export function materializeSurveyFile(
  dir: string,
  spec: GeneratedFileSpec,
): { path: string; bytes: Uint8Array } {
  const files = renderSurveyFiles(spec);
  for (const file of files) {
    if (file.name !== path.basename(file.name) || file.name === '' || file.name.startsWith('.')) {
      // A spec can come back from a trace, which a forged copy writes, so the
      // name prints as its length (#576 W51).
      throw new Error(
        `generated file name must be a plain basename, got one of ${file.name.length} character(s)`,
      );
    }
  }
  fs.mkdirSync(dir, { recursive: true });
  let primary: { path: string; bytes: Uint8Array } | null = null;
  for (const file of files) {
    const target = path.join(dir, file.name);
    fs.writeFileSync(target, file.bytes);
    primary ??= { path: target, bytes: file.bytes };
  }
  if (primary === null) throw new Error('a generated spec rendered to no files at all');
  return primary;
}
