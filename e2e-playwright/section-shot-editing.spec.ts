import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForMapIdle,
  waitForNoDialogs,
  type SectionSeed,
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

/** Edit one cell of the shot-table grid: open the overlay editor, replace the
 *  value, commit. Replaces the per-cell `<input>` the old table rendered. */
async function editCell(
  dialog: Locator,
  row: number,
  colKey: string,
  value: string
): Promise<void> {
  await dialog.getByTestId(`shot-editor-cell-${row}-${colKey}`).dblclick();
  const editor = dialog.getByTestId('shot-editor-cell-editor');
  await editor.fill(value);
  await editor.press('Enter');
}

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Main Passage',
    direction: 'in',
    shots: [
      {
        azimuth_in: 45,
        azimuth_out: 225,
        length: 10.5,
        depth_in: -2,
        depth_out: -2.5,
        left: 1.5,
        right: 2.3,
        up: 3.0,
        down: 0.8,
        comment: 'Entrance point with good visibility',
      },
      {
        azimuth_in: 90,
        azimuth_out: 270,
        length: 15.2,
        depth_in: -2.5,
        depth_out: -5.0,
        left: 0.5,
        right: 0.7,
        up: 1.2,
        down: 1.5,
        comment: null,
      },
    ],
  },
  {
    name: 'Side Tunnel',
    direction: 'out',
    shots: [
      {
        azimuth_in: 270,
        azimuth_out: 90,
        length: 5.0,
        depth_in: -3.0,
        depth_out: -4.0,
        left: 0,
        right: 0,
        up: 0,
        down: 0,
        comment: 'Narrow restriction',
      },
    ],
  },
];

async function createProjectWithSections(page: Page): Promise<void> {
  await createProject(page, 'Edit Test Project');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

async function getShotInfo(
  page: Page,
  sectionName: string,
  shotIndex: number
): Promise<{ sectionId: number; shotId: number }> {
  const result = await page.evaluate(
    ({ targetName, targetIndex }) => {
      const project = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: { getState: () => { project: { sections: Array<{ id: number; name: string; shots: Array<{ id: number; length: number }> }> } | null } };
        };
      }).__E2E_TEST__?.projectStore.getState().project;

      const section = project?.sections.find((s) => s.name === targetName);
      const shot = section?.shots?.[targetIndex];

      if (!section || !shot) return null;

      return { sectionId: section.id, shotId: shot.id };
    },
    { targetName: sectionName, targetIndex: shotIndex }
  );

  if (!result) {
    throw new Error(`Shot not found: ${sectionName} index ${shotIndex}`);
  }

  return result;
}

async function selectShotInStore(page: Page, sectionName: string, shotIndex: number): Promise<void> {
  const shotInfo = await getShotInfo(page, sectionName, shotIndex);

  await page.evaluate(({ sectionId, shotId }) => {
    const uiStore = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { setSelection: (selection: unknown) => void; setDetailPanelTab: (tab: string) => void } } };
    }).__E2E_TEST__?.uiStore;

    if (!uiStore) return;

    uiStore.getState().setDetailPanelTab('details');
    uiStore.getState().setSelection({ type: 'shot', sectionId, shotId });
  }, shotInfo);
}

async function selectSectionInStore(page: Page, sectionName: string): Promise<number> {
  const sectionId = await page.evaluate((targetName) => {
    const project = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { sections: Array<{ id: number; name: string }> } | null } };
      };
    }).__E2E_TEST__?.projectStore.getState().project;

    return project?.sections.find((s) => s.name === targetName)?.id ?? null;
  }, sectionName);

  if (sectionId === null) {
    throw new Error(`Section not found: ${sectionName}`);
  }

  await page.evaluate((id) => {
    const uiStore = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { setSelection: (selection: unknown) => void; setDetailPanelTab: (tab: string) => void } } };
    }).__E2E_TEST__?.uiStore;

    if (!uiStore) return;

    uiStore.getState().setDetailPanelTab('details');
    uiStore.getState().setSelection({ type: 'section', sectionId: id });
  }, sectionId);

  return sectionId;
}

