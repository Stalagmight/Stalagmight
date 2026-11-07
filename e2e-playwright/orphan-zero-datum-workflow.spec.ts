import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  createSectionViaStore,
  openDmpSelectionDialog,
  proceedToDmpSectionConfig,
} from './helpers/app';

async function openOrphanDialog(page: Page): Promise<void> {
  await page.evaluate(() => {
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

    e2e.uiStore.getState().openDialog('orphan-placement', {
      suggestions: [
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
      ],
    });
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

  /**
   * #576 NF-AN4-N4: after a DMP import, the orphan-placement lookup failing was
   * swallowed ("non-fatal"), so the step that offers to attach unanchored
   * sections was skipped without a word. The import still completes - it has
   * landed - but the user is told the suggestions could not be computed, and why.
   */
  test('says so when the orphan lookup after a DMP import fails', async ({ page }) => {
    await createProject(page, 'Orphan Lookup Failure');
    await createSectionViaStore(page, {
      name: 'Anchor Section',
      shots: [{ azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: -1, depth_out: -2 }],
    });
    await page.evaluate(() => {
      const w = window as unknown as Record<string, unknown>;
      (w.__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__ as (s: unknown[]) => void)([
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.95,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 0,
          importSectionName: 'Main Passage',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
      ]);
      w.__MOCK_IPC_OVERRIDES__ = {
        ...((w.__MOCK_IPC_OVERRIDES__ as Record<string, unknown> | undefined) ?? {}),
        suggest_orphan_placements: { __throw: 'orphan matcher unavailable (test)' },
      };
    });

    await openDmpSelectionDialog(page);
    const selection = page.getByTestId('dmp-selection-dialog');
    await expect(selection.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });
    await selection.getByTestId('select-none-button').click();
    await selection.getByTestId('section-checkbox-0').click();
    await proceedToDmpSectionConfig(page);
    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });
    await page.getByTestId('confirm-button').click();

    await expect(page.getByTestId('dmp-new-import-dialog')).toBeHidden({ timeout: 15000 });
    const notice = page.locator('[data-toast-key="dmp-orphan-lookup-failed"]');
    await expect(notice).toBeVisible({ timeout: 10000 });
    await expect(notice).toContainText('orphan matcher unavailable (test)');
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
