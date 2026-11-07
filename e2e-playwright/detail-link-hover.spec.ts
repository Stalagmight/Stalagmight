/**
 * E2E tests for DetailLink hover/focus highlighting.
 *
 * Tests verify that focusing DetailLink elements in the DetailPanel
 * highlights the corresponding elements on the map via uiStore state changes.
 *
 * IMPORTANT: These tests use focus() instead of hover() because:
 * 1. Playwright's hover() doesn't reliably trigger React's onMouseEnter handlers
 * 2. The DetailLink component treats focus and hover identically for highlighting
 * 3. Focus-based testing is more reliable in headless browsers
 *
 * The tests verify uiStore state rather than visual map styling because:
 * 1. The map styling is driven by uiStore state (verified in other tests)
 * 2. State verification is more reliable and faster than visual assertions
 * 3. MapLibre feature-state changes are not easily observable in Playwright
 */

import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapIdle,
  type SectionSeed,
} from './helpers/app';

/**
 * Test data with multiple sections, shots, and stations.
 */
const sectionSeeds: SectionSeed[] = [
  {
    name: 'Main Tunnel',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
      { azimuth_in: 120, azimuth_out: 300, length: 8, depth_in: -6, depth_out: -8 },
    ],
  },
  {
    name: 'Side Passage',
    direction: 'out',
    shots: [
      { azimuth_in: 180, azimuth_out: 0, length: 7, depth_in: -2, depth_out: -4 },
      { azimuth_in: 200, azimuth_out: 20, length: 5, depth_in: -4, depth_out: -5 },
    ],
  },
];

/**
 * Helper to create a project with test sections.
 */
async function createProjectWithSections(page: Page): Promise<void> {
  await createProject(page, 'Detail Link Hover Test');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapIdle(page);
}

/**
 * Helper to get section ID by name from the project store.
 * This avoids assuming section ordering is stable.
 */
async function getSectionIdByName(page: Page, name: string): Promise<number> {
  const sectionId = await page.evaluate((sectionName) => {
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
  expect(sectionId).toBeGreaterThan(0);
  return sectionId;
}

/**
 * Helper to get shot IDs for a section.
 */
async function getShotIdsForSection(page: Page, sectionId: number): Promise<number[]> {
  return page.evaluate((secId) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            project: { sections: Array<{ id: number; shots: Array<{ id: number }> }> } | null;
          };
        };
      };
    }).__E2E_TEST__;
    const section = stores?.projectStore.getState().project?.sections.find(s => s.id === secId);
    return section?.shots.map(shot => shot.id) ?? [];
  }, sectionId);
}

/**
 * Helper to get the current hover state from uiStore.
 */
async function getHoverState(page: Page): Promise<{
  hoveredStation: { section_id: number; station_index: number } | null;
  hoveredShot: { sectionId: number; shotId: number } | null;
  hoveredSectionId: number | null;
}> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: {
          getState: () => {
            hoveredStation: { section_id: number; station_index: number } | null;
            hoveredShot: { sectionId: number; shotId: number } | null;
            hoveredSectionId: number | null;
          };
        };
      };
    }).__E2E_TEST__;
    const state = stores?.uiStore.getState();
    return {
      hoveredStation: state?.hoveredStation ?? null,
      hoveredShot: state?.hoveredShot ?? null,
      hoveredSectionId: state?.hoveredSectionId ?? null,
    };
  });
}

/**
 * Helper to poll for hover state changes.
 */
async function pollHoverState(
  page: Page,
  predicate: (state: Awaited<ReturnType<typeof getHoverState>>) => boolean,
  timeout = 5000
): Promise<Awaited<ReturnType<typeof getHoverState>>> {
  let result: Awaited<ReturnType<typeof getHoverState>> = {
    hoveredStation: null,
    hoveredShot: null,
    hoveredSectionId: null,
  };
  await expect.poll(
    async () => {
      const state = await getHoverState(page);
      if (predicate(state)) {
        result = state;
        return true;
      }
      return false;
    },
    { timeout }
  ).toBe(true);
  return result;
}

