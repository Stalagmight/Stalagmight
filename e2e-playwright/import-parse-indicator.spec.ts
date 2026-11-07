import { test, expect, type Locator, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';

/**
 * #576 Q-W27-1-D1 / W62 item 5 / W71: the "Reading <file>..." pill an import
 * shows while its file parses with no dialog up floats fully above the status
 * bar both views end in, at the usual `bottom-6` (24 px) gap - not 24 px from
 * the window edge, where it straddled the bar's top border. It takes the bar's
 * height from the clearance the bar registers for the toast stack, so a taller
 * bar (one that wraps) carries the pill up with it.
 *
 * In the project view it also floats above the map's imagery attribution, which
 * rides the map's bottom edge just above the bar: Esri's terms want the source
 * names unobstructed while the imagery is drawn.
 *
 * The parse is real: a `.dmp` picked through the app's own door, its
 * `import_dmp` held on a promise so the pill stays up while it is measured.
 * What the pill says and when it shows are pinned in
 * src/components/common/__tests__/ImportParseIndicator.test.tsx.
 */

const FILE = '/mock/CaveA.dmp';
/** The pill's gap above what it clears: `bottom-6`, in CSS px at the default UI scale. */
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

/** Pixels from the pill's bottom edge down to `below`'s top edge (NaN while either has no box). */
async function gapAbove(pill: Locator, below: Locator): Promise<number> {
  const pillBox = await pill.boundingBox();
  const belowBox = await below.boundingBox();
  if (pillBox === null || belowBox === null) return Number.NaN;
  return belowBox.y - (pillBox.y + pillBox.height);
}

/** The pill is up, and its bottom edge settles GAP_PX above `below`'s top edge. */
async function expectPillAbove(pill: Locator, below: Locator, what: string): Promise<void> {
  await expect(pill).toBeVisible();
  await expect(pill).toHaveText('Reading CaveA.dmp...');
  await expect
    .poll(() => gapAbove(pill, below), { message: `the pill's gap above ${what}` })
    .toBeGreaterThan(GAP_PX - 1);
  expect(await gapAbove(pill, below), `the pill's gap above ${what}`).toBeLessThan(GAP_PX + 1);
}

/** Turn the satellite imagery (and with it the attribution) on or off from the Display menu. */
async function setSatellite(page: Page, on: boolean): Promise<void> {
  await page.getByTestId('display-menu-button').click();
  const toggle = page.getByTestId('display-toggle-showSatellite');
  await expect(toggle).toBeVisible();
  const isOn = await toggle.evaluate((el) => el.className.includes('bg-accent'));
  if (isOn !== on) await toggle.click();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('map-attribution')).toHaveCount(on ? 1 : 0);
}

test.describe('the import parse indicator clears the status bar (#576 W62)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('on the Global View, while a picked file parses', async ({ page }) => {
    await holdTheNextDmpParse(page);
    await page.getByTestId('global-open-import-button').click();

    await expectPillAbove(
      page.getByTestId('import-parse-indicator'),
      page.getByTestId('global-view').locator('footer'),
      'the status bar',
    );
  });

  test('on the Global View, a taller status bar carries the pill up with it (#576 W71)', async ({ page }) => {
    await holdTheNextDmpParse(page);
    await page.getByTestId('global-open-import-button').click();
    const pill = page.getByTestId('import-parse-indicator');
    const statusBar = page.getByTestId('global-view').locator('footer');
    await expectPillAbove(pill, statusBar, 'the status bar');

    // A bar that wraps to a second line (a narrow window, a large UI scale)
    // grows upward; stand in for the wrap with a taller box.
    const before = (await statusBar.boundingBox())!.height;
    await statusBar.evaluate((el) => {
      (el as HTMLElement).style.minHeight = `${(el as HTMLElement).offsetHeight * 2}px`;
    });
    await expect.poll(async () => (await statusBar.boundingBox())!.height).toBeGreaterThan(before);

    await expectPillAbove(pill, statusBar, 'the taller status bar');
  });

  test('in a project, while a DMP picked for it parses: above the map attribution (#576 W71)', async ({ page }) => {
    await createProject(page, 'ProjectA');
    const attribution = page.getByTestId('map-attribution');
    await expect(attribution).toBeVisible();
    await holdTheNextDmpParse(page);
    await page.keyboard.press('Control+i');

    const pill = page.getByTestId('import-parse-indicator');
    await expectPillAbove(pill, attribution, 'the map attribution');
    // Clearing the attribution clears the status bar below it as well.
    expect(await gapAbove(pill, page.getByTestId('status-bar'))).toBeGreaterThan(GAP_PX);
  });

  test('in a project with the imagery off: above the status bar', async ({ page }) => {
    await createProject(page, 'ProjectA');
    await setSatellite(page, false);
    await holdTheNextDmpParse(page);
    await page.keyboard.press('Control+i');

    await expectPillAbove(page.getByTestId('import-parse-indicator'), page.getByTestId('status-bar'), 'the status bar');
  });
});
