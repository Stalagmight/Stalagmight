import { defineConfig, devices } from '@playwright/test';

const coverageEnabled = process.env.E2E_COVERAGE === '1';

// Serve a built bundle instead of the Vite dev server (#295). In dev mode Vite
// serves the app's own source unbundled - one request per module, 346 of them
// per page load - so every test's `goto` re-pays that cost, and it scales ~9x
// under parallel workers because N workers hammer a single Node process. A
// build collapses those into a handful of static assets. Measured on an M1:
// contended per-test setup 5.74s -> 2.65s (-54%), @smoke wall-clock 81s -> 38s
// (-53%), all 83 @smoke tests green either way with no spec changes.
//
// VITE_E2E=1 keeps the window.__E2E_TEST__ store hooks the specs drive - main.tsx
// gates them on DEV || MODE === 'test' || VITE_E2E === '1', so a plain production
// build would strip them and every store-seeded spec would fail.
//
// Default: built bundle in CI (where the win is), dev server locally so the
// interactive loop keeps HMR. PW_PROD_SERVER=1/0 forces either way.
const prodServer =
  process.env.PW_PROD_SERVER === '1' ||
  (process.env.PW_PROD_SERVER !== '0' && !!process.env.CI);

// PW_PORT points the whole run - server, baseURL, reuse probe - at another port.
// Default 1420 keeps CI and the interactive loop exactly as before. The escape
// hatch exists because `reuseExistingServer` attaches to WHOEVER holds 1420: a
// dev server left running from another clone/worktree silently serves ITS code
// and every spec tests the wrong tree (bit this repo twice; see the in-flight
// board's port map). A validated integer only - anything else falls back.
const envPort = Number(process.env.PW_PORT);
const port =
  process.env.PW_PORT !== undefined && Number.isInteger(envPort) && envPort > 0 ? envPort : 1420;

// vite-plugin-istanbul already sets forceBuildInstrument, so coverage works on
// the built bundle too - it just needs VITE_COVERAGE at build time, not serve time.
const buildEnv = coverageEnabled ? 'VITE_COVERAGE=1 VITE_E2E=1' : 'VITE_E2E=1';
const devServerCommand = prodServer
  ? `${buildEnv} pnpm exec vite build && pnpm exec vite preview --port ${port} --strictPort`
  : coverageEnabled
    ? `VITE_COVERAGE=1 pnpm dev --port ${port}`
    : `pnpm dev --port ${port}`;

// Retries: CI gets 2; locally 1, so a single timing blip under load (e.g. the
// pre-push @smoke gate on a 1-worker headless VM) doesn't abort an otherwise-
// green push (issue #15). PW_RETRIES overrides, but only with a validated
// non-negative integer - anything else (PW_RETRIES=foo, -1, 1.5) falls back to
// the default instead of becoming NaN and breaking config load.
const defaultRetries = process.env.CI ? 2 : 1;
const envRetries = Number(process.env.PW_RETRIES);
const retries =
  process.env.PW_RETRIES !== undefined && Number.isInteger(envRetries) && envRetries >= 0
    ? envRetries
    : defaultRetries;

// Workers: PW_WORKERS overrides. Playwright's `workers` accepts a number OR a
// percentage string ("50%") but NOT a bare numeric *string*, which it rejects
// with "config.workers must be a number or percentage" (#132). Env vars are
// always strings, so coerce a bare integer (PW_WORKERS=3) to a Number while
// leaving a percentage ("50%") as-is. In CI default to "100%" (all cores):
// Playwright's own default is floor(cpus/2) = 1 worker (serial) on a 2-core
// runner (#120).
const envWorkers = process.env.PW_WORKERS;
const workers =
  envWorkers === undefined || envWorkers === ''
    ? process.env.CI
      ? '100%'
      : undefined
    : /^\d+$/.test(envWorkers)
      ? Number(envWorkers)
      : envWorkers;

/**
 * Playwright configuration for fast UI tests with Tauri IPC mocking.
 * These tests run against the Vite dev server with mocked Tauri commands.
 */
export default defineConfig({
  testDir: './e2e-playwright',

  // Run tests in parallel for speed
  fullyParallel: true,

  // Fail CI if test.only is committed
  forbidOnly: !!process.env.CI,

  // Derived above (validated PW_RETRIES override; CI 2 / local 1).
  retries,

  // Derived + validated above (PW_WORKERS: bare int -> number, "50%" kept as-is;
  // CI defaults to "100%" / all cores). See #120, #132. Omitted rather than
  // `undefined` when unset (exactOptionalPropertyTypes): Playwright reads an
  // absent and an undefined `workers` the same way, falling through to its own
  // default.
  ...(workers === undefined ? {} : { workers }),

  // Test reporter: 'list' for live progress, 'github' for CI annotations
  // Override with PW_REPORTER env var (e.g. PW_REPORTER=html)
  reporter: process.env.PW_REPORTER || (process.env.CI ? 'github' : 'list'),

  // Shared settings for all projects
  use: {
    // Base URL for the Vite dev server
    baseURL: `http://localhost:${port}`,
    locale: 'en-US',

    // Collect trace when retrying a failed test
    trace: 'on-first-retry',

    // Screenshots on failure
    screenshot: 'only-on-failure',

    // Video recording on failure
    video: 'retain-on-failure',
  },

  // Configure projects for cross-browser testing
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    // Add Firefox and WebKit if needed
    // {
    //   name: 'firefox',
    //   use: { ...devices['Desktop Firefox'] },
    // },
    // {
    //   name: 'webkit',
    //   use: { ...devices['Desktop Safari'] },
    // },
  ],

  // Run local dev server before starting tests
  webServer: {
    command: devServerCommand,
    url: `http://localhost:${port}`,
    // Never reuse a server in the built-bundle mode: an already-running dev
    // server on 1420 would be silently reused and the run would test unbundled
    // source instead of the bundle it was asked for.
    reuseExistingServer: !process.env.CI && !coverageEnabled && !prodServer,
    // The built-bundle mode has to run `vite build` first, so it needs more
    // headroom than a dev-server boot on a 2-core runner.
    timeout: prodServer ? 300000 : 120000,
  },

  // Test timeout
  timeout: 30000,

  // Expect timeout
  expect: {
    timeout: 5000,
  },

  // Output folder for test artifacts
  outputDir: 'test-results',
});
