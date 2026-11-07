/**
 * Pure byte emitters for the survey formats the app imports: mNemo `.dmp`, and
 * Ariane `.tmlu` (plain XML) / `.tml` (a ZIP holding `Data.xml`).
 *
 * ## Why the fuzzer records specs, not bytes
 *
 * Every function here is a total function of its spec: the same spec renders to
 * byte-identical output, on any machine, in any order. That is what lets a trace
 * (`fuzzTrace.ts`, step kind `genFile`) store the SPEC and have the replayer
 * rebuild the file - a trace stays a few hundred bytes of JSON instead of an
 * embedded fixture, and a shrunk trace is still runnable. It also means nothing
 * in this module may consult the clock, the filesystem, `Math.random`, or a
 * `Map` iteration order that depends on insertion history.
 *
 * ## No fs, no ctx
 *
 * This module emits bytes and nothing else. `surveyFileSpecs.ts` owns generation
 * and the one function that touches disk; keeping the split means the byte
 * layout can be unit-tested with no session, no temp dir and no app.
 *
 * ## What the framing is checked against
 *
 * The constants below are transcribed from the parsers, not from format docs:
 * `src-tauri/src/parsers/dmp.rs` (`SECTION_MAGIC`, `SHOT_START_MAGIC`,
 * `SHOT_END_MAGIC`, `shot_size`, `end_of_section_patterns`, `parse_shot`,
 * `parse_section`) and `src-tauri/src/parsers/tml/` (`normalize_key_bytes` for
 * the long/short element spellings, `decode.rs::find_data_entry` for the archive
 * entry name). Where the two disagree the parser wins - see the `shot_size(3)`
 * note on `endOfSectionBytes`.
 */

import { crc32 } from './crc32';

// ============================================================================
// DMP
// ============================================================================

/** Section start magic, emitted for version > 2 (`dmp.rs` `SECTION_MAGIC`). */
export const DMP_SECTION_MAGIC: readonly [number, number, number] = [68, 89, 101];
/** Per-shot start magic, version >= 5 (`dmp.rs` `SHOT_START_MAGIC`). */
export const DMP_SHOT_START_MAGIC: readonly [number, number, number] = [57, 67, 77];
/** Per-shot end magic, version >= 5 (`dmp.rs` `SHOT_END_MAGIC`). */
export const DMP_SHOT_END_MAGIC: readonly [number, number, number] = [95, 25, 35];

/** Versions `parse_dmp` accepts. A spec may name any byte; only these parse. */
export const DMP_SUPPORTED_VERSIONS: readonly number[] = [2, 3, 4, 5];

/**
 * One shot record. Numeric fields are the RAW int16 the file carries, already
 * scaled the way the device writes them - `parse_shot` divides headings and
 * pitches by 10, lengths/depths/LRUD by 100, temperature by 10. So a 45.0 degree
 * heading is `450` here and a 10.00 m length is `1000`.
 *
 * Keeping the pre-scaled integer (rather than a float the writer would round) is
 * what makes the emission exact: no spec can render to two different byte
 * strings because a float landed a half-ulp either side of a rounding boundary.
 */
export interface DmpShotSpec {
  /** Raw type byte. 0=CsA, 1=CsB, 2=Standard, 3=EndOfSurvey; anything else is rejected. */
  shotType: number;
  headingIn: number;
  headingOut: number;
  length: number;
  depthIn: number;
  depthOut: number;
  pitchIn: number;
  pitchOut: number;
  /** LRUD, emitted for version >= 4 only. */
  left: number;
  right: number;
  up: number;
  down: number;
  /** Emitted for version >= 4 only. */
  temperature: number;
  /** Time-of-shot bytes, emitted for version >= 4 only. */
  hour: number;
  minute: number;
  second: number;
  markerIndex: number;
  /** Version >= 5: replace the start magic, to force `Invalid shot start magic`. */
  startMagic?: readonly [number, number, number];
  /** Version >= 5: replace the end magic, to force `Invalid shot end magic`. */
  endMagic?: readonly [number, number, number];
  /** Emit only the first N bytes of this record, so the reader runs off the end. */
  truncateAfter?: number;
}

/** How a section terminates. `parse_dmp` splits the file on exactly these. */
export type DmpEndOfSection = 'normal' | 'buggy' | 'none';

