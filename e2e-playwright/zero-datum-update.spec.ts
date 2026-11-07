import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';

/**
 * E2E tests for the zero datum update fix (fn-30-wcf).
 *
 * Verifies that updating GPS coordinates on an existing zero datum:
 * 1. Produces exactly ONE undo entry (not two from remove+set)
 * 2. No-change guard: saving without modifying lat/lon creates no undo entry
 */

/** Track IPC calls for set_zero_datum / remove_zero_datum */
interface IpcCallRecord {
  cmd: string;
  args: Record<string, unknown>;
}

/**
 * Create a new project and wait for map readiness.
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page
    .locator('button:has-text("New Project"), h3:has-text("New Project")')
    .first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });

  // Wait for map initialization (known flakiness pitfall)
  await page.waitForFunction(
    () => (window as unknown as { __stalagmight_map__?: unknown }).__stalagmight_map__ !== undefined,
    { timeout: 10000 }
  );
}

/**
 * Inject a section with a zero datum station into the project store.
 * Station 0 of this section will have is_zero_datum=true with GPS coordinates.
 */
async function injectZeroDatumFixture(
  page: Page,
  lat: number,
  lon: number
): Promise<void> {
  // Wait for the project store to have a project
  await page.waitForFunction(
    () => {
      const store = (window as unknown as Record<string, unknown>).__E2E_TEST__ as
        | { projectStore: { getState: () => { project: unknown } } }
        | undefined;
      return store && store.projectStore.getState().project !== null;
    },
    { timeout: 5000 }
  );

  await page.evaluate(
    ({ lat, lon }) => {
      const e2e = (window as unknown as Record<string, unknown>).__E2E_TEST__ as {
        projectStore: {
          getState: () => {
            project: Record<string, unknown> & {
              sections: Array<Record<string, unknown>>;
              zero_datums: Array<Record<string, unknown>>;
            };
            stations: Array<Record<string, unknown>>;
          };
          setState: (partial: Record<string, unknown>) => void;
        };
      };

      if (!e2e?.projectStore) {
        throw new Error('E2E projectStore not available');
      }

      const state = e2e.projectStore.getState();
      const existingProject = state.project;
      if (!existingProject) {
        throw new Error('Project is null');
      }

      // Create a test section with one shot
      const testSection = {
        id: 1,
        name: 'Datum Section',
        surveyor: 'Test Surveyor',
        date: '2026-01-28',
        color: '#4ecdc4',
        direction: 'in',
        shots: [
          {
            id: 1,
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
          },
        ],
      };

      const existingSections = existingProject.sections ?? [];

      // Update project with the datum section and zero datum entry
      const project = {
        ...existingProject,
        sections: [testSection, ...existingSections],
        zero_datums: [
          {
            station_ref: { section_id: 1, station_index: 0 },
            gps: { latitude: lat, longitude: lon, depth_reference: 0 },
          },
        ],
      };

      // Create mock stations for the new section
      const existingStations = state.stations ?? [];
      const newStations = [
        {
          station_ref: { section_id: 1, station_index: 0 },
          coordinates: { x: 0, y: 0, z: 0 },
          is_zero_datum: true,
          gps_coordinates: { latitude: lat, longitude: lon, depth_reference: 0 },
          merged_with: [],
        },
        {
          station_ref: { section_id: 1, station_index: 1 },
          coordinates: { x: 10, y: 0, z: -1 },
          is_zero_datum: false,
          gps_coordinates: null,
          merged_with: [],
        },
        ...existingStations,
      ];

      // Update store state atomically
      e2e.projectStore.setState({
        project,
        stations: newStations,
      });

      // Also update the mock backend so refresh returns updated data
      const mocks = (window as unknown as Record<string, unknown>).__E2E_MOCKS__ as
        | { setCurrentProject: (p: unknown) => void }
        | undefined;
      if (mocks?.setCurrentProject) {
        mocks.setCurrentProject(project);
      }
    },
    { lat, lon }
  );

  // Wait for React to re-render with the new state
  await page.waitForFunction(
    () => {
      const e2e = (window as unknown as Record<string, unknown>).__E2E_TEST__ as
        | {
            projectStore: {
              getState: () => {
                project: { sections: Array<{ id: number; name: string }> } | null;
              };
            };
          }
        | undefined;
      return e2e?.projectStore
        .getState()
        .project?.sections?.some(
          (s: { id: number; name: string }) => s.id === 1 && s.name === 'Datum Section'
        );
    },
    { timeout: 5000 }
  );

  // Wait for the section name to be visible in the UI
  await expect(page.locator('text=Datum Section')).toBeVisible({ timeout: 5000 });
}

