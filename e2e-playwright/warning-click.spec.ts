/**
 * E2E tests for warning click navigation.
 *
 * Verifies that clicking a warning in the Warnings tab navigates to the
 * corresponding shot's details (not "Selection not found").
 * Also covers DMP-imported projects (simulated via multiple seeded lines).
 */

import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapIdle,
  type LineSeed,
} from './helpers/app';
import { warningMessages, warningResolveButtons } from './helpers/testIds';

/** Seed data: one line with shots. */
const lineSeeds: LineSeed[] = [
  {
    name: 'Main Tunnel',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      { azimuth_in: 120, azimuth_out: 300, length: 8, depth_in: -6, depth_out: -8 },
    ],
  },
];

/** Seed data simulating a DMP import: multiple lines. */
const dmpSectionSeeds: LineSeed[] = [
  {
    name: 'Main Passage',
    direction: 'in',
    shots: [
      { azimuth_in: 30, azimuth_out: 210, length: 15, depth_in: -2, depth_out: -5 },
      { azimuth_in: 60, azimuth_out: 240, length: 10, depth_in: -5, depth_out: -9 },
    ],
  },
  {
    name: 'Side Tunnel',
    direction: 'out',
    shots: [
      { azimuth_in: 180, azimuth_out: 0, length: 7, depth_in: -3, depth_out: -6 },
    ],
  },
];

/**
 * Inject mock warnings into the project store.
 * Warnings use camelCase field names matching the Rust #[serde(rename_all = "camelCase")] convention.
 */
async function injectWarnings(
  page: Page,
  warnings: Array<{
    lineId: number;
    shotId: number;
    shotIndex: number;
    code: string;
    message: string;
    severity: 'warning' | 'error';
    isResolved: boolean;
    datumStationIndex?: number;
    datumRefs?: Array<{ line_id: number; station_index: number }>;
    shotUuid?: string;
  }>
): Promise<void> {
  await page.evaluate((w) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          setState: (state: { warnings: unknown[] }) => void;
        };
      };
    }).__E2E_TEST__;
    if (stores) {
      stores.projectStore.setState({ warnings: w });
    }
  }, warnings);
}

/** Get line IDs from the project store. */
async function getLineIds(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { project: { lines: Array<{ id: number }> } | null };
        };
      };
    }).__E2E_TEST__;
    return stores?.projectStore.getState().project?.lines.map((s) => s.id) ?? [];
  });
}

/** Get shot IDs for a line. */
async function getShotIdsForLine(page: Page, lineId: number): Promise<number[]> {
  return page.evaluate(
    (secId) => {
      const stores = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: { lines: Array<{ id: number; shots: Array<{ id: number }> }> } | null;
            };
          };
        };
      }).__E2E_TEST__;
      const line = stores?.projectStore.getState().project?.lines.find((s) => s.id === secId);
      return line?.shots.map((shot) => shot.id) ?? [];
    },
    lineId
  );
}

/** Get the current selection state. */
async function getSelectionState(page: Page): Promise<{
  type: string;
  lineId?: number;
  shotId?: number;
} | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { selection: unknown } };
      };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().selection as {
      type: string;
      lineId?: number;
      shotId?: number;
    } | null;
  });
}

/** Poll for selection state changes. */
async function pollSelectionState(
  page: Page,
  predicate: (sel: Awaited<ReturnType<typeof getSelectionState>>) => boolean,
  timeout = 5000
): Promise<Awaited<ReturnType<typeof getSelectionState>>> {
  let result: Awaited<ReturnType<typeof getSelectionState>> = null;
  await expect
    .poll(
      async () => {
        const sel = await getSelectionState(page);
        if (predicate(sel)) {
          result = sel;
          return true;
        }
        return false;
      },
      { timeout }
    )
    .toBe(true);
  return result;
}

