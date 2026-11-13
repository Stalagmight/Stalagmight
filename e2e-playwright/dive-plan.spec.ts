import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithSections,
  deleteShotAt,
  selectDropdownOption,
  enterExportWorkspace,
  closeExportWorkspace,
  type SectionSeed,
} from './helpers/app';
import { gasWaypointUnresolvedIndicators } from './helpers/testIds';

/**
 * E2E for dive-plan persistence (#244): the "Previous dive plans" bar in Plan mode
 * (dropdown + Save / Save as... / Delete), dirty tracking, and the discard-on-switch
 * guard. The four `*_dive_plan` commands are mocked by an in-memory `.plnx` store
 * (mocks/tauri-ipc.ts). A plan is the compute_gas_reach_field input tuple, so loading
 * one applies the snapshot to gasPlanStore; we drive input changes + read state via
 * the exposed `__E2E_TEST__.gasPlanStore` hook and assert saved plans via
 * `__MOCK_GET_DIVE_PLANS__`.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Plan A',
    direction: 'in',
    shots: Array.from({ length: 6 }, (_, i) => ({
      azimuth_in: 90,
      azimuth_out: 270,
      length: 10,
      depth_in: i,
      depth_out: i + 1,
    })),
  },
];

type PlanMetaLite = { id: number; name: string };
type StationRefLite = { section_id: number; station_index: number };
type PlanSegmentLite = { speed_m_per_min: number | null; on_scooter: boolean };

type E2EWindow = {
  __E2E_TEST__?: {
    projectStore: { getState: () => { refreshProjectState: () => Promise<void> } };
    gasPlanStore: {
      getState: () => {
        diver: { rmv_l_per_min: number };
        loadedPlanId: number | null;
        waypoints: StationRefLite[];
        markedPoints: { station: StationRefLite; label: string | null }[];
        segments: PlanSegmentLite[];
        setDiver: (d: { rmv_l_per_min: number; label: string | null }) => void;
        addWaypoint: (ref: StationRefLite) => void;
        removeWaypoint: (index: number) => void;
        addMarkedPoint: (ref: StationRefLite) => void;
        removeMarkedPoint: (index: number) => void;
        setMarkedPointLabel: (index: number, label: string) => void;
        setSegments: (segments: PlanSegmentLite[]) => void;
      };
    };
  };
  __MOCK_SET_PROJECT_PATH__?: (path: string | null) => void;
  __MOCK_RESET_DIVE_PLANS__?: () => void;
  __MOCK_GET_DIVE_PLANS__?: () => PlanMetaLite[];
};

async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page
    .locator('button:has-text("New Project"), h3:has-text("New Project")')
    .first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

/**
 * R9/R10 (#395): Plan opens on the entry card, and `DivePlanBar` only mounts once the diver has
 * answered "Where does this dive start?" - answer it with the entrance so every test here (none
 * of which is about the entry card itself) reaches the full rig.
 */
async function enterPlan(page: Page): Promise<void> {
  await page.locator('[data-testid="mode-plan"]').click();
  // `originChosen` persists across mode switches (#244), so a RE-entry after Plan -> Survey/
  // Export -> Plan finds the full rig already up - only click through the entry card the first
  // time it appears.
  const startEntrance = page.locator('[data-testid="gas-start-entrance"]');
  if (await startEntrance.isVisible().catch(() => false)) {
    await startEntrance.click();
  }
  await expect(page.locator('[data-testid="dive-plan-bar"]')).toBeVisible({ timeout: 3000 });
}

/** Set (or, with null, clear) the open project's file path, then refetch so the bar
 *  reacts. The mock's New Project flow assigns a path, so tests that want the unsaved
 *  state must clear it explicitly. */
async function setProjectPath(page: Page, path: string | null): Promise<void> {
  await page.evaluate((p) => {
    (window as unknown as E2EWindow).__MOCK_SET_PROJECT_PATH__?.(p);
  }, path);
  await page.evaluate(async () => {
    await (window as unknown as E2EWindow).__E2E_TEST__?.projectStore
      .getState()
      .refreshProjectState();
  });
}

/** Give the open project a file path (a saved project) so the bar is enabled. */
async function giveProjectAPath(page: Page): Promise<void> {
  await setProjectPath(page, '/tmp/dive-plan-e2e.stalx');
}

