/**
 * The corruption catalogs for the five text formats: Compass `.dat`/`.mak`,
 * Walls `.srv`/`.wpj`, and GPX.
 *
 * Wave 2 of `surveyFileCorrupt.ts` (mNemo `.dmp` + Ariane `.tml`/`.tmlu`), same
 * `{ id, applies?, apply }` shape, same reason for existing: a random bit flip
 * in a whitespace-separated number file almost always lands on a digit and
 * produces "a slightly different survey", which is not a rejection path at all.
 * Every entry below names a SPECIFIC parser mechanism and says which one.
 *
 * ## Loud and quiet corruptions are both in here, on purpose
 *
 * These three parsers are much quieter than the two in wave 1. A Compass row
 * with one column too few makes `many0(parse_shot)` stop, which drops that row
 * AND every row after it in the survey with no warning at all; a `FORMAT:`
 * missing its trailing space silently reinterprets the whole survey as feet and
 * degrees; a Walls vector line with fewer than two tokens is skipped in silence;
 * a GPX `<trkseg>` with one point is dropped while still consuming a section
 * index. Those are exactly the entries worth having - a corruption that produces
 * a clean error dialog only tests the error dialog, while a corruption that
 * produces a plausible-looking import with half the data missing tests whether
 * the receipt tells the user the truth.
 *
 * ## Corruption is a pure spec transform, and the whole file SET travels
 *
 * `apply` returns a NEW spec; nothing is mutated in place. For the two project
 * formats it returns the project AND its siblings, because half of what can go
 * wrong with a `.mak` or a `.wpj` is about the sibling list rather than the
 * project text - a `.dat` that is referenced and not written is a hard error, a
 * `.srv` that is referenced and not written is a silently unresolved survey.
 */
import {
  type Rng,
  type SiblingDatSpec,
  type SiblingSrvSpec,
  chance,
  pick,
  randInt,
} from './surveyFileSpecs';
import {
  type CompassDatSpec,
  type CompassMakSpec,
  type CompassShotSpec,
  type CompassSurveySpec,
  type GpxPointSpec,
  type GpxSpec,
  type GpxTrackSpec,
  type WallsShotSpec,
  type WallsSrvSpec,
  type WallsWpjEntrySpec,
  type WallsWpjSpec,
  writeCompassDat,
  writeGpxXml,
  writeWallsWpj,
} from './surveyFileWritersText';

/** The two multi-file specs, as the catalogs see them. */
export interface CompassProjectSpec {
  mak: CompassMakSpec;
  dats: readonly SiblingDatSpec[];
}

export interface WallsProjectSpec {
  wpj: WallsWpjSpec;
  srvs: readonly SiblingSrvSpec[];
}

export interface Corruption<T> {
  id: string;
  applies?: (spec: T) => boolean;
  apply: (rng: Rng, spec: T) => T;
}

// ---------------------------------------------------------------------------
// Compass .DAT
// ---------------------------------------------------------------------------

function withRandomSurvey(
  rng: Rng,
  spec: CompassDatSpec,
  change: Partial<CompassSurveySpec>,
): CompassDatSpec {
  if (spec.surveys.length === 0) return spec;
  const index = randInt(rng, 0, spec.surveys.length - 1);
  return {
    ...spec,
    surveys: spec.surveys.map((survey, at) => (at === index ? { ...survey, ...change } : survey)),
  };
}

/**
 * Change one shot of one survey that HAS a shot.
 *
 * Only surveys with a shot are candidates, so the returned spec really carries
 * the corruption its id names rather than silently being the clean one.
 */
function withRandomDatShot(
  rng: Rng,
  spec: CompassDatSpec,
  change: (shot: CompassShotSpec) => Partial<CompassShotSpec>,
): CompassDatSpec {
  const candidates = spec.surveys
    .map((survey, index) => ({ survey, index }))
    .filter((entry) => entry.survey.shots.length > 0);
  if (candidates.length === 0) return spec;
  const chosen = pick(rng, candidates);
  const shotIndex = randInt(rng, 0, chosen.survey.shots.length - 1);
  return {
    ...spec,
    surveys: spec.surveys.map((survey, at) =>
      at === chosen.index
        ? {
            ...survey,
            shots: survey.shots.map((shot, position) =>
              position === shotIndex ? { ...shot, ...change(shot) } : shot,
            ),
          }
        : survey,
    ),
  };
}

