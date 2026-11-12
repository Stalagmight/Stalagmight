import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithSections,
  getShotScreenPositions,
  getStationScreenPositions,
  waitForDrawnStations,
  readCanvasPixel,
  type PixelRgba,
  type SectionSeed,
} from './helpers/app';

/**
 * The depth / distance colour scale (docs/color-palettes.md): the legend cog,
 * the palette picker with live preview, the Levels range, and the export
 * panel's copy of the same controls. Painted pixels are the oracle for the map
 * (the survey is a custom WebGL layer).
 */

async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

const sections: SectionSeed[] = [
  {
    name: 'Scale A',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 12, depth_in: 0, depth_out: -6 },
      { azimuth_in: 90, azimuth_out: 270, length: 8, depth_in: -6, depth_out: -12 },
      { azimuth_in: 90, azimuth_out: 270, length: 8, depth_in: -12, depth_out: -20 },
    ],
  },
];

async function waitForMapLoaded(page: Page): Promise<void> {
  await expect
    .poll(
      () =>
        page.evaluate(() => {
          const map = (window as unknown as { __stalagmight_map__?: { loaded: () => boolean } }).__stalagmight_map__;
          return map?.loaded() ?? false;
        }),
      { timeout: 10000 },
    )
    .toBe(true);
}

async function zoomIn(page: Page, delta = 4): Promise<void> {
  await page.evaluate((d) => {
    const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number; setZoom: (z: number) => void } })
      .__stalagmight_map__;
    if (map) map.setZoom(map.getZoom() + d);
  }, delta);
}

async function enableDepth(page: Page): Promise<void> {
  await page.locator('button:has-text("Display")').click();
  await page.locator('[data-testid="display-toggle-showDepthGradient"]').click();
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-testid="depth-legend"]')).toBeVisible();
}

/** Painted fill of every drawn station, as one comparable string. */
async function stationFills(page: Page): Promise<string> {
  const out: string[] = [];
  for (const st of await getStationScreenPositions(page)) {
    const px: PixelRgba = await readCanvasPixel(page, st.x, st.y);
    out.push(`${st.sectionId}:${st.stationIndex}=${px.slice(0, 3).join(',')}`);
  }
  return out.sort().join(' ');
}

/** Painted colour at the middle of every drawn shot. The mock draws every
 *  station at z = 0 (the deep end here), so shots, whose depth ramps along
 *  their length, are what a range change visibly moves. */
async function shotMids(page: Page): Promise<string> {
  const out: string[] = [];
  for (const shot of await getShotScreenPositions(page)) {
    const px: PixelRgba = await readCanvasPixel(
      page,
      shot.from.x + (shot.to.x - shot.from.x) * 0.5,
      shot.from.y + (shot.to.y - shot.from.y) * 0.5,
    );
    out.push(`${shot.sectionId}:${shot.shotId}=${px.slice(0, 3).join(',')}`);
  }
  return out.sort().join(' ');
}

async function openCog(page: Page, kind: 'depth' | 'distance'): Promise<void> {
  await page.locator(`[data-testid="${kind}-legend-settings-button"]`).click();
  await expect(page.locator(`[data-testid="color-scale-popover-${kind}"]`)).toBeVisible();
}

