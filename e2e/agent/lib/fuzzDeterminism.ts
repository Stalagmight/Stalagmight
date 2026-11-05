/**
 * Determinism mode for the real-app fuzzers: the PURE half (masking,
 * canonical forms, record streams and the first-difference search). The I/O
 * half - reading the app, the saved `.stalx` and the exported files - is
 * `fuzzDeterminismProbe.ts`; running a seed twice is `fuzzDeterminismRun.ts`.
 *
 * ## What a replay records
 *
 * One stream of records per replay, in the order the run produced them: every
 * drawn trace step (`action`), every IPC answer the scenario reads (`answer`),
 * the full project state plus both history stacks after every step (`state`),
 * every table of a saved `.stalx` row by row (`table`) and every exported file
 * (`file`). Two replays of the same seed must produce the same stream.
 *
 * ## What is MASKED - and nothing else is
 *
 *   - wall-clock values: any object key or column named in `WALL_CLOCK_KEYS`
 *     (null stays null, so "was it stamped" is still compared), zip entry
 *     times, PDF `/CreationDate` and `/ModDate`, PNG `tIME` chunks;
 *   - temp paths: a registered root (the session dir) becomes `<root>`, and a
 *     `mkdtemp` child of the OS temp dir keeps its prefix and loses its random
 *     six-character tail.
 *
 * ## What is CANONICALIZED (equality-preserving, not masked)
 *
 *   - object key order (a map's iteration order is not data);
 *   - random v4 uuids are renamed `<uuid#n>` in order of first appearance in
 *     the replay's stream. `Shot::new` mints one per shot (models/src/shot.rs),
 *     so two replays can never agree on the spelling - but the renaming keeps
 *     the STRUCTURE: a uuid that is kept in one replay and re-minted in the
 *     other still differs;
 *   - sqlite rows are read in primary-key order (rowid when there is none).
 *
 * A file record also carries the sha256 of its raw bytes. When neither replay
 * had to mask anything in it, the bytes themselves must be equal - so a
 * byte-level difference the canonical form hides (key order in a JSON export,
 * say) still fails.
 */
import { createHash } from 'crypto';
import * as zlib from 'zlib';

/** Object keys and sqlite columns whose value is a wall-clock instant. */
export const WALL_CLOCK_KEYS: ReadonlySet<string> = new Set([
  // the project model and its rows: project, sections, station_merges
  'created_at',
  'modified_at',
  'merged_at',
  // undo/redo rows and history items
  'executed_at',
  'timestamp',
  'replayed_at',
  // resolved warnings, the SpeleoDB link and its undo watermark
  'resolved_at',
  'last_edit_at',
  'last_verify_at',
  'inherited_through',
  // the JSON export header, dive plans, display-setting targets, backups
  'exported_at',
  'updated_at',
  'createdAt',
  'modifiedAt',
]);

export const WALL_CLOCK_MASK = '<wall-clock>';