const DAT_CORRUPTIONS: readonly Corruption<CompassDatSpec>[] = [
  {
    // Every header keyword is a `nom::tag`: exact, case-sensitive and anchored
    // at column 0 of its line (`survey/parser/mod.rs:29, 37, 55`,
    // `survey_parameters.rs:233`). Lowercasing ONE occurrence kills that survey,
    // and `many0` then returns only the surveys before it - so lowercasing the
    // first survey's keyword yields an empty preview with no error at all.
    id: 'lowercase-header-keyword',
    applies: (spec) => spec.rawText === undefined && spec.surveys.length > 0,
    apply: (rng, spec) => {
      const keyword = pick(rng, ['SURVEY NAME:', 'SURVEY DATE:', 'SURVEY TEAM:', 'DECLINATION:']);
      return { ...spec, rawText: writeCompassDat(spec).replace(keyword, keyword.toLowerCase()) };
    },
  },
  {
    // `tag("SURVEY TEAM:")` is followed by `multispace1`
    // (`survey/parser/mod.rs:55`), and nom's `multispace1` is greedy ACROSS
    // newlines: a blank team line is swallowed, the `DECLINATION:` line is read
    // as the team, and the parameter line then fails. 0 surveys.
    id: 'blank-team-line',
    apply: (rng, spec) => withRandomSurvey(rng, spec, { team: '' }),
  },
  {
    // A FORMAT string's length must be exactly 11, 12, 13 or 15
    // (`survey_parameters.rs:129-200`); 10, 14 and 16 are `ErrorKind::LengthValue`
    // and take the whole survey with them, silently.
    id: 'format-illegal-length',
    apply: (rng, spec) =>
      withRandomSurvey(rng, spec, {
        format: pick(rng, ['DDDDLUDRLA', 'DDDDLUDRLADadB', 'DDDDLUDRLADadBFF']),
      }),
  },
  {
    // `tag("FORMAT: ")` REQUIRES the trailing space (`survey_parameters.rs:110`).
    // Without it `parse_survey_format` returns `SurveyFormat::None` without
    // consuming, and `CORRECTIONS:` then fails to match too. The survey still
    // imports - as decimal FEET, degrees, no backsights and no corrections, with
    // NO warning. The one silent unit corruption in the format.
    id: 'format-no-trailing-space',
    applies: (spec) => spec.surveys.some((survey) => (survey.format ?? null) !== null),
    apply: (rng, spec) => withRandomSurvey(rng, spec, { formatNoSpace: true }),
  },
  {
    // A row with one column too few fails `parse_double`, `many0(parse_shot)`
    // stops, and that row AND EVERY ROW AFTER IT in the survey is dropped with
    // no warning (`survey/parser/mod.rs:98-133`). Silent truncation of the shot
    // list is the loudest thing this file can produce and it is not loud at all.
    id: 'short-data-row',
    applies: (spec) => spec.surveys.some((survey) => survey.shots.length > 0),
    apply: (rng, spec) =>
      withRandomDatShot(rng, spec, (shot) => ({
        // Six floats where seven are required (nine where eleven are, under a
        // backsight format): one column short in either direction.
        rawLine: [shot.from, shot.to, '1.00', '2.00', '3.00', '4.00', '5.00', '6.00'].join(' '),
      })),
  },
  {
    // `parse_station_name` is `take_while1` over `[A-Za-z0-9_\-'*]`
    // (`parser_utils.rs:19-21`). A `.`, `,`, `#` or `/` makes the row fail, which
    // truncates the shot list from that row on - same silent mechanism as above,
    // reached through the name rather than the column count.
    id: 'illegal-station-char',
    apply: (rng, spec) =>
      withRandomDatShot(rng, spec, () => ({ from: pick(rng, ['A.1', 'A,1', 'A#1', 'A/1', 'A 1']) })),
  },
  {
    // nom's `number::complete::double` accepts the UNSIGNED literals `nan`,
    // `inf` and `infinity` but not a signed one (`parser_utils.rs:6, 23-25`), so
    // `-inf` fails the row and truncates the shot list - while a bare `nan` in
    // the same column parses and is then treated as missing. Pinned by
    // `compass.rs:1858-1866` and the `nonfinite_signed.dat` fixture.
    id: 'signed-nonfinite-token',
    apply: (rng, spec) =>
      withRandomDatShot(rng, spec, () => ({ length: pick(rng, ['-inf', '-nan', '+nan', '-infinity']) })),
  },
  {
    // `NaiveDate::from_ymd_opt` at `survey/parser/mod.rs:141` used to `unwrap()`
    // here and PANIC the synchronous Tauri command, leaving the import dialog on
    // its spinner forever (issue #479). Since the fix the survey is rejected as
    // an ordinary nom error, so this entry is that fix's live coverage: it must
    // produce a short preview, never a hang.
    id: 'impossible-date',
    apply: (rng, spec) => {
      const [month, day] = pick(rng, [
        [2, 29],
        [2, 30],
        [13, 45],
        [0, 10],
        [7, 0],
        [256, 10],
        [4, 31],
      ]);
      return withRandomSurvey(rng, spec, { month, day, year: 1979 });
    },
  },
  {
    // `parse_uint` is `digits.parse::<u32>()` (`parser_utils.rs:32-36`), the
    // second unwrap #479 removed. A field wider than a u32 is now a nom error.
    id: 'date-field-overflow',
    apply: (rng, spec) =>
      withRandomSurvey(rng, spec, { year: pick(rng, ['99999999999', '4294967296', '-7']) }),
  },
  {
    // `recognize_line` is `not_line_ending` + `line_ending`
    // (`parser_utils.rs:38-42`) and nom's `not_line_ending` REJECTS a `\r` that
    // is not followed by `\n`. An old-Mac file therefore parses to 0 surveys.
    id: 'cr-only-line-endings',
    apply: (_rng, spec) => ({ ...spec, eol: '\r' }),
  },
  {
    // A line only exists if it is TERMINATED, so dropping the final newline
    // silently drops the last shot row of that survey.
    id: 'missing-final-newline',
    applies: (spec) => spec.surveys.length > 0,
    apply: (_rng, spec) => ({
      ...spec,
      surveys: spec.surveys.map((survey, index) =>
        index === spec.surveys.length - 1 ? { ...survey, omitFinalNewline: true } : survey,
      ),
    }),
  },
  {
    // The survey terminator is ONE literal `\x0c` (`survey/parser/mod.rs:147`).
    // Anything else - a blank line, a DOS EOF, junk - becomes the next survey's
    // CAVE NAME line, `SURVEY NAME:` then fails, and every survey from that
    // point on is lost in silence.
    id: 'non-formfeed-separator',
    applies: (spec) => spec.surveys.length > 1,
    apply: (rng, spec) => ({ ...spec, separator: pick(rng, ['\r\n', '\x1a\r\n', '@@@\r\n', '']) }),
  },
  {
    // Only ONE form feed is consumed, because the trailer is
    // `opt(ws(tag("\x0c")))` and nom's `multispace0` does not match 0x0C. The
    // second lands in the next cave name (`"\u{c}Cave Two"`) when glued.
    id: 'double-formfeed',
    applies: (spec) => spec.surveys.length > 1,
    apply: (_rng, spec) => ({ ...spec, separator: '\x0c\x0c' }),
  },
  {
    // `read_latin1` strips a UTF-8 BOM but refuses a UTF-16 one by name
    // (`parsers/encoding.rs:40-48`). These are the raw bytes FF FE / FE FF -
    // `renderSurveyFiles` writes a `.dat` through `encodeLatin1First`, so a code
    // point below 256 lands as exactly that byte.
    id: 'utf16-bom',
    apply: (rng, spec) => ({ ...spec, leading: pick(rng, ['\u00ff\u00fe', '\u00fe\u00ff']) }),
  },
  {
    // The contrast case: a UTF-8 BOM is STRIPPED and the file parses normally
    // (`encoding.rs:96`, test `compass.rs:2149-2168`). Worth a catalog id so a
    // run that starts rejecting it is visible.
    id: 'utf8-bom',
    apply: (_rng, spec) => ({ ...spec, leading: '\ufeff' }),
  },
  {
    // `gobble_labels` is `ws(tag("FROM"))` then `ws(take_till1(...))`
    // (`survey/parser/mod.rs:60-64`): the first non-whitespace token must be
    // exactly `FROM`, and at least one more character must follow. Both halves
    // are broken here, and either kills the survey.
    id: 'bad-column-header',
    apply: (rng, spec) =>
      withRandomSurvey(rng, spec, { columnHeader: pick(rng, ['XX TO LENGTH BEARING', 'FROM', '']) }),
  },
];

