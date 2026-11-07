import type { MockCommandFamily, MockReply } from './types';

/**
 * The diagnostics commands of the Playwright IPC mock: the Debug dialog's recent log lines,
 * the debug memory probe and the internal-error status, each a canned idle answer.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const diagnosticsCommands: MockCommandFamily = (ctx) => {
  return (cmd) => {
    switch (cmd) {
      case 'get_recent_logs':
        // DebugDialog expects string[] log lines prefixed with a level.
        return ['INFO [mock] app started', 'DEBUG [mock] project loaded'] satisfies MockReply<'get_recent_logs'>;

      // The debug memory probe: an idle backend and no WebKit children to enumerate.
      case 'get_memory_stats':
        return {
          backend: {
            pid: 1,
            name: 'stalagmight',
            rss_kb: 0,
            vm_size_kb: 0,
            vm_peak_kb: 0,
            vm_rss_anon_kb: 0,
            vm_rss_file_kb: 0,
            vm_rss_shmem_kb: 0,
            role: 'backend',
          },
          webkit_processes: [],
          total_rss_kb: 0,
          webkit_enumeration_supported: false,
        } satisfies MockReply<'get_memory_stats'>;

      case 'get_internal_error_status':
        // [#576 RV5-PI-3] No crash in a mocked session: no banner to raise.
        return null satisfies MockReply<'get_internal_error_status'>;

      default:
        return ctx.unhandled;
    }
  };
};
