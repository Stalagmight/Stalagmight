/**
 * RS-PROSE fix 8: the ts-rs bindings tree (`src-tauri/bindings/`, written by
 * `cargo test --workspace`) imports only files it holds, but for the known
 * types that are deliberately never exported there (docs/ipc-typegen.md,
 * "Imports the tree does not hold"). `ConversionNoticeView.ts` imported a
 * `ProsePart` the tree did not hold. Injection: removing `export` from the
 * Prose types' `#[ts(...)]` (or deleting `bindings/ProsePart.ts`) fails it.
 */
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const BINDINGS = resolve(__dirname, '../../../src-tauri/bindings');

/** `file -> import` pairs documented in docs/ipc-typegen.md as never exported here. */
const KNOWN = new Set([
  'GpxLinePlacement.ts -> ImportPlacement',
  'SpeleoFileAccess.ts -> SpeleoProjectSummary',
  'SpeleoLinkStatus.ts -> Permission',
  'TmlShot.ts -> SurveyCredit',
  'ProseStation.ts -> StationRef',
]);

/** Every `file -> import` of the tree whose import is not a file of it. */
export function danglingImports(dir: string): string[] {
  const dangling: string[] = [];
  for (const file of readdirSync(dir).filter((f) => f.endsWith('.ts'))) {
    const text = readFileSync(join(dir, file), 'utf8');
    for (const match of text.matchAll(/from "\.\/([A-Za-z0-9_]+)"/g)) {
      if (!existsSync(join(dir, `${match[1]}.ts`))) dangling.push(`${file} -> ${match[1]}`);
    }
  }
  return dangling;
}

/**
 * The allowlist must match the tree both ways: no unlisted dangling import,
 * and no listed one that has stopped dangling (delta fix 5: a stale entry
 * would let the same pair dangle again unnoticed). Injection: dropping the
 * second check fails "a stale entry".
 */
export function allowlistProblems(dangling: readonly string[], known: ReadonlySet<string>): string[] {
  return [
    ...dangling.filter((d) => !known.has(d)).map((d) => `not listed: ${d}`),
    ...[...known].filter((k) => !dangling.includes(k)).map((k) => `listed but resolved: ${k}`),
  ];
}

describe('the bindings tree (RS-PROSE fix 8)', () => {
  it('imports no file it does not hold, but exactly the documented ones', () => {
    expect(allowlistProblems(danglingImports(BINDINGS), KNOWN)).toEqual([]);
  });

  it('a stale entry (a pair that resolves now) is a failure too', () => {
    const dangling = danglingImports(BINDINGS);
    expect(allowlistProblems(dangling, new Set([...KNOWN, 'ProsePart.ts -> ProseLine']))).toEqual([
      'listed but resolved: ProsePart.ts -> ProseLine',
    ]);
  });
});