// ---------------------------------------------------------------------------
// Compass .MAK
// ---------------------------------------------------------------------------

function withFirstMakFile(
  project: CompassProjectSpec,
  change: Partial<CompassMakSpec['files'][number]>,
): CompassProjectSpec {
  if (project.mak.files.length === 0) return project;
  return {
    ...project,
    mak: {
      ...project.mak,
      files: project.mak.files.map((file, index) => (index === 0 ? { ...file, ...change } : file)),
    },
  };
}

const MAK_CORRUPTIONS: readonly Corruption<CompassProjectSpec>[] = [
  {
    // `to_file_state` (`project/parser.rs:95-103`) needs BOTH a datum and a base
    // location before any `#` entry -> `Survey file encountered before required
    // state was set`. Placing `&` after `#` fails the same way.
    id: 'file-before-state',
    apply: (_rng, project) => ({ ...project, mak: { ...project.mak, filesFirst: true } }),
  },
  {
    // `parse_datum_name` is an exact, case-sensitive match against 23 strings
    // (`project/parser.rs:213-244`); `&wgs 1984;` fails the whole file.
    id: 'unknown-datum',
    apply: (rng, project) => ({
      ...project,
      mak: { ...project.mak, datum: pick(rng, ['wgs 1984', 'WGS84', 'Nonesuch 1999', ' WGS 1984']) },
    }),
  },
  {
    // Every directive ends with `;`; a `#` entry's terminator is the first `;` at
    // bracket depth zero (`project/lexer.rs:552-569`). Dropping it is
    // `MissingSemicolon`, not a truncated path.
    id: 'missing-semicolon',
    applies: (project) => project.mak.files.length > 0,
    apply: (_rng, project) => withFirstMakFile(project, { omitSemicolon: true }),
  },
  {
    // The lexer dispatches on the first non-whitespace character
    // (`project/lexer.rs:179-197`); anything outside `/ @ & $ % * ! # [ ]` is
    // `Unexpected character '{c}'` and kills the file.
    id: 'stray-leading-char',
    apply: (rng, project) => ({
      ...project,
      mak: { ...project.mak, leading: `${pick(rng, ['^', '~', 'x', '\u00a0'])}\r\n` },
    }),
  },
  {
    // The `.mak` lexer has its OWN `parse_double` (`project/lexer.rs:247-272`):
    // sign, digits, optional `.` fraction. No exponent, no `nan`, no bare `.5`.
    // Far stricter than the `.dat` parser's nom `double`, which accepts all three.
    id: 'exponent-number',
    applies: (project) => (project.mak.baseLocation ?? null) !== null,
    apply: (rng, project) => ({
      ...project,
      mak: {
        ...project.mak,
        baseLocation:
          project.mak.baseLocation === null || project.mak.baseLocation === undefined
            ? project.mak.baseLocation
            : { ...project.mak.baseLocation, east: pick(rng, ['1e2', '.5', '1_000', '+.0']) },
      },
    }),
  },
  {
    // `Invalid station fix format` - the same lexer rule reached through the
    // `[unit,e,n,elev]` bracket, which is what `nonfinite.mak` pins
    // (`compass.rs:1801-1811`).
    id: 'nan-station-fix',
    applies: (project) => project.mak.files.length > 0,
    apply: (rng, project) =>
      withFirstMakFile(project, {
        stations: [
          {
            name: 'A0',
            fix: {
              unit: 'm',
              east: pick(rng, ['nan', 'inf', '1e3']),
              north: '2',
              elevation: '3',
            },
          },
        ],
      }),
  },
  {
    // `@east,north,elevation,zone,convergence;` with the zone as a `u8`
    // (`project/lexer.rs:279-306`). 300 overflows it -> `Invalid base location
    // format`; the zone is not range-checked beyond the type, so 99 is accepted.
    id: 'bad-base-location',
    applies: (project) => (project.mak.baseLocation ?? null) !== null,
    apply: (rng, project) => ({
      ...project,
      mak: {
        ...project.mak,
        baseLocation:
          project.mak.baseLocation === null || project.mak.baseLocation === undefined
            ? project.mak.baseLocation
            : { ...project.mak.baseLocation, zone: pick(rng, [300, 1000, -300]) },
      },
    }),
  },
  {
    // `Duplicate project parameter flag` / `Unknown project parameter flag`
    // (`project/parser.rs:246-300`). `!EA;` counts as a duplicate because the
    // three declination modes I/E/A share ONE detection key.
    id: 'duplicate-flag',
    apply: (rng, project) => ({
      ...project,
      mak: { ...project.mak, flags: pick(rng, ['GG', 'EA', 'IE', 'VV', 'Z', 'G1']) },
    }),
  },
  {
    // A referenced `.DAT` that is not on disk is a HARD error from
    // `load_survey_files`: `Failed to load Compass survey files: Survey file not
    // found: ...` (`compass.rs:200-202`). The `#` entry stays; the sibling goes.
    id: 'missing-dat-sibling',
    applies: (project) => project.dats.length > 0,
    apply: (rng, project) => {
      const dropped = randInt(rng, 0, project.dats.length - 1);
      return { ...project, dats: project.dats.filter((_sibling, index) => index !== dropped) };
    },
  },
  {
    // A `[folder;` prefixes the entry's path with a subdirectory
    // (`project/parser.rs:180-184`), and every sibling is written flat next to
    // the `.mak` - so the same `Survey file not found`, reached through path
    // resolution rather than through an absent file.
    id: 'folder-prefix',
    applies: (project) => project.mak.files.length > 0,
    apply: (rng, project) => withFirstMakFile(project, { folder: pick(rng, ['sub', 'a b', '..']) }),
  },
  {
    // Backslashes are NOT normalized (`project/lexer.rs:574-582`), so on Linux a
    // `sub\t.dat` path is a literal filename and the file is not found.
    id: 'backslash-path',
    applies: (project) => project.mak.files.length > 0,
    apply: (_rng, project) =>
      // `applies` gates this on files.length > 0.
      withFirstMakFile(project, { path: `sub\\${project.mak.files[0]!.path}` }),
  },
  {
    // `Project::read` is `fs::read_to_string` - STRICT UTF-8 - and `parse_mak`
    // calls `reject_bom` first (`compass.rs:197`, `encoding.rs:66-77`), so a
    // `.mak` BOM is refused by name where a `.dat` BOM is silently stripped.
    id: 'bom',
    apply: (rng, project) => ({
      ...project,
      mak: { ...project.mak, leading: pick(rng, ['\ufeff', '\u00ff\u00fe']) },
    }),
  },
  {
    // `lex_station_fix` (`project/lexer.rs:419-470`) allows whitespace after `[`
    // and around the unit char and NOWHERE else - not before a comma that
    // follows a number, not before `]`. One trailing space is fatal.
    id: 'station-fix-whitespace',
    applies: (project) => project.mak.files.length > 0,
    apply: (_rng, project) =>
      withFirstMakFile(project, {
        stations: [{ name: 'A0', fix: { unit: 'm', east: '1.5 ', north: '2', elevation: '3' } }],
      }),
  },
];

