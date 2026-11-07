import type { IpcCommands } from '../../../src/types/ipc/IpcCommands';
import type { RemoteLinkBits } from '../../../src/types/ipc/RemoteLinkBits';

/**
 * What the real backend resolves command `K` with, from the manifest generated
 * out of `generate_handler!` (#576 C3, E3-D3). Every mock reply is written
 * `return <value> satisfies MockReply<'k'>` (type-only: the mock runs in the page
 * through `Function.prototype.toString`, so it may import types, never values),
 * which makes `typecheck:playwright` fail when a reply drifts from the wire.
 */
export type MockReply<K extends keyof IpcCommands> = IpcCommands[K]['result'];

/**
 * The contract between the Playwright IPC mock's core (`../tauri-ipc.ts`) and the
 * command families split out of it (#576 E8). The core installs the mock in the page;
 * every command its own dispatcher does not handle is offered to each family in turn,
 * and the first answer that is not `ctx.unhandled` is the result. A family owns its
 * commands and the state only they use, and reaches the core only through `ctx`.
 */
import type { buildMockShot } from './shots';

export interface MockIpcContext {
  /** What a family returns for a command it does not handle. */
  readonly unhandled: object;
  /** The core's open project (null when none), read at call time. */
  currentProject(): { sections: unknown[] } | null;
  /** The whole mock dispatcher (overrides, guards, core and families), as the app calls it. */
  invoke(cmd: string, args?: Record<string, unknown>): unknown;
  /** The mock's one shot builder (`./shots.ts`), so a family lands shots exactly as the core does. */
  buildShot: typeof buildMockShot;
  /**
   * The `remote_link` a survey-mutation success carries (#576 D22): `{ has_unpushed_edits:
   * true, unwound: false }` while the open project is remote, else null. A simplification
   * the core documents next to its helper; a rejection carries none.
   */
  remoteLink(): RemoteLinkBits | null;
}

/**
 * A command name the backend registers. The core and every family switch on it, so a
 * `case` for a command that does not exist fails `typecheck:playwright` (#576 E3-D3).
 */
export type MockCommandName = keyof IpcCommands;

export type MockCommandHandler = (cmd: MockCommandName, args?: Record<string, unknown>) => unknown;

/** Runs in the page (serialized with `Function.prototype.toString`): only `ctx` and browser globals. */
export type MockCommandFamily = (ctx: MockIpcContext) => MockCommandHandler;
