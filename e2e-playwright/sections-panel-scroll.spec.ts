import { test, expect, type Page, type Locator } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithSections, type SectionSeed } from './helpers/app';

/** Read every section id in project order. */
async function getSectionIds(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { sections: Array<{ id: number }> } | null } };
      };
    }).__E2E_TEST__;
    return stores?.projectStore.getState().project?.sections.map((s) => s.id) ?? [];
  });
}

/** Simulate an external (map / data table / details) hover via the store setter. */
async function setExternalSectionHover(page: Page, sectionId: number | null): Promise<void> {
  await page.evaluate((id) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { setHoveredSectionId: (id: number | null) => void } };
      };
    }).__E2E_TEST__;
    stores?.uiStore.getState().setHoveredSectionId(id);
  }, sectionId);
}

/** First shot id in a section, or -1. */
async function getFirstShotId(page: Page, sectionId: number): Promise<number> {
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
    const section = stores?.projectStore.getState().project?.sections.find((s) => s.id === secId);
    return section?.shots[0]?.id ?? -1;
  }, sectionId);
}

/** Set the selection via the store, as the map / data table / details panel do. */
async function setSelection(page: Page, selection: unknown): Promise<void> {
  await page.evaluate((sel) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { setSelection: (s: unknown) => void } } };
    }).__E2E_TEST__;
    stores?.uiStore.getState().setSelection(sel);
  }, selection);
}

/**
 * The list is now VIRTUALIZED (@tanstack/react-virtual, a #mem optimization for
 * 14k-section projects). Its `scrollToIndex` reveal seeds each off-screen row at
 * an ESTIMATED height, then re-measures the real height over the next few frames
 * and nudges scrollTop to compensate. Wait until scrollTop stops moving so a
 * following un-hover restore isn't overwritten by a late re-measure scroll, and
 * so a captured baseline reflects the settled (measured) layout.
 */
