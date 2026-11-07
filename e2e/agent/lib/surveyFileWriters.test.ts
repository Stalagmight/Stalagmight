/**
 * The writers are the fuzzer's contact surface with three real parsers, so what
 * matters is not that they emit "something plausible" but that they emit the
 * exact framing `parse_dmp`, `parse_tml` and the Rust `zip` crate read. Two
 * things pin that here:
 *
 *   - a TS mirror of `parse_dmp`'s tokenizer and section walk, so a framing
 *     mistake (a wrong magic, a v3 terminator sized like a v3 shot, a signed
 *     token that does not cast back) fails as a round-trip mismatch rather than
 *     as a mysterious "0 sections" hours later inside the app;
 *   - a byte-for-byte pin of a one-entry STORED archive, derived by hand from
 *     APPNOTE rather than from this module's own output, so the pin cannot
 *     ratify a bug it inherited.
 *
 * Determinism gets its own case: traces store SPECS, and a spec that renders two
 * ways is a trace that does not replay.
 */
import { describe, expect, it } from 'vitest';

import { crc32, crc32Text } from './crc32';
import {
  DMP_SECTION_MAGIC,
  DMP_SHOT_END_MAGIC,
  DMP_SHOT_START_MAGIC,
  type DmpShotSpec,
  type DmpSpec,
  type TmlSpec,
  dmpFileBytes,
  dmpShotByteLength,
  endOfSectionBytes,
  escapeXmlText,
  writeDmpBytes,
  writeDmpText,
  writeTmlZip,
  writeTmluXml,
  zipStore,
} from './surveyFileWriters';

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
// A TS mirror of parse_dmp, deliberately naive
// ---------------------------------------------------------------------------

/** `parse_dmp`: trim, split on `;`, drop empties, trim each, parse i32, cast u8. */
function tokenizeDmp(text: string): number[] {
  return text
    .trim()
    .split(';')
    .filter((token) => token !== '')
    .map((token) => {
      const parsed = Number(token.trim());
      if (!Number.isInteger(parsed)) throw new Error(`not an integer token: ${JSON.stringify(token)}`);
      return ((parsed % 256) + 256) % 256;
    });
}

/** `split_into_sections`: cut after every window matching an end-of-section pattern. */
function splitSections(bytes: readonly number[], version: number): number[][] {
  const patterns = [endOfSectionBytes(version, 'normal')];
  if (version === 5) patterns.push(endOfSectionBytes(version, 'buggy'));
  const patternLength = patterns[0]!.length; // built with one entry just above
  if (patternLength === 0 || bytes.length < patternLength) return [];

  const sections: number[][] = [];
  let start = 0;
  for (let index = 0; index + patternLength <= bytes.length; index++) {
    const window = bytes.slice(index, index + patternLength);
    if (patterns.some((pattern) => pattern.every((value, offset) => value === window[offset]))) {
      sections.push(bytes.slice(start, index + patternLength));
      start = index + patternLength;
    }
  }
  return sections;
}

interface MirroredShot {
  shotType: number;
  values: number[];
  markerIndex: number;
}

interface MirroredSection {
  version: number;
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  name: string;
  direction: number;
  shots: MirroredShot[];
}

