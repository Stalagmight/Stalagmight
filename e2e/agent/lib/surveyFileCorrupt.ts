/**
 * The corruption catalog: a named list of ways to break a generated survey file,
 * so a fuzz run covers the rejection paths as deliberately as it covers the
 * happy one.
 *
 * ## Named, not random bit-flips
 *
 * A random flip in a semicolon-separated integer file almost always lands on a
 * digit and produces "a slightly different survey", which is not a rejection
 * path at all. Every entry here targets a specific `bail!` or a specific
 * silently-tolerated shape in the parsers, and carries the id it is known by, so
 * a failure says `dmp:month-13` rather than "seed 91 iteration 12".
 *
 * ## Corruption is a pure spec transform
 *
 * `corruptSpec` returns a NEW spec; nothing is mutated in place. A trace records
 * the corrupted spec, so replay does not need the catalog, the rng, or this
 * file's version - the same reason `surveyFileWriters.ts` is a pure function of
 * its input.
 *
 * The catalog is filtered by what the target actually is: the ZIP-container
 * entries only apply to `.tml`, the v5-magic entries only to a v5 `.dmp`, and
 * `corruptSpec` never returns a corruption it could not apply.
 */
import {
  type DmpSectionSpec,
  type DmpShotSpec,
  type DmpSpec,
  type TmlShotSpec,
  type TmlSpec,
  dmpFileBytes,
  writeDmpText,
} from './surveyFileWriters';
import { type GeneratedFileSpec, type Rng, pick, randInt, surveyFileName } from './surveyFileSpecs';
import {
  type Corruption,
  DAT_CORRUPTIONS,
  GPX_CORRUPTIONS,
  MAK_CORRUPTIONS,
  SRV_CORRUPTIONS,
  WPJ_CORRUPTIONS,
} from './surveyFileCorruptText';

/** A corrupted file, and the catalog id that says how. */
export interface CorruptedFileSpec {
  spec: GeneratedFileSpec;
  /** `dmp:month-13`, `tml:cycle`, ... - stable, greppable, safe to log. */
  corruption: string;
}

/**
 * Ceiling for the `oversized` corruption.
 *
 * `MAX_DMP_FILE_SIZE` in the parser is 50 MB, and the point of this entry is to
 * exercise a big-but-accepted file (the loader's streaming, the preview table's
 * virtualization, the line-list rendering), not to hit the cap - a run that
 * writes 50 MB per iteration spends its whole budget on I/O. 2 MB is roughly
 * 40x a real device dump and still finishes in milliseconds.
 */
const MAX_OVERSIZED_BYTES = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// DMP
// ---------------------------------------------------------------------------

type DmpCorruption = { id: string; applies?: (spec: DmpSpec) => boolean; apply: (rng: Rng, spec: DmpSpec) => DmpSpec };

function replaceLine(
  spec: DmpSpec,
  index: number,
  change: Partial<DmpSectionSpec>,
): DmpSpec {
  const lines = spec.lines.map((line, at) => (at === index ? { ...line, ...change } : line));
  return { ...spec, lines };
}

function withRandomLine(rng: Rng, spec: DmpSpec, change: Partial<DmpSectionSpec>): DmpSpec {
  if (spec.lines.length === 0) return spec;
  return replaceLine(spec, randInt(rng, 0, spec.lines.length - 1), change);
}

function withRandomShot(rng: Rng, spec: DmpSpec, change: Partial<DmpShotSpec>): DmpSpec {
  // Only lines that HAVE a shot are candidates, so the returned spec really
  // carries the corruption its id names rather than silently being the clean one.
  const candidates = spec.lines
    .map((line, index) => ({ line, index }))
    .filter((entry) => entry.line.shots.length > 0);
  if (candidates.length === 0) return spec;
  const chosen = pick(rng, candidates);
  const shotIndex = randInt(rng, 0, chosen.line.shots.length - 1);
  return replaceLine(spec, chosen.index, {
    shots: chosen.line.shots.map((shot, at) => (at === shotIndex ? { ...shot, ...change } : shot)),
  });
}

