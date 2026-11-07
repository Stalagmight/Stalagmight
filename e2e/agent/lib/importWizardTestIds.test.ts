/**
 * [#576 NF-W8D4-1] The import fuzzer reads the DMP wizard's phase from its test
 * ids: the screen digest classifies the screen by them and the dialog bridge
 * waits on its busy markers. An id the wizard no longer renders is a lookup that
 * can never match - `wizard-early-importing` and `wizard-no-suggestions` stayed
 * here after the wizard stopped rendering them - so its screen or wait silently
 * never happens. Every `wizard-*` id either file looks up must be a literal
 * `data-testid` in the app's components.
 *
 * Reading the sources rather than importing them is deliberate: both run in the
 * page, as serialized function bodies.
 */
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { describe, expect, it } from 'vitest';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const LOOKUP_FILES = ['e2e/agent/lib/importScreenDigest.ts', 'e2e/agent/scenarios/importDialogBridge.ts'];

/** Every literal `data-testid="..."` in the non-test component sources. */
function renderedTestIds(): Set<string> {
  const ids = new Set<string>();
  const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== '__tests__') walk(full);
      } else if (entry.name.endsWith('.tsx') && !entry.name.includes('.test.')) {
        for (const m of fs.readFileSync(full, 'utf8').matchAll(/data-testid=["']([\w-]+)["']/g)) ids.add(m[1]!); // group 1 always matches
      }
    }
  };
  walk(path.join(repo, 'src', 'components'));
  return ids;
}

describe('import fuzzer wizard lookups (#576 NF-W8D4-1)', () => {
  const rendered = renderedTestIds();

  it.each(LOOKUP_FILES)('%s looks up only wizard test ids the wizard renders', (file) => {
    const source = fs.readFileSync(path.join(repo, file), 'utf8');
    const looked = [...source.matchAll(/['"`](wizard-[\w-]+)['"`]/g)].map((m) => m[1]!); // group 1 always matches
    expect(looked.length, `no wizard-* lookups read from ${file}: the pattern no longer matches`).toBeGreaterThan(0);
    expect([...new Set(looked)].filter((id) => !rendered.has(id))).toEqual([]);
  });
});
