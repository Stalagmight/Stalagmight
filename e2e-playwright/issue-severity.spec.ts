import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapIdle,
  getShotScreenPositions,
  getStationScreenPositions,
  waitForDrawnShots,
  readCanvasPixel,
  type PixelRgba,
  type SectionSeed,
} from './helpers/app';

/**
 * E2E for the "Issue severity" Display coloring mode (#22). Unresolved warnings
 * come from the backend validator; the e2e mock returns none, so we inject them
 * via the project store (the same path warning-click.spec.ts uses) to drive
 * red/yellow.
 *
 * The survey is drawn by a custom WebGL layer now: the mode's color is resolved
 * per shot / station at build time and packed into a vertex attribute, so there is
 * no `survey-shots` / `survey-stations` layer to read a `line-color` /
 * `circle-color` paint off and no per-feature `colorIndex` to resolve through it.
 * The oracle is the PAINTED PIXEL, which also restores per-feature identity for
 * free (the drawn-geometry hook carries sectionId / shotId / stationIndex), so the
 * old geometry-matching against merged vector-tile groups is gone.
 *
 * Severity palette (fixed, themeColors): error #ef4444, warning #eab308,
 * clear #22c55e. The mode is graded: a shot whose readings disagree ramps from
 * clear (agree) to warning (at the threshold) to error (3x the threshold), so the
 * baseline seed records equal azimuths in and out.
 */

const SEV_ERROR = '#ef4444';
const SEV_WARNING = '#eab308';
const SEV_CLEAR = '#22c55e';

/** One section, 3 shots → 4 stations (idx 0..3). shot i joins station i and i+1. */
const sectionSeeds: SectionSeed[] = [
  {
    name: 'Severity A',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 12, depth_in: 0, depth_out: -6 },
      { azimuth_in: 90, azimuth_out: 90, length: 8, depth_in: -6, depth_out: -12 },
      { azimuth_in: 135, azimuth_out: 135, length: 6, depth_in: -12, depth_out: -15 },
    ],
  },
];

async function openDisplayMenu(page: Page): Promise<void> {
  await page.locator('button:has-text("Display")').click();
  await expect(page.locator('[data-testid="display-toggle-showStations"]')).toBeVisible({ timeout: 2000 });
}

async function isToggleOn(page: Page, toggleKey: string): Promise<boolean> {
  return (await page.locator(`[data-testid="display-toggle-${toggleKey}"]`).getAttribute('aria-pressed')) === 'true';
}

async function toggleDisplayOption(page: Page, toggleKey: string): Promise<void> {
  const colorKeys = ['showDepthGradient', 'showDistanceGradient', 'showIssueSeverity'];
  const before = await isToggleOn(page, toggleKey);
  // Color modes are single-select: turn one OFF by selecting Section, not by re-clicking the pill.
  if (before && colorKeys.includes(toggleKey)) {
    await page.locator('[data-testid="display-color-section"]').click();
  } else {
    await page.locator(`[data-testid="display-toggle-${toggleKey}"]`).click();
  }
  await expect.poll(async () => isToggleOn(page, toggleKey), { timeout: 2000 }).not.toBe(before);
}

async function getSectionIds(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { projectStore: { getState: () => { project: { sections: Array<{ id: number }> } | null } } };
    }).__E2E_TEST__;
    return stores?.projectStore.getState().project?.sections.map((s) => s.id) ?? [];
  });
}

async function getShotIds(page: Page, sectionId: number): Promise<number[]> {
  return page.evaluate((secId) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { project: { sections: Array<{ id: number; shots: Array<{ id: number }> }> } | null } };
      };
    }).__E2E_TEST__;
    const section = stores?.projectStore.getState().project?.sections.find((s) => s.id === secId);
    return section?.shots.map((shot) => shot.id) ?? [];
  }, sectionId);
}

interface MockWarning {
  sectionId: number;
  shotId: number;
  shotIndex: number;
  code: string;
  message: string;
  severity: 'warning' | 'error';
  isResolved: boolean;
}

async function injectWarnings(page: Page, warnings: MockWarning[]): Promise<void> {
  await page.evaluate((w) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { projectStore: { setState: (s: { warnings: unknown[] }) => void } };
    }).__E2E_TEST__;
    stores?.projectStore.setState({ warnings: w });
  }, warnings);
}

async function issueSeverityFlag(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { showIssueSeverity: boolean } } };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().showIssueSeverity ?? false;
  });
}

interface MapZoomHandle {
  getZoom: () => number;
  setZoom: (z: number) => void;
}

