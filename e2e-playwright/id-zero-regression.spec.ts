/**
 * E2E tests for id=0 scenarios.
 *
 * These tests verify that dialogs and components work correctly when the
 * section or shot ID is 0 (the first element). This was a regression where
 * JavaScript's falsy check (`if (!id)`) incorrectly treated id=0 as falsy.
 */
import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';

/**
 * Helper to create a new project
 */
async function createProject(page: Page, name: string) {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

/**
 * Inject a fixture with section id=0 and shot id=0 into the project store.
 * This simulates the state after loading a project where the first section/shot
 * has id=0 (as assigned by the Rust backend).
 *
 * We ADD the section to existing sections to avoid issues with React re-rendering.
 */
async function injectId0Fixture(page: Page) {
  // First wait for the project store to be ready with a project
  await page.waitForFunction(() => {
    const store = (window as any).__E2E_TEST__?.projectStore;
    return store && store.getState()?.project !== null;
  }, { timeout: 5000 });

  await page.evaluate(() => {
    // The survey credit rides on each shot (a section keeps none of its own).
    const testSection = {
      id: 0,
      name: 'Zero Section',
      color: '#4ecdc4',
      direction: 'in' as const,
      shots: [{
        id: 0,
        azimuth_in: 90,
        azimuth_out: 270,
        length: 10,
        depth_in: 0,
        depth_out: -1,
        pitch_in: null,
        pitch_out: null,
        left: 1,
        right: 1,
        up: 2,
        down: 0.5,
        comment: null,
        kind: 'Normal',
        marker_index: 0,
        temperature: null,
        credit: { surveyors: [{ name: 'Test Surveyor' }], explorers: [], date: '2026-01-20' },
      }]
    };

    // Get current state - project is guaranteed non-null by waitForFunction
    const store = (window as any).__E2E_TEST__.projectStore.getState();
    const existingProject = store.project;
    if (!existingProject) {
      throw new Error('Project is null - cannot inject fixture');
    }

    const existingSections = existingProject.sections ?? [];

    // Add section with id=0 to the BEGINNING of sections array
    // (so it's the first section like it would be from Rust backend)
    const project = {
      ...existingProject,
      sections: [testSection, ...existingSections]
    };

    // Generate mock stations for section id=0
    const existingStations = store.stations ?? [];
    const newStations = [
      {
        station_ref: { section_id: 0, station_index: 0 },
        coordinates: { x: -20, y: 0, z: 0 },
        is_zero_datum: true,
        gps_coordinates: null,
        merged_with: [],
      },
      {
        station_ref: { section_id: 0, station_index: 1 },
        coordinates: { x: -10, y: 0, z: 0 },
        is_zero_datum: false,
        gps_coordinates: null,
        merged_with: [],
      },
      ...existingStations
    ];

    // Update store state atomically
    (window as any).__E2E_TEST__.projectStore.setState({
      project,
      stations: newStations,
    });

    // Also update the mock backend so refresh returns updated data
    if ((window as any).__E2E_MOCKS__?.setCurrentProject) {
      (window as any).__E2E_MOCKS__.setCurrentProject(project);
    }
  });

  // Wait for React to re-render with the new state - use waitForFunction to be more reliable
  await page.waitForFunction(() => {
    const state = (window as any).__E2E_TEST__.projectStore.getState();
    return state.project?.sections?.some((s: any) => s.id === 0 && s.name === 'Zero Section');
  }, { timeout: 5000 });

  // Wait for the section to appear in the UI
  await expect(page.locator('text=Zero Section')).toBeVisible({ timeout: 5000 });
}

test.describe('ID=0 Bug Fixes', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'ID Zero Test');
    await injectId0Fixture(page);
  });

  test('Edit Section dialog opens, loads data, saves, and persists when sectionId === 0', async ({ page }) => {
    // Open the Edit Section dialog via store
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });

    // Wait for dialog to appear
    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Verify the dialog loads the correct section data (proves id=0 lookup works)
    const nameInput = page.getByTestId('section-name-input');
    await expect(nameInput).toHaveValue('Zero Section');

    // Modify the section name
    await nameInput.clear();
    await nameInput.fill('Modified Section Name');

    // Click Save button within the dialog
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify dialog closes (save was successful - proves id=0 is passed correctly)
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify persistence: re-open dialog and check the value persisted
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });
    await expect(dialog).toBeVisible({ timeout: 3000 });
    await expect(nameInput).toHaveValue('Modified Section Name');
  });

  test('Edit Shot dialog opens, loads data, saves, and persists when shotId === 0', async ({ page }) => {
    // Open the Edit Shot dialog via store
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-shot', { sectionId: 0, shotId: 0 });
    });

    // Wait for dialog to appear
    const dialog = page.getByTestId('edit-shot-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Verify the dialog loaded (would show error if id=0 wasn't found)
    const lengthLabel = dialog.locator('label:has-text("Length")');
    await expect(lengthLabel).toBeVisible();

    // Find the Depth Out input by its label and modify it
    const depthOutLabel = dialog.locator('label:has-text("Depth Out")');
    const depthOutInput = depthOutLabel.locator('..').locator('input');
    await depthOutInput.clear();
    await depthOutInput.fill('-2.5');

    // Click Save button within the dialog
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify dialog closes (save was successful)
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify persistence: re-open dialog and check the value persisted
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-shot', { sectionId: 0, shotId: 0 });
    });
    await expect(dialog).toBeVisible({ timeout: 3000 });
    // Check the depth out field shows the new value
    const depthOutValue = await depthOutInput.inputValue();
    expect(parseFloat(depthOutValue)).toBe(-2.5);
  });

  test('Section Adjust dialog applies changes when sectionId === 0', async ({ page }) => {
    // Verify the section exists before opening dialog
    await expect.poll(async () => {
      return page.evaluate(() => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        return state.project?.sections?.some((s: any) => s.id === 0);
      });
    }, { timeout: 5000 }).toBe(true);

    // Open the Section Adjust dialog via store
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('section-adjust', { sectionId: 0 });
    });

    // Wait for dialog to appear
    const dialog = page.getByTestId('section-adjust-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Verify the dialog shows section name (proves id=0 lookup works)
    await expect(dialog.locator('text=Zero Section')).toBeVisible({ timeout: 2000 });

    // Enter dx value
    const dxInput = page.getByTestId('dx-input');
    await dxInput.fill('5');

    // Wait for debounced preview
    await page.waitForTimeout(300);

    // Click Apply button
    const applyButton = dialog.locator('button:has-text("Apply")');
    await expect(applyButton).toBeVisible({ timeout: 2000 });
    await applyButton.click();

    // Verify dialog closes (apply was successful - proves id=0 works with moveSection)
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });

  test('Section Adjust dialog cancels correctly when sectionId === 0', async ({ page }) => {
    // Verify the section exists before opening dialog
    await expect.poll(async () => {
      return page.evaluate(() => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        return state.project?.sections?.some((s: any) => s.id === 0);
      });
    }, { timeout: 5000 }).toBe(true);

    // Open the Section Adjust dialog via store
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('section-adjust', { sectionId: 0 });
    });

    // Wait for dialog to appear
    const dialog = page.getByTestId('section-adjust-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Verify the dialog shows section name (proves id=0 lookup works)
    await expect(dialog.locator('text=Zero Section')).toBeVisible({ timeout: 2000 });

    // Enter dx value (this triggers preview)
    const dxInput = page.getByTestId('dx-input');
    await dxInput.fill('5');

    // Wait for debounced preview
    await page.waitForTimeout(300);

    // Click Cancel button
    const cancelButton = dialog.locator('button:has-text("Cancel")');
    await expect(cancelButton).toBeVisible({ timeout: 2000 });
    await cancelButton.click();

    // Verify dialog closes (cancel was successful - proves preview revert works with id=0)
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });

  test('DataTable renders warnings for section with id=0', async ({ page }) => {
    // Inject a warning for sectionId=0, shotId=0
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.projectStore.setState({
        warnings: [{
          sectionId: 0,
          shotId: 0,
          shotIndex: 0,
          code: 'TEST_WARNING',
          message: 'Test warning for shot 0',
          severity: 'warning',
          isResolved: false,
        }]
      });
    });

    // Open DataTable via store (direct state set)
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.setState({ dataTableOpen: true });
    });

    // Wait for DataTable to be visible
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });

    // Add a second section (id=1) and park the table on it first, so that the
    // subsequent switch to section id=0 is observable (the table auto-selects
    // sections[0] = Zero Section, which would mask a broken id=0 selection).
    await page.evaluate(() => {
      const store = (window as any).__E2E_TEST__.projectStore;
      const project = store.getState().project;
      const otherSection = {
        id: 1,
        name: 'One Section',
        color: '#ff6b6b',
        direction: 'in' as const,
        shots: [],
      };
      store.setState({ project: { ...project, sections: [...project.sections, otherSection] } });
      (window as any).__E2E_TEST__.uiStore.getState().setSelection({ type: 'section', sectionId: 1 });
    });
    await expect(page.getByTestId('data-table-section-pill')).toContainText('One Section', { timeout: 3000 });

    // Select the section with id=0 via the app-wide selection (the table follows
    // external selection; its header no longer has a section dropdown). This
    // exercises the `selection.sectionId !== undefined` path with sectionId=0 -
    // a falsy check would ignore the selection and leave the pill on 'One Section'.
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().setSelection({ type: 'section', sectionId: 0 });
    });

    // Header pill follows the selection - proves the id=0 section was resolved
    await expect(page.getByTestId('data-table-section-pill')).toContainText('Zero Section', { timeout: 3000 });

    // Verify the warning indicator is visible with the correct data-testid
    await expect(page.getByTestId('warning-0-0')).toBeVisible({ timeout: 3000 });
  });
});

