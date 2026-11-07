/**
 * [#576 NF-W7E-3] The warnings list swaps a warning's OWN shot reference for the
 * shot pill - decided from the warning's structured fields, not its text.
 *
 * At the Full syntactic level the list replaces the leading "Shot <section>:<n>"
 * of a warning's message with a pill for the shot. It found that reference with
 * a regex over the English text (`/^Shot \d+:\d+\s*` + `/`), so any leading shot
 * reference was cut - also one naming ANOTHER shot, whose text then vanished
 * behind a pill for the warning's own shot. The reference is now built from the
 * warning's `sectionId` and `shotIndex`, and only that one is replaced.
 *
 * `.tsx` so vitest runs it under jsdom. All data is synthetic.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: index * 56, size: 56 })),
    getTotalSize: () => count * 56,
    measureElement: () => {},
    scrollToIndex: vi.fn(),
  }),
}));

import DetailPanel from '../DetailPanel';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { warningMessageTestId, warningOpenShotTestId } from '../../../utils/testId';
import type { ProjectWarning } from '../../../types/ipc/ProjectWarning';
import type { Project } from '../../../types/project';

const warning = (message: string, shotIndex: number, shotId: number): ProjectWarning => ({
  sectionId: 1,
  shotId,
  shotIndex,
  code: 'TEST_CODE',
  message,
  severity: 'warning',
  isResolved: false,
});

function renderWarnings(warnings: ProjectWarning[]) {
  useProjectStore.setState({
    project: { name: 'CaveA', created_at: '2026-01-01T00:00:00Z', sections: [{ id: 1, name: 'SectionA', shots: [] }] } as unknown as Project,
    stations: [],
    warnings,
  });
  render(<DetailPanel />);
}

beforeEach(() => {
  useUIStore.setState({ detailPanelTab: 'warnings', syntacticLevel: 'full', selection: null });
});

describe("the warnings list's shot pill (#576 NF-W7E-3)", () => {
  it("replaces the warning's own reference with its pill", () => {
    const own = warning('Shot 1:1 has zero length', 0, 10);
    renderWarnings([own]);
    expect(screen.getByTestId(warningOpenShotTestId(own))).toHaveTextContent('1:1');
    expect(screen.getByTestId(warningMessageTestId(own))).toHaveTextContent(/^1:1 has zero length$/);
  });

  it('keeps a leading reference to ANOTHER shot as text', () => {
    const other = warning('Shot 2:3 has zero length', 0, 11);
    renderWarnings([other]);
    const text = screen.getByTestId(warningMessageTestId(other)).textContent;
    console.log(`[NF-W7E-3] warning on shot 1:1 whose text names shot 2:3 renders as: "${text}"`);
    expect(text).toContain('Shot 2:3 has zero length');
    expect(screen.queryByTestId(warningOpenShotTestId(other))).toBeNull();
  });

  it('does not take a longer number for its own (1:1 is not the start of 1:12)', () => {
    const longer = warning('Shot 1:12 length 3.0m above maximum threshold 2.0m', 0, 12);
    renderWarnings([longer]);
    const text = screen.getByTestId(warningMessageTestId(longer)).textContent;
    console.log(`[NF-W7E-3] warning on shot 1:1 whose text starts "Shot 1:12" renders as: "${text}"`);
    expect(text).toContain('Shot 1:12 length 3.0m');
  });
});