/**
 * Zoom in before probing painted pixels. The seeded survey is only a few metres
 * across, so at the fit-to-bounds zoom the whole thing is ~9 screen px wide and
 * the station discs cover the shot lines entirely, so a probe there cannot tell a
 * shot's color from a station's. `setZoom` is a jump (no animation) and the
 * geometry helpers project with the live map, so positions read after this call
 * already match the new view.
 *
 * A fit-to-bounds queued by the seed can still land after the map reports loaded
 * and take the zoom back, so hold the target zoom until the drawn shots really are
 * spread out on screen (`minSpanPx`).
 */
async function zoomIn(page: Page, delta = 4, minSpanPx = 40): Promise<void> {
  const target = await page.evaluate((d) => {
    const map = (window as unknown as { __stalagmight_map__?: MapZoomHandle }).__stalagmight_map__;
    if (!map) return 0;
    const z = map.getZoom() + d;
    map.setZoom(z);
    return z;
  }, delta);
  await expect.poll(async () => {
    await page.evaluate((z) => {
      const map = (window as unknown as { __stalagmight_map__?: MapZoomHandle }).__stalagmight_map__;
      if (map && map.getZoom() < z - 0.01) map.setZoom(z);
    }, target);
    const shots = await getShotScreenPositions(page);
    return shots.reduce((m, s) => Math.max(m, Math.hypot(s.to.x - s.from.x, s.to.y - s.from.y)), 0);
  }, { timeout: 10000 }).toBeGreaterThan(minSpanPx);
}

/** `#rrggbb` -> the RGB channels a painted pixel would carry. */
function hexRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

/**
 * Whether a painted sample is the given palette color. A tolerance keeps the
 * comparison honest without being brittle: the severity palette entries are far
 * apart from each other and from the section blue, so nothing else lands within it.
 */
function isColor(px: PixelRgba | undefined, hex: string, tolerance = 10): boolean {
  if (!px) return false;
  const [r, g, b] = hexRgb(hex);
  return (
    Math.abs(px[0] - r) <= tolerance &&
    Math.abs(px[1] - g) <= tolerance &&
    Math.abs(px[2] - b) <= tolerance
  );
}

/** Palette hex -> readable bucket name, so a color mismatch reads as a severity
 *  mismatch in the failure diff instead of two RGB triples. */
function severityName(hex: string): string {
  if (hex === SEV_ERROR) return 'error';
  if (hex === SEV_WARNING) return 'warning';
  return 'clear';
}

/** Which severity bucket a painted sample belongs to (or the raw RGB if none). */
function describeSeverity(px: PixelRgba | undefined): string {
  if (!px) return 'not-drawn';
  for (const hex of [SEV_ERROR, SEV_WARNING, SEV_CLEAR]) {
    if (isColor(px, hex)) return severityName(hex);
  }
  return `other(${px.slice(0, 3).join(',')})`;
}

/**
 * Painted color of every drawn shot, keyed by shotId, sampled at the shot's
 * midpoint. That point is clear of the station discs once zoomed in, and in
 * severity mode a shot is one flat color for its whole length (only depth mode
 * ramps along a shot).
 */
async function shotColorsById(page: Page): Promise<Record<number, PixelRgba>> {
  const out: Record<number, PixelRgba> = {};
  for (const shot of await getShotScreenPositions(page)) {
    out[shot.shotId] = await readCanvasPixel(page, shot.mid.x, shot.mid.y);
  }
  return out;
}

/**
 * Painted FILL of every drawn station, keyed by stationIndex (the seed is
 * single-section, so station_index is unique, keeping the original keying). The
 * center pixel of a station disc is its fill; the stroke is a ring around it.
 */
async function stationColorsByIndex(page: Page): Promise<Record<number, PixelRgba>> {
  const out: Record<number, PixelRgba> = {};
  for (const st of await getStationScreenPositions(page)) {
    out[st.stationIndex] = await readCanvasPixel(page, st.x, st.y);
  }
  return out;
}

