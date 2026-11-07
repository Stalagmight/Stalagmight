/**
 * E2E tests for the DataTable header section pill.
 *
 * The condensed DataTable header shows the current section as a pill
 * (`data-table-section-pill`). Hovering the pill highlights the section on the
 * map via `hoveredSectionId` in uiStore; leaving the pill (or unmounting the
 * DataTable) clears the highlight. Clicking the whole pill opens the section
 * picker dropdown (the chevron lives inside the pill); picking a section selects
 * it app-wide (uiStore selection of type 'section').
 */

import { test, expect, Page, Locator } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapIdle,
  waitForNoDialogs,
  openAppMenu,
  type SectionSeed,
} from './helpers/app';

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Alpha Passage',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
    ],
  },
  {
    name: 'Beta Tunnel',
    direction: 'out',
    shots: [
      { azimuth_in: 180, azimuth_out: 0, length: 7, depth_in: -2, depth_out: -4 },
    ],
  },
  {
    name: 'Gamma Gallery',
    direction: 'in',
    shots: [
      { azimuth_in: 270, azimuth_out: 90, length: 5, depth_in: -1, depth_out: -2 },
      { azimuth_in: 300, azimuth_out: 120, length: 8, depth_in: -2, depth_out: -5 },
      { azimuth_in: 330, azimuth_out: 150, length: 6, depth_in: -5, depth_out: -7 },
    ],
  },
];

/**
 * Setup: create project with sections for all tests.
 */
async function createProjectWithSections(page: Page): Promise<void> {
  await createProject(page, 'Pill Hover Test');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

/**
 * Open the DataTable via Ctrl+3.
 */
async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  // The Dock mounts the table only when the panel is revealed, so visibility
  // confirms it opened (the redundant data-open attribute was removed).
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
}

/**
 * Get the header section pill button.
 */
function getSectionPill(page: Page): Locator {
  return page.getByTestId('data-table-section-pill');
}

/**
 * Get the current hoveredSectionId from uiStore.
 */
async function getHoveredSectionId(page: Page): Promise<number | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { hoveredSectionId: number | null } };
      };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().hoveredSectionId ?? null;
  });
}

/**
 * Poll for hoveredSectionId to satisfy a predicate.
 */
async function pollHoveredSectionId(
  page: Page,
  predicate: (id: number | null) => boolean,
  timeout = 5000,
): Promise<number | null> {
  let result: number | null = null;
  await expect.poll(
    async () => {
      const id = await getHoveredSectionId(page);
      if (predicate(id)) {
        result = id;
        return true;
      }
      return false;
    },
    { timeout },
  ).toBe(true);
  return result;
}

/**
 * Get section ID by name from the project store.
 */
async function getSectionIdByName(page: Page, name: string): Promise<number> {
  const id = await page.evaluate((sectionName) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { project: { sections: Array<{ id: number; name: string }> } | null };
        };
      };
    }).__E2E_TEST__;
    const section = stores?.projectStore.getState().project?.sections.find(s => s.name === sectionName);
    return section?.id ?? -1;
  }, name);
  expect(id).toBeGreaterThan(0);
  return id;
}

/**
 * Drive the table to a section via the app-wide selection (the table follows
 * external selection; its header no longer has a section dropdown).
 */
async function selectSectionViaStore(page: Page, sectionId: number): Promise<void> {
  await page.evaluate((id) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { setSelection: (sel: unknown) => void } };
      };
    }).__E2E_TEST__;
    stores?.uiStore.getState().setSelection({ type: 'section', sectionId: id });
  }, sectionId);
}

