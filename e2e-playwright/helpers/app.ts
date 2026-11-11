import { expect, type Locator, type Page } from '@playwright/test';
import { setupTauriMocks } from '../mocks/tauri-ipc';

export interface SectionSeed {
  name: string;
  direction?: 'in' | 'out';
  color?: string;
  surveyor?: string;
  date?: string;
  shots: Array<{
    azimuth_in: number;
    azimuth_out: number;
    length: number;
    depth_in: number;
    depth_out: number;
    pitch_in?: number | null;
    pitch_out?: number | null;
    left?: number | null;
    right?: number | null;
    up?: number | null;
    down?: number | null;
    comment?: string | null;
    kind?: string;
  }>;
}

export async function setupMockedPage(page: Page): Promise<void> {
  await setupTauriMocks(page);
  await page.goto('/');
  await waitForAppReady(page);
}

/**
 * Turn on `map.controls.show_navigation_tools` (#409) for the whole page, so the
 * Pan/Zoom toolbar group and its P/Z shortcuts exist.
 *
 * Registers `window.__E2E_SEED_SHOW_NAVIGATION_TOOLS__` via its own
 * `page.addInitScript`, which the mock reads when it builds its config (see
 * tauri-ipc.ts). Init scripts run in registration order before any app JS on every
 * navigation, so this MUST be called BEFORE `setupMockedPage` - the toolbar reads
 * the flag from the app's first `get_config()` at boot.
 */
export async function seedNavigationTools(page: Page): Promise<void> {
  await page.addInitScript(() => {
    (window as unknown as Record<string, unknown>).__E2E_SEED_SHOW_NAVIGATION_TOOLS__ = true;
  });
}

export async function waitForAppReady(page: Page): Promise<void> {
  await page.waitForFunction(() => {
    return Boolean(
      document.querySelector('[data-testid="global-view"]') ||
        document.querySelector('[data-testid="single-system-view"]') ||
        document.querySelector('[data-testid="map-canvas"]')
    );
  });
}

export async function waitForMapReady(page: Page): Promise<void> {
  await page.waitForSelector('[data-testid="map-canvas"][data-map-loaded="true"]', {
    timeout: 20000,
  });
  await page.waitForFunction(() => {
    const map = (window as { __stalagmight_map__?: { isStyleLoaded?: () => boolean } }).__stalagmight_map__;
    return Boolean(map && map.isStyleLoaded && map.isStyleLoaded());
  });
}

export async function waitForMapIdle(page: Page): Promise<void> {
  await page.waitForFunction(() => {
    const map = (window as { __stalagmight_map__?: { isMoving?: () => boolean } }).__stalagmight_map__;
    return Boolean(map && map.isMoving && !map.isMoving());
  });
}

export async function waitForActiveTool(page: Page, tool: string): Promise<void> {
  const mapCanvas = page.getByTestId('map-canvas');
  await expect(mapCanvas).toHaveAttribute('data-active-tool', tool);
}

/** A map center in the map's own coordinates. */
export interface MapCenter {
  lng: number;
  lat: number;
}

/**
 * The live map center, read off the `__stalagmight_map__` E2E handle.
 *
 * Non-nullable on purpose: every caller reads the camera AFTER `waitForMapReady`
 * / `createProject`, so a missing handle is a broken test rather than a state
 * worth asserting on. It throws inside the page, which surfaces as a clear
 * "map handle is not available" error at the call site instead of a `null`
 * that has to be `!`-asserted six lines later.
 */
export async function getMapCenter(page: Page): Promise<MapCenter> {
  return page.evaluate(() => {
    const map = (window as unknown as {
      __stalagmight_map__?: { getCenter: () => { lng: number; lat: number } };
    }).__stalagmight_map__;
    if (!map) {
      throw new Error('window.__stalagmight_map__ is not available (map not mounted yet?)');
    }
    const center = map.getCenter();
    return { lng: center.lng, lat: center.lat };
  });
}

/**
 * The UI store's current selection, or null when nothing is selected (and when
 * the E2E store handle is not exposed at all, which is the same "nothing to
 * compare" answer for the before/after assertions this feeds).
 */
export async function getSelection(page: Page): Promise<unknown> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { selection: unknown } } };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().selection ?? null;
  });
}

