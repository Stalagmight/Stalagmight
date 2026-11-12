import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  openSettingsFromMenu,
  selectDropdownOption,
} from './helpers/app';
import type { Locator } from '@playwright/test';

/** Add a shot the way the app does everywhere: type into the trailing blank
 *  row, which materializes it. There is no Add button. */
async function addShotByTypingInBlankRow(dialog: Locator): Promise<void> {
  const lengthCells = dialog.locator(
    '[data-testid^="shot-editor-cell-"][data-testid$="-length"]'
  );
  const blankIndex = (await lengthCells.count()) - 1;
  // Fill every required field: a row with only a length is incomplete, and Save
  // refuses while any row is - the same gate the new-section dialog puts in
  // front of Create.
  for (const [col, value] of [
    ['length', '5'],
    ['_azimuth_avg', '90'],
    ['depth_in', '0'],
    ['depth_out', '-1'],
  ] as const) {
    await dialog.getByTestId(`shot-editor-cell-${blankIndex}-${col}`).dblclick();
    const editor = dialog.getByTestId('shot-editor-cell-editor');
    await editor.fill(value);
    await editor.press('Enter');
  }
}

type StationRef = { section_id: number; station_index: number };

type ProjectDialogIds = {
  sectionId: number;
  shotId: number;
  midShotId: number;
  stationRef: StationRef;
};

function buildShot(index: number) {
  const base = index + 1;
  return {
    azimuth_in: 10 * base,
    azimuth_out: 180 + 10 * base,
    length: 5 + base,
    depth_in: -1 * base,
    depth_out: -1.5 * base,
    left: 0.5 * base,
    right: 0.7 * base,
    up: 0.9 * base,
    down: 0.4 * base,
  };
}

async function setupProjectWithShots(page: Page, shotCount: number): Promise<ProjectDialogIds> {
  await setupMockedPage(page);
  await createProject(page, 'Dialog Coverage');

  const shots = Array.from({ length: shotCount }, (_, idx) => buildShot(idx));
  await seedProjectWithSections(page, [
    {
      name: 'Dialog Section',
      direction: 'in',
      shots,
    },
  ]);

  await waitForMapReady(page);
  await waitForNoDialogs(page);

  const ids = await page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: any; stations: any[] } };
      };
    }).__E2E_TEST__;

    if (!e2e) return null;

    const state = e2e.projectStore.getState();
    const section = state.project?.sections?.[0];
    const shots = section?.shots ?? [];
    const station = state.stations.find(
      (s) => s.station_ref.section_id === section?.id
    );

    return {
      sectionId: section?.id ?? null,
      shotId: shots[0]?.id ?? null,
      midShotId: shots[Math.floor(shots.length / 2)]?.id ?? null,
      stationRef: station?.station_ref ?? null,
    };
  });

  if (!ids || !ids.sectionId || !ids.shotId || !ids.midShotId || !ids.stationRef) {
    throw new Error('Failed to resolve project IDs for dialog tests.');
  }

  return ids as ProjectDialogIds;
}

async function openDialog(page: Page, name: string, data: Record<string, unknown>) {
  await page.evaluate(
    ({ dialogName, dialogData }) => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog(dialogName, dialogData);
    },
    { dialogName: name, dialogData: data }
  );
}

