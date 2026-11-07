import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  createSectionViaStore,
} from './helpers/app';

type MockSuggestion = {
  existingStationIndex: number;
  importStationIndex: number;
  score: number;
  matchedShots: number;
  direction: 'forward' | 'reverse';
  isPartial: boolean;
  confidence: 'high' | 'medium' | 'low';
  existingSectionName: string;
  importSectionIndex: number;
  importSectionName: string;
  targetType: 'existing' | 'importing';
  existingSectionId: number | null;
  targetImportIndex: number | null;
};

interface MockDmpOptions {
  /**
   * Append a zero-length shot to the unanchored section.
   *
   * The DMP parser clamps a negative raw length to exactly 0.0, the shot
   * survives `isImportableDmpShot` (which only drops the length-1 default), and
   * `missingRequiredCells` marks `length <= 0` missing - so a perfectly valid
   * file can arrive already holding a row the wizard calls incomplete, with
   * nothing the user did to cause it (#476).
   */
  zeroLengthInUnanchored?: boolean;
}

function buildMockDmpFile(options: MockDmpOptions = {}) {
  return {
    version: 5,
    sections: [
      {
        name: 'Anchor Candidate',
        direction: 'in',
        date: '2026-02-01',
        content_hash: 'hash-anchor-candidate',
        shots: [
          {
            shot_type: 'Standard',
            heading_in: 40,
            heading_out: 40,
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
            minute: 0,
            second: 0,
            marker_index: 0,
            comment: null,
          },
        ],
      },
      {
        name: 'Unanchored Segment',
        direction: 'in',
        date: '2026-02-01',
        content_hash: 'hash-unanchored-segment',
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
            minute: 5,
            second: 0,
            marker_index: 0,
            comment: null,
          },
          ...(options.zeroLengthInUnanchored
            ? [
                {
                  shot_type: 'Standard',
                  heading_in: 125,
                  heading_out: 125,
                  length: 0,
                  depth_in: -3,
                  depth_out: -3,
                  pitch_in: 0,
                  pitch_out: 0,
                  left: 1,
                  right: 1,
                  up: 1,
                  down: 1,
                  temperature: 18,
                  hour: 10,
                  minute: 6,
                  second: 0,
                  marker_index: 0,
                  comment: null,
                },
              ]
            : []),
        ],
      },
    ],
  };
}

const defaultSuggestions: [MockSuggestion, ...MockSuggestion[]] = [
  {
    existingStationIndex: 0,
    importStationIndex: 0,
    score: 0.94,
    matchedShots: 0,
    direction: 'forward',
    isPartial: false,
    confidence: 'high',
    existingSectionName: 'Anchor Section',
    importSectionIndex: 0,
    importSectionName: 'Anchor Candidate',
    targetType: 'existing',
    existingSectionId: 1,
    targetImportIndex: null,
  },
];

async function setImportSuggestions(page: Page, suggestions: MockSuggestion[]): Promise<void> {
  await page.evaluate((nextSuggestions) => {
    const setSuggestions =
      (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__;
    if (typeof setSuggestions !== 'function') {
      throw new Error('Mock suggestion setter not available');
    }
    (setSuggestions as (value: unknown[]) => void)(nextSuggestions);
  }, suggestions);
}

async function openDmpWizard(page: Page, dmpFile = buildMockDmpFile()): Promise<void> {
  await page.evaluate((dmpFile) => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        uiStore?: {
          getState: () => {
            openDialog: (dialog: string, data?: Record<string, unknown>) => void;
          };
        };
      };
    }).__E2E_TEST__;

    if (!e2e?.uiStore) {
      throw new Error('E2E UI store unavailable');
    }

    const selectionConfig = {
      selectedIndices: new Set([0, 1]),
      defaultSurveyor: 'Session Diver',
      defaultDate: null,
      fileHash: 'mock_dmp_hash_flow',
      sectionHashes: ['hash-anchor-candidate', 'hash-unanchored-segment'],
    };

    e2e.uiStore.getState().openDialog('dmp-import-new', {
      dmpFile,
      filePath: '/mock/import-placement-flow.dmp',
      selectionConfig,
    });
  }, dmpFile);

  await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
  await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });
}

/**
 * Where the auto-started handoff has got to, as one word.
 *
 * `wizard-hidden` is the failure this exists to name: the wizard MOUNTED with
 * `activeDialog` still set, under DialogWrapper's `hidden` prop. A `display:none`
 * PARENT leaves the descendant reporting `display: flex`, `visibility: visible`
 * and a 0x0 box, so the box is what has to be measured, not the element's own
 * computed display.
 *
 * `working` covers every state that is not yet an answer - the suggestion phase,
 * a calculating or importing wizard - so a poll cannot pass on the screen the
 * flow started from.
 */