/** One token per byte, so the byte count IS the token count - no second render. */
function tokenCount(spec: DmpSpec): number {
  return dmpFileBytes(spec).length;
}

function withToken(rng: Rng, spec: DmpSpec, text: string): DmpSpec {
  const total = tokenCount(spec);
  return { ...spec, tokens: { ...spec.tokens, replaceTokenAt: { index: randInt(rng, 0, total - 1), text } } };
}

const DMP_CORRUPTIONS: readonly DmpCorruption[] = [
  // Header and framing.
  { id: 'unsupported-version', apply: (rng, spec) => ({ ...spec, version: pick(rng, [0, 1, 6, 7, 255]) }) },
  {
    id: 'bad-line-magic',
    applies: (spec) => spec.version > 2,
    apply: (rng, spec) => withRandomLine(rng, spec, { lineMagic: [99, 99, 99] }),
  },
  {
    id: 'bad-shot-start-magic',
    applies: (spec) => spec.version >= 5,
    apply: (rng, spec) => withRandomShot(rng, spec, { startMagic: [1, 2, 3] }),
  },
  {
    id: 'bad-shot-end-magic',
    applies: (spec) => spec.version >= 5,
    apply: (rng, spec) => withRandomShot(rng, spec, { endMagic: [1, 2, 3] }),
  },

  // Date validation, one entry per `bail!` in `parse_line`.
  { id: 'month-0', apply: (rng, spec) => withRandomLine(rng, spec, { month: 0 }) },
  { id: 'month-13', apply: (rng, spec) => withRandomLine(rng, spec, { month: 13 }) },
  { id: 'day-0', apply: (rng, spec) => withRandomLine(rng, spec, { day: 0 }) },
  { id: 'day-32', apply: (rng, spec) => withRandomLine(rng, spec, { day: 32 }) },
  // Day 31 of a 30-day month is accepted by every range check and then rejected
  // by `NaiveDate::from_ymd_opt` - a different branch from `day-32`.
  { id: 'day-31-of-april', apply: (rng, spec) => withRandomLine(rng, spec, { month: 4, day: 31 }) },
  { id: 'hour-24', apply: (rng, spec) => withRandomLine(rng, spec, { hour: 24 }) },
  { id: 'minute-60', apply: (rng, spec) => withRandomLine(rng, spec, { minute: 60 }) },
  // The year byte is an offset from 2000 and the check is `2000..2100`, so 100
  // is the first rejected value.
  { id: 'year-2100', apply: (rng, spec) => withRandomLine(rng, spec, { year: 100 }) },
  { id: 'bad-direction', apply: (rng, spec) => withRandomLine(rng, spec, { direction: randInt(rng, 2, 255) }) },

  // Shot-level.
  { id: 'bad-shot-type', apply: (rng, spec) => withRandomShot(rng, spec, { shotType: randInt(rng, 4, 255) }) },
  { id: 'truncated-mid-shot', apply: (rng, spec) => withRandomShot(rng, spec, { truncateAfter: randInt(rng, 1, 12) }) },
  { id: 'zero-shot-line', apply: (rng, spec) => withRandomLine(rng, spec, { shots: [] }) },

  // Line termination.
  { id: 'missing-end-of-line', apply: (rng, spec) => withRandomLine(rng, spec, { endOfLine: 'none' }) },
  {
    id: 'buggy-eos',
    applies: (spec) => spec.version === 5,
    apply: (rng, spec) => withRandomLine(rng, spec, { endOfLine: 'buggy' }),
  },

  // Token stream. `parse_dmp` trims, drops empty tokens and casts i32 to u8, so
  // only two of these actually fail - which is the point of having both.
  { id: 'non-integer-token', apply: (rng, spec) => withToken(rng, spec, 'abc') },
  { id: 'empty-token', apply: (rng, spec) => withToken(rng, spec, '') },
  { id: 'whitespace-token', apply: (rng, spec) => withToken(rng, spec, '   ') },
  { id: 'float-token', apply: (rng, spec) => withToken(rng, spec, '3.5') },
  // Parses as i32, then casts: a silent value change rather than an error.
  { id: 'out-of-range-token', apply: (rng, spec) => withToken(rng, spec, '99999') },
  // Overflows i32 outright, so `parse::<i32>` is what fails.
  { id: 'overflow-token', apply: (rng, spec) => withToken(rng, spec, '99999999999999999999') },
  { id: 'doubled-semicolon', apply: (rng, spec) => ({ ...spec, tokens: { ...spec.tokens, doubledSemicolonAt: randInt(rng, 0, tokenCount(spec) - 1) } }) },
  { id: 'leading-semicolon', apply: (_rng, spec) => ({ ...spec, tokens: { ...spec.tokens, leadingSemicolon: true } }) },
  { id: 'trailing-semicolon', apply: (_rng, spec) => ({ ...spec, tokens: { ...spec.tokens, trailingSemicolon: true } }) },
  { id: 'crlf', apply: (_rng, spec) => ({ ...spec, tokens: { ...spec.tokens, crlf: true } }) },
  { id: 'bom', apply: (_rng, spec) => ({ ...spec, tokens: { ...spec.tokens, bom: true } }) },
  {
    id: 'trailing-garbage',
    apply: (rng, spec) => ({
      ...spec,
      tokens: { ...spec.tokens, trailingGarbage: pick(rng, ['\nEOF', ';;;', ' garbage ', '\u0000']) },
    }),
  },
  { id: 'truncated-file', apply: (rng, spec) => ({ ...spec, tokens: { ...spec.tokens, truncateAfterTokens: randInt(rng, 1, Math.max(1, tokenCount(spec) - 1)) } }) },

  // Size.
  { id: 'oversized', apply: (_rng, spec) => oversize(spec) },
];

