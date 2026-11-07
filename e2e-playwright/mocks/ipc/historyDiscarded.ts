import type { MockCommandFamily, MockReply } from './types';

/**
 * The discarded-history check of the Playwright IPC mock (#473): whether the open project's
 * undo history was unreadable (seeded by `__MOCK_SET_HISTORY_DISCARDED__`), and how many
 * times the app asked.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const historyDiscardedCommands: MockCommandFamily = (ctx) => {
  // [#473] Whether the loaded project's undo/redo history was unreadable and
  // discarded. Default false - every existing spec opens a project whose
  // history loads fine, and a banner appearing over them would be a false
  // failure. Seeded by __MOCK_SET_HISTORY_DISCARDED__.
  let mockHistoryDiscarded = false;
  // How many times the app has ASKED. A spec asserting the banner is absent needs
  // this: `toHaveCount(0)` passes on its first poll, so without a positive signal
  // that the post-load effect has actually run, the assertion is satisfied by the
  // effect not having happened yet and can never fail.
  let mockHistoryDiscardedCalls = 0;
  (window as unknown as Record<string, unknown>).__MOCK_SET_HISTORY_DISCARDED__ = (
    discarded: boolean
  ) => {
    mockHistoryDiscarded = !!discarded;
    mockHistoryDiscardedCalls = 0;
  };
  (window as unknown as Record<string, unknown>).__MOCK_HISTORY_DISCARDED_CALLS__ = () =>
    mockHistoryDiscardedCalls;

  return (cmd) => {
    switch (cmd) {
      case 'history_discarded':
        // #473: whether the open project's edit history was unreadable and
        // replaced by an empty stack. A peek, not a take - the real command
        // keeps answering the same thing while the project is open, so the
        // mock must too or a re-render would clear the banner.
        mockHistoryDiscardedCalls += 1;
        return mockHistoryDiscarded satisfies MockReply<'history_discarded'>;

      default:
        return ctx.unhandled;
    }
  };
};
