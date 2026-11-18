import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
  getDrawnGeometry,
  type LineSeed,
} from './helpers/app';
import { AZIMUTH_AVG_KEY } from '../src/components/SingleSystemView/dataTableEdits';

/**
 * Sections: named groupings of lines in the Sections panel (#327).
 *
 * Mocked IPC: the five Section commands keep `cave_sections` /
 * `cave_section_id` on the mock project and each records an undo snapshot;
 * `save_display_setting` stores `hiddenSections`, which a load reads back.
 */

type Section = { id: number; name: string; color: string };
type E2EWindow = Window & {
  __E2E_TEST__: {
    uiStore: {
      getState: () => Record<string, (...args: never[]) => unknown> & {
        hiddenSections: number[];
        multiSelection: Array<{ type: string; lineId?: number }>;
      };
    };
    projectStore: {
      getState: () => {
        project: { lines: Array<{ id: number; name: string | null; color: string; cave_section_id?: number }>; cave_sections: Section[] } | null;
        createCaveSection: (name: string, color: string, lineIds?: number[]) => Promise<{ success: boolean; cave_section_id?: number }>;
        undo: () => Promise<unknown>;
      };
    };
  };
};

const line = (name: string, azimuth: number): LineSeed => ({
  name,
  direction: 'in',
  shots: [
    { azimuth_in: azimuth, azimuth_out: azimuth, length: 10, depth_in: 0, depth_out: -1 },
    { azimuth_in: azimuth, azimuth_out: azimuth, length: 12, depth_in: -1, depth_out: -2 },
  ],
});

const LINES = [line('Main', 90), line('Side', 0), line('Upper', 45), line('Lower', 135)];

async function lineIds(page: Page): Promise<Record<string, number>> {
  return page.evaluate(() => {
    const project = (window as unknown as E2EWindow).__E2E_TEST__.projectStore.getState().project!;
    return Object.fromEntries(project.lines.map((l) => [l.name ?? '', l.id]));
  });
}

async function sections(page: Page): Promise<Section[]> {
  return page.evaluate(
    () => (window as unknown as E2EWindow).__E2E_TEST__.projectStore.getState().project?.cave_sections ?? [],
  );
}

async function sectionOfLine(page: Page, id: number): Promise<number | null> {
  return page.evaluate(
    (lineId) =>
      (window as unknown as E2EWindow).__E2E_TEST__.projectStore
        .getState()
        .project!.lines.find((l) => l.id === lineId)?.cave_section_id ?? null,
    id,
  );
}

async function createSection(page: Page, name: string, members: number[]): Promise<number> {
  return page.evaluate(
    async ([n, ids]) => {
      const result = await (window as unknown as E2EWindow).__E2E_TEST__.projectStore
        .getState()
        .createCaveSection(n, '#22aa66', ids);
      return result.cave_section_id ?? -1;
    },
    [name, members] as const,
  );
}

async function undo(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as E2EWindow).__E2E_TEST__.projectStore.getState().undo());
}

async function setup(page: Page): Promise<Record<string, number>> {
  await createProject(page, 'Sections Test');
  await seedProjectWithLines(page, LINES);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  return lineIds(page);
}

/** Line ids drawn on the map (custom renderer, via the E2E geometry hook). */
async function drawnLineIds(page: Page): Promise<number[]> {
  const { shots } = await getDrawnGeometry(page);
  return [...new Set(shots.map((s) => s.lineId))].sort((a, b) => a - b);
}

