/**
 * E2E: the workspace modes with Data among them.
 *
 * The carousel is Survey | Data | Plan (Export opens from the project menu and is
 * not a carousel mode). Data is EDITABLE like Survey: Import and Undo/Redo stay in
 * the toolbar, and its map pane - opened on demand beside the table - gets the
 * full tool set. Plan stays read-only. The map is the same MapLibre instance in
 * every mode: switching to Data and opening or closing its pane never rebuilds it.
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 */
import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithSections, waitForMapIdle } from './helpers/app';

type Win = {
  __E2E_TEST__: {
    uiStore: {
      getState: () => {
        appMode: string;
        geometryMode: string;
        showGrid: boolean;
        activeDialog: string | null;
        toggleDisplay: (key: string) => void;
        setSelection: (sel: unknown) => void;
        closeDialog: () => void;
      };
    };
    projectStore: { getState: () => { project: { sections: Array<{ id: number }> } | null } };
    dataWorkspaceStore: {
      getState: () => { mapPaneOpen: boolean; mapPaneWidth: number };
      setState: (s: Record<string, unknown>) => void;
    };
  };
  __stalagmight_map__?: unknown;
  __mapBeforeSwitch__?: unknown;
};

async function setMapPaneOpen(page: Page, open: boolean): Promise<void> {
  await page.evaluate((o) => {
    (window as unknown as Win).__E2E_TEST__.dataWorkspaceStore.setState({ mapPaneOpen: o });
  }, open);
}

const appMode = (page: Page) => page.evaluate(() => (window as unknown as Win).__E2E_TEST__.uiStore.getState().appMode);