/**
 * Extend mocks to:
 * 1. Track set_zero_datum / remove_zero_datum IPC calls
 * 2. Maintain a real mutating undo stack (entries pushed on set/remove, queryable via get_undo_stack)
 * MUST be called AFTER page.goto().
 */
async function extendMocksWithZeroDatumTracking(page: Page): Promise<void> {
  await page.evaluate(() => {
    // Track IPC calls for zero datum operations
    (window as unknown as Record<string, unknown>).__ZERO_DATUM_CALLS__ = [] as Array<{
      cmd: string;
      args: Record<string, unknown>;
    }>;

    // Mutating undo stack -- entries are pushed when set/remove_zero_datum is called
    // and can be queried via get_undo_stack, matching the real app behavior.
    (window as unknown as Record<string, unknown>).__ZERO_DATUM_UNDO_STACK__ = [] as Array<{
      index: number;
      description: string;
      timestamp: string;
      commandType: string;
      previewData: null;
    }>;

    const originalInvoke = (
      window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
        };
      }
    ).__TAURI_INTERNALS__.invoke;

    (
      window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
        };
      }
    ).__TAURI_INTERNALS__.invoke = async (cmd: string, args?: Record<string, unknown>) => {
      const calls = (
        window as unknown as {
          __ZERO_DATUM_CALLS__: Array<{ cmd: string; args: Record<string, unknown> }>;
        }
      ).__ZERO_DATUM_CALLS__;
      const undoStack = (
        window as unknown as {
          __ZERO_DATUM_UNDO_STACK__: Array<{
            index: number;
            description: string;
            timestamp: string;
            commandType: string;
            previewData: null;
          }>;
        }
      ).__ZERO_DATUM_UNDO_STACK__;

      if (cmd === 'set_zero_datum') {
        calls.push({ cmd, args: args ?? {} });
        undoStack.unshift({
          index: 0,
          description: 'Set zero datum',
          timestamp: new Date().toISOString(),
          commandType: 'set_zero_datum',
          previewData: null,
        });
        // Re-index entries
        undoStack.forEach((e, i) => {
          e.index = i;
        });
        // Delegate to base mock for project state mutation
        return originalInvoke(cmd, args);
      }

      if (cmd === 'remove_zero_datum') {
        calls.push({ cmd, args: args ?? {} });
        undoStack.unshift({
          index: 0,
          description: 'Remove zero datum',
          timestamp: new Date().toISOString(),
          commandType: 'remove_zero_datum',
          previewData: null,
        });
        undoStack.forEach((e, i) => {
          e.index = i;
        });
        return originalInvoke(cmd, args);
      }

      // For undo-related queries, return from our mutating stack
      if (cmd === 'get_undo_stack') {
        return undoStack;
      }

      if (cmd === 'can_undo') {
        return undoStack.length > 0;
      }

      if (cmd === 'get_undo_state') {
        return { can_undo: undoStack.length > 0, can_redo: false };
      }

      return originalInvoke(cmd, args);
    };

    console.log('[Mock IPC] Extended mocks with zero datum tracking + mutating undo stack');
  });
}

/**
 * Get the zero-datum-related IPC calls.
 */