export interface DmpSectionSpec {
  /** Three name bytes. A string is taken as its first three UTF-8 bytes, padded. */
  name: string | readonly [number, number, number];
  /** 0 = In, 1 = Out; any other byte makes `parse_section` reject the section. */
  direction: number;
  /** Year as the file stores it: the offset from 2000, so 24 means 2024. */
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  shots: readonly DmpShotSpec[];
  /** Override the section magic (version > 2) to force `Invalid section magic`. */
  sectionMagic?: readonly [number, number, number];
  /** Defaults to `'normal'`; `'buggy'` is the v5 variant, `'none'` omits it. */
  endOfSection?: DmpEndOfSection;
  /**
   * Take name, direction, shots and terminator from section `index` instead.
   *
   * `compute_section_hash` covers the name, the direction and every Standard
   * shot's measurements, and deliberately NOT the date - so a clone with its own
   * date is the exact shape that makes two sections collide on `content_hash`
   * while looking different in the import preview. Cloning only the shots would
   * not produce a collision, which is why this copies the identity fields too.
   *
   * Ignored when it does not name an earlier section (no cycles, no forward
   * references - the resolution is a single left-to-right pass).
   */
  sameContentAs?: number | undefined;
}

/** Token-stream mangling applied after the byte stream has been rendered. */
export interface DmpTokenTweaks {
  /** Prepend a UTF-8 BOM. `read_utf8` strips it; a reader that does not, breaks. */
  bom?: boolean;
  /** Separate tokens with `;\r\n` instead of `;`. */
  crlf?: boolean;
  leadingSemicolon?: boolean;
  trailingSemicolon?: boolean;
  /** Double the separator after this token index (an empty token, which is skipped). */
  doubledSemicolonAt?: number;
  /** Replace one token's text outright: `'abc'`, `''`, `'99999'`, `'-1'`, `' 3 '`. */
  replaceTokenAt?: { index: number; text: string };
  /** Emit only the first N tokens. */
  truncateAfterTokens?: number;
  /** Appended verbatim after the last token. */
  trailingGarbage?: string;
}

export interface DmpSpec {
  /** Byte 0 of the file AND of every section. Any byte; only 2..5 parse. */
  version: number;
  sections: readonly DmpSectionSpec[];
  /**
   * Render each byte as the signed value a device writes (-128..127) rather than
   * 0..255. `parse_dmp` does `parse::<i32>() as u8`, so both round-trip; real
   * mNemo exports are signed, which makes this the higher-fidelity shape.
   */
  signedTokens?: boolean;
  tokens?: DmpTokenTweaks | undefined;
}

function byte(value: number): number {
  return ((Math.trunc(value) % 256) + 256) % 256;
}

function pushI16(out: number[], value: number): void {
  const raw = Math.trunc(value);
  out.push(byte(raw >> 8), byte(raw));
}

function nameBytes(name: DmpSectionSpec['name']): [number, number, number] {
  if (typeof name !== 'string') return [byte(name[0]), byte(name[1]), byte(name[2])];
  const encoded = new TextEncoder().encode(name);
  // Three bytes exactly: `parse_section` reads three and falls back to a numeric
  // spelling when they are not UTF-8, so a short name is space-padded rather
  // than left to shift every following field.
  return [encoded[0] ?? 0x20, encoded[1] ?? 0x20, encoded[2] ?? 0x20];
}

function shotBytes(version: number, shot: DmpShotSpec): number[] {
  const out: number[] = [];
  if (version >= 5) out.push(...(shot.startMagic ?? DMP_SHOT_START_MAGIC).map(byte));
  out.push(byte(shot.shotType));
  pushI16(out, shot.headingIn);
  pushI16(out, shot.headingOut);
  pushI16(out, shot.length);
  pushI16(out, shot.depthIn);
  pushI16(out, shot.depthOut);
  pushI16(out, shot.pitchIn);
  pushI16(out, shot.pitchOut);
  if (version >= 4) {
    pushI16(out, shot.left);
    pushI16(out, shot.right);
    pushI16(out, shot.up);
    pushI16(out, shot.down);
    pushI16(out, shot.temperature);
    out.push(byte(shot.hour), byte(shot.minute), byte(shot.second));
  }
  out.push(byte(shot.markerIndex));
  if (version >= 5) out.push(...(shot.endMagic ?? DMP_SHOT_END_MAGIC).map(byte));
  if (shot.truncateAfter !== undefined) return out.slice(0, Math.max(0, shot.truncateAfter));
  return out;
}