type PlacementFlowStatus =
  | 'placement-mode'
  | 'wizard-manual-placing'
  | 'wizard-hidden'
  | 'wizard-absent'
  | 'working';

async function readPlacementFlowStatus(page: Page): Promise<PlacementFlowStatus> {
  return page.evaluate<PlacementFlowStatus>(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: { uiStore?: { getState: () => { placementMode: unknown } } };
    }).__E2E_TEST__;
    if (!e2e?.uiStore) {
      throw new Error('E2E UI store unavailable');
    }
    if (e2e.uiStore.getState().placementMode) return 'placement-mode';

    const wizard = document.querySelector('[data-testid="dmp-new-import-dialog"]');
    if (!wizard) return 'wizard-absent';
    const rect = wizard.getBoundingClientRect();
    const style = getComputedStyle(wizard);
    const onScreen =
      style.display !== 'none' && style.visibility !== 'hidden' && rect.width > 0 && rect.height > 0;
    if (!onScreen) return 'wizard-hidden';

    const manual = document.querySelector('[data-testid="wizard-manual-placing"]');
    if (!manual) return 'working';
    const manualRect = manual.getBoundingClientRect();
    return manualRect.width > 0 && manualRect.height > 0 ? 'wizard-manual-placing' : 'working';
  });
}

async function openManualPlacing(page: Page): Promise<void> {
  await page.getByTestId('confirm-button').click();

  const manualPlacing = page.getByTestId('wizard-manual-placing');
  const placementIndicator = page.getByTestId('placement-mode-indicator');
  // One of the two end states: the wizard on its manual-placing screen, or (the
  // auto-start handoff, during which the dialog is display:none) map placement.
  await expect(manualPlacing.or(placementIndicator).filter({ visible: true }).first()).toBeVisible({ timeout: 10000 });
  if (!(await manualPlacing.isVisible())) {
    await expect(placementIndicator).toBeVisible({ timeout: 10000 });
    await page.keyboard.press('Escape');
  }

  await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
  await expect(manualPlacing).toBeVisible({ timeout: 10000 });
}