async function getZeroDatumCalls(page: Page): Promise<IpcCallRecord[]> {
  return page.evaluate(() => {
    return (
      (
        window as unknown as {
          __ZERO_DATUM_CALLS__: Array<{ cmd: string; args: Record<string, unknown> }>;
        }
      ).__ZERO_DATUM_CALLS__ || []
    );
  });
}

/**
 * Get the undo stack length from the mutating mock (via IPC, same as the real app).
 */
async function getUndoStackLength(page: Page): Promise<number> {
  return page.evaluate(async () => {
    const result = await (
      window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (cmd: string) => Promise<unknown>;
        };
      }
    ).__TAURI_INTERNALS__.invoke('get_undo_stack');
    return Array.isArray(result) ? result.length : 0;
  });
}

/**
 * Open the EditStationDialog for a specific station via the store.
 */
async function openEditStationDialog(
  page: Page,
  sectionId: number,
  stationIndex: number
): Promise<void> {
  await page.evaluate(
    ({ sectionId, stationIndex }) => {
      const e2e = (window as unknown as Record<string, unknown>).__E2E_TEST__ as
        | {
            uiStore: {
              getState: () => {
                openDialog: (name: string, data: Record<string, unknown>) => void;
              };
            };
          }
        | undefined;

      if (!e2e?.uiStore) {
        throw new Error('E2E uiStore not available');
      }

      e2e.uiStore.getState().openDialog('edit-station', {
        stationRef: { section_id: sectionId, station_index: stationIndex },
      });
    },
    { sectionId, stationIndex }
  );

  // Wait for dialog to be visible
  await expect(page.getByTestId('edit-station-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Zero Datum Update - Single Undo Entry', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Zero Datum Test');
    await injectZeroDatumFixture(page, 25.0, -80.0);
    await extendMocksWithZeroDatumTracking(page);
  });

  test('updating GPS on existing zero datum produces exactly one undo entry', async ({
    page,
  }) => {
    // Record baseline undo stack length via IPC
    const baselineLength = await getUndoStackLength(page);

    // Open EditStationDialog for station 0 of section 1 (the zero datum station)
    await openEditStationDialog(page, 1, 0);

    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();

    // Verify the checkbox is checked (station is already a zero datum)
    const checkbox = dialog.locator('#zeroDatum');
    await expect(checkbox).toBeChecked();

    // Verify GPS field is populated; dialog now uses a single combined input
    const gpsInput = dialog.getByTestId('station-gps-input');
    await expect(gpsInput).toHaveValue('25, -80');

    // Change latitude from 25.0 to 26.0
    await gpsInput.fill('26, -80');

    // Click Save
    const saveButton = dialog.locator('button:has-text("Save")');
    await saveButton.click();

    // Wait for dialog to close (indicates save completed)
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify exactly one set_zero_datum call was made (no remove_zero_datum)
    const calls = await getZeroDatumCalls(page);
    const setCallCount = calls.filter((c) => c.cmd === 'set_zero_datum').length;
    const removeCallCount = calls.filter((c) => c.cmd === 'remove_zero_datum').length;

    expect(removeCallCount).toBe(0);
    expect(setCallCount).toBe(1);

    // Verify undo stack grew by exactly 1 entry (queried via real IPC get_undo_stack)
    const finalLength = await getUndoStackLength(page);
    expect(finalLength - baselineLength).toBe(1);

    // UI assertion: undo button should be enabled and dropdown should show exactly 1 item
    const undoButton = page.getByTestId('toolbar-undo-button');
    await expect(undoButton).toBeEnabled();

    // Hover to open undo dropdown
    await undoButton.hover();
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');
    await expect(dropdown).toBeVisible({ timeout: 5000 });

    // Verify exactly 1 item in the dropdown (delta = 1)
    const items = dropdown.locator('[data-testid^="toolbar-undo-button-item-"]');
    await expect(items).toHaveCount(1);
  });

  test('saving without modifying lat/lon creates no new undo entry', async ({ page }) => {
    // Record baseline undo stack length via IPC
    const baselineLength = await getUndoStackLength(page);

    // Open EditStationDialog for station 0 of section 1
    await openEditStationDialog(page, 1, 0);

    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();

    // Verify the checkbox is checked
    const checkbox = dialog.locator('#zeroDatum');
    await expect(checkbox).toBeChecked();

    // Do NOT change any values - just click Save
    const saveButton = dialog.locator('button:has-text("Save")');
    await saveButton.click();

    // Wait for dialog to close
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify no zero datum IPC calls were made at all (no-change guard)
    const calls = await getZeroDatumCalls(page);
    expect(calls.length).toBe(0);

    // Verify undo stack did not grow (queried via real IPC get_undo_stack)
    const finalLength = await getUndoStackLength(page);
    expect(finalLength - baselineLength).toBe(0);
  });
});

