import { test, expect } from './fixtures/coverage';
import { setupMockedPage, seedProjectWithLines, waitForMapReady, createProject } from './helpers/app';

/**
 * The focus ring must only appear during keyboard navigation, not on mouse
 * clicks - while the element is still focused. Because :focus-visible matches on
 * clicks in some engines (WebKit), an input-modality tracker tags <html
 * data-input-modality> and the ring is gated on it (index.css).
 */
test.describe('Focus ring (keyboard-only)', () => {
  test('input modality gates the ring: click = none, keyboard = ring', async ({ page }) => {
    await setupMockedPage(page);
    await page.goto('/');
    await createProject(page, 'Focus Test');
    await seedProjectWithLines(page, [
      { name: 'Main Line', direction: 'in', shots: [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -2, depth_out: -3, left: 1, right: 1, up: 1, down: 1 }] },
      { name: 'Side Passage', direction: 'out', shots: [{ azimuth_in: 90, azimuth_out: 270, length: 8, depth_in: -3, depth_out: -4, left: 1, right: 1, up: 1, down: 1 }] },
    ]);
    await waitForMapReady(page);

    const ids: number[] = await page.evaluate(() =>
      (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => { project: { lines: { id: number }[] } } } } })
        .__E2E_TEST__.projectStore.getState().project.lines.map((s) => s.id),
    );
    const modality = () => page.evaluate(() => document.documentElement.dataset.inputModality);

    // Click → pointer modality; the row is focused (and selected) but the ring is
    // suppressed - only transparent shadows remain (selection shows via bg tint).
    const row0 = page.locator(`[data-testid="line-row-${ids[0]}"]`);
    await row0.click();
    expect(await modality()).toBe('pointer');
    expect(await row0.evaluate((el) => el.matches(':focus'))).toBe(true);
    const clickedShadow = await row0.evaluate((el) => getComputedStyle(el).boxShadow);
    expect(clickedShadow).not.toMatch(/rgb\(/); // no opaque ring color on click (rgba(0,0,0,0) only)

    // Keyboard nav → keyboard modality; the focused row shows an opaque ring.
    await page.keyboard.press('ArrowDown');
    expect(await modality()).toBe('keyboard');
    const row1 = page.locator(`[data-testid="line-row-${ids[1]}"]`);
    expect(await row1.evaluate((el) => el.matches(':focus'))).toBe(true);
    const kbShadow = await row1.evaluate((el) => getComputedStyle(el).boxShadow);
    expect(kbShadow).toMatch(/rgb\(/); // opaque ring color on keyboard nav

    // Forcing pointer modality suppresses the ring even while still focused
    // (the case where an engine keeps :focus/:focus-visible after a click).
    const forcedShadow = await row1.evaluate((el) => {
      document.documentElement.dataset.inputModality = 'pointer';
      return getComputedStyle(el).boxShadow;
    });
    expect(forcedShadow).not.toMatch(/rgb\(/);
  });

  test('buttons show no soft focus glow on click, but do on keyboard focus', async ({ page }) => {
    await setupMockedPage(page);
    await page.goto('/');
    await createProject(page, 'Focus Test');
    await seedProjectWithLines(page, [
      { name: 'Main Line', direction: 'in', shots: [{ azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -2, depth_out: -3, left: 1, right: 1, up: 1, down: 1 }] },
    ]);
    await waitForMapReady(page);

    // The fit-to-view control is a <button> (so it gets the element-level
    // button:focus glow). Clicking it must not show the soft box-shadow glow.
    const btn = page.getByTestId('fit-to-view-button');
    await btn.click();
    expect(await page.evaluate(() => document.documentElement.dataset.inputModality)).toBe('pointer');
    // The glow (a semi-transparent box-shadow) is fully suppressed on click.
    const clickShadow = await btn.evaluate((el) => getComputedStyle(el).boxShadow);
    expect(clickShadow).toBe('none');

    // Keyboard focus → keyboard modality → the glow returns.
    const kbShadow = await btn.evaluate((el) => {
      document.documentElement.dataset.inputModality = 'keyboard';
      el.focus();
      return getComputedStyle(el).boxShadow;
    });
    expect(kbShadow).not.toBe('none'); // soft glow present on keyboard focus
  });
});
