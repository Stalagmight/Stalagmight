/**
 * `react-i18next` stand-ins whose `t` answers the key (#576 E15-NF1), so a
 * component test asserts on keys rather than on English copy. A file mocks the
 * module with one of them:
 *
 *     vi.mock('react-i18next', async () => (await import('<rel>/test/factories/i18n')).i18nKeys());
 *
 * - `i18nKeys(echo)`: the real module (`Trans`, `I18nextProvider`,
 *   `initReactI18next`, ...) with only `useTranslation` replaced.
 * - `translationStub(echo)`: `useTranslation` and an inert `initReactI18next`, and
 *   nothing else - for a test that must not reach the real module.
 *
 * `echo` is what `t` answers for a call with options: `'key'` (the key alone)
 * or `'keyAndOptions'` (`key:{"count":2}`; the key alone when there are none).
 * Every `useTranslation()` call hands out a fresh `t` and a fresh `{ t }`, as
 * the copies this replaces did, so a hook that depends on `t` re-runs exactly as
 * it did before.
 */
import { vi } from 'vitest';
import type * as ReactI18next from 'react-i18next';

/** What `t` answers when called with options. */
export type KeyEcho = 'key' | 'keyAndOptions';

function echoingUseTranslation(echo: KeyEcho): () => { t: (key: string, opts?: Record<string, unknown>) => string } {
  if (echo === 'key') return () => ({ t: (key: string) => key });
  return () => ({
    t: (key: string, opts?: Record<string, unknown>) => (opts ? `${key}:${JSON.stringify(opts)}` : key),
  });
}

/** The real `react-i18next`, with `useTranslation` echoing keys. */
export async function i18nKeys(echo: KeyEcho = 'key') {
  return {
    ...(await vi.importActual<typeof ReactI18next>('react-i18next')),
    useTranslation: echoingUseTranslation(echo),
  };
}

/** `useTranslation` echoing keys and an `initReactI18next` that does nothing; no other export. */
export function translationStub(echo: KeyEcho = 'key') {
  return {
    useTranslation: echoingUseTranslation(echo),
    initReactI18next: { type: '3rdParty', init: () => undefined },
  };
}
