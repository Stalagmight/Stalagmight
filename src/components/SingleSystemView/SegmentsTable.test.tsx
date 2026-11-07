/**
 * Component test (#317, regridded in #403) - the Segments tab: one row per span between
 * way points, the length-sync invariant is exercised at the store level
 * (gasPlanStore.test.ts), so this covers the table's OWN responsibilities: labeling a
 * span's endpoints, editing speed through the grid, gating the scooter checkbox on a
 * configured DPV rig, and marking the engine-derived park point without re-deriving it.
 *
 * The pure edit rules (clamp arithmetic, scooter/speed coupling, boolean token table) are
 * pinned in `__tests__/segmentsGridEdits.test.ts`; what is left here is the wiring - that
 * the grid's gestures reach that reducer and that the store is what changes.
 */
import { describe, it, expect, beforeEach, beforeAll, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// The grid is virtualized and jsdom gives the virtualizer no layout to measure, so without
// this mock it renders zero rows and every cell query fails - the same shim the grid's own
// component tests use.
vi.mock('@tanstack/react-virtual', () => import('../../test/factories/virtualizer'));

import type { TFunction } from 'i18next';
import { SegmentsTable } from './SegmentsTable';
import { buildSegmentColumns } from './segmentsGridColumns';
import { SEG_COL } from './segmentsGridEdits';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { defaultConfig } from '../../types/config';
import { segmentColor } from '../../utils/segmentColors';
import type { DpvRig, GasReachField } from '../../types/ipc.generated';
import { NoopResizeObserver } from '../../test/factories/dom';

const A = { section_id: 1, station_index: 2 };
const B = { section_id: 1, station_index: 5 };
const C = { section_id: 1, station_index: 8 };
const D = { section_id: 1, station_index: 11 };

const RIG: DpvRig = {
  primary: { speed_m_per_min: 24, burn_minutes: 60 },
  backup: { speed_m_per_min: 18, burn_minutes: 45 },
  range_reserve_fraction: 0.2,
};

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
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = NoopResizeObserver;
  }
});

beforeEach(() => {
  useGasPlanStore.getState().reset();
});

/** The grid's keyboard surface: the focusable scroller inside the grid root. */
const scroller = () => document.querySelector('.sg-grid [tabindex="0"]')!;

/** Open a cell's editor, type `value` and commit it with Enter. */
function editCell(cellTestId: string, value: string) {
  fireEvent.doubleClick(screen.getByTestId(cellTestId));
  const editor = screen.getByTestId('segment-cell-editor');
  fireEvent.change(editor, { target: { value } });
  fireEvent.keyDown(editor, { key: 'Enter' });
}