test.describe('section pill hover highlighting', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithSections(page);
    await openDataTable(page);
  });

  test('hovering the section pill sets hoveredSectionId', async ({ page }) => {
    // The table auto-selects the first section (Alpha Passage)
    const pill = getSectionPill(page);
    await expect(pill).toContainText('Alpha Passage');

    await pill.hover();

    const alphaId = await getSectionIdByName(page, 'Alpha Passage');
    const hoveredId = await pollHoveredSectionId(page, (id) => id === alphaId);
    expect(hoveredId).toBe(alphaId);
  });

  test('pill hover tracks the externally-selected section', async ({ page }) => {
    const betaId = await getSectionIdByName(page, 'Beta Tunnel');

    // Switch the table to Beta Tunnel via the app-wide selection
    await selectSectionViaStore(page, betaId);
    const pill = getSectionPill(page);
    await expect(pill).toContainText('Beta Tunnel');

    // Hovering the pill must highlight Beta Tunnel, not the previous section
    await pill.hover();
    const hoveredId = await pollHoveredSectionId(page, (id) => id === betaId);
    expect(hoveredId).toBe(betaId);
  });

  test('leaving the pill clears hoveredSectionId', async ({ page }) => {
    const pill = getSectionPill(page);
    await pill.hover();

    const alphaId = await getSectionIdByName(page, 'Alpha Passage');
    await pollHoveredSectionId(page, (id) => id === alphaId);

    // Move the pointer off the pill (onto the view toggle in the same header)
    await page.getByTestId('data-table-view-select').hover();

    const clearedId = await pollHoveredSectionId(page, (id) => id === null);
    expect(clearedId).toBeNull();
  });

  test('clicking the pill opens the section picker; picking selects app-wide', async ({ page }) => {
    const pill = getSectionPill(page);
    await expect(pill).toContainText('Alpha Passage');

    // Clicking the whole pill opens the dropdown (the chevron is inside it).
    await pill.click();
    const list = page.getByTestId('data-table-section-menu-list');
    await expect(list).toBeVisible();
    await expect(list.getByRole('button')).toHaveText([
      'Alpha Passage',
      'Beta Tunnel',
      'Gamma Gallery',
    ]);

    // Picking a section selects it app-wide and updates the pill.
    const betaId = await getSectionIdByName(page, 'Beta Tunnel');
    await list.getByRole('button', { name: 'Beta Tunnel' }).click();
    await expect(list).not.toBeVisible();
    await expect(pill).toContainText('Beta Tunnel');

    await expect.poll(async () =>
      page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { selection: { type: string; sectionId?: number } | null } };
          };
        }).__E2E_TEST__;
        return stores?.uiStore.getState().selection ?? null;
      }),
    ).toEqual({ type: 'section', sectionId: betaId });

    // The sections panel reflects the app-wide selection too
    await expect(page.locator(`[data-testid="section-row-${betaId}"]`)).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  test('unmounting the DataTable clears hoveredSectionId', async ({ page }) => {
    const pill = getSectionPill(page);
    await pill.hover();

    const alphaId = await getSectionIdByName(page, 'Alpha Passage');
    await pollHoveredSectionId(page, (id) => id === alphaId);

    // Navigate back to the GlobalView, which unmounts the DataTable. Its
    // cleanup effect must clear the highlight it set. The seeded project has
    // unsaved changes, so discard them when prompted.
    await openAppMenu(page);
    await page.getByTestId('back-button').click();
    const unsavedDialog = page.getByTestId('unsaved-changes-dialog');
    await expect(unsavedDialog).toBeVisible({ timeout: 3000 });
    await unsavedDialog.locator('button:has-text("Discard")').click();
    await expect(page.getByTestId('global-view')).toBeVisible({ timeout: 10000 });

    const clearedId = await pollHoveredSectionId(page, (id) => id === null);
    expect(clearedId).toBeNull();
  });
});

/**
 * Regression: hovering a section (e.g. a picker option, or map geometry) drives
 * the Sections panel's debounced auto-scroll. If that off-screen scroll fires
 * while the picker is open, the picker's outside-scroll dismissal would close it
 * - so it must close "randomly" a moment after the pointer settled. The panel
 * auto-scroll is suppressed while a dropdown is open; the picker stays open.
 */
test.describe('section picker is not dismissed by background auto-scroll', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Picker Auto-Scroll');
    const many: SectionSeed[] = Array.from({ length: 40 }, (_, i) => ({
      name: `Section ${String(i).padStart(2, '0')}`,
      direction: 'in',
      shots: [{ azimuth_in: 45, azimuth_out: 225, length: 5, depth_in: -1, depth_out: -2 }],
    }));
    await seedProjectWithSections(page, many);
    await waitForNoDialogs(page);
    await waitForMapIdle(page);
    await openDataTable(page);
  });

  test('hovering an off-screen section while the picker is open keeps it open', async ({ page }) => {
    // Park the sections list at the top so the last section's row is off-screen
    // (otherwise scrollIntoView is a no-op and there is nothing to reproduce).
    const list = page.getByTestId('sections-list');
    await expect(list).toBeVisible();
    await list.evaluate((el) => {
      el.scrollTop = 0;
    });

    // Open the picker.
    await getSectionPill(page).click();
    const menu = page.getByTestId('data-table-section-menu-list');
    await expect(menu).toBeVisible();

    // Hover an off-screen section (as hovering a picker option / map geometry
    // does) - this would auto-scroll the panel behind the picker after 120ms.
    const lastId = await page.evaluate(() => {
      const stores = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: { getState: () => { project: { sections: Array<{ id: number }> } | null } };
          uiStore: { getState: () => { setHoveredSectionId: (id: number | null) => void } };
        };
      }).__E2E_TEST__;
      const ids = stores?.projectStore.getState().project?.sections.map((s) => s.id) ?? [];
      const id = ids[ids.length - 1] ?? null;
      if (id !== null) stores?.uiStore.getState().setHoveredSectionId(id);
      return id;
    });
    expect(lastId).not.toBeNull();

    // settle: well past the 120ms hover-scroll debounce - the picker must still
    // be open, which a poll would confirm before the debounce fired.
    await page.waitForTimeout(300);
    await expect(menu).toBeVisible();
    // Positively prove the auto-scroll was SUPPRESSED, not merely slow to fire:
    // the list must still be parked at the top. If the guard regressed, the
    // debounced scrollIntoView would have moved scrollTop off 0 (and the
    // resulting scroll event would have dismissed the picker). Asserting the
    // visible-menu alone could false-pass if the 120ms timer simply hadn't run
    // yet under a loaded runner.
    expect(await list.evaluate((el) => el.scrollTop)).toBe(0);
  });
});
