/**
 * Reverse Section end-to-end (#24).
 *
 * Covers the three menu surfaces that expose the action, the confirm dialog's
 * left/right choice and the request it sends, the multi-select entry and its one
 * batch, what the frontend does with the reply (the positional refs it mirrors, the
 * state it clears), and the Edit Section dialog's disabled-while-dirty rule.
 *
 * The transform itself is the backend's (#576 E3): the mock answers a reverse with
 * its canned `reversed_sections` and leaves the shots as they were. The per-shot
 * transform, the anchor compensation and the mirroring of merges, closures, datums
 * and comments are pinned in Rust (models `project/reverse.rs` tests and
 * `project/mock_narrowing_tests.rs`, core `reverse_tests.rs`); this spec asserts the
 * request the UI sends and the UI's handling of the reply.
 */
import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';
import { recordIpcCalls, getIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';
import type { CommandError } from '../src/types/ipc/CommandError';

type StationRef = { section_id: number; station_index: number };

/** A shot whose readings differ field by field; the values are arbitrary. */
function buildShot(index: number) {
  const base = index + 1;
  return {
    azimuth_in: 10 * base,
    azimuth_out: 10 * base + 6,
    length: 5 + base,
    depth_in: -1 * base,
    depth_out: -1.5 * base,
    pitch_in: 2 * base,
    pitch_out: -(base + 0.5),
    left: 0.5 * base,
    right: 0.75 * base,
    up: 0.25 * base,
    down: 0.5 + base,
  };
}

// Typed by count: the length check at the end of setup() throws unless exactly
// `sectionCount` ids came back.
function setup(page: Page): Promise<[number]>;
function setup(page: Page, sectionCount: 2): Promise<[number, number]>;
function setup(page: Page, sectionCount: 3): Promise<[number, number, number]>;
async function setup(page: Page, sectionCount = 1): Promise<number[]> {
  await setupMockedPage(page);
  await createProject(page, 'Reverse Section');

  await seedProjectWithSections(
    page,
    Array.from({ length: sectionCount }, (_, s) => ({
      name: `Section ${s + 1}`,
      direction: 'in' as const,
      shots: Array.from({ length: 3 }, (_, idx) => buildShot(idx + s * 3)),
    }))
  );

  await waitForMapReady(page);
  await waitForNoDialogs(page);

  const ids = await page.evaluate(() => {
    const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
    return (state.project?.sections ?? []).map((s) => s.id) as number[];
  });
  if (ids.length !== sectionCount) {
    throw new Error(`Expected ${sectionCount} sections, got ${ids.length}`);
  }
  return ids;
}

async function openContextMenu(page: Page, target: Record<string, unknown>) {
  await page.evaluate(
    ({ target }) => {
      (window as E2EWindow).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
        target,
        position: { x: 140, y: 140 },
      });
    },
    { target }
  );
}

/**
 * Make the mock answer the post-reverse re-resolution of a 3-shot section's anchors the way
 * the backend does (station i becomes station 3 - i), for the station-ref-keeping tests.
 */
async function mirrorAnchorsOnReverse(page: Page, sectionId: number): Promise<void> {
  await page.evaluate((id) => {
    const resolution: Record<string, StationRef> = {};
    for (let i = 0; i <= 3; i += 1) resolution[`${id}:${i}`] = { section_id: id, station_index: 3 - i };
    (window as E2EWindow).__MOCK_SET_ANCHOR_RESOLUTION__!(resolution);
  }, sectionId);
}

/** The frontend asked the backend to re-resolve its anchors AFTER the reverse it sent. */
async function expectReResolvedAfterTheReverse(page: Page): Promise<void> {
  await expect
    .poll(async () => {
      const order = (await getIpcCalls(page)).map((c) => c.cmd);
      return order.indexOf('plan_refs_for_anchors', order.indexOf('reverse_section')) > 0;
    })
    .toBe(true);
}

async function reverseFromSectionMenu(page: Page, sectionId: number): Promise<void> {
  await openContextMenu(page, { type: 'section', sectionId });
  await page.getByRole('button', { name: 'Reverse Section' }).click();
  const dialog = page.getByTestId('reverse-section-dialog');
  await expect(dialog).toBeVisible();
  await page.getByTestId('reverse-confirm').click();
  await expect(dialog).not.toBeVisible({ timeout: 5000 });
  await waitForNoDialogs(page);
}

