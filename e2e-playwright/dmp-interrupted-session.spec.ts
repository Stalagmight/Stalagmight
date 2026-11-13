import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, createSectionViaStore } from './helpers/app';

async function openDmpNewImportDialog(page: Page): Promise<void> {
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
      throw new Error('E2E UI store not available');
    }

    const dmpFile = {
      version: 5,
      sections: [
        {
          name: 'Unanchored Segment',
          direction: 'in',
          date: '2026-02-01',
          content_hash: 'hash-unanchored',
          shots: [
            {
              shot_type: 'Standard',
              heading_in: 120,
              heading_out: 120,
              length: 8,
              depth_in: -2,
              depth_out: -3,
              pitch_in: 0,
              pitch_out: 0,
              left: 1,
              right: 1,
              up: 1,
              down: 1,
              temperature: 18,
              hour: 10,
              minute: 0,
              second: 0,
              marker_index: 0,
              comment: null,
            },
          ],
        },
        {
          name: 'Anchor Candidate',
          direction: 'in',
          date: '2026-02-01',
          content_hash: 'hash-anchor',
          shots: [
            {
              shot_type: 'Standard',
              heading_in: 45,
              heading_out: 45,
              length: 10,
              depth_in: -1,
              depth_out: -2,
              pitch_in: 0,
              pitch_out: 0,
              left: 1,
              right: 1,
              up: 1,
              down: 1,
              temperature: 18,
              hour: 10,
              minute: 5,
              second: 0,
              marker_index: 0,
              comment: null,
            },
          ],
        },
      ],
    };

    const selectionConfig = {
      selectedIndices: new Set([0, 1]),
      defaultSurveyors: [{ name: 'Session Diver' }],
      defaultExplorers: [],
      defaultDate: null,
      fileHash: 'mock_dmp_hash',
      sectionHashes: ['hash-unanchored', 'hash-anchor'],
    };

    e2e.uiStore.getState().openDialog('dmp-import-new', {
      dmpFile,
      filePath: '/mock/interrupted-session.dmp',
      selectionConfig,
    });
  });
}

test.describe('DMP Interrupted Session', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Interrupted Session Project');
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore?: {
            getState: () => {
              config: {
                general: {
                  onboarding_enabled: boolean;
                  onboarding_completed_flows: string[];
                };
              } | null;
              setConfig: (config: unknown) => void;
            };
          };
        };
      }).__E2E_TEST__;
      const store = e2e?.projectStore?.getState();
      if (!store?.config) {
        throw new Error('projectStore config is not available');
      }
      store.setConfig({
        ...store.config,
        general: {
          ...store.config.general,
          onboarding_enabled: false,
          onboarding_completed_flows: [],
        },
      });
    });

    await createSectionViaStore(page, {
      name: 'Anchor Section',
      shots: [
        {
          azimuth_in: 40,
          azimuth_out: 40,
          length: 10,
          depth_in: -1,
          depth_out: -2,
        },
      ],
    });

    await page.evaluate(() => {
      const setSuggestions = (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof setSuggestions !== 'function') {
        throw new Error('Mock suggestion setter not available');
      }
      (setSuggestions as (suggestions: unknown[]) => void)([
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.94,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 1,
          importSectionName: 'Anchor Candidate',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
      ]);
    });
  });

  test.afterEach(async ({ page }) => {
    await page.evaluate(() => {
      const resetSuggestions = (window as unknown as Record<string, unknown>).__MOCK_RESET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof resetSuggestions === 'function') {
        (resetSuggestions as () => void)();
      }
    });
  });

  test('preserves cancel safety and allows clean restart after interruption', async ({ page }) => {
    await openDmpNewImportDialog(page);
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });

    await page.getByTestId('confirm-button').click();
    // Manual placing can auto-start map placement and close the dialog immediately.
    // If that happens, Esc re-opens the wizard with cancel controls.
    const manualPlacing = page.getByTestId('wizard-manual-placing');
    await page.waitForTimeout(300);
    if (!(await manualPlacing.isVisible())) {
      await page.keyboard.press('Escape');
    }
    await expect(manualPlacing).toBeVisible({ timeout: 15000 });

    await page.getByTestId('cancel-button').click();
    await expect(page.getByTestId('cancel-confirm-dialog')).toBeVisible({ timeout: 5000 });
    await page.getByTestId('confirm-cancel-button').click();
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeHidden({ timeout: 10000 });

    await openDmpNewImportDialog(page);
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });
  });

  test('ignores rapid double-skip so early import runs only once', async ({ page }) => {
    await page.evaluate(() => {
      const setSuggestions = (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof setSuggestions !== 'function') {
        throw new Error('Mock suggestion setter not available');
      }

      (setSuggestions as (suggestions: unknown[]) => void)([
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.96,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 1,
          importSectionName: 'Anchor Candidate',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.91,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 0,
          importSectionName: 'Unanchored Segment',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
      ]);
    });

    await openDmpNewImportDialog(page);
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });

    await page.getByTestId('confirm-button').click();
    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });

    await page.getByTestId('skip-button').dblclick();

    await expect.poll(async () =>
      page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            projectStore?: {
              getState: () => { project: { sections: unknown[] } | null };
            };
          };
        }).__E2E_TEST__;
        return stores?.projectStore?.getState().project?.sections.length ?? 0;
      })
    ).toBe(2);
  });
});