/**
 * Pick `value` from a portal-based `Select` component (src/components/common/Select.tsx).
 * `Select` isn't a native `<select>` so `.selectOption()` doesn't work on it: this clicks
 * the trigger, then the option rendered at
 * `data-testid="${testId}-option-${encodeURIComponent(value)}"` - the exact convention
 * `Select.tsx` renders and the agent-harness `select_option` tool already relies on.
 * The option list is portaled to `document.body` (escaping any dialog/table it's
 * opened from), so both locators are page-scoped rather than taking a root - the
 * test-ID contract already guarantees `testId` is unique across the page.
 */
export async function selectDropdownOption(page: Page, testId: string, value: string): Promise<void> {
  await selectDropdownOptionAt(page.locator(`[data-testid="${testId}"]`), value);
}

/**
 * Same as {@link selectDropdownOption}, but for a trigger already narrowed to one
 * element via a dynamic/index-based locator (e.g. `qualifiedIndexTestIds(page,
 * 'gas-cylinder-role').last()`) rather than a known static testid.
 */
export async function selectDropdownOptionAt(trigger: Locator, value: string): Promise<void> {
  await trigger.click();
  const testId = await trigger.getAttribute('data-testid');
  if (!testId) throw new Error('selectDropdownOptionAt: trigger has no data-testid');
  await trigger.page().locator(`[data-testid="${testId}-option-${encodeURIComponent(value)}"]`).click();
}

export async function waitForDialogClosed(page: Page, testId: string): Promise<void> {
  const dialog = page.getByTestId(testId);
  await expect(dialog).toBeHidden();
}

export async function waitForNoDialogs(page: Page): Promise<void> {
  const visibleDialogs = page.locator('[role="dialog"]:visible, [role="alertdialog"]:visible');
  await expect(visibleDialogs).toHaveCount(0);
}

/**
 * Wait until the quit-after-work gate has no quit pending (no waiting prompt, no
 * watcher, no settle step scheduled), so nothing there can still re-issue a quit.
 * A spec asserting that NO quit happened waits for this instead of sleeping past
 * the settle delay.
 */
export async function waitForNoPendingQuit(page: Page): Promise<void> {
  await expect.poll(() => page.evaluate(() => {
    const pending = (window as unknown as { __E2E_TEST__?: { quitAfterWorkPending?: () => boolean } })
      .__E2E_TEST__?.quitAfterWorkPending;
    if (!pending) throw new Error('quitAfterWorkPending is not exposed');
    return pending();
  })).toBe(false);
}

export async function createSectionViaStore(page: Page, section: SectionSeed): Promise<void> {
  await page.evaluate(async (data) => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { createSection: (payload: unknown) => Promise<unknown> } };
      };
    }).__E2E_TEST__;

    if (!e2e) {
      throw new Error('E2E stores not available on window');
    }

    await e2e.projectStore.getState().createSection({
      name: data.name,
      surveyor: data.surveyor ?? 'Test Surveyor',
      date: data.date ?? new Date().toISOString().split('T')[0],
      color: data.color ?? '#3b82f6',
      direction: data.direction ?? 'in',
      shots: data.shots,
    });
  }, section);
}

/**
 * Delete shot `shotIndex` of section 1 directly via the store (#310) - the same
 * `delete_shot` command the UI's confirm/shift-geometry paths issue, without the
 * dialog chrome a repro doesn't need. Renumbers every later station down by one in
 * the mock, so it can drive a live anchor (e.g. a way-point pin) out from under it.
 */
export async function deleteShotAt(page: Page, shotIndex: number): Promise<void> {
  await page.evaluate(async (idx) => {
    const e2e = (
      window as unknown as {
        __E2E_TEST__: {
          projectStore: {
            getState: () => {
              project: { sections: Array<{ id: number; shots: Array<{ id: number }> }> };
              deleteShot: (sectionId: number, shotId: number) => Promise<unknown>;
            };
          };
        };
      }
    ).__E2E_TEST__;
    const state = e2e.projectStore.getState();
    const sec = state.project.sections[0];
    const shot = sec.shots[idx];
    if (!shot) throw new Error(`no shot at index ${idx}`);
    await state.deleteShot(sec.id, shot.id);
  }, shotIndex);
}

