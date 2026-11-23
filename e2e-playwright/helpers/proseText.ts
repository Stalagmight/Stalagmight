import { expect, type Locator } from '@playwright/test';

/**
 * Text that names stations, shots and lines draws each name as a prose pill
 * (RS-PROSE, `src/components/common/Prose.tsx`): an aria-hidden drawing beside
 * an sr-only span with the plain words. `textContent` holds both, so
 * `toHaveText`/`toContainText` on such a sentence read every name twice.
 *
 * `spokenText` is what a screen reader reads instead: the element without its
 * aria-hidden subtrees, whitespace collapsed, which is the sentence plain text
 * wrote ("Delete 2 shots (stations 1 to 3 in Main Line).").
 */
export async function spokenText(locator: Locator): Promise<string> {
  return locator.evaluate((el) => {
    const copy = el.cloneNode(true) as HTMLElement;
    copy.querySelectorAll('[aria-hidden="true"]').forEach((node) => node.remove());
    return (copy.textContent ?? '').replace(/\s+/g, ' ').trim();
  });
}

/**
 * Waits until `locator`'s spoken text is `expected`: a string it contains (or,
 * with `exact`, equals), or a pattern it matches.
 */
export async function expectSpoken(
  locator: Locator,
  expected: string | RegExp,
  { exact = false }: { exact?: boolean } = {},
): Promise<void> {
  const poll = expect.poll(() => spokenText(locator));
  if (expected instanceof RegExp) await poll.toMatch(expected);
  else if (exact) await poll.toBe(expected);
  else await poll.toContain(expected);
}

/** The prose pill kinds (`station`, `shot`, `line`) in `locator`, in order. */
export async function prosePillKinds(locator: Locator): Promise<string[]> {
  return locator.locator('.sh-ppill').evaluateAll((pills) => pills.map((p) => p.getAttribute('data-prose-kind') ?? ''));
}