describe('SegmentsTable', () => {
  it('renders one row per segment, labeling the first Entrance and the last Onward', () => {
    useGasPlanStore.getState().addWaypoint(A);
    useGasPlanStore.getState().addWaypoint(B);
    render(<SegmentsTable />);

    expect(screen.getByTestId('segment-row-0')).toHaveTextContent('Entrance');
    expect(screen.getByTestId('segment-row-2')).toHaveTextContent('Onward');
  });

  it('labels the origin instead of Entrance once one is set, keeping the station id as a tooltip', () => {
    useGasPlanStore.getState().setOrigin(A);
    render(<SegmentsTable />);
    const cell = screen.getByTestId('segment-cell-0-from');
    expect(cell).toHaveTextContent('Origin');
    expect(cell).not.toHaveTextContent('Entrance');
    // The id stays OFF the row and reachable only as a tooltip on the label span.
    expect(cell).not.toHaveTextContent('1:2');
    expect(cell.querySelector('[title]')).toHaveAttribute('title', '1:2');
  });

  it('labels every boundary by role: origin, each way point in order, and the endpoint', () => {
    useGasPlanStore.getState().setOrigin(A);
    useGasPlanStore.getState().addWaypoint(B);
    useGasPlanStore.getState().addWaypoint(C);
    useGasPlanStore.getState().setEndpoint(D);
    render(<SegmentsTable />);

    const row0 = screen.getByTestId('segment-row-0');
    expect(row0).toHaveTextContent('Origin');
    expect(row0).toHaveTextContent('WP 1');

    const row1 = screen.getByTestId('segment-row-1');
    expect(row1).toHaveTextContent('WP 1');
    expect(row1).toHaveTextContent('WP 2');

    const row2 = screen.getByTestId('segment-row-2');
    expect(row2).toHaveTextContent('WP 2');
    expect(row2).toHaveTextContent('Endpoint');
  });

  it('shows the plan speed on a span the diver has not given one, without writing it into the plan', () => {
    // #317: a fresh span carries `speed_m_per_min: null` = "fly this at the plan's speed", so the
    // cell has to render that speed while the STORE keeps the null - otherwise the diver's
    // configured swim / survey speed would stop reaching the engine the moment Plan mode opened.
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, swim_speed_m_per_min: 23, survey_speed_m_per_min: null },
    });
    render(<SegmentsTable />);

    expect(screen.getByTestId('segment-speed-0')).toHaveTextContent('23.0');
    // Dimmed marker, so the inherited number never reads as one the diver typed here.
    expect(screen.getByTestId('segment-speed-inherited-0')).toBeInTheDocument();
    expect(useGasPlanStore.getState().segments[0]?.speed_m_per_min).toBeNull();
  });

  it('prefers the survey speed over the swim speed for an unset span, as the engine does', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, swim_speed_m_per_min: 23, survey_speed_m_per_min: 7 },
    });
    render(<SegmentsTable />);
    expect(screen.getByTestId('segment-speed-0')).toHaveTextContent('7.0');
  });

  it('committing a speed edit updates the store and nulls the cached reach field', () => {
    useGasPlanStore.setState({ reachField: fakeField() });
    render(<SegmentsTable />);

    editCell('segment-cell-0-speed', '18');

    expect(useGasPlanStore.getState().segments[0]?.speed_m_per_min).toBe(18);
    expect(useGasPlanStore.getState().reachField).toBeNull();
  });

  it('disables the scooter checkbox with a hint when no DPV rig is configured', () => {
    useGasPlanStore.setState({ gasConfig: { ...defaultConfig.gas, dpv: null } });
    render(<SegmentsTable />);
    expect(screen.getByTestId('segment-scooter-0')).toBeDisabled();
    expect(screen.getByTestId('segments-no-rig-hint')).toBeInTheDocument();
  });

  it('enables the scooter checkbox once a rig is configured, and toggling it updates the store', async () => {
    const user = userEvent.setup();
    useGasPlanStore.setState({ gasConfig: { ...defaultConfig.gas, dpv: RIG } });
    render(<SegmentsTable />);

    const checkbox = screen.getByTestId('segment-scooter-0');
    expect(checkbox).not.toBeDisabled();
    expect(screen.queryByTestId('segments-no-rig-hint')).not.toBeInTheDocument();

    await user.click(checkbox);
    expect(useGasPlanStore.getState().segments[0]?.on_scooter).toBe(true);
  });

  it('sets the row speed to the rig primary when on_scooter is ticked on a span with no speed of its own', async () => {
    // #317 C3: a scootered span still flown at swim pace would understate the real battery
    // drain (range is spent by METRES, the rig drains by TIME), so the tick also fixes the
    // span's speed - the only value that keeps the two consistent for a span with no speed
    // of its own to begin with.
    const user = userEvent.setup();
    useGasPlanStore.setState({ gasConfig: { ...defaultConfig.gas, dpv: RIG } });
    render(<SegmentsTable />);

    await user.click(screen.getByTestId('segment-scooter-0'));

    const segment = useGasPlanStore.getState().segments[0];
    expect(segment?.on_scooter).toBe(true);
    expect(segment?.speed_m_per_min).toBe(24);
  });

  it('turns the scooter on from the keyboard, through the column parser', () => {
    // The checkbox is a `noInlineEdit` cell: no editor opens over it, so a typed character
    // goes straight through `parse` (`y` -> true). Without this path a keyboard-only diver
    // could focus the cell and never change it.
    useGasPlanStore.setState({ gasConfig: { ...defaultConfig.gas, dpv: RIG } });
    render(<SegmentsTable />);

    fireEvent.mouseDown(screen.getByTestId('segment-cell-0-scooter'));
    fireEvent.keyDown(scroller(), { key: 'y' });

    expect(useGasPlanStore.getState().segments[0]?.on_scooter).toBe(true);
  });

  it('leaves an explicit speed alone when on_scooter is ticked on a span that already has one', async () => {
    const user = userEvent.setup();
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: RIG },
      segments: [{ speed_m_per_min: 12, on_scooter: false }],
    });
    render(<SegmentsTable />);

    await user.click(screen.getByTestId('segment-scooter-0'));

    const segment = useGasPlanStore.getState().segments[0];
    expect(segment?.on_scooter).toBe(true);
    expect(segment?.speed_m_per_min).toBe(12);
  });

  it('unticking on_scooter always resets the speed to inherited, even if it was set by the tick', async () => {
    // #317 fix pass: the tick auto-fills the rig's primary speed onto an inheriting span (see
    // the previous test); leaving that number behind on untick would silently price the span at
    // a scooter pace the diver no longer has - the unsafe direction. Untick must always return
    // the span to `speed_m_per_min: null`, never keep whatever value happens to be sitting there.
    const user = userEvent.setup();
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: RIG },
      segments: [{ speed_m_per_min: 45, on_scooter: true }],
    });
    render(<SegmentsTable />);

    await user.click(screen.getByTestId('segment-scooter-0'));

    const segment = useGasPlanStore.getState().segments[0];
    expect(segment?.on_scooter).toBe(false);
    expect(segment?.speed_m_per_min).toBeNull();
  });

  it('gives each row a color chip matching the map route highlight color for that segment', () => {
    useGasPlanStore.getState().addWaypoint(A);
    useGasPlanStore.getState().addWaypoint(B);
    render(<SegmentsTable />);

    expect(screen.getByTestId('segment-color-0')).toHaveStyle({
      backgroundColor: segmentColor(0),
    });
    expect(screen.getByTestId('segment-color-1')).toHaveStyle({
      backgroundColor: segmentColor(1),
    });
    expect(screen.getByTestId('segment-color-2')).toHaveStyle({
      backgroundColor: segmentColor(2),
    });
    expect(screen.getByTestId('segment-color-0')).toHaveAttribute('aria-hidden', 'true');
  });

  it('marks the segment boundary the engine names as the DPV park point, and no other', () => {
    useGasPlanStore.getState().addWaypoint(A);
    useGasPlanStore.getState().addWaypoint(B);
    useGasPlanStore.setState({ reachField: fakeField({ scooter_drop_ref: A }) });
    render(<SegmentsTable />);

    // Segment 0 spans Entrance -> A: the drop is named at A, so it marks the END of row 0.
    expect(screen.getByTestId('segment-park-0')).toBeInTheDocument();
    expect(screen.queryByTestId('segment-park-1')).not.toBeInTheDocument();
  });

  it('shows no park marker when the engine has not derived one (null scooter_drop_ref)', () => {
    useGasPlanStore.getState().addWaypoint(A);
    useGasPlanStore.setState({ reachField: fakeField({ scooter_drop_ref: null }) });
    render(<SegmentsTable />);

    expect(screen.queryByTestId('segment-park-0')).not.toBeInTheDocument();
    expect(screen.queryByTestId('segment-park-1')).not.toBeInTheDocument();
  });

  /**
   * #317 fix pass: `resolve_segments` (src-tauri/src/models/project/gas/scooter.rs) SILENTLY
   * discards a segment speed outside `[0.1, 1000]` m/min and falls back to the plan speed - so
   * without a matching clamp here, the table would show the diver's typed value while the engine
   * (and the exported dive profile) priced a completely different one.
   */
  describe('speed commit clamps to the range the engine honors ([0.1, 1000] m/min)', () => {
    it('clamps an over-range commit down to the engine max (1000)', () => {
      render(<SegmentsTable />);
      editCell('segment-cell-0-speed', '5000');
      expect(useGasPlanStore.getState().segments[0]?.speed_m_per_min).toBe(1000);
    });

    it('clamps an under-range commit up to the engine min (0.1)', () => {
      render(<SegmentsTable />);
      editCell('segment-cell-0-speed', '0.01');
      expect(useGasPlanStore.getState().segments[0]?.speed_m_per_min).toBe(0.1);
    });
  });

  /**
   * D39d: a speed of 0 or below is no pace at all - clamped to 0.1 m/min it inflates the
   * span's gas use and collapses reach. The pre-grid PositiveNumberField never committed one;
   * the grid refuses it through the speed column's `validate`, the same way the Cylinders grid
   * refuses a negative pressure. Positive values keep the clamp above.
   */
  describe('speed <= 0 is rejected, not clamped (D39d)', () => {
    it('the speed column validate refuses <= 0 and accepts positives and a clear', () => {
      const t = ((key: string) => key) as unknown as TFunction;
      const speed = buildSegmentColumns({
        t,
        units: 'metric',
        hasRig: false,
        boundaryLabelAt: () => '',
        boundaryTitleAt: () => undefined,
        parksAfter: () => false,
        inheritedSpeedDisplay: 10,
        onToggleScooter: () => {},
      }).find((col) => col.key === SEG_COL.speed);
      const row = { speed_m_per_min: null, on_scooter: false };
      for (const value of [0, -0, -12, -0.05]) {
        expect(speed?.validate?.(value, row)).toBe('gasPanel.segments.nonPositiveSpeed');
      }
      // Under-range and over-range positives still commit (the reducer clamps them), and a
      // cleared cell (`null`) is the return-to-inherited gesture, never an error.
      for (const value of [0.05, 18, 5000, null]) {
        expect(speed?.validate?.(value, row)).toBeNull();
      }
    });

    it('rejects a typed 0 or negative speed: no commit, the editor stays open and flashes', () => {
      useGasPlanStore.setState({ reachField: fakeField() });
      render(<SegmentsTable />);

      for (const typed of ['0', '-12']) {
        editCell('segment-cell-0-speed', typed);
        const editor = screen.getByTestId('segment-cell-editor');
        expect(editor).toHaveValue(typed);
        expect(screen.getByTestId('segment-invalid-flash')).toBeInTheDocument();
        fireEvent.keyDown(editor, { key: 'Escape' });
        expect(screen.queryByTestId('segment-cell-editor')).not.toBeInTheDocument();
      }

      expect(useGasPlanStore.getState().segments[0]?.speed_m_per_min).toBeNull();
      // Nothing reached `setSegments`, so the cached field was never nulled for a recompute.
      expect(useGasPlanStore.getState().reachField).not.toBeNull();
    });

    it('skips a pasted 0 or negative speed and reports it as skipped', () => {
      useGasPlanStore.getState().addWaypoint(A);
      useGasPlanStore.getState().addWaypoint(B);
      const toasts: { message: string; key: string; kind: string }[] = [];
      const original = useUIStore.getState().showToast;
      useUIStore.setState({
        showToast: (message: string, key: string, kind: string) => toasts.push({ message, key, kind }),
      });
      try {
        render(<SegmentsTable />);
        fireEvent.mouseDown(screen.getByTestId('segment-cell-0-speed'));
        fireEvent.paste(scroller(), { clipboardData: { getData: () => '-12\n0\n15' } });

        expect(useGasPlanStore.getState().segments.map((s) => s.speed_m_per_min)).toEqual([
          null,
          null,
          15,
        ]);
        // The skipped-paste surfacing (#590): the two refused cells are reported, not dropped
        // silently.
        expect(toasts).toHaveLength(1);
        expect(toasts[0]?.key).toBe(TOAST_KEYS.PLAN_SEGMENTS_PASTE);
        expect(toasts[0]?.kind).toBe('warning');
        expect(toasts[0]?.message).toContain('2 skipped');
      } finally {
        useUIStore.setState({ showToast: original });
      }
    });

    it('reports a fill that meets a refused speed instead of dropping it silently (#576 FILL-1)', () => {
      // A stored speed of 0, as only a hand-edited plan file can carry: Fill down copies it
      // into the next two spans, whose column refuses it. Nothing is written - and now the
      // diver is told, the way a paste would tell them.
      useGasPlanStore.setState({
        segments: [
          { speed_m_per_min: 0, on_scooter: false },
          { speed_m_per_min: 12, on_scooter: false },
          { speed_m_per_min: 14, on_scooter: false },
        ],
      });
      const toasts: { message: string; key: string; kind: string }[] = [];
      const original = useUIStore.getState().showToast;
      useUIStore.setState({
        showToast: (message: string, key: string, kind: string) => toasts.push({ message, key, kind }),
      });
      try {
        render(<SegmentsTable />);
        fireEvent.mouseDown(screen.getByTestId('segment-cell-0-speed'));
        fireEvent.mouseDown(screen.getByTestId('segment-cell-2-speed'), { shiftKey: true });
        fireEvent.keyDown(scroller(), { key: 'd', ctrlKey: true });

        expect(useGasPlanStore.getState().segments.map((s) => s.speed_m_per_min)).toEqual([0, 12, 14]);
        expect(toasts).toHaveLength(1);
        expect(toasts[0]?.key).toBe(TOAST_KEYS.PLAN_SEGMENTS_PASTE);
        expect(toasts[0]?.kind).toBe('warning');
        expect(toasts[0]?.message).toBe('2 skipped (invalid)');
      } finally {
        useUIStore.setState({ showToast: original });
      }
    });
  });

  describe('clear-to-inherit (#403, replacing the per-row Reset button)', () => {
    it('returns the span to the plan speed and does not mark the cell missing', () => {
      // The column is `number-nullable`, so Delete writes a real `null` rather than the
      // "required field emptied" mark a plain number column would leave behind - the span
      // goes back to following the plan speed, which is the only way back once a number has
      // been typed.
      useGasPlanStore.setState({
        gasConfig: { ...defaultConfig.gas, swim_speed_m_per_min: 23, survey_speed_m_per_min: null },
        segments: [{ speed_m_per_min: 12, on_scooter: false }],
      });
      render(<SegmentsTable />);
      expect(screen.queryByTestId('segment-speed-inherited-0')).not.toBeInTheDocument();

      fireEvent.mouseDown(screen.getByTestId('segment-cell-0-speed'));
      fireEvent.keyDown(scroller(), { key: 'Delete' });

      expect(useGasPlanStore.getState().segments[0]?.speed_m_per_min).toBeNull();
      const inherited = screen.getByTestId('segment-speed-inherited-0');
      expect(inherited).toHaveTextContent('23.0');
      expect(screen.getByTestId('segment-cell-0-speed')).not.toHaveClass('sg-cell-missing');
    });
  });
});
