import { test, expect, type Locator, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';

/**
 * #576 Q-W27-1-D1 / W62 item 5: the "Reading <file>..." pill an import shows
 * while its file parses with no dialog up sits fully above the status bar both
 * views end in, at the usual `bottom-6` (24 px) clearance - not 24 px from the
 * window edge, where it straddled the bar's top border.
 *
 * The parse is real: a `.dmp` picked through the app's own door, its
 * `import_dmp` held on a promise so the pill stays up while it is measured.
 * What the pill says and when it shows are pinned in
 * src/components/common/__tests__/ImportParseIndicator.test.tsx.
 */

const FILE = '/mock/CaveA.dmp';
/** The pill's gap above the status bar: `bottom-6`, in CSS px at the default UI scale. */
const GAP_PX = 24;

interface HeldParseWindow {
  __MOCK_IPC_OVERRIDES__?: Record<string, unknown>;
}

/** Answer the next picker with FILE, and never settle its parse. */
async function holdTheNextDmpParse(page: Page): Promise<void> {
  await page.evaluate((path) => {
    const w = window as unknown as HeldParseWindow;
    w.__MOCK_IPC_OVERRIDES__ = {
      ...(w.__MOCK_IPC_OVERRIDES__ ?? {}),
      'plugin:dialog|open': path,
      import_dmp: new Promise(() => {}),
    };
  }, FILE);
}

/** The pill's bottom edge sits GAP_PX above the status bar's top edge. */
async function expectAboveStatusBar(pill: Locator, statusBar: Locator): Promise<void> {
  await expect(pill).toBeVisible();
  await expect(pill).toHaveText('Reading CaveA.dmp...');
  const pillBox = await pill.boundingBox();
  const barBox = await statusBar.boundingBox();
  expect(pillBox, 'the pill has a box').not.toBeNull();
  expect(barBox, 'the status bar has a box').not.toBeNull();
  const gap = barBox!.y - (pillBox!.y + pillBox!.height);
  expect(gap, `pill bottom ${pillBox!.y + pillBox!.height}, status bar top ${barBox!.y}`).toBeGreaterThan(GAP_PX - 1);
  expect(gap, `pill bottom ${pillBox!.y + pillBox!.height}, status bar top ${barBox!.y}`).toBeLessThan(GAP_PX + 1);
}

test.describe('the import parse indicator clears the status bar (#576 W62)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('on the Global View, while a picked file parses', async ({ page }) => {
    await holdTheNextDmpParse(page);
    await page.getByTestId('global-open-import-button').click();

    await expectAboveStatusBar(
      page.getByTestId('import-parse-indicator'),
      page.getByTestId('global-view').locator('footer'),
    );
  });

  test('in a project, while a DMP picked for it parses', async ({ page }) => {
    await createProject(page, 'ProjectA');
    await holdTheNextDmpParse(page);
    await page.keyboard.press('Control+i');

    await expectAboveStatusBar(page.getByTestId('import-parse-indicator'), page.getByTestId('status-bar'));
  });
});