/**
 * Helper to get the current selection state.
 */
async function getSelectionState(page: Page): Promise<{
  type: string;
  sectionId?: number;
  stationRef?: { section_id: number; station_index: number };
  shotId?: number;
} | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { selection: unknown } };
      };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().selection as {
      type: string;
      sectionId?: number;
      stationRef?: { section_id: number; station_index: number };
      shotId?: number;
    } | null;
  });
}

/**
 * Helper to poll for selection state changes.
 */
async function pollSelectionState(
  page: Page,
  predicate: (sel: Awaited<ReturnType<typeof getSelectionState>>) => boolean,
  timeout = 5000
): Promise<Awaited<ReturnType<typeof getSelectionState>>> {
  let result: Awaited<ReturnType<typeof getSelectionState>> = null;
  await expect.poll(
    async () => {
      const sel = await getSelectionState(page);
      if (predicate(sel)) {
        result = sel;
        return true;
      }
      return false;
    },
    { timeout }
  ).toBe(true);
  return result;
}

/**
 * Helper to select a station via store.
 */
async function selectStation(page: Page, sectionId: number, stationIndex: number): Promise<void> {
  await page.evaluate(({ secId, idx }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { setSelection: (sel: unknown) => void } };
      };
    }).__E2E_TEST__;
    if (stores) {
      stores.uiStore.getState().setSelection({
        type: 'station',
        stationRef: { section_id: secId, station_index: idx },
      });
    }
  }, { secId: sectionId, idx: stationIndex });

  // Wait for selection to be set
  await pollSelectionState(
    page,
    (sel) =>
      sel?.type === 'station' &&
      sel?.stationRef?.section_id === sectionId &&
      sel?.stationRef?.station_index === stationIndex
  );
}

/**
 * Helper to select a shot via store.
 */
async function selectShot(page: Page, sectionId: number, shotId: number): Promise<void> {
  await page.evaluate(({ secId, sId }) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { setSelection: (sel: unknown) => void } };
      };
    }).__E2E_TEST__;
    if (stores) {
      stores.uiStore.getState().setSelection({
        type: 'shot',
        sectionId: secId,
        shotId: sId,
      });
    }
  }, { secId: sectionId, sId: shotId });

  // Wait for selection to be set
  await pollSelectionState(
    page,
    (sel) => sel?.type === 'shot' && sel?.sectionId === sectionId && sel?.shotId === shotId
  );
}

/**
 * Helper to select a section by clicking its row.
 */
async function selectSectionById(page: Page, sectionId: number): Promise<void> {
  const sectionRow = page.locator(`[data-testid="section-row-${sectionId}"]`);
  await expect(sectionRow).toBeVisible();
  await sectionRow.click();

  // Wait for selection to be set
  await pollSelectionState(page, (sel) => sel?.type === 'section' && sel?.sectionId === sectionId);
}

/**
 * Helper to trigger mouse hover by dispatching mouseover/mouseout events with bubbling.
 * React's synthetic event system listens for bubbling events at the document root,
 * so we dispatch events that bubble up from the target element.
 */
async function triggerMouseHover(page: Page, testId: string): Promise<void> {
  await page.evaluate((tid) => {
    const el = document.querySelector(`[data-testid="${tid}"]`);
    if (el) {
      // Dispatch mouseover with bubbles: true so React's document-level listener catches it
      el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, cancelable: true }));
      // Also dispatch mouseenter for completeness (though React primarily uses mouseover)
      el.dispatchEvent(new MouseEvent('mouseenter', { bubbles: false, cancelable: false }));
    }
  }, testId);
}

/**
 * Helper to trigger mouse leave by dispatching mouseout/mouseleave events.
 */