export async function seedProjectWithSections(page: Page, sections: SectionSeed[]): Promise<void> {
  for (const section of sections) {
    await createSectionViaStore(page, section);
  }

  await page.waitForFunction((expectedCount) => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: { projectStore: { getState: () => { project: { sections: unknown[] } | null } } };
    }).__E2E_TEST__;
    return (e2e?.projectStore.getState().project?.sections?.length ?? 0) >= expectedCount;
  }, sections.length);
}

export async function clickFirstVisible(locators: Locator[]): Promise<void> {
  for (const locator of locators) {
    if (await locator.isVisible()) {
      await locator.click();
      return;
    }
  }
  throw new Error('No matching locator was visible');
}

/** Open the hamburger application menu (same trigger in global and in-project views). */
export async function openAppMenu(page: Page): Promise<void> {
  const trigger = page.getByTestId('app-menu-trigger');
  await expect(trigger).toBeVisible({ timeout: 5000 });
  if ((await trigger.getAttribute('data-dropdown-open')) === null) {
    await trigger.click();
  }
  await expect(page.getByTestId('app-menu')).toBeVisible({ timeout: 5000 });
}

/** Open the Settings dialog via the hamburger menu (global view). */
export async function openSettingsFromMenu(page: Page): Promise<void> {
  await openAppMenu(page);
  await page.getByTestId('settings-button').click();
  await expect(page.getByTestId('settings-dialog')).toBeVisible({ timeout: 5000 });
}

/**
 * Open the Debug Logs dialog via the hamburger menu. Debug Logs is the first row
 * of the "Debug" submenu in both hamburgers (home screen and in-project).
 */
export async function openDebugFromMenu(page: Page): Promise<void> {
  await openDebugStatsMenu(page, 'global-debug-button');
}

/**
 * Open the hamburger Debug submenu (Debug Logs, plus the Dev/Memory/Map/Zoom
 * Stats toggles in-project) and click a specific item in it.
 *
 * Its toggle rows are checkable, so the submenu stays open after a click
 * (see AppMenu's activateLeaf) - clicking the "Debug" parent row again would
 * then CLOSE it instead of opening it, so this only clicks the parent row
 * when the submenu isn't already open.
 */
export async function openDebugStatsMenu(page: Page, itemTestId: string): Promise<void> {
  await openAppMenu(page);
  const submenu = page.getByTestId('app-menu-submenu-debug-menu');
  if ((await submenu.count()) === 0) {
    await page.getByTestId('debug-menu').click();
  }
  await expect(submenu).toBeVisible({ timeout: 5000 });
  await page.getByTestId(itemTestId).click();
}

/** Switch the recent-projects view (global view) between list and map. */
export async function switchRecentView(page: Page, view: 'list' | 'map'): Promise<void> {
  await page.getByTestId(`globalview-view-toggle-${view}`).click();
}

/**
 * Close the open project via the in-project hamburger ("Close Project"), and wait
 * for the start screen. Use when no unsaved-changes prompt is expected; for the
 * dirty path, open the menu and click `back-button` directly so you can assert
 * the unsaved-changes dialog.
 */
export async function closeProjectViaMenu(page: Page): Promise<void> {
  await openAppMenu(page);
  await page.getByTestId('back-button').click();
  // A dirty project blocks the close on an unsaved-changes dialog - discard to
  // proceed so the helper works whether or not there are unsaved edits.
  const discard = page.locator('button:has-text("Discard")');
  if (await discard.isVisible({ timeout: 1000 }).catch(() => false)) {
    await discard.click();
  }
  await expect(page.getByTestId('global-view')).toBeVisible({ timeout: 8000 });
}

export async function openNewProjectDialog(page: Page): Promise<void> {
  const newProjectButton = page
    .locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]')
    .first();
  await expect(newProjectButton).toBeVisible();
  await newProjectButton.click();
  await expect(page.getByTestId('new-project-dialog')).toBeVisible();
}

export async function createProject(page: Page, name: string): Promise<void> {
  await openNewProjectDialog(page);
  await page.getByTestId('new-project-name-input').fill(name);
  await page.getByTestId('new-project-create-button').click();
  await page.getByTestId('single-system-view').waitFor({ state: 'visible', timeout: 10000 });
  await waitForMapReady(page);
}

