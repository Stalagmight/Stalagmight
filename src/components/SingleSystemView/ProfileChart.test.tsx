/**
 * Component test (#299) - the Plan-mode dive profile.
 *
 * The geometry itself is covered in `utils/__tests__/routeProfile.test.ts`; what is asserted here
 * is the wiring the chart owns: that it renders from the store rather than fetching, that the axis
 * toggle actually re-projects, and - the one that matters most - that it shows a "computing" state
 * rather than a stale chart while the reach field is null. Every plan input nulls `reachField`, so
 * that window opens on literally every edit, and a chart that kept painting the previous route
 * would be showing a depth profile for a plan the diver has already changed.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

// The export IPC is mocked so the failure path is reachable; `saveDialog` must resolve to a path or
// the export short-circuits before the command is ever called.
vi.mock('../../utils/divePlanApi', () => ({
  exportDiveProfile: vi.fn(async () => undefined),
  renderDiveProfileText: vi.fn(async () => 'profile text'),
}));
vi.mock('../../utils/fileDialogs', () => ({
  saveDialog: vi.fn(async () => '/tmp/profile.xml'),
  exportSaveFilters: vi.fn(() => []),
  ensureExtension: (p: string) => p,
}));

import { ProfileChart } from './ProfileChart';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useUIStore } from '../../store/uiStore';
import { exportDiveProfile } from '../../utils/divePlanApi';
import { defaultConfig } from '../../types/config';
import type { GasReachField, RouteProfilePoint } from '../../types/ipc.generated';

const point = (over: Partial<RouteProfilePoint>): RouteProfilePoint => ({
  route_index: 0,
  station_ref: { line_id: 1, station_index: 0 },
  distance_m: 0,
  depth_m: 0,
  gas_l: 0,
  elapsed_min: 0,
  outbound: true,
  ...over,
});

/** 200 m in at 5 m/min, 200 m out at 10 m/min - so the axes disagree about the turnaround. */
const profile: RouteProfilePoint[] = [
  point({ route_index: 0, distance_m: 0, depth_m: 0, elapsed_min: 0 }),
  point({ route_index: 1, distance_m: 100, depth_m: 12, elapsed_min: 20 }),
  point({ route_index: 2, distance_m: 200, depth_m: 24, elapsed_min: 40 }),
  point({ route_index: 1, distance_m: 300, depth_m: 12, elapsed_min: 50, outbound: false }),
  point({ route_index: 0, distance_m: 400, depth_m: 0, elapsed_min: 60, outbound: false }),
];

const fakeField = (over: Partial<GasReachField> = {}): GasReachField =>
  ({
    station_refs: [],
    gas_to_reach: [],
    reachable: [],
    turn_frontier: [],
    max_reachable_gas: 0,
    budget_litres: 2000,
    route: [],
    stage_drop_refs: [],
    final_turn_ref: null,
    turn_toward_endpoint: false,
    backgas_turn_bar: null,
    stage_drop_bars: [],
    stage_drop_over_reserve: [],
    stage_drop_distances_m: [],
    cylinder_usable_bars: [],
    route_line_length_m: 200,
    route_max_depth_m: 24,
    route_avg_depth_m: 12,
    waypoint_gas_litres: [],
    route_profile: profile,
    stage_drop_route_indices: [],
    waypoint_route_indices: [],
    ...over,
  }) as GasReachField;

const seed = (reachField: GasReachField | null) => {
  useGasPlanStore.setState({
    origin: null,
    endpoint: null,
    waypoints: [],
    diver: { rmv_l_per_min: 20, label: null },
    cylinders: [],
    gasConfig: structuredClone(defaultConfig.gas),
    reachField,
    stageDropOverrides: [],
    loadedPlanId: null,
    loadedPlanBaseline: null,
    pushyAck: false,
    hydrated: true,
  });
};

