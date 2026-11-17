import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import en from '../../i18n/locales/en.json';
import { ONBOARDING_FLOWS } from '../flows';

/**
 * Every tour step must point at strings and elements that exist. A missing
 * i18n key renders the raw key; a stale `data-testid` anchor makes the step
 * wait for an element that never mounts and fall back to a centered card.
 * Neither fails anywhere else, so this pins both against the source tree.
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SRC_ROOT = path.resolve(__dirname, '../../');

const TESTID_SELECTOR = /^\[data-testid="([^"]+)"\]$/;
// The attribute itself, or a `testId` prop a component forwards to it (as
// DisplayMenu's toggles do). The lookbehind skips selector strings such as
// flows.ts's own `[data-testid="X"]`, so only a rendered value counts.
const ATTR = String.raw`(?<![\w\[-])(?:data-testid|testId)=`;
const LITERAL_ATTR = new RegExp(`${ATTR}(?:"([^"]+)"|'([^']+)'|\\{\\s*(?:"([^"]+)"|'([^']+)')\\s*\\})`, 'g');
const TEMPLATE_ATTR = new RegExp(`${ATTR}\\{\\s*\`([^\`]+)\`\\s*\\}`, 'g');

function sourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === '__tests__' ? [] : sourceFiles(full);
    return /\.tsx?$/.test(entry.name) && !/\.test\./.test(entry.name) ? [full] : [];
  });
}

function collectTestIds(): { literals: Set<string>; templates: RegExp[] } {
  const literals = new Set<string>();
  const templates: RegExp[] = [];
  for (const file of sourceFiles(SRC_ROOT)) {
    const source = fs.readFileSync(file, 'utf8');
    for (const m of source.matchAll(LITERAL_ATTR)) {
      const id = m[1] ?? m[2] ?? m[3] ?? m[4];
      if (id) literals.add(id);
    }
    for (const m of source.matchAll(TEMPLATE_ATTR)) {
      const parts = (m[1] ?? '').split(/\$\{[^}]*\}/);
      if (parts.length === 1) {
        literals.add(parts[0] ?? '');
        continue;
      }
      // A template with no literal word (`${a}-${b}`) would match almost any id; skip it.
      if (!/[A-Za-z0-9]/.test(parts.join(''))) continue;
      const escaped = parts.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
      templates.push(new RegExp(`^${escaped.join('[A-Za-z0-9_.:-]+')}$`));
    }
  }
  return { literals, templates };
}

function lookup(key: string): unknown {
  return key.split('.').reduce<unknown>(
    (node, part) => (node !== null && typeof node === 'object' ? (node as Record<string, unknown>)[part] : undefined),
    en
  );
}

const steps = ONBOARDING_FLOWS.flatMap((flow) => flow.steps.map((step) => ({ flow, step })));

describe('onboarding flow targets', () => {
  it('every title, description and bullet key exists in en.json', () => {
    const missing: string[] = [];
    for (const { flow, step } of steps) {
      if (typeof lookup(flow.titleKey) !== 'string') missing.push(`${flow.id}: ${flow.titleKey}`);
      for (const key of [step.titleKey, step.descriptionKey]) {
        if (typeof lookup(key) !== 'string') missing.push(`${flow.id}/${step.id}: ${key}`);
      }
      if (step.descriptionItemsKey !== undefined) {
        const items = lookup(step.descriptionItemsKey);
        if (!Array.isArray(items) || items.length === 0) {
          missing.push(`${flow.id}/${step.id}: ${step.descriptionItemsKey}`);
        }
      }
    }
    expect(missing).toEqual([]);
  });

  it('every data-testid anchor is rendered somewhere under src/', () => {
    const { literals, templates } = collectTestIds();
    const exists = (id: string) => literals.has(id) || templates.some((re) => re.test(id));
    const selectors = steps.flatMap(({ flow, step }) =>
      [step.targetSelector, ...(step.targetSelectors ?? []), step.tooltipTargetSelector]
        .filter((s): s is string => s !== undefined)
        .map((selector) => ({ where: `${flow.id}/${step.id}`, selector }))
    );
    const checked = selectors.flatMap(({ where, selector }) => {
      const id = TESTID_SELECTOR.exec(selector)?.[1];
      return id === undefined ? [] : [{ where, id }];
    });

    expect(checked.length).toBeGreaterThan(0);
    expect(checked.filter(({ id }) => !exists(id)).map(({ where, id }) => `${where}: ${id}`)).toEqual([]);
  });
});
