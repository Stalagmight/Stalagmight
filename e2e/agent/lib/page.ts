/**
 * Low-level page-context helpers. We enforce WebDriver Classic (no BiDi against
 * WebKitWebDriver), so anything async in the webview goes through
 * `executeAsync` with a done-callback rather than `execute` (which does not
 * await returned promises).
 *
 * macOS (tauri-wd / tauri-plugin-webdriver-automation) constraints on every
 * script that goes through here (#421):
 * - The plugin's execute-async has a HARD 30 s server-side timeout; scripts
 *   must resolve well inside it (poll in the harness, not in one long script).
 * - A resolved value containing BOTH `error` and `message` keys is coerced into
 *   a script error by the plugin. Never return that shape as data; wrap it
 *   (e.g. {value: ...}) if an app object legitimately carries both keys.
 */
import type { Browser } from 'webdriverio';

/** Marker an in-page helper uses to report an error back across the bridge. */
interface PageError {
  __agentError: string;
}

function isPageError(value: unknown): value is PageError {
  return typeof value === 'object' && value !== null && '__agentError' in value;
}

/**
 * Invoke a Tauri command from the webview via window.__TAURI__.core.invoke and
 * return its result. Throws if the command rejects.
 */
export async function invokeInApp<T = unknown>(
  browser: Browser,
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const raw = await browser.executeAsync(
    (cmd: string, cmdArgs: Record<string, unknown> | undefined, done: (r: unknown) => void) => {
      const w = window as unknown as {
        __TAURI__?: { core?: { invoke: (c: string, a?: Record<string, unknown>) => Promise<unknown> } };
      };
      const invoke = w.__TAURI__?.core?.invoke;
      if (!invoke) {
        done({ __agentError: 'window.__TAURI__.core.invoke unavailable' });
        return;
      }
      invoke(cmd, cmdArgs)
        .then((value) => done({ value }))
        .catch((err: unknown) => {
          // [#576 RV7a-1] A command's failure is a CommandError OBJECT ({ code, message }),
          // which String() renders as "[object Object]": say `code: message`. A plain
          // string (Tauri's own argument errors) stays as it is. Inline, not a helper:
          // WebDriver ships this function's source alone into the page.
          const e = err as { code?: unknown; message?: unknown } | null;
          const text =
            typeof e === 'object' && e !== null && !(e instanceof Error) && typeof e.message === 'string'
              ? typeof e.code === 'string'
                ? `${e.code}: ${e.message}`
                : e.message
              : String(err);
          done({ __agentError: text });
        });
    },
    command,
    args,
  );
  // `ipc <command>: <the app's error>`: the runners keep that prefix and print
  // the rest as its length, for a one-word command - every Tauri command is one.
  // Any other name (a typo, a plugin path) ends a `failed:` headline instead,
  // which they print the same way (#576 W51).
  if (isPageError(raw)) {
    throw new Error(
      /^\w+$/.test(command)
        ? `ipc ${command}: ${raw.__agentError}`
        : `ipc of a ${command.length}-character command failed: ${raw.__agentError}`,
    );
  }
  return (raw as { value: T }).value;
}

/**
 * Run an agent-supplied function body in the webview (async supported). The
 * body is `return`-style JS text; `args` are passed through and JSON-returned.
 */
export async function evalInApp<T = unknown>(
  browser: Browser,
  body: string,
  args: unknown[] = [],
): Promise<T> {
  let runner: (fnArgs: unknown[], done: (r: unknown) => void) => void;
  try {
    // Compile in the Node driver. WebDriver executes the resulting fixed script
    // directly, so the webview's production CSP does not need unsafe-eval.
    runner = new Function(
      'fnArgs',
      'done',
      `try {
        const fn = async (...args) => { ${body} };
        // [#576 RV7a-1] A rethrown CommandError object reads as code: message, as in invokeInApp.
        const describe = (err) =>
          typeof err === 'object' && err !== null && !(err instanceof Error) && typeof err.message === 'string'
            ? (typeof err.code === 'string' ? err.code + ': ' + err.message : err.message)
            : String(err);
        Promise.resolve(fn(...fnArgs))
          .then((value) => done({ value }))
          .catch((err) => done({ __agentError: describe(err) }));
      } catch (err) {
        done({ __agentError: String(err) });
      }`,
    ) as (fnArgs: unknown[], done: (r: unknown) => void) => void;
  } catch (err) {
    throw new Error(`eval compile: ${String(err)}`);
  }
  const raw = await browser.executeAsync(runner, args);
  if (isPageError(raw)) throw new Error(`eval: ${raw.__agentError}`);
  return (raw as { value: T }).value;
}

/** Cap a value's JSON size, returning { value, truncated }. */
export function capJson(value: unknown, maxBytes: number): { value: unknown; truncated: boolean } {
  const json = JSON.stringify(value ?? null);
  if (json.length <= maxBytes) return { value, truncated: false };
  return { value: `${json.slice(0, maxBytes)}...[truncated ${json.length} bytes]`, truncated: true };
}
