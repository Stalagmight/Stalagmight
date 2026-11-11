import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  createSectionViaStore,
  openDmpSelectionDialog,
} from './helpers/app';

async function openOrphanDialog(page: Page): Promise<void> {
  await openOrphanDialogWith(page, [
    {
      orphanSectionId: 2,
      orphanSectionName: 'Orphan Branch',
      orphanStationIndex: 1,
      targetSectionId: 1,
      targetSectionName: 'Anchor Mainline',
      targetStationIndex: 1,
      score: 0.91,
      matchedShots: 0,
      direction: 'forward',
      isPartial: false,
      confidence: 'high',
    },
    {
      orphanSectionId: 2,
      orphanSectionName: 'Orphan Branch',
      orphanStationIndex: 1,
      targetSectionId: 1,
      targetSectionName: 'Anchor Mainline',
      targetStationIndex: 0,
      score: 0.74,
      matchedShots: 0,
      direction: 'forward',
      isPartial: false,
      confidence: 'medium',
    },
  ]);
}

async function openOrphanDialogWith(page: Page, suggestions: Array<Record<string, unknown>>): Promise<void> {
  await page.evaluate((suggestionList) => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: {
          getState: () => {
            openDialog: (dialog: string, data?: Record<string, unknown>) => void;
          };
        };
      };
    }).__E2E_TEST__;

    if (!e2e?.uiStore) {
      throw new Error('E2E UI store unavailable');
    }

    e2e.uiStore.getState().openDialog('orphan-placement', { suggestions: suggestionList });
  }, suggestions);
}

interface ProjectShape {
  sections: Array<{ id: number; shots: Array<{ id: number; kind: string; length: number; azimuth_in: number }> }>;
}

async function readProject(page: Page): Promise<ProjectShape> {
  return page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__: { projectStore: { getState: () => { project: unknown } } };
    }).__E2E_TEST__;
    return e2e.projectStore.getState().project as ProjectShape;
  });
}

