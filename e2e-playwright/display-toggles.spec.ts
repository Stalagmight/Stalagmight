import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithSections,
  waitForMapReady,
  openDebugStatsMenu,
  getStationScreenPositions,
  waitForDrawnStations,
  waitForDrawnShots,
  readCanvasPixel,
  countCanvasInk,
  type PixelRgba,
  type SectionSeed,
} from './helpers/app';

/**
 * E2E tests for visibility toggle behavior in the Display menu.
 * Tests visibility toggling, selection blocking, selection clearing,
 * toast notifications, toggle relationships, and tool operation interactions.
 */

/**
 * Helper to create a new project with mock sections/stations
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Visibility Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
      { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0, left: 0.5, right: 0.7, up: 1.2, down: 1.5 },
    ],
  },
  {
    name: 'Visibility Side',
    direction: 'out',
    shots: [
      { azimuth_in: 270, azimuth_out: 90, length: 5.0, depth_in: -3.0, depth_out: -4.0, left: 0, right: 0, up: 0, down: 0 },
    ],
  },
];

/**
 * Helper to open the Display menu
 */
async function openDisplayMenu(page: Page): Promise<void> {
  const displayButton = page.locator('button:has-text("Display")');
  await displayButton.click();
  await expect(page.locator('[data-testid="display-toggle-showStations"]')).toBeVisible({ timeout: 2000 });
}

/**
 * Helper to get the toggle checkbox state
 */
async function isToggleOn(page: Page, toggleKey: string): Promise<boolean> {
  const toggle = page.locator(`[data-testid="display-toggle-${toggleKey}"]`);
  return (await toggle.getAttribute('aria-pressed')) === 'true';
}

/**
 * Helper to toggle a display option and wait for state change
 */
async function toggleDisplayOption(page: Page, toggleKey: string): Promise<void> {
  const toggle = page.locator(`[data-testid="display-toggle-${toggleKey}"]`);
  const currentState = await isToggleOn(page, toggleKey);
  await toggle.click();
  await expect.poll(async () => isToggleOn(page, toggleKey), { timeout: 2000 }).not.toBe(currentState);
}

/**
 * Zoom in on the seeded survey before probing painted pixels.
 *
 * The seeded survey is only a few metres across, so at the fit-to-bounds zoom the
 * whole thing is ~9 screen px wide and the station discs cover the shot lines
 * entirely - a pixel probe there cannot tell "shot hidden" from "shot behind a
 * station". `setZoom` is a jump (no animation), and the geometry helpers project
 * with the live map, so positions read after this call already match the new view.
 */
async function zoomIn(page: Page, delta = 4): Promise<void> {
  await page.evaluate((d) => {
    const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number; setZoom: (z: number) => void } }).__stalagmight_map__;
    map?.setZoom(map.getZoom() + d);
  }, delta);
}

/** Background sample from an empty canvas corner, for {@link countCanvasInk}. */
async function backgroundPixel(page: Page): Promise<PixelRgba> {
  return readCanvasPixel(page, 8, 8);
}

