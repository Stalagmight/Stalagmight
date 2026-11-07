import type { Browser } from 'webdriverio';
import { randomUUID } from 'node:crypto';
import { foldThenCut } from './fixtureCorpus';
import { evalInApp } from './page';
import { cssStringLiteral, testIdSelector } from '../../../scripts/lib/test-id-selector.mjs';

export interface ElementTarget {
  testid?: string | undefined;
  selector?: string | undefined;
}

export interface DragRequest {
  fromTestid?: string | undefined;
  fromSelector?: string | undefined;
  fromX?: number | undefined;
  fromY?: number | undefined;
  toTestid?: string | undefined;
  toSelector?: string | undefined;
  toX?: number | undefined;
  toY?: number | undefined;
  button?: 'left' | 'right' | 'middle' | undefined;
  durationMs?: number | undefined;
}

export interface ScrollRequest extends ElementTarget {
  deltaX?: number | undefined;
  deltaY?: number | undefined;
  block?: ScrollLogicalPosition | undefined;
  inline?: ScrollLogicalPosition | undefined;
}

export interface SelectOptionRequest extends ElementTarget {
  value: string;
}

export interface ElementState {
  selector: string;
  count: number;
  unique: boolean;
  elements: Array<{
    index: number;
    tag: string;
    testid: string | null;
    id: string | null;
    role: string | null;
    text: string;
    value: string | null;
    displayed: boolean;
    disabled: boolean;
    checked: boolean | null;
    selected: boolean | null;
    ariaChecked: string | null;
    ariaExpanded: string | null;
    ariaPressed: string | null;
    ariaSelected: string | null;
  }>;
  truncated: boolean;
}

export interface MapClickResult {
  x: number;
  y: number;
  interaction: 'maplibre' | 'placement';
  interactionSequence: number;
}

interface MapClickProbe {
  x: number;
  y: number;
  interaction: 'maplibre' | 'placement';
  interactionSequenceBefore: number;
}

export async function dispatchMapPointer(
  browser: Browser,
  point: { x: number; y: number },
  button: 'left' | 'right' = 'left',
): Promise<string> {
  const dispatched = await evalInApp<string | null>(
    browser,
    `
      const target = document.elementFromPoint(args[0], args[1]);
      if (!target) return null;
      const right = args[2] === 'right';
      const button = right ? 2 : 0;
      const buttons = right ? 2 : 1;
      const common = {
        bubbles: true,
        cancelable: true,
        composed: true,
        button,
        clientX: args[0],
        clientY: args[1],
      };
      const PointerEventCtor = typeof PointerEvent === 'function' ? PointerEvent : MouseEvent;
      target.dispatchEvent(new PointerEventCtor('pointerdown', {
        ...common, buttons, pointerId: 1, pointerType: 'mouse', isPrimary: true,
      }));
      target.dispatchEvent(new MouseEvent('mousedown', { ...common, buttons }));
      target.dispatchEvent(new PointerEventCtor('pointerup', {
        ...common, buttons: 0, pointerId: 1, pointerType: 'mouse', isPrimary: true,
      }));
      target.dispatchEvent(new MouseEvent('mouseup', { ...common, buttons: 0 }));
      target.dispatchEvent(new MouseEvent(right ? 'contextmenu' : 'click', { ...common, buttons: 0, detail: 1 }));
      return target.tagName.toLowerCase() +
        (target.getAttribute('data-testid') ? '[data-testid=' + target.getAttribute('data-testid') + ']' : '') +
        (target.className && typeof target.className === 'string' ? '.' + target.className.trim().replace(/\s+/g, '.') : '');
    `,
    [point.x, point.y, button],
  );
  if (!dispatched) throw new Error('map point has no DOM target');
  return dispatched;
}

export function targetSelector(target: ElementTarget): string | undefined {
  return target.testid !== undefined ? testIdSelector(target.testid) : target.selector;
}

