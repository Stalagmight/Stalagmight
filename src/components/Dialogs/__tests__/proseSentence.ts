/**
 * RS-PROSE test helpers for the kept-station dialogs: a sentence that names
 * stations and lines draws each as a prose pill, so its `textContent` holds
 * the pill's drawing and its sr-only words. These read what a screen reader
 * reads (the sentence plain text wrote) and which pills it draws.
 */
import { accessibleText } from '../../../test/accessibleText';

/** The accessible text of `el`, whitespace collapsed as `toHaveTextContent` reads it. */
export function said(el: Element | null): string {
  if (el === null) throw new Error('said: no element');
  return accessibleText(el).replace(/\s+/g, ' ').trim();
}

/** The kinds of the prose pills `el` draws, in order (`station`, `shot`, `line`). */
export function pillKinds(el: Element): string[] {
  return [...el.querySelectorAll('.sh-ppill')].map((pill) => pill.getAttribute('data-prose-kind') ?? '');
}

/** What each prose pill in `el` draws (its aria-hidden drawing's text), in order. */
export function pillDrawings(el: Element): string[] {
  return [...el.querySelectorAll('.sh-ppill')].map(
    (pill) => pill.querySelector('[aria-hidden="true"]')?.textContent ?? '',
  );
}