/** Repeat the line list until the rendered file is near `MAX_OVERSIZED_BYTES`. */
function oversize(spec: DmpSpec): DmpSpec {
  const unit = writeDmpText(spec).length;
  if (unit === 0) return spec;
  const copies = Math.min(20000, Math.max(2, Math.floor(MAX_OVERSIZED_BYTES / unit)));
  const lines: DmpSectionSpec[] = [];
  for (let copy = 0; copy < copies; copy++) {
    // `sameContentAs` indices are line-local, so they are dropped rather than
    // left pointing into a previous copy.
    for (const line of spec.lines) lines.push({ ...line, sameContentAs: undefined });
  }
  return { ...spec, lines };
}

// ---------------------------------------------------------------------------
// TML / TMLU
// ---------------------------------------------------------------------------

type TmlCorruption = {
  id: string;
  /** `zip` entries only make sense for the archive format. */
  formats?: readonly ('tml' | 'tmlu')[];
  apply: (rng: Rng, spec: TmlSpec) => TmlSpec;
};

/**
 * The subset of the hostile corpus that is interesting INSIDE an XML document.
 *
 * Kept local rather than imported from `surveyFileSpecs.ts` so this list can stay
 * the markup-hazard half without dragging the 8 KB entry into every corruption
 * draw - the oversize case has its own entry.
 */
const HOSTILE_XML_COMMENTS: readonly string[] = [
  ']]>',
  '<![CDATA[',
  '&amp;',
  'raw & ampersand < > "quotes"',
  "=cmd|' /C calc'!A0",
  '\tleading tab',
  '\u202eoverridden',
];

function withRandomTmlShot(rng: Rng, spec: TmlSpec, change: Partial<TmlShotSpec>): TmlSpec {
  if (spec.shots.length === 0) return spec;
  const index = randInt(rng, 0, spec.shots.length - 1);
  return { ...spec, shots: spec.shots.map((shot, at) => (at === index ? { ...shot, ...change } : shot)) };
}

