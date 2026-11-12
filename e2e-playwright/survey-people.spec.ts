import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';
import { AZIMUTH_AVG_KEY } from '../src/components/SingleSystemView/dataTableEdits';

/**
 * Surveyors and explorers as people pills, on every shot (#328, #329).
 *
 * Mocked IPC: `edit_shot_credit` applies `CreditEdit::apply` per targeted shot
 * and is undoable, `create_section` keeps each shot's credit, and the linked
 * project's collaborators come from `__MOCK_SPELEODB_COLLABORATORS__`.
 */

type E2EWindow = Window & {
  __E2E_TEST__: {
    uiStore: { getState: () => Record<string, (...args: unknown[]) => unknown> & { placementMode: unknown } };
    projectStore: { getState: () => { project: { sections: Array<{ id: number; shots: Array<{ id: number; credit?: unknown }> }> } | null } };
    speleoStore: { setState: (s: unknown) => void };
  };
  __MOCK_EDIT_SHOT_CREDIT_CALLS__?: unknown[];
  __MOCK_SPELEODB_COLLABORATORS__?: unknown[];
  __speleoLinkStatusOverrides?: Record<string, unknown>;
};

const threeShots: SectionSeed = {
  name: 'People Line',
  direction: 'in',
  shots: [
    { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
    { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
    { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
  ],
};

async function openDialog(page: Page, dialog: string, data?: Record<string, unknown>): Promise<void> {
  await page.evaluate(
    ([name, payload]) => (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState().openDialog(name, payload),
    [dialog, data] as const,
  );
}

async function firstSection(page: Page): Promise<{ id: number; shots: Array<{ id: number; credit?: unknown }> }> {
  return page.evaluate(() => (window as unknown as E2EWindow).__E2E_TEST__.projectStore.getState().project!.sections[0]!);
}

async function creditCalls(page: Page): Promise<unknown[]> {
  return page.evaluate(() => (window as unknown as E2EWindow).__MOCK_EDIT_SHOT_CREDIT_CALLS__ ?? []);
}

async function fillCell(page: Page, row: number, colKey: string, value: string): Promise<void> {
  await page.getByTestId(`new-section-cell-${row}-${colKey}`).dblclick();
  await page.getByTestId('new-section-cell-editor').fill(value);
  await page.keyboard.press('Enter');
}

async function fillShotRow(page: Page, row: number, length: string): Promise<void> {
  await fillCell(page, row, 'length', length);
  await fillCell(page, row, AZIMUTH_AVG_KEY, '90');
  await fillCell(page, row, 'depth_in', '0');
  await fillCell(page, row, 'depth_out', '-1');
}

/** Pill labels of one people field, in order. */
async function pills(page: Page, prefix: string): Promise<string[]> {
  return page.getByTestId(new RegExp(`^${prefix}-pill-\\d+-label$`)).allTextContents();
}

async function seedSection(page: Page): Promise<number> {
  await seedProjectWithSections(page, [threeShots]);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  return (await firstSection(page)).id;
}

test.describe('Survey people', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Survey People');
  });

  test('New Section takes pills (Enter and ";"), Edit Section shows them, x removes, undo restores', async ({ page }) => {
    await waitForMapReady(page);
    await openDialog(page, 'import');
    const dialog = page.getByTestId('import-section-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('new-section-cell-0-length')).toBeVisible();
    await fillShotRow(page, 0, '10');

    const surveyors = page.getByTestId('import-section-surveyors-input');
    await surveyors.fill('Ann Diver');
    await surveyors.press('Enter');
    await surveyors.pressSequentially('Bo Lead;');
    const explorers = page.getByTestId('import-section-explorers-input');
    await explorers.fill('Cy Cave');
    await explorers.press('Enter');
    expect(await pills(page, 'import-section-surveyors')).toEqual(['Ann Diver', 'Bo Lead']);
    expect(await pills(page, 'import-section-explorers')).toEqual(['Cy Cave']);

    await page.getByTestId('import-section-create').click();
    await expect(dialog).toBeHidden();
    // Place the section where the cursor is (MapCanvas does this on a click).
    await page.evaluate(async () => {
      const ui = (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState();
      const placement = ui.confirmPlacement() as { onComplete?: (r: unknown) => Promise<void> } | null;
      await placement?.onComplete?.({ position: { x: 0, y: 0, z: 0 }, snappedToStation: null });
    });
    await expect.poll(async () => (await firstSection(page).catch(() => null))?.shots[0]?.credit ?? null).toEqual({
      surveyors: [{ name: 'Ann Diver' }, { name: 'Bo Lead' }],
      explorers: [{ name: 'Cy Cave' }],
      date: null,
    });
    const sectionId = (await firstSection(page)).id;

    await openDialog(page, 'edit-section', { sectionId });
    const edit = page.getByTestId('edit-section-dialog');
    await expect(edit).toBeVisible();
    expect(await pills(page, 'section-surveyors')).toEqual(['Ann Diver', 'Bo Lead']);
    expect(await pills(page, 'section-explorers')).toEqual(['Cy Cave']);

    await page.getByTestId('section-surveyors-pill-1-remove').click();
    expect(await pills(page, 'section-surveyors')).toEqual(['Ann Diver']);
    // Nothing is written until Save, and then exactly once.
    expect(await creditCalls(page)).toHaveLength(0);
    await page.getByTestId('edit-section-save').click();
    await expect(edit).toBeHidden();
    expect(await creditCalls(page)).toEqual([
      { targets: { type: 'section', section_id: sectionId }, edit: { remove_surveyors: [{ name: 'Bo Lead' }] } },
    ]);

    await openDialog(page, 'edit-section', { sectionId });
    expect(await pills(page, 'section-surveyors')).toEqual(['Ann Diver']);
    await page.getByTestId('edit-section-cancel').click();
    await expect(edit).toBeHidden();

    await page.getByTestId('toolbar-undo-button').click();
    await expect
      .poll(async () => ((await firstSection(page)).shots[0]?.credit as { surveyors: unknown[] }).surveyors)
      .toEqual([{ name: 'Ann Diver' }, { name: 'Bo Lead' }]);
    await openDialog(page, 'edit-section', { sectionId });
    expect(await pills(page, 'section-surveyors')).toEqual(['Ann Diver', 'Bo Lead']);
  });

  test('a person added to 2 of 3 shots in the Detail panel is a partial pill in Edit Section', async ({ page }) => {
    const sectionId = await seedSection(page);
    const shots = (await firstSection(page)).shots.map((s) => s.id);
    await page.evaluate(
      ([sid, a, b]) => {
        const ui = (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState();
        ui.setSelection({ type: 'shot', sectionId: sid, shotId: a });
        ui.addToSelection({ type: 'shot', sectionId: sid, shotId: b });
      },
      [sectionId, shots[0]!, shots[1]!] as const,
    );
    await expect(page.getByTestId('multi-shot-detail')).toBeVisible();
    const input = page.getByTestId('detail-credit-surveyors-input');
    await input.fill('Dee Deep');
    await input.press('Enter');
    await expect.poll(() => creditCalls(page)).toEqual([
      { targets: { type: 'shots', shots: [[sectionId, shots[0]], [sectionId, shots[1]]] }, edit: { add_surveyors: [{ name: 'Dee Deep' }] } },
    ]);

    await openDialog(page, 'edit-section', { sectionId });
    const dee = page.getByTestId('section-surveyors-pill-1');
    await expect(page.getByTestId('section-surveyors-pill-1-label')).toHaveText('Dee Deep');
    await expect(dee).toHaveAttribute('data-partial', 'true');
    await expect(page.getByTestId('section-surveyors-pill-1-count')).toHaveText('2/3');
    await page.getByTestId('section-surveyors-pill-1-label').hover();
    await expect(page.getByTestId('section-surveyors-pill-1-tooltip')).toContainText('On 2 of 3 shots');
  });

  test('SpeleoDB collaborators are suggested, show their email on hover, and same names are told apart', async ({ page }) => {
    const sectionId = await seedSection(page);
    await page.evaluate(() => {
      const w = window as unknown as E2EWindow;
      w.__MOCK_SPELEODB_COLLABORATORS__ = [
        { name: 'Ann Diver', email: 'ann@caves.example', level: 'READ_AND_WRITE' },
        { name: 'John Smith', email: 'jsmith@caves.example', level: 'READ_AND_WRITE' },
        { name: 'John Smith', email: 'john.smith@divers.example', level: 'READ_ONLY' },
      ];
      w.__speleoLinkStatusOverrides = {
        is_remote: true, edit_enabled: true, has_unpushed_edits: false,
        permission: 'READ_AND_WRITE', lock_holder: 'me@caves.example', remote_state: 'up_to_date',
      };
      w.__E2E_TEST__.speleoStore.setState({
        session: { loggedIn: true, userEmail: 'me@caves.example', endpoint: 'https://speleodb.internal' },
        remoteProjectId: 'people-project',
        projectLinkKind: 'remote',
        link: {
          permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: 'me@caves.example', lockedByOther: false,
          hasUnpushedEdits: false, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
          noAccess: false, serverError: null,
        },
      });
    });

    await openDialog(page, 'edit-section', { sectionId });
    await expect(page.getByTestId('edit-section-dialog')).toBeVisible();
    const input = page.getByTestId('section-surveyors-input');
    await input.fill('ann');
    const first = page.getByTestId('section-surveyors-suggestion-0');
    await expect(first).toContainText('Ann Diver');
    await expect(first).toContainText('ann@caves.example');
    await expect(page.getByTestId('section-surveyors-suggestion-0-badge')).toHaveText('SpeleoDB');
    await input.press('Enter');
    const ann = page.getByTestId('section-surveyors-pill-1-label');
    await expect(ann).toHaveText('Ann Diver');
    await ann.hover();
    const tooltip = page.getByTestId('section-surveyors-pill-1-tooltip');
    await expect(tooltip).toContainText('ann@caves.example');
    await expect(tooltip).toContainText('SpeleoDB collaborator');

    // Two collaborators share a name: Enter asks, the pick keeps its identity.
    await input.fill('John Smith');
    await input.press('Enter');
    await expect(page.getByTestId('section-surveyors-status')).toContainText('Several people are called John Smith');
    await expect(page.getByTestId('section-surveyors-suggestions')).toContainText('john.smith@divers.example');
    await page.getByTestId('section-surveyors-suggestions').getByText('john.smith@divers.example').click();
    await expect(page.getByTestId('section-surveyors-pill-2-label')).toHaveText('John Smith (john.smith)');
    await input.fill('John Smith');
    await page.getByTestId('section-surveyors-suggestions').getByText('jsmith@caves.example').click();
    await expect(page.getByTestId('section-surveyors-pill-3-label')).toHaveText('John Smith (jsmith)');
  });

  test('the typing input shares the last pill row when there is room for it', async ({ page }) => {
    // The 3-shot multi-selection whose Explorers pills ("Cy Cave 2/3",
    // "Dee Deep 1/3") left the input alone on an empty second row.
    const sectionId = await seedSection(page);
    const shots = (await firstSection(page)).shots.map((s) => s.id);
    await page.evaluate(
      async ([sid, ids]) => {
        const ps = (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => Record<string, (...a: unknown[]) => Promise<unknown>> } } })
          .__E2E_TEST__.projectStore.getState();
        await ps.editShotCredit({ type: 'shots', shots: [[sid, ids[0]], [sid, ids[1]]] }, { set_explorers: [{ name: 'Cy Cave' }] });
        await ps.editShotCredit({ type: 'shots', shots: [[sid, ids[1]]] }, { add_explorers: [{ name: 'Dee Deep' }] });
        const ui = (window as unknown as E2EWindow).__E2E_TEST__.uiStore.getState();
        ui.setSelection({ type: 'shot', sectionId: sid, shotId: ids[0] });
        ui.addToSelection({ type: 'shot', sectionId: sid, shotId: ids[1] });
        ui.addToSelection({ type: 'shot', sectionId: sid, shotId: ids[2] });
      },
      [sectionId, shots] as const,
    );
    await expect(page.getByTestId('detail-credit-explorers-pill-1-count')).toHaveText('1/3');
    const pill = await page.getByTestId('detail-credit-explorers-pill-1').boundingBox();
    const input = await page.getByTestId('detail-credit-explorers-input').boundingBox();
    expect(pill && input).toBeTruthy();
    expect(Math.abs(input!.y + input!.height / 2 - (pill!.y + pill!.height / 2))).toBeLessThan(4);
    expect(input!.x).toBeGreaterThan(pill!.x + pill!.width);
  });

  test('Escape closes the suggestion list, then clears the text, without closing the dialog', async ({ page }) => {
    const sectionId = await seedSection(page);
    await openDialog(page, 'edit-section', { sectionId });
    const dialog = page.getByTestId('edit-section-dialog');
    const input = page.getByTestId('section-explorers-input');
    await input.fill('Te');
    await expect(page.getByTestId('section-explorers-suggestions')).toBeVisible();
    await input.press('Escape');
    await expect(page.getByTestId('section-explorers-suggestions')).toBeHidden();
    await expect(dialog).toBeVisible();
    await expect(input).toHaveValue('Te');
    await input.press('Escape');
    await expect(input).toHaveValue('');
    await expect(dialog).toBeVisible();
  });

  test('data table credit columns are hidden by default, shown from the columns menu, and edited in a popover', async ({ page }) => {
    const sectionId = await seedSection(page);
    const shotId = (await firstSection(page)).shots[0]!.id;
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('grid-header-length')).toBeVisible();
    await expect(page.getByTestId('grid-header-surveyors')).toHaveCount(0);
    await expect(page.getByTestId('grid-header-survey_date')).toHaveCount(0);

    await page.getByTestId('data-table-columns-menu').click();
    const surveyorsToggle = page.getByTestId('col-vis-surveyors');
    await expect(surveyorsToggle).not.toBeChecked();
    await surveyorsToggle.click();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('grid-header-surveyors')).toBeVisible();
    const cell = page.getByTestId(`shot-cell-${sectionId}-${shotId}-surveyors`);
    await expect(cell).toContainText('Test Surveyor');

    await cell.dblclick();
    const editor = page.getByTestId('credit-cell-editor');
    await expect(editor).toBeVisible();
    const input = page.getByTestId('credit-cell-editor-people-input');
    await input.fill('Eve Echo');
    await input.press('Enter');
    await input.press('Enter');
    await expect(editor).toBeHidden();
    await expect.poll(() => creditCalls(page)).toEqual([
      {
        targets: { type: 'shots', shots: [[sectionId, shotId]] },
        edit: { set_surveyors: [{ name: 'Test Surveyor' }, { name: 'Eve Echo' }] },
      },
    ]);
    await expect(cell).toContainText('Eve Echo');
  });

  test('a credit cell hands the keyboard to its popover: double click, typing and Escape', async ({ page }) => {
    // The popover's field must own the keyboard. When it did not, the grid
    // took the keys: a letter became a one-letter explorer written straight
    // to the shot, and Escape left the popover open over the table.
    const sectionId = await seedSection(page);
    const shotId = (await firstSection(page)).shots[0]!.id;
    await page.keyboard.press('Control+3');
    await page.getByTestId('data-table-columns-menu').click();
    await page.getByTestId('col-vis-explorers').click();
    await page.keyboard.press('Escape');
    const cell = page.getByTestId(`shot-cell-${sectionId}-${shotId}-explorers`);
    const scroller = page.getByRole('grid');
    const editor = page.getByTestId('credit-cell-editor');
    const input = page.getByTestId('credit-cell-editor-people-input');
    const list = page.getByTestId('credit-cell-editor-people-suggestions');

    await cell.dblclick();
    await expect(editor).toBeVisible();
    await expect(input).toBeFocused();
    await page.keyboard.type('Al');
    await expect(input).toHaveValue('Al');
    await expect(list).toBeVisible();
    // Escape walks out: the list, the typed text, then the popover, back to the grid.
    await page.keyboard.press('Escape');
    await expect(list).toBeHidden();
    await page.keyboard.press('Escape');
    await expect(input).toHaveValue('');
    await expect(editor).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(editor).toBeHidden();
    await expect(scroller).toBeFocused();
    expect(await creditCalls(page)).toEqual([]);

    // The grid taking the focus back while the popover is open does not keep it.
    await cell.dblclick();
    await expect(input).toBeFocused();
    await scroller.evaluate((el) => (el as HTMLElement).focus());
    await expect(input).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(editor).toBeHidden();

    // A letter on the selected cell opens the popover with it; the rest follows.
    await page.keyboard.type('Zoe');
    await expect(editor).toBeVisible();
    await expect(input).toBeFocused();
    await expect(input).toHaveValue('Zoe');
    await page.keyboard.press('Enter');
    await page.keyboard.press('Enter');
    await expect(editor).toBeHidden();
    await expect(scroller).toBeFocused();
    await expect.poll(() => creditCalls(page)).toEqual([
      { targets: { type: 'shots', shots: [[sectionId, shotId]] }, edit: { set_explorers: [{ name: 'Zoe' }] } },
    ]);
  });
});
