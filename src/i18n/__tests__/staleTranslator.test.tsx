/**
 * Why a `t` missing from a hook's dependency array cannot show stale-language
 * text in this app (#576 NF-W5LINT-1).
 *
 * eslint-baseline.json freezes several exhaustive-deps warnings whose only
 * missing dependency is `t` (MapCanvas's anchored-ids effect, three useMapTools
 * click handlers). They look like stale closures: the memoized callback keeps
 * the `t` of the render that created it. They are not, because every
 * `useTranslation()` call here passes no `lng`, so react-i18next hands out
 * `i18n.getFixedT(null, ...)` - a translator that resolves against the CURRENT
 * language each time it is called. A `t` captured before a language switch
 * therefore renders the new language, exactly like the fresh one.
 *
 * If this test ever fails (a `useTranslation(ns, { lng })` call, or an i18next
 * change that binds `t` to a language), those frozen `t` warnings become real
 * stale closures: list `t` in each dependency array (or read it through
 * useLatest where re-running an effect is not wanted).
 *
 * `.tsx` so this runs in the jsdom tier (vite.config.ts environmentMatchGlobs).
 */
import { describe, it, expect, afterEach } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useTranslation } from 'react-i18next';
import i18n from '../index';

const KEY = 'map.toasts.stationsHidden';

afterEach(async () => {
  await act(async () => {
    await i18n.changeLanguage('en');
  });
});

describe('a translator captured before a language switch', () => {
  it('renders the new language, like the fresh one', async () => {
    i18n.addResourceBundle('xx', 'translation', { map: { toasts: { stationsHidden: 'XX: stations hidden' } } }, true, true);
    const { result } = renderHook(() => useTranslation());
    const captured = result.current.t;
    expect(captured(KEY)).toBe(i18n.getFixedT('en')(KEY));

    await act(async () => {
      await i18n.changeLanguage('xx');
    });

    // react-i18next still hands out a NEW `t` on the switch (so a component that
    // lists it re-renders), but the old one is not bound to English.
    expect(result.current.t).not.toBe(captured);
    expect(result.current.t(KEY)).toBe('XX: stations hidden');
    expect(captured(KEY)).toBe('XX: stations hidden');
  });
});
