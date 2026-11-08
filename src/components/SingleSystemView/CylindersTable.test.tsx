/**
 * Component test (#403) - the Cylinders tab's tank list, now hosted on SpreadsheetGrid.
 *
 * The pure rules (usable clamp, role parsing, boolean tokens, no-op identity) are pinned in
 * `__tests__/cylindersGridEdits.test.ts`; what is covered here is the wiring only the real
 * component can show: which MODE the usable column is in per row, that the grid's gestures
 * reach the reducer with the right units, that a non-budget row is visibly dimmed, and that
 * the embedded role Select opens from the keyboard.
 */
import { describe, it, expect, beforeEach, beforeAll, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

// The grid is virtualized and jsdom gives the virtualizer no layout to measure, so without
// this mock it renders zero rows and every cell query fails - the same shim the grid's own
// component tests use.
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: 33 + index * 33,
        size: 33,
      })),
    getTotalSize: () => count * 33,
    scrollToIndex: () => {},
  }),
}));

import { CylindersTable } from './CylindersTable';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { defaultConfig } from '../../types/config';
import type { Cylinder, GasReachField } from '../../types/ipc.generated';

function tank(over: Partial<Cylinder> = {}): Cylinder {
  return {
    water_volume_l: 11.1,
    rated_pressure_bar: 207,
    fill_bar: 207,
    material: 'aluminium',
    role: 'back_gas',
    label: 'AL80',
    enabled: true,
    usable_gas_bar: 0,
    ...over,
  };
}

function fakeField(overrides: Partial<GasReachField> = {}): GasReachField {
  return {
    station_refs: [],
    gas_to_reach: [],
    reachable: [],
    turn_frontier: [],
    max_reachable_gas: 0,
    budget_litres: 0,
    route: [],
    stage_drop_refs: [],
    final_turn_ref: null,
    turn_toward_endpoint: false,
    backgas_turn_bar: null,
    stage_drop_bars: [],
    stage_drop_over_reserve: [],
    stage_drop_distances_m: [],
    cylinder_usable_bars: [],
    route_line_length_m: 0,
    route_max_depth_m: null,
    route_avg_depth_m: null,
    waypoint_gas_litres: [],
    route_profile: [],
    stage_drop_route_indices: [],
    waypoint_route_indices: [],
    scooter_distance_m: 0,
    scooter_range_limit_m: null,
    scooter_drop_ref: null,
    route_runtime_min: null,
    turn_on_scooter_range: false,
    ...overrides,
  };
}