// ---------------------------------------------------------------------------
// Walls .SRV
// ---------------------------------------------------------------------------

function withRandomSrvShot(
  rng: Rng,
  spec: WallsSrvSpec,
  change: (shot: WallsShotSpec) => Partial<WallsShotSpec>,
): WallsSrvSpec {
  if (spec.shots.length === 0) return spec;
  const index = randInt(rng, 0, spec.shots.length - 1);
  return {
    ...spec,
    shots: spec.shots.map((shot, at) => (at === index ? { ...shot, ...change(shot) } : shot)),
  };
}

/** Append an argument to the `#Units` line, creating the directive if needed. */
function withUnitsArg(spec: WallsSrvSpec, argument: string): WallsSrvSpec {
  const units = spec.units ?? null;
  return { ...spec, units: units === null ? argument : `${units} ${argument}` };
}

const SRV_CORRUPTIONS: readonly Corruption<WallsSrvSpec>[] = [
  {
    // An undefined `$(name)` warns `undefined macro $(...)` and SKIPS THE WHOLE
    // LINE (`walls/srv.rs:81-87, 105-111, 826-857`) - the shot is gone, and the
    // warning is the only trace.
    id: 'undefined-macro',
    applies: (spec) => spec.shots.length > 0,
    apply: (rng, spec) =>
      withRandomSrvShot(rng, spec, (shot) => ({
        rawLine: `${shot.from} ${shot.to} $(${pick(rng, ['nope', 'cave', 'undefined'])}) 90.00 0.00`,
      })),
  },
  {
    // `Unknown #units option '{key}'` (`walls/units.rs:301-303`). Note the
    // contrast with `unknown-directive` below: an unknown UNITS key warns, an
    // unknown `#` DIRECTIVE is silently ignored.
    id: 'unknown-units-option',
    apply: (rng, spec) =>
      withUnitsArg(spec, pick(rng, ['Frobnicate=1', 'Zork', 'Order2=DAV', 'Meter5'])),
  },
  {
    // `Ignoring unparseable #Units Order='{v}'; keeping the previous order`
    // (`walls/units.rs:164-166`). The PREVIOUS order survives, so the shots that
    // follow are read against a column layout nobody declared.
    id: 'bad-order',
    applies: (spec) => (spec.units ?? null) !== null,
    apply: (rng, spec) => ({
      ...spec,
      units: (spec.units ?? '').replace(/Order=\S+/i, `Order=${pick(rng, ['DAX', 'ZZZ', 'DAVQ'])}`),
    }),
  },
  {
    // `Unparseable #Units Decl='{v}'; declination left unchanged`
    // (`walls/units.rs:226-228`). Critically it does NOT fabricate `Decl=0`,
    // which would suppress auto-derivation.
    id: 'bad-decl',
    apply: (rng, spec) => withUnitsArg(spec, `Decl=${pick(rng, ['oops', 'NaN', '1e999', '--3'])}`),
  },
  {
    // `unrecognized #Date '{raw}'` (`walls/srv.rs:196-199`) and the previous
    // valid date is KEPT, so a survey can silently carry a date from a line the
    // author meant to replace.
    id: 'bad-date',
    apply: (rng, spec) => ({
      ...spec,
      date: pick(rng, ['yesterday', '2024-13-45', '99/99/99', 'not-a-date']),
    }),
  },
  {
    // `tokens.len() < 2` -> the line is skipped SILENTLY (`walls/srv.rs:246-248`).
    // This is how a lone-station LRUD line disappears, and how a stray fragment
    // costs a shot with no warning anywhere.
    id: 'one-token-vector-line',
    applies: (spec) => spec.shots.length > 0,
    apply: (rng, spec) => withRandomSrvShot(rng, spec, (shot) => ({ rawLine: shot.from })),
  },
  {
    // `parse_length` and `parse_angle` both check `is_finite()` after
    // `str::parse` (`walls/units.rs:608-610, 676-678`), so `nan`, `inf` and
    // `1e999` all read as an OMITTED column - and the shot may then be dropped
    // by the zero-length rule. Pinned by `walls/tests.rs:1086-1093`.
    id: 'nonfinite-column',
    applies: (spec) => spec.shots.some((shot) => shot.columns.length > 0),
    apply: (rng, spec) => {
      const token = pick(rng, ['nan', 'inf', '1e999', '-inf']);
      return withRandomSrvShot(rng, spec, (shot) => ({
        columns: shot.columns.map((column, index) => (index === 0 ? token : column)),
      }));
    },
  },
  {
    // `find_span_end` (`walls/srv.rs:461-464`) only accepts a closer that sits at
    // end-of-body or immediately before whitespace. Glued to more text, the `<`
    // is a literal station-name character and the whole "LRUD" is swallowed into
    // a measurement token - which is what lets a station named `*8` survive.
    id: 'unterminated-lrud-span',
    applies: (spec) => spec.shots.length > 0,
    apply: (rng, spec) =>
      withRandomSrvShot(rng, spec, (shot) => ({
        rawLine: `${shot.from} ${shot.to} ${shot.columns.join(' ')} ${pick(rng, ['<1,2,3,4>x', '*1,2,3,4*x', '<1,2,3,4'])}`,
      })),
  },
  {
    // `#[` raises a depth counter and `#]` lowers it (`walls/srv.rs:56-63`); an
    // opener that is never closed swallows everything to EOF as a comment, with
    // NO warning either way. The writer emits the block before the shots, so
    // this really does eat the survey.
    id: 'unterminated-block-comment',
    apply: (_rng, spec) => ({ ...spec, unterminatedBlockComment: true, blockComment: [] }),
  },
  {
    // `Ignoring #Units Tape='{v}' (expected two of I/S/T, e.g. Tape=SS)`
    // (`walls/units.rs:377-380`). On a `Order=DA` survey this also silently
    // leaves dive mode off, so the two depth columns become extra tokens.
    id: 'bad-tape',
    apply: (rng, spec) => withUnitsArg(spec, `Tape=${pick(rng, ['ZZ', 'S', 'SSS', 'IX'])}`),
  },
  {
    // `Ignoring #Units LRUD order '{o}' (expected 4 of L, R, U, D); keeping
    // LRUD` (`walls/units.rs:348-352`).
    id: 'bad-lrud-order',
    apply: (rng, spec) => withUnitsArg(spec, `LRUD=${pick(rng, ['F:XYZW', 'Q', 'T:LLLL'])}`),
  },
  {
    // A leg with length <= 0, no depth, no fore- OR backsight bearing and no
    // LRUD is DROPPED (`walls/srv.rs:396-407`, `walls/convert.rs:99-113`). `--`
    // is the omitted-measurement spelling (`walls/units.rs:428-430`); a single
    // `-` is not "omitted" and simply fails to parse.
    id: 'zero-length-equate',
    applies: (spec) => spec.shots.length > 0,
    apply: (rng, spec) =>
      withRandomSrvShot(rng, spec, (shot) => ({
        rawLine: `${shot.from} ${shot.to} 0 -- --`,
        lrud: null,
      })),
  },
  {
    // `read_latin1` again: a UTF-16 BOM is a hard error naming the encoding,
    // pinned by `walls/tests.rs:2438-2450`.
    id: 'utf16-bom',
    apply: (rng, spec) => ({ ...spec, leading: pick(rng, ['\u00ff\u00fe', '\u00fe\u00ff']) }),
  },
  {
    // The contrast case for `unknown-units-option`: `handle_directive`
    // (`walls/srv.rs:217-219`) silently ignores any key it does not know, so an
    // invented directive is a no-op with no warning at all.
    id: 'unknown-directive',
    apply: (rng, spec) => ({
      ...spec,
      directives: [
        ...(spec.directives ?? []),
        pick(rng, ['#Frobnicate x', '#Zork', '#Units2 meters', '#### deep']),
      ],
    }),
  },
];

