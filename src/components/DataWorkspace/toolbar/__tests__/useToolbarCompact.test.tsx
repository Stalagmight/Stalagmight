/**
 * The toolbar drops its menu labels only when the labelled row, with the find
 * box at its narrowest, does not fit (as measured), and brings them back once
 * it would - also when the find box's own content shrinks while compact.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import '../../../../i18n';

vi.mock('../../../../hooks/useEditingBlocked', () => ({
  isEditingBlockedNow: () => false,
  useEditingBlocked: () => false,
}));

import { Harness, makeModel, resetStore } from './harness';

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

/**
 * Geometry: the find column is `findWidth` wide and its input 140 px (no CSS in
 * jsdom, so no min-width: all 140 px are slack) - at its narrowest it is
 * `findWidth - 140`. Every other child (row type, five menus, map) is 100 px
 * labelled and 40 px compact. With findWidth 300: labelled need 160 + 700 =
 * 860, compact need 160 + 280 = 440 (the labels take 420).
 */
let rowWidth = 1200;
let findWidth = 300;

beforeEach(() => {
  resetStore();
  observers.length = 0;
  rowWidth = 1200;
  findWidth = 300;
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.dataset.testid === 'data-toolbar-row') return rect(rowWidth);
    if (this.dataset.testid === 'data-find-input') return rect(140);
    if (this.parentElement?.dataset.testid === 'data-toolbar-row') {
      if (this.dataset.toolbarShrink !== undefined) return rect(findWidth);
      const compact = screen.getByTestId('data-toolbar').getAttribute('data-compact') === 'true';
      return rect(compact ? 40 : 100);
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
  it('keep their labels while the labelled row fits with the find box at its narrowest', () => {
    rowWidth = 860;
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(true);
    expect(screen.getByTestId('data-sections-button')).toHaveTextContent('Sections');
  });

  it('drop to icons once it does not, keeping the label as the accessible name and tooltip', () => {
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(true);
    rowWidth = 850;
    resize();
    expect(labelled()).toBe(false);
    const sections = screen.getByTestId('data-sections-button');
    expect(sections).not.toHaveTextContent('Sections');
    expect(sections).toHaveAttribute('aria-label', 'Sections');
    expect(sections).toHaveAttribute('title');
    expect(screen.getByTestId('data-map-toggle')).toHaveAttribute('aria-label', 'Map');
  });

  it('come back only once the row is as wide as the labelled row needs', () => {
    rowWidth = 850;
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(false);
    rowWidth = 855;
    resize();
    expect(labelled()).toBe(false);
    rowWidth = 860;
    resize();
    expect(labelled()).toBe(true);
  });

  it('come back when the find box content shrinks (a match count goes away)', () => {
    rowWidth = 850;
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(false);
    findWidth = 290; // narrowest 150: 150 + 280 + 420 = 850
    resize();
    expect(labelled()).toBe(true);
  });

  it('learn the labels from the flip, not from a find box that changed since', () => {
    // A wide count makes the labelled row overflow at 900...
    rowWidth = 900;
    findWidth = 380; // narrowest 240: 240 + 700 = 940 > 900
    render(<Harness model={makeModel()} />);
    expect(labelled()).toBe(false);
    // ...then it goes away while compact: 160 + 280 + 420 = 860 fits.
    findWidth = 300;
    resize();
    expect(labelled()).toBe(true);
  });
});