test.describe('Display Toggle Visibility', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Display Toggle Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('geometry mode control renders; closure-adjusted disabled without closures', async ({ page }) => {
    await openDisplayMenu(page);

    // Both segmented-control buttons render.
    await expect(page.locator('[data-testid="geometry-mode-measured"]')).toBeVisible();
    const adjusted = page.locator('[data-testid="geometry-mode-closureAdjusted"]');
    await expect(adjusted).toBeVisible();

    // Measured is the default selected mode...
    await expect(page.locator('[data-testid="geometry-mode-measured"]')).toHaveAttribute('aria-pressed', 'true');
    // ...and the adjusted option is disabled while the project has no loop closures.
    await expect(adjusted).toBeDisabled();
  });

  test('toggle stations hides and shows station markers on canvas @smoke', async ({ page }) => {
    // The stations are drawn by the custom WebGL layer, so there is no
    // `survey-stations` layer whose `visibility` can stand in for "markers are on
    // screen": hiding them is a skipped draw call. The oracle is painted ink around
    // the station. It cannot be the center pixel alone: a station disc and its
    // shots share the section color, so the center stays that color either way -
    // what changes is that the disc's ~9 px footprint collapses to the shot's line
    // width.
    await zoomIn(page);
    const [station] = await waitForDrawnStations(page, 1);
    const bg = await backgroundPixel(page);
    const inkBefore = await countCanvasInk(page, station!.x, station!.y, bg, 9);
    expect(inkBefore).toBeGreaterThan(0);

    // Open Display menu
    await openDisplayMenu(page);

    // Verify stations are visible by default
    expect(await isToggleOn(page, 'showStations')).toBe(true);

    // Verify store state matches
    const stationsVisibleBefore = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { showStations: boolean } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.showStations ?? true;
    });
    expect(stationsVisibleBefore).toBe(true);

    // Toggle stations off
    await toggleDisplayOption(page, 'showStations');

    // Verify toggle state changed
    expect(await isToggleOn(page, 'showStations')).toBe(false);

    // Verify store state changed
    const stationsVisibleAfter = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { showStations: boolean } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.showStations ?? true;
    });
    expect(stationsVisibleAfter).toBe(false);

    // The marker is really gone from the canvas: the ink around the station drops to
    // whatever its shots leave behind.
    await expect
      .poll(async () => countCanvasInk(page, station!.x, station!.y, bg, 9), { timeout: 2000 })
      .toBeLessThan(inkBefore);

    // Toggle stations back on
    await toggleDisplayOption(page, 'showStations');
    expect(await isToggleOn(page, 'showStations')).toBe(true);

    // ...and the marker is painted again, identically.
    await expect
      .poll(async () => countCanvasInk(page, station!.x, station!.y, bg, 9), { timeout: 2000 })
      .toBe(inkBefore);
  });

  test('toggle shots hides and shows shot lines on canvas @smoke', async ({ page }) => {
    // Same reasoning as the stations toggle: the shot lines are custom-rendered, so
    // the oracle is the painted pixel. A shot's midpoint is clear of the station
    // discs once zoomed in, so it goes to bare background when shots are hidden.
    await zoomIn(page);
    const [shot] = await waitForDrawnShots(page, 1);
    const bg = await backgroundPixel(page);
    const inkBefore = await countCanvasInk(page, shot!.mid.x, shot!.mid.y, bg, 5);
    expect(inkBefore).toBeGreaterThan(0);

    await openDisplayMenu(page);

    // Verify shots are visible by default
    expect(await isToggleOn(page, 'showShots')).toBe(true);

    // Toggle shots off
    await toggleDisplayOption(page, 'showShots');
    expect(await isToggleOn(page, 'showShots')).toBe(false);

    // Verify store state
    const shotsVisible = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { showShots: boolean } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.showShots ?? true;
    });
    expect(shotsVisible).toBe(false);

    // Nothing is painted along the shot any more.
    await expect
      .poll(async () => countCanvasInk(page, shot!.mid.x, shot!.mid.y, bg, 5), { timeout: 2000 })
      .toBe(0);

    // Toggle shots back on
    await toggleDisplayOption(page, 'showShots');
    expect(await isToggleOn(page, 'showShots')).toBe(true);

    // The line is painted again.
    await expect
      .poll(async () => countCanvasInk(page, shot!.mid.x, shot!.mid.y, bg, 5), { timeout: 2000 })
      .toBe(inkBefore);
  });
});

test.describe('Label Toggle Click Coverage', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Label Toggle Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  for (const toggleKey of ['showSectionNames', 'showStationLabels', 'showShotErrors'] as const) {
    test(`toggling ${toggleKey} flips aria-pressed and back`, async ({ page }) => {
      await openDisplayMenu(page);

      const initialState = await isToggleOn(page, toggleKey);
      await toggleDisplayOption(page, toggleKey);
      expect(await isToggleOn(page, toggleKey)).toBe(!initialState);

      await toggleDisplayOption(page, toggleKey);
      expect(await isToggleOn(page, toggleKey)).toBe(initialState);
    });
  }
});

/** Give the first seeded section's stations names, as a DMP import would, and
 *  rebuild the survey data from them (the mock threads names into
 *  get_project_state only). */