test.describe('Dialog Coverage - Edit Shot', () => {
  test('validates input and saves normalized values', async ({ page }) => {
    const { sectionId, shotId } = await setupProjectWithShots(page, 2);

    await openDialog(page, 'edit-shot', { sectionId, shotId });

    const dialog = page.getByTestId('edit-shot-dialog');
    await expect(dialog).toBeVisible();

    const lengthInput = dialog.locator('label:has-text("Length")').locator('..').locator('input');
    await lengthInput.fill('0');
    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(dialog.locator('text=Length must be greater than 0')).toBeVisible();

    await lengthInput.fill('12.5');
    const azInInput = dialog.locator('label:has-text("Azimuth In")').locator('..').locator('input');
    const azOutInput = dialog.locator('label:has-text("Azimuth Out")').locator('..').locator('input');
    await azInInput.fill('-10');
    await azOutInput.fill('-20');

    const pitchInInput = dialog.locator('label:has-text("Pitch In")').locator('..').locator('input');
    await pitchInInput.fill('5');

    const leftInput = dialog.locator('input[placeholder="L"]');
    await leftInput.fill('1.2');

    const commentInput = dialog.locator('textarea');
    await commentInput.fill('notes');

    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const updatedShot = await page.evaluate(({ sectionId: sId, shotId: shId }) => {
      const state = (window as any).__E2E_TEST__.projectStore.getState();
      const section = state.project.sections.find((s: any) => s.id === sId);
      return section?.shots.find((s: any) => s.id === shId);
    }, { sectionId, shotId });

    expect(updatedShot.azimuth_in).toBe(350);
    expect(updatedShot.azimuth_out).toBe(340);
    expect(updatedShot.length).toBe(12.5);
    expect(updatedShot.pitch_in).toBe(5);
    expect(updatedShot.left).toBe(1.2);
    expect(updatedShot.comment).toBe('notes');
  });

  test('keeps the dialog body scrollable in a short viewport', async ({ page }) => {
    await page.setViewportSize({ width: 800, height: 420 });
    const { sectionId, shotId } = await setupProjectWithShots(page, 2);
    await openDialog(page, 'edit-shot', { sectionId, shotId });

    const body = page.getByTestId('edit-shot-dialog-body');
    await expect(body).toBeVisible();
    const metrics = await body.evaluate((element) => ({
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
      overflowY: getComputedStyle(element).overflowY,
    }));
    expect(metrics.overflowY).toBe('auto');
    expect(metrics.scrollHeight).toBeGreaterThan(metrics.clientHeight);

    await body.evaluate((element) => element.scrollTo({ top: element.scrollHeight }));
    await expect.poll(() => body.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
    await expect(page.getByTestId('edit-shot-dialog').getByRole('button', { name: 'Save' })).toBeVisible();
  });
});

test.describe('Dialog Coverage - Draw Shot', () => {
  test('auto-fills fields and creates a shot', async ({ page }) => {
    const { sectionId, stationRef } = await setupProjectWithShots(page, 2);

    await openDialog(page, 'draw-shot', { fromStation: stationRef });

    const dialog = page.getByTestId('draw-shot-dialog');
    await expect(dialog).toBeVisible();

    await dialog.getByRole('button', { name: 'Create Shot' }).click();
    await expect(dialog.locator('text=Length must be greater than 0')).toBeVisible();

    const lengthInput = dialog.locator('label:has-text("Length")').locator('..').locator('input');
    await lengthInput.fill('5');

    const azInInput = dialog.locator('label:has-text("Azimuth In")').locator('..').locator('input');
    const azOutInput = dialog.locator('label:has-text("Azimuth Out")').locator('..').locator('input');
    await azInInput.fill('45');
    await expect(azOutInput).toHaveValue('45');

    const depthInInput = dialog.locator('label:has-text("Depth In")').locator('..').locator('input');
    const depthOutInput = dialog.locator('label:has-text("Depth Out")').locator('..').locator('input');
    await depthInInput.fill('3.2');
    await expect(depthOutInput).toHaveValue('3.2');

    await dialog.locator('summary:has-text("LRUD")').click();
    const leftInput = dialog.locator('label:has-text("Left")').locator('..').locator('input');
    await leftInput.fill('0.8');

    // #27 replaced the boolean "reference" checkbox with the 3-way shot kind, so
    // marking this shot a reference is now a select, not a toggle.
    await selectDropdownOption(page, 'draw-shot-kind', 'Reference');

    await dialog.getByRole('button', { name: 'Create Shot' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const selection = await page.evaluate(() => {
      return (window as any).__E2E_TEST__.uiStore.getState().selection;
    });

    expect(selection?.type).toBe('shot');
    expect(selection?.sectionId).toBe(sectionId);
  });
});

test.describe('Dialog Coverage - Edit Section', () => {
  test('shows themed tooltip for shot discrepancy warnings', async ({ page }) => {
    const { sectionId } = await setupProjectWithShots(page, 2);

    await openDialog(page, 'edit-section', { sectionId });

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible();

    // The data table's indicator: a severity-tinted icon carrying its messages
    // in the tooltip, rather than the old table's themed "!" badge.
    const warningIcon = dialog.getByTestId('shot-warning-0');
    await expect(warningIcon).toBeVisible();
    await expect(warningIcon).toHaveAttribute(
      'title',
      /Azimuth in\/out differs by more than/
    );
    await expect(warningIcon.locator('svg')).toBeVisible();
  });

  test('updates metadata, edits shots, adds and deletes rows', async ({ page }) => {
    const { sectionId } = await setupProjectWithShots(page, 2);

    await openDialog(page, 'edit-section', { sectionId });

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible();

    const nameInput = dialog.getByTestId('section-name-input');
    const surveyorInput = dialog.getByTestId('section-surveyor-input');
    const colorHexInput = dialog.getByTestId('section-color-hex-input');
    const dateInput = dialog.getByTestId('section-date-input');

    await nameInput.fill('Updated Section');
    await surveyorInput.fill('New Surveyor');
    await colorHexInput.fill('#123456');
    await dateInput.fill('2026-02-01');
    await page.keyboard.press('Escape');

    // The shot table is a SpreadsheetGrid: one editor overlays the active cell.
    await dialog.getByTestId('shot-editor-cell-0-length').dblclick();
    const lengthEditor = dialog.getByTestId('shot-editor-cell-editor');
    await lengthEditor.fill('9.5');
    await lengthEditor.press('Enter');

    // A shot is added by typing into the trailing blank row - the app-wide
    // convention; there is no Add button.
    await addShotByTypingInBlankRow(dialog);

    // Delete goes through the grid's row context menu, like the data table's -
    // the per-row X column is gone.
    await dialog.getByTestId('shot-editor-cell-1-length').click({ button: 'right' });
    await dialog.getByTestId('shot-editor-context-delete-rows').click();

    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const updatedSection = await page.evaluate((sId) => {
      const state = (window as any).__E2E_TEST__.projectStore.getState();
      return state.project.sections.find((s: any) => s.id === sId);
    }, sectionId);

    expect(updatedSection.name).toBe('Updated Section');
    // The section's surveyor is per-shot now: the legacy field sets every shot.
    expect(updatedSection.surveyor).toBeUndefined();
    expect(
      updatedSection.shots.every(
        (shot: any) => shot.credit?.surveyors?.map((p: any) => p.name).join(';') === 'New Surveyor',
      ),
    ).toBe(true);
    // Closure-adjustment weight is no longer exposed in this dialog; the stored
    // value must survive a save untouched.
    expect(updatedSection.quality).toBe(1);
    expect(updatedSection.color).toBe('#123456');
    expect(updatedSection.shots.every((shot: any) => shot.credit?.date === '2026-02-01')).toBe(true);
    expect(updatedSection.shots[0]?.length).toBe(9.5);
  });

  test('shows mid-shot delete warning and applies shift delete', async ({ page }) => {
    const { sectionId } = await setupProjectWithShots(page, 3);

    await openDialog(page, 'edit-section', { sectionId });

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible();

    // Delete goes through the grid's row context menu, like the data table's -
    // the per-row X column is gone.
    await dialog.getByTestId('shot-editor-cell-1-length').click({ button: 'right' });
    await dialog.getByTestId('shot-editor-context-delete-rows').click();

    const warning = dialog.locator('text=This is a mid-section shot.');
    await expect(warning).toBeVisible();

    const saveButton = dialog.getByRole('button', { name: 'Save' });
    await expect(saveButton).toBeDisabled();

    const shiftButton = dialog.getByRole('button', { name: 'Shift Geometry' });
    await shiftButton.click();

    await expect(warning).not.toBeVisible({ timeout: 3000 });
    await expect(saveButton).toBeEnabled();

    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });
});

test.describe('Dialog Coverage - Confirm Delete', () => {
  test('renders station warning and disables delete', async ({ page }) => {
    await setupMockedPage(page);

    await openDialog(page, 'confirm-delete', { type: 'station' });

    const dialog = page.getByTestId('confirm-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('text=delete all connected shots')).toBeVisible();

    const deleteButton = dialog.getByRole('button', { name: 'Delete' });
    await expect(deleteButton).toBeDisabled();
  });

  test('deletes a shot and clears selection', async ({ page }) => {
    const { sectionId, shotId } = await setupProjectWithShots(page, 2);

    await page.evaluate(({ sectionId: sId, shotId: shId }) => {
      (window as any).__E2E_TEST__.uiStore.getState().setSelection({
        type: 'shot',
        sectionId: sId,
        shotId: shId,
      });
    }, { sectionId, shotId });

    await openDialog(page, 'confirm-delete', {
      type: 'shot',
      sectionId,
      shotId,
      totalShots: 2,
    });

    const dialog = page.getByTestId('confirm-dialog');
    await expect(dialog).toBeVisible();

    await dialog.getByRole('button', { name: 'Delete' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const selection = await page.evaluate(() => {
      return (window as any).__E2E_TEST__.uiStore.getState().selection;
    });
    expect(selection).toBeNull();
  });

  test('shows split name inputs when split is required', async ({ page }) => {
    const { sectionId, shotId } = await setupProjectWithShots(page, 2);

    await openDialog(page, 'confirm-delete', {
      type: 'shot',
      sectionId,
      shotId,
      totalShots: 3,
      splitRequired: true,
      sectionName: 'SplitTarget',
    });

    const dialog = page.getByTestId('confirm-dialog');
    await expect(dialog).toBeVisible();

    const splitA = dialog.getByTestId('split-name-a');
    const splitB = dialog.getByTestId('split-name-b');

    await expect(splitA).toHaveValue('SplitTarget-A');
    await expect(splitB).toHaveValue('SplitTarget-B');
  });
});

test.describe('Dialog Coverage - Settings Advanced', () => {
  test('marks dirty state and validates advanced settings', async ({ page }) => {
    await setupMockedPage(page);
    await openSettingsFromMenu(page);

    const advancedTab = page.getByTestId('settings-tab-advanced');
    await advancedTab.click();

    const panel = page.getByTestId('settings-panel-advanced');
    await expect(panel).toBeVisible();

    const toggleHitboxes = panel.getByTestId('toggle-show-hitboxes');
    await toggleHitboxes.click();
    await expect(toggleHitboxes).toHaveAttribute('aria-checked', 'true');

    const logSizeInput = panel.getByTestId('input-max-log-size');
    await logSizeInput.fill('0');

    const saveButton = page.getByTestId('settings-save-button');
    await expect(saveButton).toBeDisabled();

    await logSizeInput.fill('500');
    await expect(saveButton).toBeEnabled();

    await saveButton.click();
    await expect(page.getByTestId('settings-dialog')).not.toBeVisible({ timeout: 5000 });
  });
});

test.describe('Dialog Coverage - Settings Map', () => {
  test('validates map settings and enables save after fixes', async ({ page }) => {
    await setupMockedPage(page);
    await openSettingsFromMenu(page);

    const mapTab = page.getByTestId('settings-tab-map');
    await mapTab.click();

    const panel = page.getByTestId('settings-panel-map');
    await expect(panel).toBeVisible();

    const zoomMinInput = panel.getByTestId('input-zoom-min');
    const zoomMaxInput = panel.getByTestId('input-zoom-max');

    await zoomMinInput.fill('15');
    await zoomMaxInput.fill('10');

    const saveButton = page.getByTestId('settings-save-button');
    await expect(saveButton).toBeDisabled();

    await zoomMinInput.fill('5');
    await zoomMaxInput.fill('15');

    const snapToggle = panel.getByTestId('toggle-snap-merge');
    await snapToggle.click();

    await expect(saveButton).toBeEnabled();
  });
});

test.describe('Dialog Coverage - Section Adjust', () => {
  test('shows missing section state for invalid section id', async ({ page }) => {
    await setupMockedPage(page);

    await openDialog(page, 'section-adjust', { sectionId: 9999 });

    const dialog = page.getByTestId('section-adjust-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('text=Section not found')).toBeVisible();
  });

  test('applies preview offset and cancels to revert', async ({ page }) => {
    const { sectionId } = await setupProjectWithShots(page, 2);

    await openDialog(page, 'section-adjust', { sectionId });

    const dialog = page.getByTestId('section-adjust-dialog');
    await expect(dialog).toBeVisible();

    const dxInput = dialog.getByTestId('dx-input');
    await dxInput.fill('2.5');

    await expect(dialog.locator('text=Preview applied')).toBeVisible({ timeout: 3000 });

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });
});

test.describe('Dialog Coverage - Edit Station', () => {
  test('validates zero datum coordinates before saving', async ({ page }) => {
    const { stationRef } = await setupProjectWithShots(page, 2);

    await openDialog(page, 'edit-station', { stationRef });

    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();

    const zeroDatumToggle = dialog.locator('#zeroDatum');
    await zeroDatumToggle.check();

    const gpsInput = dialog.getByTestId('station-gps-input');
    const gpsError = dialog.getByTestId('station-gps-error');

    await gpsInput.fill('200, -80');
    await expect(gpsError).toContainText('Latitude must be between -90 and 90');

    await gpsInput.fill('25, -200');
    await expect(gpsError).toContainText('Longitude must be between -180 and 180');

    await gpsInput.fill('25, -80');
    await expect(gpsError).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });
});

test.describe('Dialog Coverage - Delete Shot Choice', () => {
  test('renders impact details and shifts geometry', async ({ page }) => {
    const { sectionId, shotId } = await setupProjectWithShots(page, 3);

    await page.evaluate(({ sectionId: sId, shotId: shId }) => {
      (window as any).__E2E_TEST__.uiStore.getState().setSelection({
        type: 'shot',
        sectionId: sId,
        shotId: shId,
      });
    }, { sectionId, shotId });

    await openDialog(page, 'delete-shot-choice', {
      sectionId,
      shotId,
      sectionName: 'Impact Section',
    });

    const dialog = page.getByTestId('delete-shot-choice-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('text=Removed merges')).toBeVisible();
    await expect(dialog.locator('text=Removed closures')).toBeVisible();

    const shiftButton = dialog.getByRole('button', { name: 'Shift Geometry' });
    await expect(shiftButton).toBeEnabled();
    await shiftButton.click();

    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const selection = await page.evaluate(() => {
      return (window as any).__E2E_TEST__.uiStore.getState().selection;
    });
    expect(selection).toBeNull();
  });

  test('opens split section dialog from split action', async ({ page }) => {
    const { sectionId, shotId } = await setupProjectWithShots(page, 3);

    await openDialog(page, 'delete-shot-choice', {
      sectionId,
      shotId,
      sectionName: 'SplitTarget',
    });

    const dialog = page.getByTestId('delete-shot-choice-dialog');
    await expect(dialog).toBeVisible();

    await dialog.getByRole('button', { name: 'Split Section' }).click();

    await expect(dialog).not.toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('split-section-dialog')).toBeVisible({ timeout: 5000 });
  });
});

test.describe('Dialog Coverage - Split Section', () => {
  test('validates split names and selects the original section', async ({ page }) => {
    const { sectionId, shotId } = await setupProjectWithShots(page, 3);

    await openDialog(page, 'split-section', {
      sectionId,
      shotId,
      sectionName: 'SplitTarget',
    });

    const dialog = page.getByTestId('split-section-dialog');
    await expect(dialog).toBeVisible();

    const nameA = dialog.getByTestId('split-name-a');
    const nameB = dialog.getByTestId('split-name-b');
    await expect(nameA).toHaveValue('SplitTarget-A');
    await expect(nameB).toHaveValue('SplitTarget-B');

    await nameA.fill('');
    await nameB.fill('');

    const confirmButton = dialog.getByTestId('split-confirm');
    await expect(confirmButton).toBeDisabled();

    await nameA.fill('Split A');
    await nameB.fill('Split B');
    await expect(confirmButton).toBeEnabled();

    await confirmButton.click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const selection = await page.evaluate(() => {
      return (window as any).__E2E_TEST__.uiStore.getState().selection;
    });
    expect(selection?.type).toBe('section');
    expect(selection?.sectionId).toBe(sectionId);
  });
});

test.describe('Dialog Coverage - Merge Projects', () => {
  test('validates output name and merges selected projects', async ({ page }) => {
    await setupMockedPage(page);

    const selectedProjects = [
      {
        name: 'Alpha',
        location: null,
        file_path: '/mock/alpha.stalx',
        modified_at: '2026-01-15T00:00:00Z',
        section_count: 2,
        total_length: 120.5,
        station_count: 5,
        gps_center: null,
        gps_bounds: null,
      },
      {
        name: 'Beta',
        location: null,
        file_path: '/mock/beta.stalx',
        modified_at: '2026-01-20T00:00:00Z',
        section_count: 3,
        total_length: 1500,
        station_count: 6,
        gps_center: null,
        gps_bounds: null,
      },
    ];

    await openDialog(page, 'merge-projects', { selectedProjects });

    const dialog = page.getByTestId('merge-projects-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('merge-output-name-input')).toHaveValue('Alpha_merged');
    await expect(dialog.locator('text=Total sections:')).toBeVisible();
    await expect(dialog.locator('text=1.6 km')).toBeVisible();

    const outputInput = dialog.getByTestId('merge-output-name-input');
    await outputInput.fill('');
    const confirmButton = dialog.getByTestId('merge-projects-confirm-button');
    await expect(confirmButton).toBeDisabled();

    await outputInput.fill('Combined Project');

    const archiveCheckbox = dialog.getByTestId('merge-archive-checkbox');
    await archiveCheckbox.check();
    const openCheckbox = dialog.getByTestId('merge-open-checkbox');
    await openCheckbox.uncheck();

    await confirmButton.click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });
});

test.describe('Dialog Coverage - Error Dialog', () => {
  test('copies error details to clipboard and closes', async ({ page }) => {
    await setupMockedPage(page);

    await page.evaluate(() => {
      Object.defineProperty(navigator, 'clipboard', {
        value: { writeText: async () => {} },
        configurable: true,
      });
    });

    await openDialog(page, 'error', {
      title: 'Failure',
      message: 'Something went wrong',
      stackTrace: 'Error: Boom\\n  at line 1',
    });

    const dialog = page.getByTestId('error-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('text=Something went wrong')).toBeVisible();
    await expect(dialog.getByTestId('error-dialog-stack-trace')).toContainText('Error: Boom');

    const copyButton = dialog.getByTestId('error-dialog-copy-btn');
    await copyButton.click();
    await expect(copyButton).toHaveText('Copied!');

    await dialog.getByRole('button', { name: 'OK' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });
});
