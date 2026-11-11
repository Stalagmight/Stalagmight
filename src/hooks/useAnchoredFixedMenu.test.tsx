/**
 * Unit test (#241 P2) - useAnchoredFixedMenu's placement is computed on open and
 * would otherwise stay stale for the menu's lifetime: a menu fed async content
 * (e.g. the data-table raw-table submenu's loading -> N-row swap) would sit at
 * the on-open placement even after its natural height changes. `remeasureKey`
 * is the escape hatch: bumping it while `open` stays true must force a
 * recompute; leaving it unchanged must NOT (so a menu with stable content isn't
 * remeasured needlessly).
 *
 * `.tsx` so this runs in the jsdom tier (renderHook touches the DOM / refs).
 */
import { describe, it, expect } from 'vitest';
import { renderHook } from '@testing-library/react';

import { useAnchoredFixedMenu } from './useAnchoredFixedMenu';

function rect(partial: Partial<DOMRect>): DOMRect {
  return {
    top: 0,
    left: 0,
    right: 0,
    bottom: 0,
    width: 0,
    height: 0,
    x: 0,
    y: 0,
    toJSON: () => ({}),
    ...partial,
  } as DOMRect;
}

function setup() {
  const trigger = document.createElement('button');
  trigger.getBoundingClientRect = () => rect({ top: 100, bottom: 120, left: 100, right: 200, width: 100 });
  document.body.appendChild(trigger);

  const menu = document.createElement('div');
  document.body.appendChild(menu);

  return { triggerRef: { current: trigger }, menuRef: { current: menu }, menu };
}

describe('useAnchoredFixedMenu', () => {
  it('recomputes placement when remeasureKey changes while open (async content resize)', () => {
    const { triggerRef, menuRef, menu } = setup();

    const { result, rerender } = renderHook(
      ({ remeasureKey, naturalHeight }: { remeasureKey: string; naturalHeight: number }) => {
        menu.getBoundingClientRect = () => rect({ height: naturalHeight });
        return useAnchoredFixedMenu(true, triggerRef, menuRef, {
          align: 'center',
          gap: 4,
          width: 208,
          remeasureKey,
        });
      },
      { initialProps: { remeasureKey: 'loading', naturalHeight: 20 } }
    );

    const beforeMaxHeight = result.current.maxHeight;

    // Same remeasureKey, content grows: the hook must NOT re-place (documents
    // the on-open-only baseline the escape hatch overrides).
    rerender({ remeasureKey: 'loading', naturalHeight: 300 });
    expect(result.current.maxHeight).toBe(beforeMaxHeight);

    // remeasureKey changes (the loading -> N-row swap signal): must re-place
    // to reflect the new natural height.
    rerender({ remeasureKey: 'row-a|row-b|row-c', naturalHeight: 300 });
    expect(result.current.maxHeight).not.toBe(beforeMaxHeight);
    expect(result.current.maxHeight).toBe(300);
  });

  it('remeasures past a stale max-height clip using scrollHeight, not just the clipped rect', () => {
    const { triggerRef, menuRef, menu } = setup();

    const { result, rerender } = renderHook(
      ({ remeasureKey, rectHeight, scrollHeight }: { remeasureKey: string; rectHeight: number; scrollHeight: number }) => {
        menu.getBoundingClientRect = () => rect({ height: rectHeight });
        Object.defineProperty(menu, 'scrollHeight', { value: scrollHeight, configurable: true });
        return useAnchoredFixedMenu(true, triggerRef, menuRef, {
          align: 'center',
          gap: 4,
          width: 208,
          remeasureKey,
        });
      },
      { initialProps: { remeasureKey: 'loading', rectHeight: 20, scrollHeight: 20 } }
    );

    // First placement clips to MIN_MENU_HEIGHT (64).
    expect(result.current.maxHeight).toBe(64);

    // Remeasure: the menu is still carrying the PRIOR placement's max-height clip
    // (+ overflow-auto), so getBoundingClientRect reports back the clipped 64px box
    // even though the content grew to 300px (scrollHeight). Placement must grow.
    rerender({ remeasureKey: 'row-a|row-b|row-c', rectHeight: 64, scrollHeight: 300 });
    expect(result.current.maxHeight).toBe(300);
  });

  it('grows back past a stale clip when the scrolling list is INSIDE the menu (scrollRef)', () => {
    // A fixed header (40px) above an overflow-auto list: under the prior cap the
    // menu itself does not overflow (its scrollHeight equals its clipped box), the
    // inner list does. Without scrollRef the remeasure reads 97px and stays there.
    const { triggerRef, menuRef, menu } = setup();
    const list = document.createElement('div');
    menu.appendChild(list);
    const scrollRef = { current: list };

    const { result, rerender } = renderHook(
      ({ remeasureKey, listContent }: { remeasureKey: string; listContent: number }) => {
        const clip = 97;
        const header = 40;
        const listBox = Math.min(listContent, clip - header);
        menu.getBoundingClientRect = () => rect({ height: header + listBox });
        Object.defineProperty(menu, 'scrollHeight', { value: header + listBox, configurable: true });
        Object.defineProperty(list, 'clientHeight', { value: listBox, configurable: true });
        Object.defineProperty(list, 'scrollHeight', { value: listContent, configurable: true });
        return useAnchoredFixedMenu(true, triggerRef, menuRef, {
          align: 'center',
          gap: 4,
          width: 208,
          maxHeightCap: 420,
          remeasureKey,
          scrollRef,
        });
      },
      { initialProps: { remeasureKey: 'few', listContent: 57 } }
    );
    expect(result.current.maxHeight).toBe(97);

    // The search is cleared: the list holds 600px of rows again.
    rerender({ remeasureKey: 'all', listContent: 600 });
    expect(result.current.maxHeight).toBe(420);
  });

  it('leaves placement stable across re-renders when remeasureKey is omitted', () => {
    const { triggerRef, menuRef, menu } = setup();

    const { result, rerender } = renderHook(
      ({ naturalHeight }: { naturalHeight: number }) => {
        menu.getBoundingClientRect = () => rect({ height: naturalHeight });
        return useAnchoredFixedMenu(true, triggerRef, menuRef, {
          align: 'center',
          gap: 4,
          width: 208,
        });
      },
      { initialProps: { naturalHeight: 20 } }
    );

    const before = result.current.maxHeight;
    rerender({ naturalHeight: 300 });
    expect(result.current.maxHeight).toBe(before);
  });
});
