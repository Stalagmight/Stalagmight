import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import en from '../../i18n/locales/en.json';
import { TOOLBAR_ITEMS } from '../../components/SingleSystemView/Toolbar';
import { ONBOARDING_FLOWS, type OnboardingIllustration } from '../flows';

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

/** Selectors allowed to be something other than one `[data-testid="X"]`. None today. */
const SELECTOR_ALLOWLIST: ReadonlySet<string> = new Set();

/**
 * Anchors rendered only through a template. Each names the template it comes
 * from and the finite domain its placeholder ranges over; an anchor must be
 * `prefix` + a domain member. Anything else needs a literal match.
 */
const TEMPLATE_DOMAINS: ReadonlyArray<{ prefix: string; template: string; domain: ReadonlySet<string> }> = [
  {
    // Toolbar's ToolButton, rendered only for a standalone toolbar item.
    prefix: 'tool-',
    template: 'tool-${tool}',
    domain: new Set(TOOLBAR_ITEMS.flatMap((item) => (item.kind === 'standalone' ? [item.id] : []))),
  },
];

const PLACEMENT_ONLY_ILLUSTRATIONS: ReadonlySet<OnboardingIllustration> = new Set([
  'map-with-pan-zoom',
  'mouse-pan-zoom',
]);

function sourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === '__tests__' ? [] : sourceFiles(full);
    return /\.tsx?$/.test(entry.name) && !/\.test\./.test(entry.name) ? [full] : [];
  });
}

function collectTestIds(): { literals: Set<string>; templates: Set<string> } {
  const literals = new Set<string>();
  const templates = new Set<string>();
  for (const file of sourceFiles(SRC_ROOT)) {
    const source = fs.readFileSync(file, 'utf8');
    for (const m of source.matchAll(LITERAL_ATTR)) {
      const id = m[1] ?? m[2] ?? m[3] ?? m[4];
      if (id) literals.add(id);
    }
    for (const m of source.matchAll(TEMPLATE_ATTR)) {
      const template = m[1] ?? '';
      (template.includes('${') ? templates : literals).add(template);
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

  it('every anchor is a [data-testid] that is rendered somewhere under src/', () => {
    const { literals, templates } = collectTestIds();
    const exists = (id: string) =>
      literals.has(id) ||
      TEMPLATE_DOMAINS.some(
        ({ prefix, template, domain }) =>
          templates.has(template) && id.startsWith(prefix) && domain.has(id.slice(prefix.length))
      );

    const failures = steps.flatMap(({ flow, step }) =>
      [step.targetSelector, ...(step.targetSelectors ?? []), step.tooltipTargetSelector]
        .filter((s): s is string => s !== undefined)
        .flatMap((selector) => {
          const where = `${flow.id}/${step.id}`;
          if (SELECTOR_ALLOWLIST.has(selector)) return [];
          const id = TESTID_SELECTOR.exec(selector)?.[1];
          if (id === undefined) return [`${where}: unsupported selector ${selector}`];
          return exists(id) ? [] : [`${where}: ${id}`];
        })
    );

    expect(failures).toEqual([]);
  });

  it('shows the placement-only pan illustrations only in the placement flow', () => {
    // They teach Ctrl+drag / middle-drag because a plain drag places there;
    // anywhere else a plain drag pans, so the card would be wrong.
    const misplaced = steps
      .filter(({ step }) => step.illustration !== undefined && PLACEMENT_ONLY_ILLUSTRATIONS.has(step.illustration))
      .filter(({ flow }) => flow.id !== 'placement-mode-map')
      .map(({ flow, step }) => `${flow.id}/${step.id}`);

    expect(misplaced).toEqual([]);
  });
});