test.describe('Colour scale', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Colour Scale Test');
    await seedProjectWithSections(page, sections);
    await waitForMapLoaded(page);
    await enableDepth(page);
    await zoomIn(page);
    await waitForDrawnStations(page, 2);
  });

  test('the palette list stays against its trigger when a search shrinks it', async ({ page }) => {
    // A tall window puts the trigger low, so the list opens UPWARD: its top is
    // placed from its height, which is what goes stale when it shrinks.
    await page.setViewportSize({ width: 1280, height: 1100 });
    await openCog(page, 'depth');
    const trigger = page.locator('[data-testid="color-scale-palette-trigger"]');
    const panel = page.locator('[data-testid="color-scale-palette-panel"]');
    await trigger.click();
    await expect(panel).toBeVisible();
    const t0 = (await trigger.boundingBox())!;
    const p0 = (await panel.boundingBox())!;
    expect(p0.y + p0.height, 'the list opened above its trigger').toBeLessThanOrEqual(t0.y + 1);
    // The gap between the bottom of the list and the top of the trigger.
    const gap = async (): Promise<number> => {
      const t = (await trigger.boundingBox())!;
      const p = (await panel.boundingBox())!;
      return Math.abs(t.y - (p.y + p.height));
    };
    expect(await gap()).toBeLessThan(10);
    // One open row before (the Theme folder) and one match after: the row count
    // is unchanged while the folder headers go, and the list shrinks.
    const before = (await panel.boundingBox())!.height;
    await page.locator('[data-testid="color-scale-palette-search"]').fill('viridis');
    await expect(page.locator('[data-testid="color-scale-palette-option-mpl.viridis"]')).toBeVisible();
    await expect.poll(async () => (await panel.boundingBox())!.height).toBeLessThan(before / 2);
    await expect.poll(gap, { timeout: 2000 }).toBeLessThan(10);
    // Clearing the search grows it back, still against the trigger.
    await page.locator('[data-testid="color-scale-palette-search"]').fill('');
    await expect.poll(async () => (await panel.boundingBox())!.height).toBeGreaterThan(before - 2);
    await expect.poll(gap, { timeout: 2000 }).toBeLessThan(10);
  });

  test('hovering a palette previews it on the map, Escape reverts, a click keeps it', async ({ page }) => {
    const themed = await stationFills(page);
    await openCog(page, 'depth');
    await page.locator('[data-testid="color-scale-palette-trigger"]').click();
    await page.locator('[data-testid="color-scale-palette-search"]').fill('viridis');
    await page.locator('[data-testid="color-scale-palette-option-mpl.viridis"]').hover();
    await expect.poll(() => stationFills(page), { timeout: 4000 }).not.toBe(themed);

    await page.locator('[data-testid="color-scale-palette-search"]').press('Escape');
    await expect(page.locator('[data-testid="color-scale-palette-panel"]')).toHaveCount(0);
    await expect.poll(() => stationFills(page), { timeout: 4000 }).toBe(themed);

    await page.locator('[data-testid="color-scale-palette-trigger"]').click();
    await page.locator('[data-testid="color-scale-palette-search"]').fill('viridis');
    await page.locator('[data-testid="color-scale-palette-option-mpl.viridis"]').click();
    await expect(page.locator('[data-testid="color-scale-palette-current"]')).toHaveText('viridis');
    await expect.poll(() => stationFills(page), { timeout: 4000 }).not.toBe(themed);

    // Persisted through the partial config command, and only there.
    const calls = await page.evaluate(
      () => (window as unknown as { __MOCK_MAP_PALETTE_CALLS__?: Array<{ depth: { id: string } }> }).__MOCK_MAP_PALETTE_CALLS__,
    );
    expect(calls?.[calls.length - 1]?.depth.id).toBe('mpl.viridis');

    // Reset brings back the theme ramp.
    await page.locator('[data-testid="color-scale-palette-reset"]').click();
    await expect.poll(() => stationFills(page), { timeout: 4000 }).toBe(themed);
  });

  test('pinning puts a palette at the top of the picker', async ({ page }) => {
    await openCog(page, 'depth');
    await page.locator('[data-testid="color-scale-palette-trigger"]').click();
    await page.locator('[data-testid="color-scale-palette-search"]').fill('magma');
    await page.locator('[data-testid="color-scale-palette-option-mpl.magma"]').hover();
    await page.locator('[data-testid="color-scale-palette-pin-mpl.magma"]').click();
    await page.locator('[data-testid="color-scale-palette-search"]').fill('');
    await expect(page.locator('[data-testid="color-scale-palette-pinned-mpl.magma"]')).toBeVisible();
    await expect(page.locator('[data-testid="color-scale-palette-current"]')).toHaveText('Theme');
  });

  test('the palette list grows back to full height when a search is cleared', async ({ page }) => {
    await openCog(page, 'depth');
    await page.locator('[data-testid="color-scale-palette-trigger"]').click();
    const panel = page.locator('[data-testid="color-scale-palette-panel"]');
    const height = async () => (await panel.boundingBox())!.height;
    // Open a folder so the search below changes how many rows are visible:
    // that is what makes the list re-measure (and, before the fix, keep the
    // few-results height once the search was cleared).
    await page.locator('[data-testid="color-scale-palette-group-mpl-sequential"]').click();
    await expect.poll(height).toBeGreaterThan(200);
    const full = await height();
    const search = page.locator('[data-testid="color-scale-palette-search"]');
    await search.fill('viridis');
    await expect.poll(height).toBeLessThan(full);
    await search.fill('');
    await expect.poll(height).toBeGreaterThanOrEqual(full - 1);
  });

  test('the Levels range recolours the map and resets to the data range', async ({ page }) => {
    const themed = await shotMids(page);
    await openCog(page, 'depth');
    await expect(page.locator('[data-testid="color-scale-levels-histogram"] rect').first()).toBeVisible();
    const max = page.locator('[data-testid="color-scale-levels-max-input"]');
    const min = page.locator('[data-testid="color-scale-levels-min-input"]');
    const dataMin = Number(await min.inputValue());
    // Squeeze the range to its shallow end: everything deeper clamps to the end colour.
    await max.fill(String(dataMin + 1));
    await max.press('Enter');
    await expect.poll(() => shotMids(page), { timeout: 4000 }).not.toBe(themed);
    await expect(page.locator('[data-testid="depth-legend-max"]')).toContainText(String(dataMin + 1));

    await page.locator('[data-testid="color-scale-range-reset"]').click();
    await expect.poll(() => shotMids(page), { timeout: 4000 }).toBe(themed);
    await expect(page.locator('[data-testid="color-scale-range-reset"]')).toBeDisabled();
  });

  test('dragging the midpoint handle previews live and commits a gamma', async ({ page }) => {
    const themed = await shotMids(page);
    await openCog(page, 'depth');
    const handle = page.locator('[data-testid="color-scale-levels-mid-handle"]');
    const box = (await handle.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 - 60, box.y + box.height / 2, { steps: 6 });
    await expect.poll(() => shotMids(page), { timeout: 4000 }).not.toBe(themed);
    await page.mouse.up();
    const gamma = Number(await page.locator('[data-testid="color-scale-levels-gamma-input"]').inputValue());
    expect(gamma).toBeGreaterThan(1);
  });

  test('the popover and its palette list are on top of the panels at 1280x720', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 720 });
    await openCog(page, 'depth');
    // Hit-test the middle of each control: the element there must belong to the
    // popover, not to a side panel, the data table or a map control above it.
    const onTop = (testId: string) =>
      page.evaluate((id) => {
        const el = document.querySelector(`[data-testid="${id}"]`);
        if (!el) return 'missing';
        const r = el.getBoundingClientRect();
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return hit && el.contains(hit) ? 'ok' : `covered by ${hit?.closest('[data-testid]')?.getAttribute('data-testid') ?? hit?.tagName}`;
      }, testId);
    for (const id of [
      'color-scale-palette-trigger',
      'color-scale-levels-min-handle',
      'color-scale-levels-max-input',
      'color-scale-range-reset',
    ]) {
      expect(await onTop(id), id).toBe('ok');
    }
    await page.locator('[data-testid="color-scale-palette-trigger"]').click();
    // Opening focuses the search: typing searches instead of switching map tools.
    await page.keyboard.type('mag');
    await expect(page.locator('[data-testid="color-scale-palette-search"]')).toHaveValue('mag');
    expect(await onTop('color-scale-palette-option-mpl.magma')).toBe('ok');
    // Escape closes the list only.
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="color-scale-palette-panel"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="color-scale-popover-depth"]')).toBeVisible();
  });

  test('Escape closes the popover', async ({ page }) => {
    await openCog(page, 'depth');
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="color-scale-popover-depth"]')).toHaveCount(0);
  });

  test('the export panel offers the palette and its own range for depth', async ({ page }) => {
    await page.locator('[data-testid="mode-export"]').click();
    await expect(page.locator('[data-testid="export-mode-preview"]')).toBeVisible();
    await page.getByTestId('export-labels-options-toggle').click();
    await page.getByTestId('export-color-mode').click();
    await page.getByRole('listbox').locator('[role="option"]:has-text("By depth")').click();
    await expect(page.getByTestId('export-color-scale-depth')).toBeVisible();

    const preview = page.locator('[data-testid="export-mode-preview"] svg[data-threshold]');
    await expect(preview).toHaveAttribute('data-depth-scale', 'none');

    await page.getByTestId('export-color-scale-palette-trigger').click();
    await page.getByTestId('export-color-scale-palette-search').fill('viridis');
    await page.getByTestId('export-color-scale-palette-option-mpl.viridis').click();
    await expect(preview).toHaveAttribute('data-depth-scale', /^#440154:auto$/, { timeout: 4000 });

    const max = page.getByTestId('export-color-scale-levels-max-input');
    await max.fill('20');
    await max.press('Enter');
    await expect(preview).toHaveAttribute('data-depth-scale', /^#440154:0-20-1$/, { timeout: 4000 });
  });
});
