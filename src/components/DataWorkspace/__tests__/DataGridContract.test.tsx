/**
 * What the Data workspace hands the SpreadsheetGrid through its layout props:
 * section headers starting in the gutter, the header corner naming the
 * section whose header scrolled away, the station link chips spanning the
 * shot-only columns, and the column-order constraint.
 *
 * The props are captured (the real grid still renders), and their content is
 * rendered on its own here, so these tests hold whether or not the grid draws
 * it yet.
 */
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import type { SpreadsheetGridProps } from '../../common/SpreadsheetGrid';
import type { DataRow } from '../types';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(() => Promise.resolve(undefined)) }));
vi.mock('../../../hooks/useEditingBlocked', () => ({
  useEditingBlocked: () => false,
  isEditingBlockedNow: () => false,
}));

const captured = vi.hoisted(() => ({ props: null as unknown }));
vi.mock('../../common/SpreadsheetGrid', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../common/SpreadsheetGrid')>();
  const Real = actual.SpreadsheetGrid;
  return {
    ...actual,
    SpreadsheetGrid: (props: SpreadsheetGridProps<DataRow>) => {
      captured.props = props;
      return <Real {...props} />;
    },
  };
});

import '../../../i18n';
import { DataWorkspace } from '../DataWorkspace';
import { useDataWorkspaceStore } from '../store';
import { resetDataWorkspaceViewPersistenceForTests } from '../useDataWorkspaceViewPersistence';
import { GUTTER_RAIL_LANE } from '../DataRowLabel';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig, type Config } from '../../../types/config';
import { makeData, makeProject, WARNINGS } from './dataFixtures';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  resetDataWorkspaceViewPersistenceForTests();
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    project: makeProject(),
    surveyData: makeData(),
    surveyDataAdjusted: null,
    warnings: WARNINGS,
    frozenShots: [{ section_id: 3, shot_index: 0 }],
  } as never);
  useUIStore.setState({ toasts: [], geometryMode: 'measured', selection: null, multiSelection: [], appMode: 'data' });
  useDataWorkspaceStore.setState({ ...useDataWorkspaceStore.getInitialState(), showSectionHeaders: true });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const grid = () => captured.props as SpreadsheetGridProps<DataRow>;
const rowIndex = (key: string) => grid().rows.findIndex((r) => r.key === key);
/** Render a piece of the grid's content (a corner, a span) on its own. */
const renderPart = (node: ReactNode) => render(<div data-testid="part">{node}</div>);

describe('section header rows start in the gutter', () => {
  it('asks the grid to start the header content right after the rail', () => {
    render(<DataWorkspace />);
    expect(grid().groupContentStart).toBe('gutter');
    expect(grid().groupContentInset).toBe(GUTTER_RAIL_LANE);
  });

  it("the header row's gutter holds only the rail; the header names the section", () => {
    render(<DataWorkspace />);
    const i = rowIndex('sec:2');
    const gutter = screen.getByTestId(`data-grid-gutter-${i}`);
    expect(gutter.querySelector('.dw-rail-elbow')).not.toBeNull();
    expect(gutter.textContent).toBe('');
    const header = renderPart(grid().groupRowContent!(grid().rows[i]!, i));
    const band = within(header.getByTestId('part')).getByTestId('data-section-header-2');
    // Chevron, dot, full name, #id, counts and the "continues from" chip.
    expect(band).toHaveTextContent('Branch B#2');
    expect(within(band).getByTestId('data-section-toggle-2')).toBeInTheDocument();
    expect(within(band).getByTestId('data-section-counts-2')).toHaveTextContent('3 stations · 2 shots');
    expect(within(band).getByTestId('data-section-continues-2')).toHaveTextContent('Main:2');
  });
});

describe('header corner', () => {
  it('names the section of the top row once its header scrolled away', () => {
    render(<DataWorkspace />);
    const corner = grid().headerCorner!;
    // The header row itself is on top: nothing to repeat.
    expect(corner(rowIndex('sec:1'))).toBeNull();
    expect(corner(rowIndex('sec:2'))).toBeNull();
    // Scrolled into section 3: its dot, full name and #id.
    const part = renderPart(corner(rowIndex('sh:3:302')));
    const el = within(part.getByTestId('part')).getByTestId('data-section-corner-3');
    expect(el).toHaveTextContent('Downstream sump 1A#3');
    expect(el).toHaveAttribute('title', 'Downstream sump 1A #3');
    expect(corner(rowIndex('st:1:0'))).not.toBeNull();
  });

  it('shows nothing without rows, and nothing while sorted (rows mix sections)', () => {
    render(<DataWorkspace />);
    expect(grid().headerCorner!(-1)).toBeNull();
    cleanup();
    useDataWorkspaceStore.setState({ sort: { colKey: 'length', dir: 'asc' } });
    render(<DataWorkspace />);
    expect(grid().headerCorner!(0)).toBeNull();
    expect(grid().headerCorner!(5)).toBeNull();
  });

  it('with section headers off, names the section of the top row', () => {
    useDataWorkspaceStore.setState({ showSectionHeaders: false });
    render(<DataWorkspace />);
    const part = renderPart(grid().headerCorner!(0));
    expect(within(part.getByTestId('part')).getByTestId('data-section-corner-1')).toHaveTextContent('Main#1');
  });
});
