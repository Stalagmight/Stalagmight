import type { TFunction } from 'i18next';

/** How many names a list shows before it counts the rest. */
export const NAMES_LISTED = 3;

/**
 * Names as a sentence lists them: `A`, `A and B`, `A, B and C`, and past
 * {@link NAMES_LISTED} the first ones and a count, `A, B, C and 2 more`
 * (demo finding N-13: "Upstream and 1 more line move 24 m" never named
 * Upstream Side). The undo history lists lines the same way
 * (`core/state/labels.rs`, `listed`).
 */
export function listNames(names: readonly string[], t: TFunction): string {
  if (names.length > NAMES_LISTED) {
    return t('common.list.more', {
      head: names.slice(0, NAMES_LISTED).join(', '),
      count: names.length - NAMES_LISTED,
    });
  }
  const last = names[names.length - 1];
  if (last === undefined) return '';
  if (names.length === 1) return last;
  if (names.length === 2) return t('common.list.two', { first: names[0], last });
  return t('common.list.many', { head: names.slice(0, -1).join(', '), last });
}
