import { defineConfig, type PluginOption } from 'vite';
import react from '@vitejs/plugin-react';
import istanbul from 'vite-plugin-istanbul';
import { execSync } from 'node:child_process';
import pkg from './package.json';
// Dev-only console/eval bridge (apply: 'serve', so inert in builds). See
// scripts/lib/vite-agent-bridge.mjs and docs/debug-gates.md.
import { agentBridge } from './scripts/lib/vite-agent-bridge.mjs';

/** Short hash of the checked-out commit, or `unknown` when git is unavailable. */
function gitShortHash(): string {
  try {
    return execSync('git rev-parse --short HEAD', { stdio: ['ignore', 'pipe', 'ignore'] })
      .toString()
      .trim() || 'unknown';
  } catch {
    return 'unknown';
  }
}

/**
 * The release tag on the checked-out commit, or null.
 *
 * Must match this package version - `v0.5.0` or a candidate for it like
 * `v0.5.0-rc1` - so an unrelated tag sitting on the same commit cannot rename
 * the build.
 */
function releaseTag(): string | null {
  const matches = (t: string) => t === `v${pkg.version}` || t.startsWith(`v${pkg.version}-`);
  // In Actions on a tag push this is authoritative and needs no git state.
  if (process.env.GITHUB_REF_TYPE === 'tag') {
    const ref = process.env.GITHUB_REF_NAME ?? '';
    return matches(ref) ? ref : null;
  }
  // Otherwise ask git, so a release built by hand identifies itself the same way.
  try {
    return (
      execSync('git tag --points-at HEAD', { stdio: ['ignore', 'pipe', 'ignore'] })
        .toString()
        .split('\n')
        .map((t) => t.trim())
        .find(matches) ?? null
    );
  } catch {
    return null;
  }
}

// A released build reports its tag: the tag already pins the commit, so the hash
// is noise on the one build users actually read a version number off - and a
// candidate says so (0.5.0-rc1) rather than passing for the final release.
// Everything else keeps the commit, because that IS its only identity:
// `x.y.z-<shorthash>` for CI builds and `x.y.z-dev-<shorthash>` locally.
// Mirrors src-tauri/build.rs so the UI's About version and telemetry's
// app_version agree.
const shortHash = gitShortHash();
const tag = releaseTag();
const appVersion = tag
  ? tag.slice(1) // v0.5.0-rc1 -> 0.5.0-rc1
  : process.env.CI
    ? `${pkg.version}-${shortHash}`
    : `${pkg.version}-dev-${shortHash}`;

const coverageEnabled = process.env.VITE_COVERAGE === '1';
const plugins: PluginOption[] = [
  react(),
  agentBridge(),
  coverageEnabled
    ? istanbul({
        include: 'src/**/*',
        exclude: [
          '**/*.d.ts',
          '**/*.{test,spec}.?(c|m)[jt]s?(x)',
          '**/__tests__/**',
          '**/e2e/**',
          '**/e2e-playwright/**',
          '**/node_modules/**',
          '**/dist/**',
          '**/src-tauri/**',
          'src/types/ipc/**',
          'src/types/ipc.generated.ts',
        ],
        extension: ['.ts', '.tsx'],
        forceBuildInstrument: true,
        requireEnv: false,
      })
    : undefined,
].filter(Boolean) as PluginOption[];

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
    // Shown separately in About. A released build's version no longer carries
    // the hash, but a bug report still needs to say which build it came from.
    __APP_COMMIT__: JSON.stringify(shortHash),
    // Which update channel this build was published on (#579). Set by the
    // release workflow for weekly builds; defaults to 'stable' everywhere
    // else (dev, and stable release builds), so the About dialog's channel
    // line is hidden unless it says otherwise.
    __APP_CHANNEL__: JSON.stringify(process.env.STALAGMIGHT_CHANNEL ?? 'stable'),
  },
  // Force a single React instance. Vite's dep optimizer can intermittently
  // pre-bundle two copies of React, which surfaces as "Invalid hook call" + a
  // blank window. This was first seen on a Node major above the then-pinned 22;
  // the pin is Node 24 now (.nvmrc), so these two settings are what keeps it
  // from recurring rather than the Node version. dedupe makes every
  // `react`/`react-dom` import resolve to one copy, and the explicit
  // optimizeDeps entry keeps them pre-bundled together.
  resolve: {
    dedupe: ['react', 'react-dom'],
  },
  optimizeDeps: {
    include: ['react', 'react-dom'],
  },
  plugins,
  clearScreen: false,
  build: coverageEnabled ? { sourcemap: true } : undefined,
  server: {
    port: 1420,
    strictPort: true,

    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  test: {
    // scripts/**/*.test.ts: golden tests for the repo's Node tooling (e.g. the
    // agent replay/repro generators) so a parser regression there fails
    // `pnpm test`. Deliberately .ts-only: the standalone *.test.mjs files in
    // scripts/ are plain node self-test scripts (run via lint:*:test), not
    // vitest suites.
    // e2e/agent/lib/**/*.test.ts: pure-logic unit tests for agent-harness helpers
    // (e.g. build freshness). Scoped to `lib/` - the helper directory - rather than
    // to all of e2e/agent, because every SIBLING directory there is runtime code for
    // a different runner: `replays/` holds generated replay specs, `scenarios/`
    // drives the live app, and `mcp/` is the server. An exclude list only covered
    // `replays/`, so a scenario named *.test.ts would have been vacuumed in here and
    // failed on missing globals. A new home for vitest suites under e2e/agent is a
    // deliberate one-line addition to this list.
    // e2e/agent/os/**/*.test.ts: the Tier 2 OS-input layer, which takes its helper
    // runner and its osascript/screencapture execs as injected deps precisely so
    // these run unattended - nothing in them posts a real event or moves the cursor.
    // e2e/lib/**/*.test.ts: pure-logic unit tests for the shared harness helpers
    // (e.g. the profile config-path resolution both runners depend on). Same
    // reasoning as the e2e/agent/lib entry above - `lib/` is helper code that
    // runs under plain vitest.
    include: [
      'src/**/*.{test,spec}.?(c|m)[jt]s?(x)',
      'scripts/**/*.{test,spec}.ts',
      'e2e/agent/lib/**/*.test.ts',
      'e2e/agent/os/**/*.test.ts',
      'e2e/lib/**/*.test.ts',
    ],
    exclude: [
      '**/e2e/agent/replays/**',
      '**/e2e-playwright/**',
      '**/node_modules/**',
      '**/dist/**',
      '**/src-tauri/**',
    ],
    // Tiered by extension (issue #107): pure-logic tests stay in `node` (fast,
    // no DOM cost); component tests (`*.{test,spec}.tsx`) get a jsdom DOM so
    // real React components can be rendered with Testing Library.
    environment: 'node',
    environmentMatchGlobs: [['**/*.{test,spec}.tsx', 'jsdom']],
    setupFiles: ['./src/test/setup.ts'],
    coverage: {
      provider: 'v8',
      all: true,
      reportsDirectory: 'docs/coverage',
      include: ['src/**/*.{ts,tsx}'],
      exclude: [
        '**/*.d.ts',
        '**/*.{test,spec}.?(c|m)[jt]s?(x)',
        '**/__tests__/**',
        '**/e2e/**',
        '**/e2e-playwright/**',
        '**/node_modules/**',
        '**/dist/**',
        '**/src-tauri/**',
        '**/coverage/**',
        'src/types/ipc/**',
        'src/types/ipc.generated.ts',
      ],
      reporter: ['text', 'html', 'json', 'lcov'],
    },
  },
});