test.describe('ID=0 Edge Cases', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'ID Zero Edge Cases');
    await injectId0Fixture(page);
  });

  test('Edit Section dialog handles save with surveyor change for id=0', async ({ page }) => {
    // Open the Edit Section dialog
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Verify dialog loaded correct data: the shot's surveyor as a pill
    const firstPill = page.getByTestId('section-surveyors-pill-0-label');
    await expect(firstPill).toHaveText('Test Surveyor');

    // Change the surveyor: x the old pill, add the new one
    await page.getByTestId('section-surveyors-pill-0-remove').click();
    const surveyorInput = page.getByTestId('section-surveyors-input');
    await surveyorInput.fill('New Surveyor');
    await surveyorInput.press('Enter');
    await expect(firstPill).toHaveText('New Surveyor');

    // Save within the dialog
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify dialog closes
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify persistence: re-open and check
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });
    await expect(dialog).toBeVisible({ timeout: 3000 });
    await expect(firstPill).toHaveText('New Surveyor');
    await expect(page.getByTestId('section-surveyors-pill-1')).toHaveCount(0);
  });

  test('Edit Section dialog handles save with color change for id=0', async ({ page }) => {
    // Open the Edit Section dialog
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Verify dialog loaded correct data
    const colorHexInput = page.getByTestId('section-color-hex-input');
    await expect(colorHexInput).toHaveValue('#4ecdc4');

    // Change the color via hex input
    await colorHexInput.clear();
    await colorHexInput.fill('#ff0000');

    // Save within the dialog
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify dialog closes
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify persistence: re-open and check
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });
    await expect(dialog).toBeVisible({ timeout: 3000 });
    await expect(colorHexInput).toHaveValue('#ff0000');
  });

  test('Edit Section dialog handles save with direction change for id=0', async ({ page }) => {
    // Open the Edit Section dialog
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // The Select component is custom - need to click to open dropdown, then select option
    const directionSelect = page.getByTestId('section-direction-select');
    // The test ID is attached directly to the trigger button.
    await directionSelect.click();

    // Wait for dropdown and click the "Out" option
    // Use data-testid if available, otherwise scope carefully
    await page.waitForTimeout(100); // Wait for dropdown to render
    const outOption = page.locator('[data-testid="direction-option-out"], [role="option"]:has-text("Out")').first();
    if (await outOption.isVisible().catch(() => false)) {
      await outOption.click();
    } else {
      // Fallback: find dropdown content and click Out within it
      await page.locator('button:text-is("Out")').click();
    }

    // Save within the dialog
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify dialog closes
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify persistence: re-open and check direction changed
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });
    await expect(dialog).toBeVisible({ timeout: 3000 });
    // Check the select shows "Out"
    await expect(directionSelect).toHaveText(/Out/i);
  });

  test('Edit Section dialog handles save without changes for id=0', async ({ page }) => {
    // Open the Edit Section dialog
    await page.evaluate(() => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: 0 });
    });

    const dialog = page.getByTestId('edit-section-dialog');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Verify the dialog loaded correctly
    const nameInput = page.getByTestId('section-name-input');
    await expect(nameInput).toHaveValue('Zero Section');

    // Save without making any changes - scoped to dialog
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify dialog closes without error
    await expect(dialog).not.toBeVisible({ timeout: 5000 });
  });
});
