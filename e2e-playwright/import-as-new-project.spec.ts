import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';

/**
 * #576 HFI-4 / decision HFI-4-D1 "parse first, then switch": "Import as new
 * project" with a project open keeps the project open while the file parses,
 * with the confirm dialog showing the parse in progress; only a successful parse
 * closes the project and opens the new-project dialog, a failed parse leaves the
 * project open, and a result that settles after the confirm was cancelled is
 * dropped. The parse (`parse_walls`) is HELD on a promise the test settles in
 * the page, so every state in between can be looked at.
 *
 * Decision HFI-4-D2 "prompt after a good parse": over a project with unsaved
 * changes the save-changes prompt comes only after the parse succeeded (a failed
 * parse asks nothing); Discard there closes the project and opens the new-project
 * dialog with the parsed file, Cancel keeps the project and drops the file.
 *
 * The router-level rules - every format, a project switch during the parse, the
 * SpeleoDB hand-back prompt - are pinned in
 * src/utils/__tests__/openOrImport.importAsNew.test.ts and
 * src/components/Dialogs/__tests__/importAsNewPrompts.test.tsx.
 */

const FILE = '/mock/CaveA.srv';

/** A WallsFile in the parser's shape: one resolved survey. All synthetic. */
const WALLS_FILE = {
  project_name: 'CaveA',
  book_count: 1,
  survey_count: 1,
  resolved_survey_count: 1,
  shot_count: 10,
  splay_count: 0,
  has_georeference: false,
  datum: null,
  utm_zone: null,
  fixed_station_count: 0,
  missing_file_count: 0,
  skipped_attachment_count: 0,
  surveys: [
    { name: 'A', title: 'A', date: '', segment: '', shot_count: 10, splay_count: 0, fixed_station_count: 0, resolved: true },
  ],
  warnings: [],
};

interface HeldParseWindow {
  __MOCK_IPC_OVERRIDES__?: Record<string, unknown>;
  __heldParse?: Promise<unknown>;
  __settleParse?: { resolve: (value: unknown) => void; reject: (reason: unknown) => void };
}

/** Pick FILE in the next picker, and hold its parse until the test settles it. */
async function pickWithHeldParse(page: Page): Promise<void> {
  await page.evaluate((path) => {
    const w = window as unknown as HeldParseWindow;
    const held = new Promise((resolve, reject) => {
      w.__settleParse = { resolve, reject };
    });
    // A rejection the router has not reached yet must not surface as unhandled.
    held.catch(() => undefined);
    w.__heldParse = held;
    w.__MOCK_IPC_OVERRIDES__ = { ...(w.__MOCK_IPC_OVERRIDES__ ?? {}), 'plugin:dialog|open': path, parse_walls: held };
  }, FILE);
}

/** Settle the held parse, then let the router's continuation run before returning. */
async function settleParse(page: Page, outcome: { value: unknown } | { error: unknown }): Promise<void> {
  await page.evaluate(async (o) => {
    const w = window as unknown as HeldParseWindow;
    if ('value' in o) w.__settleParse?.resolve(o.value);
    else w.__settleParse?.reject(o.error);
    await w.__heldParse?.catch(() => undefined);
    await new Promise((resolve) => setTimeout(resolve, 0));
  }, outcome);
}

async function openProjectName(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: { projectStore: { getState: () => { project: { name: string } | null } } };
    }).__E2E_TEST__;
    return e2e?.projectStore.getState().project?.name ?? null;
  });
}

/** Mark the open project as having unsaved changes (its close then asks first). */
async function makeDirty(page: Page): Promise<void> {
  await page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__: { projectStore: { setState: (s: { isDirty: boolean }) => void } };
    }).__E2E_TEST__;
    e2e.projectStore.setState({ isDirty: true });
  });
}

/** Ctrl+I with a project open: the confirm, then "Close and import". */
async function confirmImportAsNew(page: Page): Promise<void> {
  await page.keyboard.press('Control+i');
  const confirmDialog = page.getByTestId('import-as-new-project-dialog');
  await expect(confirmDialog).toBeVisible({ timeout: 10000 });
  await page.getByTestId('import-as-new-project-confirm').click();
}