// ---------------------------------------------------------------------------
// Walls .WPJ
// ---------------------------------------------------------------------------

/** Every `.SURVEY` leaf, in document order. */
function wpjLeaves(entry: WallsWpjEntrySpec): WallsWpjEntrySpec[] {
  if (entry.kind !== 'book') return [entry];
  return (entry.children ?? []).flatMap(wpjLeaves);
}

function mapWpjLeaf(
  entry: WallsWpjEntrySpec,
  target: WallsWpjEntrySpec,
  change: Partial<WallsWpjEntrySpec>,
): WallsWpjEntrySpec {
  if (entry === target) return { ...entry, ...change };
  if (entry.kind !== 'book') return entry;
  return {
    ...entry,
    children: (entry.children ?? []).map((child) => mapWpjLeaf(child, target, change)),
  };
}

function withRandomLeaf(
  rng: Rng,
  project: WallsProjectSpec,
  change: Partial<WallsWpjEntrySpec>,
): WallsProjectSpec {
  const leaves = wpjLeaves(project.wpj.root);
  if (leaves.length === 0) return project;
  const target = pick(rng, leaves);
  return {
    ...project,
    wpj: { ...project.wpj, root: mapWpjLeaf(project.wpj.root, target, change) },
  };
}

/** Replace one whitespace-separated field of a `.REF` argument. */
function withRefField(ref: string, index: number, value: string): string {
  const fields = ref.split(/\s+/);
  if (index >= fields.length) return ref;
  return fields.map((field, at) => (at === index ? value : field)).join(' ');
}