const TML_CORRUPTIONS: readonly TmlCorruption[] = [
  // XML level.
  {
    id: 'unclosed-tag',
    apply: (rng, spec) => withRandomTmlShot(rng, spec, { rawXml: '<SurveyData><ID>1</ID><FromID>0</FromID>' }),
  },
  {
    id: 'stray-markup',
    apply: (rng, spec) => withRandomTmlShot(rng, spec, { rawXml: '<SurveyData/></Data></CaveFile><Data>' }),
  },
  { id: 'wrong-root', apply: (_rng, spec) => ({ ...spec, rootTag: 'NotACaveFile' }) },
  { id: 'omit-closing-root', apply: (_rng, spec) => ({ ...spec, omitClosingRoot: true }) },
  { id: 'no-declaration', apply: (_rng, spec) => ({ ...spec, declaration: null }) },
  {
    id: 'bad-declaration',
    apply: (rng, spec) => ({
      ...spec,
      declaration: pick(rng, [
        '<?xml version="1.0" encoding="ISO-8859-1"?>',
        '<?xml version="9.9"?>',
        '<?xml',
      ]),
    }),
  },
  { id: 'trailing-garbage', apply: (rng, spec) => ({ ...spec, trailing: pick(rng, ['<<garbage', '\u0000', ']]>']) }) },
  {
    id: 'empty-document',
    apply: (_rng, spec) => ({ ...spec, shots: [], declaration: null, rootTag: 'CaveFile' }),
  },

  // Schema level: the graph the importer reconstructs topology from.
  {
    id: 'cycle',
    apply: (_rng, spec) =>
      spec.shots.length < 2
        ? spec
        : {
            ...spec,
            shots: spec.shots.map((shot, index) =>
              index === 0
                ? { ...shot, id: 0, fromId: 1, type: 'REAL' }
                : index === 1
                  ? { ...shot, id: 1, fromId: 0 }
                  : shot,
            ),
          },
  },
  { id: 'self-parent', apply: (rng, spec) => withRandomTmlShot(rng, spec, { fromId: 0, id: 0 }) },
  { id: 'dangling-parent', apply: (rng, spec) => withRandomTmlShot(rng, spec, { fromId: 999999 }) },
  { id: 'duplicate-ids', apply: (_rng, spec) => ({ ...spec, shots: spec.shots.map((shot) => ({ ...shot, id: 1 })) }) },
  {
    id: 'negative-ids',
    apply: (_rng, spec) => ({ ...spec, shots: spec.shots.map((shot, index) => ({ ...shot, id: -index - 1 })) }),
  },
  { id: 'dangling-closure', apply: (rng, spec) => withRandomTmlShot(rng, spec, { closureToId: 999999 }) },
  { id: 'unknown-shot-type', apply: (rng, spec) => withRandomTmlShot(rng, spec, { type: pick(rng, ['', 'BOGUS', 'real', '<REAL>']) }) },

  // Field level: values that parse as text but not as what they claim to be.
  {
    id: 'garbage-numerics',
    apply: (rng, spec) =>
      withRandomTmlShot(rng, spec, {
        azimuth: pick(rng, ['NaN', 'abc', '', '1e999', '--3']),
        length: pick(rng, ['NaN', 'Infinity', '', '0x10']),
        depth: pick(rng, ['', 'null', '1,5']),
      }),
  },
  { id: 'garbage-color', apply: (rng, spec) => withRandomTmlShot(rng, spec, { color: pick(rng, ['', 'red', '0xZZ', '0x1']) }) },
  { id: 'garbage-date', apply: (rng, spec) => withRandomTmlShot(rng, spec, { date: pick(rng, ['', 'yesterday', '2024-13-45', '0000-00-00']) }) },
  { id: 'garbage-excluded', apply: (rng, spec) => withRandomTmlShot(rng, spec, { excluded: pick(rng, ['maybe', '', '1']) }) },
  { id: 'bad-unit', apply: (rng, spec) => ({ ...spec, unit: pick(rng, ['', 'furlongs', 'FEET', 'm '.repeat(400)]) }) },
  {
    id: 'garbage-elevation',
    apply: (rng, spec) => ({
      ...spec,
      firstStartAbsoluteElevation: pick(rng, ['', 'NaN', 'abc', '1e999', '-Infinity']),
    }),
  },
  {
    id: 'hostile-comment',
    apply: (rng, spec) => withRandomTmlShot(rng, spec, { comment: pick(rng, HOSTILE_XML_COMMENTS) }),
  },

  // Container level: only a `.tml` has one.
  { id: 'zip-wrong-entry', formats: ['tml'], apply: (rng, spec) => ({ ...spec, container: { ...spec.container, entryName: pick(rng, ['NotData.xml', 'data/Data.xml', 'Data.XML.bak', '']) } }) },
  { id: 'zip-truncated', formats: ['tml'], apply: (rng, spec) => ({ ...spec, container: { ...spec.container, truncateBytes: randInt(rng, 1, 120) } }) },
  {
    id: 'zip-not-a-zip',
    formats: ['tml'],
    apply: (_rng, spec) => ({
      ...spec,
      // `<?xml ...` under a `.tml` name: the extension picks the ZIP reader, so
      // this is the "user renamed their .tmlu" case, not just random bytes.
      container: { ...spec.container, rawBytes: [...new TextEncoder().encode('<?xml version="1.0"?><CaveFile/>')] },
    }),
  },
  {
    id: 'zip-extra-entry',
    formats: ['tml'],
    apply: (_rng, spec) => ({
      ...spec,
      container: { ...spec.container, extraEntries: [{ name: 'Data.xml.bak', text: '<CaveFile/>' }] },
    }),
  },
];

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/**
 * The entries of one catalog that apply to one spec, in catalog order.
 *
 * ORDER IS PART OF THE CONTRACT. `corruptSpec` draws with a single `pick` over
 * this filtered list, so inserting an entry in the MIDDLE of a catalog shifts
 * which entry every existing seed draws. The header of
 * `scenarios/importDialogFuzz.ts` carries repro lines whose trajectories depend
 * on exactly that, which is why the DMP and TML catalogs above are appended to
 * and never rearranged, and why the five new ones live in their own module and
 * are dispatched to rather than merged in.
 */
