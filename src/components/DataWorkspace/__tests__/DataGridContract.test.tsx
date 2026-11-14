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
import { act, cleanup, render, screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import type { SpreadsheetGridProps } from '../../common/SpreadsheetGrid';
import type { DataColumnKey, DataRow } from '../types';

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
import { DATA_COLUMNS } from '../columnsMeta';
import { constrainDataColumnOrder } from '../model/columnGroups';
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
const renderPart = (node: ReactNode) => {
  const { container } = render(<div>{node}</div>);
  return { getByTestId: (id: string) => within(container).getByTestId(id) as HTMLElement };
};

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
    const band = renderPart(grid().groupRowContent!(grid().rows[i]!, i)).getByTestId('data-section-header-2');
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
    const el = renderPart(corner(rowIndex('sh:3:302'))).getByTestId('data-section-corner-3');
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
    expect(renderPart(grid().headerCorner!(0)).getByTestId('data-section-corner-1')).toHaveTextContent('Main#1');
  });
});

describe("station link chips span the shot-only columns", () => {
  const span = (key: string) => grid().spanCells!(grid().rows[rowIndex(key)]!, rowIndex(key));
  const renderSpan = (key: string) => renderPart(span(key)!.content);

  it('spans the shown shot-only block on a station with links; nothing on shots or bare stations', () => {
    render(<DataWorkspace />);
    expect(span('st:1:4')).toMatchObject({ from: 'length', to: 'kind' });
    expect(span('sh:1:102')).toBeUndefined();
    expect(span('st:1:3')).toBeUndefined();
  });

  it('renders the chips with the station name bold and the misclosure inside', () => {
    render(<DataWorkspace />);
    const part = renderSpan('st:1:4');
    const closure = part.getByTestId('data-chip-closure-st-1-4-7-3-3');
    expect(closure).toHaveTextContent('Closes to Downstre…sump 1A:3·Δ 0.7 m · 1.3 %');
    expect(within(closure).getByText('Downstre…sump 1A:3')).toHaveClass('font-semibold');
    expect(within(closure).getByText(/^Closes to/).closest('.font-semibold')).toBeNull();
    expect(part.getByTestId('data-chip-misclosure-st-1-4-7')).toHaveTextContent('Δ 0.7 m · 1.3 %');
    expect(renderSpan('st:1:0').getByTestId('data-chip-zero-st-1-0')).toHaveTextContent('Zero datum');
    const branch = renderSpan('st:2:0').getByTestId('data-chip-branch-st-2-0-1-2');
    expect(branch).toHaveTextContent('Continues from Main:2');
    expect(within(branch).getByText('Main:2')).toHaveClass('font-semibold');
  });

  it('a chip jumps to the partner row, revealing it when the filters hide it', () => {
    useDataWorkspaceStore.setState({ collapsedSections: [3] });
    render(<DataWorkspace />);
    renderSpan('st:1:2').getByTestId('data-chip-merge-st-1-2-2-0').click();
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'st:2:0', align: 'center' });
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual([]);
    renderSpan('st:1:4').getByTestId('data-chip-closure-st-1-4-7-3-3').click();
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual(['st:3:3']);
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'st:3:3' });
  });

  it('a find match in the links outlines the chip it falls in', () => {
    useDataWorkspaceStore.setState({ search: { ...useDataWorkspaceStore.getState().search, open: true, query: 'branch b:0' } });
    render(<DataWorkspace />);
    const chip = renderSpan('st:1:2').getByTestId('data-chip-merge-st-1-2-2-0');
    expect(chip).toHaveClass('dw-match-outline', 'dw-match-outline-current');
    expect(renderSpan('st:1:4').getByTestId('data-chip-closure-st-1-4-7-3-3')).not.toHaveClass('dw-match-outline');
  });

  it('with no shot-only column shown, the gutter counts the links instead', () => {
    const shotOnly: DataColumnKey[] = ['length', 'azimuth', 'depth_in', 'depth_out', 'left', 'right', 'up', 'down', 'kind'];
    useDataWorkspaceStore.setState({ hiddenColumns: [...useDataWorkspaceStore.getState().hiddenColumns, ...shotOnly] });
    render(<DataWorkspace />);
    expect(span('st:1:4')).toBeUndefined();
    const linksOf = (part: string) => screen.getByTestId(['data', 'row', 'links', part].join('-'));
    const closure = linksOf('st-1-4');
    expect(closure).toHaveTextContent('1');
    expect(closure).toHaveAttribute('title', 'Closes to Downstre…sump 1A:3 · Δ 0.7 m · 1.3 %');
    closure.click();
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'st:3:3' });
    // A zero datum alone goes nowhere: no button.
    expect(linksOf('st-1-0').tagName).toBe('SPAN');
    expect(screen.queryByTestId(['data', 'row', 'links', 'st-1-3'].join('-'))).toBeNull();
  });

  it('with a shot-only column shown, the gutter does not count them', () => {
    render(<DataWorkspace />);
    expect(screen.queryByTestId(['data', 'row', 'links', 'st-1-4'].join('-'))).toBeNull();
  });
});

describe('column order: the shot-only columns stay one block', () => {
  it('hands the grid the constraint, and applies it to a reorder too', () => {
    render(<DataWorkspace />);
    expect(grid().constrainColumnOrder).toBe(constrainDataColumnOrder);
    // A drag the grid would report: Comment dropped between Length and Azimuth.
    const order: DataColumnKey[] = DATA_COLUMNS.map((c) => c.key).filter((k) => k !== 'comment');
    order.splice(order.indexOf('azimuth'), 0, 'comment');
    act(() => grid().onColumnOrderChange!(order));
    const stored = useDataWorkspaceStore.getState().columnOrder;
    // Two shot columns before it, many after: the start edge.
    expect(stored.indexOf('comment')).toBe(stored.indexOf('fromTo') - 1);
  });
});
