/**
 * RS-PROSE delta fix 1: values are cleaned of control characters (the prose
 * markers among them) without breaking i18next's placeholder scan. With
 * `skipOnVariables` (the default), i18next moves its regex cursor by the
 * length of a value as `format` returned it, but inserted the `escape`d
 * value: an escape that shortened a value by more than the gap to the next
 * placeholder made i18next skip it, so a literal `{{b}}` was shown (or
 * copied). The cleaning now happens in `format`, before i18next measures.
 * Injection: cleaning in `escape` again (escapeValue: true, escape:
 * proseValue) fails all three.
 */
import { beforeAll, describe, expect, it } from 'vitest';
import i18n from '../index';

beforeAll(() => {
  i18n.addResourceBundle(
    'en',
    'translation',
    {
      rsProseTest: {
        two: '{{a}}{{b}}',
        items_one: '{{a}}{{count}} item{{b}}',
        items_other: '{{a}}{{count}} items{{b}}',
        nested: '{{c}}$t(rsProseTest.two)',
      },
    },
    true,
    true,
  );
});

describe('interpolated values lose control characters, and every placeholder is filled', () => {
  const a = 'q\u0003\u0003\u0001';
  it('a value shortened by its controls does not swallow the next placeholder', () => {
    const out = i18n.t('rsProseTest.two', { a, b: 'B' });
    expect(out).toBe('qB');
  });

  it('in a plural', () => {
    expect(i18n.t('rsProseTest.items', { a, b: 'B', count: 2 })).toBe('q2 itemsB');
    expect(i18n.t('rsProseTest.items', { a, b: 'B', count: 1 })).toBe('q1 itemB');
  });

  it('in a nested translation', () => {
    const out = i18n.t('rsProseTest.nested', { a, b: 'B', c: 'C\u0002\u0002\u0002' });
    expect(out).toBe('CqB');
  });
});