function requireSelector(target: ElementTarget, action: string): string {
  const selector = targetSelector(target);
  if (!selector) throw new Error(`${action} needs testid or selector`);
  return selector;
}

export async function hoverElement(browser: Browser, target: ElementTarget): Promise<string> {
  const selector = requireSelector(target, 'hover');
  const element = browser.$(selector);
  await element.waitForDisplayed({ timeout: 5000 });
  await element.moveTo();
  return selector;
}

export async function dragPointer(browser: Browser, request: DragRequest): Promise<{
  fromSelector?: string | undefined;
  toSelector?: string | undefined;
  fromPoint?: { x: number; y: number } | undefined;
  toPoint?: { x: number; y: number } | undefined;
}> {
  const fromSelector = targetSelector({ testid: request.fromTestid, selector: request.fromSelector });
  const toSelector = targetSelector({ testid: request.toTestid, selector: request.toSelector });
  const hasFromPoint = typeof request.fromX === 'number' && typeof request.fromY === 'number';
  const hasToPoint = typeof request.toX === 'number' && typeof request.toY === 'number';
  if (!fromSelector && !hasFromPoint) throw new Error('drag needs fromTestid/fromSelector or fromX+fromY');
  if (!toSelector && !hasToPoint) throw new Error('drag needs toTestid/toSelector or toX+toY');

  const pointer = browser.action('pointer', { parameters: { pointerType: 'mouse' } });
  if (fromSelector) {
    const source = browser.$(fromSelector);
    await source.waitForDisplayed({ timeout: 5000 });
    pointer.move({ origin: source, x: 0, y: 0 });
  } else {
    pointer.move({ origin: 'viewport', x: Math.round(request.fromX!), y: Math.round(request.fromY!) });
  }
  pointer.down({ button: request.button ?? 'left' }).pause(50);
  if (toSelector) {
    const target = browser.$(toSelector);
    await target.waitForDisplayed({ timeout: 5000 });
    pointer.move({ origin: target, x: 0, y: 0, duration: request.durationMs ?? 500 });
  } else {
    pointer.move({
      origin: 'viewport',
      x: Math.round(request.toX!),
      y: Math.round(request.toY!),
      duration: request.durationMs ?? 500,
    });
  }
  await pointer.up({ button: request.button ?? 'left' }).perform();

  return {
    fromSelector,
    toSelector,
    fromPoint: hasFromPoint ? { x: Math.round(request.fromX!), y: Math.round(request.fromY!) } : undefined,
    toPoint: hasToPoint ? { x: Math.round(request.toX!), y: Math.round(request.toY!) } : undefined,
  };
}

export async function scrollTarget(browser: Browser, request: ScrollRequest): Promise<string | undefined> {
  const selector = targetSelector(request);
  const hasDelta = request.deltaX !== undefined || request.deltaY !== undefined;
  if (!hasDelta) {
    if (!selector) throw new Error('scroll without deltaX/deltaY needs testid or selector');
    const element = browser.$(selector);
    await element.waitForExist({ timeout: 5000 });
    await element.scrollIntoView({
      behavior: 'instant',
      block: request.block ?? 'center',
      inline: request.inline ?? 'nearest',
    });
    return selector;
  }

  await evalInApp(
    browser,
    `
      const selector = args[0];
      const target = selector ? document.querySelector(selector) : window;
      if (!target) throw new Error('scroll target not found: ' + selector);
      target.scrollBy({ left: args[1], top: args[2], behavior: 'instant' });
      return true;
    `,
    [selector ?? null, request.deltaX ?? 0, request.deltaY ?? 0],
  );
  return selector;
}