async function seedStationNames(page: Page, names: Array<[number, string]>): Promise<void> {
  await page.evaluate(async (names) => {
    const w = window as unknown as {
      __MOCK_SET_STATION_NAME__: (sectionId: number, stationIndex: number, name: string | null) => void;
      __E2E_TEST__: { projectStore: { getState: () => { project: { sections: Array<{ id: number }> }; refreshProjectState: () => Promise<void> } } };
    };
    const store = w.__E2E_TEST__.projectStore.getState();
    const sectionId = store.project.sections[0]!.id;
    for (const [index, name] of names) w.__MOCK_SET_STATION_NAME__(sectionId, index, name);
    await store.refreshProjectState();
  }, names);
}

async function stationLabelPair(page: Page): Promise<{ ids: boolean; names: boolean }> {
  return page.evaluate(() => {
    const s = (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { showStationLabels: boolean; showStationNames: boolean } } } })
      .__E2E_TEST__.uiStore.getState();
    return { ids: s.showStationLabels, names: s.showStationNames };
  });
}

test.describe('Station IDs / Station names', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Station Names Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('Station names is offered only once the project has names', async ({ page }) => {
    await openDisplayMenu(page);
    const names = page.getByTestId('display-toggle-showStationNames');
    await expect(page.getByTestId('display-toggle-showStationLabels')).toHaveText(/Station IDs/);
    await expect(names).toHaveText(/Station names/);
    await expect(names).toBeDisabled();
    await expect(names).toHaveAttribute('title', /no station names/);

    await seedStationNames(page, [[0, '7'], [2, '9']]);
    await expect(names).toBeEnabled();
    await expect(names).toHaveAttribute('title', /instead of IDs/);
  });

  test('the two pills are one choice, and both are saved', async ({ page }) => {
    const saves: string[] = [];
    page.on('console', (msg) => {
      const m = /save_display_setting: (showStation(?:Labels|Names))=(true|false)/.exec(msg.text());
      if (m) saves.push(`${m[1]}=${m[2]}`);
    });
    await seedStationNames(page, [[0, '7'], [2, '9']]);
    await openDisplayMenu(page);

    await toggleDisplayOption(page, 'showStationLabels');
    expect(await stationLabelPair(page)).toEqual({ ids: true, names: false });

    // Names on turns IDs off.
    await toggleDisplayOption(page, 'showStationNames');
    expect(await isToggleOn(page, 'showStationLabels')).toBe(false);
    expect(await stationLabelPair(page)).toEqual({ ids: false, names: true });

    // IDs on turns names off.
    await toggleDisplayOption(page, 'showStationLabels');
    expect(await isToggleOn(page, 'showStationNames')).toBe(false);
    expect(await stationLabelPair(page)).toEqual({ ids: true, names: false });

    // Turning one off leaves both off.
    await toggleDisplayOption(page, 'showStationLabels');
    expect(await stationLabelPair(page)).toEqual({ ids: false, names: false });

    // Every toggle writes the pair as it now stands, partner first, so a stored
    // pair can never be left reading both-on.
    await expect
      .poll(() => saves, { timeout: 2000 })
      .toEqual([
        'showStationNames=false',
        'showStationLabels=true',
        'showStationLabels=false',
        'showStationNames=true',
        'showStationNames=false',
        'showStationLabels=true',
        'showStationNames=false',
        'showStationLabels=false',
      ]);
  });

  test('the L key turns Station IDs on and Station names off', async ({ page }) => {
    await seedStationNames(page, [[0, '7']]);
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showStationNames');
    await page.keyboard.press('Escape');
    await page.locator('canvas').first().click({ position: { x: 5, y: 5 } });
    await page.keyboard.press('l');
    await expect.poll(() => stationLabelPair(page), { timeout: 2000 }).toEqual({ ids: true, names: false });
  });
});

test.describe('Errors labels toggle', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Errors Toggle Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('is saved with the project like the other label toggles', async ({ page }) => {
    const saves: string[] = [];
    page.on('console', (msg) => {
      const m = /\[Mock IPC\] save_display_setting: (\S+)/.exec(msg.text());
      if (m) saves.push(m[1]!);
    });
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showShotErrors');
    await expect.poll(() => saves).toContain('showShotErrors=true');
  });
});

