import type { MockCommandFamily, MockReply } from './types';

/**
 * The corruption-recovery commands of the Playwright IPC mock (#576 E3): canned
 * "nothing to recover" answers. No backup exists, a repair reports that it changed
 * nothing, nothing is salvaged, and writing the salvage file succeeds. What the backend does on disk - the safety copy, the
 * `fs::copy` of a backup over the live project, REINDEX/VACUUM - is pinned by the Rust
 * command tests (`src-tauri/src/commands/recovery_tests.rs`), never here. A spec that
 * walks the recovery dialog seeds the answers it needs with `__MOCK_IPC_OVERRIDES__`
 * (corruption-recovery.spec.ts, recovery-integrity.spec.ts).
 *
 * Runs IN THE PAGE (serialized with `Function.prototype.toString`): only `ctx` and
 * browser globals, never an import (type-only imports are erased).
 */
export const recoveryCommands: MockCommandFamily = (ctx) => {
  return (cmd, args) => {
    switch (cmd) {
      case 'list_backups':
        return [] satisfies MockReply<'list_backups'>;

      case 'attempt_repair':
        return {
          success: false,
          backupPath: '',
          message: 'Mock: no repair was attempted.',
        } satisfies MockReply<'attempt_repair'>;

      case 'restore_from_backup':
        return `Restored ${String(args?.backupPath ?? '')}` satisfies MockReply<'restore_from_backup'>;

      // [#576 Q-W37-1-D1] The salvage's open-only check: every path opens.
      case 'check_salvage_source':
        return null satisfies MockReply<'check_salvage_source'>;

      // [#576 HFI-3] The salvage file is written by the backend; the mock writes
      // nothing, and answers the counts of what it wrote: none (#576 W30-5) - so
      // not the project either (#576 Q-W30-2-D1).
      case 'write_salvaged_data':
        return {
          sectionCount: 0,
          shotCount: 0,
          mergeCount: 0,
          errors: [],
          projectIncluded: false,
        } satisfies MockReply<'write_salvaged_data'>;

      default:
        return ctx.unhandled;
    }
  };
};
