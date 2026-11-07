/**
 * The agent harness's copy of `ensureExtension` must not drift from this one.
 *
 * `e2e/agent/lib/exportOracles.ts` reimplements `ensureExtension` because
 * `tsconfig.agent.json` cannot follow an import into `src/` (no `jsx`, and
 * `types: ["node"]` has no `import.meta.env`), and `export-roundtrip-fuzz` uses
 * the copy to work out where a DOM-route export ACTUALLY writes - the app
 * rewrites the extension of whatever the save dialog answered. A copy that
 * drifted would leave the fuzzer checking a path the app never wrote to, which
 * looks exactly like every export silently failing.
 *
 * The dependency direction is the one that works: this test lives under `src/`,
 * where vitest and `tsconfig.json` are happy to reach OUT to the harness, and
 * `e2e/agent/lib/exportOracles.ts` is a pure module with no imports at all. So
 * the two implementations are compared directly rather than against a
 * transcribed table, and the harness tier still imports nothing from `src/`.
 */
import { describe, expect, it } from 'vitest';

import { ensureExtension } from '../fileDialogs';
import { ensureExtensionMirror, finalizeExportPath } from '../../../e2e/agent/lib/exportOracles';

/** Paths chosen to hit each branch of the real function, plus the fuzzer's shapes. */
const PATHS = [
  '/x/out.csv',
  '/x/out.CSV',
  '/x/out',
  '/x/out.txt',
  '/x/out.tml.txt',
  '/x/out.',
  '/x/.hidden',
  '/a.b/out',
  '/.agent-artifacts/sessions/s1/out',
  '/x/cave.stalx',
  '/x/cave.plnx',
  '/x/EXPORT.TMLU',
  '/x/out 100% "quoted".pdf',
  '/x/out-\u202e\u6d1e\u7a74.svg',
  `/x/${'n'.repeat(200)}.png`,
  'C:\\x\\out.txt',
  'C:\\x\\.svg',
  '\\\\srv\\share\\out.svg',
  '/x/CaveA\\.svg',
  '/x/CaveA\\.',
  '/x/a\\b.tml',
  'relative/out',
  'bare',
];

const EXTENSIONS = ['csv', 'json', 'tml', 'tmlu', 'png', 'svg', 'pdf', 'xml', '.png', 'stalx'];

describe('the harness mirror of ensureExtension', () => {
  it('agrees with the real function on every path x extension pair', () => {
    for (const path of PATHS) {
      for (const extension of EXTENSIONS) {
        expect(ensureExtensionMirror(path, extension), `${path} + ${extension}`).toBe(
          ensureExtension(path, extension),
        );
      }
    }
  });

  it('agrees on the property the fuzzer depends on - replace a known extension, append to anything else', () => {
    expect(ensureExtension('/x/out.png', 'csv')).toBe('/x/out.csv');
    expect(ensureExtensionMirror('/x/out.png', 'csv')).toBe('/x/out.csv');
    expect(ensureExtension('/x/out.txt', 'csv')).toBe('/x/out.txt.csv');
    expect(ensureExtensionMirror('/x/out.txt', 'csv')).toBe('/x/out.txt.csv');
  });
});

describe('finalizeExportPath mirrors ExportControls.handleExport', () => {
  // Transcribed from the component: TML keeps a `.tml`/`.tmlu` the user already
  // typed and otherwise forces `.tml`; every other data format and every image
  // format goes straight through `ensureExtension`.
  const handleExportPathForTml = (selected: string): string => {
    const lower = selected.toLowerCase();
    if (lower.endsWith('.tmlu')) return selected;
    if (lower.endsWith('.tml')) return selected;
    return ensureExtension(selected, 'tml');
  };

  it('matches the component on the TML branch', () => {
    for (const path of PATHS) {
      expect(finalizeExportPath(path, 'tml'), path).toBe(handleExportPathForTml(path));
    }
  });

  it('matches the component on the csv/json and image branches', () => {
    for (const path of PATHS) {
      expect(finalizeExportPath(path, 'csv')).toBe(ensureExtension(path, 'csv'));
      expect(finalizeExportPath(path, 'json')).toBe(ensureExtension(path, 'json'));
      expect(finalizeExportPath(path, 'png')).toBe(ensureExtension(path, 'png'));
      expect(finalizeExportPath(path, 'svg')).toBe(ensureExtension(path, 'svg'));
      expect(finalizeExportPath(path, 'pdf')).toBe(ensureExtension(path, 'pdf'));
    }
  });
});
