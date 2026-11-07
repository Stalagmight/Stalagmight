import type { IpcCommands } from '../../../src/types/ipc/IpcCommands';
import type { MockReply } from './types';

/**
 * Commands the Playwright IPC mock answers with a fixed no-op reply on purpose
 * (#576 E3-D2), each with the reason no spec needs more from it. Everything else a
 * spec reaches must be a real mock case: an unhandled command is recorded and fails
 * the test at teardown (`../../fixtures/coverage.ts`), and `mockContract.test.ts`
 * checks this list against the generated command manifest.
 *
 * Plain data: the core serializes it into the page as JSON (`setupTauriMocks`), so a
 * reply must be JSON-safe.
 */
export const INERT_COMMANDS: { [K in keyof IpcCommands]?: { reply: MockReply<K>; reason: string } } = {
  flush_telemetry: {
    reply: null,
    reason: 'background telemetry flush; no mocked flow depends on delivery',
  },
  is_telemetry_available: {
    reply: false,
    reason: 'telemetry probe; the mocked app runs as a build without a telemetry endpoint',
  },
  set_telemetry_enabled: {
    reply: null,
    reason: 'telemetry opt-in write; the setting itself round-trips through the config commands',
  },
  track_events: {
    reply: null,
    reason: 'background telemetry event batch; telemetry is pinned by its own vitest and Rust registry tests',
  },
  push_frontend_log: {
    reply: null,
    reason: 'the console bridge into the backend log; the app never reads its reply',
  },
};