test.describe('Selection Blocking When Hidden', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Selection Blocking Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('cannot select hidden stations - selection blocked', async ({ page }) => {
    // Wait for the stations to be drawn, then take their positions.
    // waitForDrawnStations guarantees at least one, so no empty-data skip is needed.
    const [targetStation] = await waitForDrawnStations(page, 1);

    // Hide stations
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showStations');
    await page.keyboard.press('Escape');

    // Switch to select tool
    await page.keyboard.press('s');

    // Get canvas position
    const canvas = page.locator('canvas').first();
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();

    // Click on the known station location
    await page.mouse.click(box!.x + targetStation.x, box!.y + targetStation.y);

    // Verify selection is blocked (no station selected)
    await expect.poll(async () => {
      const selection = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: { type: string } | null } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.selection ?? null;
      });
      return selection?.type ?? null;
    }, { timeout: 2000 }).not.toBe('station');
  });

  test('cannot select hidden shots - selection blocked', async ({ page }) => {
    // Wait for the shots to be drawn.
    await waitForDrawnShots(page, 1);

    // Hide shots
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showShots');
    await page.keyboard.press('Escape');

    // Switch to select tool
    await page.keyboard.press('s');

    // Get canvas and click in center
    const canvas = page.locator('canvas').first();
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();

    // Click on canvas where shots might be
    await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);

    // Verify no shot selection occurred
    await expect.poll(async () => {
      const selection = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: { type: string } | null } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.selection ?? null;
      });
      return selection?.type ?? null;
    }, { timeout: 2000 }).not.toBe('shot');
  });

  test('no hover effect on hidden stations', async ({ page }) => {
    // Wait for the stations to be drawn, then take their positions.
    // waitForDrawnStations guarantees at least one, so no empty-data skip is needed.
    const [targetStation] = await waitForDrawnStations(page, 1);

    // Hide stations
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showStations');
    await page.keyboard.press('Escape');

    // Get canvas position
    const canvas = page.locator('canvas').first();
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();

    // Move mouse to known station position
    await page.mouse.move(box!.x + targetStation.x, box!.y + targetStation.y);

    // Verify no hovered station
    await expect.poll(async () => {
      const hoveredStation = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { hoveredStation: unknown } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.hoveredStation ?? null;
      });
      return hoveredStation;
    }, { timeout: 1000 }).toBeNull();
  });
});

test.describe('Selection Clearing on Toggle', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Selection Clearing Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('station selection cleared when stations hidden', async ({ page }) => {
    // Add a station selection via store
    await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { addToSelection: (s: unknown) => void } } } }).__E2E_TEST__;
      stores?.uiStore?.getState()?.addToSelection({
        type: 'station',
        stationRef: { section_id: 1, station_index: 0 },
      });
    });

    // Verify station is selected
    const selectionBefore = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: { type: string } | null } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.selection ?? null;
    });
    expect(selectionBefore?.type).toBe('station');

    // Hide stations
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showStations');

    // Verify station selection is cleared
    await expect.poll(async () => {
      const selection = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: { type: string } | null } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.selection ?? null;
      });
      return selection?.type ?? null;
    }, { timeout: 2000 }).not.toBe('station');
  });

  test('shot selection cleared when shots hidden', async ({ page }) => {
    // Add a shot selection via store
    await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { addToSelection: (s: unknown) => void } } } }).__E2E_TEST__;
      stores?.uiStore?.getState()?.addToSelection({
        type: 'shot',
        sectionId: 1,
        shotId: 0,
      });
    });

    // Verify shot is selected
    const selectionBefore = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: { type: string } | null } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.selection ?? null;
    });
    expect(selectionBefore?.type).toBe('shot');

    // Hide shots
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showShots');

    // Verify shot selection is cleared
    await expect.poll(async () => {
      const selection = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { selection: { type: string } | null } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.selection ?? null;
      });
      return selection?.type ?? null;
    }, { timeout: 2000 }).not.toBe('shot');
  });

  test('mixed selection only clears hidden type', async ({ page }) => {
    // Add both station and shot selections
    await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { addToSelection: (s: unknown) => void } } } }).__E2E_TEST__;
      const state = stores?.uiStore?.getState();
      state?.addToSelection({
        type: 'station',
        stationRef: { section_id: 1, station_index: 0 },
      });
      state?.addToSelection({
        type: 'shot',
        sectionId: 1,
        shotId: 0,
      });
    });

    // Verify both are selected
    const multiBefore = await page.evaluate(() => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { multiSelection: Array<{ type: string }> } } } }).__E2E_TEST__;
      return stores?.uiStore?.getState()?.multiSelection ?? [];
    });
    expect(multiBefore.length).toBe(2);
    expect(multiBefore.some((s) => s.type === 'station')).toBe(true);
    expect(multiBefore.some((s) => s.type === 'shot')).toBe(true);

    // Hide stations
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showStations');

    // Verify only station is cleared, shot remains
    await expect.poll(async () => {
      const multi = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { multiSelection: Array<{ type: string }> } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.multiSelection ?? [];
      });
      return {
        hasStation: multi.some((s) => s.type === 'station'),
        hasShot: multi.some((s) => s.type === 'shot'),
      };
    }, { timeout: 2000 }).toEqual({ hasStation: false, hasShot: true });
  });
});

