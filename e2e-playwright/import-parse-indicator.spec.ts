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
 * names unobstructed while the imagery is drawn. So does the Global View's map
 * (#576 W71-NF10), whose attribution sits in its bottom-right corner and reaches
 * under the centred pill - all the way across it in a 900 px window. Its
 * "Systems: N" count chip sits just above the attribution in that corner, and
 * in a narrow window reaches under the pill's right end (#576 W79-NF8).
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

type Box = NonNullable<Awaited<ReturnType<Locator['boundingBox']>>>;

/** Whether two boxes share any area. */
function boxesOverlap(a: Box, b: Box): boolean {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

/**
 * On the Global View map, the pill floats the usual gap above the "Systems: N"
 * chip - the highest thing in the map's bottom-right corner - and so clear of it,
 * where the window is narrow enough that the chip runs under the pill (#576 W79-NF8).
 */
async function expectPillClearOfTheSystemsCount(page: Page): Promise<void> {
  await page.getByTestId('globalview-view-toggle-map').click();
  const count = page.getByTestId('globalview-map').getByText('Systems: 3', { exact: true });
  await expect(count).toBeVisible();
  await holdTheNextDmpParse(page);
  await page.getByTestId('global-open-import-button').click();

  const pill = page.getByTestId('import-parse-indicator');
  await expectPillAbove(pill, count, 'the systems count');
  const pillBox = (await pill.boundingBox())!;
  const countBox = (await count.boundingBox())!;
  const span = (b: Box) => `x ${b.x}-${b.x + b.width}, y ${b.y}-${b.y + b.height}`;
  // The case under test: the chip runs under the pill's right end.
  expect(
    pillBox.x < countBox.x + countBox.width && countBox.x < pillBox.x + pillBox.width,
    `the systems count (${span(countBox)}) runs under the pill (${span(pillBox)})`,
  ).toBe(true);
  expect(boxesOverlap(pillBox, countBox), `the pill (${span(pillBox)}) clear of the systems count (${span(countBox)})`).toBe(
    false,
  );
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

  test.describe('in a 900 px window', () => {
    // The Global View map's right edge stops at the metadata panel, so its
    // attribution bar (about 350 px wide) runs right under the centred pill.
    test.use({ viewport: { width: 900, height: 720 } });

    test('on the Global View map: above the map attribution, back above the bar in the list (#576 W71-NF10)', async ({
      page,
    }) => {
      await page.getByTestId('globalview-view-toggle-map').click();
      const attribution = page.getByTestId('map-attribution');
      const names = page.getByTestId('map-attribution-text');
      await expect(names).toBeVisible();
      await holdTheNextDmpParse(page);
      await page.getByTestId('global-open-import-button').click();

      const pill = page.getByTestId('import-parse-indicator');
      await expect(pill).toBeVisible();
      // At least the usual gap: the systems count above the attribution is the
      // higher edge, and sets the pill's height (pinned below).
      await expect
        .poll(() => gapAbove(pill, attribution), { message: "the pill's gap above the map attribution" })
        .toBeGreaterThan(GAP_PX - 1);
      expect(await gapAbove(pill, names), 'the pill leaves the source names uncovered').toBeGreaterThan(GAP_PX);
      // The case under test: the names run right under the pill.
      const pillBox = (await pill.boundingBox())!;
      const namesBox = (await names.boundingBox())!;
      expect(
        pillBox.x < namesBox.x + namesBox.width && namesBox.x < pillBox.x + pillBox.width,
        `the names (x ${namesBox.x}-${namesBox.x + namesBox.width}) run under the pill (x ${pillBox.x}-${pillBox.x + pillBox.width})`,
      ).toBe(true);

      // The list has no attribution: its registration goes with the map.
      await page.getByTestId('globalview-view-toggle-list').click();
      await expect(attribution).toHaveCount(0);
      await expectPillAbove(pill, page.getByTestId('global-view').locator('footer'), 'the status bar');
    });

    test('on the Global View map: clear of the systems count (#576 W79-NF8)', async ({ page }) => {
      await expectPillClearOfTheSystemsCount(page);
    });
  });

  test.describe('in an 800 px window, the minimum width', () => {
    test.use({ viewport: { width: 800, height: 720 } });

    test('on the Global View map: clear of the systems count (#576 W79-NF8)', async ({ page }) => {
      await expectPillClearOfTheSystemsCount(page);
    });
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