export async function openDmpSelectionDialog(page: Page): Promise<void> {
  await page.keyboard.press('Control+i');
  await expect(page.getByTestId('dmp-selection-dialog')).toBeVisible({ timeout: 10000 });
}

export async function ensureDmpNewImportDialogVisible(page: Page): Promise<void> {
  await page.waitForFunction(() => {
    const wizard = document.querySelector('[data-testid="dmp-new-import-dialog"]');
    const placement = document.querySelector('[data-testid="placement-mode-indicator"]');
    return Boolean(wizard || placement);
  }, undefined, { timeout: 15000 });

  const wizard = page.getByTestId('dmp-new-import-dialog');
  if (await wizard.isVisible()) {
    return;
  }

  const placementIndicator = page.getByTestId('placement-mode-indicator');
  await expect(placementIndicator).toBeVisible({ timeout: 10000 });
  await page.keyboard.press('Escape');
  await expect(wizard).toBeVisible({ timeout: 10000 });
}

export async function proceedToDmpImportWizard(page: Page): Promise<void> {
  const continueButton = page.getByTestId('continue-button');
  await expect(continueButton).toBeEnabled();
  await continueButton.click();

  const zeroDatumSelection = page.getByTestId('zero-datum-selection-dialog');
  const dmpWizard = page.getByTestId('dmp-new-import-dialog');
  const placementIndicator = page.getByTestId('placement-mode-indicator');

  await page.waitForFunction(() => {
    const dialogs = [
      document.querySelector('[data-testid="zero-datum-selection-dialog"]'),
      document.querySelector('[data-testid="dmp-new-import-dialog"]'),
      document.querySelector('[data-testid="placement-mode-indicator"]'),
    ];
    return dialogs.some(Boolean);
  }, undefined, { timeout: 10000 });

  if (await zeroDatumSelection.isVisible()) {
    await zeroDatumSelection.getByTestId('zero-datum-selection-skip').click();
  }

  if (!(await dmpWizard.isVisible()) && !(await placementIndicator.isVisible())) {
    await page.waitForFunction(() => {
      return Boolean(
        document.querySelector('[data-testid="dmp-new-import-dialog"]') ||
          document.querySelector('[data-testid="placement-mode-indicator"]')
      );
    }, undefined, { timeout: 10000 });
  }

  await ensureDmpNewImportDialogVisible(page);
}

// Backward-compatible alias used by older specs.
export async function proceedToDmpSectionConfig(page: Page): Promise<void> {
  await proceedToDmpImportWizard(page);
}

export async function placeAllSections(page: Page): Promise<void> {
  await ensureDmpNewImportDialogVisible(page);

  for (let attempt = 0; attempt < 80; attempt += 1) {
    const wizard = page.getByTestId('dmp-new-import-dialog');
    const confirm = page.getByTestId('confirm-button');
    const skip = page.getByTestId('skip-button');
    const segmentPlace = page.getByTestId('segment-place-button');
    const segmentDiscard = page.getByTestId('segment-mode-discard');
    const placeOnMap = page.getByTestId('segment-place-on-map-button');

    if (!(await wizard.isVisible())) {
      const placementIndicator = page.getByTestId('placement-mode-indicator');
      if (await placementIndicator.isVisible()) {
        await page.keyboard.press('Escape');
        await expect(wizard).toBeVisible({ timeout: 10000 });
      } else {
        return;
      }
      continue;
    }

    if (await confirm.isVisible() && await confirm.isEnabled()) {
      await confirm.click();
      continue;
    }

    if (await skip.isVisible() && await skip.isEnabled()) {
      await skip.click();
      continue;
    }

    if (await segmentPlace.isVisible() && await segmentPlace.isEnabled()) {
      await segmentPlace.click();
      continue;
    }

    if (await segmentDiscard.isVisible() && await segmentPlace.isVisible()) {
      await segmentDiscard.click();
      if (await segmentPlace.isEnabled()) {
        await segmentPlace.click();
        continue;
      }
    }

    if (await placeOnMap.isVisible() && await placeOnMap.isEnabled()) {
      await placeOnMap.click();
      const mapCanvas = page.getByTestId('map-canvas');
      await expect(mapCanvas).toBeVisible({ timeout: 10000 });
      const mapBounds = await mapCanvas.boundingBox();
      if (mapBounds) {
        await page.mouse.click(
          mapBounds.x + mapBounds.width * 0.5,
          mapBounds.y + mapBounds.height * 0.5
        );
      }
      continue;
    }

    if (await page.getByTestId('wizard-all-placed').isVisible()) {
      break;
    }

    await page.waitForTimeout(100);
  }

  const wizard = page.getByTestId('dmp-new-import-dialog');
  if (await wizard.isVisible()) {
    await page.getByTestId('cancel-button').click();
    await expect(page.getByTestId('cancel-confirm-dialog')).toBeVisible({ timeout: 5000 });
    await page.getByTestId('confirm-cancel-button').click();
    await expect(wizard).toBeHidden({ timeout: 10000 });
  }
}