test.describe('Toast Notifications', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Toast Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
  });

  /** Push a toast through the store, exactly as a feature call site would. */
  const showToast = (page: Page, message: string, key: string, kind?: string) =>
    page.evaluate(([m, k, n]) => {
      const stores = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: {
            getState: () => { showToast: (msg: string, key: string, kind?: string) => void };
          };
        };
      }).__E2E_TEST__;
      stores?.uiStore?.getState()?.showToast(m as string, k as string, n as string | undefined);
    }, [message, key, kind] as const);

  test('same key refreshes the existing toast instead of stacking a duplicate', async ({ page }) => {
    // Both pushes go through in ONE round trip. The refresh only applies inside
    // the 1 s burst window (`TOAST_DEDUP_WINDOW_MS`), and DOM assertions between
    // the two pushes spent unbounded wall-clock time inside it - on a loaded
    // runner the second push fell outside and legitimately stacked, failing the
    // count. The intermediate state is read from the store in the same tick.
    const afterFirst = await page.evaluate(() => {
      const ui = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: {
            getState: () => {
              showToast: (msg: string, key: string) => void;
              toasts: Array<{ message: string }>;
            };
          };
        };
      }).__E2E_TEST__?.uiStore;
      ui?.getState().showToast('First toast', 'test-key');
      const messages = (ui?.getState().toasts ?? []).map((t) => t.message);
      // The old behaviour swallowed this outright; the stack shows the newer
      // wording under the same key, still as a single toast.
      ui?.getState().showToast('Second toast', 'test-key');
      return messages;
    });
    expect(afterFirst).toEqual(['First toast']);

    await expect(page.getByTestId('toast')).toHaveCount(1);
    await expect(page.getByTestId('toast')).toContainText('Second toast');
  });

  test('different keys stack, newest last', async ({ page }) => {
    await showToast(page, 'First toast', 'key-1');
    await showToast(page, 'Different key toast', 'key-2');

    await expect(page.getByTestId('toast')).toHaveCount(2);
    await expect(page.getByTestId('toast').nth(0)).toContainText('First toast');
    await expect(page.getByTestId('toast').nth(1)).toContainText('Different key toast');
  });

  test('the stack is capped at five live toasts in a tall window', async ({ page }) => {
    // A one-line toast is ~36px, so 20% of 1400 (280px) has room for the ceiling
    // of five; the cap that bites here is the ceiling, not the budget.
    await page.setViewportSize({ width: 1280, height: 1400 });
    for (let i = 0; i < 8; i += 1) await showToast(page, `Toast ${i}`, `key-${i}`);

    // The three oldest are evicted (they animate out, then go).
    await expect(page.locator('[data-testid="toast"]:not([data-toast-leaving])')).toHaveCount(5);
    await expect(page.getByTestId('toast')).toHaveCount(5);
    await expect(page.getByTestId('toast').nth(0)).toContainText('Toast 3');
  });

  test('a short window fits fewer toasts, but never fewer than two', async ({ page }) => {
    // 20% of 300px is 60px - not even room for two toasts. The floor still wins,
    // so a follow-up message is never invisible.
    await page.setViewportSize({ width: 1280, height: 300 });
    for (let i = 0; i < 5; i += 1) await showToast(page, `Toast ${i}`, `key-${i}`);

    await expect(page.getByTestId('toast')).toHaveCount(2);
    await expect(page.getByTestId('toast').nth(1)).toContainText('Toast 4');
  });

  test('shrinking the window evicts the toasts that no longer fit', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1400 });
    for (let i = 0; i < 5; i += 1) await showToast(page, `Toast ${i}`, `key-${i}`);
    await expect(page.getByTestId('toast')).toHaveCount(5);

    await page.setViewportSize({ width: 1280, height: 300 });

    // Evicted, not merely hidden - nothing lingers off-screen with a live timer.
    await expect(page.getByTestId('toast')).toHaveCount(2);
    await expect(page.getByTestId('toast').nth(1)).toContainText('Toast 4');
  });

  test('a new toast enters from below and pushes the stack up', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 1400 });
    await showToast(page, 'First toast', 'key-1');

    const first = page.locator('[data-toast-key="key-1"]');
    // Settled on the anchor: the newest toast sits at offset zero.
    await expect(first).toHaveAttribute('style', /translateY\(0px\)/);

    await showToast(page, 'Second toast', 'key-2');

    // Pushed up by the height of the newcomer plus the gap, and the newcomer
    // takes the anchor. The exact px depends on the rendered height, so assert
    // the sign and the ordering rather than a magic number.
    await expect(first).toHaveAttribute('style', /translateY\(-\d+px\)/);
    await expect(page.locator('[data-toast-key="key-2"]')).toHaveAttribute(
      'style',
      /translateY\(0px\)/,
    );
  });

  test('each kind renders with its own accent', async ({ page }) => {
    await showToast(page, 'It worked', 'k-ok', 'success');
    await showToast(page, 'It failed', 'k-bad', 'error');

    await expect(page.locator('[data-toast-kind="success"]')).toContainText('It worked');
    await expect(page.locator('[data-toast-kind="error"]')).toContainText('It failed');
  });

  test('a toast can be dismissed on its own', async ({ page }) => {
    await showToast(page, 'Keep me', 'k-keep');
    await showToast(page, 'Drop me', 'k-drop');
    await expect(page.getByTestId('toast')).toHaveCount(2);

    await page.locator('[data-toast-key="k-drop"] button').click();

    await expect(page.getByTestId('toast')).toHaveCount(1);
    await expect(page.getByTestId('toast')).toContainText('Keep me');
  });
});

