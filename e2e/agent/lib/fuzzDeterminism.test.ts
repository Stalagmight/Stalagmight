import * as zlib from 'zlib';
import { describe, expect, it } from 'vitest';

import {
  Masker,
  WALL_CLOCK_MASK,
  canonicalFile,
  compareRecordStreams,
  decodeCell,
  firstDifference,
  formatDivergence,
  makeRecord,
  plantDifference,
  sha256,
  type DetRecord,
  type DetRecordKind,
  type Divergence,
} from './fuzzDeterminism';
import { zipStore } from './surveyFileWriters';

const TMP = '/tmp';

/** A replay's stream, built the way the probe builds it: one masker, records in order. */
function stream(entries: readonly [DetRecordKind, string, unknown, Buffer?][], roots: string[] = []): DetRecord[] {
  const masker = new Masker(TMP);
  for (const root of roots) masker.addRoot(root);
  let step = 0;
  return entries.map(([kind, label, value, raw]) => {
    if (kind === 'action') step += 1;
    if (raw !== undefined) {
      const file = canonicalFile(raw, label.split('.').pop() ?? '');
      const canonical = masker.value(file.body);
      return makeRecord(kind, label, step, canonical, file.masked + masker.takeCount(), sha256(raw));
    }
    const canonical = masker.value(value);
    return makeRecord(kind, label, step, canonical, masker.takeCount());
  });
}

const UUID_A1 = '0f8fad5b-d9cb-469f-a165-70867728950e';
const UUID_A2 = '7c9e6679-7425-40de-944b-e07fc1f90ae7';
const UUID_B1 = 'a3bb189e-8bf9-3888-9912-ace4e6543002';
const UUID_B2 = 'e6f1c2b4-1a2b-4c3d-8e9f-0a1b2c3d4e5f';

function project(opts: { modified: string; merged: string; length: number; uuids: [string, string] }): unknown {
  return {
    project: {
      ok: {
        name: 'synthetic',
        modified_at: opts.modified,
        created_at: '2026-01-01T00:00:00Z',
        file_path: '/srv/sessions/run-1/fixtures/synthetic.stalx',
        sections: [
          { id: 1, shots: [{ id: 10, uuid: opts.uuids[0], length: 3.25 }] },
          { id: 2, shots: [{ id: 20, uuid: opts.uuids[1], length: opts.length }] },
        ],
        station_merges: [{ a: [1, 1], b: [2, 0], merged_at: opts.merged }],
      },
    },
    undo: { ok: [{ description: 'Create shot', timestamp: opts.modified }] },
    redo: { ok: [] },
  };
}

