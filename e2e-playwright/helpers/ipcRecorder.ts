import { expect, type Page } from '../fixtures/coverage';

/**
 * Record the IPC calls the app makes, so a spec can assert that a feature
 * actually reached the backend with the right arguments - not just that the UI
 * did not crash (#576 RV3-FE-7: several @smoke canaries asserted only a closed
 * dialog or a visible canvas, and still passed with the feature's handler
 * stubbed out).
 *
 * Wraps `window.__TAURI_INTERNALS__.invoke` in the page (the app resolves it at
 * call time, as autosave-leave-prompt.spec.ts and draft-row.spec.ts rely on), so
 * every call AFTER `recordIpcCalls` is seen, mock overrides included. Arguments
 * are snapshotted as JSON when the call is made; typed arrays are reduced to
 * their length so an image or binary payload does not bloat the log.
 */
export interface RecordedIpcCall {
  cmd: string;
  args: Record<string, unknown>;
}

type InvokeFn = (cmd: string, args?: Record<string, unknown>, opts?: unknown) => Promise<unknown>;

/** Start recording the given commands (all commands when `commands` is empty). Idempotent per page. */
export async function recordIpcCalls(page: Page, commands: string[] = []): Promise<void> {
  await page.evaluate((only) => {
    const w = window as unknown as {
      __TAURI_INTERNALS__: { invoke: InvokeFn };
      __E2E_IPC_CALLS__?: RecordedIpcCall[];
      __E2E_IPC_FILTER__?: string[];
    };
    w.__E2E_IPC_FILTER__ = only;
    if (w.__E2E_IPC_CALLS__) {
      w.__E2E_IPC_CALLS__ = [];
      return;
    }
    w.__E2E_IPC_CALLS__ = [];
    const original = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = (cmd, args, opts) => {
      const filter = w.__E2E_IPC_FILTER__ ?? [];
      if (filter.length === 0 || filter.includes(cmd)) {
        const snapshot = JSON.parse(
          JSON.stringify(args ?? {}, (_key, value) =>
            ArrayBuffer.isView(value) ? { byteLength: (value as ArrayBufferView).byteLength } : value,
          ),
        ) as Record<string, unknown>;
        w.__E2E_IPC_CALLS__?.push({ cmd, args: snapshot });
      }
      return original(cmd, args, opts);
    };
  }, commands);
}

/** The recorded calls, optionally only those of one command, in call order. */
export async function getIpcCalls(page: Page, cmd?: string): Promise<RecordedIpcCall[]> {
  const calls = await page.evaluate(
    () => (window as unknown as { __E2E_IPC_CALLS__?: RecordedIpcCall[] }).__E2E_IPC_CALLS__ ?? [],
  );
  return cmd === undefined ? calls : calls.filter((c) => c.cmd === cmd);
}

/**
 * Wait until `cmd` has been called `count` times (default: at least once), and return its calls.
 * `count` is >= 1 at every call site, so the result is typed non-empty: the poll only resolves
 * once that many calls are recorded, and the log is append-only until the next recordIpcCalls.
 */
export async function waitForIpcCall(
  page: Page,
  cmd: string,
  count = 1,
  timeout = 10000,
): Promise<[RecordedIpcCall, ...RecordedIpcCall[]]> {
  await expect
    .poll(async () => (await getIpcCalls(page, cmd)).length, { timeout, message: `waiting for IPC ${cmd} x${count}` })
    .toBeGreaterThanOrEqual(count);
  return getIpcCalls(page, cmd) as Promise<[RecordedIpcCall, ...RecordedIpcCall[]]>;
}