test.describe('Toggle Relationship Tests', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Toggle Relationship Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('Reference Shots toggle is disabled when Shots hidden', async ({ page }) => {
    await openDisplayMenu(page);

    // Reference Shots toggle should be enabled initially (Shots is on)
    const referenceShotsToggle = page.locator('[data-testid="display-toggle-showReferenceShots"]');
    await expect(referenceShotsToggle).toBeVisible();
    await expect(referenceShotsToggle).not.toBeDisabled();

    // Hide Shots
    await toggleDisplayOption(page, 'showShots');

    // Reference Shots toggle should now be disabled
    await expect(referenceShotsToggle).toBeDisabled();
  });

  test('Shot Data toggle is disabled when Shots hidden', async ({ page }) => {
    await openDisplayMenu(page);

    // Shot Data toggle should be enabled initially (Shots is on)
    const shotDataToggle = page.locator('[data-testid="display-toggle-showShotData"]');
    await expect(shotDataToggle).toBeVisible();
    await expect(shotDataToggle).not.toBeDisabled();

    // Hide Shots
    await toggleDisplayOption(page, 'showShots');

    // Shot Data toggle should now be disabled
    await expect(shotDataToggle).toBeDisabled();
  });

  test('dependent toggles re-enable when Shots shown', async ({ page }) => {
    await openDisplayMenu(page);

    // Hide shots
    await toggleDisplayOption(page, 'showShots');

    // Verify dependent toggles are disabled
    const referenceShotsToggle = page.locator('[data-testid="display-toggle-showReferenceShots"]');
    const shotDataToggle = page.locator('[data-testid="display-toggle-showShotData"]');
    await expect(referenceShotsToggle).toBeDisabled();
    await expect(shotDataToggle).toBeDisabled();

    // Show shots again
    await toggleDisplayOption(page, 'showShots');

    // Dependent toggles should be re-enabled
    await expect(referenceShotsToggle).not.toBeDisabled();
    await expect(shotDataToggle).not.toBeDisabled();
  });

  test('dependent toggle state is preserved through parent toggle cycle', async ({ page }) => {
    await openDisplayMenu(page);

    // Get initial showReferenceShots state (should be true by default)
    const initialState = await isToggleOn(page, 'showReferenceShots');
    expect(initialState).toBe(true);

    // Turn off Reference Shots
    await toggleDisplayOption(page, 'showReferenceShots');
    expect(await isToggleOn(page, 'showReferenceShots')).toBe(false);

    // Toggle parent (showShots) off then on
    await toggleDisplayOption(page, 'showShots');
    await toggleDisplayOption(page, 'showShots');

    // Reference Shots state should be preserved (still off)
    expect(await isToggleOn(page, 'showReferenceShots')).toBe(false);
  });
});

