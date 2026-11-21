import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import '../../../i18n';
import { toolbarCondenseLayout } from '../Toolbar';
import { ModeCarousel } from '../ModeCarousel';

/**
 * The Toolbar's progressive condensation, at the two points jsdom can actually
 * speak to: the pure stage -> policy mapping, and the mode carousel's own box.
 *
 * The rect SWEEP is an e2e (toolbar-condense.spec.ts) - jsdom measures every box
 * as 0x0, so it cannot see an overlap. What it can pin is why the final stage used
 * to overlap anyway: a flex item's box is not bounded by its parent's width, so a
 * carousel in a shrunk slot kept its full intrinsic width and was laid out across
 * the tool cluster (persistently, at 540 CSS px and below - reachable by the UI
 * zoom alone). `max-w-full` is what makes "flexbox lays it out BESIDE the clusters"
 * actually true.
 */

describe('toolbarCondenseLayout', () => {
  it('is cumulative: every stage keeps everything the earlier ones dropped', () => {
    const stages = [0, 1, 2, 3, 4, 5, 6].map(toolbarCondenseLayout);
    for (const [i, s] of stages.entries()) {
      if (s.historyCondensed) expect(s.importCondensed, `stage ${i}`).toBe(true);
      if (s.importCondensed) expect(s.displayCondensed, `stage ${i}`).toBe(true);
      if (s.tight) expect(s.historyCondensed, `stage ${i}`).toBe(true);
      if (s.centerInFlow) expect(s.tight, `stage ${i}`).toBe(true);
    }
  });

  it('drops each label at its own stage and nothing before it', () => {
    expect(toolbarCondenseLayout(0)).toMatchObject({
      displayCondensed: false,
      importCondensed: false,
      historyCondensed: false,
      tight: false,
      centerInFlow: false,
    });
    expect(toolbarCondenseLayout(1).displayCondensed).toBe(true);
    expect(toolbarCondenseLayout(1).importCondensed).toBe(false);
    expect(toolbarCondenseLayout(2).importCondensed).toBe(true);
    expect(toolbarCondenseLayout(2).historyCondensed).toBe(false);
    expect(toolbarCondenseLayout(3).historyCondensed).toBe(true);
    expect(toolbarCondenseLayout(3).tight).toBe(false);
    expect(toolbarCondenseLayout(4).tight).toBe(true);
    expect(toolbarCondenseLayout(4).centerInFlow).toBe(false);
    expect(toolbarCondenseLayout(5).centerInFlow).toBe(true);
  });

  it('lets the carousel slot - and only the carousel slot - give up width', () => {
    // In flow the slot shrinks, so the clusters reclaim width before they start
    // running past the header's edge...
    const inFlow = toolbarCondenseLayout(5);
    expect(inFlow.centerCls).toContain('shrink');
    expect(inFlow.centerCls).not.toContain('absolute');
    // ...but it keeps a floor, and specifically NOT `min-w-0`. The carousel caps its
    // box at the slot, so a slot that may reach 0 renders a zero-width carousel -
    // the mode switcher gone, not merely cramped, and it is the only way into
    // Data or Plan.
    expect(inFlow.centerCls).not.toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(inFlow.centerCls).toMatch(/min-w-\[/);

    // Out of flow it is positioned, not laid out.
    expect(toolbarCondenseLayout(0).centerCls).toContain('absolute');
    expect(toolbarCondenseLayout(0).centerCls).not.toContain('shrink');
  });

  it('never makes a cluster shrinkable or a clipping context', () => {
    // Both clusters own absolutely-positioned popovers anchored to their own buttons
    // (DisplayMenu's panel, the tool/tool-group flyouts). A shrinkable cluster with
    // its own overflow - or an `overflow-hidden` header - would clip those menus to
    // the toolbar's height, which is why the surplus is left to the carousel slot.
    for (const condense of [0, 1, 2, 3, 4, 5]) {
      const { clusterCls } = toolbarCondenseLayout(condense);
      expect(clusterCls, `stage ${condense}`).toContain('flex-shrink-0');
      expect(clusterCls, `stage ${condense}`).not.toContain('overflow');
      expect(clusterCls, `stage ${condense}`).not.toMatch(/(^|\s)shrink(\s|$)/);
    }
  });
});

describe('ModeCarousel box', () => {
  it('caps its own width at its slot and scrolls rather than clipping', () => {
    render(<ModeCarousel compact />);
    const group = screen.getByTestId('mode-carousel');
    // The cap is what stops the box being laid out across the tool cluster.
    expect(group.className).toContain('max-w-full');
    // Scrolls, never clips: it is the only way between Survey, Data and Plan (no
    // menu item, no shortcut), so an eaten mode is an unreachable workspace.
    expect(group.className).toContain('overflow-x-auto');
    expect(group.className).not.toMatch(/(^|\s)overflow-hidden(\s|$)/);
  });

  it('keeps every mode present and named in the compact variant', () => {
    render(<ModeCarousel compact />);
    for (const mode of ['survey', 'data', 'plan']) {
      const button = screen.getByTestId(`mode-${mode}`);
      expect(button).toHaveAccessibleName(/.+/);
      // The buttons must scroll, not squash into unreadable slivers.
      expect(button.className).toContain('shrink-0');
    }
    // Export left the carousel: it opens from the project menu / Ctrl+E.
    expect(screen.queryByTestId('mode-export')).toBeNull();
  });
});