beforeAll(() => {
  // jsdom has no ResizeObserver; the grid measures its scroller with one.
  if (typeof ResizeObserver === 'undefined') {
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  // jsdom does not implement scrollIntoView; the role Select calls it on open.
  Element.prototype.scrollIntoView = vi.fn();
});

beforeEach(() => {
  useGasPlanStore.getState().reset();
});

/** The grid's keyboard surface: the focusable scroller inside the grid root. */
const scroller = () => document.querySelector('.sg-grid [tabindex="0"]')!;

/** Open a cell's editor, type `value` and commit it with Enter. */
function editCell(cellTestId: string, value: string) {
  fireEvent.doubleClick(screen.getByTestId(cellTestId));
  const editor = screen.getByTestId('gas-cylinder-cell-editor');
  fireEvent.change(editor, { target: { value } });
  fireEvent.keyDown(editor, { key: 'Enter' });
}

/** Seed the tank list, optionally with a patched gas config. */
function seed(cylinders: Cylinder[], gas: Partial<typeof defaultConfig.gas> = {}) {
  useGasPlanStore.setState({ cylinders, gasConfig: { ...defaultConfig.gas, ...gas } });
}

describe('CylindersTable usable-gas column', () => {
  it('is read-only under a computed strategy, showing the engine value and a dash for a row with none', () => {
    seed([tank(), tank({ enabled: false })]);
    useGasPlanStore.setState({ reachField: fakeField({ cylinder_usable_bars: [140, 0] }) });
    render(<CylindersTable />);

    expect(screen.getByTestId('gas-cylinder-usable-readonly-0')).toHaveTextContent('140');
    expect(screen.queryByTestId('gas-cylinder-usable-0')).not.toBeInTheDocument();
    // Row 1 has no engine value yet (0) - a dash, never a bare zero the diver could read
    // as "this tank credits nothing".
    expect(screen.getByTestId('gas-cylinder-usable-readonly-1')).toHaveTextContent('-');
    expect(screen.queryByTestId('gas-cylinder-usable-1')).not.toBeInTheDocument();
  });

  it('is editable under Free only for a carried budget-role row, never both modes on one row', () => {
    seed([tank(), tank({ enabled: false }), tank({ role: 'deco' })], { default_reserve: 'free' });
    render(<CylindersTable />);

    expect(screen.getByTestId('gas-cylinder-usable-0')).toBeInTheDocument();
    expect(screen.queryByTestId('gas-cylinder-usable-readonly-0')).not.toBeInTheDocument();
    // Switched off, and a role outside the penetration budget: the engine owns both.
    expect(screen.getByTestId('gas-cylinder-usable-readonly-1')).toBeInTheDocument();
    expect(screen.queryByTestId('gas-cylinder-usable-1')).not.toBeInTheDocument();
    expect(screen.getByTestId('gas-cylinder-usable-readonly-2')).toBeInTheDocument();
    expect(screen.queryByTestId('gas-cylinder-usable-2')).not.toBeInTheDocument();
  });

  it('clamps an edit to the conservative ceiling (rated, below an overfill) and accepts zero', () => {
    // An overfilled LP steel: 250 in a 207-rated tank. With conservative gas on, the engine
    // budgets against the rated pressure, so the cell must not credit more than that.
    seed([tank({ fill_bar: 250, rated_pressure_bar: 207 })], { default_reserve: 'free' });
    render(<CylindersTable />);

    editCell('gas-cylinder-cell-0-usable', '300');
    expect(useGasPlanStore.getState().cylinders[0]?.usable_gas_bar).toBe(207);

    // "Carry it, plan to breathe none of it" is a real answer, so an emptied cell is 0
    // rather than a rejected/missing edit.
    editCell('gas-cylinder-cell-0-usable', '');
    expect(useGasPlanStore.getState().cylinders[0]?.usable_gas_bar).toBe(0);
  });
});

describe('CylindersTable rows', () => {
  it('dims a row that does not count toward the penetration budget', () => {
    seed([tank(), tank({ enabled: false }), tank({ role: 'deco' })]);
    render(<CylindersTable />);

    expect(screen.getByTestId('gas-cylinder-row-0')).not.toHaveClass('opacity-50');
    expect(screen.getByTestId('gas-cylinder-row-1')).toHaveClass('opacity-50');
    expect(screen.getByTestId('gas-cylinder-row-2')).toHaveClass('opacity-50');
  });

  it('switches a tank off from its own checkbox inside the cell', () => {
    // The grid's cell mousedown calls preventDefault (it owns selection), which suppresses
    // FOCUS but not click activation - so the native checkbox still toggles.
    seed([tank()]);
    render(<CylindersTable />);

    fireEvent.click(screen.getByTestId('gas-cylinder-enabled-0'));

    expect(useGasPlanStore.getState().cylinders[0]?.enabled).toBe(false);
  });

  it('removes a tank from the row button and nulls the cached reach field', () => {
    seed([tank({ label: 'AL80' }), tank({ label: 'LP85' })]);
    useGasPlanStore.setState({ reachField: fakeField() });
    render(<CylindersTable />);

    fireEvent.click(screen.getByTestId('gas-cylinder-remove-0'));

    const { cylinders, reachField } = useGasPlanStore.getState();
    expect(cylinders).toHaveLength(1);
    expect(cylinders[0]?.label).toBe('LP85');
    expect(reachField).toBeNull();
  });
});

describe('CylindersTable role picker', () => {
  it('opens from the keyboard on the focused cell, and a picked option writes the store', () => {
    // The role cell is `noInlineEdit` - no editor opens over it - so Space stands in for the
    // click that opens the embedded Select (whose listbox portals out of the grid).
    seed([tank()]);
    render(<CylindersTable />);

    expect(screen.queryByTestId('gas-cylinder-role-0-listbox')).not.toBeInTheDocument();
    fireEvent.mouseDown(screen.getByTestId('gas-cylinder-cell-0-role'));
    fireEvent.keyDown(scroller(), { key: ' ' });

    expect(screen.getByTestId('gas-cylinder-role-0-listbox')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('gas-cylinder-role-0-option-stage'));

    expect(useGasPlanStore.getState().cylinders[0]?.role).toBe('stage');
  });
});

describe('CylindersTable fill column', () => {
  it('reads and writes psi under imperial units, keeping the store in bar', () => {
    // The grid's value space is DISPLAY units; the store is canonical bar. A fill typed as
    // psi must land as its bar equivalent, not as 3000 bar.
    seed([tank({ fill_bar: 207 })], { units: 'imperial' });
    render(<CylindersTable />);

    expect(screen.getByTestId('gas-cylinder-fill-0')).toHaveTextContent('3,002');

    editCell('gas-cylinder-cell-0-fill', '3000');
    expect(useGasPlanStore.getState().cylinders[0]?.fill_bar).toBeCloseTo(206.84, 2);
  });

  it('writes a metric fill through unchanged', () => {
    seed([tank({ fill_bar: 207 })]);
    render(<CylindersTable />);

    editCell('gas-cylinder-cell-0-fill', '232');
    expect(useGasPlanStore.getState().cylinders[0]?.fill_bar).toBe(232);
  });
});