// The Draw tool was removed in #71/#74 (the draw shortcut is unbound and
// uiStore.setDrawFromStation no longer exists), so its coverage above is gone.
// Merge still requires visible stations to select targets, so that behavior
// is kept as a live test below (Refs #275).
test.describe('Merge Tool Interaction', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Tool Operation Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('merge operation canceled when stations hidden mid-merge', async ({ page }) => {
    // Unlike draw, merge requires visible stations to select targets
    const [station] = await getStationScreenPositions(page);

    // Switch to merge tool
    await page.keyboard.press('m');

    // Set a merge station A via store (simulating first station selected)
    await page.evaluate((ref) => {
      const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { setMergeStationA: (ref: unknown) => void } } } }).__E2E_TEST__;
      stores?.uiStore?.getState()?.setMergeStationA(ref);
    }, { section_id: station.sectionId, station_index: station.stationIndex });

    // Verify merge station A is set
    await expect.poll(async () => {
      const mergeA = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { mergeStationA: unknown } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.mergeStationA ?? null;
      });
      return mergeA;
    }, { timeout: 1000 }).not.toBeNull();

    // Hide stations
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showStations');

    // Merge should be canceled (mergeStationA should be null)
    await expect.poll(async () => {
      const mergeA = await page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { mergeStationA: unknown } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.mergeStationA ?? null;
      });
      return mergeA;
    }, { timeout: 2000 }).toBeNull();
  });
});

test.describe('Display Menu State Persistence', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'State Persistence Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForDrawnStations(page, 1);
  });

  test('toggle states persist when menu is closed and reopened', async ({ page }) => {
    // Open menu and change some toggles
    await openDisplayMenu(page);

    // Toggle stations off
    await toggleDisplayOption(page, 'showStations');
    expect(await isToggleOn(page, 'showStations')).toBe(false);

    // Toggle grid on
    await toggleDisplayOption(page, 'showGrid');
    expect(await isToggleOn(page, 'showGrid')).toBe(true);

    // Close menu
    await page.keyboard.press('Escape');

    // Verify menu is closed
    await expect(page.locator('[data-testid="display-toggle-showStations"]')).not.toBeVisible();

    // Reopen menu
    await openDisplayMenu(page);

    // Verify states are preserved
    expect(await isToggleOn(page, 'showStations')).toBe(false);
    expect(await isToggleOn(page, 'showGrid')).toBe(true);
  });

  test('all display toggles have correct default states', async ({ page }) => {
    await openDisplayMenu(page);

    // Check default values from DISPLAY_DEFAULTS
    expect(await isToggleOn(page, 'showStations')).toBe(true);
    expect(await isToggleOn(page, 'showShots')).toBe(true);
    expect(await isToggleOn(page, 'showSectionNames')).toBe(true);
    expect(await isToggleOn(page, 'showStationLabels')).toBe(false);
    expect(await isToggleOn(page, 'showStationNames')).toBe(false);
    expect(await isToggleOn(page, 'showShotData')).toBe(false);
    expect(await isToggleOn(page, 'showShotErrors')).toBe(false);
    expect(await isToggleOn(page, 'showReferenceShots')).toBe(true);
    expect(await isToggleOn(page, 'showZeroDatum')).toBe(true);
    expect(await isToggleOn(page, 'showGrid')).toBe(false);
    expect(await isToggleOn(page, 'showSatellite')).toBe(true);
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(false);
  });
});