function applicable<T>(entries: readonly Corruption<T>[], spec: T): readonly Corruption<T>[] {
  return entries.filter((entry) => entry.applies?.(spec) ?? true);
}

/** Catalog ids that could be applied to `spec`, in catalog order. */
export function corruptionsFor(spec: GeneratedFileSpec): string[] {
  switch (spec.format) {
    case 'dmp':
      return DMP_CORRUPTIONS.filter((entry) => entry.applies?.(spec.dmp) ?? true).map(
        (entry) => `dmp:${entry.id}`,
      );
    case 'tml':
    case 'tmlu':
      return TML_CORRUPTIONS.filter((entry) =>
        (entry.formats ?? ['tml', 'tmlu']).includes(spec.format),
      ).map((entry) => `tml:${entry.id}`);
    case 'dat':
      return applicable(DAT_CORRUPTIONS, spec.dat).map((entry) => `dat:${entry.id}`);
    case 'mak':
      return applicable(MAK_CORRUPTIONS, { mak: spec.mak, dats: spec.dats }).map(
        (entry) => `mak:${entry.id}`,
      );
    case 'srv':
      return applicable(SRV_CORRUPTIONS, spec.srv).map((entry) => `srv:${entry.id}`);
    case 'wpj':
      return applicable(WPJ_CORRUPTIONS, { wpj: spec.wpj, srvs: spec.srvs }).map(
        (entry) => `wpj:${entry.id}`,
      );
    default:
      return applicable(GPX_CORRUPTIONS, spec.gpx).map((entry) => `gpx:${entry.id}`);
  }
}