/**
 * Bytes a shot record occupies. Not `shot_size` from the parser: for version 3
 * those two DISAGREE. `shot_size(3)` returns 21, but `parse_shot` reads
 * `type + 7*int16 + marker` = 16 bytes for anything below version 4 (LRUD, temp
 * and time are gated on `version >= 4`). The parser uses 21 only as the
 * `has_remaining` guard and as the end-of-section pattern length, and the repo's
 * own `build_v3_section_data` fixture emits 16-byte shots followed by a 21-byte
 * terminator. This mirrors the fixture, i.e. the real read path.
 */
export function dmpShotByteLength(version: number): number {
  if (version >= 5) return 35;
  if (version >= 4) return 29;
  return 16;
}

/**
 * The end-of-section pattern `split_into_sections` scans for, per version.
 *
 * Its LENGTH is `shot_size(version)` in the parser, which is why v3's terminator
 * is 21 bytes while a v3 shot is 16: the two are independent numbers that happen
 * to coincide for v2, v4 and v5.
 */
export function endOfSectionBytes(version: number, kind: DmpEndOfSection = 'normal'): number[] {
  if (kind === 'none') return [];
  if (version === 5) {
    const trailer = [...DMP_SHOT_END_MAGIC];
    if (kind === 'buggy') {
      // The variant real devices emit and `end_of_section_patterns` also accepts.
      return [...DMP_SHOT_START_MAGIC, 3, 7, 8, 7, 8, ...new Array<number>(24).fill(0), ...trailer];
    }
    return [...DMP_SHOT_START_MAGIC, 3, ...new Array<number>(28).fill(0), ...trailer];
  }
  const patternLength = version === 2 ? 16 : version === 3 ? 21 : version === 4 ? 29 : 0;
  if (patternLength === 0) return [];
  return [3, ...new Array<number>(patternLength - 1).fill(0)];
}

function sectionBytes(spec: DmpSpec, index: number): number[] {
  // In range: the only caller, dmpFileBytes, loops index over spec.sections.
  const section = spec.sections[index]!;
  const cloneIndex = section.sameContentAs;
  const clone =
    cloneIndex !== undefined && cloneIndex >= 0 && cloneIndex < index
      ? spec.sections[cloneIndex]
      : null;
  const identity = clone ?? section;

  const out: number[] = [byte(spec.version)];
  if (spec.version > 2) out.push(...(section.sectionMagic ?? DMP_SECTION_MAGIC).map(byte));
  out.push(
    byte(section.year),
    byte(section.month),
    byte(section.day),
    byte(section.hour),
    byte(section.minute),
  );
  out.push(...nameBytes(identity.name));
  out.push(byte(identity.direction));
  for (const shot of identity.shots) out.push(...shotBytes(spec.version, shot));
  out.push(...endOfSectionBytes(spec.version, identity.endOfSection ?? 'normal'));
  return out;
}

/** The whole file as raw bytes, before it is rendered to semicolon-separated text. */
export function dmpFileBytes(spec: DmpSpec): number[] {
  const out: number[] = [];
  for (let index = 0; index < spec.sections.length; index++) out.push(...sectionBytes(spec, index));
  return out;
}

/**
 * The file as the text `parse_dmp` actually reads: signed-or-unsigned decimal
 * integers separated by `;`.
 *
 * `parse_dmp` trims the whole document and each token, skips empty tokens, and
 * casts every parsed `i32` to `u8` - so a doubled separator, surrounding
 * whitespace and an out-of-range value are all things the format tolerates,
 * which is exactly why `DmpTokenTweaks` can produce them.
 */
export function writeDmpText(spec: DmpSpec): string {
  const raw = dmpFileBytes(spec);
  const tweaks = spec.tokens ?? {};
  let tokens = raw.map((value) =>
    String(spec.signedTokens === true && value > 127 ? value - 256 : value),
  );

  const replace = tweaks.replaceTokenAt;
  if (replace !== undefined && replace.index >= 0 && replace.index < tokens.length) {
    tokens[replace.index] = replace.text;
  }
  if (tweaks.truncateAfterTokens !== undefined) {
    tokens = tokens.slice(0, Math.max(0, tweaks.truncateAfterTokens));
  }

  const separator = tweaks.crlf === true ? ';\r\n' : ';';
  const doubled = tweaks.doubledSemicolonAt;
  const parts: string[] = [];
  for (let index = 0; index < tokens.length; index++) {
    parts.push(tokens[index]!);
    if (index === tokens.length - 1) break;
    parts.push(doubled !== undefined && doubled === index ? `${separator};` : separator);
  }

  let text = parts.join('');
  if (tweaks.leadingSemicolon === true) text = `;${text}`;
  if (tweaks.trailingSemicolon === true) text = `${text};`;
  if (tweaks.trailingGarbage !== undefined) text += tweaks.trailingGarbage;
  // Written as an escape, not as a literal: a raw U+FEFF in source is invisible
  // in every editor and diff, which is how it ends up somewhere nobody meant.
  if (tweaks.bom === true) text = `\uFEFF${text}`;
  return text;
}

