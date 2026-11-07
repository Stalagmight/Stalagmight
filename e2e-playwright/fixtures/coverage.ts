import { test as base, expect } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';

const coverageEnabled = process.env.E2E_COVERAGE === '1';
const defaultCoverageDir = path.resolve(process.cwd(), 'docs', 'coverage', 'raw', 'playwright');

function sanitizeFileName(value: string): string {
  return value.replace(/[^a-zA-Z0-9._-]+/g, '_');
}

/**
 * [#576 E3-D2] A spec that reaches an IPC command the mock does not answer fails at
 * teardown, naming the command. The mock (`../mocks/tauri-ipc.ts`) still answers null so
 * the flow under test is not derailed mid-test, but records the command: live through
 * this binding (which survives a reload or a navigation) and in
 * `window.__MOCK_UNHANDLED_COMMANDS__` (read here at teardown in case a call raced the
 * binding). Commands answered with a fixed no-op on purpose are listed, with a reason
 * each, in `../mocks/ipc/inert.ts`.
 *
 * [#576 NF-W12FE-1] A plugin call the real app refuses (arguments its Rust command cannot
 * take, or a command the capabilities do not grant: `../mocks/ipc/opener.ts`) fails
 * the test the same way, through `__E2E_REPORT_REFUSED_PLUGIN_CALL__` and
 * `window.__MOCK_REFUSED_PLUGIN_CALLS__`. The mock rejects the call as Tauri does, so a
 * caller that catches the rejection would otherwise pass.
 */
const guarded = base.extend<{ _unmockedIpcGuard: void }>({
  _unmockedIpcGuard: [
    async ({ page }, use) => {
      const reached: string[] = [];
      const refused: string[] = [];
      await page.exposeBinding('__E2E_REPORT_UNMOCKED_IPC__', (_source, cmd: unknown) => {
        reached.push(String(cmd));
      });
      await page.exposeBinding('__E2E_REPORT_REFUSED_PLUGIN_CALL__', (_source, refusal: unknown) => {
        refused.push(String(refusal));
      });
      await use();
      try {
        const inPage = await page.evaluate(() => {
          const w = window as unknown as { __MOCK_UNHANDLED_COMMANDS__?: string[]; __MOCK_REFUSED_PLUGIN_CALLS__?: string[] };
          return { unhandled: w.__MOCK_UNHANDLED_COMMANDS__ ?? [], refused: w.__MOCK_REFUSED_PLUGIN_CALLS__ ?? [] };
        });
        reached.push(...inPage.unhandled);
        refused.push(...inPage.refused);
      } catch {
        // The page is already closed: the bindings have delivered what they saw.
      }
      const unmocked = [...new Set(reached)].sort();
      if (unmocked.length > 0) {
        throw new Error(
          `This test reached IPC command(s) the Playwright mock does not answer: ${unmocked.join(', ')}. ` +
            'Add a case to e2e-playwright/mocks/tauri-ipc.ts or its family under mocks/ipc/, ' +
            'or a reasoned entry in mocks/ipc/inert.ts (#576 E3).',
        );
      }
      const refusals = [...new Set(refused)].sort();
      if (refusals.length > 0) {
        throw new Error(
          `This test made plugin call(s) the real app refuses: ${refusals.join('; ')}. ` +
            'Call it through the plugin\'s own JS API, which sends what its command takes, and only if ' +
            'src-tauri/capabilities/default.json grants it (e2e-playwright/mocks/ipc/opener.ts, #576 NF-W12FE-1).',
        );
      }
    },
    { auto: true },
  ],
});

export const test = coverageEnabled
  ? guarded.extend<{ _coverageCollector: void }>({
      _coverageCollector: [async ({ page }, use, testInfo) => {
        await use();
        // teardown: collect coverage after test
        try {
          const coverage = await page.evaluate(() => {
            return (window as unknown as { __coverage__?: unknown }).__coverage__ ?? null;
          });
          if (!coverage) return;

          const coverageDir = process.env.COVERAGE_DIR || defaultCoverageDir;
          fs.mkdirSync(coverageDir, { recursive: true });

          const title = sanitizeFileName(testInfo.titlePath.join(' '));
          const fileBase = sanitizeFileName(path.basename(testInfo.file));
          const fileName = `${fileBase}-${title}-${testInfo.workerIndex}.json`;
          const filePath = path.join(coverageDir, fileName);
          fs.writeFileSync(filePath, JSON.stringify(coverage));
        } catch {
          // page may already be closed - silently ignore
        }
      }, { auto: true }],
    })
  : guarded;
export { expect };
export * from '@playwright/test';