test.describe('Sections (groups of lines)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('without Sections the panel is the flat list, plus New Section @smoke', async ({ page }) => {
    const ids = await setup(page);
    const tree = page.getByTestId('sections-tree');
    await expect(tree).toHaveAttribute('role', 'tree');
    await expect(page.getByTestId(`line-row-${ids.Main}`)).toHaveAttribute('aria-level', '1');
    await expect(page.getByTestId('sections-ungrouped-header')).toHaveCount(0);
    await expect(page.getByTestId('sections-new-section-button')).toBeEnabled();
  });

  test('New Section creates "Section 1", renames inline, and undo takes both back @smoke', async ({ page }) => {
    const ids = await setup(page);
    await page.getByTestId('sections-new-section-button').click();

    const input = page.locator('[data-testid^="section-rename-input-"]');
    await expect(input).toBeFocused();
    await expect(input).toHaveValue('Section 1');
    const [created] = await sections(page);
    expect(created?.name).toBe('Section 1');
    const header = page.getByTestId(`section-header-${created!.id}`);
    await expect(header).toHaveAttribute('aria-level', '1');
    await expect(header).toHaveAttribute('aria-expanded', 'true');
    // Every line is ungrouped, under the Ungrouped header, one level down.
    await expect(page.getByTestId('sections-ungrouped-header')).toBeVisible();
    await expect(page.getByTestId(`line-row-${ids.Main}`)).toHaveAttribute('aria-level', '2');

    await input.fill('Upper level');
    await input.press('Enter');
    await expect(page.getByTestId(`section-name-${created!.id}`)).toHaveText('Upper level');
    await expect(header).toBeFocused();
    await expect.poll(async () => (await sections(page))[0]?.name).toBe('Upper level');

    await undo(page);
    await expect(page.getByTestId(`section-name-${created!.id}`)).toHaveText('Section 1');
    await undo(page);
    await expect(header).toHaveCount(0);
    await expect(page.getByTestId('sections-ungrouped-header')).toHaveCount(0);
  });

  test('a taken or empty name is refused inline; Escape keeps the old name', async ({ page }) => {
    await setup(page);
    await createSection(page, 'Upper level', []);
    const lower = await createSection(page, 'Lower level', []);
    await page.getByTestId(`section-header-${lower}`).click();
    await page.keyboard.press('F2');
    const input = page.getByTestId(`section-rename-input-${lower}`);
    await expect(input).toBeFocused();

    await input.fill('Upper level');
    await input.press('Enter');
    await expect(page.getByTestId(`section-rename-error-${lower}`)).toHaveText(
      'There is already a section named "Upper level".',
    );
    await expect(input).toHaveAttribute('aria-invalid', 'true');
    await input.fill('   ');
    await input.press('Enter');
    await expect(page.getByTestId(`section-rename-error-${lower}`)).toHaveText('A section needs a name.');

    await input.press('Escape');
    await expect(page.getByTestId(`section-name-${lower}`)).toHaveText('Lower level');
    expect((await sections(page)).map((s) => s.name)).toEqual(['Upper level', 'Lower level']);
  });

  test('Move to Section moves a line, or the whole line selection, from the context menu', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', []);

    await page.getByTestId(`line-row-${ids.Main}`).click({ button: 'right' });
    await page.getByRole('button', { name: 'Move to Section' }).hover();
    await page.getByTestId('context-menu-submenu').getByRole('button', { name: 'Upper level' }).click();
    await expect.poll(() => sectionOfLine(page, ids.Main!)).toBe(upper);
    // The line is listed under its Section now.
    const header = page.getByTestId(`section-header-${upper}`);
    await expect(page.getByTestId(`section-stats-${upper}`)).toContainText('1 line');
    await expect(header).toHaveAttribute('aria-setsize', '2');

    // A selection of two lines moves together.
    await page.getByTestId(`line-row-${ids.Side}`).click();
    await page.getByTestId(`line-row-${ids.Upper}`).click({ modifiers: ['Shift'] });
    await page.getByTestId(`line-row-${ids.Upper}`).click({ button: 'right' });
    await page.getByRole('button', { name: 'Move 2 Lines to Section' }).hover();
    const submenu = page.getByTestId('context-menu-submenu');
    await submenu.getByRole('button', { name: 'Upper level' }).click();
    await expect.poll(() => sectionOfLine(page, ids.Side!)).toBe(upper);
    await expect.poll(() => sectionOfLine(page, ids.Upper!)).toBe(upper);
    await expect(page.getByTestId(`section-stats-${upper}`)).toContainText('3 lines');

    // "No Section" takes one back out; the Section it is in is offered disabled.
    await page.getByTestId(`line-row-${ids.Main}`).click({ button: 'right' });
    await page.getByRole('button', { name: 'Move to Section' }).hover();
    await expect(submenu.getByRole('button', { name: 'Upper level' })).toBeDisabled();
    await submenu.getByRole('button', { name: 'No Section' }).click();
    await expect.poll(() => sectionOfLine(page, ids.Main!)).toBeNull();
  });

  test('New Section... in the line menu groups the line and opens the rename', async ({ page }) => {
    const ids = await setup(page);
    await page.getByTestId(`line-row-${ids.Lower}`).click({ button: 'right' });
    await page.getByRole('button', { name: 'Move to Section' }).hover();
    await page.getByTestId('context-menu-submenu').getByRole('button', { name: 'New Section...' }).click();

    const input = page.locator('[data-testid^="section-rename-input-"]');
    await expect(input).toBeFocused();
    await input.fill('Sump');
    await input.press('Enter');
    const [sump] = await sections(page);
    await expect.poll(async () => (await sections(page))[0]?.name).toBe('Sump');
    expect(await sectionOfLine(page, ids.Lower!)).toBe(sump!.id);
  });

  test('dragging a line row onto a Section header moves it; onto Ungrouped takes it out', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', []);
    const header = page.getByTestId(`section-header-${upper}`);
    const row = page.getByTestId(`line-row-${ids.Side}`);

    const rowBox = (await row.boundingBox())!;
    const headerBox = (await header.boundingBox())!;
    await page.mouse.move(rowBox.x + 100, rowBox.y + rowBox.height / 2);
    await page.mouse.down();
    await page.mouse.move(rowBox.x + 100, rowBox.y - 10, { steps: 4 });
    await page.mouse.move(headerBox.x + 60, headerBox.y + headerBox.height / 2, { steps: 6 });
    // A clear target: the header lights up and the ghost names it.
    await expect(header).toHaveAttribute('data-drop-target', 'true');
    await expect(page.getByTestId('sections-drag-ghost')).toHaveText('Move to Upper level');
    await page.mouse.up();
    await expect.poll(() => sectionOfLine(page, ids.Side!)).toBe(upper);
    await expect(page.getByTestId('sections-drag-ghost')).toHaveCount(0);

    // Drag it back onto the Ungrouped header.
    const moved = (await page.getByTestId(`line-row-${ids.Side}`).boundingBox())!;
    const ungrouped = (await page.getByTestId('sections-ungrouped-header').boundingBox())!;
    await page.mouse.move(moved.x + 100, moved.y + moved.height / 2);
    await page.mouse.down();
    await page.mouse.move(ungrouped.x + 60, ungrouped.y + ungrouped.height / 2, { steps: 8 });
    await expect(page.getByTestId('sections-ungrouped-header')).toHaveAttribute('data-drop-target', 'true');
    await page.mouse.up();
    await expect.poll(() => sectionOfLine(page, ids.Side!)).toBeNull();
  });

  test('dragging a row of a multi-line selection carries the whole selection', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', []);
    await page.getByTestId(`line-row-${ids.Main}`).click();
    await page.getByTestId(`line-row-${ids.Lower}`).click({ modifiers: ['Control'] });

    const row = (await page.getByTestId(`line-row-${ids.Lower}`).boundingBox())!;
    const header = (await page.getByTestId(`section-header-${upper}`).boundingBox())!;
    await page.mouse.move(row.x + 100, row.y + row.height / 2);
    await page.mouse.down();
    await page.mouse.move(header.x + 60, header.y + header.height / 2, { steps: 10 });
    await expect(page.getByTestId('sections-drag-ghost')).toHaveText('Move 2 lines to Upper level');
    // Escape cancels: nothing moves.
    await page.keyboard.press('Escape');
    await page.mouse.up();
    expect(await sectionOfLine(page, ids.Main!)).toBeNull();
    // The press did not narrow the selection.
    await expect(page.getByTestId(`line-row-${ids.Main}`)).toHaveAttribute('aria-selected', 'true');

    await page.mouse.move(row.x + 100, row.y + row.height / 2);
    await page.mouse.down();
    await page.mouse.move(header.x + 60, header.y + header.height / 2, { steps: 10 });
    await page.mouse.up();
    await expect.poll(() => sectionOfLine(page, ids.Main!)).toBe(upper);
    await expect.poll(() => sectionOfLine(page, ids.Lower!)).toBe(upper);
  });

  test('the eye hides a Section on the map and saves it; a selected hidden line still shows', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', [ids.Upper!, ids.Lower!]);
    await expect.poll(() => drawnLineIds(page)).toEqual(Object.values(ids).sort((a, b) => a - b));

    await page.getByTestId(`section-toggle-visibility-${upper}`).click();
    await expect.poll(() => drawnLineIds(page)).toEqual([ids.Main!, ids.Side!].sort((a, b) => a - b));
    await expect(page.getByTestId(`section-header-${upper}`)).toHaveAttribute('data-hidden-on-map', 'true');
    await expect(page.getByTestId(`line-row-${ids.Upper}`)).toHaveAttribute('data-hidden-on-map', 'true');
    await expect(page.getByTestId(`section-toggle-visibility-${upper}`)).toHaveAttribute('aria-pressed', 'true');
    // Saved as the per-project display setting the backend reads back on load.
    const stored = await page.evaluate(async () => {
      const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string) => Promise<Record<string, string>> } })
        .__TAURI_INTERNALS__.invoke;
      return (await invoke('load_display_settings')).hiddenSections;
    });
    expect(JSON.parse(stored ?? '[]')).toEqual([upper]);

    // Selecting a hidden line in the panel shows just that line until the
    // selection moves on.
    await page.getByTestId(`line-row-${ids.Upper}`).click();
    await expect.poll(() => drawnLineIds(page)).toContain(ids.Upper!);
    expect(await drawnLineIds(page)).not.toContain(ids.Lower!);
    await page.getByTestId(`line-row-${ids.Main}`).click();
    await expect.poll(() => drawnLineIds(page)).not.toContain(ids.Upper!);

    await page.getByTestId(`section-toggle-visibility-${upper}`).click();
    await expect.poll(() => drawnLineIds(page)).toEqual(Object.values(ids).sort((a, b) => a - b));
  });

  test('H and the header menu hide / show a Section; a hidden one has nothing to zoom to', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', [ids.Upper!, ids.Lower!]);
    const header = page.getByTestId(`section-header-${upper}`);
    await header.click();
    await expect(header).toBeFocused();
    await page.keyboard.press('h');
    await expect(header).toHaveAttribute('data-hidden-on-map', 'true');
    await expect.poll(() => drawnLineIds(page)).toEqual([ids.Main!, ids.Side!].sort((a, b) => a - b));
    await expect(page.locator('#selection-announcer')).toHaveText('Upper level is hidden on the map');

    // Hidden: the menu's Zoom to Section is off and says why; Show on Map
    // brings the Section back.
    await header.click({ button: 'right' });
    await expect(page.getByRole('button', { name: 'Zoom to Section' })).toBeDisabled();
    await expect(page.getByTestId('context-menu')).toContainText('Show the section on the map first');
    await page.getByRole('button', { name: 'Show on Map' }).click();
    await expect(header).not.toHaveAttribute('data-hidden-on-map', 'true');
    await expect.poll(() => drawnLineIds(page)).toEqual(Object.values(ids).sort((a, b) => a - b));
    await expect(page.locator('#selection-announcer')).toHaveText('Upper level is shown on the map');

    // And the menu hides it again.
    await header.click({ button: 'right' });
    await page.getByRole('button', { name: 'Hide on Map' }).click();
    await expect(header).toHaveAttribute('data-hidden-on-map', 'true');
  });

  test('keyboard: arrows walk the tree, Left/Right collapse and expand, Space toggles', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', [ids.Upper!]);
    const header = page.getByTestId(`section-header-${upper}`);
    const upperRow = page.getByTestId(`line-row-${ids.Upper}`);

    await header.click();
    await expect(header).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(upperRow).toBeFocused();
    await page.keyboard.press('ArrowLeft');
    await expect(header).toBeFocused();
    await page.keyboard.press('ArrowLeft');
    await expect(header).toHaveAttribute('aria-expanded', 'false');
    await expect(upperRow).toHaveCount(0);
    await expect(page.getByTestId(`section-stats-${upper}`)).toContainText('1 line');
    await page.keyboard.press('ArrowRight');
    await expect(header).toHaveAttribute('aria-expanded', 'true');
    await page.keyboard.press('ArrowRight');
    await expect(upperRow).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(page.getByTestId('sections-ungrouped-header')).toBeFocused();
    await page.keyboard.press(' ');
    await expect(page.getByTestId('sections-ungrouped-header')).toHaveAttribute('aria-expanded', 'false');
    await expect(page.getByTestId(`line-row-${ids.Main}`)).toHaveCount(0);
    await page.keyboard.press('Home');
    await expect(header).toBeFocused();

    // Selecting a line of a collapsed group (from the map, the table...) opens it.
    await page.evaluate(
      (id) => (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState().setSelection({ type: 'line', lineId: id } as never),
      ids.Main!,
    );
    await expect(page.getByTestId('sections-ungrouped-header')).toHaveAttribute('aria-expanded', 'true');
    await expect(page.getByTestId(`line-row-${ids.Main}`)).toHaveAttribute('aria-selected', 'true');
  });

  test('header menu: colour, Apply Color to Lines, Delete Section keeps lines, undo restores', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', [ids.Upper!, ids.Lower!]);

    // The dot commits one pick through update_cave_section.
    const dot = page.getByTestId(`section-color-${upper}`);
    await dot.evaluate((el) => {
      (el as HTMLInputElement).value = '#aa3311';
      el.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await expect.poll(async () => (await sections(page))[0]?.color).toBe('#aa3311');

    await page.getByTestId(`section-header-${upper}`).click({ button: 'right' });
    await page.getByRole('button', { name: 'Apply Color to Lines' }).click();
    await expect
      .poll(() =>
        page.evaluate(
          ([a, b]) =>
            (window as unknown as E2EWindow).__E2E_TEST__.projectStore
              .getState()
              .project!.lines.filter((l) => l.id === a || l.id === b)
              .map((l) => l.color),
          [ids.Upper!, ids.Lower!] as const,
        ),
      )
      .toEqual(['#aa3311', '#aa3311']);

    await page.getByTestId(`section-header-${upper}`).click({ button: 'right' });
    await page.getByRole('button', { name: 'Delete Section' }).click();
    await expect(page.getByTestId(`section-header-${upper}`)).toHaveCount(0);
    // The lines stay; the panel is flat again.
    await expect(page.getByTestId(`line-row-${ids.Upper}`)).toHaveAttribute('aria-level', '1');
    expect(await sectionOfLine(page, ids.Upper!)).toBeNull();

    await undo(page);
    await expect(page.getByTestId(`section-header-${upper}`)).toBeVisible();
    await expect.poll(() => sectionOfLine(page, ids.Lower!)).toBe(upper);
  });

  test('read-only: the closure view disables New Section, rename and the menus', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', [ids.Upper!]);
    // The closure-adjusted view only sticks with an eligible loop closure, so
    // seed one (as panel-interactions.spec.ts does); without it MapCanvas
    // reverts to measured.
    await page.evaluate((lineId) => {
      const store = (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => { project: Record<string, unknown> & { loop_closures: unknown[] } }; setState: (s: unknown) => void } } }).__E2E_TEST__.projectStore;
      const project = store.getState().project;
      store.setState({
        project: {
          ...project,
          loop_closures: [
            ...project.loop_closures,
            {
              id: 1,
              station_a: { line_id: lineId, station_index: 0 },
              station_b: { line_id: lineId, station_index: 2 },
              path_length: 22,
              misclosure_distance: 0.3,
              error_percentage: 1.2,
              horizontal_misclosure: 0.25,
              vertical_misclosure: 0.15,
              strategy: 'Batch',
              is_aligned: false,
            },
          ],
        },
      });
    }, ids.Main!);
    await page.evaluate(() =>
      (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState().setGeometryMode('closureAdjusted' as never, 'e2e' as never),
    );
    await expect
      .poll(() => page.evaluate(() => (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState().geometryMode))
      .toBe('closureAdjusted');
    await expect(page.getByTestId('sections-new-section-button')).toBeDisabled();
    await page.getByTestId(`section-header-${upper}`).click({ button: 'right' });
    await expect(page.getByRole('button', { name: 'Delete Section' })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Zoom to Section' })).toBeEnabled();
    // Hide / Show on Map is a view setting too.
    await expect(page.getByRole('button', { name: 'Hide on Map' })).toBeEnabled();
    await page.keyboard.press('Escape');
    await page.getByTestId(`section-header-${upper}`).click();
    await page.keyboard.press('F2');
    await expect(page.getByTestId(`section-rename-input-${upper}`)).toHaveCount(0);
    // Hiding is a view setting, not an edit: still available.
    await page.getByTestId(`section-toggle-visibility-${upper}`).click();
    await expect(page.getByTestId(`section-header-${upper}`)).toHaveAttribute('data-hidden-on-map', 'true');
  });

  test('the New Line dialog puts the new line straight into a Section', async ({ page }) => {
    const ids = await setup(page);
    const upper = await createSection(page, 'Upper level', [ids.Upper!]);
    await page.evaluate(() => (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState().openDialog('import' as never));
    const dialog = page.getByTestId('import-line-dialog');
    await expect(dialog).toBeVisible();
    await page.getByTestId('import-line-section-select').click();
    await page.getByRole('option', { name: 'Upper level' }).click();
    await page.getByTestId('line-name-input').fill('Fresh');
    for (const [key, value] of [['length', '5'], [AZIMUTH_AVG_KEY, '90'], ['depth_in', '0'], ['depth_out', '-1']]) {
      await page.getByTestId(`new-line-cell-0-${key}`).dblclick();
      await page.getByTestId('new-line-cell-editor').fill(value!);
      await page.keyboard.press('Enter');
    }
    await page.getByTestId('import-line-create').click();
    await expect(dialog).toBeHidden();
    await page.evaluate(async () => {
      const ui = (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState();
      const placement = ui.confirmPlacement() as { onComplete?: (r: unknown) => Promise<void> } | null;
      await placement?.onComplete?.({ position: { x: 0, y: 0, z: 0 }, snappedToStation: null });
    });
    await expect.poll(async () => (await lineIds(page)).Fresh).toBeDefined();
    const fresh = (await lineIds(page)).Fresh!;
    await expect.poll(() => sectionOfLine(page, fresh)).toBe(upper);
    // The line detail names its Section.
    await page.getByTestId(`line-row-${fresh}`).click();
    await expect(page.getByTestId('line-detail-section')).toContainText('Upper level');
  });
});