/** `parse_section` + `parse_shot`, minus the validation and the pitch flip. */
function parseSection(bytes: readonly number[]): MirroredSection {
  let cursor = 0;
  const read = (): number => {
    if (cursor >= bytes.length) throw new Error('read beyond buffer');
    return bytes[cursor++]!;
  };
  const readI16 = (): number => {
    const raw = (read() << 8) | read();
    return raw >= 0x8000 ? raw - 0x10000 : raw;
  };

  const version = read();
  if (version > 2) {
    const magic = [read(), read(), read()];
    if (!magic.every((value, index) => value === DMP_SECTION_MAGIC[index])) {
      throw new Error(`invalid section magic ${magic.join(',')}`);
    }
  }
  const year = read();
  const month = read();
  const day = read();
  const hour = read();
  const minute = read();
  const name = String.fromCharCode(read(), read(), read());
  const direction = read();

  const shots: MirroredShot[] = [];
  const stride = version >= 5 ? 35 : version >= 4 ? 29 : version === 3 ? 21 : 16;
  while (cursor + stride <= bytes.length) {
    const before = cursor;
    try {
      if (version >= 5) {
        const magic = [read(), read(), read()];
        if (!magic.every((value, index) => value === DMP_SHOT_START_MAGIC[index])) {
          throw new Error('invalid shot start magic');
        }
      }
      const shotType = read();
      if (shotType > 3) throw new Error(`invalid shot type ${shotType}`);
      const values: number[] = [];
      for (let i = 0; i < 7; i++) values.push(readI16());
      if (version >= 4) {
        for (let i = 0; i < 5; i++) values.push(readI16());
        values.push(read(), read(), read());
      }
      const markerIndex = read();
      if (version >= 5) {
        const magic = [read(), read(), read()];
        if (!magic.every((value, index) => value === DMP_SHOT_END_MAGIC[index])) {
          throw new Error('invalid shot end magic');
        }
      }
      shots.push({ shotType, values, markerIndex });
    } catch {
      // `parse_section` breaks out of the shot loop on the first failure.
      cursor = before;
      break;
    }
  }
  return { version, year, month, day, hour, minute, name, direction, shots };
}