async function waitForScrollStable(_page: Page, list: Locator, timeout = 3000): Promise<number> {
  // The re-measure runs on animation frames, so wait for frames, not wall time:
  // scrollTop unchanged across STABLE_FRAMES consecutive frames (or the timeout).
  return list.evaluate(
    (el, { timeoutMs, stableFrames }) =>
      new Promise<number>((resolve) => {
        const start = performance.now();
        let prev = el.scrollTop;
        let stable = 0;
        const tick = () => {
          const cur = el.scrollTop;
          stable = cur === prev ? stable + 1 : 0;
          prev = cur;
          if (stable >= stableFrames || performance.now() - start > timeoutMs) resolve(cur);
          else requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      }),
    { timeoutMs: timeout, stableFrames: 5 },
  );
}

/**
 * Regression: the sections sidebar must scroll when its content overflows.
 *
 * The list lives in a `flex-1 overflow-auto` container, but its wrapper in
 * SingleSystemView was missing `h-full`, breaking the height chain so the panel
 * grew to fit all sections and got clipped by an `overflow-hidden` ancestor -
 * i.e. nothing scrolled (two-finger / wheel scroll did nothing).
 */
test.describe('Sections panel scrolling', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('sections list is scrollable when content overflows', async ({ page }) => {
    await createProject(page, 'Scroll Test');

    // Seed enough sections to overflow the sidebar at the default viewport.
    const seeds: SectionSeed[] = Array.from({ length: 40 }, (_, i) => ({
      name: `Section ${i + 1}`,
      shots: [],
    }));
    await seedProjectWithSections(page, seeds);

    const list = page.getByTestId('sections-list');
    await expect(list).toBeVisible();

    // The container must have a bounded height with overflowing content. If the
    // height chain is broken, clientHeight == scrollHeight (no overflow → no scroll).
    const { clientHeight, scrollHeight } = await list.evaluate((el) => ({
      clientHeight: el.clientHeight,
      scrollHeight: el.scrollHeight,
    }));
    expect(scrollHeight).toBeGreaterThan(clientHeight);

    // And it must actually scroll.
    await list.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    const scrolledTop = await list.evaluate((el) => el.scrollTop);
    expect(scrolledTop).toBeGreaterThan(0);
  });

  test('external hover auto-scrolls the list, then restores scroll on un-hover', async ({ page }) => {
    await createProject(page, 'Restore Test');
    const seeds: SectionSeed[] = Array.from({ length: 40 }, (_, i) => ({
      name: `Section ${i + 1}`,
      shots: [],
    }));
    await seedProjectWithSections(page, seeds);

    const list = page.getByTestId('sections-list');
    await expect(list).toBeVisible();

    const ids = await getSectionIds(page);
    expect(ids.length).toBe(40);
    const lastId = ids[ids.length - 1]!;

    // Park the list at the top - this is the position we expect restored.
    await list.evaluate((el) => {
      el.scrollTop = 0;
    });
    expect(await list.evaluate((el) => el.scrollTop)).toBe(0);

    // An external hover on the off-screen last section scrolls it into view.
    await setExternalSectionHover(page, lastId);
    await expect.poll(async () => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
    // Let the virtualizer's re-measure retries finish before un-hovering, so the
    // restore isn't clobbered by a late scrollToIndex nudge.
    await waitForScrollStable(page, list);

    // Clearing the hover (nothing hovered anywhere) restores the pre-hover scroll
    // (the top - captured before the auto-scroll moved the list).
    await setExternalSectionHover(page, null);
    await expect.poll(async () => list.evaluate((el) => el.scrollTop)).toBe(0);
  });

  test('restores to a non-zero manual scroll position after external hover', async ({ page }) => {
    await createProject(page, 'Restore Midscroll Test');
    const seeds: SectionSeed[] = Array.from({ length: 40 }, (_, i) => ({
      name: `Section ${i + 1}`,
      shots: [],
    }));
    await seedProjectWithSections(page, seeds);

    const list = page.getByTestId('sections-list');
    await expect(list).toBeVisible();

    const ids = await getSectionIds(page);
    const firstId = ids[0]!;
    const lastId = ids[ids.length - 1]!;

    // Manually scroll to the middle, then let the virtualizer settle so the
    // baseline reflects measured (not estimated) row heights.
    await list.evaluate((el) => {
      el.scrollTop = Math.round(el.scrollHeight / 2);
    });
    const baseline = await waitForScrollStable(page, list);
    expect(baseline).toBeGreaterThan(0);

    // Hover the last (below) section, then the first (above) section.
    await setExternalSectionHover(page, lastId);
    await expect.poll(async () => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(baseline);
    await setExternalSectionHover(page, firstId);
    await expect.poll(async () => list.evaluate((el) => el.scrollTop)).toBeLessThan(baseline);
    // Settle the "hover first" scroll before restoring.
    await waitForScrollStable(page, list);

    // Un-hover restores the original manual position, not either hovered extreme.
    // Allow a small virtualization drift: the restore lands on the pre-hover
    // scrollTop, but re-measuring rows around that offset can nudge it a few px -
    // far smaller than the hundreds-of-px gap to either hovered extreme.
    const RESTORE_TOLERANCE_PX = 40;
    await setExternalSectionHover(page, null);
    await expect
      .poll(async () => list.evaluate((el) => el.scrollTop))
      .toBeGreaterThan(baseline - RESTORE_TOLERANCE_PX);
    expect(await list.evaluate((el) => el.scrollTop)).toBeLessThan(baseline + RESTORE_TOLERANCE_PX);
  });

  test('selecting a shot scrolls to and selects the owning section row', async ({ page }) => {
    await createProject(page, 'Select Shot Scroll Test');
    // Only the last (off-screen) section has a shot to select.
    const seeds: SectionSeed[] = Array.from({ length: 40 }, (_, i) => ({
      name: `Section ${i + 1}`,
      shots:
        i === 39
          ? [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 }]
          : [],
    }));
    await seedProjectWithSections(page, seeds);

    const list = page.getByTestId('sections-list');
    await expect(list).toBeVisible();

    const ids = await getSectionIds(page);
    const lastId = ids[ids.length - 1]!;
    const lastRow = page.locator(`[data-testid="section-row-${lastId}"]`);

    // Park at top; the owning row starts unselected. It is off-screen, so the
    // virtualized list has it UNMOUNTED - assert it is not rendered as selected
    // (true whether or not virtualization has mounted it), instead of reading an
    // attribute off a row that may not be in the DOM yet.
    await list.evaluate((el) => {
      el.scrollTop = 0;
    });
    expect(await list.evaluate((el) => el.scrollTop)).toBe(0);
    await expect(page.locator(`[data-testid="section-row-${lastId}"][aria-selected="true"]`)).toHaveCount(0);

    // Select a shot in the last section, as the map / data table / details would.
    const shotId = await getFirstShotId(page, lastId);
    expect(shotId).toBeGreaterThan(0);
    await setSelection(page, { type: 'shot', sectionId: lastId, shotId });

    // The owning section row is now selected and revealed.
    await expect(lastRow).toHaveAttribute('aria-selected', 'true');
    await expect.poll(async () => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  });

  test('selecting a station scrolls to and selects the owning section row', async ({ page }) => {
    await createProject(page, 'Select Station Scroll Test');
    const seeds: SectionSeed[] = Array.from({ length: 40 }, (_, i) => ({
      name: `Section ${i + 1}`,
      shots:
        i === 39
          ? [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 }]
          : [],
    }));
    await seedProjectWithSections(page, seeds);

    const list = page.getByTestId('sections-list');
    await expect(list).toBeVisible();

    const ids = await getSectionIds(page);
    const lastId = ids[ids.length - 1]!;
    const lastRow = page.locator(`[data-testid="section-row-${lastId}"]`);

    // The owning row is off-screen (virtualized out) and un-selected. Assert it
    // is not rendered as selected, rather than reading an attribute off a row
    // that isn't mounted yet.
    await list.evaluate((el) => {
      el.scrollTop = 0;
    });
    await expect(page.locator(`[data-testid="section-row-${lastId}"][aria-selected="true"]`)).toHaveCount(0);

    // Select the section's first station (station_index 0), as the map would.
    await setSelection(page, {
      type: 'station',
      stationRef: { section_id: lastId, station_index: 0 },
    });

    await expect(lastRow).toHaveAttribute('aria-selected', 'true');
    await expect.poll(async () => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  });

  test('selecting during a hover anchors un-hover to the selection, not the pre-hover position', async ({ page }) => {
    await createProject(page, 'Hover Select Unhover Test');
    const seeds: SectionSeed[] = Array.from({ length: 40 }, (_, i) => ({
      name: `Section ${i + 1}`,
      shots:
        i === 39
          ? [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 }]
          : [],
    }));
    await seedProjectWithSections(page, seeds);

    const list = page.getByTestId('sections-list');
    await expect(list).toBeVisible();

    const ids = await getSectionIds(page);
    const lastId = ids[ids.length - 1]!;

    // Park at top - this is the pre-hover position that must NOT be restored.
    await list.evaluate((el) => {
      el.scrollTop = 0;
    });
    expect(await list.evaluate((el) => el.scrollTop)).toBe(0);

    // Hover the off-screen last section (as a map shot hover would) → auto-scroll.
    await setExternalSectionHover(page, lastId);
    await expect.poll(async () => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);

    // Click to select the shot while still hovering - selection becomes the anchor.
    const shotId = await getFirstShotId(page, lastId);
    await setSelection(page, { type: 'shot', sectionId: lastId, shotId });

    // Un-hover: the list stays at the selected section, not back at the top.
    await setExternalSectionHover(page, null);
    // settle: give the restore effect a chance to (wrongly) run, then assert it
    // didn't - a poll would accept the unchanged value on its first read.
    await page.waitForTimeout(150);
    expect(await list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  });
});