const UUID_PATTERN = /[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/g;

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function sha256(data: string | Uint8Array): string {
  return createHash('sha256').update(data).digest('hex');
}

/**
 * One replay's masker. It is STATEFUL on purpose: the uuid renaming numbers
 * uuids in the order the replay's stream first shows them, so both replays
 * must push their records through their own masker in the same order.
 */
export class Masker {
  private readonly uuids = new Map<string, string>();
  private readonly roots: string[] = [];
  private readonly tmpPattern: RegExp | null;
  private substitutions = 0;

  constructor(tmpDir: string | null) {
    const base = tmpDir === null ? '' : tmpDir.replace(/[\\/]+$/, '');
    this.tmpPattern =
      base === ''
        ? null
        : new RegExp(`${escapeRegExp(base)}[\\\\/]([^\\\\/\\s"'<>]*?)[A-Za-z0-9]{6}(?=[\\\\/\\s"'<>]|$)`, 'g');
  }

  /** A directory whose spelling differs per replay (a session dir): masked as `<root>`. */
  addRoot(dir: string): void {
    const root = dir.replace(/[\\/]+$/, '');
    if (root === '' || this.roots.includes(root)) return;
    this.roots.push(root);
    // Longest first, so a root nested in another is replaced whole.
    this.roots.sort((a, b) => b.length - a.length);
  }

  /** Substitutions made since the last call. */
  takeCount(): number {
    const count = this.substitutions;
    this.substitutions = 0;
    return count;
  }

  text(input: string): string {
    let out = input;
    for (const root of this.roots) {
      if (!out.includes(root)) continue;
      const parts = out.split(root);
      this.substitutions += parts.length - 1;
      out = parts.join('<root>');
    }
    if (this.tmpPattern !== null) {
      out = out.replace(this.tmpPattern, (_match, prefix: string) => {
        this.substitutions += 1;
        return `<tmp>/${prefix}XXXXXX`;
      });
    }
    return out.replace(UUID_PATTERN, (uuid) => {
      this.substitutions += 1;
      let token = this.uuids.get(uuid);
      if (token === undefined) {
        token = `<uuid#${this.uuids.size + 1}>`;
        this.uuids.set(uuid, token);
      }
      return token;
    });
  }

  /** The canonical, masked copy of a JSON-shaped value (keys sorted). */
  value(input: unknown, key: string | null = null): unknown {
    if (key !== null && WALL_CLOCK_KEYS.has(key) && input !== null && input !== undefined) {
      this.substitutions += 1;
      return WALL_CLOCK_MASK;
    }
    if (typeof input === 'string') return this.text(input);
    if (typeof input === 'bigint') return input.toString();
    if (typeof input === 'number') return Number.isFinite(input) ? input : String(input);
    if (Array.isArray(input)) return input.map((item) => this.value(item));
    if (input instanceof Uint8Array) return { blob: sha256(input), size: input.length };
    if (input !== null && typeof input === 'object') {
      const out: Record<string, unknown> = {};
      const source = input as Record<string, unknown>;
      for (const name of Object.keys(source).sort()) out[this.text(name)] = this.value(source[name], name);
      return out;
    }
    return input === undefined ? null : input;
  }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

export type DetRecordKind = 'action' | 'answer' | 'state' | 'table' | 'file';

export interface DetRecord {
  readonly kind: DetRecordKind;
  /** Masked, so two replays' labels compare equal. */
  readonly label: string;
  /** Trace steps drawn when the record was taken: the step index a repro names. */
  readonly step: number;
  /** sha256 of the canonical JSON. */
  readonly hash: string;
  /** The canonical JSON, deflated: a run keeps every record until the comparison. */
  readonly body: Buffer;
  /** Substitutions the masker made in this record. */
  readonly masked: number;
  /** sha256 of a file's raw bytes; compared only when neither replay masked anything. */
  readonly rawHash: string | null;
}

export function makeRecord(
  kind: DetRecordKind,
  label: string,
  step: number,
  canonical: unknown,
  masked: number,
  rawHash: string | null = null,
): DetRecord {
  const json = JSON.stringify(canonical) ?? 'null';
  return {
    kind,
    label,
    step,
    hash: sha256(json),
    body: zlib.deflateRawSync(Buffer.from(json, 'utf8')),
    masked,
    rawHash,
  };
}

export function recordBody(record: DetRecord): unknown {
  return JSON.parse(zlib.inflateRawSync(record.body).toString('utf8')) as unknown;
}

// ---------------------------------------------------------------------------
// First difference
// ---------------------------------------------------------------------------

export interface Difference {
  path: string;
  a: string;
  b: string;
}

const PREVIEW = 160;

function preview(value: unknown): string {
  const text = value === undefined ? '<absent>' : (JSON.stringify(value) ?? String(value));
  return text.length > PREVIEW ? `${text.slice(0, PREVIEW)}...(${text.length} chars)` : text;
}

/** Two differing strings, shown around the first character where they part. */
function stringPair(a: string, b: string): { a: string; b: string } {
  let at = 0;
  while (at < a.length && at < b.length && a[at] === b[at]) at += 1;
  const from = Math.max(0, at - 40);
  const show = (text: string): string =>
    `${from > 0 ? '...' : ''}${JSON.stringify(text.slice(from, at + 60))}${text.length > at + 60 ? '...' : ''} (char ${at})`;
  return { a: show(a), b: show(b) };
}

function shape(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  return typeof value;
}

const IDENTIFIER = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

/** The first path (keys in sorted order, arrays by index) where `a` and `b` differ. */
export function firstDifference(a: unknown, b: unknown, path = '$'): Difference | null {
  if (a === b) return null;
  const kind = shape(a);
  if (kind !== shape(b)) return { path, a: preview(a), b: preview(b) };
  if (kind === 'array') {
    const left = a as unknown[];
    const right = b as unknown[];
    const shared = Math.min(left.length, right.length);
    for (let index = 0; index < shared; index++) {
      const found = firstDifference(left[index], right[index], `${path}[${index}]`);
      if (found !== null) return found;
    }
    if (left.length !== right.length) {
      return { path: `${path}.length`, a: String(left.length), b: String(right.length) };
    }
    return null;
  }
  if (kind === 'object') {
    const left = a as Record<string, unknown>;
    const right = b as Record<string, unknown>;
    const keys = [...new Set([...Object.keys(left), ...Object.keys(right)])].sort();
    for (const key of keys) {
      const next = IDENTIFIER.test(key) ? `${path}.${key}` : `${path}[${JSON.stringify(key)}]`;
      if (!(key in left)) return { path: next, a: '<absent>', b: preview(right[key]) };
      if (!(key in right)) return { path: next, a: preview(left[key]), b: '<absent>' };
      const found = firstDifference(left[key], right[key], next);
      if (found !== null) return found;
    }
    return null;
  }
  if (kind === 'string') return { path, ...stringPair(a as string, b as string) };
  return { path, a: preview(a), b: preview(b) };
}

// ---------------------------------------------------------------------------
// Comparing two replays
// ---------------------------------------------------------------------------

export interface Divergence {
  /** 0-based index into the record streams. */
  index: number;
  /** The trace step (replay A's count) the record was taken at. */
  step: number;
  kind: string;
  label: string;
  /** The first differing path, or `<record>` / `<end of stream>` / `$<bytes>`. */
  path: string;
  a: string;
  b: string;
}

const describe = (record: DetRecord): string => `${record.kind} "${record.label}"`;

export function compareRecordStreams(a: readonly DetRecord[], b: readonly DetRecord[]): Divergence | null {
  const shared = Math.min(a.length, b.length);
  for (let index = 0; index < shared; index++) {
    const left = a[index] as DetRecord;
    const right = b[index] as DetRecord;
    const at = { index, step: left.step, kind: left.kind, label: left.label };
    if (left.kind !== right.kind || left.label !== right.label) {
      return { ...at, path: '<record>', a: describe(left), b: describe(right) };
    }
    if (left.hash !== right.hash) {
      const found = firstDifference(recordBody(left), recordBody(right)) ?? {
        path: '$',
        a: `sha256 ${left.hash}`,
        b: `sha256 ${right.hash}`,
      };
      return { ...at, ...found };
    }
    if (
      left.rawHash !== null &&
      right.rawHash !== null &&
      left.masked === 0 &&
      right.masked === 0 &&
      left.rawHash !== right.rawHash
    ) {
      return { ...at, path: '$<bytes>', a: `sha256 ${left.rawHash}`, b: `sha256 ${right.rawHash}` };
    }
  }
  if (a.length === b.length) return null;
  const extra = (a.length > shared ? a[shared] : b[shared]) as DetRecord;
  const ended = (side: string, records: readonly DetRecord[]): string =>
    records.length > shared ? describe(records[shared] as DetRecord) : `<replay ${side} ended after ${records.length} records>`;
  return {
    index: shared,
    step: extra.step,
    kind: extra.kind,
    label: extra.label,
    path: '<end of stream>',
    a: ended('A', a),
    b: ended('B', b),
  };
}

export interface DivergenceMeta {
  scenario: string;
  seed: number;
  iterations: number;
  fixture: string;
  /** The knob value that reproduces it: `1` or `2sessions`. */
  knob: string;
  /** The last drawn action at or before the divergence, masked, if any. */
  lastAction: string | null;
}

export function formatDivergence(meta: DivergenceMeta, divergence: Divergence): string {
  return [
    `determinism: two replays of seed ${meta.seed} diverged at trace step ${divergence.step} ` +
      `(record #${divergence.index + 1}, ${divergence.kind} "${divergence.label}")`,
    `  first differing path: ${divergence.path}`,
    `  replay A: ${divergence.a}`,
    `  replay B: ${divergence.b}`,
    ...(meta.lastAction === null ? [] : [`  last drawn action: ${meta.lastAction}`]),
    `  reproduce: STALAGMIGHT_FUZZ_DETERMINISM=${meta.knob} STALAGMIGHT_FUZZ_FIXTURE=${meta.fixture} ` +
      `pnpm agent:scenario ${meta.scenario} --seed ${meta.seed} --iterations ${meta.iterations}`,
  ].join('\n');
}

// ---------------------------------------------------------------------------
// Canonical forms of what the app writes
// ---------------------------------------------------------------------------

const TEXT_EXTENSIONS = new Set(['csv', 'tmlu', 'svg', 'xml', 'txt', 'gpx', 'kml', 'dxf']);

function parseJsonOrText(text: string): unknown {
  const head = text.trimStart();
  if (head.startsWith('{') || head.startsWith('[')) {
    try {
      return { json: JSON.parse(text) as unknown };
    } catch {
      // Not JSON after all: compared as text.
    }
  }
  return text;
}

/**
 * A sqlite cell as compared: a gzip blob (the undo/redo payloads) is inflated
 * and parsed, JSON text is parsed, any other blob is hashed. Masking happens
 * afterwards, by column name and inside the parsed payload.
 */
export function decodeCell(value: unknown): unknown {
  if (value instanceof Uint8Array) {
    const bytes = Buffer.from(value.buffer, value.byteOffset, value.byteLength);
    if (bytes.length >= 2 && bytes[0] === 0x1f && bytes[1] === 0x8b) {
      try {
        return { gzip: parseJsonOrText(zlib.gunzipSync(bytes).toString('utf8')) };
      } catch {
        // A truncated or foreign gzip stream: hashed like any other blob.
      }
    }
    return { blob: sha256(bytes), size: bytes.length };
  }
  if (typeof value === 'string') return parseJsonOrText(value);
  return value;
}

/** Zip extra-field ids that carry file times (extended timestamp, NTFS). */
const TIME_EXTRA_IDS = new Set([0x5455, 0x000a]);

function zipExtra(bytes: Buffer): { fields: unknown[]; masked: number } {
  const fields: unknown[] = [];
  let masked = 0;
  let at = 0;
  while (at + 4 <= bytes.length) {
    const id = bytes.readUInt16LE(at);
    const size = bytes.readUInt16LE(at + 2);
    const data = bytes.subarray(at + 4, at + 4 + size);
    if (TIME_EXTRA_IDS.has(id)) masked += 1;
    fields.push({ id, data: TIME_EXTRA_IDS.has(id) ? WALL_CLOCK_MASK : data.toString('hex') });
    at += 4 + size;
  }
  return { fields, masked };
}

/** Every entry of a zip, with its content; null when it is not a well-formed zip. */
function zipEntries(bytes: Buffer): { body: unknown; masked: number } | null {
  let end = -1;
  for (let at = bytes.length - 22; at >= Math.max(0, bytes.length - 65557); at--) {
    if (bytes.readUInt32LE(at) === 0x06054b50) {
      end = at;
      break;
    }
  }
  if (end < 0) return null;
  const count = bytes.readUInt16LE(end + 10);
  let at = bytes.readUInt32LE(end + 16);
  let masked = 0;
  const entries: unknown[] = [];
  for (let n = 0; n < count; n++) {
    if (at + 46 > bytes.length || bytes.readUInt32LE(at) !== 0x02014b50) return null;
    const method = bytes.readUInt16LE(at + 10);
    const compressedSize = bytes.readUInt32LE(at + 20);
    const nameLength = bytes.readUInt16LE(at + 28);
    const extraLength = bytes.readUInt16LE(at + 30);
    const commentLength = bytes.readUInt16LE(at + 32);
    const local = bytes.readUInt32LE(at + 42);
    const name = bytes.subarray(at + 46, at + 46 + nameLength).toString('utf8');
    const extra = zipExtra(bytes.subarray(at + 46 + nameLength, at + 46 + nameLength + extraLength));
    if (local + 30 > bytes.length || bytes.readUInt32LE(local) !== 0x04034b50) return null;
    const dataStart = local + 30 + bytes.readUInt16LE(local + 26) + bytes.readUInt16LE(local + 28);
    const raw = bytes.subarray(dataStart, dataStart + compressedSize);
    let content: unknown;
    try {
      content =
        method === 0
          ? raw.toString('utf8').split('\n')
          : method === 8
            ? zlib.inflateRawSync(raw).toString('utf8').split('\n')
            : { sha256: sha256(raw) };
    } catch {
      content = { undecodable: sha256(raw) };
    }
    // The DOS time and date (offsets 12 and 14) are the entry's mtime: masked.
    masked += 1 + extra.masked;
    entries.push({
      name,
      method,
      flags: bytes.readUInt16LE(at + 8),
      crc32: bytes.readUInt32LE(at + 16),
      compressedSize,
      size: bytes.readUInt32LE(at + 24),
      extra: extra.fields,
      content,
    });
    at += 46 + nameLength + extraLength + commentLength;
  }
  return { body: { zip: entries }, masked };
}

function pngChunks(bytes: Buffer): { body: unknown; masked: number } {
  const chunks: unknown[] = [];
  let masked = 0;
  let at = 8;
  while (at + 12 <= bytes.length) {
    const length = bytes.readUInt32BE(at);
    const type = bytes.subarray(at + 4, at + 8).toString('latin1');
    if (type === 'tIME') {
      masked += 1;
      chunks.push({ type, data: WALL_CLOCK_MASK });
    } else {
      chunks.push({ type, length, sha256: sha256(bytes.subarray(at + 8, at + 8 + length)) });
    }
    at += 12 + length;
  }
  return { body: { png: chunks }, masked };
}

function pdfText(bytes: Buffer): { body: unknown; masked: number } {
  let masked = 0;
  const text = bytes
    .toString('latin1')
    .replace(/\/(CreationDate|ModDate)\s*\(([^)]*)\)/g, (_match, key: string) => {
      masked += 1;
      return `/${key} (${WALL_CLOCK_MASK})`;
    })
    .replace(/<(xmp:CreateDate|xmp:ModifyDate|xmp:MetadataDate)>[^<]*<\/\1>/g, (_match, tag: string) => {
      masked += 1;
      return `<${tag}>${WALL_CLOCK_MASK}</${tag}>`;
    });
  return { body: { pdf: text.split('\n') }, masked };
}

/**
 * An exported file's canonical form, by content first (zip, PNG, PDF) and by
 * extension otherwise. `masked` counts only the format-level masks (times);
 * paths and uuids are masked when the body goes through the replay's masker.
 */
export function canonicalFile(bytes: Buffer, extension: string): { body: unknown; masked: number } {
  const ext = extension.toLowerCase().replace(/^\./, '');
  if (bytes.length >= 4 && bytes.readUInt32LE(0) === 0x04034b50) {
    const zip = zipEntries(bytes);
    if (zip !== null) return zip;
  }
  if (bytes.length >= 8 && bytes.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) {
    return pngChunks(bytes);
  }
  if (bytes.subarray(0, 5).toString('latin1') === '%PDF-') return pdfText(bytes);
  if (ext === 'json') {
    try {
      return { body: { json: JSON.parse(bytes.toString('utf8')) as unknown }, masked: 0 };
    } catch {
      // Not parseable: compared as text below.
    }
  }
  if (ext === 'json' || TEXT_EXTENSIONS.has(ext)) return { body: { lines: bytes.toString('utf8').split('\n') }, masked: 0 };
  return { body: { sha256: sha256(bytes), size: bytes.length }, masked: 0 };
}

/**
 * The planted difference behind `STALAGMIGHT_FUZZ_DETERMINISM_PLANT`: nudge the
 * first non-integer number (depth first, keys sorted) by 1e-6, or the first
 * number by 1 when there is none. Returns the path it changed, or null.
 */
export function plantDifference(value: unknown, path = '$'): string | null {
  const visit = (node: unknown, at: string, wantFloat: boolean): string | null => {
    if (Array.isArray(node)) {
      for (let index = 0; index < node.length; index++) {
        const item: unknown = node[index];
        if (typeof item === 'number' && (!wantFloat || !Number.isInteger(item))) {
          node[index] = item + (wantFloat ? 1e-6 : 1);
          return `${at}[${index}]`;
        }
        const found = visit(item, `${at}[${index}]`, wantFloat);
        if (found !== null) return found;
      }
      return null;
    }
    if (node !== null && typeof node === 'object') {
      const record = node as Record<string, unknown>;
      for (const key of Object.keys(record).sort()) {
        const item = record[key];
        const next = IDENTIFIER.test(key) ? `${at}.${key}` : `${at}[${JSON.stringify(key)}]`;
        if (typeof item === 'number' && (!wantFloat || !Number.isInteger(item))) {
          record[key] = item + (wantFloat ? 1e-6 : 1);
          return next;
        }
        const found = visit(item, next, wantFloat);
        if (found !== null) return found;
      }
    }
    return null;
  };
  return visit(value, path, true) ?? visit(value, path, false);
}
