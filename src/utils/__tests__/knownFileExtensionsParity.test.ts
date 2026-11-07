/**
 * The three copies of "every file type Stalagmight reads or writes" must agree
 * (#576 RV2-FE-4).
 *
 * `ensureExtension` (src/utils/fileDialogs.ts), the persistence crate's
 * `with_stalx_extension` (open.rs) and the agent harness's mirror
 * (e2e/agent/lib/exportOracles.ts) each REPLACE a trailing extension that names
 * a known type and APPEND to any other (#576 D35). Each copy says it mirrors the
 * others, and only the harness mirror had a check (exportPathMirror.test.ts,
 * which compares behaviour, not the list). A type added to one list alone makes
 * the same typed name save differently through the frontend and the backend.
 *
 * The Rust and harness lists are read from source - the repo's source-scan
 * idiom (mockContract.test.ts reads lib.rs the same way) - and the scan fails
 * loudly if it cannot find the array, so renaming it cannot turn this into a
 * vacuous pass.
 */
import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { KNOWN_FILE_EXTENSIONS } from '../fileDialogs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../');
const RUST_FILE = 'src-tauri/crates/persistence/src/persistence/sqlite/open.rs';
const HARNESS_FILE = 'e2e/agent/lib/exportOracles.ts';

/** The quoted strings of the array literal that follows `anchor`, or a thrown error. */
function arrayAfter(rel: string, anchor: RegExp): string[] {
  const source = fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8');
  const match = anchor.exec(source);
  if (!match) throw new Error(`${rel}: no ${anchor} - renamed? Update this parity test with it.`);
  const open = source.indexOf('[', match.index + match[0].length - 1);
  const close = source.indexOf(']', open);
  const body = source.slice(open + 1, close);
  const items = [...body.matchAll(/["']([^"']*)["']/g)].map((m) => m[1] as string);
  if (items.length === 0) throw new Error(`${rel}: the array after ${anchor} is empty or unparsed`);
  return items;
}

const sorted = (xs: readonly string[]) => [...xs].sort();

describe('KNOWN_FILE_EXTENSIONS parity: frontend, backend and agent harness', () => {
  const ts = sorted(KNOWN_FILE_EXTENSIONS);
  const rust = sorted(arrayAfter(RUST_FILE, /const KNOWN_FILE_EXTENSIONS: &\[&str\] = &\[/));
  const harness = sorted(arrayAfter(HARNESS_FILE, /const KNOWN_FILE_EXTENSIONS_MIRROR: readonly string\[\] = \[/));

  it('the persistence crate (with_stalx_extension) knows exactly the frontend set', () => {
    expect(rust).toEqual(ts);
  });

  it('the agent-harness mirror knows exactly the frontend set', () => {
    expect(harness).toEqual(ts);
  });

  it('no list repeats an entry, and every entry is a bare lowercase extension', () => {
    for (const [name, list] of [['frontend', ts], ['rust', rust], ['harness', harness]] as const) {
      expect(new Set(list).size, `${name} has a duplicate`).toBe(list.length);
      for (const ext of list) expect(ext, `${name}: "${ext}"`).toMatch(/^[a-z0-9]+$/);
    }
  });
});