export async function selectOption(browser: Browser, request: SelectOptionRequest): Promise<{
  selector: string;
  optionSelector?: string;
  native: boolean;
}> {
  const selector = requireSelector(request, 'select_option');
  const trigger = browser.$(selector);
  await trigger.waitForDisplayed({ timeout: 5000 });
  if ((await trigger.getTagName()).toLowerCase() === 'select') {
    await trigger.selectByAttribute('value', request.value);
    return { selector, native: true };
  }

  const testId = request.testid ?? (await trigger.getAttribute('data-testid'));
  if (!testId) {
    throw new Error('custom select_option needs a testid so its portaled option can be targeted');
  }
  await trigger.waitForClickable({ timeout: 5000 });
  await trigger.click();
  const optionSelector = testIdSelector(`${testId}-option-${encodeURIComponent(request.value)}`);
  const option = browser.$(optionSelector);
  await option.waitForClickable({ timeout: 5000 });
  await option.click();
  return { selector, optionSelector, native: false };
}

export async function queryElementState(browser: Browser, target: ElementTarget): Promise<ElementState> {
  const selector = requireSelector(target, 'element_state');
  const state = await evalInApp<ElementState>(
    browser,
    `
      const selector = args[0];
      const nodes = Array.from(document.querySelectorAll(selector));
      const elements = nodes.slice(0, 50).map((node, index) => {
        const el = node;
        const style = getComputedStyle(el);
        const rect = el.getBoundingClientRect();
        const hasValue = 'value' in el;
        const hasChecked = 'checked' in el;
        const hasSelected = 'selected' in el;
        return {
          index,
          tag: el.tagName.toLowerCase(),
          testid: el.getAttribute('data-testid'),
          id: el.id || null,
          role: el.getAttribute('role'),
          text: (el.textContent || '').trim(),
          value: hasValue ? String(el.value) : null,
          displayed: style.display !== 'none' && style.visibility !== 'hidden' && rect.width > 0 && rect.height > 0,
          disabled: Boolean(el.disabled || el.getAttribute('aria-disabled') === 'true'),
          checked: hasChecked ? Boolean(el.checked) : null,
          selected: hasSelected ? Boolean(el.selected) : null,
          ariaChecked: el.getAttribute('aria-checked'),
          ariaExpanded: el.getAttribute('aria-expanded'),
          ariaPressed: el.getAttribute('aria-pressed'),
          ariaSelected: el.getAttribute('aria-selected'),
        };
      });
      return { selector, count: nodes.length, unique: nodes.length === 1, elements, truncated: nodes.length > elements.length };
    `,
    [selector],
  );
  // The page sends each element's whole text, and it is cut to 500 characters
  // here, after the corpus fold (#576 W33-NF3). Cut in the page, a corpus name
  // across character 500 reached the MCP server's fold as a stub no fold
  // matches, and its start went into the tool answer and the action log.
  return { ...state, elements: state.elements.map((element) => ({ ...element, text: foldThenCut(element.text, 500) })) };
}

