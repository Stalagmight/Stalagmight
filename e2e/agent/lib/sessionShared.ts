/**
 * Attach + readiness steps that are identical on every platform: the remote()
 * handshake against a WebDriver endpoint already listening on 127.0.0.1, and the
 * post-attach checks that decide whether the app under it is usable.
 *
 * Both throw rather than tearing the session down themselves; the caller owns
 * teardown because only it knows the stop reason to record.
 */
import { remote, type Browser } from 'webdriverio';
import { waitForAppReady, waitForE2EHooks, disableOnboardingGuides } from './waiters';

/** verifyReadiness failure that means the binary itself is wrong, not slow. */
export class E2EHooksMissingError extends Error {
  constructor() {
    super(
      'window.__E2E_TEST__ not present: the binary was built without VITE_E2E=1. Rerun app_start with build:"force".',
    );
    this.name = 'E2EHooksMissingError';
  }
}

export async function attachBrowser(
  port: number,
  capabilities: Record<string, unknown>,
  driverName = 'tauri-driver',
): Promise<Browser> {
  try {
    return await remote({
      hostname: '127.0.0.1',
      port,
      path: '/',
      logLevel: 'silent',
      connectionRetryCount: 3,
      connectionRetryTimeout: 30000,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any -- a caller-built record (tauri:options included) that webdriverio's Capabilities type does not model
      capabilities: capabilities as any,
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    if (msg.includes('UND_ERR_INVALID_ARG')) {
      throw new Error(
        `Failed to attach to ${driverName} (UND_ERR_INVALID_ARG), an error from webdriverio's ` +
          `undici HTTP layer. You are on Node ${process.versions.node}. Session creation is ` +
          'known to work on Node 22 and 24, so this is more likely a bad dispatcher/proxy ' +
          'option or an HTTP_PROXY/NO_PROXY in the environment than the Node version. If a ' +
          'different Node major does fix it, please record which one in docs/agent-harness.md.',
      );
    }
    // WebDriver's error can quote the driver's reply (the binary's path, the
    // app's text), so it ends a `failed:` headline, which the runners print as
    // its length (#576 W51).
    throw new Error(`Failed to attach to ${driverName} on port ${port}; the session request failed: ${msg}`);
  }
}

/** App-ready wait, __E2E_TEST__ hook check, onboarding disable. */
export async function verifyReadiness(browser: Browser): Promise<void> {
  await waitForAppReady(browser);
  if (!(await waitForE2EHooks(browser))) throw new E2EHooksMissingError();
  await disableOnboardingGuides(browser);
}
