import type { Page } from '../fixtures/coverage';
import type { MockCommandName, MockReply } from '../mocks/ipc/types';

/** The mock's two failure forms: a backend refusal (a `CommandError`) or a transport error. */
export type IpcOverrideFailure = { __reject: unknown } | { __throw: string };

/**
 * Canned replies a spec lays over the mock, per command (#576 E3-D3): each is the command's
 * result type from the generated manifest (or a failure form), so an off-wire canned reply
 * fails `typecheck:playwright` like an off-wire mock case does. Tauri plugin bridge calls
 * have no manifest entry and stay unchecked.
 */
export type IpcOverrides = { [K in MockCommandName]?: MockReply<K> | IpcOverrideFailure } & {
  [plugin: `plugin:${string}`]: unknown;
};

/**
 * Merge command overrides into `window.__MOCK_IPC_OVERRIDES__` (mocks/tauri-ipc.ts
 * reads this map to short-circuit or fail specific IPC commands). Shared by every
 * spec that needs to simulate a backend error or force a specific mocked response.
 *
 * A value is the command's reply, or one of the mock's two failure forms:
 * `{ __reject: { code, message, ...payload } }` rejects with that `CommandError`
 * (a backend refusal, #576 C4 - write it `satisfies CommandError`), and
 * `{ __throw: msg }` throws an Error (a transport / JS failure).
 */
export async function setIpcOverrides(page: Page, overrides: IpcOverrides): Promise<void> {
  await page.evaluate((nextOverrides) => {
    const current =
      (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ as
        | Record<string, unknown>
        | undefined;
    (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = {
      ...(current ?? {}),
      ...nextOverrides,
    };
  }, overrides);
}

/** Remove specific command overrides (e.g. to stop simulating an error mid-test). */
export async function clearIpcOverrideKeys(page: Page, commands: Array<keyof IpcOverrides>): Promise<void> {
  await page.evaluate((keys) => {
    const current =
      (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ as
        | Record<string, unknown>
        | undefined;
    if (!current) {
      return;
    }

    for (const key of keys) {
      delete current[key];
    }

    (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = {
      ...current,
    };
  }, commands);
}