export async function answerFileDialog(
  browser: Browser,
  triggerTestId: string,
  kind: 'open' | 'save',
  value: string | string[] | null,
): Promise<{ answerId: string; selector: string; consumed: true }> {
  const answerId = randomUUID();
  const selector = testIdSelector(triggerTestId);
  const queued = await evalInApp<boolean>(
    browser,
    `
      const testApi = window.__E2E_TEST__;
      const hooks = testApi && testApi.fileDialogAnswers;
      if (!hooks || !Array.isArray(hooks[args[0]])) return false;
      if (hooks[args[0]].length > 0) throw new Error('a file dialog answer is already pending');
      const trigger = document.querySelector(args[4]);
      if (!trigger) throw new Error('file dialog trigger not found: ' + args[3]);
      const entry = { id: args[1], value: args[2], triggerTestId: args[3], armed: false };
      hooks[args[0]].push(entry);
      trigger.addEventListener('click', () => {
        entry.armed = true;
        testApi.activeFileDialogTriggerTestId = entry.triggerTestId;
      }, { capture: true, once: true });
      return true;
    `,
    [kind, answerId, value, triggerTestId, selector],
  );
  if (!queued) throw new Error('file dialog hooks are unavailable; start an E2E build');

  try {
    const trigger = browser.$(selector);
    await trigger.waitForDisplayed({ timeout: 5000 });
    await trigger.waitForClickable({ timeout: 5000 });
    await trigger.click();
    await browser.waitUntil(
      () =>
        evalInApp<boolean>(
          browser,
          `return window.__E2E_TEST__?.fileDialogConsumptions?.[args[0]] === args[1];`,
          [answerId, kind],
        ),
      {
        timeout: 5000,
        timeoutMsg: `file dialog answer ${answerId} was not consumed by ${triggerTestId}`,
      },
    );
    return { answerId, selector, consumed: true };
  } finally {
    await evalInApp(
      browser,
      `
        const testApi = window.__E2E_TEST__;
        const queue = testApi?.fileDialogAnswers?.[args[0]];
        const index = queue?.findIndex((entry) => entry.id === args[1]) ?? -1;
        if (index >= 0) queue.splice(index, 1);
        if (testApi?.activeFileDialogTriggerTestId === args[2]) {
          delete testApi.activeFileDialogTriggerTestId;
        }
        if (testApi?.fileDialogConsumptions) delete testApi.fileDialogConsumptions[args[1]];
      `,
      [kind, answerId, triggerTestId],
    ).catch(() => undefined);
  }
}

export async function mapClick(
  browser: Browser,
  lngLat: [number, number],
  button: 'left' | 'right' = 'left',
): Promise<MapClickResult> {
  const outcome = await evalInApp<MapClickProbe | { error: string } | null>(
    browser,
    `
      const map = window.__stalagmight_map__;
      const readInteraction = window.__stalagmight_map_interaction__;
      if (!map || !map.project) return null;
      if (typeof readInteraction !== 'function') return { error: 'map interaction hooks are unavailable' };
      const interactionSequenceBefore = readInteraction();
      const projected = map.project(args[0]);
      const canvasRoot = document.querySelector('[data-testid="map-canvas"]');
      const surface = map.getCanvas && map.getCanvas();
      if (!canvasRoot || !surface) return null;
      const rect = surface.getBoundingClientRect();
      if (projected.x < 0 || projected.y < 0 || projected.x >= rect.width || projected.y >= rect.height) {
        return {
          error: 'map point is outside the visible viewport: [' + args[0][0] + ', ' + args[0][1] + '] projects to [' +
            Math.round(projected.x) + ', ' + Math.round(projected.y) + '] for a ' +
            Math.round(rect.width) + 'x' + Math.round(rect.height) + ' map',
        };
      }
      const point = { x: Math.round(rect.left + projected.x), y: Math.round(rect.top + projected.y) };
      return {
        ...point,
        interaction: canvasRoot.dataset.placementMode === 'true' ? 'placement' : 'maplibre',
        interactionSequenceBefore,
      };
    `,
    [lngLat, button],
  );
  if (!outcome) throw new Error('map not available');
  // The page's refusal quotes the point, and a map position is where the cave
  // is: it ends a `failed:` headline, which the runners print as its length.
  if ('error' in outcome) throw new Error(`map click failed: ${outcome.error}`);
  const target = await dispatchMapPointer(browser, { x: outcome.x, y: outcome.y }, button);
  const interactionSequence = await evalInApp<number>(
    browser,
    `
      const readInteraction = window.__stalagmight_map_interaction__;
      if (typeof readInteraction !== 'function') throw new Error('map interaction hooks are unavailable');
      return readInteraction();
    `,
  );
  if (interactionSequence <= outcome.interactionSequenceBefore) {
    // The target is the page's own description of the element - its test id and
    // classes - so it goes on a later line, which the runners withhold.
    throw new Error(`map ${outcome.interaction} handler did not process the click\n  dispatched to: ${target}`);
  }
  return { x: outcome.x, y: outcome.y, interaction: outcome.interaction, interactionSequence };
}

export { cssStringLiteral, testIdSelector };
