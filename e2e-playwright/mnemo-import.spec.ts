import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, openAppMenu } from './helpers/app';

/**
 * Import from MNemo over USB (#725). The mock answers one connected MNemo and a
 * successful pull whose saved path is a `.dmp`, so these cover the menu rows,
 * the dialog, and the hand-off to the normal open/import route. The serial
 * protocol itself is pinned in Rust (`commands/mnemo`), including over a real
 * pseudo-terminal.
 */

async function openMnemoDialog(page: Page): Promise<void> {
  await openAppMenu(page);
  await page.getByTestId('app-menu-import-from-mnemo').click();
  await expect(page.getByTestId('mnemo-import-dialog')).toBeVisible({ timeout: 5000 });
}

async function readMnemo(page: Page): Promise<void> {
  await expect(page.getByTestId('mnemo-port-select')).toHaveValue('/dev/cu.usbmodem101');
  await page.getByTestId('mnemo-download').click();
  await expect(page.getByTestId('mnemo-result-summary')).toContainText('2 sections, 14 shots');
}

function pullCalls(page: Page): Promise<unknown> {
  return page.evaluate(() => (window as unknown as Record<string, unknown>).__MOCK_PULL_MNEMO_CALLS__ ?? []);
}

async function overrideIpc(page: Page, cmd: string, reply: unknown): Promise<void> {
  await page.evaluate(
    ([command, value]) => {
      const win = window as unknown as Record<string, unknown>;
      const overrides = (win.__MOCK_IPC_OVERRIDES__ as Record<string, unknown> | undefined) ?? {};
      overrides[command as string] = value;
      win.__MOCK_IPC_OVERRIDES__ = overrides;
    },
    [cmd, reply] as const,
  );
}

test.describe('Import from MNemo', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('from the home screen, a pull continues into a new project for the import', async ({ page }) => {
    await openMnemoDialog(page);
    await readMnemo(page);
    expect(await pullCalls(page)).toEqual([{ port: '/dev/cu.usbmodem101' }]);

    await page.getByTestId('mnemo-continue').click();
    await expect(page.getByTestId('mnemo-import-dialog')).toBeHidden();
    const newProject = page.getByTestId('new-project-for-import-dialog');
    await expect(newProject).toBeVisible({ timeout: 5000 });
    await expect(newProject.getByTestId('project-name-input')).toHaveValue('MNemo-2026-11-13_143005');
  });

  test('in a project, a pull continues into the DMP section choice', async ({ page }) => {
    await createProject(page, 'MNemo Import Test');
    await openMnemoDialog(page);
    await readMnemo(page);

    await page.getByTestId('mnemo-continue').click();
    const selection = page.getByTestId('dmp-selection-dialog');
    await expect(selection).toBeVisible({ timeout: 5000 });
    await expect(selection.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });
  });

  test('in a read-only mode, the saved file meets the same import guard as any .dmp', async ({ page }) => {
    await createProject(page, 'MNemo Read-only Test');
    await page.getByTestId('mode-export').click();
    await openMnemoDialog(page);
    await readMnemo(page);

    await page.getByTestId('mnemo-continue').click();
    await expect(page.getByTestId('import-blocked-dialog')).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('dmp-selection-dialog')).toHaveCount(0);
  });

  test('with nothing connected, it says so and cannot read', async ({ page }) => {
    await overrideIpc(page, 'list_mnemo_ports', []);
    await openMnemoDialog(page);
    await expect(page.getByTestId('mnemo-no-ports')).toBeVisible();
    await expect(page.getByTestId('mnemo-download')).toBeDisabled();
    expect(await pullCalls(page)).toEqual([]);
  });
});