test.describe('Orphan + Zero Datum Workflow', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('accepts orphan placement suggestion and executes merge', async ({ page }) => {
    await createProject(page, 'Orphan Merge Scenario');

    await createSectionViaStore(page, {
      name: 'Anchor Mainline',
      shots: [
        { azimuth_in: 10, azimuth_out: 10, length: 10, depth_in: -1, depth_out: -1.5 },
        { azimuth_in: 20, azimuth_out: 20, length: 8, depth_in: -1.5, depth_out: -2 },
      ],
    });

    await createSectionViaStore(page, {
      name: 'Orphan Branch',
      shots: [
        { azimuth_in: 100, azimuth_out: 100, length: 6, depth_in: -2, depth_out: -2.2 },
        { azimuth_in: 110, azimuth_out: 110, length: 5, depth_in: -2.2, depth_out: -2.4 },
      ],
    });

    await openOrphanDialog(page);
    await expect(page.getByTestId('orphan-placement-dialog')).toBeVisible({ timeout: 10000 });

    await page.getByTestId('toggle-alternatives').click();
    await expect(page.getByTestId('alternatives-list')).toBeVisible();
    await page.getByTestId('alternative-0').click();

    await page.getByTestId('orphan-accept').click();
    await expect(page.getByTestId('orphan-placement-dialog')).toBeHidden({ timeout: 10000 });

    const mergeCalls = await page.evaluate(() => {
      const getter = (window as unknown as Record<string, unknown>).__MOCK_GET_MERGE_STATION_CALLS__;
      if (typeof getter !== 'function') return [];
      return (getter as () => unknown[])();
    });

    expect(mergeCalls.length).toBeGreaterThan(0);
  });

  /** Mainline 3 shots; the orphan re-surveys Mainline shots 2-3, then carries on (#681). */
  async function openRepeatScenario(page: Page, viewport = { width: 1024, height: 640 }) {
    await page.setViewportSize(viewport);
    await createProject(page, 'Orphan Repeat Scenario');

    await createSectionViaStore(page, {
      name: 'Anchor Mainline',
      shots: [
        { azimuth_in: 10, azimuth_out: 10, length: 10, depth_in: -1, depth_out: -1.5 },
        { azimuth_in: 20, azimuth_out: 20, length: 8, depth_in: -1.5, depth_out: -2 },
        { azimuth_in: 30, azimuth_out: 30, length: 6, depth_in: -2, depth_out: -2.5 },
      ],
    });
    // The orphan starts by re-surveying Mainline shots 2 and 3 (the second one
    // re-measured 0.5 m longer), then carries on into new passage.
    await createSectionViaStore(page, {
      name: 'Orphan Branch',
      shots: [
        { azimuth_in: 20, azimuth_out: 20, length: 8, depth_in: -1.5, depth_out: -2 },
        { azimuth_in: 30, azimuth_out: 30, length: 6.5, depth_in: -2, depth_out: -2.5 },
        { azimuth_in: 100, azimuth_out: 100, length: 5, depth_in: -2.5, depth_out: -2.6 },
      ],
    });

    await openOrphanDialogWith(page, [
      {
        orphanSectionId: 2,
        orphanSectionName: 'Orphan Branch',
        orphanStationIndex: 0,
        targetSectionId: 1,
        targetSectionName: 'Anchor Mainline',
        targetStationIndex: 1,
        // The matcher's both-sides boost can push a score past 1.
        score: 1.18,
        matchedShots: 2,
        existingMatchedShots: 2,
        preferredMergeOffset: 1,
        direction: 'forward',
        isPartial: false,
        confidence: 'high',
      },
    ]);
    const dialog = page.getByTestId('orphan-placement-dialog');
    await expect(dialog).toBeVisible({ timeout: 10000 });
    return dialog;
  }

  test('a repeated section resolves every repeated shot, one choice per shot (#681)', async ({ page }) => {
    const dialog = await openRepeatScenario(page);

    await expect(dialog.getByText('100% score')).toBeVisible();
    await expect(page.getByTestId('overlap-row-0')).toHaveAttribute('data-differs', 'false');
    await expect(page.getByTestId('overlap-row-1')).toHaveAttribute('data-differs', 'true');
    await expect(page.getByTestId('overlap-row-2')).toHaveCount(0);

    // The table fits the details column at 1024x640: no horizontal scroll.
    const overflow = await page.getByTestId('wizard-suggestion').evaluate((el) => el.scrollWidth - el.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);

    await page.getByTestId('overlap-choice-1').click();
    await page.getByTestId('overlap-choice-1-option-use_new').click();
    await expect(page.getByTestId('overlap-choice-1')).toContainText('Use new');

    await page.getByTestId('orphan-accept').click();
    await expect(dialog).toBeHidden({ timeout: 10000 });

    // Default connection: the END of the repeat (orphan stn 2 = Mainline stn 3).
    const mergeCalls = await page.evaluate(() => {
      const getter = (window as unknown as Record<string, unknown>).__MOCK_GET_MERGE_STATION_CALLS__;
      return typeof getter === 'function' ? (getter as () => unknown[])() : [];
    });
    expect(mergeCalls).toEqual([
      { stationA: { section_id: 2, station_index: 2 }, stationB: { section_id: 1, station_index: 3 } },
    ]);

    const project = await readProject(page);
    const mainline = project.sections.find((s) => s.id === 1)!;
    const orphan = project.sections.find((s) => s.id === 2)!;
    // Both repeated copies are kept as Reference; the new passage stays Normal.
    expect(orphan.shots.map((s) => s.kind)).toEqual(['Reference', 'Reference', 'Normal']);
    // Keep existing on row 0, Use new on row 1 - written into the EXISTING shot.
    expect(mainline.shots.map((s) => s.length)).toEqual([10, 8, 6.5]);
    expect(mainline.shots.map((s) => s.kind)).toEqual(['Normal', 'Normal', 'Normal']);
  });

  /**
   * No horizontal scroll in the details column, and each row's choice inside it.
   * INJECTION (tried): the choice back as the table's last column reds the
   * scale-0.5 case (overflow 67); with the fixed 380 px preview column as well,
   * the 800 px case reds too (overflow 71).
   */
  async function expectChoicesInView(page: Page) {
    const details = page.getByTestId('wizard-suggestion');
    const overflow = await details.evaluate((el) => el.scrollWidth - el.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    const column = (await details.boundingBox())!;
    for (const index of [0, 1]) {
      const select = (await page.getByTestId(`overlap-choice-${index}`).boundingBox())!;
      expect(select.x).toBeGreaterThanOrEqual(column.x);
      expect(select.x + select.width).toBeLessThanOrEqual(column.x + column.width);
    }
  }

  test('every row\'s choice is fully in view at the 800 px window minimum (#681)', async ({ page }) => {
    // The choice used to be the table's last column, behind horizontal scroll
    // in any window narrower than ~880 px. 800x888 is the narrowest window; the
    // max UI scale caps the CSS viewport at 800 px wide too (uiScale.ts).
    await openRepeatScenario(page, { width: 800, height: 888 });
    await expect(page.getByTestId('overlap-row-1')).toBeVisible();

    await expectChoicesInView(page);
  });

  test('every row\'s choice is fully in view at UI scale 0.5 (#681)', async ({ page }) => {
    // Scale 0.5 doubles the CSS viewport, so the dialog is at its 900 px cap,
    // but WebKit's minimum font size turns the 12 px table text into ~18 CSS
    // px. Chromium has no such floor, so it is emulated with a style rule.
    await openRepeatScenario(page, { width: 1600, height: 1776 });
    await page.addStyleTag({ content: '* { font-size: max(18px, 1em) !important; }' });
    await expect(page.getByTestId('overlap-row-1')).toBeVisible();

    await expectChoicesInView(page);
  });

  test('a mid-repeat merge resolves only the shot before it; the rest stays Normal (#681)', async ({ page }) => {
    const dialog = await openRepeatScenario(page);

    // Merge at orphan stn 1 = Mainline stn 2: the repeat is now one shot long.
    // INJECTION (tried): `computeOverlapPairs` pairing the whole overlap again
    // reds this at the row-count check (row 1 still listed).
    // Picked and confirmed as a unit: one run under heavy load (load average
    // ~50) ended on a different option, so the row checks below saw no rows.
    await expect(async () => {
      await page.getByTestId('merge-station-select').click();
      await page.getByTestId('merge-station-select-option-1').click({ timeout: 2000 });
      await expect(page.getByTestId('merge-station-select')).toContainText('stn 1 ', { timeout: 1000 });
    }).toPass({ timeout: 15000 });
    await expect(page.getByTestId('overlap-row-0')).toBeVisible();
    await expect(page.getByTestId('overlap-row-1')).toHaveCount(0);

    await page.getByTestId('orphan-accept').click();
    await expect(dialog).toBeHidden({ timeout: 10000 });

    const project = await readProject(page);
    const orphan = project.sections.find((s) => s.id === 2)!;
    // Shot 2 runs on from the merge station: a Reference there would cut the
    // rest of the orphan off in Plan mode.
    expect(orphan.shots.map((s) => s.kind)).toEqual(['Reference', 'Normal', 'Normal']);
  });

  test('walks through zero datum selection into new import wizard', async ({ page }) => {
    await createProject(page, 'Zero Datum Calibration');
    await openDmpSelectionDialog(page);

    const selectionDialog = page.getByTestId('dmp-selection-dialog');
    await expect(selectionDialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });

    await selectionDialog.getByTestId('continue-button').click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 10000 });

    await page.getByTestId('zero-datum-section-card-0').click();
    await expect(page.getByTestId('zero-datum-config-dialog')).toBeVisible({ timeout: 10000 });

    const gpsInput = page.getByTestId('zero-datum-gps-input');
    await gpsInput.fill('24.550123, -81.780456');
    await expect(page.getByTestId('zero-datum-gps-parsed')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('zero-datum-config-confirm').click();
    // New workflow auto-starts map placement and hides the wizard dialog until Esc.
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeHidden({ timeout: 10000 });
    await expect(page.getByTestId('placement-mode-indicator')).toBeVisible({ timeout: 15000 });
  });
});
