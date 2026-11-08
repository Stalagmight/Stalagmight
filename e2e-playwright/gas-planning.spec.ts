import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithSections,
  deleteShotAt,
  waitForDrawnStations,
  waitForMapIdle,
  getStationScreenPositions,
  readCanvasPixel,
  readCanvasPixelBlock,
  selectDropdownOption,
  selectDropdownOptionAt,
  type PixelRgba,
  type SectionSeed,
} from './helpers/app';
import { GAS_NEAR, GAS_FAR } from '../src/utils/gasReach';
import {
  gasCylinderRows,
  gasMarkedLabelInputs,
  gasMarkedRemoveButtons,
  gasMarkedRows,
  gasWaypointDownButtons,
  gasWaypointRemoveButtons,
  gasWaypointRows,
  qualifiedIndexTestIds,
} from './helpers/testIds';

/**
 * E2E for the gas reachable-frontier overlay, now the Plan workspace mode (#38,
 * the workspace-mode reframe). Plan is entered from the top-middle Survey/Plan/Export carousel
 * (not the old toolbar toggle); it forces the read-only closure map, swaps the right
 * Dock for the gas setup sidebar, and the bottom Dock for the tabbed plan tables.
 *
 * The survey is drawn by the custom WebGL renderer, so the old overlay oracle is
 * gone: there is no `survey-stations` layer whose `circle-color` paint could be
 * checked for a `gasReach` feature-state interpolate, and no
 * `survey-data-stations` vector-tile source to read that feature-state from. The
 * renderer resolves the SAME ramp color per feature at build time
 * (surveyLayer/customRender/overlayColor.ts, byte-identical to the old
 * data-driven paint), so the oracle here is the PAINTED STATION PIXEL: "a station
 * carries a numeric gasReach >= 0" becomes "a station is painted a reachable
 * gold->purple ramp color, while the over-budget ones are painted grey".
 * `survey-gas-route` is still a stock layer, so its assertions are unchanged.
 * The IPC field is mocked (`compute_gas_reach_field`).
 */

async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

// A single section long enough that the mock's fixed budget (1500 L) leaves the
// far stations out of range - so the overlay has both reachable (gold→purple) and
// unreachable (grey) stations and a non-empty turn frontier.
const sectionSeeds: SectionSeed[] = [
  {
    name: 'Gas A',
    direction: 'in',
    // Positive depths (z = depth, positive below surface - the real convention), so the route
    // depth readouts show physically-sensible values a regression would visibly break.
    shots: Array.from({ length: 10 }, (_, i) => ({
      azimuth_in: 90,
      azimuth_out: 270,
      length: 10,
      depth_in: i,
      depth_out: i + 1,
    })),
  },
];

/**
 * Station indices the pixel probes use, from the mock's synthetic field: it puts
 * station `i` at `x = i * 10` and charges `|x| * 20` litres against a fixed
 * 1500 L budget, so stations 0..7 are reachable (7 -> 1400 L, the last one inside
 * the budget) and 8..10 are over budget. Of those, 8 stays CONNECTED-but-over-budget
 * (finite `gas_to_reach`, `reachable: false`) - beyond gas range: dimmed, clamped
 * to the ramp's FAR (purple) color, never the section color, never grey (#395,
 * revised). The mock's LAST station (10) is additionally patched to a null
 * `gas_to_reach` - genuinely topologically UNCONNECTED - the only case that still
 * renders grey.
 */
const LAST_REACHABLE = 7;
const BEYOND_RANGE = 8;
const UNCONNECTED = 10;

/**
 * Zoom bump for the pixel probes. +3 spreads the 11 stations to ~37 px apart (so a
 * 9 px footprint block is one station's) while keeping the whole 100 m survey on
 * the canvas; +4 pushes the far stations off-screen.
 */
const PROBE_ZOOM_DELTA = 3;

async function waitForMapLoaded(page: Page): Promise<void> {
  await expect
    .poll(
      async () =>
        page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { loaded: () => boolean } })
            .__stalagmight_map__;
          return map?.loaded() ?? false;
        }),
      { timeout: 10000 },
    )
    .toBe(true);
}

type StationRefLite = { section_id: number; station_index: number };

type E2EWindow = {
  __E2E_TEST__?: {
    uiStore: {
      getState: () => { appMode: string; activeTool: string; setActiveTool: (t: string) => void };
    };
    gasPlanStore: {
      getState: () => {
        origin: StationRefLite | null;
        endpoint: StationRefLite | null;
        waypoints: StationRefLite[];
        markedPoints: { station: StationRefLite; label: string | null }[];
        reachField: { turn_frontier: StationRefLite[] } | null;
      };
    };
  };
};

/**
 * A Plan context-menu entry, addressed by ROLE. The menu rows are buttons, and matching them by
 * text alone collides with prose that quotes the same label (the Origin hint names "Set as start
 * point" so the user can find it).
 */
function planMenuItem(page: Page, label: string) {
  return page.getByRole('button', { name: label, exact: true });
}

/** The gas plan's ordered way points (#298). */
async function gasWaypoints(page: Page): Promise<StationRefLite[]> {
  return page.evaluate(
    () => (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().waypoints ?? [],
  );
}

async function appMode(page: Page): Promise<string> {
  return page.evaluate(() => (window as unknown as E2EWindow).__E2E_TEST__?.uiStore.getState().appMode ?? '');
}

/** The gas plan's chosen route endpoint (null if unset). Shared so the endpoint-pick
 *  specs don't each re-spell the `__E2E_TEST__.gasPlanStore` cast. `?? null` keeps a
 *  `.not.toBeNull()` assertion honest - a missing hook reads null, not undefined. */
async function gasEndpoint(page: Page): Promise<StationRefLite | null> {
  return page.evaluate(
    () => (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().endpoint ?? null,
  );
}

async function activeTool(page: Page): Promise<string> {
  return page.evaluate(
    () => (window as unknown as E2EWindow).__E2E_TEST__?.uiStore.getState().activeTool ?? '',
  );
}

/** `#rrggbb` -> the `r,g,b,a` key the renderer packs it to (opaque). */
function colorKey(hex: string): string {
  const n = parseInt(hex.slice(1), 16);
  return `${(n >> 16) & 0xff},${(n >> 8) & 0xff},${n & 0xff},255`;
}

/** The near (gold) end of the reachable ramp: a station at gas cost 0 is painted
 *  exactly this (MapLibre's `interpolate` short-circuits the 0 stop, and
 *  `packRampColor` reproduces that). */
const GAS_NEAR_KEY = colorKey(GAS_NEAR);

/** The far (purple) end of the reachable ramp: a station at or beyond the turn
 *  budget clamps to this, at full alpha - the beyond-range station is the SAME
 *  hue, dimmed (blended with reduced alpha), so its painted pixel differs from
 *  this exact key while still reading "far ramp color" to the eye. */
const GAS_FAR_KEY = colorKey(GAS_FAR);

/** The theme's gray line, which both the stock ramp and the custom resolver use
 *  for "unreachable on this gas plan". Read from the live theme so a theme change
 *  can't silently turn this into a stale literal. */
async function greyKey(page: Page): Promise<string> {
  return colorKey(
    await page.evaluate(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--cc-gray-line').trim(),
    ),
  );
}

/** Absolute map zoom, so probes can be pinned to one calibrated view. */
async function mapZoom(page: Page): Promise<number> {
  return page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } })
      .__stalagmight_map__;
    return map?.getZoom() ?? 0;
  });
}

/**
 * Jump the map to an absolute zoom (no animation). Probing painted pixels needs a
 * zoomed-in view: at the fit-to-bounds zoom the seeded survey is only a few screen
 * px wide and every station disc overlaps its neighbours. Absolute (not `+= delta`)
 * so re-pinning after a mode switch or a rebuild can never compound and push the
 * far stations off the canvas.
 */
async function setMapZoom(page: Page, zoom: number): Promise<void> {
  await page.evaluate((z) => {
    const map = (window as unknown as { __stalagmight_map__?: { setZoom: (z: number) => void } })
      .__stalagmight_map__;
    map?.setZoom(z);
  }, zoom);
}

async function stationAt(page: Page, stationIndex: number): Promise<{ x: number; y: number }> {
  const stations = await getStationScreenPositions(page);
  const station = stations.find((s) => s.stationIndex === stationIndex);
  if (!station) throw new Error(`no drawn station ${stationIndex}`);
  return station;
}

/**
 * The 9x9 CSS-px block painted around one station, as a raw RGBA array.
 *
 * A block rather than the single center pixel because Plan draws the gas ROUTE
 * line over the survey, and it covers the center of every station it passes
 * through; the station's own overlay fill has to be read from the rest of its
 * disc. Station spacing at the calibrated zoom is ~37 px, so a 9 px block never
 * reaches a neighbour.
 */
async function stationBlock(page: Page, stationIndex: number): Promise<number[]> {
  const { x, y } = await stationAt(page, stationIndex);
  return readCanvasPixelBlock(page, x, y, 9);
}

/** The distinct `r,g,b,a` colors painted inside a station's footprint. */
async function stationColors(page: Page, stationIndex: number): Promise<Set<string>> {
  const buf = await stationBlock(page, stationIndex);
  const out = new Set<string>();
  for (let i = 0; i < buf.length; i += 4) {
    out.add(`${buf[i]},${buf[i + 1]},${buf[i + 2]},${buf[i + 3]}`);
  }
  return out;
}

async function stationPaints(page: Page, stationIndex: number, color: string): Promise<boolean> {
  return (await stationColors(page, stationIndex)).has(color);
}

/** The station's own fill in Survey mode, where nothing is drawn over it. */
async function stationCenter(page: Page, stationIndex: number): Promise<PixelRgba> {
  const { x, y } = await stationAt(page, stationIndex);
  return readCanvasPixel(page, x, y);
}

async function enterMode(page: Page, mode: 'survey' | 'plan' | 'export'): Promise<void> {
  await page.locator(`[data-testid="mode-${mode}"]`).click();
}

/**
 * R9/R10 (#395): answer Plan mode's entry question ("Where does this dive start?") with the
 * entrance, revealing the full rig. Idempotent - safe to call again after a mode round-trip,
 * since `originChosen` persists across mode switches (#244).
 */
async function chooseEntrance(page: Page): Promise<void> {
  const entranceButton = page.locator('[data-testid="gas-start-entrance"]');
  if (await entranceButton.isVisible().catch(() => false)) {
    await entranceButton.click();
  }
}

/** Enter Plan mode AND answer the entry question, for the (overwhelming majority of) tests that
 *  exercise the full rig rather than the entry card itself. */
async function enterPlanMode(page: Page): Promise<void> {
  await enterMode(page, 'plan');
  await chooseEntrance(page);
}

/** Enter Plan, add a stage, select + acknowledge the 1-stage strategy → one ▼ drop marker + route. */
async function setupOneStageDrop(page: Page): Promise<void> {
  await enterPlanMode(page);
  await expect(page.locator('[data-testid="cylinders-table"]')).toBeVisible({ timeout: 3000 });
  await selectDropdownOption(page, 'gas-add-tank', 'al80');
  await selectDropdownOptionAt(qualifiedIndexTestIds(page, 'gas-cylinder-role').last(), 'stage');
  const oneStage = page.locator('[data-testid="gas-reserve-one_stage"]');
  await expect(oneStage).toBeEnabled();
  await oneStage.click();
  await page.locator('[data-testid="gas-pushy-ack-confirm"]').click();
  await expect
    .poll(() => page.locator('[data-testid="gas-drop-marker"]').count(), { timeout: 3000 })
    .toBeGreaterThan(0);
}