test.describe('Import as new project: parse first, then switch (#576 HFI-4)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'ProjectA');
    await pickWithHeldParse(page);
  });

  test('keeps the project open with the parse in progress, then switches once the parse succeeds', async ({ page }) => {
    await confirmImportAsNew(page);

    // While the file parses: the confirm stays up and says so, Close and import
    // is spent, and the project is still open underneath.
    const parsing = page.getByTestId('import-as-new-project-parsing');
    await expect(parsing).toBeVisible();
    await expect(parsing).toContainText('CaveA.srv');
    await expect(page.getByTestId('import-as-new-project-confirm')).toBeDisabled();
    await expect(page.getByTestId('import-as-new-project-cancel')).toBeEnabled();
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    expect(await openProjectName(page)).toBe('ProjectA');

    await settleParse(page, { value: WALLS_FILE });

    // Only now: the project closes and the new-project dialog takes over.
    await expect(page.getByTestId('new-project-for-import-dialog')).toBeVisible();
    await expect(page.getByTestId('import-as-new-project-dialog')).toBeHidden();
    await expect.poll(() => openProjectName(page)).toBeNull();
  });

  test('a failed parse shows the error and leaves the project open', async ({ page }) => {
    await confirmImportAsNew(page);
    await expect(page.getByTestId('import-as-new-project-parsing')).toBeVisible();

    await settleParse(page, { error: { code: 'PARSE_FAILED', message: 'Synthetic: the survey file could not be read' } });

    await expect(page.getByTestId('error-dialog')).toContainText('Synthetic: the survey file could not be read');
    await expect(page.getByTestId('new-project-for-import-dialog')).toBeHidden();
    expect(await openProjectName(page)).toBe('ProjectA');
  });

  test('cancelling while the file parses drops the late result: no new-project dialog, the project stays open', async ({ page }) => {
    await confirmImportAsNew(page);
    await expect(page.getByTestId('import-as-new-project-parsing')).toBeVisible();

    await page.getByTestId('import-as-new-project-cancel').click();
    await expect(page.getByTestId('import-as-new-project-dialog')).toBeHidden();

    // settleParse returns only after the router has handled the late result.
    await settleParse(page, { value: WALLS_FILE });

    await expect(page.getByTestId('new-project-for-import-dialog')).toBeHidden();
    expect(await openProjectName(page)).toBe('ProjectA');
    await expect(page.getByTestId('single-system-view')).toBeVisible();
  });
});

test.describe('Import as new project over unsaved changes: the prompt comes after a good parse (#576 HFI-4-D2)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'ProjectA');
    await makeDirty(page);
    await pickWithHeldParse(page);
  });

  test('parses first with no prompt; after the parse, Discard closes the project and opens the new-project dialog', async ({ page }) => {
    await confirmImportAsNew(page);

    // While the file parses: no save-changes prompt yet, the project is open.
    await expect(page.getByTestId('import-as-new-project-parsing')).toBeVisible();
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeHidden();
    expect(await openProjectName(page)).toBe('ProjectA');

    await settleParse(page, { value: WALLS_FILE });

    // Only after the good parse: the prompt, over the still-open project.
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();
    await expect(page.getByTestId('import-as-new-project-dialog')).toBeHidden();
    expect(await openProjectName(page)).toBe('ProjectA');
    // It says what it is about (#576 HFI-4-D3), not the generic "go back" text.
    await expect(page.getByTestId('unsaved-changes-dialog')).toContainText('Import as a new project?');
    await expect(page.getByTestId('unsaved-changes-dialog')).toContainText('Unsaved changes in ProjectA will be lost.');

    await page.getByTestId('unsaved-changes-discard').click();

    // The parsed file goes on to the new-project dialog: no second pick, no second parse.
    await expect(page.getByTestId('new-project-for-import-dialog')).toBeVisible();
    await expect.poll(() => openProjectName(page)).toBeNull();
  });

  test('a failed parse shows its error and asks nothing', async ({ page }) => {
    await confirmImportAsNew(page);
    await expect(page.getByTestId('import-as-new-project-parsing')).toBeVisible();

    await settleParse(page, { error: { code: 'PARSE_FAILED', message: 'Synthetic: the survey file could not be read' } });

    await expect(page.getByTestId('error-dialog')).toContainText('Synthetic: the survey file could not be read');
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeHidden();
    expect(await openProjectName(page)).toBe('ProjectA');
  });

  test('cancelling the prompt keeps the project, unsaved changes and all, and drops the parsed file', async ({ page }) => {
    await confirmImportAsNew(page);
    await settleParse(page, { value: WALLS_FILE });
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();

    await page.getByTestId('unsaved-changes-cancel').click();

    await expect(page.getByTestId('unsaved-changes-dialog')).toBeHidden();
    await expect(page.getByTestId('new-project-for-import-dialog')).toBeHidden();
    expect(await openProjectName(page)).toBe('ProjectA');
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    expect(
      await page.evaluate(
        () => (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => { isDirty: boolean } } } }).__E2E_TEST__.projectStore.getState().isDirty,
      ),
    ).toBe(true);
  });
});