/**
 * Matches the `stalagmight-tiles` custom tile protocol in EITHER platform form.
 *
 * Tauri v2 surfaces a registered custom scheme differently per webview:
 *   - macOS/Linux (WRY):        `stalagmight-tiles://<provider>/{z}/{y}/{x}`
 *   - Windows/WebView2:         `http://stalagmight-tiles.localhost/<provider>/{z}/{y}/{x}`
 * The Vite + Chromium test environment reports the `http://…localhost/` form. Both
 * are served by the same registered Rust protocol handler (`src-tauri/src/lib.rs`
 * `register_asynchronous_uri_scheme_protocol("stalagmight-tiles", …)`) - neither is
 * a raw network fetch. This regex intentionally accepts only those two forms, so a
 * real external tile server (`https://server.arcgisonline.com/…`) still fails the
 * assertion (the original CORS-bug guard these satellite tests were written for).
 */
export const CUSTOM_TILE_URL_RE =
  /^(?:stalagmight-tiles:\/\/|http:\/\/stalagmight-tiles\.localhost\/)/;

// ---------------------------------------------------------------------------
// Drawn-geometry screen positions (custom WebGL renderer)
// ---------------------------------------------------------------------------
//
// The survey is drawn by a custom WebGL layer, so `queryRenderedFeatures` on the
// old stock `survey-stations` / `survey-shots` layers returns NOTHING (those
// layers no longer exist). Tests that need on-screen positions read the drawn
// geometry from the app's `__stalagmight_geometry__` E2E hook (the same SoA
// geometry index the renderer and hit-testing use) and project it with the live
// map, so the coordinates match what is actually painted.

export interface ScreenPoint {
  x: number;
  y: number;
}

/** A station's identity plus its current screen position. */
export interface StationScreenPos extends ScreenPoint {
  sectionId: number;
  stationIndex: number;
}

/** A shot's identity plus both endpoints' screen positions and its midpoint. */
export interface ShotScreenPos {
  sectionId: number;
  shotId: number;
  from: ScreenPoint;
  to: ScreenPoint;
  mid: ScreenPoint;
}

interface GeometrySnapshot {
  stations: StationScreenPos[];
  shots: ShotScreenPos[];
}

/**
 * Screen positions of every drawn station and shot, honoring the live
 * reference-shots toggle (hidden reference legs are absent, matching the
 * renderer). Empty arrays until the map + geometry are ready - poll with
 * `expect.poll` rather than asserting immediately after a seed.
 */
export async function getDrawnGeometry(page: Page): Promise<GeometrySnapshot> {
  return page.evaluate(() => {
    const w = window as unknown as {
      __stalagmight_map__?: { project: (c: [number, number]) => { x: number; y: number } };
      __stalagmight_geometry__?: () => {
        stations: Array<{ sectionId: number; stationIndex: number; lngLat: [number, number] }>;
        shots: Array<{
          sectionId: number;
          shotId: number;
          from: [number, number];
          to: [number, number];
        }>;
      } | null;
    };
    const map = w.__stalagmight_map__;
    const geometry = w.__stalagmight_geometry__?.();
    if (!map || !geometry) return { stations: [], shots: [] };
    const stations = geometry.stations.map((s) => {
      const p = map.project(s.lngLat);
      return { sectionId: s.sectionId, stationIndex: s.stationIndex, x: p.x, y: p.y };
    });
    const shots = geometry.shots.map((sh) => {
      const from = map.project(sh.from);
      const to = map.project(sh.to);
      return {
        sectionId: sh.sectionId,
        shotId: sh.shotId,
        from: { x: from.x, y: from.y },
        to: { x: to.x, y: to.y },
        mid: { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 },
      };
    });
    return { stations, shots };
  });
}