test.describe('Reverse Section', () => {
  test('is offered from the sections panel, map station and map shot menus', async ({ page }) => {
    const [sectionId] = await setup(page);
    const shotIds = await page.evaluate((id) => {
      const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
      return state.project!.sections
        .find((s) => s.id === id)!
        .shots.map((s) => s.id);
    }, sectionId);

    for (const target of [
      { type: 'section', sectionId },
      { type: 'station', ref: { section_id: sectionId, station_index: 0 } as StationRef },
      { type: 'shot', sectionId, shotId: shotIds[0] },
    ]) {
      await openContextMenu(page, target);
      await expect(page.getByRole('button', { name: 'Reverse Section' })).toBeVisible();
      await page.keyboard.press('Escape');
      await waitForNoDialogs(page);
    }
  });

  test('is absent from the shot-scoped datatable menu', async ({ page }) => {
    const [sectionId] = await setup(page);
    const shotIds = await page.evaluate((id) => {
      const state = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
      return state.project!.sections.find((s) => s.id === id)!.shots.map((s) => s.id);
    }, sectionId);

    await openContextMenu(page, {
      type: 'datatable-shot',
      sectionId,
      shotId: shotIds[1],
      isFirst: false,
      isLast: false,
      totalShots: 3,
      sectionName: 'Section 1',
    });
    await expect(page.getByRole('button', { name: 'Reverse Section' })).toHaveCount(0);
  });

  test('confirming sends the reverse, leaving left/right alone by default', async ({ page }) => {
    const [sectionId] = await setup(page);
    await recordIpcCalls(page, ['reverse_section', 'execute_batch']);

    await openContextMenu(page, { type: 'section', sectionId });
    await page.getByRole('button', { name: 'Reverse Section' }).click();

    const dialog = page.getByTestId('reverse-section-dialog');
    await expect(dialog).toBeVisible();
    // The left/right choice is present and starts unticked.
    await expect(page.getByTestId('reverse-swap-lr')).not.toBeChecked();
    await page.getByTestId('reverse-confirm').click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // One single-section reverse reached the backend, without the left/right swap.
    const [call] = await waitForIpcCall(page, 'reverse_section');
    expect(call.args).toEqual({ sectionId, swapLeftRight: false });
    expect(await getIpcCalls(page, 'reverse_section')).toHaveLength(1);
    expect(await getIpcCalls(page, 'execute_batch')).toHaveLength(0);
  });

  test('opting in sends the left/right swap with the reverse', async ({ page }) => {
    const [sectionId] = await setup(page);
    await recordIpcCalls(page, ['reverse_section']);

    await openContextMenu(page, { type: 'section', sectionId });
    await page.getByRole('button', { name: 'Reverse Section' }).click();
    await page.getByTestId('reverse-swap-lr').check();
    await page.getByTestId('reverse-confirm').click();
    await expect(page.getByTestId('reverse-section-dialog')).not.toBeVisible({ timeout: 5000 });

    const [call] = await waitForIpcCall(page, 'reverse_section');
    expect(call.args).toEqual({ sectionId, swapLeftRight: true });
  });

  test('offers a separate entry for a multi-section selection', async ({ page }) => {
    const ids = await setup(page, 2);

    await page.evaluate((sectionIds) => {
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      ui.setSelection({ type: 'section', sectionId: sectionIds[0] });
      ui.addToSelection({ type: 'section', sectionId: sectionIds[1] });
    }, ids);

    await openContextMenu(page, { type: 'section', sectionId: ids[0] });
    await expect(page.getByRole('button', { name: 'Reverse Section', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Reverse 2 Sections' })).toBeVisible();

    await page.getByRole('button', { name: 'Reverse 2 Sections' }).click();
    const dialog = page.getByTestId('reverse-section-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('reverse-section-list').locator('li')).toHaveCount(2);
  });

  test('reverses every selected section as one batched action', async ({ page }) => {
    const ids = await setup(page, 2);

    await page.evaluate((sectionIds) => {
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      ui.setSelection({ type: 'section', sectionId: sectionIds[0] });
      ui.addToSelection({ type: 'section', sectionId: sectionIds[1] });
    }, ids);

    await openContextMenu(page, { type: 'section', sectionId: ids[0] });
    await page.getByRole('button', { name: 'Reverse 2 Sections' }).click();
    await expect(page.getByTestId('reverse-section-dialog')).toBeVisible();
    await page.getByTestId('reverse-confirm').click();
    await waitForNoDialogs(page);

    // One batch, carrying a reverse of both sections - not one command per section,
    // which would half-apply on failure and need one Undo each.
    await expect
      .poll(() => page.evaluate(() => ((window as E2EWindow).__MOCK_EXECUTE_BATCH_CALLS__ ?? []).length))
      .toBe(1);
    const batches = await page.evaluate(
      () => (window as E2EWindow).__MOCK_EXECUTE_BATCH_CALLS__ as unknown[] | undefined
    );
    expect(batches).toHaveLength(1);
    expect(batches![0]).toEqual([
      { type: 'reverse_section', section_id: ids[0], swap_left_right: false },
      { type: 'reverse_section', section_id: ids[1], swap_left_right: false },
    ]);
  });

  test('shift-clicking sections panel rows reaches the multi-section entry', async ({ page }) => {
    // The multi entry is gated on `multiSelection` holding >1 section, and the
    // sections panel is the ONLY affordance that can produce that - every other
    // addToSelection call site emits a station or a shot. Driven through real
    // clicks rather than page.evaluate, so it proves the entry is reachable.
    const ids = await setup(page, 2);

    await page.getByTestId(`section-row-${ids[0]}`).click();
    await page.getByTestId(`section-row-${ids[1]}`).click({ modifiers: ['Shift'] });

    await openContextMenu(page, { type: 'section', sectionId: ids[0] });
    await expect(page.getByRole('button', { name: 'Reverse 2 Sections' })).toBeVisible();
  });

  test('ctrl-clicking sections panel rows toggles them in and out of the selection', async ({
    page,
  }) => {
    // The ctrl/meta branch of `handleSectionClick` is a behaviour change to an
    // existing widget (#24) and no other spec modifier-clicks a section row.
    // Unlike Shift's addToSelection, it TOGGLES - a second ctrl-click removes
    // the row again, which is what drops the menu back to the single entry.
    const ids = await setup(page, 2);
    const selectedSectionIds = () =>
      page.evaluate(() =>
        ((window as E2EWindow).__E2E_TEST__.uiStore.getState().multiSelection ?? [])
          .filter((s) => s.type === 'section')
          .map((s) => s.sectionId as number)
      );

    await page.getByTestId(`section-row-${ids[0]}`).click();
    await page.getByTestId(`section-row-${ids[1]}`).click({ modifiers: ['ControlOrMeta'] });
    expect(await selectedSectionIds()).toEqual([ids[0], ids[1]]);

    await openContextMenu(page, { type: 'section', sectionId: ids[0] });
    await expect(page.getByRole('button', { name: 'Reverse 2 Sections' })).toBeVisible();
    // Dismissed through the store rather than Escape: the Escape shortcut also
    // CLEARS the selection, which would hide whether the second ctrl-click below
    // toggled the row off or the keypress did it.
    await page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().closeDialog());
    await waitForNoDialogs(page);

    await page.getByTestId(`section-row-${ids[1]}`).click({ modifiers: ['ControlOrMeta'] });
    expect(await selectedSectionIds()).toEqual([ids[0]]);

    await openContextMenu(page, { type: 'section', sectionId: ids[0] });
    await expect(page.getByRole('button', { name: 'Reverse Section', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /Reverse \d+ Sections/ })).toHaveCount(0);
  });

  test('offers only the single entry for a section outside the multi-selection', async ({
    page,
  }) => {
    // Right-clicking a section that is NOT part of the selection must act on the
    // section under the cursor alone - offering "Reverse 2 Sections" there would
    // reverse two sections the user never pointed at.
    const ids = await setup(page, 3);

    await page.evaluate((sectionIds) => {
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      ui.setSelection({ type: 'section', sectionId: sectionIds[0] });
      ui.addToSelection({ type: 'section', sectionId: sectionIds[1] });
    }, ids);

    await openContextMenu(page, { type: 'section', sectionId: ids[2] });
    await expect(page.getByRole('button', { name: 'Reverse Section', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /Reverse \d+ Sections/ })).toHaveCount(0);

    // And it reverses only the section that was right-clicked.
    await page.getByRole('button', { name: 'Reverse Section', exact: true }).click();
    await expect(page.getByTestId('reverse-section-list').locator('li')).toHaveCount(1);
    await expect(page.getByTestId('reverse-section-list')).toContainText('Section 3');
  });

  test('counts a repeated section in the selection once', async ({ page }) => {
    // The menu dedupes `multiSelection` before counting: a repeated id would
    // reverse that section twice in the batch (the second a silent no-op) and
    // inflate the count the label promises. The store's own add/toggle paths
    // dedupe, so the duplicate is injected directly.
    const ids = await setup(page, 2);

    await page.evaluate((sectionIds) => {
      (window as E2EWindow).__E2E_TEST__.uiStore.setState({
        selection: { type: 'section', sectionId: sectionIds[0] },
        multiSelection: [
          { type: 'section', sectionId: sectionIds[0] },
          { type: 'section', sectionId: sectionIds[1] },
          { type: 'section', sectionId: sectionIds[0] },
        ],
      });
    }, ids);

    await openContextMenu(page, { type: 'section', sectionId: ids[0] });
    await expect(page.getByRole('button', { name: 'Reverse 2 Sections' })).toBeVisible();
    await expect(page.getByRole('button', { name: /Reverse 3 Sections/ })).toHaveCount(0);

    await page.getByRole('button', { name: 'Reverse 2 Sections' }).click();
    await expect(page.getByTestId('reverse-section-list').locator('li')).toHaveCount(2);
    await expect(page.getByTestId('reverse-confirm')).toHaveText('Reverse 2 Sections');
  });

  test('surfaces a rejected reverse and leaves the dialog open to retry', async ({ page }) => {
    // The read-only self-check has its own test; this is the OTHER error surface -
    // a refusal from the backend (a `CommandError` rejection), whose message must
    // render verbatim rather than closing the dialog on a reverse that never happened.
    const [sectionId] = await setup(page);
    await recordIpcCalls(page, ['reverse_section']);

    await page.evaluate(() => {
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = {
        reverse_section: { __reject: { code: 'FAILED', message: 'Section is locked by another editor' } satisfies CommandError },
      };
    });

    await openContextMenu(page, { type: 'section', sectionId });
    await page.getByRole('button', { name: 'Reverse Section' }).click();
    await page.getByTestId('reverse-confirm').click();

    await expect(page.getByTestId('reverse-error')).toHaveText('Section is locked by another editor');
    await expect(page.getByTestId('reverse-section-dialog')).toBeVisible();
    // Re-armed, so the user can retry once the cause is cleared.
    await expect(page.getByTestId('reverse-confirm')).toBeEnabled();
    // The message is the backend's answer to the one request that was sent.
    expect((await getIpcCalls(page, 'reverse_section')).map((c) => c.args)).toEqual([
      { sectionId, swapLeftRight: false },
    ]);
  });

  test('Cancel closes the dialog without reversing anything', async ({ page }) => {
    const [sectionId] = await setup(page);
    await recordIpcCalls(page, ['reverse_section', 'execute_batch']);
    await page.evaluate(() =>
      (window as E2EWindow).__E2E_TEST__.projectStore.setState({ isDirty: false })
    );

    await openContextMenu(page, { type: 'section', sectionId });
    await page.getByRole('button', { name: 'Reverse Section' }).click();
    const dialog = page.getByTestId('reverse-section-dialog');
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();

    await expect(dialog).not.toBeVisible();
    // Nothing reached the backend, so the project is still clean.
    expect(
      await page.evaluate(
        () => (window as E2EWindow).__E2E_TEST__.projectStore.getState().isDirty as boolean
      )
    ).toBe(false);
    expect(await getIpcCalls(page)).toEqual([]);
  });

  test('moves the gas plan pins with the sections they sit on', async ({ page }) => {
    // Plan pins are positional StationRefs the backend cannot reach, so a reverse
    // leaves them in range while they name the opposite physical station - the
    // plan would silently re-price a different route.
    const [sectionId] = await setup(page);

    await page.evaluate((id) => {
      const gas = (window as E2EWindow).__E2E_TEST__.gasPlanStore.getState();
      gas.setOrigin({ section_id: id, station_index: 0 });
      gas.setEndpoint({ section_id: id, station_index: 1 });
    }, sectionId);
    // [#576 E3] The backend re-places an anchored pin after the reverse (3 shots -> 4
    // stations: 0 becomes 3, 1 becomes 2 - pinned by core dive_plan
    // `anchored_pins_survive_a_section_reverse`); the mock answers what the spec says it
    // does. What is asserted here is the frontend's half: it re-resolves its pins around
    // the edit and applies the answer.
    await mirrorAnchorsOnReverse(page, sectionId);
    await recordIpcCalls(page, ['reverse_section', 'plan_refs_for_anchors']);

    await reverseFromSectionMenu(page, sectionId);
    await expectReResolvedAfterTheReverse(page);

    const pins = await page.evaluate(() => {
      const gas = (window as E2EWindow).__E2E_TEST__.gasPlanStore.getState();
      return { origin: gas.origin, endpoint: gas.endpoint };
    });
    // 3 shots -> 4 stations: 0 mirrors to 3, 1 mirrors to 2.
    expect(pins.origin).toEqual({ section_id: sectionId, station_index: 3 });
    expect(pins.endpoint).toEqual({ section_id: sectionId, station_index: 2 });
  });

  test('moves the loaded-plan baseline with the pins, so the plan stays clean', async ({
    page,
  }) => {
    // The baseline holds the same POSITIONAL refs as the live pins. Mirroring one without the
    // other makes selectIsPlanDirty compare two different numberings and report a plan the
    // diver never edited as unsaved - nagging a save, or blocking a plan switch, over a route
    // that is still the same physical route.
    const [sectionId] = await setup(page);

    await page.evaluate((id) => {
      const gas = (window as E2EWindow).__E2E_TEST__.gasPlanStore.getState();
      const origin = { section_id: id, station_index: 0 };
      gas.setOrigin(origin);
      gas.applyLoadedPlan(1, {
        origin,
        endpoint: null,
        waypoints: [],
        segments: [],
        diver: gas.diver,
        cylinders: gas.cylinders,
        gas_config: gas.gasConfig,
        stage_drop_overrides: [],
        marked_points: [],
      });
    }, sectionId);

    await mirrorAnchorsOnReverse(page, sectionId);
    await recordIpcCalls(page, ['reverse_section', 'plan_refs_for_anchors']);

    await reverseFromSectionMenu(page, sectionId);
    await expectReResolvedAfterTheReverse(page);

    const after = await page.evaluate(() => {
      const gas = (window as E2EWindow).__E2E_TEST__.gasPlanStore.getState();
      return { origin: gas.origin, baseline: gas.loadedPlanBaseline?.origin ?? null };
    });
    expect(after.origin).toEqual({ section_id: sectionId, station_index: 3 });
    expect(after.baseline).toEqual({ section_id: sectionId, station_index: 3 });
  });

  test('clears the selection and any half-staged merge', async ({ page }) => {
    // Unlike the pins there is no "same place" for a highlight or a half-picked merge: a
    // staged operation that silently retargets the far end is worse than one the user has to
    // restart, so these are CLEARED rather than mirrored.
    const [sectionId] = await setup(page);

    await page.evaluate((id) => {
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      ui.setSelection({ type: 'section', sectionId: id });
      ui.setMergeStationA({ section_id: id, station_index: 0 });
    }, sectionId);

    // Precondition: both are actually set, so the assertions below cannot pass vacuously.
    const before = await page.evaluate(() => {
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      return { selection: ui.selection, mergeStationA: ui.mergeStationA };
    });
    expect(before.selection).not.toBeNull();
    expect(before.mergeStationA).not.toBeNull();

    await reverseFromSectionMenu(page, sectionId);

    const after = await page.evaluate(() => {
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      return { selection: ui.selection, mergeStationA: ui.mergeStationA };
    });
    expect(after.selection).toBeNull();
    expect(after.mergeStationA).toBeNull();
  });

  test('moves the distance-gradient origin with the section too', async ({ page }) => {
    // Same positional-ref hazard as the plan pins, and the origin's only other
    // reset is a project switch - so without this the penetration ramp silently
    // re-roots from the opposite end while still looking correct.
    const [sectionId] = await setup(page);

    await page.evaluate((id) => {
      (window as E2EWindow).__E2E_TEST__.uiStore
        .getState()
        .setDistanceGradientOrigin({ section_id: id, station_index: 0 });
    }, sectionId);
    await mirrorAnchorsOnReverse(page, sectionId);
    await recordIpcCalls(page, ['reverse_section', 'plan_refs_for_anchors']);

    await reverseFromSectionMenu(page, sectionId);
    await expectReResolvedAfterTheReverse(page);

    const origin = await page.evaluate(
      () => (window as E2EWindow).__E2E_TEST__.uiStore.getState().distanceGradientOrigin
    );
    expect(origin).toEqual({ section_id: sectionId, station_index: 3 });
  });

  test('refuses to confirm while the view is read-only', async ({ page }) => {
    // The closure-adjusted / Plan / Export read-only mode has NO server backstop,
    // and the mode can flip while the dialog is up - so the check has to run at
    // confirm time, not at open time.
    const [sectionId] = await setup(page);
    await recordIpcCalls(page, ['reverse_section', 'execute_batch']);

    await page.evaluate((id) => {
      const ui = (window as E2EWindow).__E2E_TEST__.uiStore.getState();
      ui.openDialog('reverse-section', { sectionIds: [id] });
      ui.setAppMode('plan');
    }, sectionId);

    await expect(page.getByTestId('reverse-section-dialog')).toBeVisible();
    await page.getByTestId('reverse-confirm').click();

    await expect(page.getByTestId('reverse-error')).toBeVisible();
    // The dialog stays up and no reverse was sent.
    await expect(page.getByTestId('reverse-section-dialog')).toBeVisible();
    expect(await getIpcCalls(page)).toEqual([]);
  });

  test('disables confirm when a listed section has gone away', async ({ page }) => {
    // The count, the label and the submitted batch all come off the RESOLVED
    // sections, so a section deleted between opening the menu and confirming
    // can't show "Reverse 2 Sections" over a 1-item list and then fail the whole
    // batch.
    const ids = await setup(page, 2);

    await page.evaluate((sectionIds) => {
      (window as E2EWindow).__E2E_TEST__.uiStore
        .getState()
        .openDialog('reverse-section', { sectionIds });
    }, ids);
    await expect(page.getByTestId('reverse-section-dialog')).toBeVisible();
    await expect(page.getByTestId('reverse-confirm')).toHaveText('Reverse 2 Sections');

    await page.evaluate(
      (id) => (window as E2EWindow).__E2E_TEST__.projectStore.getState().deleteSection(id),
      ids[1]
    );

    await expect(page.getByTestId('reverse-section-list').locator('li')).toHaveCount(1);
    await expect(page.getByTestId('reverse-confirm')).toHaveText('Reverse');
    await expect(page.getByTestId('reverse-confirm')).toBeDisabled();
    await expect(page.getByTestId('reverse-stale')).toBeVisible();
  });

  test('does not dirty the project for a section with no shots', async ({ page }) => {
    // A section with no shots is a single station and reverses to itself, so the
    // backend deliberately changes nothing - sending it anyway would cost an
    // unsaved-changes marker (and an Undo) for a provable no-op.
    await setupMockedPage(page);
    await createProject(page, 'Reverse Empty');
    await seedProjectWithSections(page, [{ name: 'Empty', direction: 'in', shots: [] }]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    const sectionId = await page.evaluate(
      () => (window as E2EWindow).__E2E_TEST__.projectStore.getState().project!.sections[0]!.id as number
    );
    await page.evaluate(() =>
      (window as E2EWindow).__E2E_TEST__.projectStore.setState({ isDirty: false })
    );

    // The dialog now REFUSES rather than submitting a no-op: with nothing that would
    // actually change, Confirm is disabled, so the user is not offered an action that
    // would close the dialog and fire a completed-reverse event having done nothing.
    await page.evaluate((id) => {
      (window as E2EWindow).__E2E_TEST__.uiStore
        .getState()
        .openDialog('reverse-section', { sectionIds: [id] });
    }, sectionId);
    await expect(page.getByTestId('reverse-section-dialog')).toBeVisible();
    await expect(page.getByTestId('reverse-confirm')).toBeDisabled();

    await page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().closeDialog());

    expect(
      await page.evaluate(
        () => (window as E2EWindow).__E2E_TEST__.projectStore.getState().isDirty as boolean
      )
    ).toBe(false);
  });

  test('Edit Section dialog disables Reverse only while dirty', async ({ page }) => {
    const [sectionId] = await setup(page);

    await page.evaluate((id) => {
      (window as E2EWindow).__E2E_TEST__.uiStore.getState().openDialog('edit-section', { sectionId: id });
    }, sectionId);
    await expect(page.getByTestId('edit-section-dialog')).toBeVisible();

    const reverseButton = page.getByTestId('edit-section-reverse');
    await expect(reverseButton).toBeEnabled();

    // Dirty the dialog via its section-name field.
    await page.getByTestId('section-name-input').fill('Renamed while editing');
    await expect(reverseButton).toBeDisabled();
    await expect(reverseButton).toHaveAttribute('title', /Save or discard/);
  });
});
