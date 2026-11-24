/**
 * RS-FX6, P3-17 (shots 01, 24, 40 of the RS capture): the Sections panel
 * drew a line's colour dot with no edge, so a near-white line vanished on
 * the light theme and a near-black one on the dark theme, while its pills
 * kept a ring. The dot takes the pills' ring rule (`panelDotOf`), drawn as a
 * border; any other colour stays bare. Injection: drop the ring.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => Array.from({ length: count }, (_, index) => ({ index, key: index, start: index * 40, size: 40 })),
    getTotalSize: () => count * 40,
    scrollToIndex: vi.fn(),
    measureElement: vi.fn(),
  }),
}));
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn().mockResolvedValue(undefined), getErrorMessage: String }));

import '../../../i18n';
import SectionsPanel from '../SectionsPanel';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { panelDotOf } from '../../../theme/pillColors';
import type { Line } from '../../../types/ipc/Line';

const lines = [
  { id: 1, name: 'Main', color: '#000080', shots: [] },
  { id: 2, name: 'Side', color: '#f4f4f2', shots: [] },
  { id: 3, name: 'Loop', color: '#141414', shots: [] },
] as unknown as Line[];

function renderOn(theme: 'light' | 'dark') {
  useProjectStore.setState({ project: { created_at: 'p1', lines, cave_sections: [] } as never, surveyData: null });
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    selection: null,
    multiSelection: [],
    hiddenSections: [],
    collapsedSections: {},
    resolvedTheme: theme,
  });
  useSpeleoStore.setState({ projectLinkKind: 'local' } as never);
  render(<SectionsPanel lines={lines} />);
}

describe('panelDotOf', () => {
  it('rings a near-white dot on the light theme and a near-black one on the dark theme, nothing else', () => {
    expect(panelDotOf('#f4f4f2', 'light').ring).not.toBeNull();
    expect(panelDotOf('#141414', 'dark').ring).not.toBeNull();
    // A colour that stands out on the panel stays bare.
    expect(panelDotOf('#000080', 'light').ring).toBeNull();
    expect(panelDotOf('#ffd700', 'dark').ring).toBeNull();
    expect(panelDotOf('#141414', 'light').ring).toBeNull();
    expect(panelDotOf('#f4f4f2', 'dark').ring).toBeNull();
  });
});

describe('the Sections panel dot', () => {
  it('draws the ring as a border on the light theme', () => {
    renderOn('light');
    const ring = panelDotOf('#f4f4f2', 'light').ring!;
    expect(screen.getByTestId('line-color-dot-2').style.border).toContain('1px solid');
    const rgb = [1, 3, 5].map((i) => parseInt(ring.slice(i, i + 2), 16)).join(', ');
    expect(screen.getByTestId('line-color-dot-2').style.border).toBe(`1px solid rgb(${rgb})`);
    expect(screen.getByTestId('line-color-dot-1').style.border).toBe('');
  });

  it('rings the near-black dot on the dark theme', () => {
    renderOn('dark');
    expect(screen.getByTestId('line-color-dot-3').style.border).toContain('1px solid');
    expect(screen.getByTestId('line-color-dot-2').style.border).toBe('');
  });
});
