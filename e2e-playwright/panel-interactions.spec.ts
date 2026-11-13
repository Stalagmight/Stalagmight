import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapIdle,
  waitForNoDialogs,
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
  {
    name: 'Side Tunnel',
    direction: 'out',
    shots: [
      { azimuth_in: 270, azimuth_out: 90, length: 5.0, depth_in: -3.0, depth_out: -4.0, left: 0, right: 0, up: 0, down: 0 },
    ],
  },
];

/**
 * Helper to create a project with sections for panel testing.
 * Returns when the project is loaded with sections containing shots.
 *
 * Uses store seeding to avoid dialog overlays during tests.
 */
async function createProjectWithSections(page: Page): Promise<void> {
  await createProject(page, 'Panel Test Project');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
}

/**
 * Panel interaction tests use the two-step import flow and auto-placement helpers.
 */
test.describe('Panel Interactions', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('should toggle sections panel with Ctrl+1', async ({ page }) => {
    // Create a project to access the SingleSystemView with panels
    await createProjectWithSections(page);

    // The sections panel should be visible by default (sectionsPanelOpen: true in uiStore)
    // Look for the "Sections" header or panel content
    const sectionsPanel = page.getByTestId('sections-panel');
    await expect(sectionsPanel).toBeVisible({ timeout: 3000 });

    // Toggle off with Ctrl+1
    await page.keyboard.press('Control+1');
    await waitForMapIdle(page);

    // Sections panel header should no longer be visible
    await expect(sectionsPanel).toBeHidden({ timeout: 2000 });

    // Toggle back on with Ctrl+1
    await page.keyboard.press('Control+1');
    await waitForMapIdle(page);

    // Sections panel should be visible again
    await expect(sectionsPanel).toBeVisible({ timeout: 2000 });

    // App should remain functional
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });

  test('should toggle detail panel with Ctrl+2', async ({ page }) => {
    // Create a project to access the SingleSystemView with panels
    await createProjectWithSections(page);

    // The detail panel should be visible by default (detailPanelOpen: true in uiStore)
    // Look for the "Details" tab button
    const detailPanel = page.getByTestId('detail-panel');
    await expect(detailPanel).toBeVisible({ timeout: 3000 });

    // Toggle off with Ctrl+2
    await page.keyboard.press('Control+2');
    await waitForMapIdle(page);

    // Detail panel tabs should no longer be visible
    await expect(detailPanel).toBeHidden({ timeout: 2000 });

    // Toggle back on with Ctrl+2
    await page.keyboard.press('Control+2');
    await waitForMapIdle(page);

    // Detail panel should be visible again
    await expect(detailPanel).toBeVisible({ timeout: 2000 });

    // App should remain functional
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });

  test('should toggle data table panel with Ctrl+3', async ({ page }) => {
    // Create a project to access the SingleSystemView with panels
    await createProjectWithSections(page);

    // Closed here because the IPC mock's config says `data_table_open: false`,
    // which is what applyPanelsConfig loads. A fresh install defaults to OPEN;
    // this spec is about the toggle, so it starts from the closed side.
    // Verify the data table is NOT visible initially
    const dataTable = page.getByTestId('data-table');
    await expect(dataTable).toBeHidden({ timeout: 2000 });

    // Toggle open with Ctrl+3
    await page.keyboard.press('Control+3');
    await waitForMapIdle(page);

    // Data table should now be visible
    // Look for the condensed header: section pill + view toggle
    await expect(dataTable).toBeVisible({ timeout: 3000 });

    // Header section pill should show the auto-selected first section
    const sectionPill = page.getByTestId('data-table-section-pill');
    await expect(sectionPill).toBeVisible({ timeout: 2000 });
    await expect(sectionPill).toContainText('Main Passage');

    // View toggle (shots/stations radio group) should be visible
    await expect(page.getByTestId('data-table-view-select')).toBeVisible({ timeout: 2000 });
    await expect(page.getByTestId('data-table-view-shots')).toHaveAttribute('aria-checked', 'true');

    // Grid rows should be visible
    const firstRow = dataTable.locator('[data-testid^="grid-row-"]').first();
    await expect(firstRow).toBeVisible({ timeout: 2000 });

    // Toggle closed with Ctrl+3
    await page.keyboard.press('Control+3');
    await waitForMapIdle(page);

    // Data table text should no longer be visible when closed
    await expect(dataTable).toBeHidden({ timeout: 2000 });

    // App should remain functional
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });

  test('should resize panels by dragging', async ({ page }) => {
    // Create a project to access the SingleSystemView with panels
    await createProjectWithSections(page);

    // Get the sections panel (left side)
    // The ResizablePanel has a resize handle on the right edge
    const sectionsPanel = page.getByTestId('sections-panel');

    // Get initial width
    const initialBox = await sectionsPanel.boundingBox();
    expect(initialBox).not.toBeNull();
    const initialWidth = initialBox!.width;

    // Find the resize handle (it's a div with cursor-ew-resize)
    // The handle is positioned at the right edge of the panel
    const resizeHandle = sectionsPanel.locator('[class*="cursor-ew-resize"]');

    if (await resizeHandle.isVisible()) {
      const handleBox = await resizeHandle.boundingBox();
      expect(handleBox).not.toBeNull();

      // Drag the handle to resize the panel
      const handleCenterX = handleBox!.x + handleBox!.width / 2;
      const handleCenterY = handleBox!.y + handleBox!.height / 2;

      // Drag to make panel wider (move right by 50px)
      await page.mouse.move(handleCenterX, handleCenterY);
      await page.mouse.down();
      await page.mouse.move(handleCenterX + 50, handleCenterY);
      await page.mouse.up();

      await waitForMapIdle(page);

      // Check that the panel width changed
      const newBox = await sectionsPanel.boundingBox();
      expect(newBox).not.toBeNull();

      // Width should have increased (accounting for min/max constraints)
      // The panel has minSize=150 and maxSize=400
      expect(newBox!.width).toBeGreaterThanOrEqual(Math.min(initialWidth, 150));
      expect(newBox!.width).toBeLessThanOrEqual(400);

      // Try dragging to make it smaller (back to original-ish)
      const newHandleBox = await resizeHandle.boundingBox();
      if (newHandleBox) {
        const newHandleCenterX = newHandleBox.x + newHandleBox.width / 2;
        const newHandleCenterY = newHandleBox.y + newHandleBox.height / 2;

        await page.mouse.move(newHandleCenterX, newHandleCenterY);
        await page.mouse.down();
        await page.mouse.move(newHandleCenterX - 100, newHandleCenterY);
        await page.mouse.up();

        await waitForMapIdle(page);

        // Verify panel respects min size constraint
        const finalBox = await sectionsPanel.boundingBox();
        expect(finalBox).not.toBeNull();
        expect(finalBox!.width).toBeGreaterThanOrEqual(150);
      }
    }

    // App should remain functional
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });

  test('should expand/collapse section in sections panel', async ({ page }) => {
    // Create a project with sections
    await createProjectWithSections(page);

    // The sections panel shows a list of sections
    // Each section item has a name and stats (e.g., "3 shots - 33.7m")
    const sectionsPanel = page.getByTestId('sections-panel');
    await expect(sectionsPanel).toBeVisible({ timeout: 3000 });

    // Find a section item to select/expand
    const sectionRow = page.locator('[data-testid^="section-row-"]').filter({ hasText: 'Main Passage' });
    await expect(sectionRow).toBeVisible({ timeout: 2000 });

    // Initially the section shows basic info (name and stats)
    const sectionStats = sectionRow.locator('text=/Shots:\\s*\\d+/');
    await expect(sectionStats).toBeVisible();

    // Click on the section to select it
    await sectionRow.click();
    await waitForMapIdle(page);

    // When a section is selected, aria-selected should reflect it
    await expect(sectionRow).toHaveAttribute('aria-selected', 'true');

    // The detail panel should now show section details
    const detailPanel = page.getByTestId('detail-panel');
    if (await detailPanel.isVisible()) {
      const sectionDetail = page.getByTestId('section-detail');
      await expect(sectionDetail).toBeVisible({ timeout: 2000 });
      await expect(sectionDetail.locator('text=Section stats')).toBeVisible({ timeout: 2000 });
    }

    // Click on a different section to verify selection changes
    const anotherSection = page.locator('[data-testid^="section-row-"]').filter({ hasText: 'Side Tunnel' });
    if (await anotherSection.isVisible()) {
      await anotherSection.click();
      await waitForMapIdle(page);

      // The new section should be selected
      await expect(anotherSection).toHaveAttribute('aria-selected', 'true');

      // The previous section should no longer be selected
      await expect(sectionRow).toHaveAttribute('aria-selected', 'false');
    }

    // App should remain functional
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });

  test('clicking a section color dot recolors the section without selecting it (#654)', async ({ page }) => {
    await createProjectWithSections(page);
    const sectionRow = page.locator('[data-testid^="section-row-"]').filter({ hasText: 'Main Passage' });
    const sectionId = Number((await sectionRow.getAttribute('data-testid'))!.replace('section-row-', ''));
    const dot = page.getByTestId(`section-color-${sectionId}`);
    const readColor = () =>
      page.evaluate((id) => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        return state.project.sections.find((s: any) => s.id === id).color as string;
      }, sectionId);
    // The picker opens at the section's current color.
    await expect(dot).toHaveValue((await readColor()).toLowerCase());

    let updateCalls = 0;
    page.on('console', (msg) => {
      if (msg.text().startsWith('[Mock IPC] update_section called')) updateCalls += 1;
    });

    // The dot is a control of its own: clicking it does not select the row.
    await dot.click();
    await expect(sectionRow).toHaveAttribute('aria-selected', 'false');

    // Steps through the open picker (`input`) commit nothing; only the
    // confirmed pick (`change`) does, so one pick is one command (one undo step).
    const pick = (type: 'input' | 'change', value: string) =>
      dot.evaluate((el, [t, v]) => {
        (el as HTMLInputElement).value = v!;
        el.dispatchEvent(new Event(t!, { bubbles: true }));
      }, [type, value]);
    await pick('input', '#00ff00');
    await pick('input', '#0000ff');
    await pick('change', '#ff0000');
    await expect.poll(readColor).toBe('#ff0000');
    expect(updateCalls).toBe(1);

    // A color changed elsewhere (undo, Edit Section) resyncs the dot.
    await page.evaluate(
      (id) => (window as any).__E2E_TEST__.projectStore.getState().updateSection(id, { color: '#336699' }),
      sectionId,
    );
    await expect(dot).toHaveValue('#336699');

    // The visible dot draws any CSS color the section carries (the free-text
    // field in Edit Section accepts one), not the input's #rrggbb-only value.
    await page.evaluate(
      (id) => (window as any).__E2E_TEST__.projectStore.getState().updateSection(id, { color: 'red' }),
      sectionId,
    );
    await expect
      .poll(() => dot.evaluate((el) => getComputedStyle(el.parentElement!).backgroundColor))
      .toBe('rgb(255, 0, 0)');
  });

  test('section color dot is inert while editing is blocked (#654)', async ({ page }) => {
    await createProjectWithSections(page);
    const sectionRow = page.locator('[data-testid^="section-row-"]').filter({ hasText: 'Main Passage' });
    const sectionId = Number((await sectionRow.getAttribute('data-testid'))!.replace('section-row-', ''));
    const dot = page.getByTestId(`section-color-${sectionId}`);
    await expect(dot).toBeEnabled();

    const before = await page.evaluate((id) => {
      const state = (window as any).__E2E_TEST__.projectStore.getState();
      return state.project.sections.find((s: any) => s.id === id).color as string;
    }, sectionId);

    // A remote SpeleoDB project opened without the write lock is read-only
    // (Plan/Export would also block, but they hide this panel).
    await page.evaluate(() =>
      (window as any).__E2E_TEST__.speleoStore.getState().setLinkOnOpen({ permission: 'READ_AND_WRITE' }),
    );
    await expect(dot).toBeDisabled();
    // The global `input:disabled { opacity: 0.5 }` must not surface the native
    // swatch over the drawn dot.
    await expect(dot).toHaveCSS('opacity', '0');

    // Clicking the dot says why nothing opens, and does not select the row.
    const readOnlyToast = page.locator('[data-testid="toast"][data-toast-key="edit-disabled"]');
    await page.getByTestId(`section-color-dot-${sectionId}`).click();
    await expect(readOnlyToast).toContainText('read-only');
    await expect(sectionRow).toHaveAttribute('aria-selected', 'false');
    await page.evaluate(() => (window as any).__E2E_TEST__.uiStore.setState({ toasts: [] }));

    // A pick confirmed after editing became blocked (the system picker was
    // already open) is refused at commit time, and the dot reverts.
    await dot.evaluate((el) => {
      (el as HTMLInputElement).value = '#00ff00';
      el.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await expect(dot).toHaveValue(before.toLowerCase());
    await expect(readOnlyToast).toContainText('read-only');
    expect(
      await page.evaluate((id) => {
        const state = (window as any).__E2E_TEST__.projectStore.getState();
        return state.project.sections.find((s: any) => s.id === id).color as string;
      }, sectionId),
    ).toBe(before);
  });

  test('section picker in the data-table sub-header switches sections', async ({ page }) => {
    await createProjectWithSections(page);
    await page.keyboard.press('Control+3');
    await waitForMapIdle(page);
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
    await expect(page.getByTestId('data-table-section-pill')).toContainText('Main Passage');

    await page.getByTestId('data-table-section-menu').click();
    const list = page.getByTestId('data-table-section-menu-list');
    await expect(list.getByRole('button')).toHaveText(['Main Passage', 'Side Tunnel']);
    await list.getByRole('button', { name: 'Side Tunnel' }).click();
    await expect(list).not.toBeVisible();

    await expect(page.getByTestId('data-table-section-pill')).toContainText('Side Tunnel');
    // The pick selects the section app-wide, like the old dropdown did.
    const selection = await page.evaluate(
      () => (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { selection: unknown } } } })
        .__E2E_TEST__.uiStore.getState().selection
    );
    expect(selection).toMatchObject({ type: 'section' });
  });

  test('should handle panel overflow with scrolling', async ({ page }) => {
    // Create a project with sections
    await createProjectWithSections(page);

    // The sections panel has overflow-auto set for scrolling
    const sectionsList = page.getByTestId('sections-panel').locator('.overflow-auto').first();

    // Check that the overflow container is present
    await expect(sectionsList).toBeVisible({ timeout: 3000 });

    // The mock data provides 6 sections:
    // - Main Passage, Side Tunnel, Mixed LRUD Section, Undefined LRUD Section, Empty Section, Validation Test Section
    // With normal viewport, all sections should be visible without scrolling
    // Let's check that all imported sections are in the list

    const mainPassage = page.locator('[data-testid^="section-item-"]').filter({ hasText: 'Main Passage' });
    const sideTunnel = page.locator('[data-testid^="section-item-"]').filter({ hasText: 'Side Tunnel' });

    await expect(mainPassage).toBeVisible({ timeout: 2000 });
    await expect(sideTunnel).toBeVisible({ timeout: 2000 });

    // Test the data table panel for scrolling behavior
    // First, expand the data table
    await page.keyboard.press('Control+3');
    await waitForMapIdle(page);

    // Drive the table to a section with shots via the app-wide selection
    // (the table follows external selection; its header no longer has a section dropdown)
    await page.evaluate(() => {
      const e2e = (window as any).__E2E_TEST__;
      const section = e2e.projectStore
        .getState()
        .project?.sections?.find((s: any) => s.name === 'Main Passage');
      if (!section) throw new Error('Main Passage section not found');
      e2e.uiStore.getState().setSelection({ type: 'section', sectionId: section.id });
    });
    // Header pill reflects the externally-selected section
    await expect(page.getByTestId('data-table-section-pill')).toContainText('Main Passage');
    await waitForMapIdle(page);

    // The data table should show shots with scrollable content if there are many
    const tableContainer = page.getByTestId('data-table');

    if (await tableContainer.isVisible()) {
      // Check that the grid has rows
      const gridRows = tableContainer.locator('[data-testid^="grid-row-"]');
      const rowCount = await gridRows.count();
      expect(rowCount).toBeGreaterThan(0);

      // The shots grid scrolls inside its own container (not the panel element)
      const gridScroller = tableContainer.locator('[data-testid="spreadsheet-grid"] > div[tabindex="0"]');
      const canScroll = await gridScroller.evaluate((el) => el.scrollHeight > el.clientHeight);
      if (canScroll) {
        // Get scroll position
        const scrollTop = await gridScroller.evaluate((el) => el.scrollTop);

        // Scroll down
        await gridScroller.evaluate((el) => el.scrollBy(0, 100));
        await waitForMapIdle(page);

        // Check that scroll position changed
        const newScrollTop = await gridScroller.evaluate((el) => el.scrollTop);
        expect(newScrollTop).toBeGreaterThan(scrollTop);
      }
    }

    // Test the detail panel scroll area
    const detailPanelContent = page.getByTestId('detail-panel').locator('.overflow-auto').first();

    if (await detailPanelContent.isVisible()) {
      // The detail panel should have an overflow container for long content
      const hasOverflow = await detailPanelContent.evaluate((el) => {
        const style = window.getComputedStyle(el);
        return style.overflowY === 'auto' || style.overflow === 'auto';
      });
      expect(hasOverflow).toBeTruthy();
    }

    // App should remain functional
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });

  // A Reference shot repeats passage already surveyed, so a section's length
  // leaves it out (#689): 10 m Normal + 3 m Jump = 13 m, not the 17 m of tape.
  // Both readouts compute client-side from the shots, so this drives the real
  // components, not the mock's stats.
  test('section length leaves out Reference shots (#689)', async ({ page }) => {
    await createProject(page, 'Reference Length Project');
    await seedProjectWithSections(page, [
      {
        name: 'Tie Run',
        direction: 'in',
        shots: [
          { azimuth_in: 0, azimuth_out: 180, length: 10, depth_in: -1, depth_out: -2 },
          { azimuth_in: 90, azimuth_out: 270, length: 4, depth_in: -2, depth_out: -2, kind: 'Reference' },
          { azimuth_in: 45, azimuth_out: 225, length: 3, depth_in: -2, depth_out: -3, kind: 'Jump' },
        ],
      },
      {
        name: 'Repeat Only',
        direction: 'in',
        shots: [
          { azimuth_in: 0, azimuth_out: 180, length: 10, depth_in: -1, depth_out: -2, kind: 'Reference' },
          { azimuth_in: 0, azimuth_out: 180, length: 10, depth_in: -2, depth_out: -3, kind: 'Reference' },
        ],
      },
    ]);
    await waitForNoDialogs(page);

    const sectionRow = page.locator('[data-testid^="section-row-"]').filter({ hasText: 'Tie Run' });
    await expect(sectionRow).toContainText(/Shots: 3 \| 13(\.0+)?m/);

    await sectionRow.click();
    const sectionDetail = page.getByTestId('section-detail');
    await expect(sectionDetail).toBeVisible();
    const statValue = (label: string) =>
      sectionDetail.getByText(label, { exact: true }).locator('xpath=following-sibling::div[1]');
    await expect(statValue('Total length')).toHaveText(/^13(\.0+)? m$/);
    // Averaged over the two shots that count toward the length.
    await expect(statValue('Average shot')).toHaveText(/^6\.50* m$/);
    // The card says how much Reference length it leaves out.
    await expect(sectionDetail.getByTestId('section-reference-length-note')).toHaveText(
      /^Reference shots \(4(\.0+)? m\) are not counted in the length\.$/
    );

    // A section of Reference shots only: 0 m long, and no shot to average, so
    // the average reads "-" rather than a 0 m shot.
    await page.locator('[data-testid^="section-row-"]').filter({ hasText: 'Repeat Only' }).click();
    await expect(sectionDetail).toContainText('Repeat Only');
    await expect(statValue('Total length')).toHaveText(/^0(\.0+)? m$/);
    await expect(statValue('Average shot')).toHaveText('-');
    await expect(sectionDetail.getByTestId('section-reference-length-note')).toHaveText(
      /^Reference shots \(20(\.0+)? m\) are not counted in the length\.$/
    );
  });
});
