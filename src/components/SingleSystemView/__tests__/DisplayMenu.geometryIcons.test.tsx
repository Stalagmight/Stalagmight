/**
 * Component test (#408): the padlock glyphs on the Geometry pills.
 *
 * The rename to "Raw Data View" / "Closures Computed View" dropped the only two
 * words that said which mode is editable, so the pills carry the meaning in a
 * glyph instead: an OPEN padlock on the raw (editable) mode, a CLOSED one on the
 * read-only closure solve. Wired backwards, both pills still render, both still
 * carry their test IDs, and every existing spec still passes - which is exactly
 * why this file pins the icon-to-mode mapping directly.
 *
 * The pins are made against REFERENCE icons rendered in the same tree by the same
 * React path, so the comparison is on the actual `LockOpenIcon` / `LockIcon`
 * output rather than on a hand-copied path string that would rot the moment the
 * SVG is retouched. Swapping the two `icon={...}` props in `DisplayMenu.tsx`
 * fails both mapping assertions.
 *
 * The glyphs are gated on the syntactic-highlighting level (`icons`+), like every
 * other icon in this menu, so the level-gate is pinned too.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// `t` is not under test here (the labels are pinned by the i18n/e2e tiers); the
// identity mapping is. Returning the key keeps this file independent of copy.
// `initReactI18next` is stubbed too: `projectStore` transitively imports
// `src/i18n/index.ts`, which installs it at module load.
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).translationStub());

import DisplayMenu from '../DisplayMenu';
import { LockIcon, LockOpenIcon } from '../../common/icons';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import type { SyntacticHighlightLevel } from '../../../types/config';

/** Reference glyphs, rendered beside the menu so both go through one React path. */
function Harness() {
  return (
    <>
      <div data-testid="ref-lock-open">
        <LockOpenIcon size={14} />
      </div>
      <div data-testid="ref-lock-closed">
        <LockIcon size={14} />
      </div>
      <DisplayMenu />
    </>
  );
}

const svgOf = (testId: string): SVGSVGElement | null =>
  screen.getByTestId(testId).querySelector('svg');

/** The glyph's drawn content - the part that differs between open and closed. */
const glyphOf = (testId: string): string => {
  const svg = svgOf(testId);
  expect(svg, `${testId} renders an svg`).not.toBeNull();
  return svg!.innerHTML;
};

async function openMenuAt(level: SyntacticHighlightLevel) {
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', syntacticLevel: level });
  render(<Harness />);
  await userEvent.click(screen.getByTestId('display-menu-button'));
  expect(screen.getByTestId('display-menu')).toBeInTheDocument();
}

beforeEach(() => {
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    syntacticLevel: 'full',
    // The grid-spacing <select> is the menu's only other conditional subtree;
    // it is irrelevant here and its <option> labels want a live i18n formatter.
    showGrid: false,
  });
  useProjectStore.setState({ project: null });
});

describe('DisplayMenu geometry pills (#408)', () => {
  it('gives the raw mode the OPEN padlock and the closure mode the CLOSED one', async () => {
    await openMenuAt('icons');

    const open = glyphOf('ref-lock-open');
    const closed = glyphOf('ref-lock-closed');
    // Guard the guard: if the two icons ever render identically, every mapping
    // assertion below would pass vacuously.
    expect(open).not.toBe(closed);

    expect(glyphOf('geometry-mode-measured')).toBe(open);
    expect(glyphOf('geometry-mode-closureAdjusted')).toBe(closed);
  });

  it('still maps the glyphs correctly at the full level', async () => {
    await openMenuAt('full');

    expect(glyphOf('geometry-mode-measured')).toBe(glyphOf('ref-lock-open'));
    expect(glyphOf('geometry-mode-closureAdjusted')).toBe(glyphOf('ref-lock-closed'));
  });

  it('renders no padlock on either pill below the icons level', async () => {
    await openMenuAt('toolbar_only');

    // Both pills are still there - it is the glyph that is gated, not the control.
    expect(screen.getByTestId('geometry-mode-measured')).toBeInTheDocument();
    expect(screen.getByTestId('geometry-mode-closureAdjusted')).toBeInTheDocument();
    expect(svgOf('geometry-mode-measured')).toBeNull();
    expect(svgOf('geometry-mode-closureAdjusted')).toBeNull();
  });
});