function parseDmp(text: string, version: number): MirroredSection[] {
  return splitSections(tokenizeDmp(text), version).map(parseSection);
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function shot(overrides: Partial<DmpShotSpec> = {}): DmpShotSpec {
  return {
    shotType: 2,
    headingIn: 450,
    headingOut: 460,
    length: 1000,
    depthIn: 200,
    depthOut: 300,
    pitchIn: 50,
    pitchOut: -50,
    left: 100,
    right: 150,
    up: 200,
    down: 80,
    temperature: 185,
    hour: 14,
    minute: 45,
    second: 30,
    markerIndex: 1,
    ...overrides,
  };
}

function dmpSpec(version: number, overrides: Partial<DmpSpec> = {}): DmpSpec {
  return {
    version,
    sections: [
      {
        name: 'TST',
        direction: 0,
        year: 24,
        month: 6,
        day: 15,
        hour: 10,
        minute: 30,
        shots: [shot(), shot({ headingIn: 1800, markerIndex: 2 })],
      },
    ],
    ...overrides,
  };
}

const TML_SHOT = {
  id: 1,
  fromId: 0,
  type: 'REAL',
  section: 'Main',
  azimuth: 90,
  length: 10,
  depth: 5,
  depthIn: -1,
  inclination: 0,
  left: 1,
  right: 2,
  up: 3,
  down: 4,
  comment: '',
  explorer: 'Tester',
  date: '2024-06-15',
  color: '0x8f6112ff',
  excluded: false,
  latitude: 0,
  longitude: 0,
};

function tmlSpec(overrides: Partial<TmlSpec> = {}): TmlSpec {
  return {
    caveName: 'Test Cave',
    unit: 'm',
    useMagneticAzimuth: true,
    firstStartAbsoluteElevation: '100.0',
    shots: [{ ...TML_SHOT, type: 'START', id: 0, fromId: -1 }, TML_SHOT],
    ...overrides,
  };
}

/**
 * Minimal well-formedness: a balanced tag stack, and no raw `<`/`>` in text.
 *
 * That is enough BECAUSE `escapeXmlText` is the only path text takes into the
 * document - so the absence of a stray angle bracket is exactly the property
 * being checked, not an assumption.
 */
function xmlIsWellFormed(xml: string): boolean {
  const body = xml.replace(/<\?[\s\S]*?\?>/g, '');
  const stack: string[] = [];
  const tag = /<(\/?)([A-Za-z_][\w.-]*)([^<>]*?)(\/?)>/g;
  let lastEnd = 0;
  let match: RegExpExecArray | null;
  while ((match = tag.exec(body)) !== null) {
    if (/[<>]/.test(body.slice(lastEnd, match.index))) return false;
    lastEnd = tag.lastIndex;
    if (match[1] === '/') {
      if (stack.pop() !== match[2]) return false;
    } else if (match[4] !== '/') {
      stack.push(match[2]!); // group 2 is non-optional
    }
  }
  if (/[<>]/.test(body.slice(lastEnd))) return false;
  return stack.length === 0;
}

function toHex(bytes: Uint8Array): string {
  return [...bytes].map((value) => value.toString(16).padStart(2, '0')).join('');
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('crc32', () => {
  it('matches the check value every CRC-32 implementation publishes', () => {
    // "123456789" -> 0xCBF43926 is the IEEE check vector; if this is wrong,
    // every ZIP this module writes is rejected by every extractor.
    expect(crc32Text('123456789')).toBe(0xcbf43926);
    expect(crc32(new Uint8Array())).toBe(0);
  });
});

describe('writeDmpText framing', () => {
  for (const version of [2, 3, 4, 5]) {
    it(`round trips a v${version} section through a mirror of parse_dmp`, () => {
      const spec = dmpSpec(version);
      const sections = parseDmp(writeDmpText(spec), version);

      expect(sections).toHaveLength(1);
      const parsed = sections[0]!; // toHaveLength(1) above
      expect(parsed.version).toBe(version);
      expect(parsed.name).toBe('TST');
      expect(parsed.direction).toBe(0);
      expect([parsed.year, parsed.month, parsed.day, parsed.hour, parsed.minute]).toEqual([
        24, 6, 15, 10, 30,
      ]);

      // Two written shots plus the terminator, which `parse_shot` reads as an
      // ordinary EndOfSurvey record rather than skipping.
      expect(parsed.shots.map((s) => s.shotType)).toEqual([2, 2, 3]);
      // Three shots: the shotType list above.
      expect(parsed.shots[0]!.values.slice(0, 7)).toEqual([450, 460, 1000, 200, 300, 50, -50]);
      expect(parsed.shots[0]!.markerIndex).toBe(1);
      expect(parsed.shots[1]!.values[0]).toBe(1800);
      if (version >= 4) {
        expect(parsed.shots[0]!.values.slice(7, 12)).toEqual([100, 150, 200, 80, 185]);
        expect(parsed.shots[0]!.values.slice(12)).toEqual([14, 45, 30]);
      } else {
        expect(parsed.shots[0]!.values).toHaveLength(7);
      }
    });
  }

  it('sizes a v3 record like parse_shot (16) and its terminator like shot_size (21)', () => {
    // The one place the parser disagrees with itself. Getting this backwards
    // yields a file that splits into zero sections.
    expect(dmpShotByteLength(3)).toBe(16);
    expect(endOfSectionBytes(3)).toHaveLength(21);
    expect(dmpShotByteLength(2)).toBe(endOfSectionBytes(2).length);
    expect(dmpShotByteLength(4)).toBe(endOfSectionBytes(4).length);
    expect(dmpShotByteLength(5)).toBe(endOfSectionBytes(5).length);
    expect(endOfSectionBytes(5, 'buggy')).toHaveLength(35);
    expect(endOfSectionBytes(5, 'none')).toEqual([]);
  });

  it('repeats the version byte at the head of every section', () => {
    const spec: DmpSpec = {
      version: 4,
      sections: [
        // dmpSpec builds exactly one section when no override replaces them.
        { ...dmpSpec(4).sections[0]!, name: 'AAA' },
        { ...dmpSpec(4).sections[0]!, name: 'BBB', direction: 1, day: 16 },
      ],
    };
    const sections = parseDmp(writeDmpText(spec), 4);
    expect(sections.map((s) => s.name)).toEqual(['AAA', 'BBB']);
    expect(sections.map((s) => s.version)).toEqual([4, 4]);
    expect(sections[1]!.direction).toBe(1); // two sections: the name list above
  });

  it('sameContentAs clones the fields the content hash covers, not the date', () => {
    const base = dmpSpec(5).sections[0]!; // dmpSpec builds exactly one section
    const spec: DmpSpec = {
      version: 5,
      sections: [
        { ...base, name: 'DUP', direction: 1 },
        { ...base, name: 'zzz', direction: 0, day: 28, sameContentAs: 0 },
      ],
    };
    const sections = parseDmp(writeDmpText(spec), 5);
    expect(at(sections, 1).name).toBe('DUP');
    expect(at(sections, 1).direction).toBe(1);
    expect(at(sections, 1).shots).toEqual(at(sections, 0).shots);
    // The date is the section's own: that is what makes it a duplicate the
    // preview shows twice while `compute_section_hash` collapses them.
    expect(at(sections, 0).day).toBe(15);
    expect(at(sections, 1).day).toBe(28);
  });

  it('ignores a sameContentAs that is not an earlier section', () => {
    const base = dmpSpec(4).sections[0]!; // dmpSpec builds exactly one section
    const spec: DmpSpec = {
      version: 4,
      sections: [
        { ...base, name: 'AAA', sameContentAs: 0 },
        { ...base, name: 'BBB', sameContentAs: 7 },
      ],
    };
    expect(parseDmp(writeDmpText(spec), 4).map((s) => s.name)).toEqual(['AAA', 'BBB']);
  });

  it('renders signed tokens that cast back to the same bytes', () => {
    const spec = dmpSpec(4);
    const signed = writeDmpText({ ...spec, signedTokens: true });
    expect(signed).toContain('-50');
    expect(tokenizeDmp(signed)).toEqual(dmpFileBytes(spec));
    expect(tokenizeDmp(writeDmpText(spec))).toEqual(dmpFileBytes(spec));
  });

  it('applies the token tweaks the format tolerates without moving the bytes', () => {
    const spec = dmpSpec(4);
    const expected = dmpFileBytes(spec);
    const tolerated: DmpSpec['tokens'][] = [
      { leadingSemicolon: true },
      { trailingSemicolon: true },
      { doubledSemicolonAt: 3 },
      { crlf: true },
      { bom: true },
    ];
    for (const tokens of tolerated) {
      // The BOM is stripped by `read_utf8` before tokenizing, so the mirror
      // gets the same treatment rather than a special case.
      const text = writeDmpText({ ...spec, tokens }).replace(/^\uFEFF/, '');
      expect(tokenizeDmp(text)).toEqual(expected);
    }
    expect(toHex(writeDmpBytes({ ...spec, tokens: { bom: true } })).startsWith('efbbbf')).toBe(true);
  });

  it('produces the corruption shapes the catalog needs', () => {
    const spec = dmpSpec(4);
    expect(() => tokenizeDmp(writeDmpText({ ...spec, tokens: { replaceTokenAt: { index: 2, text: 'abc' } } }))).toThrow(
      /not an integer/,
    );
    const truncated = tokenizeDmp(writeDmpText({ ...spec, tokens: { truncateAfterTokens: 12 } }));
    expect(truncated).toHaveLength(12);
    expect(parseDmp(writeDmpText({ ...spec, tokens: { truncateAfterTokens: 12 } }), 4)).toEqual([]);
    expect(writeDmpText({ ...spec, tokens: { trailingGarbage: 'oops' } }).endsWith('oops')).toBe(true);

    const noEos: DmpSpec = {
      version: 4,
      sections: [{ ...spec.sections[0]!, endOfSection: 'none' }], // dmpSpec: one section
    };
    expect(parseDmp(writeDmpText(noEos), 4)).toEqual([]);

    const badMagic: DmpSpec = {
      version: 4,
      sections: [{ ...spec.sections[0]!, sectionMagic: [99, 99, 99] }],
    };
    expect(() => parseSection(tokenizeDmp(writeDmpText(badMagic)))).toThrow(/invalid section magic/);
  });

  it('takes the v5 buggy terminator as a section end too', () => {
    const spec: DmpSpec = {
      version: 5,
      sections: [{ ...dmpSpec(5).sections[0]!, endOfSection: 'buggy' }], // dmpSpec: one section
    };
    const sections = parseDmp(writeDmpText(spec), 5);
    expect(sections).toHaveLength(1);
    expect(sections[0]!.shots.map((s) => s.shotType)).toEqual([2, 2, 3]); // toHaveLength(1) above
  });

  /**
   * The strongest check available without running cargo: reproduce, token for
   * token, the two synthetic files the Rust parser's OWN tests assert it parses
   * (`build_v3_section_data` and `build_v4_section_data` in
   * `src-tauri/src/parsers/dmp.rs`, consumed by `test_parse_dmp_v3_format` and
   * `test_parse_dmp_v4_with_lrud`).
   *
   * The TS mirror above can only prove this module agrees with a second thing
   * this module's author wrote. These strings are the real parser's accepted
   * input, transcribed - so a shared misreading of the format shows up here.
   */
  it('reproduces build_v3_section_data token for token', () => {
    const spec: DmpSpec = {
      version: 3,
      sections: [
        {
          name: 'TST',
          direction: 0,
          year: 24,
          month: 6,
          day: 15,
          hour: 10,
          minute: 30,
          shots: [
            {
              ...shot(),
              headingIn: 450,
              headingOut: 460,
              length: 1000,
              depthIn: 200,
              depthOut: 300,
              pitchIn: 0,
              pitchOut: 0,
              markerIndex: 1,
            },
          ],
        },
      ],
    };
    const expected = [
      '3', // version
      '68;89;101', // section magic
      '24;6;15;10;30', // date
      '84;83;84', // "TST"
      '0', // direction: In
      '2', // shot type: Standard
      '1;194;1;204;3;232;0;200;1;44;0;0;0;0', // 7 int16 BE
      '1', // marker index
      ['3', ...new Array<string>(20).fill('0')].join(';'), // 21-byte terminator
    ].join(';');
    expect(writeDmpText(spec)).toBe(expected);
  });

  it('reproduces build_v4_section_data token for token', () => {
    const spec: DmpSpec = {
      version: 4,
      sections: [
        {
          name: 'V4S',
          direction: 1,
          year: 24,
          month: 8,
          day: 20,
          hour: 14,
          minute: 45,
          shots: [
            {
              shotType: 2,
              headingIn: 900,
              headingOut: 910,
              length: 500,
              depthIn: 100,
              depthOut: 150,
              pitchIn: 50,
              pitchOut: -50,
              left: 100,
              right: 150,
              up: 200,
              down: 80,
              temperature: 185,
              hour: 14,
              minute: 45,
              second: 30,
              markerIndex: 2,
            },
          ],
        },
      ],
    };
    const expected = [
      '4',
      '68;89;101',
      '24;8;20;14;45',
      '86;52;83', // "V4S"
      '1', // direction: Out
      '2',
      '3;132;3;142;1;244;0;100;0;150;0;50;255;206', // -50 as two's complement
      '0;100;0;150;0;200;0;80', // LRUD
      '0;185', // temperature
      '14;45;30', // time
      '2', // marker index
      ['3', ...new Array<string>(28).fill('0')].join(';'),
    ].join(';');
    expect(writeDmpText(spec)).toBe(expected);
  });

  it('emits exactly three name bytes whatever the name is', () => {
    for (const [name, expected] of [
      ['AB', 'AB '],
      ['ABCD', 'ABC'],
      ['', '   '],
    ] as const) {
      // dmpSpec builds exactly one section.
      const spec: DmpSpec = { version: 4, sections: [{ ...dmpSpec(4).sections[0]!, name }] };
      expect(at(parseDmp(writeDmpText(spec), 4), 0).name).toBe(expected);
    }
    // A non-ASCII name still occupies three bytes; the parser falls back to a
    // numeric spelling when they are not valid UTF-8 on their own.
    const highBytes: DmpSpec = {
      version: 4,
      sections: [{ ...dmpSpec(4).sections[0]!, name: [0xc3, 0x28, 0xff] }], // dmpSpec: one section
    };
    expect(dmpFileBytes(highBytes).slice(9, 12)).toEqual([0xc3, 0x28, 0xff]);
  });
});

describe('writeTmluXml', () => {
  it('emits a well-formed document with the long element spellings', () => {
    const xml = writeTmluXml(tmlSpec());
    expect(xmlIsWellFormed(xml)).toBe(true);
    expect(xml).toContain('<CaveFile>');
    expect(xml).toContain('<SurveyData><ID>0</ID><FromID>-1</FromID><ClosureToID>-1</ClosureToID>');
    expect(xml).toContain('<Azimut>90</Azimut>');
    expect(xml).toContain('<firstStartAbsoluteElevation>100.0</firstStartAbsoluteElevation>');
  });

  it('emits exactly the element set the parser test fixtures use', () => {
    // Transcribed from `tml_shot_xml` / `tmlu_shot_xml` in
    // `src-tauri/src/parsers/tml/test_helpers.rs`, in their order. A tag this
    // module invented would be dropped silently by `normalize_key_bytes`, and
    // the field would just read as its default all the way into the import.
    const tagsIn = (xml: string, open: string, close: string): string[] =>
      [...xml.slice(xml.indexOf(open) + open.length, xml.indexOf(close)).matchAll(/<([A-Za-z]+)>/g)].map(
        (match) => match[1]!, // group 1 is non-optional
      );

    expect(tagsIn(writeTmluXml(tmlSpec()), '<SurveyData>', '</SurveyData>')).toEqual([
      'ID', 'FromID', 'ClosureToID', 'Type', 'Section', 'Azimut', 'Length', 'Depth', 'DepthIn',
      'Inclination', 'Left', 'Right', 'Up', 'Down', 'Comment', 'Explorer', 'Date', 'Color',
      'Excluded', 'Latitude', 'Longitude',
    ]);
    expect(tagsIn(writeTmluXml(tmlSpec({ dialect: 'short' })), '<SRVD>', '</SRVD>')).toEqual([
      'ID', 'FRID', 'CID', 'TY', 'SC', 'AZ', 'LG', 'DP', 'DPI', 'INC', 'L', 'R', 'U', 'D',
      'CM', 'EX', 'DT', 'CL', 'EXC', 'LT', 'LGT',
    ]);
  });

  it('switches every field name for the short (TMLU) dialect', () => {
    const xml = writeTmluXml(tmlSpec({ dialect: 'short' }));
    expect(xmlIsWellFormed(xml)).toBe(true);
    expect(xml).toContain('<SRVD><ID>0</ID><FRID>-1</FRID><CID>-1</CID><TY>START</TY>');
    expect(xml).toContain('<LGT>0</LGT>');
    expect(xml).not.toContain('<SurveyData>');
  });

  it('carries hostile comment text as DATA, not as markup', () => {
    const hostile = "]]><![CDATA[ =cmd|' /C calc'!A0 & <Data/> \"q\"";
    const xml = writeTmluXml(tmlSpec({ shots: [{ ...TML_SHOT, comment: hostile }] }));
    expect(xmlIsWellFormed(xml)).toBe(true);
    expect(xml).toContain(
      '<Comment>]]&gt;&lt;![CDATA[ =cmd|&apos; /C calc&apos;!A0 &amp; &lt;Data/&gt; &quot;q&quot;</Comment>',
    );
    // The payload must not have opened a second Data element.
    expect(xml.match(/<Data>/g)).toHaveLength(1);
  });

  it('escapes the ampersand first, so an entity is not double-decoded', () => {
    expect(escapeXmlText('&amp; & <')).toBe('&amp;amp; &amp; &lt;');
  });

  it('passes rawXml through verbatim and ignores the typed fields beside it', () => {
    const xml = writeTmluXml(
      tmlSpec({ shots: [{ ...TML_SHOT, comment: 'ignored', rawXml: '<SurveyData><ID>9</ID>' }] }),
    );
    expect(xml).toContain('<SurveyData><ID>9</ID>');
    expect(xml).not.toContain('ignored');
    expect(xmlIsWellFormed(xml)).toBe(false);
  });

  it('produces the document-level corruptions the catalog needs', () => {
    expect(writeTmluXml(tmlSpec({ rootTag: 'NotACaveFile' }))).toContain('<NotACaveFile>');
    expect(writeTmluXml(tmlSpec({ omitClosingRoot: true })).endsWith('</Data>')).toBe(true);
    expect(writeTmluXml(tmlSpec({ declaration: null })).startsWith('<CaveFile>')).toBe(true);
    expect(writeTmluXml(tmlSpec({ trailing: '<<garbage' }))).toContain('<<garbage');
  });
});

describe('zipStore', () => {
  it('matches a hand-derived STORED archive byte for byte', () => {
    // Derived from APPNOTE, not from this module: local header (30 + name +
    // data), central directory entry (46 + name), end-of-central-directory (22).
    // Fixed 1980-01-01 timestamp, method 0, no extra fields, no comment.
    const expected =
      '504b0304' + '1400' + '0000' + '0000' + '0000' + '2100' +
      '2639f4cb' + '09000000' + '09000000' + '0500' + '0000' +
      '612e747874' + '313233343536373839' +
      '504b0102' + '1400' + '1400' + '0000' + '0000' + '0000' + '2100' +
      '2639f4cb' + '09000000' + '09000000' + '0500' + '0000' + '0000' + '0000' + '0000' +
      '00000000' + '00000000' + '612e747874' +
      '504b0506' + '0000' + '0000' + '0100' + '0100' + '33000000' + '2c000000' + '0000';

    const archive = zipStore([
      { name: 'a.txt', data: new TextEncoder().encode('123456789') },
    ]);
    expect(toHex(archive)).toBe(expected);
    expect(archive).toHaveLength(44 + 51 + 22);
  });

  it('records each entry local-header offset in the central directory', () => {
    const encoder = new TextEncoder();
    const archive = zipStore([
      { name: 'a', data: encoder.encode('xx') },
      { name: 'bb', data: encoder.encode('yyy') },
    ]);
    const view = new DataView(archive.buffer, archive.byteOffset, archive.byteLength);
    const firstLocal = 30 + 1 + 2;
    expect(view.getUint32(firstLocal, true)).toBe(0x04034b50);
    // Second central-directory record's relative-offset field.
    const centralStart = firstLocal + 30 + 2 + 3;
    const secondCentral = centralStart + 46 + 1;
    expect(view.getUint32(secondCentral, true)).toBe(0x02014b50);
    expect(view.getUint32(secondCentral + 42, true)).toBe(firstLocal);
  });
});

describe('writeTmlZip', () => {
  it('stores the document as Data.xml by default', () => {
    const archive = writeTmlZip(tmlSpec());
    const text = new TextDecoder().decode(archive);
    expect(text.startsWith('PK\u0003\u0004')).toBe(true);
    expect(text).toContain('Data.xml');
    expect(text).toContain('<CaveFile>');
  });

  it('honours the container corruptions', () => {
    expect(new TextDecoder().decode(writeTmlZip(tmlSpec({ container: { entryName: 'Nope.xml' } })))).toContain(
      'Nope.xml',
    );
    expect(writeTmlZip(tmlSpec({ container: { truncateBytes: 40 } }))).toHaveLength(40);
    expect([...writeTmlZip(tmlSpec({ container: { rawBytes: [1, 2, 3] } }))]).toEqual([1, 2, 3]);
    const extra = new TextDecoder().decode(
      writeTmlZip(tmlSpec({ container: { extraEntries: [{ name: 'Notes.txt', text: 'hi' }] } })),
    );
    expect(extra).toContain('Notes.txt');
  });
});

describe('determinism', () => {
  it('renders the same spec to identical bytes, which is what a trace relies on', () => {
    const dmp = dmpSpec(5, { signedTokens: true, tokens: { crlf: true } });
    expect(toHex(writeDmpBytes(dmp))).toBe(toHex(writeDmpBytes(structuredClone(dmp) as DmpSpec)));

    const tml = tmlSpec({ shots: [{ ...TML_SHOT, comment: 'x'.repeat(300) }] });
    expect(writeTmluXml(tml)).toBe(writeTmluXml(structuredClone(tml) as TmlSpec));
    expect(toHex(writeTmlZip(tml))).toBe(toHex(writeTmlZip(structuredClone(tml) as TmlSpec)));
  });
});