/**
 * Corrupt one generated file.
 *
 * Deterministic in `rng`: the same stream position and the same input spec
 * always produce the same catalog entry and the same corrupted spec. The name is
 * recomputed, so a corrupted file never collides with the clean one it came from
 * - and for a `.mak`/`.wpj` the WHOLE file set comes back, siblings included,
 * because a corruption may have changed the sibling list rather than the project
 * text (`mak:missing-dat-sibling`, `wpj:missing-srv`).
 */
export function corruptSpec(rng: Rng, spec: GeneratedFileSpec): CorruptedFileSpec {
  if (spec.format === 'dmp') {
    const candidates = DMP_CORRUPTIONS.filter((entry) => entry.applies?.(spec.dmp) ?? true);
    const entry = pick(rng, candidates);
    const dmp = entry.apply(rng, spec.dmp);
    return {
      spec: { format: 'dmp', name: surveyFileName('dmp', dmp), profile: spec.profile, dmp },
      corruption: `dmp:${entry.id}`,
    };
  }

  if (spec.format === 'tml' || spec.format === 'tmlu') {
    const format = spec.format;
    const candidates = TML_CORRUPTIONS.filter((entry) => (entry.formats ?? ['tml', 'tmlu']).includes(format));
    const entry = pick(rng, candidates);
    const tml = entry.apply(rng, spec.tml);
    // Spelled out per variant rather than `{ format, ... }`: the union's two arms
    // differ only in the literal, and TypeScript will not widen one object into a
    // union of two literal-discriminated members.
    const next: GeneratedFileSpec =
      format === 'tml'
        ? { format: 'tml', name: surveyFileName('tml', tml), profile: spec.profile, tml }
        : { format: 'tmlu', name: surveyFileName('tmlu', tml), profile: spec.profile, tml };
    return { spec: next, corruption: `tml:${entry.id}` };
  }

  const profile = spec.profile;
  if (spec.format === 'dat') {
    const entry = pick(rng, applicable(DAT_CORRUPTIONS, spec.dat));
    const dat = entry.apply(rng, spec.dat);
    return {
      spec: { format: 'dat', name: surveyFileName('dat', dat), profile, dat },
      corruption: `dat:${entry.id}`,
    };
  }
  if (spec.format === 'mak') {
    const project = { mak: spec.mak, dats: spec.dats };
    const entry = pick(rng, applicable(MAK_CORRUPTIONS, project));
    const next = entry.apply(rng, project);
    return {
      spec: {
        format: 'mak',
        name: surveyFileName('mak', next),
        profile,
        mak: next.mak,
        dats: next.dats,
      },
      corruption: `mak:${entry.id}`,
    };
  }
  if (spec.format === 'srv') {
    const entry = pick(rng, applicable(SRV_CORRUPTIONS, spec.srv));
    const srv = entry.apply(rng, spec.srv);
    return {
      spec: { format: 'srv', name: surveyFileName('srv', srv), profile, srv },
      corruption: `srv:${entry.id}`,
    };
  }
  if (spec.format === 'wpj') {
    const project = { wpj: spec.wpj, srvs: spec.srvs };
    const entry = pick(rng, applicable(WPJ_CORRUPTIONS, project));
    const next = entry.apply(rng, project);
    return {
      spec: {
        format: 'wpj',
        name: surveyFileName('wpj', next),
        profile,
        wpj: next.wpj,
        srvs: next.srvs,
      },
      corruption: `wpj:${entry.id}`,
    };
  }

  const entry = pick(rng, applicable(GPX_CORRUPTIONS, spec.gpx));
  const gpx = entry.apply(rng, spec.gpx);
  return {
    spec: { format: 'gpx', name: surveyFileName('gpx', gpx), profile, gpx },
    corruption: `gpx:${entry.id}`,
  };
}
