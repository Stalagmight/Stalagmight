import type { MockCommandFamily, MockReply } from './types';

/**
 * The per-project display settings of the Playwright IPC mock: an in-memory store that
 * `save_display_setting` writes and `load_display_settings` answers from.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const displaySettingsCommands: MockCommandFamily = (ctx) => {
  // Display settings persistence - in-memory storage for testing reload behavior
  const mockDisplaySettings: Record<string, string> = {
    showStations: 'true',
    showShots: 'true',
    showSectionNames: 'true',
    showDepthGradient: 'false',
  };

  return (cmd, args) => {
    switch (cmd) {
      // Display settings (per-project) - uses in-memory storage for persistence testing
      case 'load_display_settings':
        // Return current display settings from in-memory storage
        return { ...mockDisplaySettings } satisfies MockReply<'load_display_settings'>;

      case 'save_display_setting': {
        // Persist setting to in-memory storage
        const key = args?.key as string;
        const value = args?.value as string;
        if (key && value !== undefined) {
          mockDisplaySettings[key] = value;
          console.log(`[Mock IPC] save_display_setting: ${key}=${value}`);
        }
        return null satisfies MockReply<'save_display_setting'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