async function setDiverRmv(page: Page, rmv: number): Promise<void> {
  await page.evaluate((r) => {
    (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore
      .getState()
      .setDiver({ rmv_l_per_min: r, label: null });
  }, rmv);
}

async function diverRmv(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().diver.rmv_l_per_min ??
      -1,
  );
}

async function loadedPlanId(page: Page): Promise<number | null> {
  return page.evaluate(
    () =>
      (window as unknown as E2EWindow).__E2E_TEST__?.gasPlanStore.getState().loadedPlanId ?? null,
  );
}

async function savedPlans(page: Page): Promise<PlanMetaLite[]> {
  return page.evaluate(() => (window as unknown as E2EWindow).__MOCK_GET_DIVE_PLANS__?.() ?? []);
}

/** Save-as a new plan through the bar UI. */
async function saveAs(page: Page, name: string): Promise<void> {
  await page.locator('[data-testid="dive-plan-save-as"]').click();
  await page.locator('[data-testid="dive-plan-name-input"]').fill(name);
  await page.locator('[data-testid="dive-plan-save-as-confirm"]').click();
  await expect(page.locator('[data-testid="dive-plan-save-as-row"]')).toHaveCount(0);
}

test.describe('Dive-plan persistence (#244)', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Dive Plan Test');
    await seedProjectWithSections(page, sectionSeeds);
    await page.evaluate(() => (window as unknown as E2EWindow).__MOCK_RESET_DIVE_PLANS__?.());
  });

  test('an unsaved project disables saving with a hint @smoke', async ({ page }) => {
    // The mock's New Project flow assigns a path, so clear it to get the unsaved state.
    await setProjectPath(page, null);
    await enterPlan(page);
    // No file_path → no sibling location for the .plnx.
    await expect(page.locator('[data-testid="dive-plan-needs-save"]')).toBeVisible();
    await expect(page.locator('[data-testid="dive-plan-save-as"]')).toBeDisabled();
    await expect(page.locator('[data-testid="dive-plan-save"]')).toBeDisabled();
  });

  test('Save as creates a plan and loads it (dropdown + backend round-trip) @smoke', async ({
    page,
  }) => {
    await giveProjectAPath(page);
    await enterPlan(page);
    await expect(page.locator('[data-testid="dive-plan-save-as"]')).toBeEnabled();

    await setDiverRmv(page, 15);
    await saveAs(page, 'Deep push');

    const plans = await savedPlans(page);
    expect(plans).toHaveLength(1);
    expect(plans[0].name).toBe('Deep push');
    // The just-saved plan is now the loaded one (baseline = current inputs → not dirty).
    expect(await loadedPlanId(page)).toBe(plans[0].id);
    await expect(page.locator('[data-testid="dive-plan-dirty"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="dive-plan-select"]')).toHaveText(plans[0].name);
  });

  test('loading another plan applies its snapshot to the inputs @smoke', async ({ page }) => {
    await giveProjectAPath(page);
    await enterPlan(page);

    await setDiverRmv(page, 15);
    await saveAs(page, 'Plan A');
    await setDiverRmv(page, 25);
    await saveAs(page, 'Plan B'); // now loaded, diver 25

    const plans = await savedPlans(page);
    const planA = plans.find((p) => p.name === 'Plan A');
    expect(planA).toBeDefined();

    // Not dirty (just saved B), so selecting A loads immediately (no confirm).
    await selectDropdownOption(page, 'dive-plan-select', String(planA!.id));
    await expect
      .poll(() => loadedPlanId(page), { timeout: 3000 })
      .toBe(planA!.id);
    expect(await diverRmv(page)).toBe(15); // A's snapshot restored
  });

  test('way points and marked points survive a save/load round trip (#298)', async ({ page }) => {
    await giveProjectAPath(page);
    await enterPlan(page);

    await page.evaluate(() => {
      const gas = (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState();
      gas.addWaypoint({ section_id: 1, station_index: 2 });
      gas.addWaypoint({ section_id: 1, station_index: 4 });
      gas.addMarkedPoint({ section_id: 1, station_index: 3 });
      gas.setMarkedPointLabel(0, 'photo here');
    });
    await saveAs(page, 'With route points');

    // Load a second, bare plan so the inputs genuinely change, then come back to the first: a
    // round trip that never left the store would pass even if nothing were persisted.
    await page.evaluate(() => {
      const gas = (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState();
      gas.removeWaypoint(0);
      gas.removeWaypoint(0);
      gas.removeMarkedPoint(0);
    });
    await saveAs(page, 'Bare');

    const plans = await savedPlans(page);
    const withPoints = plans.find((p) => p.name === 'With route points');
    expect(withPoints).toBeDefined();
    await selectDropdownOption(page, 'dive-plan-select', String(withPoints!.id));
    await expect.poll(() => loadedPlanId(page), { timeout: 3000 }).toBe(withPoints!.id);

    const restored = await page.evaluate(() => {
      const gas = (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState();
      return { waypoints: gas.waypoints, markedPoints: gas.markedPoints };
    });
    // Order is part of the plan, so it must come back in order.
    expect(restored.waypoints).toEqual([
      { section_id: 1, station_index: 2 },
      { section_id: 1, station_index: 4 },
    ]);
    expect(restored.markedPoints).toEqual([
      { station: { section_id: 1, station_index: 3 }, label: 'photo here' },
    ]);
  });

  test('segments (per-span pace/propulsion) survive a save/load round trip (#317)', async ({
    page,
  }) => {
    await giveProjectAPath(page);
    await enterPlan(page);

    await page.evaluate(() => {
      const gas = (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState();
      // A way point splits the default single segment into two; edit both away from their
      // defaults so a round trip that silently dropped the edits (or fell back to fresh
      // defaults) would be caught.
      gas.addWaypoint({ section_id: 1, station_index: 2 });
      gas.setSegments([
        { speed_m_per_min: 22, on_scooter: true },
        { speed_m_per_min: 8, on_scooter: false },
      ]);
    });
    await saveAs(page, 'With segments');

    // Load a second, bare plan so the inputs genuinely change, then come back to the first: a
    // round trip that never left the store would pass even if nothing were persisted.
    await page.evaluate(() => {
      const gas = (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState();
      gas.removeWaypoint(0);
    });
    await saveAs(page, 'Bare');

    const plans = await savedPlans(page);
    const withSegments = plans.find((p) => p.name === 'With segments');
    expect(withSegments).toBeDefined();
    await selectDropdownOption(page, 'dive-plan-select', String(withSegments!.id));
    await expect.poll(() => loadedPlanId(page), { timeout: 3000 }).toBe(withSegments!.id);

    const restored = await page.evaluate(
      () => (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState().segments,
    );
    expect(restored).toEqual([
      { speed_m_per_min: 22, on_scooter: true },
      { speed_m_per_min: 8, on_scooter: false },
    ]);
  });

  test('editing after load shows the unsaved badge; Save clears it @smoke', async ({ page }) => {
    await giveProjectAPath(page);
    await enterPlan(page);
    await setDiverRmv(page, 15);
    await saveAs(page, 'Plan');
    await expect(page.locator('[data-testid="dive-plan-dirty"]')).toHaveCount(0);

    await setDiverRmv(page, 22);
    await expect(page.locator('[data-testid="dive-plan-dirty"]')).toBeVisible();
    const save = page.locator('[data-testid="dive-plan-save"]');
    await expect(save).toBeEnabled();
    await save.click();
    await expect(page.locator('[data-testid="dive-plan-dirty"]')).toHaveCount(0);
  });

  test('switching plans while dirty prompts to discard; discard loads the other @smoke', async ({
    page,
  }) => {
    await giveProjectAPath(page);
    await enterPlan(page);
    await setDiverRmv(page, 15);
    await saveAs(page, 'Plan A');
    await setDiverRmv(page, 25);
    await saveAs(page, 'Plan B');

    // Load A, then make it dirty.
    const plans = await savedPlans(page);
    const planA = plans.find((p) => p.name === 'Plan A')!;
    const planB = plans.find((p) => p.name === 'Plan B')!;
    await selectDropdownOption(page, 'dive-plan-select', String(planA.id));
    await expect.poll(() => loadedPlanId(page), { timeout: 3000 }).toBe(planA.id);
    await setDiverRmv(page, 99); // dirty

    // Selecting B must NOT switch yet - it prompts.
    await selectDropdownOption(page, 'dive-plan-select', String(planB.id));
    await expect(page.locator('[data-testid="dive-plan-switch-confirm"]')).toBeVisible();
    expect(await loadedPlanId(page)).toBe(planA.id); // still A until confirmed

    await page.locator('[data-testid="dive-plan-switch-confirm-yes"]').click();
    await expect.poll(() => loadedPlanId(page), { timeout: 3000 }).toBe(planB.id);
    expect(await diverRmv(page)).toBe(25); // B's snapshot restored
  });

  test('a loaded plan persists across mode switches (Plan -> Survey/Export -> Plan) @smoke', async ({
    page,
  }) => {
    await giveProjectAPath(page);
    await enterPlan(page);
    await setDiverRmv(page, 17);
    await saveAs(page, 'Persist me');
    const id = await loadedPlanId(page);
    expect(id).not.toBeNull();

    // Plan -> Survey -> Plan: the panel unmounts, but the store keeps the plan.
    await page.locator('[data-testid="mode-survey"]').click();
    await expect(page.locator('[data-testid="gas-panel"]')).toHaveCount(0);
    await enterPlan(page);
    expect(await loadedPlanId(page)).toBe(id);
    expect(await diverRmv(page)).toBe(17);

    // Plan -> Export -> Plan: same. Close export returns to Plan by itself.
    await enterExportWorkspace(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toHaveCount(0);
    await closeExportWorkspace(page);
    await expect(page.locator('[data-testid="gas-panel"]')).toBeVisible();
    expect(await loadedPlanId(page)).toBe(id);
    expect(await diverRmv(page)).toBe(17);
    await expect(page.locator('[data-testid="dive-plan-select"]')).toHaveText('Persist me');
  });

  test('deleting the loaded plan removes it @smoke', async ({ page }) => {
    await giveProjectAPath(page);
    await enterPlan(page);
    await setDiverRmv(page, 15);
    await saveAs(page, 'Doomed');
    expect(await savedPlans(page)).toHaveLength(1);

    await page.locator('[data-testid="dive-plan-delete"]').click();
    await expect(page.locator('[data-testid="dive-plan-delete-confirm"]')).toBeVisible();
    await page.locator('[data-testid="dive-plan-delete-confirm-yes"]').click();

    await expect.poll(() => savedPlans(page).then((p) => p.length), { timeout: 3000 }).toBe(0);
    expect(await loadedPlanId(page)).toBeNull();
  });

  test('#310: losing both neighbours of a pinned way point marks it unresolved and blocks Save', async ({
    page,
  }) => {
    await giveProjectAPath(page);
    await enterPlan(page);

    // Way point at station 3 of the 6-shot seed (7 stations, 0..6): neighbours are shot[2]
    // (before) and shot[3] (after), both interior - deleting both leaves nothing left to
    // anchor this pin to.
    await page.evaluate(() => {
      (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState().addWaypoint({
        section_id: 1,
        station_index: 3,
      });
    });
    await saveAs(page, 'Stall repro');

    // A second way point dirties the plan against its just-saved baseline, so Save is enabled
    // for a reason OTHER than the stall below - proving the assertions that follow are actually
    // driven by the anchor loss, not by a plan that just happened to already block saving.
    await page.evaluate(() => {
      (window as unknown as E2EWindow).__E2E_TEST__!.gasPlanStore.getState().addWaypoint({
        section_id: 1,
        station_index: 6,
      });
    });
    await expect(page.locator('[data-testid="dive-plan-save"]')).toBeEnabled();

    await deleteShotAt(page, 2);
    await deleteShotAt(page, 2); // shot[3] is now at index 2 after the first delete.

    // The #310 fix landed in this same branch: losing both neighbours of a pinned way point
    // marks that pin unresolved (`gasPlanStore.ts`'s `unresolved.waypoints`, per the doc comment
    // "a flagged pin is never sent to the gas engine and blocks a Save"), and `GasPlanPanel.tsx`
    // renders an indexed `gas-waypoint-unresolved-*` test ID on the affected row. Both the store
    // predicate and the DOM testid are asserted below so the test pins the mechanism, not just
    // one layer of it.
    await expect
      .poll(
        () =>
          page.evaluate(
            () =>
              (
                window as unknown as {
                  __E2E_TEST__?: {
                    gasPlanStore: { getState: () => { unresolved: { waypoints: boolean[] } } };
                  };
                }
              ).__E2E_TEST__?.gasPlanStore.getState().unresolved.waypoints[0] ?? false,
          ),
        { timeout: 3000 },
      )
      .toBe(true);
    await expect(gasWaypointUnresolvedIndicators(page).first()).toBeVisible({
      timeout: 3000,
    });
    await expect(page.locator('[data-testid="dive-plan-save"]')).toBeDisabled();
  });
});