test.describe('Workspace modes', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic modes');
    await seedProjectWithSections(page, [{
      name: 'Synthetic passage', direction: 'in',
      shots: [
        { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
        { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      ],
    }]);
    await waitForMapIdle(page);
    // The pane flag is persisted view state: start every test from closed.
    await setMapPaneOpen(page, false);
  });

  test('the carousel shows exactly Survey, Data and Plan', async ({ page }) => {
    const buttons = page.getByTestId('mode-carousel').getByRole('button');
    await expect(buttons).toHaveCount(3);
    expect(await buttons.evaluateAll((els) => els.map((el) => el.getAttribute('data-testid')))).toEqual([
      'mode-survey',
      'mode-data',
      'mode-plan',
    ]);
    await expect(buttons).toHaveText(['Survey', 'Data', 'Plan']);
    await expect(page.getByTestId('mode-export')).toHaveCount(0);
    await expect(page.getByTestId('mode-survey')).toHaveAttribute('aria-pressed', 'true');
  });

  test('Data shows its workspace in place of the docks, and the map pane beside it on request', async ({ page }) => {
    await page.evaluate(() => {
      const w = window as unknown as Win;
      w.__mapBeforeSwitch__ = w.__stalagmight_map__;
    });
    await page.getByTestId('mode-data').click();
    await expect(page.getByTestId('mode-data')).toHaveAttribute('aria-pressed', 'true');
    const workspace = page.getByTestId('data-workspace');
    await expect(workspace).toBeVisible();
    // Neither dock in Data; no map controls while the pane is closed.
    await expect(page.getByTestId('detail-panel-bar')).toHaveCount(0);
    await expect(page.getByTestId('data-panel-bar')).toHaveCount(0);
    await expect(page.getByTestId('data-map-pane-splitter')).toHaveCount(0);
    await expect(page.getByTestId('toolbar-tools-group')).toHaveCount(0);
    await expect(page.getByTestId('display-menu-button')).toHaveCount(0);
    await expect(page.getByTestId('status-bar')).toBeVisible();

    await setMapPaneOpen(page, true);
    const splitter = page.getByTestId('data-map-pane-splitter');
    await expect(splitter).toBeVisible();
    const map = page.getByTestId('map-canvas');
    await expect(map).toBeVisible();
    // The map is the right-hand pane; the table keeps the rest, to its left.
    const mapBox = (await map.boundingBox())!;
    const workspaceBox = (await workspace.boundingBox())!;
    expect(mapBox.x).toBeGreaterThanOrEqual(workspaceBox.x + workspaceBox.width - 1);
    const stored = await page.evaluate(() => (window as unknown as Win).__E2E_TEST__.dataWorkspaceStore.getState().mapPaneWidth);
    await expect(splitter).toHaveAttribute('aria-valuenow', String(stored));
    expect(Math.round(mapBox.width)).toBe(stored);
    // Data is editable: the open pane gets the full tool set and the Display menu.
    await expect(page.getByTestId('toolbar-tools-group')).toBeVisible();
    await expect(page.getByTestId('tool-merge')).toBeVisible();
    await expect(page.getByTestId('display-menu-button')).toBeVisible();

    // The splitter is a keyboard control: ArrowLeft widens the map pane.
    await splitter.focus();
    await page.keyboard.press('ArrowLeft');
    await expect(splitter).toHaveAttribute('aria-valuenow', String(stored + 16));

    // Through all of it the map is the same MapLibre instance (never remounted).
    await setMapPaneOpen(page, false);
    await page.getByTestId('mode-survey').click();
    expect(await page.evaluate(() => {
      const w = window as unknown as Win;
      return w.__stalagmight_map__ !== undefined && w.__stalagmight_map__ === w.__mapBeforeSwitch__;
    })).toBe(true);
  });

  test('Data <-> Survey keeps the geometry and the display flags', async ({ page }) => {
    await page.evaluate(() => (window as unknown as Win).__E2E_TEST__.uiStore.getState().toggleDisplay('showGrid'));
    await page.getByTestId('mode-data').click();
    await page.getByTestId('mode-survey').click();
    const s = await page.evaluate(() => {
      const ui = (window as unknown as Win).__E2E_TEST__.uiStore.getState();
      return { geometryMode: ui.geometryMode, showGrid: ui.showGrid };
    });
    expect(s).toEqual({ geometryMode: 'measured', showGrid: true });
  });

  test('Undo/Redo and Import are offered in Data and absent in Plan', async ({ page }) => {
    await page.getByTestId('mode-data').click();
    await expect(page.getByTestId('toolbar-undo-button')).toBeVisible();
    await expect(page.getByTestId('toolbar-redo-button')).toBeVisible();
    await expect(page.getByTestId('toolbar-import-button')).toBeVisible();

    await page.getByTestId('mode-plan').click();
    await expect(page.getByTestId('mode-plan')).toHaveAttribute('aria-pressed', 'true');
    await expect(page.getByTestId('toolbar-undo-button')).toHaveCount(0);
    await expect(page.getByTestId('toolbar-redo-button')).toHaveCount(0);
    await expect(page.getByTestId('toolbar-import-button')).toHaveCount(0);
  });

  test('Plan stays read-only', async ({ page }) => {
    const selectStation = () => page.evaluate(() => {
      const e2e = (window as unknown as Win).__E2E_TEST__;
      const sectionId = e2e.projectStore.getState().project!.sections[0]!.id;
      e2e.uiStore.getState().setSelection({ type: 'station', stationRef: { section_id: sectionId, station_index: 1 } });
    });
    const activeDialog = () => page.evaluate(() => (window as unknown as Win).__E2E_TEST__.uiStore.getState().activeDialog);
    // Positive control: Delete on a selected station asks to delete it in Survey.
    await selectStation();
    await page.locator('body').press('Delete');
    await expect.poll(activeDialog).toBe('confirm-delete');
    await page.evaluate(() => (window as unknown as Win).__E2E_TEST__.uiStore.getState().closeDialog());

    await page.getByTestId('mode-plan').click();
    expect(await appMode(page)).toBe('plan');
    await expect(page.getByTestId('data-workspace')).toHaveCount(0);
    // The mutating tools are hidden, not merely disabled.
    await expect(page.getByTestId('toolbar-tools-group')).toBeVisible();
    await expect(page.getByTestId('tool-merge')).toHaveCount(0);
    // No closures here, so the forced closure view fell back to measured - Plan is
    // read-only by mode all the same (the isMapReadOnly appMode term).
    await selectStation();
    await page.locator('body').press('Delete');
    expect(await activeDialog()).toBeNull();
    await page.keyboard.press('Control+z');
    expect(await appMode(page)).toBe('plan');
  });
});