test.describe('Warning Click Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('clicking a warning selects the shot and stays on the Warnings tab', async ({ page }) => {
    // Create project with lines
    await createProject(page, 'Warning Click Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    // Get line and shot IDs
    const lineIds = await getLineIds(page);
    const mainLineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, mainLineId);
    const targetShotId = shotIds[1]!; // Second shot

    // Inject a warning for the second shot
    await injectWarnings(page, [
      {
        lineId: mainLineId,
        shotId: targetShotId,
        shotIndex: 1,
        code: 'AZIMUTH_DIFF',
        message: `Shot ${mainLineId}:2 azimuth difference exceeds threshold`,
        severity: 'warning',
        isResolved: false,
      },
    ]);

    // Switch to Warnings tab
    const warningsTab = page.getByTestId('detail-tab-warnings');
    await warningsTab.click();

    // Verify the warning is displayed
    const warningText = page.locator('text=azimuth difference exceeds threshold');
    await expect(warningText).toBeVisible();

    // Clicking the warning row (the message area, away from the shot pill)
    // selects the problematic shot but stays on the Warnings tab (issue #57);
    // the entity pills are the way to jump to Details.
    await warningMessages(page).first().click({ position: { x: 4, y: 4 } });

    const selection = await pollSelectionState(
      page,
      (sel) => sel?.type === 'shot' && sel?.shotId === targetShotId
    );
    expect(selection?.type).toBe('shot');
    expect(selection?.lineId).toBe(mainLineId);
    expect(selection?.shotId).toBe(targetShotId);

    // Still on the Warnings tab - the row select does not navigate to Details.
    await expect(page.getByTestId('warnings-content')).toBeVisible();
  });

  test('clicking a warning on a DMP-imported project selects the shot (stays on Warnings tab)', async ({
    page,
  }) => {
    // Create project with multiple lines (simulating a DMP import)
    await createProject(page, 'DMP Warning Test');
    await seedProjectWithLines(page, dmpSectionSeeds);
    await waitForMapIdle(page);

    // Get line and shot IDs
    const lineIds = await getLineIds(page);
    expect(lineIds.length).toBe(2);
    const firstLineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, firstLineId);
    expect(shotIds.length).toBeGreaterThanOrEqual(1);
    const targetShotId = shotIds[0]!;

    // Inject a warning for the first shot of the first line
    await injectWarnings(page, [
      {
        lineId: firstLineId,
        shotId: targetShotId,
        shotIndex: 0,
        code: 'LENGTH_ZERO',
        message: `Shot ${firstLineId}:1 length is zero`,
        severity: 'error',
        isResolved: false,
      },
    ]);

    // Switch to Warnings tab
    const warningsTab = page.getByTestId('detail-tab-warnings');
    await warningsTab.click();

    // Verify the warning is displayed
    const warningText = page.locator('text=length is zero');
    await expect(warningText).toBeVisible();

    // Clicking the row (message area, away from the shot pill) selects the shot
    // but stays on the Warnings tab (#57).
    await warningMessages(page).first().click({ position: { x: 4, y: 4 } });

    const selection = await pollSelectionState(
      page,
      (sel) => sel?.type === 'shot' && sel?.shotId === targetShotId
    );
    expect(selection?.type).toBe('shot');
    expect(selection?.lineId).toBe(firstLineId);
    expect(selection?.shotId).toBe(targetShotId);

    // Still on the Warnings tab - the row select does not navigate to Details.
    await expect(page.getByTestId('warnings-content')).toBeVisible();
  });
});