describe('Masker', () => {
  it('masks every wall-clock key but keeps null as null and leaves other keys alone', () => {
    const masker = new Masker(null);
    const out = masker.value({
      modified_at: '2026-11-05T10:00:00Z',
      merged_at: null,
      nested: [{ executed_at: '2026-11-05 10:00:01', date: '2026-11-05', name: 'x' }],
    });
    expect(out).toEqual({
      merged_at: null,
      modified_at: WALL_CLOCK_MASK,
      nested: [{ date: '2026-11-05', executed_at: WALL_CLOCK_MASK, name: 'x' }],
    });
    // `date` is survey data, not a wall-clock stamp: it is NOT in the set.
    expect(masker.takeCount()).toBe(2);
  });

  it('renames uuids by first appearance, so two replays with different random uuids agree', () => {
    const a = new Masker(null).value({ x: [UUID_A1, UUID_A2, UUID_A1] });
    const b = new Masker(null).value({ x: [UUID_B1, UUID_B2, UUID_B1] });
    expect(a).toEqual({ x: ['<uuid#1>', '<uuid#2>', '<uuid#1>'] });
    expect(firstDifference(a, b)).toBeNull();
  });

  it('still sees a uuid that one replay kept and the other re-minted', () => {
    const kept = new Masker(null).value({ before: UUID_A1, after: UUID_A1 });
    const reminted = new Masker(null).value({ before: UUID_B1, after: UUID_B2 });
    expect(firstDifference(kept, reminted)).toEqual({ path: '$.before', a: expect.any(String), b: expect.any(String) });
  });

  it('masks a registered root and the random tail of an mkdtemp dir, nothing else of the path', () => {
    const masker = new Masker('/tmp/');
    masker.addRoot('/srv/sessions/run-1');
    expect(masker.text('/srv/sessions/run-1/fuzz-exports/ep0/m1/a.csv')).toBe('<root>/fuzz-exports/ep0/m1/a.csv');
    expect(masker.text('saved to /tmp/stalagmight-fuzz-save-Ab3dE9/fuzz-roundtrip.stalx')).toBe(
      'saved to <tmp>/stalagmight-fuzz-save-XXXXXX/fuzz-roundtrip.stalx',
    );
    expect(masker.text('/home/user/data/a.stalx')).toBe('/home/user/data/a.stalx');
  });

  it('sorts object keys, so a map iteration order is not a difference', () => {
    expect(JSON.stringify(new Masker(null).value({ b: 1, a: { d: 2, c: 3 } }))).toBe('{"a":{"c":3,"d":2},"b":1}');
  });
});

describe('compareRecordStreams', () => {
  const opened = { modified: '2026-11-05T10:00:00Z', merged: '2026-11-05T10:00:00Z', length: 4.5 };

  it('ignores masked timestamps, random uuids and the session dir', () => {
    const a = stream(
      [
        ['action', 'trace step 1', { kind: 'mutate', action: 'createShot', args: [2, { length: 4.5 }] }],
        ['state', 'after fused step 1', project({ ...opened, uuids: [UUID_A1, UUID_A2] })],
      ],
      ['/srv/sessions/run-1'],
    );
    const later = { modified: '2026-11-05T10:09:59Z', merged: '2026-11-05T10:09:58Z', length: 4.5 };
    const bState = JSON.parse(
      JSON.stringify(project({ ...later, uuids: [UUID_B1, UUID_B2] })).replace(/run-1/g, 'run-2'),
    ) as unknown;
    const b = stream(
      [
        ['action', 'trace step 1', { kind: 'mutate', action: 'createShot', args: [2, { length: 4.5 }] }],
        ['state', 'after fused step 1', bState],
      ],
      ['/srv/sessions/run-2'],
    );
    expect(compareRecordStreams(a, b)).toBeNull();
  });

  it('catches a planted difference and names the step, the record and the first differing path', () => {
    const a = stream([
      ['action', 'trace step 1', { kind: 'mutate' }],
      ['state', 'after fused step 1', project({ ...opened, uuids: [UUID_A1, UUID_A2] })],
      ['action', 'trace step 2', { kind: 'undo' }],
      ['state', 'after fused step 2', project({ ...opened, uuids: [UUID_A1, UUID_A2] })],
    ]);
    const b = stream([
      ['action', 'trace step 1', { kind: 'mutate' }],
      ['state', 'after fused step 1', project({ ...opened, uuids: [UUID_B1, UUID_B2] })],
      ['action', 'trace step 2', { kind: 'undo' }],
      ['state', 'after fused step 2', project({ ...opened, length: 4.5000001, uuids: [UUID_B1, UUID_B2] })],
    ]);
    const found = compareRecordStreams(a, b);
    expect(found).toMatchObject({
      index: 3,
      step: 2,
      kind: 'state',
      label: 'after fused step 2',
      path: '$.project.ok.sections[1].shots[0].length',
      a: '4.5',
      b: '4.5000001',
    });
    const text = formatDivergence(
      { scenario: 'undo-redo-fuzz', seed: 7, iterations: 40, fixture: 'naharon.stalx', knob: '1', lastAction: '{"kind":"undo"}' },
      found as Divergence,
    );
    expect(text).toContain('seed 7 diverged at trace step 2');
    expect(text).toContain('first differing path: $.project.ok.sections[1].shots[0].length');
    expect(text).toContain(
      'reproduce: STALAGMIGHT_FUZZ_DETERMINISM=1 STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx pnpm agent:scenario undo-redo-fuzz --seed 7 --iterations 40',
    );
  });

  it('a different drawn action is a difference in the trace itself', () => {
    const a = stream([['action', 'trace step 1', { kind: 'mutate', action: 'createShot', args: [2] }]]);
    const b = stream([['action', 'trace step 1', { kind: 'mutate', action: 'createShot', args: [3] }]]);
    expect(compareRecordStreams(a, b)).toMatchObject({ kind: 'action', step: 1, path: '$.args[0]', a: '2', b: '3' });
  });

  it('reports a record one replay took and the other did not, and a stream that ended early', () => {
    const a = stream([
      ['action', 'trace step 1', {}],
      ['file', 'export move 1 file', { x: 1 }],
    ]);
    const b = stream([
      ['action', 'trace step 1', {}],
      ['state', 'after export move 1', { x: 1 }],
    ]);
    expect(compareRecordStreams(a, b)).toMatchObject({ index: 1, path: '<record>' });
    const short = stream([['action', 'trace step 1', {}]]);
    expect(compareRecordStreams(a, short)).toMatchObject({
      index: 1,
      path: '<end of stream>',
      b: '<replay B ended after 1 records>',
    });
  });
});

