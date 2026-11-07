/**
 * Check that webdriverio can create a WebDriver session on the current Node.
 *
 * This exists because the harness once carried a hard "Node 22 only" rule,
 * justified by webdriverio handing an `Agent` from its bundled undici to Node's
 * built-in `fetch` so the undici majors had to match. That is not what
 * `webdriver` does (it imports `fetch` from undici alongside `Agent` and calls
 * that), and the rule cost a split toolchain for as long as nobody re-checked it.
 *
 * The failure it described was at `POST /session`, i.e. purely in the HTTP layer,
 * so it does not need tauri-driver, WebKitWebDriver or the app to reproduce. A
 * stub server is enough - which matters, because the real harness is Linux-only
 * and so cannot answer this question from a macOS box.
 *
 * Run it after any webdriverio or Node bump:
 *   pnpm agent:http-probe
 *
 * This covers the HTTP layer ONLY. A green run does not say the full harness
 * works on this Node, just that the thing the old rule blamed is fine.
 */
import http from 'node:http';
import type { AddressInfo } from 'node:net';

import { shareableErrorText } from './lib/shareableFailure';

const STUB_SESSION_ID = 'http-probe-session';

// webdriver snapshots the proxy env into a plain object at MODULE INIT
// (webdriver/build/node.js:2190-2196, `variables: { PROXY_URL: process.env.HTTP_PROXY ... }`),
// and then routes requests through it unless NO_PROXY covers the host (:2008-2009).
// For a loopback stub that means an unrelated proxy setting makes this probe fail
// with "make sure browser driver is running" - blaming the driver for the
// environment, in the one tool whose job is to tell those apart.
//
// Because the snapshot happens at init, clearing the env after a static import is
// too late; webdriverio has to be imported AFTER this runs, hence the dynamic
// import in main(). Nothing about a 127.0.0.1 stub should traverse a proxy.
function dropProxyEnv(): string[] {
  const vars = ['HTTP_PROXY', 'http_proxy', 'HTTPS_PROXY', 'https_proxy'] as const;
  const present = vars.filter((v) => process.env[v]);
  for (const v of vars) delete process.env[v];
  return [...present];
}

async function main(): Promise<void> {
  const dropped = dropProxyEnv();
  const { remote } = await import('webdriverio');
  const seen: string[] = [];
  if (dropped.length > 0) {
    console.log(`  (ignored ${dropped.join(', ')} - a loopback stub needs no proxy)`);
  }

  const server = http.createServer((req, res) => {
    req.resume();
    req.on('end', () => {
      seen.push(`${req.method} ${req.url}`);
      res.setHeader('content-type', 'application/json');
      if (req.method === 'POST' && req.url?.endsWith('/session')) {
        res.end(
          JSON.stringify({
            value: {
              sessionId: STUB_SESSION_ID,
              capabilities: { browserName: 'wry', platformName: 'linux' },
            },
          }),
        );
        return;
      }
      res.end(JSON.stringify({ value: null }));
    });
  });

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;

  const undici = process.versions.undici ?? 'n/a';
  console.log(`node ${process.versions.node} (built-in undici ${undici})`);

  try {
    const browser = await remote({
      hostname: '127.0.0.1',
      port,
      path: '/',
      protocol: 'http',
      automationProtocol: 'webdriver',
      connectionRetryCount: 0,
      logLevel: 'error',
      capabilities: { browserName: 'wry' },
    });
    if (browser.sessionId !== STUB_SESSION_ID) {
      throw new Error(`unexpected sessionId ${browser.sessionId}`);
    }
    await browser.deleteSession().catch(() => {
      // The stub does not model teardown; only session creation is under test.
    });
    console.log(`  ok: session created and closed (${seen.join(', ')})`);
  } finally {
    server.close();
  }
}

main().catch((err: unknown) => {
  const msg = err instanceof Error ? err.message : String(err);
  // The shareable crash form (#576 W51), as every runner's `fatal:` line: the
  // headline as it is unless it ends in another program's text, the rest counted.
  console.error(`  FAILED: ${shareableErrorText(err)}`);
  if (msg.includes('UND_ERR_INVALID_ARG')) {
    console.error(
      '  That is the error the old Node-22-only rule cited. Check HTTP_PROXY /\n' +
        '  NO_PROXY first, then record what actually fixed it in docs/agent-harness.md.',
    );
  }
  process.exitCode = 1;
});
