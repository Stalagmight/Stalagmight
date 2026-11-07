import type { SpeleoStore } from './speleoStore';

/**
 * Late-bound handle on the SpeleoDB store, for projectStore and its action
 * slices (#576 D27-32(b)).
 *
 * speleoStore imports projectStore (it reads the open project's path and
 * reloads its config). projectStore importing speleoStore back made the two a
 * value-import cycle: their evaluation order depended on which one some other
 * module imported first, so a single top-level cross read would have been a
 * start-up ReferenceError for one of the two orders. projectStore now reaches
 * speleoStore through this module, which imports only its type.
 *
 * speleoStore registers itself here when its module evaluates, and main.tsx
 * imports speleoStore at startup, so in the app every call reaches the real
 * store. Until it registers (a unit test that never loads speleoStore),
 * `speleoState()` is null: the calls are skipped and nothing is read. The
 * getter is called on every access, so a test that swaps a speleoStore action
 * with `setState` is seen here too.
 *
 * `pnpm lint:import-cycles` keeps the cycle from coming back.
 */
let getState: (() => SpeleoStore) | null = null;

export function registerSpeleoStore(get: () => SpeleoStore): void {
  getState = get;
}

/** The SpeleoDB store's current state, or null before speleoStore has loaded. */
export function speleoState(): SpeleoStore | null {
  return getState ? getState() : null;
}
