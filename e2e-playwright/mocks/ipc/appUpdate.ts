import type { CommandError } from '../../../src/types/ipc/CommandError';
import type { MockCommandFamily, MockReply } from './types';

/**
 * The in-app updater (#579) in the Playwright IPC mock.
 *
 * The default is "nothing on offer", so the launch check every spec triggers
 * resolves quietly instead of parking an update banner over whatever that spec
 * is asserting on. A spec that wants the banner overrides `check_for_app_update`
 * (`__MOCK_IPC_OVERRIDES__`), as `app-update.spec.ts` does.
 *
 * Runs IN THE PAGE (serialized with `Function.prototype.toString`): only `ctx` and
 * browser globals, never an import (type-only imports are erased).
 */
export const appUpdateCommands: MockCommandFamily = (ctx) => {
  return (cmd) => {
    switch (cmd) {
      case 'check_for_app_update':
        return { status: { kind: 'up_to_date' }, current_version: '0.5.0' } satisfies MockReply<'check_for_app_update'>;

      // The backend's one-shot install consent. Succeeds here so a spec that
      // seeds an available update can drive Restart through the real exit
      // guards; the refusal cases are covered in Rust.
      case 'arm_app_update_restart':
        return null satisfies MockReply<'arm_app_update_restart'>;
      case 'unarm_app_update_restart':
        return null satisfies MockReply<'unarm_app_update_restart'>;

      // Unreachable from the default state above (both refuse without a checked
      // update), and a real install would end the browser session. Answered so
      // the contract holds, and refused as the backend refuses - a rejection
      // with a `CommandError` - so an accidental call is a visible failure rather
      // than an unhandled null.
      case 'download_app_update':
      case 'install_app_update_and_restart':
        return Promise.reject({
          code: 'FAILED',
          message: `${cmd} is not driven by the default mock state`,
        } satisfies CommandError);

      default:
        return ctx.unhandled;
    }
  };
};
