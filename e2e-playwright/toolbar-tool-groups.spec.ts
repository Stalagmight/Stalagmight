import { test, expect } from './fixtures/coverage';
import type { Page, Locator } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedNavigationTools,
  seedProjectWithSections,
  waitForMapReady,
  waitForActiveTool,
  waitForMapIdle,
  waitForNoDialogs,
  waitForDrawnStations,
  getShotScreenPositions,
  readCanvasPixel,
  readCanvasPixelBlock,
  type PixelRgba,
  type ScreenPoint,
  type SectionSeed,
} from './helpers/app';

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Main Passage',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
      { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0, left: 0.5, right: 0.7, up: 1.2, down: 1.5 },
    ],
  },
];

async function createProjectWithSections(page: import('./fixtures/coverage').Page): Promise<void> {
  await createProject(page, 'Tool Groups Test');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

async function pressUntilFocused(
  page: Page,
  locator: Locator,
  key: string,
  maxAttempts = 5,
): Promise<void> {
  // Retry the press (focus walks one stop per key), but wait on focus rather
  // than sleeping between tries.
  let attempts = 0;
  await expect(async () => {
    attempts += 1;
    expect(attempts, `${key} never moved focus to the target`).toBeLessThanOrEqual(maxAttempts);
    await page.keyboard.press(key);
    await expect(locator).toBeFocused({ timeout: 250 });
  }).toPass({ timeout: 5000 });
}

// ---------------------------------------------------------------------------
// Painted-fade oracle (custom WebGL renderer)
// ---------------------------------------------------------------------------
//
// The anchored fade used to be a MapLibre `anchored` feature-state on the
// `survey-data-stations` vector-tile source, which a paint expression turned
// into opacity 0.25. Both are gone: the survey is drawn by a custom WebGL layer
// with no sources and no feature-state, and "anchored" is now a per-vertex `dim`
// attribute (SurveyLayer.setAnchoredSections -> syncCustomDimming ->
// customRender.setDimming, uploaded by controller.setDimming). The shader turns
// it into the same alpha multiplier the stock expression used
// (`dimF = a_dim > 0.5 ? 0.25 : 1.0` in surveyLayer/customRender/shaders.ts), so
// the only honest observable left is the painted pixel: an anchored feature is
// blended 3:1 toward the map background.

/** CSS-px block sampled around a station center (comfortably wider than a disc). */
const STATION_BLOCK = 15;

/**
 * How much paint a block holds: summed per-pixel distance from the map
 * background, worst channel. Unlike a thresholded pixel count this is monotone
 * in the feature's alpha, so a fade to 0.25 shows up as a ~4x drop instead of
 * depending on whether the faded color still clears an ink tolerance.
 */
async function inkMass(
  page: Page,
  probes: ScreenPoint[],
  bg: PixelRgba,
  block = STATION_BLOCK,
): Promise<number> {
  if (probes.length === 0) throw new Error('no probe points');
  let mass = 0;
  for (const probe of probes) {
    const buf = await readCanvasPixelBlock(page, probe.x, probe.y, block);
    for (let i = 0; i < buf.length; i += 4) {
      mass += Math.max(
        Math.abs((buf[i] ?? 0) - bg[0]),
        Math.abs((buf[i + 1] ?? 0) - bg[1]),
        Math.abs((buf[i + 2] ?? 0) - bg[2]),
      );
    }
  }
  return mass;
}

/**
 * Ink mass once the paint stops changing between reads. The pixel probe is
 * deterministic for an unchanged scene, so two equal consecutive reads mean
 * nothing is still settling (label reveal, tile fade-in).
 */
async function settledMass(page: Page, probes: ScreenPoint[], bg: PixelRgba): Promise<number> {
  let previous = -1;
  await expect
    .poll(
      async () => {
        const current = await inkMass(page, probes, bg);
        const stable = current === previous;
        previous = current;
        return stable;
      },
      { timeout: 10000 },
    )
    .toBe(true);
  return previous;
}

/** Poll the ink mass until it satisfies `predicate` (the dim re-upload lands a
 *  frame after the anchored-ids fetch resolves). */
async function pollMass(
  page: Page,
  probes: ScreenPoint[],
  bg: PixelRgba,
  predicate: (mass: number) => boolean,
): Promise<number> {
  let result = -1;
  await expect
    .poll(
      async () => {
        result = await inkMass(page, probes, bg);
        return predicate(result);
      },
      { timeout: 10000 },
    )
    .toBe(true);
  return result;
}

/**
 * Zoom in on the seeded survey before probing pixels. The mock lays stations out
 * at `x = index * 10` metres, so at the fit-to-bounds zoom the whole section is a
 * ~9 px smear with no per-station pixel to read. `jumpTo` is not animated, so
 * positions read afterwards already match the new view.
 */
async function zoomInOnSurvey(page: Page, zoomDelta = 4): Promise<void> {
  await page.evaluate((dz) => {
    const w = window as unknown as {
      __stalagmight_map__?: {
        jumpTo: (opts: { center: [number, number]; zoom: number }) => void;
        getZoom: () => number;
      };
      __stalagmight_geometry__?: () => { stations: Array<{ lngLat: [number, number] }> } | null;
    };
    const map = w.__stalagmight_map__;
    const geometry = w.__stalagmight_geometry__?.();
    if (!map || !geometry || geometry.stations.length === 0) {
      throw new Error('map + drawn geometry must be ready before zooming the probe view');
    }
    const count = geometry.stations.length;
    const lng = geometry.stations.reduce((sum, s) => sum + s.lngLat[0], 0) / count;
    const lat = geometry.stations.reduce((sum, s) => sum + s.lngLat[1], 0) / count;
    map.jumpTo({ center: [lng, lat], zoom: map.getZoom() + dz });
  }, zoomDelta);
}

test.describe('Toolbar Tool Groups', () => {
  test.beforeEach(async ({ page }) => {
    // The navigate group is one of the two groups this spec exercises, and it is
    // hidden unless `map.controls.show_navigation_tools` is on (#409). Seeded
    // before setupMockedPage: the toolbar reads the flag from the app's first
    // `get_config` at boot. The default-config toolbar is covered by the
    // describe at the bottom of this file.
    await seedNavigationTools(page);
    await setupMockedPage(page);
  });

  test('tool group buttons are visible with correct testids @smoke', async ({ page }) => {
    await createProjectWithSections(page);

    // Navigate group and Move group should be visible
    const navigateGroup = page.getByTestId('tool-group-navigate');
    const moveGroup = page.getByTestId('tool-group-move');

    await expect(navigateGroup).toBeVisible();
    await expect(moveGroup).toBeVisible();
  });

  test('left-click on inactive group activates last-used subtool @smoke', async ({ page }) => {
    await createProjectWithSections(page);

    // Start with select tool active
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');

    // Click the move group button - should activate default subtool (move_segment)
    const moveGroup = page.getByTestId('tool-group-move');
    await moveGroup.click();
    await waitForActiveTool(page, 'move_segment');
  });

  test('left-click on active group cycles to next subtool', async ({ page }) => {
    await createProjectWithSections(page);

    // Activate move_segment via keyboard
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');

    // Click the move group button - should cycle to next subtool (move_shot)
    const moveGroup = page.getByTestId('tool-group-move');
    await moveGroup.click();
    await waitForActiveTool(page, 'move_shot');

    // Click again - should cycle to move_station
    await moveGroup.click();
    await waitForActiveTool(page, 'move_station');

    // Click again - should wrap back to move_segment
    await moveGroup.click();
    await waitForActiveTool(page, 'move_segment');
  });

  test('right-click on group opens flyout panel @smoke', async ({ page }) => {
    await createProjectWithSections(page);

    // Right-click on the move group
    const moveGroup = page.getByTestId('tool-group-move');
    await moveGroup.dispatchEvent('contextmenu');

    // Flyout should be visible
    const flyout = page.getByTestId('subtool-flyout-move');
    await expect(flyout).toBeVisible();

    // Subtool items should be visible
    await expect(page.getByTestId('subtool-move_segment')).toBeVisible();
    await expect(page.getByTestId('subtool-move_shot')).toBeVisible();
    await expect(page.getByTestId('subtool-move_station')).toBeVisible();
  });

  test('clicking subtool in flyout activates it and closes flyout', async ({ page }) => {
    await createProjectWithSections(page);

    // Open flyout for navigate group
    const navigateGroup = page.getByTestId('tool-group-navigate');
    await navigateGroup.dispatchEvent('contextmenu');

    const flyout = page.getByTestId('subtool-flyout-navigate');
    await expect(flyout).toBeVisible();

    // Click the zoom subtool
    await page.getByTestId('subtool-zoom').click();

    // Zoom should now be active
    await waitForActiveTool(page, 'zoom');

    // Flyout should close
    await expect(flyout).not.toBeVisible();
  });

  test('clicking outside flyout closes it', async ({ page }) => {
    await createProjectWithSections(page);

    // Open flyout
    const moveGroup = page.getByTestId('tool-group-move');
    await moveGroup.dispatchEvent('contextmenu');

    const flyout = page.getByTestId('subtool-flyout-move');
    await expect(flyout).toBeVisible();

    // Click on the canvas (outside flyout)
    const canvas = page.getByTestId('map-canvas');
    await canvas.click({ position: { x: 200, y: 200 } });

    // Flyout should close
    await expect(flyout).not.toBeVisible();
  });

  test('flyout keyboard navigation: ArrowDown/ArrowUp move focus, Enter and Space select', async ({ page }) => {
    await createProjectWithSections(page);

    // Activate move_segment first so flyout opens with it focused
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');

    // Open flyout for move group
    const moveGroup = page.getByTestId('tool-group-move');
    await moveGroup.dispatchEvent('contextmenu');

    const flyout = page.getByTestId('subtool-flyout-move');
    await expect(flyout).toBeVisible();

    // The active subtool (move_segment, index 0) should be focused initially
    const moveSegmentItem = page.getByTestId('subtool-move_segment');
    await expect(moveSegmentItem).toBeFocused({ timeout: 2000 });

    // ArrowDown should move focus to next item (move_shot, index 1)
    const moveShotItem = page.getByTestId('subtool-move_shot');
    await pressUntilFocused(page, moveShotItem, 'ArrowDown');

    // ArrowDown again to move_station (index 2)
    const moveStationItem = page.getByTestId('subtool-move_station');
    await pressUntilFocused(page, moveStationItem, 'ArrowDown');

    // ArrowUp should go back to move_shot (index 1)
    await pressUntilFocused(page, moveShotItem, 'ArrowUp');

    // Press Enter to select move_shot
    await page.keyboard.press('Enter');

    // Flyout should close and move_shot should be active
    await expect(flyout).not.toBeVisible();
    await waitForActiveTool(page, 'move_shot');

    // Now test Space selection: open flyout again
    await moveGroup.dispatchEvent('contextmenu');
    await expect(flyout).toBeVisible();

    // move_shot should be focused (since it's now active)
    await expect(moveShotItem).toHaveAttribute('aria-current', 'true');
    await moveShotItem.focus();
    await expect(moveShotItem).toBeFocused({ timeout: 2000 });

    // Navigate to move_station with ArrowDown
    await pressUntilFocused(page, moveStationItem, 'ArrowDown');

    // Press Space to select move_station
    await page.keyboard.press(' ');

    // Flyout should close and move_station should be active
    await expect(flyout).not.toBeVisible();
    await waitForActiveTool(page, 'move_station');
  });

  test('flyout keyboard navigation: Escape closes flyout', async ({ page }) => {
    await createProjectWithSections(page);

    // Open flyout for move group
    const moveGroup = page.getByTestId('tool-group-move');
    await moveGroup.dispatchEvent('contextmenu');

    const flyout = page.getByTestId('subtool-flyout-move');
    await expect(flyout).toBeVisible();

    // Focus into the flyout so the keydown handler fires
    // The flyout should auto-focus a menu item when opened
    const firstMenuItem = flyout.locator('[role="menuitem"]').first();
    await expect(firstMenuItem).toBeFocused({ timeout: 2000 });

    // Escape should close the flyout
    await page.keyboard.press('Escape');
    await expect(flyout).not.toBeVisible();
  });

  test('keyboard shortcuts: V activates move_segment, G activates move_shot, T activates move_station', async ({ page }) => {
    await createProjectWithSections(page);

    // V should activate move_segment
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');

    // T should activate move_station
    await page.keyboard.press('t');
    await waitForActiveTool(page, 'move_station');

    // G should activate move_shot
    await page.keyboard.press('g');
    await waitForActiveTool(page, 'move_shot');
  });

  test('toolbar separators are visible between tool groups', async ({ page }) => {
    await createProjectWithSections(page);

    // The toolbar container should have separator dividers (thin vertical lines)
    const toolbar = page.getByTestId('toolbar');
    await expect(toolbar).toBeVisible();

    // Separators are rendered as div.w-px.h-6.bg-border elements inside the tools group.
    const separators = page.getByTestId('toolbar-tools-group').locator('.w-px.h-6.bg-border');
    // Layout (#51): [Select Pan/Zoom] | [Merge Comment Move] | [Measure] → 2 dividers.
    const count = await separators.count();
    expect(count).toBe(2);
  });

  test('anchored sections are faded when Move Segment tool is active', async ({ page }) => {
    await createProjectWithSections(page);

    // Set a zero datum on the first section's station 0 via store
    await page.evaluate(async () => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              setZeroDatum: (stationRef: unknown, coordinates: unknown) => Promise<void>;
            };
          };
        };
      }).__E2E_TEST__;
      if (!e2e) throw new Error('E2E stores not available');
      await e2e.projectStore.getState().setZeroDatum(
        { section_id: 1, station_index: 0 },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 }
      );
    });

    // Baseline the paint on section 1's own geometry BEFORE the tool is active,
    // so the fade is a before/after fact about the exact same pixels. Station
    // index 0 is skipped because it carries the zero datum, whose marker is drawn
    // on top of the disc. The seeded project has a single section and the datum
    // anchors all of it, so there is no unanchored section to contrast against
    // here (the mock lays every section on the same coordinates, so a second one
    // would only stack on these pixels anyway).
    await waitForDrawnStations(page, 1);
    await zoomInOnSurvey(page);
    await waitForMapIdle(page);
    const stations = await waitForDrawnStations(page, 3);
    const shots = await getShotScreenPositions(page);
    const canvasBox = await page.evaluate(() => {
      const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } })
        .__stalagmight_map__;
      const canvas = map!.getCanvas();
      return { width: canvas.clientWidth, height: canvas.clientHeight };
    });
    // A probe block that hangs off the canvas edge reads back as zeroes, which
    // against the dark basemap looks like a constant slab of "paint" that never
    // dims, diluting the ratio below into meaninglessness. At this zoom
    // the outer stations sit outside the viewport, so keep only blocks that fit.
    const onCanvas = (p: ScreenPoint): boolean =>
      p.x - STATION_BLOCK / 2 >= 0 &&
      p.y - STATION_BLOCK / 2 >= 0 &&
      p.x + STATION_BLOCK / 2 <= canvasBox.width &&
      p.y + STATION_BLOCK / 2 <= canvasBox.height;
    const probes: ScreenPoint[] = [
      ...stations.filter((s) => s.sectionId === 1 && s.stationIndex > 0).map((s) => ({ x: s.x, y: s.y })),
      ...shots.filter((s) => s.sectionId === 1).map((s) => s.mid),
    ].filter(onCanvas);
    expect(probes.length).toBeGreaterThan(0);

    const bg = await readCanvasPixel(page, 8, 8);
    const brightMass = await settledMass(page, probes, bg);
    expect(brightMass).toBeGreaterThan(0);

    // Activate move_segment tool
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');

    // The anchored section is really PAINTED faded once the anchored-id fetch
    // resolves. A dimmed shot body lands on exactly the 0.25 the stock opacity
    // expression used; a dimmed station disc reads higher (~0.58 measured)
    // because two translucent round caps and the disc overlap at a junction and
    // premultiplied blending applies three times, which shaders.ts documents as
    // accepted (stock's round-capped sublayer behaved the same). Hence a bound
    // that is unambiguously a fade rather than the exact blend.
    const fadedMass = await pollMass(page, probes, bg, (mass) => mass < brightMass * 0.5);
    expect(fadedMass).toBeLessThan(brightMass * 0.5);

    // The app should remain functional with the move_segment tool active
    const canvas = page.getByTestId('map-canvas');
    await expect(canvas).toBeVisible();
    await expect(canvas).toHaveAttribute('data-active-tool', 'move_segment');

    // The fade belongs to Move Segment's anchored set, not to some one-way
    // repaint: leaving the tool empties the set (MapCanvas clears
    // anchoredSectionIds for every other tool) and the section is painted at full
    // opacity again, byte-for-byte back to the baseline.
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');
    expect(await pollMass(page, probes, bg, (mass) => mass === brightMass)).toBe(brightMass);
  });

  // The fade above is painted by the CUSTOM renderer, which dims from the real
  // section id. This covers the other consumer of the anchored set: the interact
  // overlay, which must withhold the hover highlight for an anchored shot.
  //
  // It also pins that a late-arriving anchored set invalidates the overlay's
  // id-only cache key, by never moving the pointer after the first hover.
  //
  // NOTE (#301): this does NOT pin the floor(fid / STRIDE) decode that #301 fixed.
  // That bug needed a shot whose id is section-encoded, and the IPC mock does not
  // reproduce the backend's convention: insert_shot_* allocates max(existing) + 1
  // (mocks/tauri-ipc.ts), so every section's shots are 1, 2, 3 ..., while the real
  // backend seeds next_shot_id = section.id * FEATURE_ID_STRIDE
  // (src-tauri/src/models/section.rs). With flat ids the old decode happened to be
  // right, so this test passed before the fix too. The decode itself is pinned at
  // unit level in src/utils/__tests__/ids.test.ts; teaching the mock the real id
  // convention would make this test cover it too, and is tracked separately.
  test('anchored section suppresses the shot hover highlight in Move Segment', async ({ page }) => {
    await createProjectWithSections(page);

    await page.evaluate(async () => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              setZeroDatum: (stationRef: unknown, coordinates: unknown) => Promise<void>;
            };
          };
        };
      }).__E2E_TEST__;
      if (!e2e) throw new Error('E2E stores not available');
      await e2e.projectStore.getState().setZeroDatum(
        { section_id: 1, station_index: 0 },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 }
      );
    });

    await waitForDrawnStations(page, 1);
    await zoomInOnSurvey(page);
    await waitForMapIdle(page);
    await waitForDrawnStations(page, 3);

    const shot = (await getShotScreenPositions(page)).find((s) => s.sectionId === 1);
    if (!shot) throw new Error('no section-1 shot to hover');

    // Read the interact-overlay source rather than pixels: it is the exact
    // collection refreshInteractOverlay builds, so a leaked highlight is a
    // present feature rather than a brightness delta. getStyle() is used instead
    // of getSource() because the latter hands back a MapLibre class instance
    // (see satellite-bug-regression.spec.ts).
    const overlayShotCount = async (): Promise<number> =>
      page.evaluate(() => {
        const map = (window as unknown as {
          __stalagmight_map__?: { getStyle: () => { sources: Record<string, unknown> } };
        }).__stalagmight_map__;
        const src = map?.getStyle().sources['survey-interact'] as
          | { data?: { features?: { properties?: { kind?: string } | null }[] } }
          | undefined;
        const features = src?.data?.features ?? [];
        return features.filter((f) => f.properties?.kind === 'shot').length;
      });

    // map.project() is canvas-relative; page.mouse wants viewport coords.
    const canvasOrigin = await page.getByTestId('map-canvas').boundingBox();
    if (!canvasOrigin) throw new Error('map canvas has no bounding box');
    const hoverPoint = { x: canvasOrigin.x + shot.mid.x, y: canvasOrigin.y + shot.mid.y };

    // Non-throwing: expect.poll awaits its generator OUTSIDE its try/catch, so a
    // throw from here would abort the poll instead of retrying it.
    const hoverShot = async (): Promise<boolean> => {
      await page.mouse.move(hoverPoint.x, hoverPoint.y, { steps: 4 });
      return page
        .waitForFunction(
          (expected) => {
            const ui = (window as unknown as {
              __E2E_TEST__?: { uiStore: { getState: () => { hoveredShot: { shotId: number } | null } } };
            }).__E2E_TEST__;
            return ui?.uiStore.getState().hoveredShot?.shotId === expected;
          },
          shot.shotId,
          { timeout: 2000 }
        )
        .then(() => true)
        .catch(() => false);
    };

    // Positive control first: with no anchored set (Select tool), hovering that
    // exact pixel really does produce one shot highlight. Without this the
    // assertion below could pass simply because the hover never landed.
    expect(await hoverShot()).toBe(true);
    await expect.poll(overlayShotCount, { timeout: 5000 }).toBe(1);

    // Switch to Move Segment WITHOUT moving the pointer. The anchored ids arrive
    // asynchronously, one IPC round-trip after activation, and they are not part
    // of the interact overlay's cache key - so this also pins that
    // setAnchoredSections invalidates that cache. If it stops doing so, the
    // highlight painted above stays bright forever and this poll times out.
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');
    await expect.poll(overlayShotCount, { timeout: 10000 }).toBe(0);

    // And the inverse: leaving the tool empties the anchored set, which must
    // bring the highlight back with the pointer still held still.
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');
    await expect.poll(overlayShotCount, { timeout: 10000 }).toBe(1);

    // The cursor must agree with the highlight. It is only recomputed when the
    // hovered id changes, so without a recompute on the dim-set change it stays
    // as setActiveTool left it and the feature reads as un-grabbable while
    // painting a hover highlight - the cursor/highlight disagreement #301 is about.
    await expect
      .poll(
        () =>
          page.evaluate(() => {
            const map = (window as unknown as {
              __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement };
            }).__stalagmight_map__;
            return map?.getCanvas().style.cursor ?? '';
          }),
        { timeout: 10000 }
      )
      .toBe('pointer');
  });
});

/**
 * The default toolbar - no `seedNavigationTools`, so the mock's config reports
 * `map.controls.show_navigation_tools: false` exactly like a fresh install.
 *
 * Its own top-level describe rather than a nested one: the seed registers an
 * init script and must run BEFORE `setupMockedPage`, so a nested `beforeEach`
 * (which runs after the outer one has already navigated) cannot opt out of it.
 */
test.describe('Toolbar Tool Groups - default config (#409)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('the navigate group is absent while the other tools stay', async ({ page }) => {
    await createProjectWithSections(page);

    // Gone from the DOM, not merely hidden - the toolbar filters the item out.
    await expect(page.getByTestId('tool-group-navigate')).toHaveCount(0);

    // Everything else the survey toolbar offers is untouched.
    await expect(page.getByTestId('tool-select')).toBeVisible();
    await expect(page.getByTestId('tool-merge')).toBeVisible();
    await expect(page.getByTestId('tool-measure')).toBeVisible();
    await expect(page.getByTestId('tool-group-move')).toBeVisible();
  });
});