/** Screen positions of the drawn stations (see {@link getDrawnGeometry}). */
export async function getStationScreenPositions(page: Page): Promise<StationScreenPos[]> {
  return (await getDrawnGeometry(page)).stations;
}

/** Screen positions of the drawn shots (see {@link getDrawnGeometry}). */
export async function getShotScreenPositions(page: Page): Promise<ShotScreenPos[]> {
  return (await getDrawnGeometry(page)).shots;
}

/** Wait until at least `minStations` stations are drawn, then return them. */
export async function waitForDrawnStations(
  page: Page,
  minStations = 1,
  timeout = 10000,
): Promise<StationScreenPos[]> {
  await expect
    .poll(async () => (await getStationScreenPositions(page)).length, { timeout })
    .toBeGreaterThanOrEqual(minStations);
  return getStationScreenPositions(page);
}

/**
 * How much measured-path (ruler) tint is drawn: tinted shot segments + tinted
 * station discs. The tint is a custom GL layer with no MapLibre feature-state,
 * so this replaces reading a `measured` feature-state off the (deleted) survey
 * vector-tile sources.
 */
export async function getMeasuredPathCounts(
  page: Page,
): Promise<{ shots: number; stations: number }> {
  return page.evaluate(() => {
    const w = window as unknown as {
      __stalagmight_measured__?: () => { shots: number; stations: number };
    };
    return w.__stalagmight_measured__?.() ?? { shots: 0, stations: 0 };
  });
}

/** Which drag-preview path is drawing, and the live cursor delta in CSS px. */
export interface DragGhostState {
  mode: 'custom' | 'stock';
  dx: number;
  dy: number;
}

/**
 * The live drag-preview offset, and optionally where a map coordinate projects
 * RIGHT NOW, from a single evaluate so both come from the same frame.
 *
 * The moving preview is the custom WebGL ghost layer: its vertices are the
 * captured ORIGINAL positions and the cursor delta is a shader uniform, so a
 * grabbed feature paints at `project(original) + offset`. That is the arithmetic
 * the deleted `getPaintProperty('survey-drag-stations','circle-translate')` read
 * used to feed. `mode` names the live path, so the stock GeoJSON fallback (which
 * re-bakes its geometry AT the offset and leaves only a residual translate) can
 * never masquerade as the custom path. Null while no drag preview is active.
 *
 * Backed by `window.__stalagmight_drag_ghost__` (src/dev/mapE2eHooks.ts ->
 * SurveyLayer.dragGhostOffset()), so no test reaches into the ghost layer's
 * private fields.
 */
export async function getDragGhostState(
  page: Page,
  lngLat?: { lng: number; lat: number },
): Promise<(DragGhostState & { screen: ScreenPoint | null }) | null> {
  return page.evaluate((ll) => {
    const w = window as unknown as {
      __stalagmight_map__?: { project: (c: [number, number]) => { x: number; y: number } };
      __stalagmight_drag_ghost__?: () => { mode: 'custom' | 'stock'; dx: number; dy: number } | null;
    };
    const ghost = w.__stalagmight_drag_ghost__?.() ?? null;
    if (!ghost) return null;
    let screen: { x: number; y: number } | null = null;
    if (ll && w.__stalagmight_map__) {
      const p = w.__stalagmight_map__.project([ll.lng, ll.lat]);
      screen = { x: p.x, y: p.y };
    }
    return { mode: ghost.mode, dx: ghost.dx, dy: ghost.dy, screen };
  }, lngLat ?? null);
}