/** `writeDmpText` encoded to the bytes that go on disk. */
export function writeDmpBytes(spec: DmpSpec): Uint8Array {
  return new TextEncoder().encode(writeDmpText(spec));
}

// ============================================================================
// TML / TMLU
// ============================================================================

/**
 * One `<SurveyData>` (or `<SRVD>`) element. Every value is a string or a number
 * so a spec can carry text a typed field could not - `id: '1e9999'`,
 * `color: 'not-a-color'`, `excluded: 'maybe'` - and the writer emits it verbatim
 * (escaped) rather than coercing it.
 */
export interface TmlShotSpec {
  id: number | string;
  fromId: number | string;
  closureToId?: number | string;
  /** `REAL` / `VIRTUAL` / `START` are the ones the importer knows. */
  type: string;
  section: string;
  azimuth: number | string;
  length: number | string;
  depth: number | string;
  /** `-1.0` is the file's "not recorded" sentinel; `resolve_tml_depth_in` reads it. */
  depthIn: number | string;
  inclination: number | string;
  left: number | string;
  right: number | string;
  up: number | string;
  down: number | string;
  comment: string;
  explorer: string;
  /** `2000-01-01` is Ariane's "no date" sentinel. */
  date: string;
  color: string;
  excluded: boolean | string;
  latitude: number | string;
  longitude: number | string;
  /** Ariane 26+ per-row identity (`UUID` / `UU`). Omitted when absent. */
  uuid?: string;
  /**
   * Escape hatch: emitted VERBATIM in place of the whole element, with no
   * escaping and no well-formedness check. This is how a spec produces an
   * unclosed tag, a stray `]]>` outside a CDATA section, or a nested `<Shape>`
   * subtree. Every OTHER field is ignored when this is set.
   */
  rawXml?: string;
}

/** Container-level shape of a `.tml`, which is a ZIP rather than a document. */
export interface TmlContainerSpec {
  /** Defaults to `Data.xml`; `find_data_entry` also accepts any case variant. */
  entryName?: string;
  /** Extra STORED entries, so the archive is not a single-file one. */
  extraEntries?: readonly { name: string; text: string }[];
  /** Keep only the first N bytes of the finished archive. */
  truncateBytes?: number;
  /** Replace the whole file with these bytes - a non-ZIP payload under a .tml name. */
  rawBytes?: readonly number[];
}

export interface TmlSpec {
  caveName: string;
  /** `m` / `ft` / `metric` / `imperial` are recognised; anything else warns. */
  unit: string;
  useMagneticAzimuth: boolean | string;
  /** A string so a spec can carry text no `f64` holds (`classify_unit`'s sibling case). */
  firstStartAbsoluteElevation: string;
  shots: readonly TmlShotSpec[];
  /** `long` = `<SurveyData><ID>...`, `short` = `<SRVD><ID>...` (the TMLU spelling). */
  dialect?: 'long' | 'short';
  /** Replace the XML declaration, or omit it entirely with `null`. */
  declaration?: string | null;
  /** Defaults to `CaveFile`; anything else is a document the parser finds no shots in. */
  rootTag?: string;
  /** Drop the closing root tag, so `open_elements` never returns to zero. */
  omitClosingRoot?: boolean;
  /** Appended verbatim after the document. */
  trailing?: string;
  container?: TmlContainerSpec;
}

/**
 * Escape text so it is DATA, never markup.
 *
 * All five predefined entities, including both quote forms: the corpus carries
 * spreadsheet-injection payloads (`=cmd|' /C calc'!A0`) and XML fragments
 * (`]]>`, `<![CDATA[`) whose whole point is that they must arrive at the import
 * dialog as literal characters. Escaping `>` is what neutralises `]]>` without
 * special-casing it.
 *
 * NOT escaped, because they cannot be: C0 control characters other than tab, CR
 * and LF have no legal spelling in XML 1.0 - not even as a numeric reference. A
 * spec that wants them uses `rawXml` and is by construction a malformed-XML case.
 */
