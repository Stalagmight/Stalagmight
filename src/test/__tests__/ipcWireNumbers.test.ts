import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * No generated IPC type may declare `bigint` (#576 C6).
 *
 * ts-rs 7 declares every Rust `u64`/`i64` as `bigint`, but no IPC path in this
 * app delivers one: `invoke()` results and events are JSON (serde_json writes
 * the integer as a JSON number) and the msgpack load envelope is decoded with
 * `@msgpack/msgpack`'s default `useBigInt64: false`, which yields numbers too.
 * A declared `bigint` therefore lies about the wire, and code that trusts it
 * misbehaves only against the real backend - `0 === 0n` is false, so Clear
 * cache never disabled (CacheSettings) - while the Playwright mock, which
 * speaks the declared type, returned real bigints and stayed green.
 *
 * Every such field carries `#[ts(type = "number")]` instead (all of them stay
 * far below 2^53, where a JS number is exact), and `pnpm gen:ipc-types`
 * refuses to write a `bigint`. This test pins the committed tree.
 */

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const IPC_DIR = path.resolve(__dirname, '../../types/ipc');

/** `File.ts: field` for every field whose declared type mentions `bigint`. */
function bigintFields(): string[] {
  const found: string[] = [];
  for (const name of fs.readdirSync(IPC_DIR).sort()) {
    if (!name.endsWith('.ts')) continue;
    const source = fs.readFileSync(path.join(IPC_DIR, name), 'utf8');
    for (const match of source.matchAll(/(\w+)\??: [^,;}]*\bbigint\b/g)) {
      found.push(`${name}: ${match[1]}`);
    }
  }
  return found;
}

describe('generated IPC types match the JSON wire (#576 C6)', () => {
  it('reads the generated tree', () => {
    expect(fs.readdirSync(IPC_DIR).filter((n) => n.endsWith('.ts')).length).toBeGreaterThan(100);
  });

  it('declares no field as bigint', () => {
    const fields = bigintFields();
    expect(fields, `${fields.length} generated field(s) declared bigint; the wire delivers number`).toEqual([]);
  });
});