describe('saved .stalx rows', () => {
  const gz = (value: unknown): Uint8Array => zlib.gzipSync(Buffer.from(JSON.stringify(value)));

  it('masks executed_at and the timestamps INSIDE a gzip undo payload, and nothing else', () => {
    const row = (executed: string, merged: string, shotId: number): unknown => ({
      id: 1,
      sequence: 0,
      executed_at: executed,
      undo_data_json: decodeCell(gz({ MergedStations: { merged_at: merged, shot_id: shotId } })),
      command_json: decodeCell(gz({ MergeStations: { a: 1 } })),
    });
    const a = stream([['table', 'saved: table undo_stack', { rows: [row('2026-11-05 10:00', '2026-11-05T10:00Z', 7)] }]]);
    const b = stream([['table', 'saved: table undo_stack', { rows: [row('2026-11-05 11:00', '2026-11-05T11:00Z', 7)] }]]);
    expect(compareRecordStreams(a, b)).toBeNull();
    const c = stream([['table', 'saved: table undo_stack', { rows: [row('2026-11-05 11:00', '2026-11-05T11:00Z', 8)] }]]);
    expect(compareRecordStreams(a, c)).toMatchObject({
      path: '$.rows[0].undo_data_json.gzip.json.MergedStations.shot_id',
      a: '7',
      b: '8',
    });
  });

  it('hashes a blob that is not gzip', () => {
    expect(decodeCell(Uint8Array.from([1, 2, 3]))).toEqual({ blob: sha256(Uint8Array.from([1, 2, 3])), size: 3 });
  });
});

