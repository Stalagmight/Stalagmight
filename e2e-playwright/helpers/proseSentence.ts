/**
 * Prose pills in Playwright specs (RS-PROSE). A sentence that names a
 * station, shot or line draws each name as a pill: an `aria-hidden` drawing
 * plus an `sr-only` span with the plain words. `toHaveText` reads
 * textContent, which holds both, so a spec reads the sentence a screen reader
 * reads instead: every text node, skipping `aria-hidden` subtrees (as
 * `src/test/accessibleText.ts` does for vitest).
 */
import { expect, type Locator } from '@playwright/test';

/** The text a screen reader reads in the element: the plain sentence. */
export function proseSentence(locator: Locator): Promise<string> {
  return locator.evaluate((root) => {
    let out = '';
    const walk = (node: Node) => {
      if (node.nodeType === Node.TEXT_NODE) {
        out += node.textContent ?? '';
        return;
      }
      if (node instanceof Element && node.getAttribute('aria-hidden') === 'true') return;
      node.childNodes.forEach(walk);
    };
    walk(root);
    return out;
  });
}

/** The kinds of the prose pills in the element, in order. */
export function prosePillKinds(locator: Locator): Promise<Array<string | null>> {
  return locator.evaluate((root) =>
    [...root.querySelectorAll('.sh-ppill')].map((pill) => pill.getAttribute('data-prose-kind')),
  );
}

/**
 * Waits until the element reads `expected` (exact, or a pattern) and draws
 * pills of `kinds`, in order.
 */
export async function expectProse(
  locator: Locator,
  expected: string | RegExp,
  kinds: string[],
  options?: { timeout?: number },
): Promise<void> {
  const poll = expect.poll(() => proseSentence(locator), options);
  if (typeof expected === 'string') await poll.toBe(expected);
  else await poll.toMatch(expected);
  await expect.poll(() => prosePillKinds(locator), options).toEqual(kinds);
}