/** The store's per-stage drop overrides (#246). */
async function stageDropOverrides(page: Page): Promise<Array<StationRefLite | null>> {
  return page.evaluate(
    () =>
      (
        window as unknown as {
          __E2E_TEST__?: {
            gasPlanStore: {
              getState: () => { stageDropOverrides: Array<StationRefLite | null> };
            };
          };
        }
      ).__E2E_TEST__?.gasPlanStore.getState().stageDropOverrides ?? [],
  );
}

/** The `endpoint` argument the LAST compute_gas_reach_field call was invoked with (#310). */
async function lastGasReachEndpoint(page: Page): Promise<StationRefLite | null> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ endpoint: StationRefLite | null }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1].endpoint : null;
  });
}

/** The `stage_drop_overrides` argument the LAST compute_gas_reach_field call was invoked with. */
async function lastGasReachOverrides(page: Page): Promise<Array<StationRefLite | null>> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ stageDropOverrides: Array<StationRefLite | null> }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1].stageDropOverrides : [];
  });
}

type PlanSegmentLite = { speed_m_per_min: number | null; on_scooter: boolean };

/** The `segments` argument the LAST compute_gas_reach_field call was invoked with (#317) - the
 *  wiring guard for the Segments table: a table that only re-rendered itself would leave this
 *  stale, since only `setSegments` (which nulls `reachField`) triggers a refetch. */
async function lastGasReachSegments(page: Page): Promise<PlanSegmentLite[]> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ segments: PlanSegmentLite[] }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1].segments : [];
  });
}

/** The `dpv` field the LAST compute_gas_reach_field call's `gasConfig` carried (#317) - proves a
 *  rig toggle reached the actual REQUEST, not just the client-side `hasDpvRig` flag the readout
 *  also gates on (that flag flips a render tick before any recompute round-trips). */
async function lastGasReachDpv(page: Page): Promise<unknown> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ dpv: unknown }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1].dpv : undefined;
  });
}

/** Enable a DPV rig from the Plan-mode bottom panel's "DPV" tab (#317, moved out of Settings;
 *  its own tab since the Equipment split). Requires Plan mode already entered with the bottom
 *  panel visible - unlike the old Settings dialog, there is no route to the rig editor before
 *  `plan-tables-panel` exists. */
async function enableDpvRig(page: Page): Promise<void> {
  await expect(page.locator('[data-testid="plan-tables-panel"]')).toBeVisible({ timeout: 3000 });
  await page.locator('[data-testid="plan-tab-dpv"]').click();
  await page.locator('[data-testid="toggle-dpv-enabled"]').click();
  await expect(page.locator('[data-testid="dpv-rig-fields"]')).toBeVisible();
}

/**
 * Edit one cell of a Plan-mode grid: open the overlay editor, replace the value, commit
 * (#403). Both plan tables are `SpreadsheetGrid` hosts now, so a cell is a div and the only
 * `<input>` in play is the grid's single shared editor - there is no per-cell field to
 * `.fill()`, and nothing commits until Enter (or a blur/navigate), unlike the per-keystroke
 * fields these tables used to render.
 */
async function editGridCell(
  page: Page,
  prefix: 'segment' | 'gas-cylinder',
  row: number,
  colKey: string,
  value: string
): Promise<void> {
  await page.getByTestId(`${prefix}-cell-${row}-${colKey}`).dblclick();
  const editor = page.getByTestId(`${prefix}-cell-editor`);
  await expect(editor).toBeVisible();
  await editor.fill(value);
  await editor.press('Enter');
  await expect(editor).toHaveCount(0);
}

