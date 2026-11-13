/**
 * The toolbar drops its menu labels only when the labelled row overflows (as
 * measured), and brings them back once the row is as wide as the labelled row
 * needed - not at a fixed breakpoint.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import '../../../../i18n';

vi.mock('../../../../hooks/useEditingBlocked', () => ({
  isEditingBlockedNow: () => false,
  useEditingBlocked: () => false,
}));

import { Harness, makeModel, resetStore } from './harness';
import { nextCompact } from '../useToolbarCompact';

const observers: FakeResizeObserver[] = [];
class FakeResizeObserver {
  constructor(private cb: () => void) {
    observers.push(this);
  }
  observe() {}
  unobserve() {}
  disconnect() {
    const i = observers.indexOf(this);
    if (i >= 0) observers.splice(i, 1);
  }
  fire() {
    this.cb();
  }
}
const resize = () =>
  act(() => {
    for (const o of [...observers]) o.fire();
  });

const rect = (width: number) => ({ left: 0, top: 0, right: width, bottom: 28, width, height: 28, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;

/** The row is `rowWidth` wide; its content ends at 950 px with labels, 700 px without. */
let rowWidth = 1200;
const LABELLED = 950;
const ICONS = 700;

beforeEach(() => {
  resetStore();
  observers.length = 0;
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.dataset.testid === 'data-toolbar-row') return rect(rowWidth);
    if (this.parentElement?.dataset.testid === 'data-toolbar-row') {
      const compact = screen.getByTestId('data-toolbar').getAttribute('data-compact') === 'true';
      return rect(compact ? ICONS : LABELLED);
    }
    return rect(0);
  });
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const labelled = () => screen.getByTestId('data-toolbar').getAttribute('data-compact') !== 'true';

describe('Data toolbar labels', () => {
  it('keep their labels while the labelled row fits, at any width', () => {
    rowWidth = 960;
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(true);
    expect(screen.getByTestId('data-sections-button')).toHaveTextContent('Sections');
  });

  it('drop to icons once the row overflows, keeping the label as the accessible name and tooltip', () => {
    rowWidth = 1200;
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(true);
    rowWidth = 900;
    resize();
    expect(labelled()).toBe(false);
    const sections = screen.getByTestId('data-sections-button');
    expect(sections).not.toHaveTextContent('Sections');
    expect(sections).toHaveAttribute('aria-label', 'Sections');
    expect(sections).toHaveAttribute('title');
    expect(screen.getByTestId('data-map-toggle')).toHaveAttribute('aria-label', 'Map');
  });

  it('come back only once the row is as wide as the labelled row needed', () => {
    rowWidth = 900;
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(false);
    rowWidth = 940;
    resize();
    expect(labelled()).toBe(false);
    rowWidth = LABELLED;
    resize();
    expect(labelled()).toBe(true);
  });

  it('nextCompact ignores an unlaid-out row', () => {
    expect(nextCompact(false, 0, 999)).toBe(false);
    expect(nextCompact(true, 0, 1)).toBe(true);
    expect(nextCompact(false, 900, 950)).toBe(true);
    expect(nextCompact(true, 950, 950)).toBe(false);
  });
});