beforeEach(() => {
  // Without this, `mock.calls[0]` in one test is the call another test made.
  vi.clearAllMocks();
  seed(null);
});

describe('ProfileChart (#299)', () => {
  it('shows the computing state, not a stale chart, while the field is null', () => {
    render(<ProfileChart />);
    expect(screen.getByTestId('plan-profile-empty')).toBeTruthy();
    expect(screen.queryByTestId('plan-profile-svg')).toBeNull();
  });

  it('distinguishes "recomputing" from "this plan has no route"', () => {
    // Both render the same empty pane, and conflating them is a real UX bug: an unreachable
    // endpoint would sit there claiming to be computing forever.
    seed(fakeField({ route_profile: [] }));
    render(<ProfileChart />);
    expect(screen.getByTestId('plan-profile-empty').textContent).toContain('No route to plot');
  });

  it('draws the round trip from the store without asking for a compute', () => {
    seed(fakeField());
    render(<ProfileChart />);
    const svg = screen.getByTestId('plan-profile-svg');
    expect(svg.getAttribute('data-axis')).toBe('distance');
    const line = screen.getByTestId('plan-profile-line').getAttribute('d') ?? '';
    // One command per profile point - the whole round trip, not just the way in.
    expect(line.match(/[ML]/g)?.length).toBe(profile.length);
    expect(line).not.toContain('NaN');
    expect(screen.getByTestId('plan-profile-marker-turn')).toBeTruthy();
  });

  it('re-projects when the axis is toggled, and the turn moves because the legs differ in speed', () => {
    seed(fakeField());
    render(<ProfileChart />);
    const turnX = () => screen.getByTestId('plan-profile-turn-line').getAttribute('x1');
    const byDistance = turnX();

    fireEvent.click(screen.getByTestId('plan-profile-axis-time'));

    expect(screen.getByTestId('plan-profile-svg').getAttribute('data-axis')).toBe('time');
    // On distance the turn is at 200/400 = halfway; on time it is at 40/60 = two thirds. If the
    // toggle only relabelled the axis instead of re-projecting, these would be identical.
    expect(turnX()).not.toBe(byDistance);
    expect(screen.getByTestId('plan-profile-axis-time').getAttribute('aria-pressed')).toBe('true');
    expect(
      screen.getByTestId('plan-profile-axis-distance').getAttribute('aria-pressed')
    ).toBe('false');
  });

  it('marks each stage drop on its outbound pass and flags an over-reserve one', () => {
    seed(
      fakeField({
        stage_drop_route_indices: [1],
        stage_drop_over_reserve: [true],
        waypoint_route_indices: [2],
      })
    );
    render(<ProfileChart />);
    const drop = screen.getByTestId('plan-profile-marker-stage-drop-0');
    expect(drop.getAttribute('data-over-reserve')).toBe('true');
    expect(screen.getByTestId('plan-profile-marker-waypoint-0')).toBeTruthy();
    // Route index 1 also occurs on the return leg (at 300 m of 400). The drop must be drawn on the
    // outbound pass, i.e. left of the turnaround line.
    const dropX = Number(drop.querySelector('line')?.getAttribute('x1'));
    const turn = Number(screen.getByTestId('plan-profile-turn-line').getAttribute('x1'));
    expect(dropX).toBeLessThan(turn);
  });

  it('announces the one-way penetration, not the round-trip distance', () => {
    // The series ends at 400 m (back at the entrance); penetration is 200 m. Announcing 400 told a
    // screen-reader user double the distance they are planning to swim in on.
    seed(fakeField());
    render(<ProfileChart />);
    const label = screen.getByTestId('plan-profile-svg').getAttribute('aria-label') ?? '';
    expect(label).toContain('200 m penetration');
    expect(label).not.toContain('400 m penetration');
    // Runtime IS the round trip, and stays so.
    expect(label).toContain('60 minutes round trip');
  });

  it('does not claim "Copied" when there is no clipboard, and says why', async () => {
    // `navigator.clipboard?.writeText(...)` resolves to undefined where the API is absent - it is,
    // on WebKitGTK and in insecure contexts - so an optional-chained write flips the button to
    // "Copied" over an empty clipboard. A refusal must not read as success.
    const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
    const toasts: string[] = [];
    useUIStore.setState({ showToast: (m: string) => toasts.push(m) });
    try {
      seed(fakeField());
      render(<ProfileChart />);
      fireEvent.click(screen.getByTestId('plan-profile-copy'));
      await waitFor(() => expect(toasts.length).toBe(1));
      expect(toasts[0]).toContain('clipboard unavailable');
      expect(screen.getByTestId('plan-profile-copy').textContent).not.toContain('Copied');
    } finally {
      if (original) Object.defineProperty(navigator, 'clipboard', original);
    }
  });

  it('surfaces a backend refusal instead of silently re-enabling the button', async () => {
    // The backend refuses on reachable paths (no route, clobbering the open project, any I/O
    // error). Swallowed to console, the user picks a filename and simply gets no file.
    vi.mocked(exportDiveProfile).mockRejectedValueOnce(
      new Error('Cannot export over the open project file')
    );
    const toasts: string[] = [];
    useUIStore.setState({ showToast: (m: string) => toasts.push(m) });
    seed(fakeField());
    render(<ProfileChart />);
    fireEvent.click(screen.getByTestId('plan-profile-export-xml'));
    await waitFor(() => expect(toasts.length).toBe(1));
    expect(toasts[0]).toContain('Cannot export over the open project file');
  });

  /**
   * #317 fix pass: a pin the backend can no longer place (#310) is withheld from the compute gate
   * and blocks Save - the export door was the one place still reading the raw store pins, so it
   * would happily write a document routed through whichever station the STALE positional ref now
   * names.
   */
  it('disables both export buttons, with a title that changes to explain why, while any plan pin is unresolved', () => {
    seed(fakeField());
    const { rerender } = render(<ProfileChart />);
    const copyBtn = screen.getByTestId('plan-profile-copy');
    const xmlBtn = screen.getByTestId('plan-profile-export-xml');
    expect(copyBtn).not.toBeDisabled();
    const resolvedCopyTitle = copyBtn.getAttribute('title');
    const resolvedXmlTitle = xmlBtn.getAttribute('title');

    useGasPlanStore.setState({
      unresolved: {
        origin: true,
        endpoint: false,
        waypoints: [],
        markedPoints: [],
        stageDropOverrides: [],
      },
    });
    rerender(<ProfileChart />);

    expect(copyBtn).toBeDisabled();
    expect(xmlBtn).toBeDisabled();
    // The title must actually explain the NEW reason, not keep showing the plain export hint - a
    // disabled control with a stale hint is as misleading as no gate at all.
    expect(copyBtn.getAttribute('title')).not.toBe(resolvedCopyTitle);
    expect(xmlBtn.getAttribute('title')).not.toBe(resolvedXmlTitle);
    expect(copyBtn.getAttribute('title')).toBe(xmlBtn.getAttribute('title'));
  });

  it('re-enables both export buttons once the unresolved pin clears', () => {
    seed(fakeField());
    useGasPlanStore.setState({
      unresolved: {
        origin: false,
        endpoint: false,
        waypoints: [],
        markedPoints: [],
        stageDropOverrides: [],
      },
    });
    render(<ProfileChart />);
    expect(screen.getByTestId('plan-profile-copy')).not.toBeDisabled();
    expect(screen.getByTestId('plan-profile-export-xml')).not.toBeDisabled();
  });

  it('sends the plan as it is at click time, not as it was at render time', async () => {
    // The XML path awaits a save dialog first; on Android that dialog is an in-app picker the user
    // can edit the plan behind. A render-time closure would export a plan that is no longer on
    // screen.
    seed(fakeField());
    render(<ProfileChart />);
    const moved = { line_id: 9, station_index: 9 };
    useGasPlanStore.setState({ endpoint: moved });
    fireEvent.click(screen.getByTestId('plan-profile-export-xml'));
    await waitFor(() => expect(vi.mocked(exportDiveProfile)).toHaveBeenCalled());
    const call = vi.mocked(exportDiveProfile).mock.calls[0];
    expect(call?.[2].endpoint).toEqual(moved);
  });

  it('labels every marker rule, so a vertical line is not a riddle', () => {
    // Unlabelled, a stage drop and a way point are the same anonymous vertical line - which defeats
    // the point of distinguishing them.
    seed(
      fakeField({
        stage_drop_route_indices: [1],
        stage_drop_over_reserve: [false],
        waypoint_route_indices: [1],
      })
    );
    render(<ProfileChart />);
    expect(screen.getByTestId('plan-profile-label-stage-drop-0').textContent).toBe('Stage 1');
    expect(screen.getByTestId('plan-profile-label-waypoint-0').textContent).toBe('WP 1');
    expect(screen.getByTestId('plan-profile-label-turn').textContent).toBe('Turnaround');
  });

  it('stops the turnaround rule at the profile, not at the plot floor', () => {
    // The turn is not always the deepest point; a full-height rule then hangs below the depth curve
    // and reads as a drawing error.
    //
    // This needs a fixture where the turn is NOT the deepest point. With the default one (turn ==
    // deepest) `p.y(turnDepth)` IS the plot floor, so the assertion held for the broken
    // implementation too - it compared the rule against the marker dot, and production draws both
    // from the same expression, making it a tautology. Here the turn is at 10 m over a 30 m
    // maximum, so the correct y2 is a third of the way down and the bug's y2 is the floor.
    const shallowTurn: RouteProfilePoint[] = [
      point({ route_index: 0, distance_m: 0, depth_m: 0, elapsed_min: 0 }),
      point({ route_index: 1, distance_m: 100, depth_m: 30, elapsed_min: 20 }),
      point({ route_index: 2, distance_m: 200, depth_m: 10, elapsed_min: 40 }),
      point({ route_index: 1, distance_m: 300, depth_m: 30, elapsed_min: 50, outbound: false }),
      point({ route_index: 0, distance_m: 400, depth_m: 0, elapsed_min: 60, outbound: false }),
    ];
    seed(fakeField({ route_profile: shallowTurn, route_max_depth_m: 30 }));
    render(<ProfileChart />);
    const lineY2 = Number(screen.getByTestId('plan-profile-turn-line').getAttribute('y2'));
    const lineY1 = Number(screen.getByTestId('plan-profile-turn-line').getAttribute('y1'));
    const dotY = Number(screen.getByTestId('plan-profile-marker-turn').getAttribute('cy'));
    // The plot floor the broken version reached to: the deepest gridline, whichever tick that is.
    const depthTicks = screen.getAllByTestId(/^plan-profile-depth-tick-/);
    const floor = Number(depthTicks[depthTicks.length - 1]?.getAttribute('y')) - 3;
    expect(lineY2).toBeCloseTo(dotY, 5);
    expect(lineY2).toBeLessThan(floor);
    // And it is a real rule, not a zero-length one that would trivially satisfy the above.
    expect(lineY2).toBeGreaterThan(lineY1);
  });

  it('recovers when the field flips back to null mid-session', () => {
    // The blink is not a one-off at mount: it reopens on every single plan edit.
    const { rerender } = render(<ProfileChart />);
    seed(fakeField());
    rerender(<ProfileChart />);
    expect(screen.getByTestId('plan-profile-svg')).toBeTruthy();

    seed(null);
    rerender(<ProfileChart />);
    expect(screen.queryByTestId('plan-profile-svg')).toBeNull();
    expect(screen.getByTestId('plan-profile-empty')).toBeTruthy();
  });
});