test.describe('Issue Severity coloring (#22)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Issue Severity Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapIdle(page);
    await waitForDrawnShots(page, 3);
    await zoomIn(page);
  });

  test('toggle paints clear geometry green and reverts when turned off', async ({ page }) => {
    await openDisplayMenu(page);
    expect(await isToggleOn(page, 'showIssueSeverity')).toBe(false);

    // Baseline: section colors, not the clear-green severity color.
    const shotsBefore = await shotColorsById(page);
    expect(Object.values(shotsBefore).some((c) => !isColor(c, SEV_CLEAR))).toBe(true);

    await toggleDisplayOption(page, 'showIssueSeverity');
    expect(await issueSeverityFlag(page)).toBe(true);

    // No warnings ⇒ every shot and station is green (clear).
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      const vals = Object.values(shots);
      return vals.length > 0 && vals.every((c) => isColor(c, SEV_CLEAR));
    }, { timeout: 3000 }).toBe(true);
    await expect.poll(async () => {
      const stations = await stationColorsByIndex(page);
      const vals = Object.values(stations);
      return vals.length > 0 && vals.every((c) => isColor(c, SEV_CLEAR));
    }, { timeout: 3000 }).toBe(true);

    // Off ⇒ section colors return (no longer all green).
    await toggleDisplayOption(page, 'showIssueSeverity');
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return Object.values(shots).some((c) => !isColor(c, SEV_CLEAR));
    }, { timeout: 3000 }).toBe(true);
  });

  test('error/warning issues color the right shots and propagate to endpoint stations', async ({ page }) => {
    const sectionId = (await getSectionIds(page))[0]!;
    const shotIds = await getShotIds(page, sectionId);
    expect(shotIds.length).toBe(3);

    await injectWarnings(page, [
      { sectionId, shotId: shotIds[0]!, shotIndex: 0, code: 'ZERO_LENGTH', message: 'err', severity: 'error', isResolved: false },
      { sectionId, shotId: shotIds[1]!, shotIndex: 1, code: 'SHORT_SHOT', message: 'warn', severity: 'warning', isResolved: false },
      // Resolved error on the last shot must NOT color it.
      { sectionId, shotId: shotIds[2]!, shotIndex: 2, code: 'ZERO_LENGTH', message: 'err', severity: 'error', isResolved: true },
    ]);

    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showIssueSeverity');
    expect(await issueSeverityFlag(page)).toBe(true);

    // The old test read each feature's `colorIndex` (i.e. "this shot is in severity
    // bucket N"); the equivalent now is "this shot is PAINTED the severity-N color".
    const expectedShots = [SEV_ERROR, SEV_WARNING, SEV_CLEAR];
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return expectedShots.map((_, i) => describeSeverity(shots[shotIds[i]!])).join(',');
    }, { timeout: 4000 }).toBe(expectedShots.map(severityName).join(','));

    // Stations: idx0 (shot0 only) = error; idx1 (shot0 error + shot1 warning) =
    // error (max); idx2 (shot1 warning + resolved shot2) = warning; idx3 = clear.
    const expectedStations = [SEV_ERROR, SEV_ERROR, SEV_WARNING, SEV_CLEAR];
    await expect.poll(async () => {
      const st = await stationColorsByIndex(page);
      return expectedStations.map((_, i) => describeSeverity(st[i])).join(',');
    }, { timeout: 4000 }).toBe(expectedStations.map(severityName).join(','));
  });

  test('is mutually exclusive with the depth gradient', async ({ page }) => {
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showIssueSeverity');
    expect(await issueSeverityFlag(page)).toBe(true);

    // Enabling depth turns issue severity off (both paint the shots layer).
    await toggleDisplayOption(page, 'showDepthGradient');
    expect(await isToggleOn(page, 'showIssueSeverity')).toBe(false);
    expect(await issueSeverityFlag(page)).toBe(false);
    await expect(page.locator('[data-testid="depth-legend"]')).toBeVisible();

    // And the reverse: enabling issue severity turns depth off.
    await toggleDisplayOption(page, 'showIssueSeverity');
    expect(await issueSeverityFlag(page)).toBe(true);
    expect(await isToggleOn(page, 'showDepthGradient')).toBe(false);
  });

  test('the Warnings tab turns on Survey Issues and Errors for the visit; leaving brings the saved settings back', async ({ page }) => {
    // The visit is DERIVED from the tab (issue #57, finding #6) - never written to
    // the saved flags - so assert the rendered colors and the Display menu, not a
    // store flag. The saved toggles stay off throughout and nothing is saved.
    const saves: string[] = [];
    page.on('console', (msg) => {
      const m = /\[Mock IPC\] save_display_setting: (\S+)/.exec(msg.text());
      if (m) saves.push(m[1]!);
    });
    expect(await issueSeverityFlag(page)).toBe(false);

    // Baseline (Details tab): section colors, not the clear-green severity color.
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return Object.values(shots).some((c) => !isColor(c, SEV_CLEAR));
    }, { timeout: 3000 }).toBe(true);

    // Opening the Warnings tab paints Survey Issues (setting defaults on). With
    // agreeing readings and no warnings every shot paints clear-green.
    await page.getByTestId('detail-tab-warnings').click();
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      const vals = Object.values(shots);
      return vals.length > 0 && vals.every((c) => isColor(c, SEV_CLEAR));
    }, { timeout: 3000 }).toBe(true);
    expect(await issueSeverityFlag(page)).toBe(false);
    await expect(page.getByTestId('issue-legend-ramp')).toBeVisible();

    // The Display menu shows both on, marked automatic, with the visit note.
    await openDisplayMenu(page);
    for (const key of ['showIssueSeverity', 'showShotErrors']) {
      const pill = page.locator(`[data-testid="display-toggle-${key}"]`);
      await expect(pill).toHaveAttribute('aria-pressed', 'true');
      await expect(pill).toHaveAttribute('data-auto', 'true');
    }
    await expect(page.getByTestId('display-warnings-visit-note')).toBeVisible();

    // A pick during the visit holds for the visit: Depth paints the depth legend.
    await page.locator('[data-testid="display-toggle-showDepthGradient"]').click();
    await expect(page.locator('[data-testid="depth-legend"]')).toBeVisible();
    await expect(page.locator('[data-testid="display-toggle-showIssueSeverity"]')).toHaveAttribute('aria-pressed', 'false');
    await page.keyboard.press('Escape');

    // Leaving recomputes from the saved toggles: section colors, no legend.
    await page.getByTestId('detail-tab-details').click();
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return Object.values(shots).some((c) => !isColor(c, SEV_CLEAR));
    }, { timeout: 3000 }).toBe(true);
    await expect(page.locator('[data-testid="depth-legend"]')).toHaveCount(0);
    await expect(page.getByTestId('issue-legend')).toHaveCount(0);
    expect(saves.filter((s) => /showDepthGradient|showIssueSeverity|showShotErrors/.test(s))).toEqual([]);

    // Coming back starts a fresh visit with the defaults on again.
    await page.getByTestId('detail-tab-warnings').click();
    await expect(page.getByTestId('issue-legend-ramp')).toBeVisible();
  });

  test('Export mode does not inherit the Warnings-tab colouring', async ({ page }) => {
    await page.getByTestId('detail-tab-warnings').click();
    await expect(page.getByTestId('issue-legend-ramp')).toBeVisible();
    await page.locator('[data-testid="mode-export"]').click();
    await expect(page.getByTestId('issue-legend')).toHaveCount(0);
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return Object.values(shots).some((c) => !isColor(c, SEV_CLEAR));
    }, { timeout: 3000 }).toBe(true);
  });
});