test.describe('Validation Error Recovery (SPEC 14.5)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('Resolve button is rendered for unresolved warnings', async ({ page }) => {
    await createProject(page, 'Dismiss Warning Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineIds = await getLineIds(page);
    const lineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);

    // Inject two unresolved warnings
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'LENGTH_SHORT',
        message: 'Shot length below minimum',
        severity: 'warning',
        isResolved: false,
      },
      {
        lineId,
        shotId: shotIds[1]!,
        shotIndex: 1,
        code: 'AZIMUTH_DIFF',
        message: 'Azimuth difference exceeds threshold',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    // Switch to Warnings tab
    await page.getByTestId('detail-tab-warnings').click();

    // Both warnings visible
    await expect(page.locator('text=Shot length below minimum')).toBeVisible();
    await expect(page.locator('text=Azimuth difference exceeds threshold')).toBeVisible();

    // Resolve buttons rendered for each unresolved warning
    const resolveButtons = warningResolveButtons(page);
    await expect(resolveButtons).toHaveCount(2);
    await expect(resolveButtons.first()).toHaveText('Resolve');
  });

  test('Resolve all button is visible when unresolved warnings exist', async ({ page }) => {
    await createProject(page, 'Resolve All Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineIds = await getLineIds(page);
    const lineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);

    // Inject warnings
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'LENGTH_ZERO',
        message: 'Shot has zero length',
        severity: 'error',
        isResolved: false,
      },
      {
        lineId,
        shotId: shotIds[1]!,
        shotIndex: 1,
        code: 'AZIMUTH_DIFF',
        message: 'Azimuth difference too large',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    await page.getByTestId('detail-tab-warnings').click();

    // Resolve all button should be visible
    const resolveAllBtn = page.getByTestId('resolve-all-warnings');
    await expect(resolveAllBtn).toBeVisible();
    await expect(resolveAllBtn).toHaveText('Resolve all');
  });

  test('Resolve opens the inline resolution view and Back returns to the list', async ({ page }) => {
    await createProject(page, 'Resolve Flow Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineIds = await getLineIds(page);
    const lineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);

    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'AZIMUTH_DEVIATION',
        message: 'Azimuth in/out difference exceeds threshold',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    await page.getByTestId('detail-tab-warnings').click();
    await warningResolveButtons(page).first().click();

    // The resolution view reuses this panel (issue #57) and offers the
    // azimuth-specific options plus Ignore.
    await expect(page.getByTestId('warning-resolution')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-average')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-use-in')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-ignore')).toBeVisible();

    // Back returns to the list.
    await page.getByTestId('warning-resolution-back').click();
    await expect(page.getByTestId('warnings-content')).toBeVisible();
    await expect(warningResolveButtons(page).first()).toBeVisible();
  });

  test('Resolve all steps through warnings with a progress indicator', async ({ page }) => {
    await createProject(page, 'Resolve All Flow Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineIds = await getLineIds(page);
    const lineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);

    await injectWarnings(page, [
      { lineId, shotId: shotIds[0]!, shotIndex: 0, code: 'AZIMUTH_DEVIATION', message: 'Azimuth deviation', severity: 'warning', isResolved: false },
      { lineId, shotId: shotIds[1]!, shotIndex: 1, code: 'SHORT_SHOT', message: 'Short shot', severity: 'warning', isResolved: false },
    ]);

    await page.getByTestId('detail-tab-warnings').click();
    await page.getByTestId('resolve-all-warnings').click();

    await expect(page.getByTestId('warning-resolution')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-progress')).toHaveText('1 of 2');
    await expect(page.getByTestId('warning-resolution-skip')).toBeVisible();
  });

  test('GPS gap warning offers go-to-station and edit-datum, and opens Edit Station', async ({ page }) => {
    await createProject(page, 'Datum Gap Resolve Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);

    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'datum_gps_gap',
        message: 'The GPS of this zero datum is 60 m from where the survey puts it.',
        severity: 'warning',
        isResolved: false,
        datumStationIndex: 1,
      },
    ]);

    await page.getByTestId('detail-tab-warnings').click();
    await warningResolveButtons(page).first().click();

    // A GPS fix that disagrees with the survey (#741) is advisory: look at the
    // station, fix its datum in Edit Station, or ignore it.
    await expect(page.getByTestId('warning-resolution')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-go-to-station')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-edit-datum')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-ignore')).toBeVisible();

    await page.getByTestId('warning-resolution-option-edit-datum').check();
    await page.getByTestId('warning-resolution-apply').click();
    await expect(page.getByTestId('edit-station-dialog')).toBeVisible();
  });

  test('inclination/depth mismatch offers use-depth and use-inclination resolutions', async ({ page }) => {
    await createProject(page, 'Inclination Mismatch Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'INCLINATION_DEPTH_MISMATCH',
        message: 'Shot 1:1 recorded inclination disagrees with the depth/length geometry',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    await page.getByTestId('detail-tab-warnings').click();
    await warningResolveButtons(page).first().click();
    await expect(page.getByTestId('warning-resolution')).toBeVisible();
    // Depth is trusted, so resolution adjusts the inclination (and length when a
    // pitch is recorded) - never the depth. This seed shot has no recorded pitch,
    // so the length/split options are omitted; adjust-inclination stays, and the
    // old depth-changing option must not appear.
    await expect(page.getByTestId('warning-resolution-option-incl-from-depth')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-depth-from-incl')).toHaveCount(0);
    await expect(page.getByTestId('warning-resolution-option-ignore')).toBeVisible();
  });

  test('short shot offers a delete-shot resolution', async ({ page }) => {
    await createProject(page, 'Short Shot Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'SHORT_SHOT',
        message: 'Shot 1:1 length 0.2m below minimum threshold 0.3m',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    await page.getByTestId('detail-tab-warnings').click();
    await warningResolveButtons(page).first().click();
    await expect(page.getByTestId('warning-resolution')).toBeVisible();
    // A shot failing the minimum-length test offers Set length, Delete this shot,
    // and Ignore. "Set length" is the default (first enabled); delete is offered.
    await expect(page.getByTestId('warning-resolution-option-manual')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-delete-shot')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-ignore')).toBeVisible();

    // Deleting selects the parent line (not the now-gone shot), so Details
    // doesn't show "Selection not found".
    await page.getByTestId('warning-resolution-option-delete-shot').click();
    await page.getByTestId('warning-resolution-apply').click();
    const selection = await pollSelectionState(page, (sel) => sel?.type === 'line');
    expect(selection?.type).toBe('line');
    expect(selection?.lineId).toBe(lineId);
  });

  test('virtualizes the warnings list (only a window of rows is in the DOM)', async ({ page }) => {
    await createProject(page, 'Warnings Virtualization Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);
    const many = Array.from({ length: 200 }, (_, i) => ({
      lineId,
      shotId: shotIds[i % shotIds.length]!,
      shotIndex: i % shotIds.length,
      code: 'AZIMUTH_DEVIATION',
      message: `Shot ${lineId}:${(i % shotIds.length) + 1} azimuth in/out difference #${i}`,
      severity: 'warning' as const,
      isResolved: false,
    }));
    await injectWarnings(page, many);

    await page.getByTestId('detail-tab-warnings').click();
    await expect(page.getByTestId('warnings-content')).toBeVisible();
    // With 200 warnings the virtualizer mounts only the visible window, not all
    // rows - so far fewer Resolve buttons are in the DOM than warnings.
    const rendered = await warningResolveButtons(page).count();
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(many.length);
  });

  test('clicking the data-table warning indicator opens Warnings and the resolve view', async ({ page }) => {
    await createProject(page, 'Table Warning Resolve Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'AZIMUTH_DEVIATION',
        message: 'Shot 1:1 azimuth in/out difference exceeds threshold',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    // Open the data table and click the warning indicator on that shot's row.
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
    const indicator = page.getByTestId(`warning-${lineId}-${shotIds[0]!}`);
    await expect(indicator).toBeVisible();
    await indicator.click();

    // The Warnings tab opens straight into the resolution view for that issue.
    await expect(page.getByTestId('warning-resolution')).toBeVisible();
    await expect(page.getByTestId('warning-resolution-option-average')).toBeVisible();
  });

  test('an inclination in/out warning offers keep-in, keep-out, average (default), set and ignore', async ({ page }) => {
    await createProject(page, 'Pitch Deviation Resolve Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'PITCH_DEVIATION',
        message: 'Shot 1:1 inclination in/out difference 14.0° exceeds threshold 10.0°',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
    await page.getByTestId(`warning-${lineId}-${shotIds[0]!}`).click();

    await expect(page.getByTestId('warning-resolution')).toBeVisible();
    for (const id of ['pitch-use-in', 'pitch-use-out', 'pitch-average', 'manual', 'ignore']) {
      await expect(page.getByTestId(`warning-resolution-option-${id}`)).toBeVisible();
    }
    await expect(page.getByTestId('warning-resolution-option-pitch-average')).toBeChecked();
  });

  test('data-table warning indicator opens the resolve view even when the detail panel is closed', async ({ page }) => {
    await createProject(page, 'Closed Panel Resolve Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'AZIMUTH_DEVIATION',
        message: 'Shot 1:1 azimuth in/out difference exceeds threshold',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });

    // Collapse the detail panel (as line-focus mode does).
    await page.evaluate(() => {
      (window as unknown as { __E2E_TEST__?: { uiStore: { setState: (s: Record<string, unknown>) => void } } })
        .__E2E_TEST__?.uiStore.setState({ detailPanelOpen: false });
    });
    await expect(page.getByTestId('detail-panel')).toHaveCount(0);

    // Clicking the indicator must reopen the panel and open the resolve view.
    await page.getByTestId(`warning-${lineId}-${shotIds[0]!}`).click();
    await expect(page.getByTestId('warning-resolution')).toBeVisible();
  });

  test('opening Resolve fully focuses the shot and offers a zoom button', async ({ page }) => {
    await createProject(page, 'Resolve Focus Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineId = (await getLineIds(page))[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'AZIMUTH_DEVIATION',
        message: 'Shot 1:1 azimuth in/out difference exceeds threshold',
        severity: 'warning',
        isResolved: false,
      },
    ]);

    await page.getByTestId('detail-tab-warnings').click();
    await warningResolveButtons(page).first().click();
    await expect(page.getByTestId('warning-resolution')).toBeVisible();

    // Full focus: the shot is selected (which highlights it on the map and
    // focuses the details, lines and data panels - same as a link click).
    const selection = await pollSelectionState(
      page,
      (sel) => sel?.type === 'shot' && sel?.shotId === shotIds[0]!,
    );
    expect(selection?.lineId).toBe(lineId);
    expect(selection?.shotId).toBe(shotIds[0]!);

    // A zoom button is offered to close in on the problematic shot.
    await expect(page.getByTestId('warning-resolution-zoom')).toBeVisible();
    await page.getByTestId('warning-resolution-zoom').click(); // does not throw
  });

  test('badge only counts unresolved warnings', async ({ page }) => {
    await createProject(page, 'Badge Count Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineIds = await getLineIds(page);
    const lineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);

    // Inject 3 warnings: 2 unresolved, 1 resolved
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'LENGTH_SHORT',
        message: 'Shot length below minimum',
        severity: 'warning',
        isResolved: false,
      },
      {
        lineId,
        shotId: shotIds[1]!,
        shotIndex: 1,
        code: 'AZIMUTH_DIFF',
        message: 'Azimuth difference too large',
        severity: 'warning',
        isResolved: false,
      },
      {
        lineId,
        shotId: shotIds[2]!,
        shotIndex: 2,
        code: 'LENGTH_LONG',
        message: 'Shot length above maximum',
        severity: 'warning',
        isResolved: true,
      },
    ]);

    // Badge should show 2 (only unresolved), not 3
    const badge = page.getByTestId('detail-tab-warnings').locator('span.rounded-full');
    await expect(badge).toHaveText('2');
  });

  test('resolved warnings hidden by default, shown with toggle', async ({ page }) => {
    await createProject(page, 'Resolved Toggle Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);

    const lineIds = await getLineIds(page);
    const lineId = lineIds[0]!;
    const shotIds = await getShotIdsForLine(page, lineId);

    // Inject one unresolved and one resolved warning
    await injectWarnings(page, [
      {
        lineId,
        shotId: shotIds[0]!,
        shotIndex: 0,
        code: 'LENGTH_SHORT',
        message: 'Shot length below minimum',
        severity: 'warning',
        isResolved: false,
      },
      {
        lineId,
        shotId: shotIds[1]!,
        shotIndex: 1,
        code: 'AZIMUTH_DIFF',
        message: 'Azimuth difference too large',
        severity: 'warning',
        isResolved: true,
      },
    ]);

    await page.getByTestId('detail-tab-warnings').click();

    // Unresolved warning visible
    await expect(page.locator('text=Shot length below minimum')).toBeVisible();
    // Resolved warning hidden by default
    await expect(page.locator('text=Azimuth difference too large')).toBeHidden();

    // Click "Show resolved" toggle
    await page.locator('text=Show resolved').click();

    // Now resolved warning is visible with line-through
    await expect(page.locator('text=Azimuth difference too large')).toBeVisible();
  });
});