async function triggerMouseLeave(page: Page, testId: string): Promise<void> {
  await page.evaluate((tid) => {
    const el = document.querySelector(`[data-testid="${tid}"]`);
    if (el) {
      // Dispatch mouseout with bubbles: true so React's document-level listener catches it
      el.dispatchEvent(new MouseEvent('mouseout', { bubbles: true, cancelable: true }));
      // Also dispatch mouseleave for completeness
      el.dispatchEvent(new MouseEvent('mouseleave', { bubbles: false, cancelable: false }));
    }
  }, testId);
}

test.describe('Detail Link Focus Highlighting', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test.describe('Station Link Focus', () => {
    test('focusing station link sets hoveredStation in uiStore', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show its detail panel with station links
      await selectShot(page, mainSectionId, firstShotId);

      // Find the Start Station link (station index 0, position 0, 0-indexed)
      const startStationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await expect(startStationLink).toBeVisible();

      // Check initial hover state
      const initialHoverState = await getHoverState(page);
      expect(initialHoverState.hoveredStation).toBeNull();

      // Focus the station link (triggers onFocus which calls onHoverStart)
      await startStationLink.focus();

      // Verify link is focused
      await expect(startStationLink).toBeFocused();

      // Verify hoveredStation is set correctly
      const hoverState = await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 0
      );
      expect(hoverState.hoveredStation?.section_id).toBe(mainSectionId);
      expect(hoverState.hoveredStation?.station_index).toBe(0);
    });

    test('blur clears hoveredStation', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show its detail panel
      await selectShot(page, mainSectionId, firstShotId);

      // Focus the Start Station link
      const startStationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await startStationLink.focus();

      // Wait for hover state to be set - use specific predicate
      await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 0
      );

      // Blur by focusing a non-DetailLink element (the center button)
      const centerButton = page.getByTestId('center-shot-button');
      await centerButton.focus();

      // Verify the link is no longer focused
      await expect(startStationLink).not.toBeFocused();

      // Verify hoveredStation is cleared (center button doesn't trigger hover state)
      const clearedState = await pollHoverState(page, (state) => state.hoveredStation === null);
      expect(clearedState.hoveredStation).toBeNull();
    });

    test('focusing different station links updates hoveredStation', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select first shot to show detail panel
      await selectShot(page, mainSectionId, firstShotId);

      // Focus Start Station link (index 0)
      const startStationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await startStationLink.focus();

      await pollHoverState(page, (state) => state.hoveredStation?.station_index === 0);

      // Now focus End Station link (index 1)
      const endStationLink = page.getByTestId(`station-${mainSectionId}-1-link`);
      await endStationLink.focus();

      // Verify hoveredStation updated to the new station
      const hoverState = await pollHoverState(
        page,
        (state) => state.hoveredStation?.station_index === 1
      );
      expect(hoverState.hoveredStation?.section_id).toBe(mainSectionId);
      expect(hoverState.hoveredStation?.station_index).toBe(1);
    });
  });

  test.describe('Shot Link Focus', () => {
    test('focusing shot link sets hoveredShot in uiStore', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);

      // Select station at index 1 (has both Previous and Next shots)
      await selectStation(page, mainSectionId, 1);

      // Find the Next Shot link (shot at index 1, position 2)
      const nextShotLink = page.getByTestId(`shot-${mainSectionId}-2-link`);
      await expect(nextShotLink).toBeVisible();

      // Focus the shot link
      await nextShotLink.focus();

      // Verify link is focused
      await expect(nextShotLink).toBeFocused();

      // Verify hoveredShot is set correctly
      const expectedShotId = shotIds[1]!;
      const hoverState = await pollHoverState(
        page,
        (state) =>
          state.hoveredShot !== null &&
          state.hoveredShot.sectionId === mainSectionId &&
          state.hoveredShot.shotId === expectedShotId
      );
      expect(hoverState.hoveredShot?.sectionId).toBe(mainSectionId);
      expect(hoverState.hoveredShot?.shotId).toBe(expectedShotId);
    });

    test('blur clears hoveredShot', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');

      // Select station at index 1
      await selectStation(page, mainSectionId, 1);

      // Focus the shot link
      const shotLink = page.getByTestId(`shot-${mainSectionId}-2-link`);
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const expectedShotId = shotIds[1]!;
      await shotLink.focus();

      // Wait for hover state to be set - use specific predicate
      await pollHoverState(
        page,
        (state) =>
          state.hoveredShot !== null &&
          state.hoveredShot.sectionId === mainSectionId &&
          state.hoveredShot.shotId === expectedShotId
      );

      // Blur by focusing a non-DetailLink element (the center button)
      const centerButton = page.getByTestId('center-station-button');
      await centerButton.focus();

      // Verify hoveredShot is cleared
      const clearedState = await pollHoverState(page, (state) => state.hoveredShot === null);
      expect(clearedState.hoveredShot).toBeNull();
    });
  });

  test.describe('Section Link Focus', () => {
    test('focusing section link sets hoveredSectionId in uiStore', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show its detail panel with section link
      await selectShot(page, mainSectionId, firstShotId);

      // Find the Section link
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await expect(sectionLink).toBeVisible();

      // Focus the section link
      await sectionLink.focus();

      // Verify link is focused
      await expect(sectionLink).toBeFocused();

      // Verify hoveredSectionId is set correctly
      const hoverState = await pollHoverState(
        page,
        (state) => state.hoveredSectionId === mainSectionId
      );
      expect(hoverState.hoveredSectionId).toBe(mainSectionId);
    });

    test('blur clears hoveredSectionId', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show detail panel
      await selectShot(page, mainSectionId, firstShotId);

      // Focus the section link
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await sectionLink.focus();

      // Wait for hover state to be set - use specific predicate
      await pollHoverState(page, (state) => state.hoveredSectionId === mainSectionId);

      // Blur by focusing a non-DetailLink element (the center button)
      const centerButton = page.getByTestId('center-shot-button');
      await centerButton.focus();

      // Verify hoveredSectionId is cleared
      const clearedState = await pollHoverState(page, (state) => state.hoveredSectionId === null);
      expect(clearedState.hoveredSectionId).toBeNull();
    });
  });

  test.describe('Focus Does Not Interfere with Selection', () => {
    test('focusing link does not change selection state', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot
      await selectShot(page, mainSectionId, firstShotId);

      // Focus a station link
      const startStationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await startStationLink.focus();

      // Verify hover state is set - use specific predicate
      await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 0
      );

      // Verify the shot is still selected (focus doesn't change selection)
      const selection = await getSelectionState(page);
      expect(selection).toMatchObject({
        type: 'shot',
        sectionId: mainSectionId,
        shotId: firstShotId,
      });
    });
  });

  test.describe('Section Detail Station Links', () => {
    test('focusing First Station link in section detail sets hover state', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');

      // Select the section to show section detail
      await selectSectionById(page, mainSectionId);

      // Find the First Station link (station index 0, position 0, 0-indexed)
      const firstStationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await expect(firstStationLink).toBeVisible();

      // Focus the link
      await firstStationLink.focus();

      // Verify link is focused
      await expect(firstStationLink).toBeFocused();

      // Verify hoveredStation is set
      const hoverState = await pollHoverState(page, (state) => state.hoveredStation !== null);
      expect(hoverState.hoveredStation?.section_id).toBe(mainSectionId);
      expect(hoverState.hoveredStation?.station_index).toBe(0);
    });

    test('focusing Last Station link in section detail sets hover state', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');

      // Select the section to show section detail
      await selectSectionById(page, mainSectionId);

      // Main section has 3 shots, so 4 stations (indices 0-3)
      // Last Station is at index 3, position 3 (0-indexed)
      const lastStationLink = page.getByTestId(`station-${mainSectionId}-3-link`);
      await expect(lastStationLink).toBeVisible();

      // Focus the link
      await lastStationLink.focus();

      // Verify link is focused
      await expect(lastStationLink).toBeFocused();

      // Verify hoveredStation is set to the last station
      const hoverState = await pollHoverState(page, (state) => state.hoveredStation !== null);
      expect(hoverState.hoveredStation?.section_id).toBe(mainSectionId);
      expect(hoverState.hoveredStation?.station_index).toBe(3);
    });
  });

  test.describe('Focus Transitions', () => {
    test('focusing from station to section link updates hover state correctly', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show detail panel with multiple links
      await selectShot(page, mainSectionId, firstShotId);

      // Focus station link first
      const stationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await stationLink.focus();

      // Verify station hover state - use specific predicate
      await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 0
      );
      let hoverState = await getHoverState(page);
      expect(hoverState.hoveredStation).not.toBeNull();
      expect(hoverState.hoveredSectionId).toBeNull();

      // Now focus section link
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await sectionLink.focus();

      // Verify section hover state - station should clear, section should set
      await pollHoverState(page, (state) => state.hoveredSectionId === mainSectionId);
      hoverState = await getHoverState(page);
      expect(hoverState.hoveredStation).toBeNull();
      expect(hoverState.hoveredSectionId).toBe(mainSectionId);
    });
  });

  test.describe('Mouse Hover Events', () => {
    // Test mouse hover using dispatched MouseEvents with bubbles: true.
    // React's synthetic event system captures bubbling mouse events at the document root.
    // We use triggerMouseHover/triggerMouseLeave helpers to dispatch proper events.

    test('hover on station link sets hoveredStation', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show its detail panel with station links
      await selectShot(page, mainSectionId, firstShotId);

      // Find the Start Station link (station index 0, position 0, 0-indexed)
      const stationLinkTestId = `station-${mainSectionId}-0-link`;
      const startStationLink = page.getByTestId(stationLinkTestId);
      await expect(startStationLink).toBeVisible();

      // Check initial hover state
      const initialState = await getHoverState(page);
      expect(initialState.hoveredStation).toBeNull();

      // Trigger hover via dispatched mouse events
      await triggerMouseHover(page, stationLinkTestId);

      // Verify hoveredStation is set correctly
      const hoverState = await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 0
      );
      expect(hoverState.hoveredStation?.section_id).toBe(mainSectionId);
      expect(hoverState.hoveredStation?.station_index).toBe(0);
    });

    test('mouse leave on station link clears hoveredStation', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show detail panel
      await selectShot(page, mainSectionId, firstShotId);

      const stationLinkTestId = `station-${mainSectionId}-0-link`;
      const startStationLink = page.getByTestId(stationLinkTestId);
      await expect(startStationLink).toBeVisible();

      // Trigger hover to set state
      await triggerMouseHover(page, stationLinkTestId);
      await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 0
      );

      // Trigger mouse leave to clear state
      await triggerMouseLeave(page, stationLinkTestId);

      // Verify hoveredStation is cleared
      const clearedState = await pollHoverState(page, (state) => state.hoveredStation === null);
      expect(clearedState.hoveredStation).toBeNull();
    });

    test('hover on shot link sets hoveredShot', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);

      // Select station at index 1 (has both Previous and Next shots)
      await selectStation(page, mainSectionId, 1);

      // Find the Next Shot link (shot at index 1, position 2)
      const shotLinkTestId = `shot-${mainSectionId}-2-link`;
      const nextShotLink = page.getByTestId(shotLinkTestId);
      await expect(nextShotLink).toBeVisible();

      // Check initial state
      const initialState = await getHoverState(page);
      expect(initialState.hoveredShot).toBeNull();

      // Trigger hover via dispatched mouse events
      await triggerMouseHover(page, shotLinkTestId);

      // Verify hoveredShot is set correctly
      const expectedShotId = shotIds[1]!;
      const hoverState = await pollHoverState(
        page,
        (state) =>
          state.hoveredShot !== null &&
          state.hoveredShot.sectionId === mainSectionId &&
          state.hoveredShot.shotId === expectedShotId
      );
      expect(hoverState.hoveredShot?.sectionId).toBe(mainSectionId);
      expect(hoverState.hoveredShot?.shotId).toBe(expectedShotId);
    });

    test('mouse leave on shot link clears hoveredShot', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const expectedShotId = shotIds[1]!;

      // Select station at index 1
      await selectStation(page, mainSectionId, 1);

      const shotLinkTestId = `shot-${mainSectionId}-2-link`;
      const nextShotLink = page.getByTestId(shotLinkTestId);
      await expect(nextShotLink).toBeVisible();

      // Trigger hover to set state
      await triggerMouseHover(page, shotLinkTestId);
      await pollHoverState(
        page,
        (state) =>
          state.hoveredShot !== null &&
          state.hoveredShot.sectionId === mainSectionId &&
          state.hoveredShot.shotId === expectedShotId
      );

      // Trigger mouse leave to clear state
      await triggerMouseLeave(page, shotLinkTestId);

      // Verify hoveredShot is cleared
      const clearedState = await pollHoverState(page, (state) => state.hoveredShot === null);
      expect(clearedState.hoveredShot).toBeNull();
    });

    test('hover on section link sets hoveredSectionId', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show detail panel with section link
      await selectShot(page, mainSectionId, firstShotId);

      // Find the section link
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await expect(sectionLink).toBeVisible();

      // Check initial state
      const initialState = await getHoverState(page);
      expect(initialState.hoveredSectionId).toBeNull();

      // Hover the section link
      await sectionLink.hover();

      // Verify hoveredSectionId is set
      const hoverState = await pollHoverState(
        page,
        (state) => state.hoveredSectionId === mainSectionId
      );
      expect(hoverState.hoveredSectionId).toBe(mainSectionId);
    });

    test('mouse leave on section link clears hoveredSectionId', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show detail panel
      await selectShot(page, mainSectionId, firstShotId);

      // Hover the section link
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await sectionLink.hover();

      // Wait for hover state to be set
      await pollHoverState(page, (state) => state.hoveredSectionId === mainSectionId);

      // Move mouse away to trigger mouse leave
      const centerButton = page.getByTestId('center-shot-button');
      await centerButton.hover();

      // Verify hoveredSectionId is cleared
      const clearedState = await pollHoverState(page, (state) => state.hoveredSectionId === null);
      expect(clearedState.hoveredSectionId).toBeNull();
    });

    test('section row hover works (verifies panel hover behavior)', async ({ page }) => {
      // Verify that section row hover in SectionsPanel works with page.hover()
      // This demonstrates that Playwright hover() does work for React elements
      await createProjectWithSections(page);

      const sideSectionId = await getSectionIdByName(page, 'Side Passage');

      // Initially no hover state
      const initialState = await getHoverState(page);
      expect(initialState.hoveredSectionId).toBeNull();

      // Hover over section row in Sections Panel
      const sectionRow = page.locator(`[data-testid="section-row-${sideSectionId}"]`);
      await expect(sectionRow).toBeVisible();
      await sectionRow.hover();

      // Verify hoveredSectionId is set
      const hoverState = await pollHoverState(
        page,
        (state) => state.hoveredSectionId === sideSectionId
      );
      expect(hoverState.hoveredSectionId).toBe(sideSectionId);
    });
  });

  test.describe('Keyboard Tab Navigation', () => {
    test('Tab navigates between DetailLinks and triggers hover', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show its detail panel with multiple links
      await selectShot(page, mainSectionId, firstShotId);

      // Focus the first station link
      const startStationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await startStationLink.focus();

      // Verify first station is hovered
      await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.station_index === 0
      );

      // Press Tab to move to next link (End Station)
      await page.keyboard.press('Tab');

      // Verify End Station link is now focused
      const endStationLink = page.getByTestId(`station-${mainSectionId}-1-link`);
      await expect(endStationLink).toBeFocused();

      // Verify hover state updated to station index 1
      const hoverState = await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.station_index === 1
      );
      expect(hoverState.hoveredStation?.section_id).toBe(mainSectionId);
      expect(hoverState.hoveredStation?.station_index).toBe(1);
    });

    test('Tab away from last link clears hover state', async ({ page }) => {
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show its detail panel
      await selectShot(page, mainSectionId, firstShotId);

      // Focus the section link (last link in the panel)
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await sectionLink.focus();

      // Verify section is hovered
      await pollHoverState(page, (state) => state.hoveredSectionId === mainSectionId);

      // Keep pressing Tab until we leave all DetailLinks
      // The panel has: Start Station, End Station, Section links
      // After Section link, Tab should go to action buttons
      await page.keyboard.press('Tab');

      // Verify section link is no longer focused
      await expect(sectionLink).not.toBeFocused();

      // Verify all hover states are cleared
      const clearedState = await pollHoverState(
        page,
        (state) =>
          state.hoveredStation === null &&
          state.hoveredShot === null &&
          state.hoveredSectionId === null
      );
      expect(clearedState.hoveredStation).toBeNull();
      expect(clearedState.hoveredShot).toBeNull();
      expect(clearedState.hoveredSectionId).toBeNull();
    });
  });

  test.describe('Coexistence with Other Hover Behaviors', () => {
    test('section row hover still sets hoveredSectionId', async ({ page }) => {
      // Verify that section row hover in SectionsPanel still works
      // and doesn't interfere with DetailLink hover
      await createProjectWithSections(page);

      // Kept for its existence check (getSectionIdByName asserts); the id is unused.
      await getSectionIdByName(page, 'Main Tunnel');
      const sideSectionId = await getSectionIdByName(page, 'Side Passage');

      // Initially no hover state
      const initialState = await getHoverState(page);
      expect(initialState.hoveredSectionId).toBeNull();

      // Hover over section row in Sections Panel
      const sectionRow = page.locator(`[data-testid="section-row-${sideSectionId}"]`);
      await expect(sectionRow).toBeVisible();
      await sectionRow.hover();

      // Verify hoveredSectionId is set for the row we hovered
      const hoverState = await pollHoverState(
        page,
        (state) => state.hoveredSectionId === sideSectionId
      );
      expect(hoverState.hoveredSectionId).toBe(sideSectionId);

      // Move mouse away to a neutral toolbar element (the workspace carousel in the
      // toolbar center; #38 replaced the active-tool label there).
      await page.getByTestId('mode-carousel').hover();

      // Verify hover state cleared
      await pollHoverState(page, (state) => state.hoveredSectionId === null);
    });

    test('DetailLink hover and section row hover use same state', async ({ page }) => {
      // Verify that both DetailLink and section row use the same
      // hoveredSectionId state, not separate states
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // Select a shot to show detail panel with section link
      await selectShot(page, mainSectionId, firstShotId);

      // Focus section link in detail panel
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await sectionLink.focus();

      // Verify section is hovered via detail link
      await pollHoverState(page, (state) => state.hoveredSectionId === mainSectionId);

      // Now hover a different section row (sections panel)
      const sideSectionId = await getSectionIdByName(page, 'Side Passage');
      const sectionRow = page.locator(`[data-testid="section-row-${sideSectionId}"]`);
      await sectionRow.hover();

      // Verify the hover state changed to the row's section
      // (the section link lost focus when we moved to hover)
      const newHoverState = await pollHoverState(
        page,
        (state) => state.hoveredSectionId === sideSectionId
      );
      expect(newHoverState.hoveredSectionId).toBe(sideSectionId);
    });

    test('map-sourced hover coexists with DetailLink hover (bidirectional state)', async ({ page }) => {
      // This test verifies that hover state can be set from either source (map or DetailLink)
      // and uses the same uiStore state. The actual map hover pipeline (SurveyLayer mousemove
      // -> queryRenderedFeatures -> onStationHover callback) is tested via the callbacks
      // that call uiStore setters. We simulate the "map hover" by calling the same store
      // setters that the SurveyLayer callbacks use.
      //
      // This matches the test approach in section-highlighting.spec.ts which documents:
      // "simulating map hover via store manipulation, since MapLibre layer events are
      // challenging to trigger reliably in Playwright. The actual MapLibre event handlers
      // are covered by unit tests."
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const sideSectionId = await getSectionIdByName(page, 'Side Passage');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // First, use DetailLink focus to set state (same callback as hover)
      await selectShot(page, mainSectionId, firstShotId);
      const sectionLink = page.getByTestId(`section-${mainSectionId}-link`);
      await sectionLink.focus();
      await pollHoverState(page, (state) => state.hoveredSectionId === mainSectionId);

      // Clear via blur
      const centerButton = page.getByTestId('center-shot-button');
      await centerButton.focus();
      await pollHoverState(page, (state) => state.hoveredSectionId === null);

      // Now simulate map hover by calling the same setter that SurveyLayer uses
      // in its onFeatureHover callback (SurveyLayer -> onSectionHover -> setHoveredSectionId)
      await page.evaluate((secId) => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { setHoveredSectionId: (id: number | null) => void } };
          };
        }).__E2E_TEST__;
        if (stores) {
          stores.uiStore.getState().setHoveredSectionId(secId);
        }
      }, sideSectionId);

      // Verify map-sourced hover state is set (proves same state path)
      const mapHoverState = await pollHoverState(
        page,
        (state) => state.hoveredSectionId === sideSectionId
      );
      expect(mapHoverState.hoveredSectionId).toBe(sideSectionId);

      // Clear map hover
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { setHoveredSectionId: (id: null) => void } };
          };
        }).__E2E_TEST__;
        if (stores) {
          stores.uiStore.getState().setHoveredSectionId(null);
        }
      });

      // Verify clears
      const clearedState = await pollHoverState(page, (state) => state.hoveredSectionId === null);
      expect(clearedState.hoveredSectionId).toBeNull();
    });

    test('map-sourced station hover coexists with DetailLink station hover', async ({ page }) => {
      // Same approach as above - verify bidirectional state sharing for station hover
      await createProjectWithSections(page);

      const mainSectionId = await getSectionIdByName(page, 'Main Tunnel');
      const shotIds = await getShotIdsForSection(page, mainSectionId);
      const firstShotId = shotIds[0]!;

      // First test DetailLink station focus (same callback as hover)
      await selectShot(page, mainSectionId, firstShotId);
      const stationLink = page.getByTestId(`station-${mainSectionId}-0-link`);
      await stationLink.focus();
      await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 0
      );

      // Clear via blur
      const centerButton = page.getByTestId('center-shot-button');
      await centerButton.focus();
      await pollHoverState(page, (state) => state.hoveredStation === null);

      // Now simulate map station hover (what SurveyLayer's onStationHover callback calls)
      await page.evaluate(({ secId }) => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: {
              getState: () => {
                setHoveredStation: (ref: { section_id: number; station_index: number } | null) => void;
              };
            };
          };
        }).__E2E_TEST__;
        if (stores) {
          stores.uiStore.getState().setHoveredStation({ section_id: secId, station_index: 2 });
        }
      }, { secId: mainSectionId });

      // Verify map-sourced station hover is set
      const hoverState = await pollHoverState(
        page,
        (state) =>
          state.hoveredStation !== null &&
          state.hoveredStation.section_id === mainSectionId &&
          state.hoveredStation.station_index === 2
      );
      expect(hoverState.hoveredStation?.section_id).toBe(mainSectionId);
      expect(hoverState.hoveredStation?.station_index).toBe(2);

      // Clear via store
      await page.evaluate(() => {
        const stores = (window as unknown as {
          __E2E_TEST__?: {
            uiStore: { getState: () => { setHoveredStation: (ref: null) => void } };
          };
        }).__E2E_TEST__;
        if (stores) {
          stores.uiStore.getState().setHoveredStation(null);
        }
      });

      // Verify cleared
      const clearedState = await pollHoverState(page, (state) => state.hoveredStation === null);
      expect(clearedState.hoveredStation).toBeNull();
    });
  });
});