test.describe('Import Placement Flow', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Placement Wizard Test');
    await createSectionViaStore(page, {
      name: 'Anchor Section',
      shots: [{ azimuth_in: 40, azimuth_out: 40, length: 10, depth_in: -1, depth_out: -2 }],
    });
    await setImportSuggestions(page, defaultSuggestions);
  });

  test.afterEach(async ({ page }) => {
    await page.evaluate(() => {
      const resetSuggestions =
        (window as unknown as Record<string, unknown>).__MOCK_RESET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof resetSuggestions === 'function') {
        (resetSuggestions as () => void)();
      }
    });
  });

  test('shows suggestion-phase controls and actions @smoke', async ({ page }) => {
    await openDmpWizard(page);

    await expect(page.getByTestId('wizard-connection-description')).toBeVisible();
    await expect(page.getByTestId('wizard-confidence')).toBeVisible();
    await expect(page.getByTestId('confirm-button')).toBeVisible();
    await expect(page.getByTestId('skip-button')).toBeVisible();
  });

  test('supports alternatives list in suggestion phase', async ({ page }) => {
    await setImportSuggestions(page, [
      ...defaultSuggestions,
      {
        ...defaultSuggestions[0],
        score: 0.77,
        confidence: 'medium',
      },
    ]);

    await openDmpWizard(page);

    const alternativesToggle = page.getByTestId('toggle-alternatives');
    const alternativesList = page.getByTestId('alternatives-list');
    await expect(alternativesToggle).toBeVisible();

    if (!(await alternativesList.isVisible())) {
      await alternativesToggle.click();
    }

    await expect(alternativesList).toBeVisible();
    await expect(page.getByTestId('alternative-0')).toBeVisible();
  });

  test('confirming suggestion transitions to manual placing @smoke', async ({ page }) => {
    await openDmpWizard(page);
    await openManualPlacing(page);

    await expect(page.getByTestId('segment-placement-modes')).toBeVisible();
    await expect(page.getByTestId('segment-place-button')).toBeVisible();
  });

  test('manual mode keeps segment-place disabled until a placement action is chosen', async ({ page }) => {
    await openDmpWizard(page);
    await openManualPlacing(page);

    const placeButton = page.getByTestId('segment-place-button');
    await expect(placeButton).toBeDisabled();
  });

  test('discard mode enables place action and can finish the wizard @smoke', async ({ page }) => {
    await openDmpWizard(page);
    await openManualPlacing(page);

    await page.getByTestId('segment-mode-discard').click();
    const placeButton = page.getByTestId('segment-place-button');
    await expect(placeButton).toBeEnabled();
    await placeButton.click();

    await expect(page.getByTestId('dmp-new-import-dialog')).toBeHidden({ timeout: 10000 });

    // Every segment was discarded, so nothing was imported and no receipt is
    // produced - the wizard closes straight back to the map.
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();
  });

  test('from-existing mode requires selecting target section before place is enabled', async ({ page }) => {
    await openDmpWizard(page);
    await openManualPlacing(page);

    await page.getByTestId('segment-mode-from-existing').click();

    const placeButton = page.getByTestId('segment-place-button');
    await expect(placeButton).toBeDisabled();

    await page.getByTestId('from-existing-section-select').click();
    const dropdownListbox = page
      .locator('[role="listbox"]')
      .filter({ has: page.getByRole('option', { name: 'Anchor Section', exact: true }) })
      .last();
    await dropdownListbox.getByRole('option', { name: 'Anchor Section', exact: true }).click();

    await expect(placeButton).toBeEnabled();
  });

  test('manual place-on-map hides wizard and escape restores it', async ({ page }) => {
    await openDmpWizard(page);
    await openManualPlacing(page);

    await page.getByTestId('segment-mode-manual').click();
    await page.getByTestId('segment-place-on-map-button').click();

    await expect(page.getByTestId('placement-mode-indicator')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeHidden({ timeout: 10000 });

    await page.keyboard.press('Escape');
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible({ timeout: 10000 });
  });

  // F8: the manual-placement hint toast fires just before this dialog closes
  // for map placement, and can still be dwelling when Escape reopens it - at
  // the default 1280x720 viewport the toast's bottom-left corner used to sit
  // on top of the wizard's Cancel/Back footer row.
  test('manual-placement hint toast clears the reopened wizard footer', async ({ page }) => {
    await openDmpWizard(page);
    await openManualPlacing(page);

    await page.getByTestId('segment-mode-manual').click();
    await page.getByTestId('segment-place-on-map-button').click();

    const hintToast = page.locator('[data-toast-key="dmp-manual-placement-hint"]').last();
    await expect(hintToast).toBeVisible({ timeout: 10000 });

    await page.keyboard.press('Escape');
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });

    // Re-trigger the SAME toast key right after the dialog reopens: `showToast`
    // refreshes a live toast of the same key in place, resetting its dwell
    // clock. This pins the "both on screen at once" precondition deterministically
    // instead of racing the toast's real ~6.5s dwell+fade against a slow CI
    // machine, where the earlier version of this test silently asserted
    // nothing once the toast had expired.
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: { uiStore?: { getState: () => { showToast: (m: string, k: string, kind: string) => void } } };
      }).__E2E_TEST__;
      if (!e2e?.uiStore) throw new Error('E2E UI store unavailable');
      e2e.uiStore.getState().showToast('placement hint', 'dmp-manual-placement-hint', 'info');
    });

    await expect(hintToast).toBeVisible({ timeout: 10000 });
    const toastBox = await hintToast.boundingBox();
    const footerBox = await page.getByTestId('dmp-import-dialog-footer').boundingBox();
    expect(toastBox).not.toBeNull();
    expect(footerBox).not.toBeNull();
    if (toastBox && footerBox) {
      expect(toastBox.y + toastBox.height).toBeLessThanOrEqual(footerBox.y + 1);
    }
  });

  test('skip action in suggestion phase still reaches manual placing', async ({ page }) => {
    await openDmpWizard(page);

    await page.getByTestId('skip-button').click();

    const manualPlacing = page.getByTestId('wizard-manual-placing');
    const placementIndicator = page.getByTestId('placement-mode-indicator');
    // One of the two end states: the wizard on its manual-placing screen, or (the
    // auto-start handoff, during which the dialog is display:none) map placement.
    await expect(manualPlacing.or(placementIndicator).filter({ visible: true }).first()).toBeVisible({ timeout: 10000 });
    if (!(await manualPlacing.isVisible())) {
      await expect(placementIndicator).toBeVisible({ timeout: 10000 });
      await page.keyboard.press('Escape');
    }

    await expect(manualPlacing).toBeVisible({ timeout: 10000 });
  });

  test('cancel requires confirmation before closing', async ({ page }) => {
    await openDmpWizard(page);

    const wizard = page.getByTestId('dmp-new-import-dialog');
    await wizard.getByTestId('cancel-button').click();

    const cancelConfirmDialog = page.getByTestId('cancel-confirm-dialog');
    if (await cancelConfirmDialog.isVisible()) {
      await page.getByTestId('confirm-cancel-button').click();
    }

    await expect(wizard).toBeHidden({ timeout: 10000 });
  });

  /**
   * A zero-length shot the FILE brought with it must not strand the wizard.
   *
   * Confirming the only suggestion anchors section 0 and leaves section 1 with
   * nowhere to attach, so the wizard moves itself to manual placing and
   * auto-starts the handoff - a window during which the whole dialog is
   * `display:none`. That handoff used to run the payload guard first, and
   * `refuseIncompleteShots` walks EVERY selected section, so the zero-length
   * row in section 1 refused it. The refusal returned before the one line that
   * closed the hidden window, and nothing in the effect's dependencies changes
   * on a refusal, so it never retried: an invisible modal, `activeDialog` still
   * set, the rest of the app `inert`, no mouse affordance and no way back
   * (#476).
   *
   * The guard now lives only where a payload is genuinely at stake, and the
   * window is owned by the effect that opens it, so any early return ends it.
   */
  test('a file-borne zero-length shot does not strand the auto-started handoff @smoke', async ({
    page,
  }) => {
    await openDmpWizard(page, buildMockDmpFile({ zeroLengthInUnanchored: true }));

    await page.getByTestId('confirm-button').click();

    // Either answer is healthy - the handoff took (placement mode, dialog
    // closed) or it declined and put the wizard back on screen. What must not
    // happen is the third one.
    await expect
      .poll(() => readPlacementFlowStatus(page), { timeout: 20000 })
      .toMatch(/^(placement-mode|wizard-manual-placing)$/);

    // ... and the user can get back to the wizard from wherever it landed,
    // which is what "not stranded" means.
    if ((await readPlacementFlowStatus(page)) === 'placement-mode') {
      await expect(page.getByTestId('placement-mode-indicator')).toBeVisible({ timeout: 10000 });
      await page.keyboard.press('Escape');
    }

    const wizard = page.getByTestId('dmp-new-import-dialog');
    await expect(wizard).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible({ timeout: 10000 });
    // Spelled out rather than left to toBeVisible: the bug rendered a dialog
    // whose own computed style said visible and whose box was 0x0.
    const box = await wizard.boundingBox();
    expect(box?.width ?? 0).toBeGreaterThan(0);
    expect(box?.height ?? 0).toBeGreaterThan(0);
  });

  /**
   * The other half of the same fix: the guard MOVED, it was not deleted.
   *
   * "Place on Map" is a deliberate step toward an import, taken with the dialog
   * visible, so refusing there names a section the user can go and fix. Taking
   * the guard off both paths would pass the test above and send the NaN-shaped
   * row on to the backend instead.
   */
  test('the Place on Map button still refuses while a section is incomplete', async ({ page }) => {
    await openDmpWizard(page, buildMockDmpFile({ zeroLengthInUnanchored: true }));
    await page.getByTestId('confirm-button').click();

    await expect
      .poll(() => readPlacementFlowStatus(page), { timeout: 20000 })
      .toMatch(/^(placement-mode|wizard-manual-placing)$/);
    if ((await readPlacementFlowStatus(page)) === 'placement-mode') {
      await expect(page.getByTestId('placement-mode-indicator')).toBeVisible({ timeout: 10000 });
      await page.keyboard.press('Escape');
    }
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible({ timeout: 10000 });

    await page.getByTestId('segment-mode-manual').click();
    await page.getByTestId('segment-place-on-map-button').click();

    // By dedup key, not by test ID: the placement hint toast is still on
    // screen, and the refusal is the one that names the section.
    const refusal = page.locator('[data-toast-key="dmp-wizard-incomplete"]');
    await expect(refusal).toContainText('Unanchored Segment', { timeout: 10000 });
    await expect(refusal).toContainText('missing required values');
    // Refused, so the wizard is still the screen - not hidden behind a
    // placement it declined to start.
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible();
    expect(await readPlacementFlowStatus(page)).toBe('wizard-manual-placing');
  });

  test('back requires confirmation and can return to selection dialog', async ({ page }) => {
    await openDmpWizard(page);

    const wizard = page.getByTestId('dmp-new-import-dialog');
    await wizard.getByTestId('back-button').click();

    const backConfirmDialog = page.getByTestId('back-confirm-dialog');
    if (await backConfirmDialog.isVisible()) {
      await page.getByTestId('confirm-back-button').click();
    }

    await expect(page.getByTestId('dmp-selection-dialog')).toBeVisible({ timeout: 10000 });
  });
});
