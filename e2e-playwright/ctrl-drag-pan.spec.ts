import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  getMapCenter,
  getSelection,
  seedNavigationTools,
  seedProjectWithSections,
  waitForDrawnStations,
  waitForMapReady,
  waitForActiveTool,
  waitForMapIdle,
  type SectionSeed,
  type StationScreenPos,
} from './helpers/app';

/**
 * Helper to create a new project and get to SingleSystemView with map canvas.
 */
async function createProjectWithMap(page: Page): Promise<void> {
  await createProject(page, 'Ctrl Drag Test');
  await waitForMapReady(page);
}

async function middleDrag(page: Page, startX: number, startY: number, endX: number, endY: number): Promise<void> {
  await page.mouse.move(startX, startY);
  await page.mouse.down({ button: 'middle' });
  await page.mouse.move(endX, endY, { steps: 8 });
  await page.mouse.up({ button: 'middle' });
}

/** Three stations, from two shots, for the Ctrl+click selection cases. */
const SELECTION_SEEDS: SectionSeed[] = [
  {
    name: 'Toggler',
    direction: 'in',
    shots: [
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: 0, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -2, depth_out: -4 },
    ],
  },
];

/**
 * The multi-selection, as `sectionId:stationIndex` keys. `getSelection` reads the
 * single "current" selection; Shift/Ctrl+click build the LIST beside it, which is
 * what a toggle has to be asserted against.
 */
async function getMultiSelectionKeys(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: {
          getState: () => {
            multiSelection: Array<{
              type: string;
              stationRef?: { section_id: number; station_index: number };
            }>;
          };
        };
      };
    }).__E2E_TEST__;
    return (stores?.uiStore.getState().multiSelection ?? []).map((s) =>
      s.type === 'station' && s.stationRef
        ? `${s.stationRef.section_id}:${s.stationRef.station_index}`
        : s.type,
    );
  });
}

/**
 * Centre the camera on the drawn survey and zoom in.
 *
 * A 10m shot is about two pixels at the opening zoom, so all three seeded
 * stations land inside one 10px hit area and a per-station click test would be
 * clicking whichever one the resolver happened to prefer.
 */
async function zoomToDrawnSurvey(page: Page, zoom: number): Promise<void> {
  await page.evaluate((z) => {
    const w = window as unknown as {
      __stalagmight_map__?: {
        setZoom: (z: number) => void;
        setCenter: (c: [number, number]) => void;
      };
      __stalagmight_geometry__?: () => { stations: Array<{ lngLat: [number, number] }> } | null;
    };
    const map = w.__stalagmight_map__;
    const stations = w.__stalagmight_geometry__?.()?.stations ?? [];
    if (!map || stations.length === 0) throw new Error('no drawn stations to centre on');
    const lngs = stations.map((s) => s.lngLat[0]);
    const lats = stations.map((s) => s.lngLat[1]);
    map.setCenter([
      (Math.min(...lngs) + Math.max(...lngs)) / 2,
      (Math.min(...lats) + Math.max(...lats)) / 2,
    ]);
    map.setZoom(z);
  }, zoom);
  await waitForMapIdle(page);
}

/** Click a drawn station (canvas-relative position) with a modifier held. */
async function clickStationWith(
  page: Page,
  box: { x: number; y: number },
  station: StationScreenPos,
  modifier: 'Shift' | 'Control',
): Promise<void> {
  await page.keyboard.down(modifier);
  await page.mouse.click(box.x + station.x, box.y + station.y);
  await page.keyboard.up(modifier);
}