function withRootRef(project: WallsProjectSpec, ref: string): WallsProjectSpec {
  return { ...project, wpj: { ...project.wpj, root: { ...project.wpj.root, ref } } };
}

/** A `.REF` the corruptions can edit even when the clean spec carried none. */
const WPJ_BASELINE_REF =
  '2258942.000 467427.000 16 -0.109 30 6 20 25 42.950 87 18 44.073 19 "NAD83"';

function currentRef(project: WallsProjectSpec): string {
  return project.wpj.root.ref ?? WPJ_BASELINE_REF;
}

const WPJ_CORRUPTIONS: readonly Corruption<WallsProjectSpec>[] = [
  {
    // `resolve_survey` rejects a `.NAME` containing `/`, `\` or `:` as a
    // traversal attempt and reports the survey MISSING rather than reading it
    // (`walls/wpj.rs:362-364`, test `walls/tests.rs:1141-1170`).
    id: 'name-with-separator',
    apply: (rng, project) =>
      withRandomLeaf(rng, project, {
        name: pick(rng, ['../../etc/passwd', 'sub/TRIP', 'C:\\TRIP', 'a\\b']),
      }),
  },
  {
    // An unresolved `.SRV` becomes a placeholder `ParsedSurvey { resolved: false,
    // vectors: [] }` and the path is pushed to `missing_files`
    // (`walls/wpj.rs:186-204`) - no error, just a survey with nothing in it.
    id: 'missing-srv',
    applies: (project) => project.srvs.length > 0,
    apply: (rng, project) => {
      const dropped = randInt(rng, 0, project.srvs.length - 1);
      return { ...project, srvs: project.srvs.filter((_sibling, index) => index !== dropped) };
    },
  },
  {
    // `Unparseable .STATUS '{raw}'; treating as 0 (entry may import
    // unexpectedly)` (`walls/wpj.rs:261-263`) - and the DETACHED / TYPE_OTHER
    // bits are lost with it, which is the "imports unexpectedly" part.
    id: 'bad-status',
    apply: (rng, project) =>
      withRandomLeaf(rng, project, { status: pick(rng, ['eight', '', '1.5', '-2', '1e9']) }),
  },
  {
    // `parse_ref` requires at least 12 finite numeric fields
    // (`walls/wpj.rs:431-449`) -> `Ignoring malformed .REF: expected >=12 finite
    // numeric fields, got {n}` and the entry inherits NO georeference.
    id: 'short-ref',
    apply: (rng, project) => withRootRef(project, pick(rng, ['1 2 3 "NAD83"', '"NAD83"', '1 2'])),
  },
  {
    // A `.REF` field of `1e400` parses to `+inf` and is rejected by the same
    // finiteness check (test `walls/tests.rs:2345-2351`).
    id: 'nonfinite-ref',
    apply: (rng, project) =>
      withRootRef(project, withRefField(currentRef(project), 0, pick(rng, ['1e400', 'nan', 'inf']))),
  },
  {
    // Field 2 is the UTM zone and must be integral with `|zone|` in 1..=60, else
    // `invalid UTM zone '{z}'` (`walls/wpj.rs:441-449`).
    id: 'bad-zone-ref',
    apply: (rng, project) =>
      withRootRef(project, withRefField(currentRef(project), 2, pick(rng, ['99', '0', '16.5', '-61']))),
  },
  {
    // Field 5 is a flags bitfield and must be integral in 0..=65535, else
    // `invalid .REF flags field '{f}'`.
    id: 'bad-flags-ref',
    apply: (rng, project) =>
      withRootRef(project, withRefField(currentRef(project), 5, pick(rng, ['99999', '-1', '6.5']))),
  },
  {
    // `resolve_survey` requires a non-empty `.NAME` (`walls/wpj.rs:352-356`); a
    // leaf without one is unresolvable no matter what is on disk.
    id: 'no-name',
    apply: (rng, project) => withRandomLeaf(rng, project, { name: pick(rng, [null, '', '   ']) }),
  },
  {
    // `.STATUS` bit 1 (value 2) is DETACHED: the entry is skipped ENTIRELY
    // during compilation (`walls/wpj.rs:135-138`), so it vanishes from
    // `survey_count` as well as from the import.
    id: 'detached-status',
    apply: (rng, project) => withRandomLeaf(rng, project, { status: 2 }),
  },
  {
    // Bit 16 (value 65536) is TYPE_OTHER: counted in `skipped_attachment_count`
    // and never read - a different disappearance from DETACHED, and the only one
    // the preview reports a number for.
    id: 'attachment-status',
    apply: (rng, project) => withRandomLeaf(rng, project, { status: 65536 }),
  },
  {
    // An absolute `.PATH` outside the project root is HONORED (genuine Walls
    // does) but warns `survey '{label}' resolves through an absolute .PATH
    // outside the project directory` (`walls/wpj.rs:146-153`). Every child then
    // resolves into a directory that has none of the siblings.
    id: 'escaping-path',
    apply: (rng, project) => ({
      ...project,
      wpj: {
        ...project.wpj,
        root: { ...project.wpj.root, path: pick(rng, ['/etc', '/tmp/not-here', 'C:\\walls']) },
      },
    }),
  },
  {
    // `.wpj` goes through `read_latin1` too (`walls/wpj.rs:79`), so a UTF-16 BOM
    // is a hard error naming the encoding. `WallsWpjSpec` has no `leading`, so
    // the document is rebuilt with the BOM in front of it.
    id: 'utf16-bom',
    apply: (rng, project) => ({
      ...project,
      wpj: {
        ...project.wpj,
        rawText: `${pick(rng, ['\u00ff\u00fe', '\u00fe\u00ff'])}${writeWallsWpj(project.wpj)}`,
      },
    }),
  },
];

