import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';

/**
 * "Changes on this computer are not on SpeleoDB" follows the app's dialog layout:
 * what each answer does is said once in the body, and every button sits in the
 * one footer row - Cancel, the discard, then the primary open - on one line,
 * before and after the discard is armed, whatever the other account's name.
 * Synthetic data only; the prompt is raised the way the open flow raises it.
 */

async function raise(page: Page, otherOwner: string | null): Promise<void> {
  await page.evaluate((owner) => {
    (window as unknown as {
      __E2E_TEST__: { uiStore: { getState: () => { openDialog: (name: string, data: unknown) => void } } };
    }).__E2E_TEST__.uiStore.getState().openDialog('speleodb-unpushed-local', {
      summary: {
        id: 'synthetic', name: 'Synthetic cave', permission: 'READ_AND_WRITE', lock_holder: null,
        type_tag: 'stalx', latest_commit_sha: null,
      },
      unpushedLocal: { local_path: '/synthetic/cave.stalx', last_edit_at: null, other_owner: owner },
    });
  }, otherOwner);
  await expect(page.getByTestId('speleodb-unpushed-local-dialog')).toBeVisible();
}

async function expectOneFooterLine(page: Page, labels: string[]): Promise<void> {
  const buttons = page.getByTestId('speleodb-unpushed-local-footer').getByRole('button');
  await expect(buttons).toHaveText(labels);
  const tops = await buttons.evaluateAll((els) => els.map((el) => Math.round(el.getBoundingClientRect().top)));
  expect(new Set(tops).size, `button tops ${JSON.stringify(tops)}`).toBe(1);
  // The three answers and the header's X: nothing else in the dialog is a button.
  await expect(page.getByTestId('speleodb-unpushed-local-dialog').getByRole('button')).toHaveCount(4);
}

test.describe('Unpushed local copy prompt layout', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test("this account's copy: one footer line, armed or not", async ({ page }) => {
    await raise(page, null);
    await expect(page.getByTestId('speleodb-unpushed-local-open-hint')).toContainText('"Discard my changes" deletes them');
    await expectOneFooterLine(page, ['Cancel', 'Discard my changes', 'Open my local copy']);
    await page.getByTestId('speleodb-unpushed-local-take-remote').click();
    await expect(page.getByTestId('speleodb-unpushed-local-discard-warning')).toBeVisible();
    await expectOneFooterLine(page, ['Cancel', 'Yes, discard my changes', 'Open my local copy']);
    await page.getByTestId('speleodb-unpushed-local-cancel').click();
    await expect(page.getByTestId('speleodb-unpushed-local-dialog')).toHaveCount(0);
  });

  test("another account's copy with a long name: still one footer line", async ({ page }) => {
    await raise(page, 'a.rather.long.account.name@speleo-club.example.org');
    await expect(page.getByTestId('speleodb-unpushed-local-message'))
      .toContainText('a.rather.long.account.name@speleo-club.example.org');
    await expectOneFooterLine(page, ['Cancel', 'Discard their changes', 'Open it read-only']);
    await page.getByTestId('speleodb-unpushed-local-take-remote').click();
    await expectOneFooterLine(page, ['Cancel', 'Yes, discard their changes', 'Open it read-only']);
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('speleodb-unpushed-local-dialog')).toHaveCount(0);
  });
});