test.describe('Debug submenu (Dev/Memory/Map/Zoom Stats)', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Dev Stats Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
  });

  // The memory probe used to be a hidden Ctrl+Shift+M toggle, which collided
  // with rebound global shortcuts. It is now revealed by the hamburger menu's
  // Debug ▸ "Memory Stats" toggle instead (showMemoryStats), with no keyboard
  // side effects.
  test('Memory Stats toggle reveals and hides the memory probe panel', async ({ page }) => {
    const probe = page.locator('[data-testid="memory-probe-panel"]');

    // Hidden by default.
    await expect(probe).toHaveCount(0);

    // Enabling "Memory Stats" from the hamburger Debug submenu reveals the probe.
    await openDebugStatsMenu(page, 'display-toggle-showMemoryStats');
    await expect(probe).toBeVisible({ timeout: 2000 });

    // Disabling it removes the probe again.
    await openDebugStatsMenu(page, 'display-toggle-showMemoryStats');
    await expect(probe).toHaveCount(0);
  });

  // Guards the regression directly: Ctrl+Shift+M must no longer do anything
  // (no preventDefault, no probe), so it can't swallow a rebound shortcut.
  test('Ctrl+Shift+M no longer toggles the probe', async ({ page }) => {
    const probe = page.locator('[data-testid="memory-probe-panel"]');
    await expect(probe).toHaveCount(0);
    await page.keyboard.press('Control+Shift+M');
    // Give any (now-removed) handler a chance to react before asserting absence.
    await expect(probe).toHaveCount(0);
  });

  // The four Debug submenu items are independent flags: each is off by
  // default and toggling one must not affect the others.
  test('Dev Stats, Map Stats, and Zoom Stats toggle independently and are off by default', async ({ page }) => {
    const devPanel = page.locator('[data-testid="map-dev-stats"]');
    const mapPanel = page.locator('[data-testid="map-stats-panel"]');
    const zoomPanel = page.locator('[data-testid="map-zoom-stats"]');
    const probe = page.locator('[data-testid="memory-probe-panel"]');

    await expect(devPanel).toHaveCount(0);
    await expect(mapPanel).toHaveCount(0);
    await expect(zoomPanel).toHaveCount(0);
    await expect(probe).toHaveCount(0);
    // The wheel-chain diagnostics used to be a separate overlay left on
    // screen at all times in dev builds - it must not appear on its own.
    await expect(page.getByTestId('zoom-stats-wheel-chain')).toHaveCount(0);

    // Enable Map Stats only - the others must stay off.
    await openDebugStatsMenu(page, 'display-toggle-showMapStats');
    await expect(mapPanel).toBeVisible({ timeout: 2000 });
    await expect(devPanel).toHaveCount(0);
    await expect(zoomPanel).toHaveCount(0);
    await expect(probe).toHaveCount(0);

    // Enable Zoom Stats too - both panels stack without hiding each other.
    // Its wheel-chain diagnostics (folded in from the old always-on dev
    // overlay) must render only now, inside the panel - not before.
    await openDebugStatsMenu(page, 'display-toggle-showZoomStats');
    await expect(zoomPanel).toBeVisible({ timeout: 2000 });
    await expect(page.getByTestId('zoom-stats-wheel-chain')).toBeVisible();
    await expect(mapPanel).toBeVisible();
    await expect(devPanel).toHaveCount(0);

    // Disable Map Stats - Zoom Stats stays up.
    await openDebugStatsMenu(page, 'display-toggle-showMapStats');
    await expect(mapPanel).toHaveCount(0);
    await expect(zoomPanel).toBeVisible();

    // Enable Dev Stats - independent of the other two.
    await openDebugStatsMenu(page, 'display-toggle-showDevStats');
    await expect(devPanel).toBeVisible({ timeout: 2000 });
  });
});
