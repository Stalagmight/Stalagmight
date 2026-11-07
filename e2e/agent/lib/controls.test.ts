import { beforeEach, describe, expect, it, vi } from 'vitest';

const evalInAppMock = vi.hoisted(() => vi.fn());
vi.mock('./page', () => ({ evalInApp: evalInAppMock }));

import { answerFileDialog, hoverElement, mapClick, selectOption, targetSelector, testIdSelector } from './controls';
import { shareableFailureText } from './shareableFailure';
import type { Browser } from 'webdriverio';

function fakeBrowser(elements: Record<string, Record<string, ReturnType<typeof vi.fn>>>) {
  const canvas = { waitForDisplayed: vi.fn(), moveTo: vi.fn() };
  const pointer = {
    move: vi.fn().mockReturnThis(),
    down: vi.fn().mockReturnThis(),
    up: vi.fn().mockReturnThis(),
    perform: vi.fn(),
  };
  return {
    $: vi.fn((selector: string) => elements[selector] ?? canvas),
    action: vi.fn(() => pointer),
    waitUntil: vi.fn(async (condition: () => Promise<boolean>) => {
      if (!(await condition())) throw new Error('condition was not met');
      return true;
    }),
  } as unknown as Browser;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('agent controls', () => {
  it('builds safe target selectors and hovers the resolved element', async () => {
    const selector = testIdSelector('C:\\Caves\\"survey"');
    const element = { waitForDisplayed: vi.fn(), moveTo: vi.fn() };
    const browser = fakeBrowser({ [selector]: element });

    expect(targetSelector({ testid: 'C:\\Caves\\"survey"' })).toBe(selector);
    await expect(hoverElement(browser, { testid: 'C:\\Caves\\"survey"' })).resolves.toBe(selector);
    expect(element.waitForDisplayed).toHaveBeenCalledWith({ timeout: 5000 });
    expect(element.moveTo).toHaveBeenCalledOnce();
  });

  it('selects a custom option through its encoded option test ID', async () => {
    const triggerSelector = '[data-testid="format-select"]';
    const optionSelector = '[data-testid="format-select-option-value%2Fwith%20space"]';
    const trigger = {
      waitForDisplayed: vi.fn(),
      getTagName: vi.fn().mockResolvedValue('button'),
      getAttribute: vi.fn().mockResolvedValue('format-select'),
      waitForClickable: vi.fn(),
      click: vi.fn(),
    };
    const option = { waitForClickable: vi.fn(), click: vi.fn() };
    const browser = fakeBrowser({ [triggerSelector]: trigger, [optionSelector]: option });

    await selectOption(browser, { testid: 'format-select', value: 'value/with space' });
    expect(trigger.click).toHaveBeenCalledOnce();
    expect(option.waitForClickable).toHaveBeenCalledWith({ timeout: 5000 });
    expect(option.click).toHaveBeenCalledOnce();
  });

  it('arms, triggers, and verifies a native-dialog answer atomically', async () => {
    evalInAppMock.mockResolvedValueOnce(true).mockResolvedValueOnce(true).mockResolvedValueOnce(undefined);
    const selector = testIdSelector('import-dmp-button');
    const trigger = { waitForDisplayed: vi.fn(), waitForClickable: vi.fn(), click: vi.fn() };
    const browser = fakeBrowser({ [selector]: trigger });

    const result = await answerFileDialog(browser, 'import-dmp-button', 'open', 'C:\\Caves\\survey.dmp');

    expect(result).toMatchObject({ selector, consumed: true });
    expect(trigger.click).toHaveBeenCalledOnce();
    expect(evalInAppMock.mock.calls[0]?.[1]).toContain('entry.armed = true');
    expect(evalInAppMock.mock.calls[0]?.[1]).not.toContain('setTimeout');
    expect(evalInAppMock.mock.calls[1]?.[1]).toContain('fileDialogConsumptions');
  });

  it('fails clearly when the app was not built with E2E hooks', async () => {
    evalInAppMock.mockResolvedValue(false);
    await expect(answerFileDialog(fakeBrowser({}), 'save-project-button', 'save', null)).rejects.toThrow(
      'file dialog hooks are unavailable',
    );
  });

  it('clicks geographic points relative to the live canvas element', async () => {
    evalInAppMock
      .mockResolvedValueOnce({
        x: 500,
        y: 400,
        interaction: 'maplibre',
        interactionSequenceBefore: 6,
      })
      .mockResolvedValueOnce('canvas.maplibregl-canvas')
      .mockResolvedValueOnce(7);
    const browser = fakeBrowser({});

    await expect(mapClick(browser, [12, 34], 'right')).resolves.toEqual({
      x: 500,
      y: 400,
      interaction: 'maplibre',
      interactionSequence: 7,
    });
    expect(evalInAppMock.mock.calls[0]?.[1]).not.toContain('map.fire');
    expect(evalInAppMock.mock.calls[0]?.[1]).toContain("canvasRoot.dataset.placementMode === 'true'");
    expect(evalInAppMock.mock.calls[0]?.[1]).toContain('projected.x >= rect.width');
    expect(evalInAppMock.mock.calls[1]?.[1]).toContain('document.elementFromPoint');
  });

  it('reports placement routing and rejects false-positive map clicks', async () => {
    const browser = fakeBrowser({});
    evalInAppMock
      .mockResolvedValueOnce({
        x: 320,
        y: 240,
        interaction: 'placement',
        interactionSequenceBefore: 2,
      })
      .mockResolvedValueOnce('canvas.maplibregl-canvas')
      .mockResolvedValueOnce(3);
    await expect(mapClick(browser, [-81.78, 24.55])).resolves.toEqual({
      x: 320,
      y: 240,
      interaction: 'placement',
      interactionSequence: 3,
    });

    evalInAppMock.mockResolvedValueOnce({ error: 'map point is outside the visible viewport' });
    await expect(mapClick(browser, [0, 0])).rejects.toThrow('map point is outside the visible viewport');

    evalInAppMock.mockResolvedValueOnce({ error: 'map point is outside the visible viewport at right edge' });
    await expect(mapClick(browser, [1, 1])).rejects.toThrow('right edge');
  });

  it("keeps the point and the page's target off the line a runner prints (#576 W51)", async () => {
    const browser = fakeBrowser({});
    const headline = async (run: Promise<unknown>): Promise<string> => {
      const err = await run.then(() => new Error('resolved'), (e: unknown) => e as Error);
      return shareableFailureText(err.message).split('\n')[0] ?? '';
    };
    evalInAppMock.mockResolvedValueOnce({ error: 'map point is outside the visible viewport: [-87.123, 20.456]' });
    const refused = await headline(mapClick(browser, [-87.123, 20.456]));
    expect(refused).toMatch(/^map click failed: \(\d+ characters of the app's text withheld\)$/);

    evalInAppMock
      .mockResolvedValueOnce({ x: 320, y: 240, interaction: 'maplibre', interactionSequenceBefore: 2 })
      .mockResolvedValueOnce('div[data-testid=system-card-qzCaveA]')
      .mockResolvedValueOnce(2);
    const missed = await headline(mapClick(browser, [-87.123, 20.456]));
    expect(missed).toBe('map maplibre handler did not process the click');
  });
});