// ---------------------------------------------------------------------------
// GPX
// ---------------------------------------------------------------------------

/** Every point in the document, so a corruption always has a target. */
function gpxPointCount(spec: GpxSpec): number {
  return (
    (spec.waypoints?.length ?? 0) +
    (spec.tracks ?? []).reduce(
      (total, track) =>
        total + track.segments.reduce((inner, segment) => inner + segment.length, 0),
      0,
    ) +
    (spec.routes ?? []).reduce((total, route) => total + route.points.length, 0)
  );
}

/**
 * Change the Nth point of the document, counting waypoints, then trackpoints,
 * then routepoints - the same order `gpx.rs:898-985` allocates section indices
 * in, so an id that says "point 4" means the same point on both sides.
 */
function withNthPoint(spec: GpxSpec, target: number, change: Partial<GpxPointSpec>): GpxSpec {
  let seen = 0;
  const take = (point: GpxPointSpec): GpxPointSpec => {
    const hit = seen === target;
    seen += 1;
    return hit ? { ...point, ...change } : point;
  };
  const next: GpxSpec = { ...spec };
  if (spec.waypoints !== undefined) next.waypoints = spec.waypoints.map(take);
  if (spec.tracks !== undefined) {
    next.tracks = spec.tracks.map((track) => ({
      ...track,
      segments: track.segments.map((segment) => segment.map(take)),
    }));
  }
  if (spec.routes !== undefined) {
    next.routes = spec.routes.map((route) => ({ ...route, points: route.points.map(take) }));
  }
  return next;
}

function withRandomPoint(rng: Rng, spec: GpxSpec, change: Partial<GpxPointSpec>): GpxSpec {
  const total = gpxPointCount(spec);
  if (total === 0) return spec;
  return withNthPoint(spec, randInt(rng, 0, total - 1), change);
}