// ---------------------------------------------------------------------------
// Canvas pixel probe (custom WebGL renderer)
// ---------------------------------------------------------------------------
//
// With the custom renderer there is no `getPaintProperty('survey-stations',
// 'circle-color')` and no per-feature state to read: a station's / shot's color
// lives in a packed vertex attribute on the GPU, and "is it hidden" is a skipped
// draw call. The honest oracle for both is therefore the painted pixel.
//
// The map canvas is created with `preserveDrawingBuffer: false`, so the drawing
// buffer is only readable *inside* the frame that drew it. Reading during
// MapLibre's `render` event (fired synchronously at the end of `_render()`,
// before the compositor swap) is valid and deterministic - two reads of the same
// unchanged scene return byte-identical pixels.

/** One RGBA sample (each channel 0..255). */
export type PixelRgba = [r: number, g: number, b: number, a: number];

/**
 * Read a `size` x `size` CSS-pixel block of the map canvas centered on
 * (`x`, `y`) (canvas-relative CSS px, e.g. a {@link StationScreenPos}).
 * Returns a flat RGBA array in WebGL row order, or `[]` if the map/GL context
 * is unavailable.
 */
export async function readCanvasPixelBlock(
  page: Page,
  x: number,
  y: number,
  size = 1,
): Promise<number[]> {
  return page.evaluate(
    ([px, py, sz]) =>
      new Promise<number[]>((resolve) => {
        const map = (
          window as unknown as {
            __stalagmight_map__?: {
              getCanvas: () => HTMLCanvasElement;
              triggerRepaint: () => void;
              once: (ev: string, cb: () => void) => void;
            };
          }
        ).__stalagmight_map__;
        if (!map) return resolve([]);
        const canvas = map.getCanvas();
        // `getContext` returns the context MapLibre already created; the fallback
        // covers a WebGL1 map (a machine without WebGL2).
        const gl = (canvas.getContext('webgl2') ??
          canvas.getContext('webgl')) as WebGL2RenderingContext | null;
        if (!gl) return resolve([]);
        const dpr = canvas.width / canvas.clientWidth;
        map.once('render', () => {
          const w = Math.max(1, Math.round(sz * dpr));
          const h = Math.max(1, Math.round(sz * dpr));
          const gx = Math.round(px * dpr - w / 2);
          // WebGL's origin is bottom-left; CSS coords are top-left.
          const gy = Math.round(canvas.height - py * dpr - h / 2);
          const buf = new Uint8Array(w * h * 4);
          gl.bindFramebuffer(gl.FRAMEBUFFER, null);
          gl.readPixels(gx, gy, w, h, gl.RGBA, gl.UNSIGNED_BYTE, buf);
          resolve(Array.from(buf));
        });
        map.triggerRepaint();
      }),
    [x, y, size] as const,
  );
}

/** The single painted pixel at (`x`, `y`) - see {@link readCanvasPixelBlock}. */
export async function readCanvasPixel(page: Page, x: number, y: number): Promise<PixelRgba> {
  const buf = await readCanvasPixelBlock(page, x, y, 1);
  return [buf[0] ?? 0, buf[1] ?? 0, buf[2] ?? 0, buf[3] ?? 0];
}

/**
 * How many pixels in a `size` x `size` block around (`x`, `y`) differ from the
 * map background by more than `tolerance` per channel. "Ink" = something was
 * drawn there, so this answers "are the markers/lines on screen?" without
 * hard-coding any palette. Pass the same `bg` you sampled from an empty part of
 * the canvas.
 */
export async function countCanvasInk(
  page: Page,
  x: number,
  y: number,
  bg: PixelRgba,
  size = 9,
  tolerance = 12,
): Promise<number> {
  const buf = await readCanvasPixelBlock(page, x, y, size);
  let ink = 0;
  for (let i = 0; i < buf.length; i += 4) {
    const dr = Math.abs((buf[i] ?? 0) - bg[0]);
    const dg = Math.abs((buf[i + 1] ?? 0) - bg[1]);
    const db = Math.abs((buf[i + 2] ?? 0) - bg[2]);
    if (dr > tolerance || dg > tolerance || db > tolerance) ink++;
  }
  return ink;
}

/** Wait until at least `minShots` shots are drawn, then return them. */
export async function waitForDrawnShots(
  page: Page,
  minShots = 1,
  timeout = 10000,
): Promise<ShotScreenPos[]> {
  await expect
    .poll(async () => (await getShotScreenPositions(page)).length, { timeout })
    .toBeGreaterThanOrEqual(minShots);
  return getShotScreenPositions(page);
}
