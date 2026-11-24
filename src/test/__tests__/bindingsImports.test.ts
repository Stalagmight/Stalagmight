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

describe('the bindings tree (RS-PROSE fix 8)', () => {
  it('imports no file it does not hold, but the documented ones', () => {
    const dangling: string[] = [];
    for (const file of readdirSync(BINDINGS).filter((f) => f.endsWith('.ts'))) {
      const text = readFileSync(join(BINDINGS, file), 'utf8');
      for (const match of text.matchAll(/from "\.\/([A-Za-z0-9_]+)"/g)) {
        if (!existsSync(join(BINDINGS, `${match[1]}.ts`))) dangling.push(`${file} -> ${match[1]}`);
      }
    }
    expect(dangling.filter((d) => !KNOWN.has(d))).toEqual([]);
  });
});
