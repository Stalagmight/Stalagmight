import type { MockCommandFamily, MockReply } from './types';

/**
 * The move-tool commands of the Playwright IPC mock (move segment, section, station): each
 * records its call on `__MOCK_MOVE_CALLS__` for the commit-assertion specs and is
 * acknowledged, leaving the project as it was.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const moveToolCommands: MockCommandFamily = (ctx) => {
  return (cmd, args) => {
    switch (cmd) {
      case 'move_segment': {
        // Record the dispatched offset for commit-assertion tests. These handlers
        // don't mutate currentProject (refreshProjectState refetches generated
        // coords), so tests assert on the committed offset, not a coord read-back.
        console.log('[Mock IPC] move_segment called with:', args);
        const win = window as unknown as Record<string, unknown>;
        const calls = (win.__MOCK_MOVE_CALLS__ as unknown[]) ?? [];
        calls.push({ command: 'move_segment', args: args ?? null });
        win.__MOCK_MOVE_CALLS__ = calls;
        return { warnings: [], remote_link: ctx.remoteLink() } satisfies MockReply<'move_segment'>;
      }

      case 'move_section': {
        // Record the dispatched offset (mirrors move_segment/move_station) so
        // commit-assertion tests can check the offset that reached the backend.
        console.log('[Mock IPC] move_section called with:', args);
        const win = window as unknown as Record<string, unknown>;
        const calls = (win.__MOCK_MOVE_CALLS__ as unknown[]) ?? [];
        calls.push({ command: 'move_section', args: args ?? null });
        win.__MOCK_MOVE_CALLS__ = calls;
        return { warnings: [], remote_link: ctx.remoteLink() } satisfies MockReply<'move_section'>;
      }

      case 'move_station': {
        console.log('[Mock IPC] move_station called with:', args);
        const win = window as unknown as Record<string, unknown>;
        const calls = (win.__MOCK_MOVE_CALLS__ as unknown[]) ?? [];
        calls.push({ command: 'move_station', args: args ?? null });
        win.__MOCK_MOVE_CALLS__ = calls;
        return { warnings: [], remote_link: ctx.remoteLink() } satisfies MockReply<'move_station'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