test.describe('Gas planning', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Gas Planning Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapLoaded(page);
  });

  test('Plan mode shows the gas overlay (panel + painted ramp + legend); back to Survey resets @smoke', async ({
    page,
  }) => {
    expect(await appMode(page)).toBe('survey');
    await expect(page.locator('[data-testid="gas-panel"]')).toHaveCount(0);

    // Calibrate one absolute zoom for every probe in this test, and capture the
    // baseline: in Survey the stations are painted their section color and no part
    // of the gas ramp is on screen.
    await waitForDrawnStations(page, 1);
    const zoom = (await mapZoom(page)) + PROBE_ZOOM_DELTA;
    await setMapZoom(page, zoom);
    const grey = await greyKey(page);
    const sectionKey = (await stationCenter(page, LAST_REACHABLE)).join(',');
    expect(await stationPaints(page, LAST_REACHABLE, sectionKey)).toBe(true);
    expect(await stationPaints(page, 0, GAS_NEAR_KEY)).toBe(false);
    expect(await stationPaints(page, UNCONNECTED, grey)).toBe(false);

    await enterPlanMode(page);
    expect(await appMode(page)).toBe('plan');

    // The setup sidebar replaces the detail panel in the right Dock slot.
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    // Legend appears with the entrance origin.
    await expect(page.locator('[data-testid="gas-legend"]')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="gas-legend-origin"]')).toContainText('entrance');
    // The symbol key explains the dim (#395: beyond-range geometry is dimmed, not
    // marked with a turn-point dot anymore).
    await expect(page.locator('[data-testid="gas-legend-key"]')).toContainText('Beyond gas range');

    // The reachable ramp is populated, not an all-grey field: the origin station sits
    // at gas cost 0, the exact gold near end of the ramp. Polled (the field is
    // fetched over IPC, and entering Plan can re-fit the view) with the zoom
    // re-pinned first so the probe reads the calibrated view.
    await setMapZoom(page, zoom);
    await expect.poll(() => stationPaints(page, 0, GAS_NEAR_KEY), { timeout: 3000 }).toBe(true);

    // The last station inside the budget is painted an INTERIOR ramp color: not the
    // near end, not the unreachable grey, and no longer its section color.
    const lastReachable = await stationColors(page, LAST_REACHABLE);
    expect(lastReachable.has(sectionKey)).toBe(false);
    expect(lastReachable.has(GAS_NEAR_KEY)).toBe(false);
    expect(lastReachable.has(grey)).toBe(false);
    // The beyond-range station (connected, but past the budget) is clamped to the
    // ramp's FAR color, dimmed: distinct from the in-budget ramp pixels (near end
    // and its own section color), the plain undimmed far color, and grey (#395).
    const beyondRange = await stationColors(page, BEYOND_RANGE);
    expect(beyondRange.has(grey)).toBe(false);
    expect(beyondRange.has(GAS_NEAR_KEY)).toBe(false);
    expect(beyondRange.has(GAS_FAR_KEY)).toBe(false);
    expect(beyondRange.has(sectionKey)).toBe(false);
    // ...while the genuinely UNCONNECTED station still goes grey (the conservative
    // direction the renderer reserves for "not part of this cave section at all").
    const unconnected = await stationColors(page, UNCONNECTED);
    expect(unconnected.has(grey)).toBe(true);
    expect(unconnected.has(sectionKey)).toBe(false);

    // Back to Survey → panel + legend gone, section colors restored (the overlay is
    // appMode-gated; the gas store itself persists across mode switches, #244).
    await enterMode(page, 'survey');
    expect(await appMode(page)).toBe('survey');
    await expect(page.locator('[data-testid="gas-panel"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="gas-legend"]')).not.toBeVisible();
    await setMapZoom(page, zoom);
    await expect
      .poll(() => stationPaints(page, LAST_REACHABLE, sectionKey), { timeout: 3000 })
      .toBe(true);
    expect(await stationPaints(page, 0, GAS_NEAR_KEY)).toBe(false);
    expect(await stationPaints(page, UNCONNECTED, grey)).toBe(false);
  });

  test('R9/R10 (#395): Plan opens with the entry card - no legend, no ramp, no bottom panel until an origin is chosen @smoke', async ({
    page,
  }) => {
    await waitForDrawnStations(page, 1);
    const zoom = (await mapZoom(page)) + PROBE_ZOOM_DELTA;
    await setMapZoom(page, zoom);
    const sectionKey = (await stationCenter(page, LAST_REACHABLE)).join(',');

    await enterMode(page, 'plan');
    expect(await appMode(page)).toBe('plan');

    await expect(page.locator('[data-testid="gas-entry-card"]')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="gas-start-entrance"]')).toBeVisible();
    await expect(page.locator('[data-testid="gas-assumptions-summary"]')).toBeVisible();
    // No shading, no legend, no bottom tables panel before the question is answered.
    await expect(page.locator('[data-testid="gas-legend"]')).not.toBeVisible();
    await expect(page.locator('[data-testid="gas-legend-loading"]')).not.toBeVisible();
    await expect(page.locator('[data-testid="cylinders-table"]')).not.toBeVisible();
    await setMapZoom(page, zoom);
    // No leaked SECTION color either (#395 revision, owner feedback): the whole
    // survey renders the gas ramp's dim far tint while the origin is unanswered -
    // matching "beyond gas range" everywhere, so it's coherent with the gas
    // scheme instead of the section-color map underneath it. Not the plain
    // section color, not the exact undimmed far color (it's alpha-blended), and
    // not grey (grey is reserved for "unconnected", which has no meaning yet).
    const idleColors = await stationColors(page, LAST_REACHABLE);
    expect(idleColors.has(sectionKey)).toBe(false);
    expect(idleColors.has(GAS_FAR_KEY)).toBe(false);
    expect(idleColors.has(await greyKey(page))).toBe(false);

    // Positive check the negative-only assertions above cannot give us: capture the
    // idle-tint pixel now, and compare it against a station in a DIFFERENT reachability
    // bucket that is ALSO idle-tinted (LAST_REACHABLE would be gold once a ramp exists;
    // BEYOND_RANGE would be dimmed purple) - if idle painted anything but a uniform
    // color, this would catch it, unlike a set of "is not any of these N colors" checks
    // that can never fail against a wrong-but-unlisted 4th color.
    const idlePixelKey = (await stationCenter(page, LAST_REACHABLE)).join(',');
    expect((await stationCenter(page, BEYOND_RANGE)).join(',')).toBe(idlePixelKey);

    // Answering it reveals the full rig and the ramp.
    await chooseEntrance(page);
    await expect(page.locator('[data-testid="gas-entry-card"]')).toBeHidden();
    await expect(page.locator('[data-testid="gas-legend"]')).toBeVisible({ timeout: 3000 });
    await setMapZoom(page, zoom);
    await expect.poll(() => stationPaints(page, 0, GAS_NEAR_KEY), { timeout: 3000 }).toBe(true);
    // The idle tint and the post-origin "beyond range" dim are the SAME construction
    // (GAS_FAR blended at GAS_DIM_ALPHA), so once the ramp resolves, BEYOND_RANGE reads the
    // EXACT pixel the idle tint painted before an origin existed - the positive twin of the
    // negative checks above. Polled (the ramp resolves over IPC), then read once settled.
    await expect
      .poll(async () => (await stationCenter(page, BEYOND_RANGE)).join(','), { timeout: 3000 })
      .toBe(idlePixelKey);
  });

  test('R9/R10 (#395): "Pick on map" arms a one-shot origin pick, disarmed by Escape or a second press @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await waitForDrawnStations(page);
    await setMapZoom(page, (await mapZoom(page)) + PROBE_ZOOM_DELTA);
    await waitForDrawnStations(page);
    const canvasBox = await page.locator('canvas').first().boundingBox();
    expect(canvasBox).not.toBeNull();

    const pickButton = page.locator('[data-testid="gas-pick-origin"]');
    await expect(pickButton).toBeVisible();
    await expect(pickButton).toHaveAttribute('aria-pressed', 'false');

    // Arm, then a station click sets the ORIGIN, not the endpoint - even though one already
    // exists (the entrance default from `chooseEntrance`).
    await pickButton.click();
    await expect(pickButton).toHaveAttribute('aria-pressed', 'true');
    const pt = await stationAt(page, 4);
    await page.mouse.click(canvasBox!.x + pt.x, canvasBox!.y + pt.y);
    await expect
      .poll(
        () =>
          page.evaluate(
            () => (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().origin ?? null,
          ),
        { timeout: 2000 },
      )
      .toEqual({ section_id: 1, station_index: 4 });
    expect(await gasEndpoint(page)).toBeNull();
    // One-shot: disarmed after the pick.
    await expect(pickButton).toHaveAttribute('aria-pressed', 'false');

    // Arm again, then Escape disarms without picking.
    await pickButton.click();
    await expect(pickButton).toHaveAttribute('aria-pressed', 'true');
    await page.keyboard.press('Escape');
    await expect(pickButton).toHaveAttribute('aria-pressed', 'false');
    expect(
      await page.evaluate(
        () => (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().origin ?? null,
      ),
    ).toEqual({ section_id: 1, station_index: 4 });

    // Arm, then a second press disarms without picking.
    await pickButton.click();
    await expect(pickButton).toHaveAttribute('aria-pressed', 'true');
    await pickButton.click();
    await expect(pickButton).toHaveAttribute('aria-pressed', 'false');
  });

  test('Plan gates the staged reserve presets to a stage-carrying rig; readout reads as gas @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    expect(await appMode(page)).toBe('plan');
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });

    // Swim speed input is present, populated with the 10 m/min default (#236).
    await expect(page.locator('[data-testid="gas-panel"]')).toContainText('Swim speed');
    const swim = page.locator('[data-testid="gas-swim-speed-input"]');
    await expect(swim).toBeVisible();
    await expect(swim).toHaveValue(/^10(\.0)?$/);

    // Default rig is twin-AL80 back gas (ZERO stages) → the pushy presets are greyed (the
    // advanced gate); All-thirds stays available.
    await expect(page.locator('[data-testid="gas-reserve-all_thirds"]')).toBeEnabled();
    await expect(page.locator('[data-testid="gas-reserve-one_stage"]')).toBeDisabled();
    await expect(page.locator('[data-testid="gas-reserve-two_stage"]')).toBeDisabled();

    // The "max" readout is labelled as a gas volume, not a penetration distance.
    await expect(page.locator('[data-testid="gas-readout-max"]')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="gas-panel"]')).toContainText('Gas to farthest point');
    // The strategy is "Gas strategy" with the back-gas turn-pressure gauge (All-thirds default).
    await expect(page.locator('[data-testid="gas-panel"]')).toContainText('Gas strategy');
    await expect(page.locator('[data-testid="gas-panel"]')).toContainText('Backgas turnaround pressure');
    await expect(page.locator('[data-testid="gas-readout-backgas-turn"]')).toBeVisible();
  });

  test('Plan readout shows the route distance + max/avg depth once a route resolves @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    expect(await appMode(page)).toBe('plan');
    // Entering Plan resolves a route to the deepest reachable station, so the distance + depth
    // rows appear, formatted with a length unit (#236). Values come straight from the backend.
    const distance = page.locator('[data-testid="gas-readout-route-distance"]');
    await expect(distance).toBeVisible({ timeout: 3000 });
    await expect(distance).toContainText('m');
    await expect(page.locator('[data-testid="gas-panel"]')).toContainText('Distance to turnaround');
    // The Max / Average depth rows render once a route is known. This asserts the WIRING only: the
    // e2e mock's get_project_state hardcodes station z = 0 (it doesn't derive depth from shot data),
    // so these readouts show "0.0 m" here regardless of the seed - there's no meaningful depth value
    // to assert at the mock level. The exact distance/depth math (tie-exclusion, deepest-point, sign)
    // is pinned in the Rust unit tests (gas/tests.rs).
    await expect(page.locator('[data-testid="gas-readout-max-depth"]')).toBeVisible();
    await expect(page.locator('[data-testid="gas-readout-avg-depth"]')).toBeVisible();
  });

  test('Profile tab draws the planned dive profile and switches axis (#299)', async ({ page }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="plan-tables-panel"]')).toBeVisible({ timeout: 3000 });
    await page.locator('[data-testid="plan-tab-profile"]').click();

    // Entering Plan resolves a route, so the chart has something to draw - not the empty pane.
    const svg = page.locator('[data-testid="plan-profile-svg"]');
    await expect(svg).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="plan-profile-empty"]')).toHaveCount(0);
    await expect(svg).toHaveAttribute('data-axis', 'distance');

    // A real polyline, and the turnaround is marked. Asserted on the path DATA, not just presence:
    // a chart whose projection divided by a zero span would still render an <svg> element, just an
    // empty or NaN-filled one. (The mock hardcodes station z = 0, so the profile here is flat by
    // construction - the depth math itself is pinned in the Rust tests.)
    const d = await page.locator('[data-testid="plan-profile-line"]').getAttribute('d');
    expect(d).toBeTruthy();
    expect(d).not.toContain('NaN');
    expect((d ?? '').match(/[ML]/g)?.length ?? 0).toBeGreaterThan(1);
    await expect(page.locator('[data-testid="plan-profile-marker-turn"]')).toBeVisible();

    // The axis toggle is a real re-projection, and it is reflected in aria-pressed for a11y.
    await page.locator('[data-testid="plan-profile-axis-time"]').click();
    await expect(svg).toHaveAttribute('data-axis', 'time');
    await expect(page.locator('[data-testid="plan-profile-axis-time"]')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    const timePath = await page.locator('[data-testid="plan-profile-line"]').getAttribute('d');
    expect(timePath).not.toContain('NaN');

    // Switching tabs away and back keeps the chart (the tab state is local + transient).
    await page.locator('[data-testid="plan-tab-cylinders"]').click();
    await expect(page.locator('[data-testid="cylinders-table"]')).toBeVisible();
    await page.locator('[data-testid="plan-tab-profile"]').click();
    await expect(page.locator('[data-testid="plan-profile-svg"]')).toBeVisible();
  });

  test('Profile exports carry the plan inputs, not a client-held field (#299)', async ({ page }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="plan-tables-panel"]')).toBeVisible({ timeout: 3000 });
    await page.locator('[data-testid="plan-tab-profile"]').click();
    await expect(page.locator('[data-testid="plan-profile-svg"]')).toBeVisible({ timeout: 3000 });

    // Copy renders server-side and lands on the clipboard.
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    const copy = page.locator('[data-testid="plan-profile-copy"]');
    await expect(copy).toBeEnabled();
    await copy.click();
    // What this proves is the WIRING - button to IPC to clipboard - and nothing more. The text
    // itself comes from the mock, so asserting the disclaimer here would pass even if the real
    // renderer dropped it; `both_formats_say_this_is_not_a_deco_schedule` (Rust) is that guard.
    // Asserted on the table header the mock and the renderer both emit.
    const clipboard = await page.evaluate(() => navigator.clipboard.readText());
    expect(clipboard).toContain('Dist(m)');
    // The "Copied" badge self-clears after 2 s, which is INSIDE Playwright's 5 s poll window - a
    // slow machine could start polling after it has gone and then burn the full timeout. Bound the
    // wait below the badge's own lifetime so a miss reports promptly instead of flaking late.
    await expect(copy).toContainText('Copied', { timeout: 1500 });

    // The XML export sends the plan INPUTS - the backend recomputes the reach field from them
    // rather than trusting whatever the UI is holding, so a request missing them would silently
    // export a route the diver has already changed. The mock's save dialog returns a path.
    await page.locator('[data-testid="plan-profile-export-xml"]').click();
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              ((window as unknown as Record<string, unknown>)
                .__MOCK_DIVE_PROFILE_EXPORTS__ as unknown[] | undefined)?.length ?? 0,
          ),
        { timeout: 5000 },
      )
      .toBe(1);
    const sent = await page.evaluate(
      () =>
        (
          (window as unknown as Record<string, unknown>).__MOCK_DIVE_PROFILE_EXPORTS__ as Array<{
            path: string;
            format: string;
            request: Record<string, unknown>;
          }>
        )[0],
    );
    expect(sent.format).toBe('subsurface-xml');
    expect(sent.path.endsWith('.xml')).toBe(true);
    // The whole input tuple, not just the endpoints. `segments` joined in #317: without them an
    // exported profile would be priced at the config swim speed for a scootered dive.
    expect(Object.keys(sent.request).sort()).toEqual([
      'cylinders',
      'diver',
      'endpoint',
      'gas_config',
      'origin',
      'segments',
      'stage_drop_overrides',
      'waypoints',
    ]);
    expect(Array.isArray(sent.request.cylinders)).toBe(true);
  });

  test('Plan swim speed persists to the saved config @smoke', async ({ page }) => {
    await enterPlanMode(page);
    const swim = page.locator('[data-testid="gas-swim-speed-input"]');
    await expect(swim).toBeVisible({ timeout: 3000 });
    // Editing swim speed writes back to the saved config (#236) - unlike the panel's other
    // per-session inputs - so it sticks across Plan exit / restart and stays in sync with Settings.
    await swim.fill('14');
    await swim.blur();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (
              window as unknown as {
                __E2E_TEST__: {
                  projectStore: {
                    getState: () => { config: { gas: { swim_speed_m_per_min: number } } | null };
                  };
                };
              }
            ).__E2E_TEST__.projectStore.getState().config?.gas.swim_speed_m_per_min,
        ),
      )
      .toBe(14);
  });

  test('Display menu is in Survey and Plan (not Export), without the survey-only geometry toggle @smoke', async ({
    page,
  }) => {
    const openMenu = async () => {
      await page.locator('[data-testid="display-menu-button"]').click();
      await expect(page.locator('[data-testid="display-menu"]')).toBeVisible();
    };
    const closeMenu = async () => {
      await page.locator('[data-testid="display-menu-button"]').click();
      await expect(page.locator('[data-testid="display-menu"]')).toHaveCount(0);
    };

    // Survey: the full menu - visibility toggles + the geometry toggle + the color overlays.
    await openMenu();
    await expect(page.locator('[data-testid="display-toggle-showStations"]')).toBeVisible();
    await expect(page.locator('[data-testid="geometry-mode-measured"]')).toBeVisible();
    await expect(page.locator('[data-testid="display-color-section"]')).toBeVisible();
    await closeMenu();

    // Plan: the visibility toggles remain, but the geometry toggle (closure map is forced) and the
    // color overlays (the gas gradient owns the coloring) are gone.
    await enterPlanMode(page);
    await openMenu();
    await expect(page.locator('[data-testid="display-toggle-showStations"]')).toBeVisible();
    await expect(page.locator('[data-testid="geometry-mode-measured"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="display-color-section"]')).toHaveCount(0);
    await closeMenu();

    // Export: the Display menu is gone entirely - the Export panel's own controls decide what
    // the exported image contains, so a survey-style Display menu would be a confusing no-op.
    await enterMode(page, 'export');
    await expect(page.locator('[data-testid="display-menu-button"]')).toHaveCount(0);
  });

  test('adding a stage enables a pushy preset behind the advanced acknowledgment @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="cylinders-table"]')).toBeVisible({ timeout: 3000 });

    // Default twin-AL80 rig has no stages → 1-stage is greyed and selecting it is impossible.
    await expect(page.locator('[data-testid="gas-reserve-one_stage"]')).toBeDisabled();

    // Add a cylinder and set its role to Stage → the rig now carries one stage.
    await selectDropdownOption(page, 'gas-add-tank', 'al80');
    await selectDropdownOptionAt(qualifiedIndexTestIds(page, 'gas-cylinder-role').last(), 'stage');

    // 1-stage is now selectable, but clicking it raises the advanced-acknowledgment gate
    // (it must NOT apply until acknowledged).
    const oneStage = page.locator('[data-testid="gas-reserve-one_stage"]');
    await expect(oneStage).toBeEnabled();
    await oneStage.click();
    await expect(page.locator('[data-testid="gas-pushy-ack"]')).toBeVisible();

    // Confirming applies the strategy (selected ring) and dismisses the gate.
    await page.locator('[data-testid="gas-pushy-ack-confirm"]').click();
    await expect(page.locator('[data-testid="gas-pushy-ack"]')).toHaveCount(0);
    await expect(oneStage).toHaveClass(/ring-accent/);

    // The staged strategy refetches a field with a route → a ▼ drop marker + the ◉ final-turn
    // marker render, and the readout gains the per-stage drop gauge.
    await expect
      .poll(() => page.locator('[data-testid="gas-drop-marker"]').count(), { timeout: 3000 })
      .toBeGreaterThan(0);
    await expect
      .poll(() => page.locator('[data-testid="gas-final-turn-marker"]').count(), { timeout: 3000 })
      .toBeGreaterThan(0);
    await expect(page.locator('[data-testid="gas-panel"]')).toContainText('Stage 1 drop');
    // The per-stage reserve input appears for a staged strategy (default 30 bar).
    await expect(page.locator('[data-testid="gas-stage-reserve-input"]')).toBeVisible();
  });

  test('a cylinder can be disabled - the row dims and it drops out of the plan (#245) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="cylinders-table"]')).toBeVisible({ timeout: 3000 });
    const firstRow = gasCylinderRows(page).first();
    const toggle = qualifiedIndexTestIds(page, 'gas-cylinder-enabled').first();
    await expect(toggle).toBeChecked();
    // Pin the BEFORE state: a carried back-gas tank is undimmed, so the class below is a real
    // state change and not something every row happens to carry.
    await expect(firstRow).not.toHaveClass(/opacity-50/);
    await toggle.uncheck();
    // The row shows its disabled state, and the store carries the flag through to the engine.
    // On the grid (#403) that state is the dim class the row carries, not a `data-enabled`
    // attribute a grid row has no way to hold.
    await expect(firstRow).toHaveClass(/opacity-50/);
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              (
                window as unknown as {
                  __E2E_TEST__: {
                    gasPlanStore: { getState: () => { cylinders: Array<{ enabled: boolean }> } };
                  };
                }
              ).__E2E_TEST__.gasPlanStore.getState().cylinders[0].enabled,
          ),
        { timeout: 2000 },
      )
      .toBe(false);
  });

  test('disabling the only stage drops the staged reserve control (UI/backend parity #245) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="cylinders-table"]')).toBeVisible({ timeout: 3000 });
    // Carry one stage and select the (acked) 1-stage strategy → the stage-reserve control shows.
    await selectDropdownOption(page, 'gas-add-tank', 'al80');
    await selectDropdownOptionAt(qualifiedIndexTestIds(page, 'gas-cylinder-role').last(), 'stage');
    await page.locator('[data-testid="gas-reserve-one_stage"]').click();
    await page.locator('[data-testid="gas-pushy-ack-confirm"]').click();
    await expect(page.locator('[data-testid="gas-stage-reserve-input"]')).toBeVisible();

    // Disable that stage. The engine counts only ENABLED stages, so it falls back to All-thirds;
    // the panel must mirror that - the staged-only reserve control disappears and the 1-stage
    // button greys out (before the fix, stageCount counted the disabled stage and both stayed live).
    await qualifiedIndexTestIds(page, 'gas-cylinder-enabled').last().uncheck();
    await expect(page.locator('[data-testid="gas-stage-reserve-input"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="gas-reserve-one_stage"]')).toBeDisabled();
  });

  test('the Free strategy makes the usable-gas column editable per cylinder (#245) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="cylinders-table"]')).toBeVisible({ timeout: 3000 });
    // Under a reserve strategy the usable column is READ-ONLY (engine-computed).
    await expect(qualifiedIndexTestIds(page, 'gas-cylinder-usable-readonly').first()).toBeVisible();
    await expect(qualifiedIndexTestIds(page, 'gas-cylinder-usable')).toHaveCount(0);

    // Free is advanced (manual reserves, incl. zero-reserve), so it routes through the advanced
    // acknowledgment the first time - then flips the column to an editable cell per budget-role
    // tank (the `-usable-N` / `-usable-readonly-N` test ids name which mode the row is in).
    await page.locator('[data-testid="gas-reserve-free"]').click();
    await expect(page.locator('[data-testid="gas-pushy-ack"]')).toBeVisible();
    await page.locator('[data-testid="gas-pushy-ack-confirm"]').click();
    await expect(page.locator('[data-testid="gas-reserve-free"]')).toHaveClass(/ring-accent/);
    const usable = qualifiedIndexTestIds(page, 'gas-cylinder-usable').first();
    await expect(usable).toBeVisible();

    // Typing a usable value stores it on the cylinder (metric mock ⇒ bar 1:1).
    const usableOf = (i: number) =>
      page.evaluate(
        (idx) =>
          (
            window as unknown as {
              __E2E_TEST__: {
                gasPlanStore: { getState: () => { cylinders: Array<{ usable_gas_bar: number }> } };
              };
            }
          ).__E2E_TEST__.gasPlanStore.getState().cylinders[idx].usable_gas_bar,
        i,
      );
    await editGridCell(page, 'gas-cylinder', 0, 'usable', '80');
    await expect(usable).toHaveText('80');
    await expect.poll(() => usableOf(0), { timeout: 2000 }).toBe(80);

    // A diver must be able to zero out a tank's usable in Free (#245 bug: the field rejected 0;
    // on the grid the cell's own `parse` keeps an emptied/zeroed cell writing a real 0).
    await editGridCell(page, 'gas-cylinder', 0, 'usable', '0');
    await expect(usable).toHaveText('0');
    await expect.poll(() => usableOf(0), { timeout: 2000 }).toBe(0);
  });

  test('readout: no Turn points row; backgas label switches; stage drops show distance (#245) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    const readout = page.locator('[data-testid="gas-readout"]');
    await expect(readout).toBeVisible({ timeout: 3000 });
    // "Turn points" row is removed.
    await expect(readout).not.toContainText('Turn points');
    // All-thirds (default, no stages): the back-gas gauge reads "turn pressure".
    await expect(readout).toContainText('Backgas turnaround pressure');
    await expect(readout).not.toContainText('Backgas switch pressure');

    // Add a stage + select the staged strategy → the gauge flips to "switch pressure", and the
    // stage-drop row shows both a pressure and a distance ("… @ …m").
    await selectDropdownOption(page, 'gas-add-tank', 'al80');
    await selectDropdownOptionAt(qualifiedIndexTestIds(page, 'gas-cylinder-role').last(), 'stage');
    await page.locator('[data-testid="gas-reserve-one_stage"]').click();
    await page.locator('[data-testid="gas-pushy-ack-confirm"]').click();
    await expect(readout).toContainText('Backgas switch pressure');
    await expect(readout).not.toContainText('Backgas turnaround pressure');
    await expect(page.locator('[data-testid="gas-readout-stage1-drop"]')).toContainText('@');
  });

  test('All-thirds safety margin input appears only in thirds and round-trips to config (#245) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    // The margin input shows only for All-thirds (the default); the staged reserve input does not.
    const margin = page.locator('[data-testid="gas-safety-margin-input"]');
    await expect(margin).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="gas-stage-reserve-input"]')).toHaveCount(0);

    // The mock budget is a fixed 1500 regardless of margin (cylinders are ignored there), so assert
    // the input round-trips into the config instead - the budget math is unit-tested in Rust.
    await margin.fill('20');
    await margin.blur();
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              (
                window as unknown as {
                  __E2E_TEST__: {
                    gasPlanStore: {
                      getState: () => {
                        gasConfig: { reserve_profile: { all_thirds_safety_margin_bar: number } };
                      };
                    };
                  };
                }
              ).__E2E_TEST__.gasPlanStore.getState().gasConfig.reserve_profile
                .all_thirds_safety_margin_bar,
          ),
        { timeout: 2000 },
      )
      .toBe(20);
  });

  test('Plan right-click offers gas route picks, not the survey menu; Set as endpoint stores it @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });

    // Open the station context menu programmatically (the right-click hit-test itself is covered by
    // survey-mode specs). In Plan it must REPLACE the survey mutation menu with the gas route picks.
    await page.evaluate(() => {
      const e2e = (
        window as unknown as {
          __E2E_TEST__: {
            projectStore: { getState: () => { project: { sections: Array<{ id: number }> } } };
            uiStore: { getState: () => { openDialog: (d: string, data: unknown) => void } };
          };
        }
      ).__E2E_TEST__;
      const sec = e2e.projectStore.getState().project.sections[0];
      e2e.uiStore.getState().openDialog('context-menu', {
        target: { type: 'station', ref: { section_id: sec.id, station_index: 1 } },
        position: { x: 220, y: 220 },
      });
    });

    // The Plan menu is the route-picking menu (#298).
    await expect(planMenuItem(page, 'Set as endpoint')).toBeVisible();
    await expect(planMenuItem(page, 'Set as start point')).toBeVisible();
    await expect(planMenuItem(page, 'Set as way point')).toBeVisible();
    await expect(planMenuItem(page, 'Set marked point')).toBeVisible();
    // No stage drop entry on the default all-thirds rig: the backend returns no drops for it, so
    // offering a slot the engine would ignore would be a lie.
    await expect(page.getByRole('button', { name: /Set as stage/ })).toHaveCount(0);
    // The survey mutation menu must NOT appear (the read-only bug): no "Edit Station".
    await expect(page.getByText('Edit Station')).toHaveCount(0);

    // Picking the endpoint stores it in the gas plan store and closes the menu.
    await planMenuItem(page, 'Set as endpoint').click();
    await expect(planMenuItem(page, 'Set as endpoint')).toHaveCount(0);
    expect(await gasEndpoint(page)).not.toBeNull();
  });

  /** Open the Plan right-click menu for `stationIndex` of the first section. */
  async function openPlanMenu(page: Page, stationIndex: number): Promise<void> {
    await page.evaluate((idx) => {
      const e2e = (
        window as unknown as {
          __E2E_TEST__: {
            projectStore: { getState: () => { project: { sections: Array<{ id: number }> } } };
            uiStore: { getState: () => { openDialog: (d: string, data: unknown) => void } };
          };
        }
      ).__E2E_TEST__;
      const sec = e2e.projectStore.getState().project.sections[0];
      e2e.uiStore.getState().openDialog('context-menu', {
        target: { type: 'station', ref: { section_id: sec.id, station_index: idx } },
        position: { x: 220, y: 220 },
      });
    }, stationIndex);
  }

  test('a plain left-click on a station sets the ENDPOINT, not the origin @smoke', async ({
    page,
  }) => {
    // #298 flipped this: a map click used to re-root the origin. Picking a destination is the
    // frequent action, and the origin is nearly always the entrance. #395: this only holds once
    // an origin exists at all - answer the entry question first.
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await waitForDrawnStations(page);
    // Zoom in the same way the pixel probes do: at the fit-to-bounds zoom the 11 stations are only
    // a few px apart and a click lands ambiguously between discs.
    await setMapZoom(page, (await mapZoom(page)) + PROBE_ZOOM_DELTA);
    await waitForDrawnStations(page);
    // `stationAt` is CANVAS-relative (it feeds the pixel probes); the mouse works in viewport
    // coordinates, so the canvas origin has to be added or the click lands above the map.
    const pt = await stationAt(page, 3);
    const canvasBox = await page.locator('canvas').first().boundingBox();
    expect(canvasBox).not.toBeNull();
    await page.mouse.click(canvasBox!.x + pt.x, canvasBox!.y + pt.y);

    await expect.poll(() => gasEndpoint(page), { timeout: 2000 }).not.toBeNull();
    // The origin must be untouched (still the entrance default) - the whole point of the change.
    expect(
      await page.evaluate(
        () => (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().origin ?? null,
      ),
    ).toBeNull();
  });

  test('before an origin is chosen, a plain left-click sets the ORIGIN @smoke', async ({
    page,
  }) => {
    // R9/R10 (#395): the entry question needs an answer before a click can mean "endpoint" -
    // the first click IS the answer.
    await enterMode(page, 'plan');
    await expect(page.locator('[data-testid="gas-entry-card"]')).toBeVisible({ timeout: 3000 });
    await waitForDrawnStations(page);
    await setMapZoom(page, (await mapZoom(page)) + PROBE_ZOOM_DELTA);
    await waitForDrawnStations(page);
    const pt = await stationAt(page, 3);
    const canvasBox = await page.locator('canvas').first().boundingBox();
    expect(canvasBox).not.toBeNull();
    await page.mouse.click(canvasBox!.x + pt.x, canvasBox!.y + pt.y);

    await expect.poll(
      () =>
        page.evaluate(
          () => (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().origin ?? null,
        ),
      { timeout: 2000 },
    ).toEqual({ section_id: 1, station_index: 3 });
    expect(await gasEndpoint(page)).toBeNull();
    // Answering the question reveals the full rig.
    await expect(page.locator('[data-testid="gas-entry-card"]')).toBeHidden();
    await expect(page.locator('[data-testid="gas-origin-readout"]')).toBeVisible();
  });

  test('#397: before an origin is chosen, a plain left-click on a SHOT sets the origin at the endpoint nearer the entrance', async ({
    page,
  }) => {
    // Zero-datum at the FAR end (station 10) reverses "nearer the entrance" from the trivial
    // lower-index case, so a correct pick proves the entrance-distance rule ran, rather than the
    // pre-#397 bug of always anchoring to the shot's lower-indexed (`from`) station.
    const secId = await page.evaluate(async () => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: { sections: Array<{ id: number }> } | null;
              setZeroDatum: (
                ref: { section_id: number; station_index: number },
                gps: { latitude: number; longitude: number; depth_reference: number },
              ) => Promise<unknown>;
              refreshProjectState: () => Promise<void>;
            };
          };
        };
      }).__E2E_TEST__;
      const ps = e2e!.projectStore.getState();
      const secId = ps.project!.sections[0].id;
      await ps.setZeroDatum(
        { section_id: secId, station_index: 10 },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 },
      );
      await ps.refreshProjectState();
      return secId;
    });

    // `setZeroDatum` triggers `triggerFitToBounds` (projectStore.ts), which re-fits the camera
    // asynchronously - wait for that request to be consumed before applying the probe zoom, or
    // the fit-to-bounds can land AFTER our zoom and silently zoom back out.
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              (window as unknown as {
                __E2E_TEST__?: { uiStore: { getState: () => { requestFitToBounds: unknown } } };
              }).__E2E_TEST__?.uiStore.getState().requestFitToBounds ?? false,
          ),
        { timeout: 3000 },
      )
      .toBe(false);

    await enterMode(page, 'plan');
    await expect(page.locator('[data-testid="gas-entry-card"]')).toBeVisible({ timeout: 3000 });
    await waitForDrawnStations(page);
    // Let the datum-triggered fit-to-bounds fully settle BEFORE reading the zoom to bump - a
    // mid-animation read would bake a transient value into `targetZoom`, and the later
    // `waitForMapIdle` calls would then just settle onto that (wrong) target.
    await waitForMapIdle(page);
    const targetZoom = (await mapZoom(page)) + PROBE_ZOOM_DELTA;
    await setMapZoom(page, targetZoom);
    await waitForMapIdle(page);
    await waitForDrawnStations(page);

    // Stations near the CENTER of the survey (not the edges): a +3 zoom is centered on the
    // bounds midpoint, so an edge station's screen position can end up off-canvas.
    const a = await stationAt(page, 4);
    const b = await stationAt(page, 5);
    const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
    const canvasBox = await page.locator('canvas').first().boundingBox();
    expect(canvasBox).not.toBeNull();
    await page.mouse.click(canvasBox!.x + mid.x, canvasBox!.y + mid.y);

    // Station 5 is nearer the (far-end, station-10) entrance than station 4, so the click
    // anchors there - not the shot's lower-indexed endpoint.
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().origin ?? null,
          ),
        { timeout: 2000 },
      )
      .toEqual({ section_id: secId, station_index: 5 });
  });

  test('#397: once an origin is chosen, a plain left-click on a SHOT sets the endpoint', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await waitForDrawnStations(page);
    await setMapZoom(page, (await mapZoom(page)) + PROBE_ZOOM_DELTA);
    await waitForDrawnStations(page);

    const a = await stationAt(page, 5);
    const b = await stationAt(page, 6);
    const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
    const canvasBox = await page.locator('canvas').first().boundingBox();
    expect(canvasBox).not.toBeNull();
    await page.mouse.click(canvasBox!.x + mid.x, canvasBox!.y + mid.y);

    await expect.poll(() => gasEndpoint(page), { timeout: 2000 }).not.toBeNull();
    // The origin must be untouched (still the entrance default), matching the station-click case.
    expect(
      await page.evaluate(
        () => (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().origin ?? null,
      ),
    ).toBeNull();
  });

  test('Set as start point sets the ORIGIN, not the endpoint (#298)', async ({ page }) => {
    await enterMode(page, 'plan');
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await openPlanMenu(page, 2);
    await planMenuItem(page, 'Set as start point').click();
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().origin ?? null,
          ),
        { timeout: 2000 },
      )
      .toEqual({ section_id: 1, station_index: 2 });
    // The origin pick must NOT double as an endpoint pick - they are separate route roles.
    expect(await gasEndpoint(page)).toBeNull();
  });

  test('#310: a live endpoint pin follows its physical station across an upstream shot delete', async ({
    page,
  }) => {
    await enterMode(page, 'plan');
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });

    // Station 8 of the 11-station seed (10 shots): before = shot[7], after = shot[8].
    await openPlanMenu(page, 8);
    await planMenuItem(page, 'Set as endpoint').click();
    await expect
      .poll(() => gasEndpoint(page), { timeout: 2000 })
      .toEqual({ section_id: 1, station_index: 8 });

    // Deleting shot 0 (upstream of station 8) shifts every later station down by one, so the
    // SAME physical station - once station 8 - is now station 7. A positional ref that is
    // never re-anchored stays the literal `station_index: 8`, which after this edit names a
    // DIFFERENT physical station (the one that used to be station 9) - the #310 bug: the pin
    // silently retargets instead of following its station or dropping out.
    const WRONG_OLD_BEHAVIOUR_INDEX = 8;
    const CORRECT_INDEX = 7;

    await enterMode(page, 'survey');
    await deleteShotAt(page, 0);
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });

    // Discriminating assertion: the corrected ref must reach the engine, and it must NOT be
    // the stale literal index - which remains a perfectly valid (if wrong) station in the
    // shrunken 10-station project, so a regression here would still produce SOME endpoint,
    // just the wrong one, and pass a weaker "is not null" check.
    await expect
      .poll(() => lastGasReachEndpoint(page), { timeout: 3000 })
      .toEqual({ section_id: 1, station_index: CORRECT_INDEX });
    const sent = await lastGasReachEndpoint(page);
    expect(sent?.station_index).not.toBe(WRONG_OLD_BEHAVIOUR_INDEX);
  });

  test('way points: add, reorder, remove - order reaches the engine @smoke', async ({ page }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    // Empty state first, so the list rows below are proof of the add and not pre-existing.
    await expect(page.locator('[data-testid="gas-waypoints-empty"]')).toBeVisible();

    await openPlanMenu(page, 2);
    await planMenuItem(page, 'Set as way point').click();
    await openPlanMenu(page, 4);
    await planMenuItem(page, 'Set as way point').click();

    // Click order IS visit order.
    await expect.poll(() => gasWaypoints(page), { timeout: 2000 }).toEqual([
      { section_id: 1, station_index: 2 },
      { section_id: 1, station_index: 4 },
    ]);
    await expect(gasWaypointRows(page)).toHaveCount(2);
    // One numbered pin per way point on the map.
    await expect(page.locator('[data-testid="gas-waypoint-marker"]')).toHaveCount(2);

    // The engine receives them, in order - this is a gas input, not a drawing hint.
    const sent = await page.evaluate(
      () =>
        (
          (window as unknown as Record<string, unknown>).__MOCK_GAS_REACH_CALLS__ as Array<{
            waypoints: StationRefLite[];
          }>
        ).at(-1)?.waypoints ?? [],
    );
    expect(sent).toEqual([
      { section_id: 1, station_index: 2 },
      { section_id: 1, station_index: 4 },
    ]);

    // Reordering is a real change of plan, so it must actually swap the list.
    await gasWaypointDownButtons(page).first().click();
    await expect.poll(() => gasWaypoints(page), { timeout: 2000 }).toEqual([
      { section_id: 1, station_index: 4 },
      { section_id: 1, station_index: 2 },
    ]);

    await gasWaypointRemoveButtons(page).first().click();
    await expect
      .poll(() => gasWaypoints(page), { timeout: 2000 })
      .toEqual([{ section_id: 1, station_index: 2 }]);
    await expect(page.locator('[data-testid="gas-waypoint-marker"]')).toHaveCount(1);
  });

  test('marked points: pin, label, remove - and no gas recompute (#298)', async ({ page }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="gas-marked-empty"]')).toBeVisible();
    // Let the initial field settle so the call count below reflects only what marking does.
    await expect(page.locator('[data-testid="gas-legend"]')).toBeVisible({ timeout: 3000 });
    const callsBefore = await page.evaluate(
      () =>
        ((window as unknown as Record<string, unknown>).__MOCK_GAS_REACH_CALLS__ as unknown[])
          ?.length ?? 0,
    );

    await openPlanMenu(page, 3);
    await planMenuItem(page, 'Set marked point').click();
    await expect(gasMarkedRows(page)).toHaveCount(1);
    await expect(page.locator('[data-testid="gas-marked-point-marker"]')).toHaveCount(1);

    await gasMarkedLabelInputs(page).fill('photo here');
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState()
                .markedPoints[0]?.label ?? null,
          ),
        { timeout: 2000 },
      )
      .toBe('photo here');

    // A marked point is an annotation: it must never re-run the gas solve. If this ever fires, the
    // store setter has started nulling reachField and the overlay will flicker on every keystroke.
    const callsAfter = await page.evaluate(
      () =>
        ((window as unknown as Record<string, unknown>).__MOCK_GAS_REACH_CALLS__ as unknown[])
          ?.length ?? 0,
    );
    expect(callsAfter).toBe(callsBefore);

    await gasMarkedRemoveButtons(page).click();
    await expect(gasMarkedRows(page)).toHaveCount(0);
    await expect(page.locator('[data-testid="gas-marked-point-marker"]')).toHaveCount(0);
  });

  test('the legend keys the route line and the new route symbols (#298)', async ({ page }) => {
    await enterPlanMode(page);
    const key = page.locator('[data-testid="gas-legend-key"]');
    await expect(key).toBeVisible({ timeout: 3000 });
    // The route line is drawn by default (origin -> deepest reachable), so its key is always there.
    await expect(key.getByText('Planned route')).toBeVisible();
    // Way point / marked point rows appear only once those exist, so the key stays short.
    await expect(key.getByText('Way point')).toHaveCount(0);
    await openPlanMenu(page, 2);
    await planMenuItem(page, 'Set as way point').click();
    await expect(key.getByText('Way point')).toBeVisible();
  });

  test('undo/redo shortcuts are inert in Plan AND Export (read-only guard parity with the hidden buttons) @smoke', async ({
    page,
  }) => {
    const getCalls = () =>
      page.evaluate(
        () =>
          (
            window as unknown as { __MOCK_GET_UNDO_REDO_CALLS__: () => string[] }
          ).__MOCK_GET_UNDO_REDO_CALLS__(),
      );

    await enterPlanMode(page);
    await page.evaluate(() =>
      (
        window as unknown as { __MOCK_RESET_UNDO_REDO_CALLS__: () => void }
      ).__MOCK_RESET_UNDO_REDO_CALLS__(),
    );
    await page.keyboard.press('Control+z');
    await page.keyboard.press('Control+y');
    expect(await getCalls()).toEqual([]);

    // Export is guarded by the same `appMode !== 'survey'` term, not a plan-specific
    // check - pin it so a refactor to `=== 'plan'` can't silently re-open Export.
    await enterMode(page, 'export');
    await page.keyboard.press('Control+z');
    await page.keyboard.press('Control+y');
    expect(await getCalls()).toEqual([]);

    // Survey positive control + injection proof: both fire here, so the []s above
    // reflect the guard (not a dead keystroke path), and a guard leak in Plan/Export
    // would have shown up as extra entries before this exact-equality settles.
    await enterMode(page, 'survey');
    await page.keyboard.press('Control+z');
    await page.keyboard.press('Control+y');
    await expect.poll(getCalls, { timeout: 2000 }).toEqual(['undo', 'redo']);
  });

  test('Ctrl+I in Plan opens the picker; the DMP raises import-blocked with Stay and Switch-and-continue (#243, #407) @smoke', async ({
    page,
  }) => {
    const openCount = () =>
      page.evaluate(
        () =>
          (
            window as unknown as { __MOCK_GET_DIALOG_OPEN_COUNT__: () => number }
          ).__MOCK_GET_DIALOG_OPEN_COUNT__(),
      );

    await enterPlanMode(page);
    await page.evaluate(() =>
      (
        window as unknown as { __MOCK_RESET_DIALOG_OPEN_COUNT__: () => void }
      ).__MOCK_RESET_DIALOG_OPEN_COUNT__(),
    );
    // One door (#407): Ctrl+I opens the unified picker even in Plan mode - the old
    // pre-guard that swallowed the keystroke is gone, because a .stalx open or an
    // import-as-new-project is legal here. The #243 read-only guard moved to the
    // router: the mocked picker returns a .dmp (the one additive, mutating import),
    // and THAT raises the import-blocked CHOICE dialog - not an error, and not
    // silence. The default seed has no eligible closures, so Plan's forced
    // closureAdjusted reverts to measured - the exact trap a geometryMode-only
    // guard would miss.
    await page.keyboard.press('Control+i');
    await expect.poll(openCount, { timeout: 2000 }).toBeGreaterThan(0);
    const blocked = page.getByTestId('import-blocked-dialog');
    await expect(blocked).toBeVisible({ timeout: 5000 });
    await expect(blocked).toContainText('Switch to Survey mode for import');
    // Raised from Plan, so the dismiss button names Plan.
    await expect(page.getByTestId('import-blocked-stay')).toHaveText('Stay in Plan');

    // "Stay in Plan" abandons the import and leaves the mode alone.
    await page.getByTestId('import-blocked-stay').click();
    await expect(blocked).toBeHidden({ timeout: 5000 });
    await expect(page.getByTestId('dmp-selection-dialog')).toHaveCount(0);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible();

    // Second pass: "Switch to Survey" flips the mode AND continues importing the
    // file that was already picked - straight into the DMP flow, no re-pick, no
    // error dialog. This is also the positive control proving the block above is
    // the read-only guard, not a broken keybinding.
    await page.keyboard.press('Control+i');
    await expect(blocked).toBeVisible({ timeout: 5000 });
    await page.getByTestId('import-blocked-switch').click();
    await expect(blocked).toBeHidden({ timeout: 5000 });
    await expect(page.getByTestId('dmp-selection-dialog')).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);
    // Back in Survey: the Plan workspace panel is gone.
    await expect(page.locator('[data-testid="gas-panel"]')).toHaveCount(0);
  });

  test('the planned route renders as a highlighted overlay @smoke', async ({ page }) => {
    await enterPlanMode(page);
    // The default route (origin → deepest reachable) draws the solid route line over the survey.
    await expect
      .poll(
        () =>
          page.evaluate(() => {
            const map = (
              window as unknown as {
                __stalagmight_map__?: {
                  queryRenderedFeatures: (o: { layers: string[] }) => unknown[];
                };
              }
            ).__stalagmight_map__;
            try {
              return map?.queryRenderedFeatures({ layers: ['survey-gas-route'] }).length ?? 0;
            } catch {
              return 0;
            }
          }),
        { timeout: 4000 },
      )
      .toBeGreaterThan(0);

    // Jump legs are FADED, not dashed (#243): one line layer, opacity driven by a
    // data expression on `isJump`. Pin the expression so reverting to a constant
    // opacity (or typo-ing the property) - which MapLibre evaluates silently - fails.
    const opacity = await page.evaluate(() => {
      const map = (
        window as unknown as {
          __stalagmight_map__?: { getPaintProperty: (l: string, p: string) => unknown };
        }
      ).__stalagmight_map__;
      try {
        return JSON.stringify(map?.getPaintProperty('survey-gas-route', 'line-opacity'));
      } catch {
        return '';
      }
    });
    expect(opacity).toContain('isJump');
    expect(opacity).toContain('0.45');
  });

  test('carousel switches Survey/Plan/Export; Plan is read-only; Export has controls @smoke', async ({
    page,
  }) => {
    await expect(page.locator('[data-testid="mode-carousel"]')).toBeVisible();

    // Export directly from Survey: the right Dock must reveal the panel (it would stay
    // hidden in a closed dock if entry didn't reveal it).
    await enterMode(page, 'export');
    expect(await appMode(page)).toBe('export');
    await expect(page.locator('[data-testid="export-panel"]')).toBeVisible({ timeout: 3000 });

    // Plan: setup sidebar + tabbed tables; the map is read-only.
    await enterPlanMode(page);
    expect(await appMode(page)).toBe('plan');
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="plan-tables-panel"]')).toBeVisible();
    await expect(page.locator('[data-testid="cylinders-table"]')).toBeVisible();
    // Mutating tools (merge/comment/move) are HIDDEN in Plan, not just disabled; the
    // non-mutating Select/Pan/Measure remain.
    await expect(page.locator('[data-testid="tool-merge"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="tool-select"]')).toBeVisible();
    await expect(page.locator('[data-testid="tool-measure"]')).toBeVisible();
    // Import and Undo/Redo are likewise HIDDEN in the read-only workspaces (#243).
    await expect(page.locator('[data-testid="toolbar-import-button"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="toolbar-undo-button"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="toolbar-redo-button"]')).toHaveCount(0);
    // A mutating tool selected programmatically (e.g. via shortcut) snaps back to select.
    await page.evaluate(() =>
      (window as unknown as E2EWindow).__E2E_TEST__?.uiStore.getState().setActiveTool('merge'),
    );
    await expect.poll(() => activeTool(page), { timeout: 2000 }).toBe('select');
    // Profile tab renders the dive-profile chart (#299), not the old stub.
    await page.locator('[data-testid="plan-tab-profile"]').click();
    await expect(page.locator('[data-testid="plan-profile"]')).toBeVisible();
    await expect(page.locator('[data-testid="plan-profile-axis-distance"]')).toBeVisible();

    // Export: panel hosts the real export controls (no longer a stub), no gas/plan tables.
    await enterMode(page, 'export');
    expect(await appMode(page)).toBe('export');
    await expect(page.locator('[data-testid="export-panel"]')).toBeVisible({ timeout: 3000 });
    await expect(
      page.locator('[data-testid="export-panel"]').getByTestId('export-button'),
    ).toBeVisible();
    await expect(page.locator('[data-testid="gas-panel"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="plan-tables-panel"]')).toHaveCount(0);
    // Export hides the file/history controls too (#243).
    await expect(page.locator('[data-testid="toolbar-import-button"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="toolbar-undo-button"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="toolbar-redo-button"]')).toHaveCount(0);

    // Back to Survey.
    await enterMode(page, 'survey');
    expect(await appMode(page)).toBe('survey');
    await expect(page.locator('[data-testid="export-panel"]')).toHaveCount(0);
    // The survey toolbar gets its file/history controls back (#243).
    await expect(page.locator('[data-testid="toolbar-import-button"]')).toBeVisible();
    await expect(page.locator('[data-testid="toolbar-undo-button"]')).toBeVisible();
    await expect(page.locator('[data-testid="toolbar-redo-button"]')).toBeVisible();
  });

  test('Export panel: inline controls, large WYSIWYG preview, colors are raster-only', async ({
    page,
  }) => {
    await enterMode(page, 'export');
    expect(await appMode(page)).toBe('export');

    const panel = page.locator('[data-testid="export-panel"]');
    await expect(panel).toBeVisible({ timeout: 3000 });

    // Defaults to the image type → the large WYSIWYG preview overlays the map area.
    await expect(page.locator('[data-testid="export-mode-preview"]')).toBeVisible();
    await expect(panel.getByTestId('export-type-image')).toBeChecked();

    // The label/color controls live in a collapsible "Comments & Labels Options"
    // section, collapsed by default to keep the panel uncluttered. Expand it.
    await expect(panel.getByTestId('export-show-labels')).toHaveCount(0);
    await panel.getByTestId('export-labels-options-toggle').click();

    // The full label toggle set is present (mirrors the Display tab's group),
    // alongside the penetration toggle and the Color dropdown.
    await expect(panel.getByTestId('export-show-section-names')).toBeVisible();
    await expect(panel.getByTestId('export-show-station-labels')).toBeVisible();
    await expect(panel.getByTestId('export-show-station-comments')).toBeVisible();
    await expect(panel.getByTestId('export-show-shot-comments')).toBeVisible();
    await expect(panel.getByTestId('export-show-shot-data')).toBeVisible();
    // The penetration-labels toggle + the Color dropdown, which offers the
    // depth/distance/section gradients that match the survey map.
    await expect(panel.getByTestId('export-show-labels')).toBeVisible();
    const colorSelect = panel.getByTestId('export-color-mode');
    await expect(colorSelect).toBeVisible();
    await colorSelect.click();
    const colorList = page.getByRole('listbox');
    await expect(colorList.locator('[role="option"]:has-text("By depth")')).toBeVisible();
    await expect(colorList.locator('[role="option"]:has-text("By distance")')).toBeVisible();
    await colorList.locator('[role="option"]:has-text("By depth")').click();

    // Switch the format to SVG → the Color dropdown disables (SVG is always monochrome).
    const formatSelect = panel.getByTestId('image-format-select');
    await formatSelect.click();
    await page.getByRole('listbox').locator('[role="option"]:has-text("SVG")').click();
    await expect(colorSelect).toBeDisabled();

    // Switch to the data export type → the map preview overlay goes away.
    await panel.getByTestId('export-type-data').click();
    await expect(page.locator('[data-testid="export-mode-preview"]')).toHaveCount(0);
  });

  test('Plan map is read-only: Delete on a selection opens no confirm dialog @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    // Select a shot via the store, then press Delete. Plan/Export are read-only even on a
    // no-closures project (where geometryMode falls back to 'measured'), so the appMode
    // guard must block the delete - no confirm dialog opens.
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: { sections: Array<{ id: number; shots: Array<{ id: number }> }> } | null;
            };
          };
          uiStore: {
            getState: () => {
              setSelection: (s: { type: string; sectionId: number; shotId: number }) => void;
            };
          };
        };
      }).__E2E_TEST__;
      const sec = e2e!.projectStore.getState().project!.sections[0];
      e2e!.uiStore
        .getState()
        .setSelection({ type: 'shot', sectionId: sec.id, shotId: sec.shots[0].id });
    });
    await page.keyboard.press('Delete');
    await expect(page.locator('[data-testid="confirm-dialog"]')).toHaveCount(0);
  });

  test('Plan stars the zero datum; clicking the star picks it as the origin @smoke', async ({
    page,
  }) => {
    // Flag station 0 of the first section as a zero datum, then refresh so the map
    // reflects it (the gas solve's "Entrance" default already starts from the datum;
    // this marks it so the user can see + pick it).
    await page.evaluate(async () => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: { sections: Array<{ id: number }> } | null;
              setZeroDatum: (
                ref: { section_id: number; station_index: number },
                gps: { latitude: number; longitude: number; depth_reference: number },
              ) => Promise<unknown>;
              refreshProjectState: () => Promise<void>;
            };
          };
        };
      }).__E2E_TEST__;
      const ps = e2e!.projectStore.getState();
      const secId = ps.project!.sections[0].id;
      await ps.setZeroDatum(
        { section_id: secId, station_index: 0 },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 },
      );
      await ps.refreshProjectState();
    });

    // #395: the entry question is still unanswered - clicking the star must count as answering
    // it (the full rig appears), not require choosing the entrance first.
    await enterMode(page, 'plan');
    await expect(page.locator('[data-testid="gas-entry-card"]')).toBeVisible({ timeout: 3000 });
    // The black star renders on the zero-datum station.
    await expect
      .poll(() => page.locator('[data-testid="gas-zero-datum-marker"]').count(), { timeout: 3000 })
      .toBeGreaterThan(0);
    await page.locator('[data-testid="gas-zero-datum-marker"]').first().click();
    await expect(page.locator('[data-testid="gas-entry-card"]')).toBeHidden();
    const readout = page.locator('[data-testid="gas-origin-readout"]');
    await expect(readout).not.toHaveText('Entrance');
  });

  test('the gas ramp (and the beyond-range dim / unconnected grey) survives a feature rebuild', async ({
    page,
  }) => {
    await waitForDrawnStations(page, 1);
    const zoom = (await mapZoom(page)) + PROBE_ZOOM_DELTA;
    await setMapZoom(page, zoom);
    const grey = await greyKey(page);
    const sectionKey = (await stationCenter(page, LAST_REACHABLE)).join(',');

    await enterPlanMode(page);
    await setMapZoom(page, zoom);
    await expect.poll(() => stationPaints(page, 0, GAS_NEAR_KEY), { timeout: 3000 }).toBe(true);
    // The last station inside the budget is painted an interior ramp color, the
    // beyond-range one dimmed (not grey), the unconnected one grey. Snapshot the
    // reachable one's whole footprint so "still ramp-painted after the rebuild" can
    // be asserted byte-for-byte.
    const reachableBefore = await stationBlock(page, LAST_REACHABLE);
    const reachableColors = await stationColors(page, LAST_REACHABLE);
    expect(reachableColors.has(sectionKey)).toBe(false);
    expect(reachableColors.has(GAS_NEAR_KEY)).toBe(false);
    expect(reachableColors.has(grey)).toBe(false);
    const beyondRangeBefore = await stationColors(page, BEYOND_RANGE);
    expect(beyondRangeBefore.has(grey)).toBe(false);
    expect(beyondRangeBefore.has(GAS_FAR_KEY)).toBe(false);
    expect(beyondRangeBefore.has(sectionKey)).toBe(false);
    expect(await stationPaints(page, UNCONNECTED, grey)).toBe(true);

    // A survey edit rebuilds every feature (the custom renderer rebuilds its vertex
    // buffers from scratch; the stock path had setData wipe the feature-state). The
    // overlay must re-apply its ramp AND the dim afterward, the key correctness risk.
    await seedProjectWithSections(page, [
      {
        name: 'Gas B',
        direction: 'in',
        shots: [{ azimuth_in: 0, azimuth_out: 180, length: 5, depth_in: -3, depth_out: -9 }],
      },
    ]);
    // The rebuild can re-fit the view, so re-pin the calibrated zoom.
    await setMapZoom(page, zoom);
    await waitForDrawnStations(page, 1);

    // The rebuilt stations are painted exactly what they were: the ramp re-applied.
    // (Probed on the first section's far stations, which the new section does not
    // overlap: the mock lays every section out along the same axis and never bumps
    // `modified_at` on create_section, the field's refetch key, so the new section's
    // stations paint grey where they land on top of the first two of this one.)
    await expect.poll(() => stationBlock(page, LAST_REACHABLE), { timeout: 3000 }).toEqual(reachableBefore);
    const beyondRangeAfter = await stationColors(page, BEYOND_RANGE);
    expect(beyondRangeAfter.has(grey)).toBe(false);
    expect(beyondRangeAfter.has(GAS_FAR_KEY)).toBe(false);
    expect(beyondRangeAfter.has(sectionKey)).toBe(false);
    const unconnected = await stationColors(page, UNCONNECTED);
    expect(unconnected.has(grey)).toBe(true);
    expect(unconnected.has(sectionKey)).toBe(false);
    await expect(page.locator('[data-testid="gas-legend"]')).toBeVisible();
  });

  test('a stage drop is keyboard-movable - ArrowLeft pins it, sends an override, reset clears it (#246) @smoke', async ({
    page,
  }) => {
    await setupOneStageDrop(page);
    const marker = page.locator('[data-testid="gas-drop-marker"]').first();
    await marker.focus();
    // Nudge one route station SHALLOWER - deterministic (the default drop is never at the origin).
    await page.keyboard.press('ArrowLeft');
    // The store pins the drop AND the backend is re-invoked with the override (the full-recompute wiring).
    await expect
      .poll(async () => (await stageDropOverrides(page)).some((r) => r != null), { timeout: 3000 })
      .toBe(true);
    await expect
      .poll(async () => (await lastGasReachOverrides(page)).some((r) => r != null), { timeout: 3000 })
      .toBe(true);
    // a11y: focus survives the recompute - the rebuilt ▼ marker regains focus so an AT user can
    // chain nudges (regression guard for the stale-field-rebuild focus loss the review caught).
    await expect(page.locator('[data-testid="gas-drop-marker"]').first()).toBeFocused();
    // The readout offers a per-drop reset; clicking it clears the override and the field recomputes.
    const reset = page.locator('[data-testid="gas-readout-stage1-reset"]');
    await expect(reset).toBeVisible();
    await reset.click();
    await expect
      .poll(async () => (await stageDropOverrides(page)).every((r) => r == null), { timeout: 3000 })
      .toBe(true);
    await expect(reset).toHaveCount(0);
  });

  test('dragging a stage drop deeper flags it over-reserve (red marker + panel warning) (#246)', async ({
    page,
  }) => {
    await setupOneStageDrop(page);
    const marker = page.locator('[data-testid="gas-drop-marker"]').first();
    const finalTurn = page.locator('[data-testid="gas-final-turn-marker"]').first();
    const from = await marker.boundingBox();
    const to = await finalTurn.boundingBox();
    expect(from).not.toBeNull();
    expect(to).not.toBeNull();
    // Drag the ▼ onto the deepest (final-turn) station - deeper than its computed drop => over-reserve.
    await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2);
    await page.mouse.down();
    await page.mouse.move(to!.x + to!.width / 2, to!.y + to!.height / 2, { steps: 12 });
    await page.mouse.up();
    // The recomputed field marks the drop over-reserve → the marker turns red and the panel warns.
    await expect
      .poll(
        () => page.locator('[data-testid="gas-drop-marker"][data-over-reserve="true"]').count(),
        { timeout: 3000 },
      )
      .toBeGreaterThan(0);
    await expect(page.locator('[data-testid="gas-readout-over-reserve-warning"]')).toBeVisible();
  });

  test('Segments tab: one row per span; a way point adds a row, removing it takes it away (#317) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await page.locator('[data-testid="plan-tab-segments"]').click();
    await expect(page.locator('[data-testid="segments-table"]')).toBeVisible();

    // No way points yet -> exactly one segment, spanning the whole route (entrance -> onward).
    await expect(page.locator('[data-testid^="segment-row-"]')).toHaveCount(1);
    await expect(page.locator('[data-testid="segment-row-0"]')).toContainText('Entrance');
    await expect(page.locator('[data-testid="segment-row-0"]')).toContainText('Onward');
    await expect(page.locator('[data-testid="segment-speed-0"]')).toBeVisible();
    await expect(page.locator('[data-testid="segment-scooter-0"]')).toBeVisible();

    // A way point splits the span it falls in - one MORE segment per way point, not per station.
    await openPlanMenu(page, 2);
    await planMenuItem(page, 'Set as way point').click();
    await expect(page.locator('[data-testid^="segment-row-"]')).toHaveCount(2);
    await openPlanMenu(page, 4);
    await planMenuItem(page, 'Set as way point').click();
    await expect(page.locator('[data-testid^="segment-row-"]')).toHaveCount(3);

    // Removing a way point merges the two spans it used to separate back into one row.
    await gasWaypointRemoveButtons(page).first().click();
    await expect(page.locator('[data-testid^="segment-row-"]')).toHaveCount(2);
  });

  test('editing a segment speed reaches the backend, not just a local re-render (#317) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await page.locator('[data-testid="plan-tab-segments"]').click();
    const speed = page.locator('[data-testid="segment-speed-0"]');
    // Shows the plan's own speed (the default swim speed, 10 m/min metric) without adopting it -
    // dimmed + marked inherited (#403), so the displayed number never reads as one typed here.
    await expect(speed).toHaveText('10.0');
    await expect(page.locator('[data-testid="segment-speed-inherited-0"]')).toBeVisible();
    // ...and the request carries NO speed for that span, so the engine applies the configured one.
    // Sending a number here - even the right one - is exactly what disabled the swim-speed setting
    // in the first pass, while every screen assertion above still passed.
    await expect
      .poll(
        async () => {
          const segs = await lastGasReachSegments(page);
          return segs.length === 0 ? 'no-compute-yet' : segs[0].speed_m_per_min;
        },
        { timeout: 3000 }
      )
      .toBeNull();

    await editGridCell(page, 'segment', 0, 'speed', '23');
    // The span now carries a speed of its own: the inherited marker is gone and the cell shows
    // the committed number.
    await expect(page.locator('[data-testid="segment-speed-inherited-0"]')).toHaveCount(0);
    await expect(speed).toHaveText('23.0');

    // The wiring guard: `setSegments` nulls the cached field, so this is only reachable if the
    // edit flowed all the way to a recompute request - a table that only re-rendered its own
    // local state would pass every other assertion in this file and still fail here.
    await expect
      .poll(async () => (await lastGasReachSegments(page))[0]?.speed_m_per_min, { timeout: 3000 })
      .toBe(23);

    // Clearing the cell hands the span back to the plan speed. This is the ONLY route back once a
    // number has been typed - the per-row Reset button is gone with the grid migration (#403) -
    // and it has to reach the engine as `null`, not as the 10 the cell goes back to showing.
    await page.locator('[data-testid="segment-cell-0-speed"]').click();
    await page.keyboard.press('Delete');
    await expect(page.locator('[data-testid="segment-speed-inherited-0"]')).toBeVisible();
    await expect(speed).toHaveText('10.0');
    await expect
      .poll(async () => (await lastGasReachSegments(page))[0]?.speed_m_per_min, { timeout: 3000 })
      .toBeNull();
  });

  test('the DPV scooter checkbox is disabled without a configured rig, and the readout has no scooter row (#317) @smoke', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });

    // The runtime row is engine-derived and rig-independent - it appears from the first route
    // resolve regardless of DPV.
    await expect(page.locator('[data-testid="gas-readout-route-runtime"]')).toBeVisible({
      timeout: 3000,
    });
    // No rig configured (the seeded default has none) -> no scooter distance/range row.
    await expect(page.locator('[data-testid="gas-readout-scooter-range"]')).toHaveCount(0);

    await page.locator('[data-testid="plan-tab-segments"]').click();
    await expect(page.locator('[data-testid="segments-table"]')).toBeVisible();
    await expect(page.locator('[data-testid="segment-scooter-0"]')).toBeDisabled();
    await expect(page.locator('[data-testid="segments-no-rig-hint"]')).toBeVisible();
  });

  test('configuring a DPV rig in the DPV tab enables the segment scooter checkbox and its readout row (#317)', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await enableDpvRig(page);

    // With a rig configured, the scooter distance/range row appears (even before anything is
    // scootered - it starts at 0 of the rig's usable range) and the checkbox is live.
    const readout = page.locator('[data-testid="gas-readout-scooter-range"]');
    await expect(readout).toBeVisible({ timeout: 3000 });
    // Pin the mock's copy of `DpvRig::usable_range_m` (src-tauri/src/config/gas.rs) AND
    // `src/utils/dpv.ts`'s `makeDefaultDpvRig` (primary 45 m/min x 60 min, backup 45 m/min x 45
    // min, range_reserve_fraction 0.1 - the toggle's "enable" default): `min(45*60, 45*45) *
    // (1 - 0.1)` = `min(2700, 2025) * 0.9` = `2025 * 0.9` = 1822.5, which the metric formatter
    // rounds and groups to "1,823 m". Nothing is scootered yet, so the distance half reads "0 m".
    // The editor's own readout must show the identical figure - both format the same `DpvRig`.
    await expect(readout).toHaveText('0 m / 1,823 m');
    await expect(page.locator('[data-testid="dpv-usable-range-value"]')).toHaveText('1,823 m');

    await page.locator('[data-testid="plan-tab-segments"]').click();
    await expect(page.locator('[data-testid="segment-scooter-0"]')).toBeEnabled();
    await expect(page.locator('[data-testid="segments-no-rig-hint"]')).toHaveCount(0);
  });

  test('a DPV rig configured in one Plan-mode entry survives leaving to Survey and coming back (#317)', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="gas-readout-scooter-range"]')).toHaveCount(0);
    await enableDpvRig(page);
    await expect(page.locator('[data-testid="gas-readout-scooter-range"]')).toBeVisible({
      timeout: 3000,
    });

    // Leaving Plan unmounts the panel (and the rig editor with it); the gas store itself persists
    // across mode switches (#244), so re-entering must find the rig exactly as it was left,
    // without a project reload.
    await enterMode(page, 'survey');
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('[data-testid="gas-readout-scooter-range"]')).toBeVisible({
      timeout: 3000,
    });
    await page.locator('[data-testid="plan-tab-dpv"]').click();
    await expect(page.locator('[data-testid="toggle-dpv-enabled"]')).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  test('the scooter park marker appears mid-route and disappears once every segment is scootered (#317)', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await enableDpvRig(page);

    // Two way points -> three segments, so the middle one is a genuine middle span, not an end one.
    await openPlanMenu(page, 2);
    await planMenuItem(page, 'Set as way point').click();
    await openPlanMenu(page, 4);
    await planMenuItem(page, 'Set as way point').click();
    await expect.poll(() => gasWaypoints(page), { timeout: 2000 }).toHaveLength(2);

    await page.locator('[data-testid="plan-tab-segments"]').click();
    await expect(page.locator('[data-testid="segment-row-2"]')).toBeVisible();

    // Scooter the middle segment (1) only - the last (2) stays swum: the units are parked at the
    // boundary between them, not carried all the way to the turnaround.
    await page.locator('[data-testid="segment-scooter-1"]').check();
    await expect
      .poll(() => page.locator('[data-testid="gas-scooter-drop-marker"]').count(), { timeout: 3000 })
      .toBeGreaterThan(0);
    // The table names the very same boundary the map derives the marker from.
    await expect(page.locator('[data-testid="segment-park-1"]')).toBeVisible();

    // Scooter the last segment too - the units now run all the way to the turnaround, so there is
    // nothing left to park at. This is the ONLY way to clear the derived marker: it is never
    // placed directly (see the Map tooltip / SegmentsTable's parkHereHint).
    await page.locator('[data-testid="segment-scooter-2"]').check();
    await expect
      .poll(() => page.locator('[data-testid="gas-scooter-drop-marker"]').count(), { timeout: 3000 })
      .toBe(0);
    await expect(page.locator('[data-testid="segment-park-1"]')).toHaveCount(0);
  });

  test('unticking a scootered segment resets it at the backend, not just the checkbox (#317)', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await enableDpvRig(page);

    await page.locator('[data-testid="plan-tab-segments"]').click();
    const checkbox = page.locator('[data-testid="segment-scooter-0"]');
    await checkbox.check();
    // Ticking auto-fills the rig's primary speed onto a span still inheriting the plan speed
    // (`SegmentsTable`'s `setScooter` contract) - confirms the fixture is actually scootered
    // before the untick under test.
    await expect
      .poll(async () => (await lastGasReachSegments(page))[0]?.speed_m_per_min, { timeout: 3000 })
      .toBe(45);

    await checkbox.uncheck();
    // The wiring guard: unticking must reach the backend with the span fully reset -
    // `on_scooter: false` AND the auto-filled speed cleared back to `null` (inheriting the plan
    // speed again). A table that only re-rendered its own local state, or that cleared the
    // checkbox but left the auto-filled speed behind, would still leave a span priced at scooter
    // pace with no scooter under it - the unsafe direction `setScooter` exists to close.
    await expect
      .poll(async () => (await lastGasReachSegments(page))[0], { timeout: 3000 })
      .toEqual({ on_scooter: false, speed_m_per_min: null });
  });

  test('disabling the DPV rig reaches the backend with no rig, resets scootered segments, and persists (#317)', async ({
    page,
  }) => {
    await enterPlanMode(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible({ timeout: 3000 });
    await enableDpvRig(page);

    await page.locator('[data-testid="plan-tab-segments"]').click();
    await page.locator('[data-testid="segment-scooter-0"]').check();
    await expect
      .poll(async () => (await lastGasReachSegments(page))[0]?.on_scooter, { timeout: 3000 })
      .toBe(true);

    await page.locator('[data-testid="plan-tab-dpv"]').click();
    await page.locator('[data-testid="toggle-dpv-enabled"]').click();
    await expect(page.locator('[data-testid="dpv-rig-fields"]')).toHaveCount(0);

    // The next compute request carries no rig at all - not just the client-side `hasDpvRig` flag
    // that happens to hide the readout - AND `setDpvRig`'s removal path resets every segment that
    // was scootered (#317).
    await expect.poll(() => lastGasReachDpv(page), { timeout: 3000 }).toBeNull();
    await page.locator('[data-testid="plan-tab-segments"]').click();
    await expect
      .poll(async () => (await lastGasReachSegments(page))[0], { timeout: 3000 })
      .toEqual({ on_scooter: false, speed_m_per_min: null });

    // The disable is a real config write, not just an in-memory reset - `DpvRigEditor` writes
    // through to the saved config on every commit, the same persisted-write contract the
    // swim-speed field has ("Plan swim speed persists to the saved config", above).
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (
              window as unknown as {
                __E2E_TEST__: {
                  projectStore: {
                    getState: () => { config: { gas: { dpv: unknown } } | null };
                  };
                };
              }
            ).__E2E_TEST__.projectStore.getState().config?.gas.dpv,
        ),
      )
      .toBeNull();
  });
});