const GPX_CORRUPTIONS: readonly Corruption<GpxSpec>[] = [
  {
    // The reader scans for a Start element whose LOCAL name is `gpx`
    // (`gpx.rs:137-150`); with none it reaches Eof and bails `Not a valid GPX
    // file: no <gpx> root element found`.
    id: 'no-root',
    apply: (rng, spec) => ({ ...spec, rootTag: pick(rng, ['notgpx', 'kml', 'GPX2', 'x']) }),
  },
  {
    // A self-closing `<gpx/>` never produces a Start event at all, so it fails
    // the same way from the opposite direction - a file that LOOKS like a root.
    id: 'self-closing-root',
    apply: (_rng, spec) => ({
      ...spec,
      rawText: '<?xml version="1.0" encoding="UTF-8"?>\n<gpx version="1.1" />\n',
    }),
  },
  {
    // `lat` and `lon` are both REQUIRED (`gpx.rs:222-259`) -> `Missing lat
    // attribute`, or `<wpt> element missing lat/lon attributes` at the top level.
    // There is no per-point skip: the whole file is rejected.
    id: 'missing-lat',
    applies: (spec) => gpxPointCount(spec) > 0,
    apply: (rng, spec) =>
      withRandomPoint(rng, spec, chance(rng, 0.5) ? { lat: undefined } : { lon: undefined }),
  },
  {
    // `Latitude {v} out of range [-90, 90]` (`gpx.rs:251-256`), aborting the
    // entire file - test `gpx.rs:1141-1152`.
    id: 'out-of-range-lat',
    applies: (spec) => gpxPointCount(spec) > 0,
    apply: (rng, spec) => withRandomPoint(rng, spec, { lat: pick(rng, ['91', '-91', '900', '90.0001']) }),
  },
  {
    id: 'out-of-range-lon',
    applies: (spec) => gpxPointCount(spec) > 0,
    apply: (rng, spec) =>
      withRandomPoint(rng, spec, { lon: pick(rng, ['181', '-181', '3600', '180.0001']) }),
  },
  {
    // `nan`/`inf`/`1e999` all PARSE as f64 and are then caught by the range
    // check, because `!(-90.0..=90.0).contains(&lat)` is false for NaN
    // (`gpx.rs:251-256`). Same message as an out-of-range number, different
    // mechanism reaching it.
    id: 'nonfinite-coord',
    applies: (spec) => gpxPointCount(spec) > 0,
    apply: (rng, spec) =>
      withRandomPoint(rng, spec, { lat: pick(rng, ['nan', 'inf', '1e999', '-inf']) }),
  },
  {
    // `Invalid lat value` / `Invalid lon value` (`gpx.rs:233, 241`) - the
    // `str::parse::<f64>` failure, before any range check runs.
    id: 'non-numeric-coord',
    applies: (spec) => gpxPointCount(spec) > 0,
    apply: (rng, spec) => withRandomPoint(rng, spec, { lon: pick(rng, ['abc', '', 'N45', '45,5']) }),
  },
  {
    // `XML parse error: {e}` from any of the reader's loops. A mismatched end
    // tag is the unambiguous one: quick-xml checks end names by default, so the
    // reader errors on the event rather than silently recovering.
    id: 'malformed-xml',
    applies: (spec) => spec.rawText === undefined,
    apply: (rng, spec) => {
      const tag = spec.rootTag ?? 'gpx';
      const text = writeGpxXml(spec);
      const mangled = pick(rng, [
        text.replace(`</${tag}>`, `</${tag}x>`),
        text.replace(`<${tag}`, `</${tag}0><${tag}`),
        text.slice(0, Math.max(1, text.length - 4)),
      ]);
      return { ...spec, rawText: mangled };
    },
  },
  {
    // A segment with fewer than 2 points is skipped while its `section_index` is
    // STILL consumed (`gpx.rs:920-923`, test `gpx.rs:1418-1432`), so every later
    // placement index shifts by one and the dialog's list stops lining up with
    // what actually imported. Appended rather than edited, so the corruption
    // lands even on a file that had no tracks.
    id: 'empty-track-segment',
    apply: (rng, spec) => {
      const thin: GpxTrackSpec = {
        name: 'thin',
        segments: chance(rng, 0.5) ? [[]] : [[{ lat: '45.0', lon: '-90.0', ele: '100' }]],
      };
      return { ...spec, tracks: [...(spec.tracks ?? []), thin] };
    },
  },
  {
    // `read_text` (`gpx.rs:262-281`) matches `Event::Text` and never
    // `Event::CData`, so CDATA content is silently DROPPED and `<ele>` reads as
    // None - depth 0 relative to the segment's first point.
    id: 'cdata-elevation',
    applies: (spec) => gpxPointCount(spec) > 0,
    apply: (rng, spec) => withRandomPoint(rng, spec, { ele: '350.2', eleCdata: true }),
  },
  {
    // Dropping the closing tag leaves the child loop's depth counter above zero,
    // so it exits on `Event::Eof` with whatever it has (`gpx.rs:207`) rather
    // than erroring - a truncated file that still imports, and the one shape a
    // half-written download produces.
    id: 'no-closing-root',
    apply: (_rng, spec) => ({ ...spec, omitClosingRoot: true }),
  },
  {
    // The `version` attribute is never read and neither is any namespace
    // (`gpx.rs:137-150`), so a nonsense declaration is tolerated. The catalog
    // carries it so a run that starts rejecting it is visible.
    id: 'bad-declaration',
    apply: (rng, spec) => ({
      ...spec,
      declaration: pick(rng, [
        '<?xml version="9.9"?>',
        '<?xml version="1.0" encoding="ISO-8859-1"?>',
        '<?xml',
      ]),
    }),
  },
  {
    // A `<name>`/`<cmt>` deeper than depth 2 is ignored by the guards at
    // `gpx.rs:362, 434, 441, 595, 602, 676`. Emitted as element TEXT that
    // contains the markup, so the payload has to survive escaping to prove the
    // writer is not building a document out of user data.
    id: 'hostile-name',
    applies: (spec) => gpxPointCount(spec) > 0,
    apply: (rng, spec) =>
      withRandomPoint(rng, spec, {
        name: pick(rng, ['<name>nested</name>', ']]><![CDATA[', '&bogus;', '"><wpt lat="91"']),
      }),
  },
];

export {
  DAT_CORRUPTIONS,
  GPX_CORRUPTIONS,
  MAK_CORRUPTIONS,
  SRV_CORRUPTIONS,
  WPJ_CORRUPTIONS,
};