/**
 * #576 (D39e, D37): what the Edit Station GPS field previews is what Save
 * stores, an ambiguous reading is shown rather than guessed silently, and a
 * reading impossible for its own letter cannot be saved.
 */
test.describe('Zero Datum Update - coordinate entry', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Zero Datum Entry Test');
    await injectZeroDatumFixture(page, 25.0, -80.0);
    await extendMocksWithZeroDatumTracking(page);
  });

  test('previews, notes and stores UTM "33S" as the southern hemisphere; refuses an impossible band', async ({
    page,
  }) => {
    await openEditStationDialog(page, 1, 0);
    const dialog = page.getByTestId('edit-station-dialog');
    const gpsInput = dialog.getByTestId('station-gps-input');

    // Band U cannot hold a northing at 78.96N: error, no preview, Save refuses.
    await gpsInput.fill('33U 512345 8765432');
    await expect(dialog.getByTestId('station-gps-error')).toContainText('band U');
    await expect(dialog.getByTestId('station-gps-parsed')).toHaveCount(0);
    await dialog.getByTestId('edit-station-save').click();
    await expect(dialog).toContainText('Please enter valid GPS coordinates');
    expect(await getZeroDatumCalls(page)).toHaveLength(0);

    // "33S" with a southern northing: read as the southern hemisphere, with a note.
    await gpsInput.fill('33S 512345 8765432');
    await expect(dialog.getByTestId('station-gps-parsed')).toHaveText('Lat: -11.168093, Lon: 15.113069');
    await expect(dialog.getByTestId('station-gps-note')).toContainText('southern hemisphere');
    await expect(dialog.getByTestId('station-gps-format')).toHaveText('UTM');
    await dialog.getByTestId('edit-station-save').click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    const calls = (await getZeroDatumCalls(page)).filter((c) => c.cmd === 'set_zero_datum');
    expect(calls).toHaveLength(1);
    const coords = calls[0]!.args.coordinates as { latitude: number; longitude: number };
    expect(coords.latitude).toBeCloseTo(-11.1680928, 5);
    expect(coords.longitude).toBeCloseTo(15.1130686, 5);
  });

  test('keeps the sign of a -0 DDM degree and flags a swapped decimal pair', async ({ page }) => {
    await openEditStationDialog(page, 1, 0);
    const dialog = page.getByTestId('edit-station-dialog');
    const gpsInput = dialog.getByTestId('station-gps-input');

    await gpsInput.fill(`-0°30.5'N 87°18.5'W`);
    await expect(dialog.getByTestId('station-gps-parsed')).toHaveText('Lat: -0.508333, Lon: -87.308333');
    await expect(dialog.getByTestId('station-gps-note')).toContainText('minus sign');

    // Advisory only: the swapped pair still parses.
    await gpsInput.fill('-87.3, 20.5');
    await expect(dialog.getByTestId('station-gps-transposed-warning')).toBeVisible();
    await expect(dialog.getByTestId('station-gps-note')).toHaveCount(0);

    // Clearing the field clears the preview, the note and the hint.
    await gpsInput.fill('');
    await expect(dialog.getByTestId('station-gps-parsed')).toHaveCount(0);
    await expect(dialog.getByTestId('station-gps-transposed-warning')).toHaveCount(0);
  });
});