test.describe('Ctrl+Drag Panning', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithMap(page);
  });

  // Supported tools that should allow Ctrl+drag panning.
  // NOTE: draw (d) / delete (x) were removed from the toolbar and unbound.
  const supportedTools = [
    { key: 's', name: 'select' },
    { key: 'm', name: 'merge' },
    { key: 'r', name: 'measure' },
    { key: 't', name: 'move_station' },
    { key: 'g', name: 'move_shot' },
    { key: 'v', name: 'move_segment' },
  ];

  test.describe('Cursor Changes', () => {
    for (const tool of supportedTools) {
      test(`should show grab cursor when Ctrl held in ${tool.name} mode`, async ({ page }) => {
        // Switch to the tool
        await page.keyboard.press(tool.key);
        await waitForActiveTool(page, tool.name);

        const canvas = page.getByTestId('map-canvas');
        await expect(canvas).toBeVisible();

        // Hold Ctrl
        await page.keyboard.down('Control');
        await page.waitForFunction(() => {
          const canvas = document.querySelector('canvas');
          return canvas?.style.cursor === 'grab';
        });

        // Get the cursor from the MapLibre canvas
        const cursor = await page.evaluate(() => {
          const canvas = document.querySelector('canvas');
          return canvas?.style.cursor || '';
        });
        expect(cursor).toBe('grab');

        await page.keyboard.up('Control');
      });
    }

    test('ending a Ctrl-pan under Move Station restores the arrow, not "move" (#316)', async ({ page }) => {
      // The mouseup that ends a Ctrl-pan restores a cursor of its own. It must
      // hand that to SurveyLayer rather than writing the canvas directly:
      // SurveyLayer owns the precedence, and Move Station/Shot deliberately rest
      // on an arrow (not their own `move`) so MapLibre's grab does not show
      // through over empty map. Writing `move` straight to the DOM defeats that
      // with nothing to correct it - hover only re-derives when the hovered id
      // CHANGES, so a pointer left on empty map keeps the wrong cursor.
      await page.keyboard.press('t');
      await waitForActiveTool(page, 'move_station');

      const canvas = page.getByTestId('map-canvas');
      const box = (await canvas.boundingBox())!;
      const x = box.x + box.width / 2;
      const y = box.y + box.height / 2;

      await page.keyboard.down('Control');
      await page.waitForFunction(() => document.querySelector('canvas')?.style.cursor === 'grab');
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x + 60, y + 60, { steps: 6 });
      // Release Ctrl BEFORE mouseup: that is what takes the restore's non-grab
      // branch, the one that used to write the tool cursor raw.
      await page.keyboard.up('Control');
      await page.mouse.up();
      await waitForMapIdle(page);

      // Settle first, then assert on ONE read. A bare `poll(...).toBe('default')`
      // would accept the wrong cursor followed by any later re-render healing it,
      // which is exactly the failure mode under test - the raw write is what a
      // subsequent recompute papers over.
      await expect
        .poll(() => page.evaluate(() => document.querySelector('canvas')?.style.cursor ?? ''), {
          timeout: 5000,
        })
        .not.toBe('');
      const settled = await page.evaluate(
        () => document.querySelector('canvas')?.style.cursor ?? '',
      );
      expect(settled).toBe('default');
    });
  });

  test.describe('Ctrl+Drag Panning Behavior', () => {
    for (const tool of supportedTools) {
      test(`should allow Ctrl+drag to pan in ${tool.name} mode`, async ({ page }) => {
        // Switch to the tool
        await page.keyboard.press(tool.key);
        await waitForActiveTool(page, tool.name);

        const canvas = page.getByTestId('map-canvas');
        const box = await canvas.boundingBox();
        expect(box).not.toBeNull();

        // Perform Ctrl+drag
        await page.keyboard.down('Control');
        await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
        await page.mouse.down();
        await page.mouse.move(box!.x + box!.width / 2 + 100, box!.y + box!.height / 2 + 100, { steps: 8 });
        await page.mouse.up();
        await page.keyboard.up('Control');

        await waitForMapIdle(page);

        // Map should remain functional after pan
        await expect(canvas).toBeVisible();
      });
    }
  });

  test.describe('Middle-Click Panning Behavior', () => {
    for (const tool of supportedTools) {
      test(`should allow middle-drag to pan in ${tool.name} mode and keep active tool`, async ({ page }) => {
        // Switch to the tool
        await page.keyboard.press(tool.key);
        await waitForActiveTool(page, tool.name);

        const canvas = page.getByTestId('map-canvas');
        const box = await canvas.boundingBox();
        expect(box).not.toBeNull();

        const centerBefore = await getMapCenter(page);
        expect(centerBefore).not.toBeNull();

        await middleDrag(
          page,
          box!.x + box!.width / 2,
          box!.y + box!.height / 2,
          box!.x + box!.width / 2 + 120,
          box!.y + box!.height / 2 + 80
        );
        await waitForMapIdle(page);

        // Pan override must not change the active tool.
        await waitForActiveTool(page, tool.name);

        const centerAfter = await getMapCenter(page);
        expect(centerAfter).not.toBeNull();

        const centerDelta = Math.abs(centerAfter!.lng - centerBefore!.lng) + Math.abs(centerAfter!.lat - centerBefore!.lat);
        expect(centerDelta).toBeGreaterThan(0.00001);
      });
    }
  });

  // Box select is gone (#409): plain left-drag pans instead, under every tool.
  // What survives from the old "Ctrl+drag must not box select" case is the claim
  // that Ctrl+drag in select mode PANS and selects nothing.
  test('Ctrl+drag in select mode pans and changes no selection', async ({ page }) => {
    // Switch to select mode
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');

    const canvas = page.getByTestId('map-canvas');
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();

    const selectionBefore = await getSelection(page);
    const centerBefore = await getMapCenter(page);
    expect(centerBefore).not.toBeNull();

    // Perform Ctrl+drag
    await page.keyboard.down('Control');
    await page.mouse.move(box!.x + 50, box!.y + 50);
    await page.mouse.down();
    await page.mouse.move(box!.x + 200, box!.y + 200, { steps: 8 });
    await page.mouse.up();
    await page.keyboard.up('Control');
    await waitForMapIdle(page);

    const centerAfter = await getMapCenter(page);
    expect(centerAfter).not.toBeNull();
    expect(
      Math.abs(centerAfter!.lng - centerBefore!.lng) + Math.abs(centerAfter!.lat - centerBefore!.lat)
    ).toBeGreaterThan(0.00001);

    // No box, so nothing to sweep into a selection.
    expect(await getSelection(page)).toEqual(selectionBefore);
  });

  /**
   * The other half of that claim: a Ctrl press that never MOVES is a click, and
   * the click is the documented multi-select toggle (CHANGELOG #409, SPEC 6.6
   * "Multi-select: Shift+click (add), Ctrl+click (toggle)").
   *
   * It was unreachable: the custom Ctrl-pan claimed every Ctrl+left press under
   * every tool but Pan and then swallowed the click behind it, so
   * `handleStationClick`'s `ctrlKey` branch (useMapTools) never ran. The press is
   * now only WATCHED until it travels past MapLibre's click tolerance.
   */
  test.describe('Ctrl+Click Selection Toggle', () => {
    test('Ctrl+click removes a selected station and adds an unselected one', async ({ page }) => {
      await seedProjectWithSections(page, SELECTION_SEEDS);
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');
      await waitForDrawnStations(page, 3);
      // Spread the stations out before clicking them one at a time.
      await zoomToDrawnSurvey(page, 20);

      const canvas = page.getByTestId('map-canvas');
      const box = (await canvas.boundingBox())!;
      const stations = await waitForDrawnStations(page, 3);
      const [a, b, c] = [...stations].sort((p, q) => p.stationIndex - q.stationIndex);
      // Guard the premise: three stations, each well outside the others' 10px hit
      // area, all on screen. Without this a failure below could just mean the
      // clicks all landed on the same station.
      for (const s of [a!, b!, c!]) {
        expect(s.x).toBeGreaterThan(20);
        expect(s.y).toBeGreaterThan(20);
        expect(s.x).toBeLessThan(box.width - 20);
        expect(s.y).toBeLessThan(box.height - 20);
      }
      expect(Math.hypot(a!.x - b!.x, a!.y - b!.y)).toBeGreaterThan(30);
      expect(Math.hypot(b!.x - c!.x, b!.y - c!.y)).toBeGreaterThan(30);

      const key = (s: StationScreenPos) => `${s.sectionId}:${s.stationIndex}`;

      // Build a two-station multi-selection with the gesture that always worked.
      await clickStationWith(page, box, a!, 'Shift');
      await clickStationWith(page, box, b!, 'Shift');
      await expect.poll(() => getMultiSelectionKeys(page)).toEqual([key(a!), key(b!)]);

      // Toggle OFF: Ctrl+click a station already in the selection.
      await clickStationWith(page, box, b!, 'Control');
      await expect.poll(() => getMultiSelectionKeys(page)).toEqual([key(a!)]);

      // Toggle ON: Ctrl+click one that is not.
      await clickStationWith(page, box, c!, 'Control');
      await expect.poll(() => getMultiSelectionKeys(page)).toEqual([key(a!), key(c!)]);
    });

    test('a Ctrl+drag over a station still pans and toggles nothing', async ({ page }) => {
      // The complement of the case above, and the reason the takeover is measured
      // in travel rather than dropped altogether: pressing Ctrl ON a station and
      // dragging must still pan, not add it to the selection.
      await seedProjectWithSections(page, SELECTION_SEEDS);
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');
      await waitForDrawnStations(page, 3);
      await zoomToDrawnSurvey(page, 20);

      const canvas = page.getByTestId('map-canvas');
      const box = (await canvas.boundingBox())!;
      const stations = await waitForDrawnStations(page, 3);
      const start = [...stations].sort((p, q) => p.stationIndex - q.stationIndex)[0]!;

      const centerBefore = await getMapCenter(page);
      await page.keyboard.down('Control');
      await page.mouse.move(box.x + start.x, box.y + start.y);
      await page.mouse.down();
      await page.mouse.move(box.x + start.x + 90, box.y + start.y + 60, { steps: 8 });
      await page.mouse.up();
      await page.keyboard.up('Control');
      await waitForMapIdle(page);

      const centerAfter = await getMapCenter(page);
      expect(
        Math.abs(centerAfter.lng - centerBefore.lng) + Math.abs(centerAfter.lat - centerBefore.lat),
      ).toBeGreaterThan(0.0000001);
      expect(await getMultiSelectionKeys(page)).toEqual([]);
      expect(await getSelection(page)).toBeNull();
    });
  });

  test.describe('Edge Cases', () => {
    test('should handle window blur while Ctrl is held', async ({ page }) => {
      // Switch to select mode
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      // Hold Ctrl
      await page.keyboard.down('Control');
      await page.waitForFunction(() => {
        const canvas = document.querySelector('canvas');
        return canvas?.style.cursor === 'grab';
      });

      // Verify cursor is grab
      let cursor = await page.evaluate(() => {
        const canvas = document.querySelector('canvas');
        return canvas?.style.cursor || '';
      });
      expect(cursor).toBe('grab');

      // Simulate window blur
      await page.evaluate(() => window.dispatchEvent(new Event('blur')));
      await page.waitForFunction(() => {
        const canvas = document.querySelector('canvas');
        return canvas?.style.cursor !== 'grab';
      });

      // Cursor should no longer be grab (ctrlHeld reset)
      cursor = await page.evaluate(() => {
        const canvas = document.querySelector('canvas');
        return canvas?.style.cursor || '';
      });
      expect(cursor).not.toBe('grab');

      // Clean up
      await page.keyboard.up('Control');
    });

    test('should allow Ctrl+click for selection (not drag)', async ({ page }) => {
      // Switch to select mode
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      const canvas = page.getByTestId('map-canvas');
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Ctrl+click (no drag) should work without issues
      await page.keyboard.down('Control');
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);
      await page.keyboard.up('Control');

      // Map should remain functional
      await expect(canvas).toBeVisible();
    });

    test('should stop showing grab cursor when Ctrl released mid-drag', async ({ page }) => {
      // Switch to select mode
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      const canvas = page.getByTestId('map-canvas');
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Start Ctrl+drag
      await page.keyboard.down('Control');
      await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
      await page.mouse.down();
      await page.mouse.move(box!.x + box!.width / 2 + 50, box!.y + box!.height / 2);

      // Release Ctrl mid-drag
      await page.keyboard.up('Control');
      await page.waitForFunction(() => {
        const canvas = document.querySelector('canvas');
        return canvas?.style.cursor !== 'grab';
      });

      // Cursor should no longer be grab
      const cursor = await page.evaluate(() => {
        const canvas = document.querySelector('canvas');
        return canvas?.style.cursor || '';
      });
      expect(cursor).not.toBe('grab');

      // Clean up
      await page.mouse.move(box!.x + box!.width / 2 + 100, box!.y + box!.height / 2);
      await page.mouse.up();

      // Map should remain functional
      await expect(canvas).toBeVisible();
    });

    test('should work with rapid Ctrl press/release cycles', async ({ page }) => {
      // Switch to select mode
      await page.keyboard.press('s');
      await waitForActiveTool(page, 'select');

      const canvas = page.getByTestId('map-canvas');
      const box = await canvas.boundingBox();
      expect(box).not.toBeNull();

      // Rapid Ctrl press/release cycles
      for (let i = 0; i < 5; i++) {
        await page.keyboard.down('Control');
        await page.keyboard.up('Control');
      }

      // Map should remain functional
      await expect(canvas).toBeVisible();

      // Verify final cursor is default (not stuck on grab)
      const cursor = await page.evaluate(() => {
        const canvas = document.querySelector('canvas');
        return canvas?.style.cursor || '';
      });
      expect(cursor).not.toBe('grab');
    });

    // Both pointer listeners recover a swallowed release, and each is its own line of
    // defence: `pointerdown` covers reaching for the toolbar, `mousemove` covers simply
    // moving the mouse without clicking anything. Covering only one leaves the other
    // free to be deleted, so the case runs once per event.
    for (const recoveryEvent of ['pointerdown', 'mousemove'] as const) {
      test(`a Ctrl release the drag guard swallowed is cleared by the next ${recoveryEvent}`, async ({
        page,
      }) => {
        // The spurious-keyup guard suppresses any Ctrl keyup landing within 50ms of a
        // Ctrl+drag mouse event, so a quick genuine release is swallowed and no further
        // keyup is ever coming. Recovering only from later KEY events left the flag
        // latched for the commonest next action there is - reaching for the toolbar
        // with the mouse - which put the pan grab over the tool just chosen.
        await page.keyboard.press('s');
        await waitForActiveTool(page, 'select');

        await page.keyboard.down('Control');
        await page.waitForFunction(() => document.querySelector('canvas')?.style.cursor === 'grab');

        const canvas = page.getByTestId('map-canvas');
        const box = await canvas.boundingBox();
        expect(box).not.toBeNull();
        const cx = box!.x + box!.width / 2;
        const cy = box!.y + box!.height / 2;
        await page.mouse.move(cx, cy);
        await page.mouse.down();
        await page.mouse.move(cx + 60, cy + 60, { steps: 6 });

        // The release has to land INSIDE the suppression window or this test proves
        // nothing: the ordinary keyup path would clear the flag, leaving nothing latched
        // for the pointer listeners to recover, and they could be deleted with the test
        // still green. Two separate Playwright calls cannot guarantee that - each is its
        // own round trip, and the guard reads BOTH a `< 50ms` elapsed check and a flag
        // that a 50ms timer clears.
        //
        // So the mouseup and the release are dispatched in ONE task. The guard measures
        // from the mouseup it sees here, which makes the elapsed time zero, and the timer
        // that would clear the flag cannot run while this block is still executing. The
        // window is satisfied by construction rather than by luck.
        await page.evaluate(() => {
          window.dispatchEvent(new MouseEvent('mouseup', { bubbles: true, button: 0, ctrlKey: true }));
          window.dispatchEvent(new KeyboardEvent('keyup', { key: 'Control', bubbles: true }));
        });
        // Assert it WAS swallowed - the half that was missing, and the half that has to
        // read the LATCH rather than the cursor. `grab` and `grabbing` are also driven
        // by the drag itself, so a cursor assertion here passed even with both recovery
        // listeners deleted; `data-ctrl-held` is the belief the listeners correct.
        //
        // settle: this proves a state does NOT change and a poll would accept the
        // pre-update value. No pointer event may intervene: both recovery
        // listeners are exactly what the rest of the test exercises.
        await page.waitForTimeout(100);
        await expect(canvas).toHaveAttribute('data-ctrl-held', 'true');

        // Now the recovery, driven the way the listener contracts: an event reporting
        // `ctrlKey: false` is proof the key is up, whatever the swallowed release left
        // behind. It has to be dispatched rather than performed, because the release
        // above was synthetic - Playwright's own keyboard still holds Control, so every
        // REAL pointer event it sends honestly reports `ctrlKey: true` and the listener
        // is right to ignore it. Releasing Playwright's Control instead would send a
        // second, unsuppressed keyup and clear the flag through the keyboard path,
        // proving nothing about the pointer.
        await page.evaluate((type: string) => {
          // `PointerEvent` for both: it extends `MouseEvent`, so a `mousemove` built this
          // way still satisfies a `mousemove` listener, and one constructor keeps the two
          // cases identical apart from the name under test.
          window.dispatchEvent(new PointerEvent(type, { bubbles: true, ctrlKey: false }));
        }, recoveryEvent);
        await expect(canvas).toHaveAttribute('data-ctrl-held', 'false');

        // Release for real now that the claim is proved, so the toolbar click below runs
        // with the input state a user would have.
        await page.mouse.up();
        await page.keyboard.up('Control');

        // Switch tools BY MOUSE. Measure is standalone on the toolbar (the move tools
        // sit behind a group flyout).
        await page.getByTestId('tool-measure').click();
        await waitForActiveTool(page, 'measure');

        // Measure's own crosshair, not the stale pan grab.
        await expect
          .poll(() => page.evaluate(() => document.querySelector('canvas')?.style.cursor || ''))
          .toBe('crosshair');
      });
    }
  });
});