export function escapeXmlText(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&apos;');
}

function element(tag: string, value: string | number | boolean): string {
  return `<${tag}>${escapeXmlText(String(value))}</${tag}>`;
}

/** Long/short spellings, straight from `normalize_key_bytes`. */
const TML_TAGS = {
  long: {
    shot: 'SurveyData',
    id: 'ID',
    fromId: 'FromID',
    closureToId: 'ClosureToID',
    type: 'Type',
    section: 'Section',
    azimuth: 'Azimut',
    length: 'Length',
    depth: 'Depth',
    depthIn: 'DepthIn',
    inclination: 'Inclination',
    left: 'Left',
    right: 'Right',
    up: 'Up',
    down: 'Down',
    comment: 'Comment',
    explorer: 'Explorer',
    date: 'Date',
    color: 'Color',
    excluded: 'Excluded',
    latitude: 'Latitude',
    longitude: 'Longitude',
    uuid: 'UUID',
  },
  short: {
    shot: 'SRVD',
    id: 'ID',
    fromId: 'FRID',
    closureToId: 'CID',
    type: 'TY',
    section: 'SC',
    azimuth: 'AZ',
    length: 'LG',
    depth: 'DP',
    depthIn: 'DPI',
    inclination: 'INC',
    left: 'L',
    right: 'R',
    up: 'U',
    down: 'D',
    comment: 'CM',
    explorer: 'EX',
    date: 'DT',
    color: 'CL',
    excluded: 'EXC',
    latitude: 'LT',
    longitude: 'LGT',
    uuid: 'UU',
  },
} as const;

export function writeTmlShotXml(shot: TmlShotSpec, dialect: 'long' | 'short' = 'long'): string {
  if (shot.rawXml !== undefined) return shot.rawXml;
  const t = TML_TAGS[dialect];
  const fields: string[] = [
    element(t.id, shot.id),
    element(t.fromId, shot.fromId),
    element(t.closureToId, shot.closureToId ?? -1),
    element(t.type, shot.type),
    element(t.section, shot.section),
    element(t.azimuth, shot.azimuth),
    element(t.length, shot.length),
    element(t.depth, shot.depth),
    element(t.depthIn, shot.depthIn),
    element(t.inclination, shot.inclination),
    element(t.left, shot.left),
    element(t.right, shot.right),
    element(t.up, shot.up),
    element(t.down, shot.down),
    element(t.comment, shot.comment),
    element(t.explorer, shot.explorer),
    element(t.date, shot.date),
    element(t.color, shot.color),
    element(t.excluded, shot.excluded),
    element(t.latitude, shot.latitude),
    element(t.longitude, shot.longitude),
  ];
  if (shot.uuid !== undefined) fields.push(element(t.uuid, shot.uuid));
  return `<${t.shot}>${fields.join('')}</${t.shot}>`;
}

const DEFAULT_DECLARATION = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>';

/** The XML document, which IS a `.tmlu` and is a `.tml`'s `Data.xml` entry. */
export function writeTmluXml(spec: TmlSpec): string {
  const root = spec.rootTag ?? 'CaveFile';
  const dialect = spec.dialect ?? 'long';
  const declaration = spec.declaration === null ? '' : (spec.declaration ?? DEFAULT_DECLARATION);
  const head = [
    element('caveName', spec.caveName),
    element('unit', spec.unit),
    element('useMagneticAzimuth', spec.useMagneticAzimuth),
    element('firstStartAbsoluteElevation', spec.firstStartAbsoluteElevation),
  ].join('\n  ');
  const shots = spec.shots.map((shot) => writeTmlShotXml(shot, dialect)).join('\n    ');
  const body = `<${root}>\n  ${head}\n  <Data>\n    ${shots}\n  </Data>${
    spec.omitClosingRoot === true ? '' : `\n</${root}>`
  }`;
  return `${declaration === '' ? '' : `${declaration}\n`}${body}${spec.trailing ?? ''}`;
}

