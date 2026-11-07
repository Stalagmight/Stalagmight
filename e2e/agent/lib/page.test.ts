import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Browser } from 'webdriverio';

import { evalInApp, invokeInApp } from './page';
import { shareableFailureText } from './shareableFailure';

function executingBrowser(): Browser {
  return {
    executeAsync: vi.fn(
      (runner: (args: unknown[], done: (result: unknown) => void) => void, args: unknown[]) =>
        new Promise((resolve) => runner(args, resolve)),
    ),
  } as unknown as Browser;
}

describe('evalInApp', () => {
  it('compiles in the driver and executes the fixed function through WebDriver', async () => {
    const browser = executingBrowser();
    await expect(evalInApp<number>(browser, 'return args[0] * args[1];', [6, 7])).resolves.toBe(42);

    const runner = vi.mocked(browser.executeAsync).mock.calls[0]?.[0];
    expect(String(runner)).not.toContain('new Function');
  });

  it('returns page-script failures as agent errors', async () => {
    await expect(evalInApp(executingBrowser(), 'throw new Error("broken");')).rejects.toThrow('eval: Error: broken');
  });

  it('reports a CommandError an eval body rethrows as code: message (#576 RV7a-1)', async () => {
    await expect(
      evalInApp(executingBrowser(), 'throw { code: "WORKSPACE_READ_ONLY", message: "the Plan workspace is read-only" };'),
    ).rejects.toThrow('eval: WORKSPACE_READ_ONLY: the Plan workspace is read-only');
  });
});

/** A WebDriver double that runs an `executeAsync(fn, ...args)` script in-process. */
function spreadingBrowser(): Browser {
  return {
    executeAsync: vi.fn(
      (script: (...args: unknown[]) => void, ...args: unknown[]) =>
        new Promise((resolve) => script(...args, resolve)),
    ),
  } as unknown as Browser;
}

/** The page's Tauri bridge, answering every command with `answer`. */
function stubInvoke(answer: () => Promise<unknown>): void {
  vi.stubGlobal('window', { __TAURI__: { core: { invoke: vi.fn(answer) } } });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

// [#576 RV7a-1] Since every command rejects with a CommandError object, String(err)
// turned every harness-side IPC failure into "[object Object]".
describe('invokeInApp', () => {
  it("reports a command's coded refusal as code: message", async () => {
    stubInvoke(() => Promise.reject({ code: 'EDIT_DISABLED', message: 'Update shot is disabled' }));
    const failure = await invokeInApp(spreadingBrowser(), 'update_shot', { sectionId: 1 }).catch(
      (e: Error) => e.message,
    );
    console.log(`[RV7a-1] harness ipc failure reads: ${String(failure)}`);
    expect(failure).toBe('ipc update_shot: EDIT_DISABLED: Update shot is disabled');
  });

  it('keeps a plain-string rejection (a Tauri argument error) as it is', async () => {
    stubInvoke(() => Promise.reject('missing required key sectionId'));
    await expect(invokeInApp(spreadingBrowser(), 'delete_section')).rejects.toThrow(
      'ipc delete_section: missing required key sectionId',
    );
  });

  it("keeps the app's error after a prefix the runners withhold, whatever the command is called (#576 W51)", async () => {
    stubInvoke(() => Promise.reject('qzCaveA not found'));
    for (const command of ['rename_station', 'plugin:qz|x y']) {
      const failure = await invokeInApp(spreadingBrowser(), command).catch((e: Error) => e.message);
      const printed = shareableFailureText(String(failure));
      expect(printed).not.toContain('qzCaveA');
      expect(printed).toMatch(/\(17 characters of the app's text withheld\)$/);
    }
  });

  it('returns the value of a command that answers', async () => {
    stubInvoke(() => Promise.resolve({ ok: 1 }));
    await expect(invokeInApp(spreadingBrowser(), 'get_config')).resolves.toEqual({ ok: 1 });
  });
});
