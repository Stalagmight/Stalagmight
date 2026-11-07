import type { MockCommandFamily, MockReply } from './types';

/**
 * Undo/redo of the Playwright IPC mock: INERT by design (#576 E3). The mock keeps no
 * history, so `can_undo` / `can_redo` answer false and both stacks are empty; every
 * undo / redo / undo_to / redo_to is recorded (`__MOCK_GET_UNDO_REDO_CALLS__`) and
 * acknowledged with an empty outcome, and the project is left as it was. What an undo
 * actually restores is the backend's (two-phase capture, `UndoData` replay) and is
 * pinned by the Rust undo fuzz gate and the history command tests, never here. A spec
 * that needs a populated stack or an enabled Undo button seeds it with an override
 * (`setIpcOverrides(page, { can_undo: true, get_undo_stack: [...] })`).
 *
 * Runs IN THE PAGE (serialized with `Function.prototype.toString`): only `ctx` and
 * browser globals, never an import (type-only imports are erased).
 */
export const historyCommands: MockCommandFamily = (ctx) => {
  // Records undo/redo invocations so specs can prove the shortcuts are inert in the
  // read-only workspaces (zero calls in Plan/Export, then one back in Survey).
  let undoRedoCalls: string[] = [];
  (window as unknown as Record<string, unknown>).__MOCK_GET_UNDO_REDO_CALLS__ = () => [...undoRedoCalls];
  (window as unknown as Record<string, unknown>).__MOCK_RESET_UNDO_REDO_CALLS__ = () => {
    undoRedoCalls = [];
  };

  return (cmd) => {
    switch (cmd) {
      case 'undo':
      case 'redo':
      case 'undo_to':
      case 'redo_to': {
        undoRedoCalls.push(cmd);
        // [#576 RV7a-3] An undo or redo never forgets the Unwind state: the backend reads
        // the link row, which the step does not touch. The mock's only record of it is the
        // link status it answers (`speleodb_link_status`, spec-seeded through
        // __speleoLinkStatusOverrides), so the step reports that value, not a fixed false.
        const link = ctx.remoteLink();
        const status = (window as unknown as { __speleoLinkStatusOverrides?: { unwound?: unknown } })
          .__speleoLinkStatusOverrides;
        return {
          reversed_steps: [],
          warnings: [],
          remote_link: link ? { ...link, unwound: status?.unwound === true } : null,
        } satisfies MockReply<'undo'> & MockReply<'redo'> & MockReply<'undo_to'> & MockReply<'redo_to'>;
      }

      case 'can_undo':
      case 'can_redo':
        return false satisfies MockReply<'can_undo'> & MockReply<'can_redo'>;

      case 'get_undo_stack':
      case 'get_redo_stack':
        return [] satisfies MockReply<'get_undo_stack'> & MockReply<'get_redo_stack'>;

      default:
        return ctx.unhandled;
    }
  };
};