/** The `.tml` archive: `writeTmluXml`'s output, STORED in a ZIP. */
export function writeTmlZip(spec: TmlSpec): Uint8Array {
  const container = spec.container ?? {};
  if (container.rawBytes !== undefined) return Uint8Array.from(container.rawBytes.map(byte));

  const encoder = new TextEncoder();
  const entries = [
    { name: container.entryName ?? 'Data.xml', data: encoder.encode(writeTmluXml(spec)) },
    ...(container.extraEntries ?? []).map((extra) => ({
      name: extra.name,
      data: encoder.encode(extra.text),
    })),
  ];
  const archive = zipStore(entries);
  if (container.truncateBytes === undefined) return archive;
  return archive.slice(0, Math.max(0, container.truncateBytes));
}

// ============================================================================
// ZIP (STORED)
// ============================================================================

/**
 * A fixed DOS timestamp for every entry: 1980-01-01 00:00:00.
 *
 * Zero would be a nominally invalid date (month 0, day 0). The point is that it
 * is a CONSTANT - a real mtime would make the same spec render to different
 * bytes on two runs, which is the one property this module promises not to break.
 */
const DOS_TIME = 0;
const DOS_DATE = (0 << 9) | (1 << 5) | 1;

class ByteSink {
  private readonly bytes: number[] = [];

  u16(value: number): void {
    this.bytes.push(value & 0xff, (value >>> 8) & 0xff);
  }

  u32(value: number): void {
    this.bytes.push(value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff);
  }

  raw(values: Uint8Array | readonly number[]): void {
    for (const value of values) this.bytes.push(value & 0xff);
  }

  get length(): number {
    return this.bytes.length;
  }

  toUint8Array(): Uint8Array {
    return Uint8Array.from(this.bytes);
  }
}

/**
 * A ZIP archive with every entry STORED (compression method 0).
 *
 * Hand-rolled, because the harness adds no npm dependency for a container this
 * small, and because STORED output is byte-predictable in a way a deflate
 * stream is not - a compressor's tie-breaking is an implementation detail, and
 * the determinism guarantee at the top of this file would be at its mercy.
 *
 * The Rust `zip` crate the app reads with handles STORED entries natively, so
 * nothing downstream can tell the difference.
 */
export function zipStore(entries: readonly { name: string; data: Uint8Array }[]): Uint8Array {
  const encoder = new TextEncoder();
  const prepared = entries.map((entry) => ({
    nameBytes: encoder.encode(entry.name),
    data: entry.data,
    crc: crc32(entry.data),
  }));

  const local = new ByteSink();
  const offsets: number[] = [];
  for (const entry of prepared) {
    offsets.push(local.length);
    local.u32(0x04034b50); // local file header signature
    local.u16(20); // version needed to extract (2.0)
    local.u16(0); // general purpose bit flag
    local.u16(0); // compression method: stored
    local.u16(DOS_TIME);
    local.u16(DOS_DATE);
    local.u32(entry.crc);
    local.u32(entry.data.length); // compressed size == uncompressed for stored
    local.u32(entry.data.length);
    local.u16(entry.nameBytes.length);
    local.u16(0); // extra field length
    local.raw(entry.nameBytes);
    local.raw(entry.data);
  }

  const central = new ByteSink();
  for (let index = 0; index < prepared.length; index++) {
    const entry = prepared[index]!;
    central.u32(0x02014b50); // central directory header signature
    central.u16(20); // version made by
    central.u16(20); // version needed to extract
    central.u16(0); // general purpose bit flag
    central.u16(0); // compression method: stored
    central.u16(DOS_TIME);
    central.u16(DOS_DATE);
    central.u32(entry.crc);
    central.u32(entry.data.length);
    central.u32(entry.data.length);
    central.u16(entry.nameBytes.length);
    central.u16(0); // extra field length
    central.u16(0); // file comment length
    central.u16(0); // disk number start
    central.u16(0); // internal file attributes
    central.u32(0); // external file attributes
    central.u32(offsets[index]!); // the loop above pushed one offset per prepared entry
    central.raw(entry.nameBytes);
  }

  const out = new ByteSink();
  out.raw(local.toUint8Array());
  out.raw(central.toUint8Array());
  out.u32(0x06054b50); // end of central directory signature
  out.u16(0); // this disk
  out.u16(0); // disk with the central directory
  out.u16(prepared.length);
  out.u16(prepared.length);
  out.u32(central.length);
  out.u32(local.length);
  out.u16(0); // archive comment length
  return out.toUint8Array();
}
