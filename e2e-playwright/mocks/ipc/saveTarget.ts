import type { MockCommandFamily, MockReply } from './types';

/**
 * The write-path probes of the Playwright IPC mock: the New Project pre-check
 * (`check_create_target`, #576 RV15-2, below), and the overwrite prompts'
 * existence probe (#576 HFI-2):
 * `save_target_exists` answers false, the mocked file system being empty, so a Save As
 * or a rewritten export name saves without a prompt. What the backend checks - the
 * write-path policy, a refused or failed check - is pinned by the Rust command tests
 * (`src-tauri/src/commands/save_target_tests.rs`) and the callers' answers to each
 * outcome by vitest (`overwriteGuard.test.ts`, `projectActions.saveAs.test.ts`), never
 * here. A spec that needs a file to be there overrides it
 * (`__MOCK_IPC_OVERRIDES__.save_target_exists = true`).
 *
 * Runs IN THE PAGE (serialized with `Function.prototype.toString`): only `ctx` and
 * browser globals, never an import (type-only imports are erased).
 */
export const saveTargetCommands: MockCommandFamily = (ctx) => {
  return (cmd) => {
    switch (cmd) {
      case 'save_target_exists':
        return false satisfies MockReply<'save_target_exists'>;

      // [#576 RV15-2] The New Project pre-check: the mocked file system has no
      // folder the write-path policy refuses, so every create may go ahead. A spec
      // that needs a refusal overrides it.
      case 'check_create_target':
        return null satisfies MockReply<'check_create_target'>;

      default:
        return ctx.unhandled;
    }
  };
};