/**
 * The cases that need the Pan / Zoom tools themselves. They are off the toolbar
 * and their P/Z shortcuts are inert unless `map.controls.show_navigation_tools`
 * is on (#409), so this is a SEPARATE top-level describe: `seedNavigationTools`
 * registers an init script and has to run before `setupMockedPage`, which a
 * nested `beforeEach` cannot do (outer hooks run first, and the page has already
 * navigated by then).
 */
test.describe('Ctrl+Drag Panning - navigation tools enabled', () => {
  test.beforeEach(async ({ page }) => {
    await seedNavigationTools(page);
    await setupMockedPage(page);
    await createProjectWithMap(page);
  });

  // Unsupported tools (have their own drag behaviors that conflict with Ctrl+pan)
  // Note: zoom mode supports Ctrl+drag panning (added in box zoom feature), so it's not in this list
  const unsupportedTools = [
    { key: 'p', name: 'pan' },
  ];

  test.describe('Unsupported Tools', () => {
    for (const tool of unsupportedTools) {
      test(`should NOT change cursor to grab with Ctrl in ${tool.name} mode`, async ({ page }) => {
        // Switch to the tool
        await page.keyboard.press(tool.key);
        await waitForActiveTool(page, tool.name);

        const canvas = page.getByTestId('map-canvas');
        await expect(canvas).toBeVisible();

        // Hold Ctrl
        await page.keyboard.down('Control');
        await page.waitForFunction(() => {
          const canvas = document.querySelector('canvas');
          return typeof canvas?.style.cursor === 'string';
        });

        // Get the cursor
        const cursor = await page.evaluate(() => {
          const canvas = document.querySelector('canvas');
          return canvas?.style.cursor || '';
        });

        // Pan tool naturally shows 'grab', so that's expected
        // Other tools should NOT show grab cursor when Ctrl is held
        if (tool.name === 'pan') {
          expect(cursor).toBe('grab');
        } else {
          expect(cursor).not.toBe('grab');
        }

        await page.keyboard.up('Control');
      });
    }
  });

  /**
   * Why the custom pan handler still exists at all, pinned as a fact about
   * MapLibre rather than a comment: MapLibre's own dragPan REFUSES a Ctrl+left
   * press - `generateMousePanHandler` builds its state manager with
   * `checkCorrectEvent: (e) => mouseButton(e) === LEFT_BUTTON && !e.ctrlKey`
   * (maplibre-gl 5.16), and `DragHandler.dragStart` returns without arming when
   * that says no. Ctrl+left is MapLibre's rotate/pitch gesture, and this map
   * disables all of those, so under the Pan tool - the one tool the custom
   * handler deliberately keeps its hands off - a Ctrl+drag moves nothing.
   *
   * Consequence for every OTHER tool: handing Ctrl+drag to the enabled dragPan
   * would not pan either. The custom handler has to keep taking the gesture over
   * once it is a drag; what it must not do is take the CLICK with it.
   */
  test('MapLibre dragPan ignores a Ctrl+left drag (pan tool, no custom handler)', async ({ page }) => {
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');
    await waitForMapIdle(page);

    const canvas = page.getByTestId('map-canvas');
    const box = (await canvas.boundingBox())!;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    // Baseline: the SAME drag without Ctrl pans, so a null result below is about
    // the modifier and not about the drag never reaching the map.
    const plainBefore = await getMapCenter(page);
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 120, cy, { steps: 8 });
    await page.mouse.up();
    await waitForMapIdle(page);
    const plainAfter = await getMapCenter(page);
    expect(Math.abs(plainAfter.lng - plainBefore.lng)).toBeGreaterThan(0.00001);

    const ctrlBefore = await getMapCenter(page);
    await page.keyboard.down('Control');
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 120, cy, { steps: 8 });
    await page.mouse.up();
    await page.keyboard.up('Control');
    await waitForMapIdle(page);
    const ctrlAfter = await getMapCenter(page);

    expect(Math.abs(ctrlAfter.lng - ctrlBefore.lng)).toBeLessThan(1e-12);
    expect(Math.abs(ctrlAfter.lat - ctrlBefore.lat)).toBeLessThan(1e-12);
  });

  // Zoom mode supports Ctrl+drag panning (shows grab cursor when Ctrl is held)
  test('should show grab cursor with Ctrl in zoom mode for panning', async ({ page }) => {
    // Switch to zoom tool
    await page.keyboard.press('z');
    await waitForActiveTool(page, 'zoom');

    const canvas = page.locator('canvas').first();
    await expect(canvas).toBeVisible();

    // Hold Ctrl
    await page.keyboard.down('Control');

    // The cursor should settle on 'grab' for Ctrl+pan support
    await expect
      .poll(() => page.evaluate(() => document.querySelector('canvas')?.style.cursor || ''))
      .toBe('grab');

    await page.keyboard.up('Control');
  });

  test('should keep active tool while Ctrl is held and allow switch after release', async ({ page }) => {
    // Start in select mode
    await page.keyboard.press('s');
    await waitForActiveTool(page, 'select');

    // Hold Ctrl (pan modifier)
    await page.keyboard.down('Control');
    await page.waitForFunction(() => {
      const canvas = document.querySelector('canvas');
      return canvas?.style.cursor === 'grab';
    });

    // Verify cursor is grab while Ctrl is held
    let cursor = await page.evaluate(() => {
      const canvas = document.querySelector('canvas');
      return canvas?.style.cursor || '';
    });
    expect(cursor).toBe('grab');

    // Drag to pan while Ctrl is held (no tool switch expected)
    const canvas = page.getByTestId('map-canvas');
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();
    if (box) {
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width / 2 + 80, box.y + box.height / 2 + 80, { steps: 6 });
      await page.mouse.up();
    }

    // Active tool should remain select
    await waitForActiveTool(page, 'select');

    // Release Ctrl
    await page.keyboard.up('Control');

    // Now switch to pan tool explicitly (no modifier)
    await page.keyboard.press('p');
    await waitForActiveTool(page, 'pan');

    // Switch to zoom tool
    await page.keyboard.press('z');
    await waitForActiveTool(page, 'zoom');

    // Now cursor should be zoom-in
    cursor = await page.evaluate(() => {
      const canvas = document.querySelector('canvas');
      return canvas?.style.cursor || '';
    });
    expect(cursor).toBe('zoom-in');

    // Map should remain functional after all these switches
    await expect(canvas).toBeVisible();
  });
});