/**
 * The graded ramp: five shots whose azimuths in and out disagree by 0, 10, 20,
 * 40 and 60 degrees against the default 20 degree threshold score 0, 0.5, 1, 2
 * and 3. Their colours are the ramp stops (displayColor.test.ts pins the bytes).
 */
const RAMP = ['#22c55e', '#86bc33', '#eab308', '#ed7c26', '#ef4444'];
const gradientSeeds: SectionSeed[] = [
  {
    name: 'Gradient',
    direction: 'in',
    shots: [0, 10, 20, 40, 60].map((delta, i) => ({
      azimuth_in: 90 + (i % 2 === 0 ? 0 : 60),
      azimuth_out: 90 + (i % 2 === 0 ? 0 : 60) + delta,
      length: 10,
      depth_in: -i,
      depth_out: -i,
    })),
  },
];

test.describe('Survey Issues gradient', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Gradient Test');
    await seedProjectWithSections(page, gradientSeeds);
    await waitForMapIdle(page);
    await waitForDrawnShots(page, 5);
    await zoomIn(page);
  });

  test('a shot is painted by how far its readings disagree', async ({ page }) => {
    const sectionId = (await getSectionIds(page))[0]!;
    const shotIds = await getShotIds(page, sectionId);
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showIssueSeverity');
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return shotIds.map((id, i) => isColor(shots[id], RAMP[i]!)).join(',');
    }, { timeout: 4000 }).toBe('true,true,true,true,true');
  });

  test('the colour cog regrades by its levels, drops a switched-off error, and never touches the warnings', async ({ page }) => {
    const sectionId = (await getSectionIds(page))[0]!;
    const shotIds = await getShotIds(page, sectionId);
    // The validator's own azimuth warnings for the 40 and 60 degree shots (over
    // the 20 degree threshold); the mock does not validate, so inject them.
    const azWarnings: MockWarning[] = [3, 4].map((i) => ({
      sectionId, shotId: shotIds[i]!, shotIndex: i, code: 'AZIMUTH_DEVIATION', message: 'az', severity: 'warning', isResolved: false,
    }));
    await injectWarnings(page, azWarnings);
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showIssueSeverity');
    await page.keyboard.press('Escape');
    const ipc: string[] = [];
    page.on('console', (msg) => {
      const m = /\[Mock IPC\] (update_map_palette|save_display_setting|update_project_settings)/.exec(msg.text());
      if (m) ipc.push(m[1]!);
    });
    await page.getByTestId('issues-legend-settings-button').click();
    await expect(page.getByTestId('color-scale-popover-issues')).toBeVisible();

    // Warn 10 / Max 20 instead of the threshold's 20 / 60: deltas 0, 10, 20, 40, 60
    // now score 0, 1, 3, 3, 3.
    const field = (h: string) => page.getByTestId(`issue-scale-azimuth-${h}-input`);
    await field('warn').fill('10');
    await field('warn').press('Enter');
    await field('max').fill('20');
    await field('max').press('Enter');
    const strict = [SEV_CLEAR, SEV_WARNING, SEV_ERROR, SEV_ERROR, SEV_ERROR];
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return shotIds.map((id, i) => isColor(shots[id], strict[i]!)).join(',');
    }, { timeout: 4000 }).toBe('true,true,true,true,true');
    // The validation threshold (the project's, else the config's) is untouched.
    expect(await page.evaluate(() => {
      type V = { validation?: { azimuth_diff_threshold: number } } | null;
      const stores = (window as unknown as {
        __E2E_TEST__?: { projectStore: { getState: () => { project: V; config: V } } };
      }).__E2E_TEST__;
      const s = stores?.projectStore.getState();
      return (s?.project?.validation ?? s?.config?.validation)?.azimuth_diff_threshold;
    })).toBe(20);

    // Azimuth off: no other error disagrees, so every shot paints clear - the
    // azimuth warnings themselves are still open, and set no yellow floor.
    await page.getByTestId('issue-scale-enable-azimuth').click();
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return shotIds.every((id) => isColor(shots[id], SEV_CLEAR));
    }, { timeout: 4000 }).toBe(true);
    // The switch is a config write; nothing goes to the project file or the
    // validator, and the warnings are the same two.
    expect(ipc).toContain('update_map_palette');
    expect(ipc.filter((c) => c !== 'update_map_palette')).toEqual([]);
    expect(await page.evaluate(() => {
      const stores = (window as unknown as {
        __E2E_TEST__?: { projectStore: { getState: () => { warnings: Array<{ code: string; isResolved: boolean }> } } };
      }).__E2E_TEST__;
      return stores?.projectStore.getState().warnings.filter((w) => w.code === 'AZIMUTH_DEVIATION' && !w.isResolved).length;
    })).toBe(2);
  });

  test('a palette picked in the colour cog paints the same scale', async ({ page }) => {
    const sectionId = (await getSectionIds(page))[0]!;
    const shotIds = await getShotIds(page, sectionId);
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showIssueSeverity');
    await page.keyboard.press('Escape');
    await page.getByTestId('issues-legend-settings-button').click();
    await page.getByTestId('issue-scale-palette-trigger').click();
    await page.getByTestId('issue-scale-palette-search').fill('viridis');
    await page.locator('[data-testid="issue-scale-palette-option-mpl.viridis"]').click();
    await expect(page.getByTestId('issue-scale-palette-current')).toHaveText('viridis');
    // Scores 0, 1 and 3 sit at the start, middle and end of the palette.
    const expected: Array<[number, string]> = [[0, '#440154'], [2, '#21918c'], [4, '#fde725']];
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return expected.map(([i, hex]) => isColor(shots[shotIds[i]!], hex, 12)).join(',');
    }, { timeout: 4000 }).toBe('true,true,true');
  });

  test('ignoring the azimuth warning drops that delta from the colour', async ({ page }) => {
    const sectionId = (await getSectionIds(page))[0]!;
    const shotIds = await getShotIds(page, sectionId);
    await injectWarnings(page, [
      { sectionId, shotId: shotIds[3]!, shotIndex: 3, code: 'AZIMUTH_DEVIATION', message: 'az', severity: 'warning', isResolved: true },
    ]);
    await openDisplayMenu(page);
    await toggleDisplayOption(page, 'showIssueSeverity');
    await expect.poll(async () => {
      const shots = await shotColorsById(page);
      return [isColor(shots[shotIds[3]!], SEV_CLEAR), isColor(shots[shotIds[4]!], SEV_ERROR)].join(',');
    }, { timeout: 4000 }).toBe('true,true');
  });
});