async function openEditSectionDialog(page: Page, sectionName: string): Promise<void> {
  const sectionId = await selectSectionInStore(page, sectionName);

  await page.evaluate((id) => {
    const uiStore = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { openDialog: (dialog: string, data?: Record<string, unknown>) => void } } };
    }).__E2E_TEST__?.uiStore;

    if (!uiStore) return;

    uiStore.getState().openDialog('edit-section', { sectionId: id });
  }, sectionId);

  await expect(page.getByTestId('edit-section-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Section and Shot Editing', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithSections(page);
  });

  test('should show shot details in detail panel @smoke', async ({ page }) => {
    await selectShotInStore(page, 'Main Passage', 0);

    const detailPanel = page.getByTestId('detail-panel');
    await expect(detailPanel).toBeVisible();
    await expect(detailPanel.getByRole('heading', { name: 'Shot' })).toBeVisible();

    await expect(detailPanel).toContainText('Length');
    await expect(detailPanel).toContainText('10.5 m');
    await expect(detailPanel).toContainText('Azimuth (in)');
    await expect(detailPanel).toContainText('45.0°');
    await expect(detailPanel).toContainText('LRUD:');
    await expect(detailPanel).toContainText('Entrance point with good visibility');
  });

  test('should edit shot values in edit section dialog @smoke', async ({ page }) => {
    await openEditSectionDialog(page, 'Main Passage');

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible();

    // The shot table is a SpreadsheetGrid: one editor overlays the active cell
    // rather than an input per cell, so edits are open-type-commit.
    await editCell(dialog, 0, 'length', '12.3');
    await expect(dialog.getByTestId('shot-editor-cell-0-length')).toHaveText('12.3');

    await editCell(dialog, 0, 'comment', 'Updated comment');
    await expect(dialog.getByTestId('shot-editor-cell-0-comment')).toHaveText('Updated comment');

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('the eye menu hides a column, and the layout persists across reopen', async ({ page }) => {
    await openEditSectionDialog(page, 'Main Passage');
    const dialog = page.getByTestId('edit-section-dialog');

    // Everything is visible by default - this grid edits a survey that exists.
    await expect(dialog.getByTestId('shot-editor-header-comment')).toBeVisible();

    await dialog.getByTestId('shot-editor-columns-menu').click();
    await dialog.getByTestId('shot-editor-col-vis-comment').click();
    await page.keyboard.press('Escape');
    await expect(dialog.getByTestId('shot-editor-header-comment')).toHaveCount(0);

    // The debounced save reaches update_edit_section_view, and the stored config.
    await page.waitForFunction(
      () => {
        const hidden = ((window as unknown as Record<string, unknown>).__E2E_TEST__ as
          | { projectStore: { getState: () => { config?: { panels?: Record<string, string[]> } } } }
          | undefined
        )?.projectStore.getState().config?.panels?.edit_section_hidden_columns ?? [];
        return hidden.includes('comment');
      },
      undefined,
      { timeout: 5000 }
    );

    // Reopen: the layout comes back rather than resetting.
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
    await openEditSectionDialog(page, 'Main Passage');
    await expect(
      page.getByTestId('edit-section-dialog').getByTestId('shot-editor-header-comment')
    ).toHaveCount(0);
  });

  test('an incomplete row reds its missing cells; emptying it stops blocking Save', async ({
    page,
  }) => {
    await openEditSectionDialog(page, 'Main Passage');
    const dialog = page.getByTestId('edit-section-dialog');
    const save = dialog.getByTestId('edit-section-save');
    await expect(save).toBeEnabled();

    // The trailing blank row materializes on the first keystroke. With only a
    // length it is incomplete, so the cells it still needs go red and Save
    // refuses - the same treatment the data table gives a missing required cell.
    const lengthCells = dialog.locator(
      '[data-testid^="shot-editor-cell-"][data-testid$="-length"]'
    );
    const blank = (await lengthCells.count()) - 1;
    await dialog.getByTestId(`shot-editor-cell-${blank}-length`).dblclick();
    const editor = dialog.getByTestId('shot-editor-cell-editor');
    await editor.fill('7');
    await editor.press('Enter');

    for (const col of ['_azimuth_avg', 'depth_in', 'depth_out']) {
      await expect(dialog.getByTestId(`shot-editor-cell-${blank}-${col}`)).toHaveClass(
        /sg-cell-missing/
      );
    }
    await expect(dialog.getByTestId(`shot-editor-cell-${blank}-length`)).not.toHaveClass(
      /sg-cell-missing/
    );
    await expect(save).toBeDisabled();
    await expect(dialog.getByTestId('edit-section-blocker')).toContainText('missing required');

    // Clearing the one value it holds makes it nothing again: an added row left
    // empty is an invitation, not an obligation, so it blocks nothing.
    await dialog.getByTestId(`shot-editor-cell-${blank}-length`).click();
    await page.keyboard.press('Delete');
    await expect(save).toBeEnabled();
    await expect(dialog.getByTestId('edit-section-blocker')).toHaveCount(0);
  });

  test('Expand enlarges the shot table and Shrink puts it back', async ({ page }) => {
    // A tall viewport, because the collapsed dialog is already 90vh: on a short
    // window fullscreen frees almost nothing and the assertion below would be
    // measuring the window rather than the feature.
    await page.setViewportSize({ width: 1280, height: 1200 });
    await openEditSectionDialog(page, 'Main Passage');
    const dialog = page.getByTestId('edit-section-dialog');
    const table = dialog.getByTestId('shot-table');
    const collapsed = (await table.boundingBox())!.height;

    await dialog.getByTestId('shot-editor-expand').click();
    await expect.poll(async () => (await table.boundingBox())!.height).toBeGreaterThan(collapsed);
    // Save stays reachable while expanded - the point of growing the dialog
    // rather than letting the grid take over the window.
    await expect(dialog.getByRole('button', { name: 'Save' })).toBeVisible();

    await dialog.getByTestId('shot-editor-expand').click();
    await expect.poll(async () => (await table.boundingBox())!.height).toBe(collapsed);
  });

  test('should edit LRUD values', async ({ page }) => {
    await openEditSectionDialog(page, 'Main Passage');

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible();

    await editCell(dialog, 0, 'left', '2.2');
    await editCell(dialog, 0, 'right', '3.1');
    await editCell(dialog, 0, 'up', '4.4');
    await editCell(dialog, 0, 'down', '1.7');

    await expect(dialog.getByTestId('shot-editor-cell-0-left')).toHaveText('2.2');
    await expect(dialog.getByTestId('shot-editor-cell-0-right')).toHaveText('3.1');
    await expect(dialog.getByTestId('shot-editor-cell-0-up')).toHaveText('4.4');
    await expect(dialog.getByTestId('shot-editor-cell-0-down')).toHaveText('1.7');

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should delete shot with confirmation', async ({ page }) => {
    await selectShotInStore(page, 'Main Passage', 0);

    const detailPanel = page.getByTestId('detail-panel');
    await expect(detailPanel.getByRole('heading', { name: 'Shot' })).toBeVisible();

    await detailPanel.getByRole('button', { name: 'Delete' }).click();

    const confirmDialog = page.getByTestId('confirm-dialog');
    await expect(confirmDialog).toBeVisible({ timeout: 3000 });

    await confirmDialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(confirmDialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should edit section metadata', async ({ page }) => {
    await openEditSectionDialog(page, 'Main Passage');

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible();

    await expect(dialog.getByTestId('section-name-input')).toHaveValue('Main Passage');
    await expect(dialog.getByTestId('section-surveyors-pill-0-label')).toHaveText('Test Surveyor');
    await expect(dialog.getByTestId('section-date-input')).toBeVisible();
    await expect(dialog.getByTestId('section-color-hex-input')).toHaveValue('#3b82f6');
    await expect(dialog.getByTestId('section-direction-select')).toContainText('In');

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should change section direction in edit dialog', async ({ page }) => {
    await openEditSectionDialog(page, 'Main Passage');

    const dialog = page.getByTestId('edit-section-dialog');
    const directionSelect = dialog.getByTestId('section-direction-select');
    await expect(directionSelect).toContainText('In');

    await directionSelect.click();

    const outOption = page.getByRole('listbox').locator('[role="option"]:has-text("Out")');
    await outOption.click();

    await expect(directionSelect).toContainText('Out');

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should delete section with confirmation', async ({ page }) => {
    const sectionId = await selectSectionInStore(page, 'Side Tunnel');

    // Section delete now lives in the right-click / long-press context menu
    // (the per-row button is the focus-mode affordance).
    await page.getByTestId(`section-row-${sectionId}`).click({ button: 'right' });
    await page.getByRole('button', { name: 'Delete Section' }).click();

    const confirmDialog = page.getByTestId('confirm-dialog');
    await expect(confirmDialog).toBeVisible({ timeout: 3000 });

    await confirmDialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(confirmDialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should add new shot to section @smoke', async ({ page }) => {
    await openEditSectionDialog(page, 'Main Passage');

    const dialog = page.getByTestId('edit-section-dialog');
    const shotRows = dialog.locator('[data-testid^="shot-row-"]');
    const initialCount = await shotRows.count();

    // A shot is added by typing into the trailing blank row - the app-wide
    // convention; there is no Add button.
    await addShotByTypingInBlankRow(dialog);

    await expect(shotRows).toHaveCount(initialCount + 1);

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });
});