describe('exported files', () => {
  const text = (value: string): Buffer => Buffer.from(value, 'utf8');

  it('a JSON export differing only in exported_at is equal', () => {
    const a = stream([['file', 'export.json', null, text('{"exported_at":"2026-11-05T10:00:00Z","shots":[1,2]}')]]);
    const b = stream([['file', 'export.json', null, text('{"exported_at":"2026-11-05T10:05:00Z","shots":[1,2]}')]]);
    expect(compareRecordStreams(a, b)).toBeNull();
  });

  it('bytes must match when nothing was masked: key order in a JSON export is a difference', () => {
    const a = stream([['file', 'export.json', null, text('{"a":1,"b":2}')]]);
    const b = stream([['file', 'export.json', null, text('{"b":2,"a":1}')]]);
    expect(compareRecordStreams(a, b)).toMatchObject({ path: '$<bytes>' });
  });

  it('a CSV difference is located by line', () => {
    const a = stream([['file', 'export.csv', null, text('h1,h2\n1,2\n3,4\n')]]);
    const b = stream([['file', 'export.csv', null, text('h1,h2\n1,2\n3,5\n')]]);
    expect(compareRecordStreams(a, b)).toMatchObject({ path: '$.lines[2]' });
  });

  it('a zip (.tml) differing only in entry times is equal; a content difference is located', () => {
    const zip = (body: string, dosTime: number): Buffer => {
      const bytes = Buffer.from(zipStore([{ name: 'Data.xml', data: text(body) }]));
      for (let at = 0; at + 4 <= bytes.length; at++) {
        if (bytes.readUInt32LE(at) === 0x04034b50) bytes.writeUInt16LE(dosTime, at + 10);
        if (bytes.readUInt32LE(at) === 0x02014b50) bytes.writeUInt16LE(dosTime, at + 12);
      }
      return bytes;
    };
    const a = stream([['file', 'export.tml', null, zip('<a>\n<b>1</b>\n</a>', 0x1234)]]);
    const b = stream([['file', 'export.tml', null, zip('<a>\n<b>1</b>\n</a>', 0x5678)]]);
    expect(compareRecordStreams(a, b)).toBeNull();
    const c = stream([['file', 'export.tml', null, zip('<a>\n<b>2</b>\n</a>', 0x5678)]]);
    const found = compareRecordStreams(a, c);
    expect(found?.path).toMatch(/^\$\.zip\[0\]\.(content\[1\]|crc32)$/);
  });

  it('PNG: a tIME chunk is masked, a pixel difference is not', () => {
    const chunk = (type: string, data: Buffer): Buffer => {
      const head = Buffer.alloc(8);
      head.writeUInt32BE(data.length, 0);
      head.write(type, 4, 'latin1');
      return Buffer.concat([head, data, Buffer.alloc(4)]);
    };
    const png = (time: number, pixel: number): Buffer =>
      Buffer.concat([
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        chunk('IHDR', Buffer.alloc(13)),
        chunk('tIME', Buffer.from([time])),
        chunk('IDAT', Buffer.from([pixel])),
      ]);
    expect(compareRecordStreams(stream([['file', 'a.png', null, png(1, 9)]]), stream([['file', 'a.png', null, png(2, 9)]]))).toBeNull();
    expect(
      compareRecordStreams(stream([['file', 'a.png', null, png(1, 9)]]), stream([['file', 'a.png', null, png(1, 8)]])),
    ).toMatchObject({ path: '$.png[2].sha256' });
  });

  it('PDF: /CreationDate and /ModDate are masked', () => {
    const pdf = (date: string): Buffer => text(`%PDF-1.7\n<< /CreationDate (D:${date}) /ModDate (D:${date}) >>\n%%EOF`);
    const a = stream([['file', 'a.pdf', null, pdf('20261105100000Z')]]);
    const b = stream([['file', 'a.pdf', null, pdf('20261105110000Z')]]);
    expect(compareRecordStreams(a, b)).toBeNull();
  });
});

describe('plantDifference', () => {
  it('nudges the first non-integer number, keys in sorted order', () => {
    const value = { b: { length: 2.5 }, a: { id: 3, ratio: 0.25 } };
    expect(plantDifference(value)).toBe('$.a.ratio');
    expect(value.a.ratio).toBeCloseTo(0.250001, 9);
    expect(value.b.length).toBe(2.5);
  });

  it('falls back to the first integer, and to null when there is no number at all', () => {
    const value = { list: [{ id: 3 }] };
    expect(plantDifference(value)).toBe('$.list[0].id');
    expect(value.list[0]?.id).toBe(4);
    expect(plantDifference({ name: 'x' })).toBeNull();
  });
});
