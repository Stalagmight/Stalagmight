/**
 * Component test (#241) - swim-speed disk-persist debounce.
 *
 * `PositiveNumberField` fires `onCommit` on every valid keystroke (not just
 * blur), and the swim-speed field's `onCommit` used to call
 * `useProjectStore.getState().updateGasConfig(...)` unsequenced on every one of
 * those commits - a fast typist fired one disk write per keystroke, and
 * out-of-order completion could persist an earlier intermediate value. The
 * fix debounces the disk write (keeping the in-memory `setGasConfig` update
 * immediate) so only the LATEST value is ever persisted.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('./DivePlanBar', () => ({ DivePlanBar: () => null }));

import { GasPlanPanel } from './GasPlanPanel';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useProjectStore } from '../../store/projectStore';
import { flushGasConfigWrite } from '../../utils/gasConfigDisk';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';
import type { DpvRig, GasReachField } from '../../types/ipc.generated';

const RIG: DpvRig = {
  primary: { speed_m_per_min: 24, burn_minutes: 60 },
  backup: { speed_m_per_min: 18, burn_minutes: 45 },
  range_reserve_fraction: 0.2,
};

/** A station a zero datum's `station_ref` can resolve against (#480: the entrance-no-warning
 *  guard checks a zero datum's `station_ref` for a matching entry in `stations`, not
 *  `zero_datums.length` - a dangling datum can leave a stale entry in the latter). */
const RESOLVABLE_STATION_REF = { line_id: 1, station_index: 0 };
/** A ref that never appears in `stations` - simulates a zero datum whose station was deleted. */
const DANGLING_STATION_REF = { line_id: 99, station_index: 99 };

/** A full GasReachField with a real turnaround (satisfies `hasRoute`), overridable per test. */
function makeReachField(overrides: Partial<GasReachField> = {}): GasReachField {
  const origin = { line_id: 1, station_index: 0 };
  const turn = { line_id: 1, station_index: 1 };
  return {
    station_refs: [origin, turn],
    gas_to_reach: [0, 100],
    reachable: [true, true],
    turn_frontier: [turn],
    max_reachable_gas: 100,
    budget_litres: 1000,
    route: [{ from: origin, to: turn, is_jump: false }],
    stage_drop_refs: [],
    final_turn_ref: turn,
    turn_toward_endpoint: false,
    backgas_turn_bar: 150,
    stage_drop_bars: [],
    stage_drop_over_reserve: [],
    stage_drop_distances_m: [],
    cylinder_usable_bars: [],
    route_line_length_m: 200,
    route_max_depth_m: 20,
    route_avg_depth_m: 10,
    waypoint_gas_litres: [],
    route_profile: [],
    stage_drop_route_indices: [],
    waypoint_route_indices: [],
    scooter_distance_m: 0,
    scooter_range_limit_m: null,
    scooter_drop_ref: null,
    route_runtime_min: 20,
    turn_on_scooter_range: false,
    ...overrides,
  };
}

beforeEach(async () => {
  // Drain anything a previous test left in the shared gas-config write queue into the OLD
  // mocks, so it cannot pollute this test's call counts.
  await flushGasConfigWrite();
  vi.useFakeTimers();
  useGasPlanStore.setState({
    // Entry card is a separate concern (#395, see its own describe block below); every existing
    // test in this file exercises the full rig, so answer the entry question up front.
    originChosen: true,
    originPickArmed: false,
    origin: null,
    endpoint: null,
    diver: { rmv_l_per_min: 20, label: null },
    cylinders: [],
    gasConfig: structuredClone(defaultConfig.gas),
    reachField: null,
    stageDropOverrides: [],
    loadedPlanId: null,
    loadedPlanBaseline: null,
    pushyAck: false,
    hydrated: true, // skip the one-time re-seed effect so gasConfig sticks
  });
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    // A zero datum RESOLVABLE against `stations` by default: every existing test in this file
    // exercises the "healthy" origin path, and the no-zero-datum / dangling-zero-datum cases
    // (#480) get their own describe block below with their own project + stations state.
    project: { zero_datums: [{ station_ref: RESOLVABLE_STATION_REF }] } as never,
    stations: [{ station_ref: RESOLVABLE_STATION_REF }] as never,
    updateGasConfig: vi.fn(async () => ({ success: true, error: null, warnings: [], line_ids: [] })),
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('GasPlanPanel swim-speed persist debounce (#241)', () => {
  it('coalesces rapid keystrokes into a single debounced updateGasConfig call', async () => {
    render(<GasPlanPanel />);
    const input = screen.getByTestId('gas-swim-speed-input');

    fireEvent.change(input, { target: { value: '11' } });
    fireEvent.change(input, { target: { value: '12' } });
    fireEvent.change(input, { target: { value: '13' } });

    // No disk write yet - only the debounce timer has been (re)armed.
    expect(useProjectStore.getState().updateGasConfig).not.toHaveBeenCalled();

    vi.runAllTimers();
    // The debounce timer only QUEUES the write onto the serialized chain - await it landing.
    await flushGasConfigWrite();

    const updateGasConfig = useProjectStore.getState().updateGasConfig;
    expect(updateGasConfig).toHaveBeenCalledTimes(1);
    expect(updateGasConfig).toHaveBeenCalledWith(
      expect.objectContaining({ swim_speed_m_per_min: 13 }),
    );
  });

  it('updates the in-memory working config immediately on every keystroke', () => {
    render(<GasPlanPanel />);
    const input = screen.getByTestId('gas-swim-speed-input');

    fireEvent.change(input, { target: { value: '14' } });

    expect(useGasPlanStore.getState().gasConfig.swim_speed_m_per_min).toBe(14);
    // Still debounced - the disk write hasn't fired.
    expect(useProjectStore.getState().updateGasConfig).not.toHaveBeenCalled();
  });

  it('flushes a pending write on unmount so a value typed right before leaving is not lost', async () => {
    const { unmount } = render(<GasPlanPanel />);
    const input = screen.getByTestId('gas-swim-speed-input');

    fireEvent.change(input, { target: { value: '15' } });
    expect(useProjectStore.getState().updateGasConfig).not.toHaveBeenCalled();

    // Leaving Plan mode unmounts the panel before the 450ms debounce elapses; the
    // pending value must still be persisted (parity with the pre-debounce behavior
    // that wrote every keystroke), not dropped.
    unmount();
    // Unmount flushes fire-and-forget; await the chain so the assertion sees the landed write.
    await flushGasConfigWrite();

    const updateGasConfig = useProjectStore.getState().updateGasConfig;
    expect(updateGasConfig).toHaveBeenCalledTimes(1);
    expect(updateGasConfig).toHaveBeenCalledWith(
      expect.objectContaining({ swim_speed_m_per_min: 15 }),
    );
  });

  it('disarms a stale origin pick on unmount, so leaving Plan mode never leaks it into the next click (#395 review)', () => {
    const { unmount } = render(<GasPlanPanel />);
    fireEvent.click(screen.getByTestId('gas-pick-origin'));
    expect(useGasPlanStore.getState().originPickArmed).toBe(true);

    unmount();

    expect(useGasPlanStore.getState().originPickArmed).toBe(false);
  });
});

describe('GasPlanPanel scooter (DPV) readout (#317)', () => {
  it('does not render the scooter rows without a configured rig, but still shows runtime', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: null },
      reachField: makeReachField(),
    });
    render(<GasPlanPanel />);

    expect(screen.queryByTestId('gas-readout-scooter-range')).toBeNull();
    expect(screen.queryByTestId('gas-readout-scooter-turn-warning')).toBeNull();
    // Runtime is useful with or without a rig - the segment speed still applies either way.
    expect(screen.getByTestId('gas-readout-route-runtime')).toHaveTextContent('20.0 min');
  });

  it('renders the scooter distance/range row and runtime once a rig is configured', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: RIG },
      reachField: makeReachField({ scooter_distance_m: 150, scooter_range_limit_m: 220 }),
    });
    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-readout-scooter-range')).toHaveTextContent('150 m / 220 m');
    expect(screen.getByTestId('gas-readout-route-runtime')).toHaveTextContent('20.0 min');
    expect(screen.queryByTestId('gas-readout-scooter-turn-warning')).toBeNull();
  });

  it('reports a scooter-range turn ONLY from the engine flag, never re-derived from the numbers', () => {
    // Adversarial case: distance == limit (the two would read as "equal" under a naive
    // re-derivation) but the engine says gas, not scooter range, is what turned this dive.
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: RIG },
      reachField: makeReachField({
        scooter_distance_m: 220,
        scooter_range_limit_m: 220,
        turn_on_scooter_range: false,
      }),
    });
    render(<GasPlanPanel />);

    expect(screen.queryByTestId('gas-readout-scooter-turn-warning')).toBeNull();
  });

  it('shows the scooter-range turn message when the engine flag is true', () => {
    useGasPlanStore.setState({
      gasConfig: { ...defaultConfig.gas, dpv: RIG },
      reachField: makeReachField({
        scooter_distance_m: 200,
        scooter_range_limit_m: 220,
        turn_on_scooter_range: true,
      }),
    });
    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-readout-scooter-turn-warning')).toBeInTheDocument();
  });
});

/**
 * #317: the jump RMV multiplier (alongside the include-reference-legs toggle) has NO in-panel
 * control, so Settings is its only source. The panel's seed-once `hydrated` gate (#244, which
 * exists so a mode-switch remount cannot clobber working inputs or a loaded plan) therefore
 * swallowed it entirely: nudge the jump multiplier after opening Plan mode even once and the new
 * value never appeared, with nothing on screen to explain why.
 *
 * The fix mirrors a Settings-only field EDITED IN SETTINGS during the session into the working
 * config via `gasPlanStore.syncSettingsOnlyGasFields`, keyed on the persisted value changing
 * rather than differing - which is what keeps a loaded plan's own value from being silently
 * overwritten by a personal default it was never planned on. That diffing lives in the STORE, not
 * a component ref, because the panel remounts on every mode switch and a ref would lose its
 * "already observed this session" memory exactly when the fix needs it most (see the remount
 * test below - the bug this replaced was that exact ref losing its memory on remount).
 *
 * (The DPV rig used to be probed here too. It now has its own in-panel editor and its own store
 * setter, `setDpvRig` - see `gasPlanStore.test.ts` - so it is no longer part of this sync.)
 */
describe('GasPlanPanel Settings-only gas sync (#317)', () => {
  it('picks up a jump-multiplier edit made in Settings while the panel stays mounted', () => {
    // `hydrated: true` from beforeEach is the whole point: this is the state the bug lived in.
    const { rerender } = render(<GasPlanPanel />);
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(
      defaultConfig.gas.jump_rmv_multiplier
    );

    // The diver opens Settings > Gas and changes the jump multiplier.
    const next = structuredClone(defaultConfig) as Config;
    next.gas.jump_rmv_multiplier = 2.5;
    useProjectStore.setState({ config: next });
    rerender(<GasPlanPanel />);

    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(2.5);
  });

  it('picks up a jump-multiplier edit made in Settings AFTER the panel unmounts and remounts (mode switch)', () => {
    // THE regression test: a real `unmount()` + fresh `render()`, not `rerender()` of the same
    // instance. `rerender` cannot reproduce the bug this guards - it never destroys the component,
    // so a component-local ref would still have its memory intact and the test would pass even
    // with the broken mechanism reinstated. The realistic flow is "leave Plan mode (unmounts the
    // panel), change the jump multiplier in Settings, come back to Plan (a brand-new component
    // instance)".
    const { unmount } = render(<GasPlanPanel />);
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(
      defaultConfig.gas.jump_rmv_multiplier
    );
    unmount();

    const next = structuredClone(defaultConfig) as Config;
    next.gas.jump_rmv_multiplier = 2.5;
    useProjectStore.setState({ config: next });

    render(<GasPlanPanel />);
    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(2.5);
  });

  it('leaves a loaded plan’s own jump multiplier alone when Settings did not change', () => {
    // THE adversarial case, and the reason this syncs on CHANGE rather than on difference. A plan
    // carries its own gas config in its snapshot; a dive planned on a different jump multiplier
    // must not be re-configured just because the diver's personal default disagrees. A naive "if
    // working !== persisted, copy persisted" would fail here, so this is what separates the two
    // designs.
    const settingsConfig = structuredClone(defaultConfig) as Config;
    settingsConfig.gas.jump_rmv_multiplier = 2.5;
    useProjectStore.setState({ config: settingsConfig });
    useGasPlanStore.setState({
      gasConfig: { ...structuredClone(defaultConfig.gas), jump_rmv_multiplier: 3.5 },
    });

    const { rerender } = render(<GasPlanPanel />);
    rerender(<GasPlanPanel />);

    expect(useGasPlanStore.getState().gasConfig.jump_rmv_multiplier).toBe(3.5);
  });
});

/**
 * R9/R10 (#395): the entry card - Plan mode opens with one question ("Where does this dive
 * start?") before showing the full sidebar. `originChosen` gates the split.
 */
describe('GasPlanPanel entry card (#395)', () => {
  beforeEach(() => {
    useGasPlanStore.setState({ originChosen: false, originPickArmed: false });
  });

  it('renders the entry card, not the full rig, before an origin is chosen', () => {
    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-entry-card')).toBeInTheDocument();
    expect(screen.getByTestId('gas-start-entrance')).toBeInTheDocument();
    expect(screen.queryByTestId('gas-origin-readout')).toBeNull();
    expect(screen.queryByTestId('gas-readout')).toBeNull();
    expect(screen.queryByTestId('gas-rmv-input')).toBeNull();
  });

  it('reveals the full rig once "Start from entrance" is clicked', () => {
    render(<GasPlanPanel />);

    fireEvent.click(screen.getByTestId('gas-start-entrance'));

    expect(useGasPlanStore.getState().originChosen).toBe(true);
    expect(screen.queryByTestId('gas-entry-card')).toBeNull();
    expect(screen.getByTestId('gas-origin-readout')).toBeInTheDocument();
    expect(screen.getByTestId('gas-rmv-input')).toBeInTheDocument();
  });

  it('toggles the assumptions disclosure and reflects live store values, not constants', () => {
    useGasPlanStore.setState({
      diver: { rmv_l_per_min: 25, label: null },
      cylinders: [],
    });
    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-assumptions-summary')).toHaveTextContent('25');
    expect(screen.queryByTestId('gas-rmv-input')).toBeNull();

    fireEvent.click(screen.getByTestId('gas-assumptions-toggle'));
    expect(screen.getByTestId('gas-rmv-input')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('gas-assumptions-toggle'));
    expect(screen.queryByTestId('gas-rmv-input')).toBeNull();
  });

  it('does not resurrect the entry card when setOrigin(null) resets the origin after chooseEntrance', () => {
    render(<GasPlanPanel />);
    fireEvent.click(screen.getByTestId('gas-start-entrance'));
    expect(screen.getByTestId('gas-origin-readout')).toBeInTheDocument();

    useGasPlanStore.getState().setOrigin(null);

    expect(useGasPlanStore.getState().originChosen).toBe(true);
    expect(screen.queryByTestId('gas-entry-card')).toBeNull();
    expect(screen.getByTestId('gas-origin-readout')).toBeInTheDocument();
  });
});

/**
 * #480: with no RESOLVABLE zero datum, `Project::zero_datum_roots` (backend, pathfinding.rs)
 * returns an empty root set and the entrance origin (`origin === null`) walks from nowhere -
 * every station comes back unreachable with nothing on screen to explain why. The CTA still
 * works (blocking it would strand a diver who has not set a zero datum yet with no way to
 * answer the entry question), but a warning must appear so the empty field is never silent;
 * the same `origin === null` state, whether chosen via the CTA or restored by a loaded plan
 * (`applyLoadedPlan`), needs its own warning once the entry card is gone.
 *
 * The guard checks a zero datum's `station_ref` against `stations` (the resolvable set), not
 * `zero_datums.length`: a dangling datum (its station deleted) can leave a stale entry in the
 * array while producing the same degenerate field - the codebase already learned this for TML
 * export anchoring (`ExportControls.tsx`, #411).
 */
describe('GasPlanPanel entrance requires a zero datum (#480)', () => {
  it('shows a hint but still lets "Start from entrance" through when the project has no zero datum', () => {
    useGasPlanStore.setState({ originChosen: false, originPickArmed: false });
    useProjectStore.setState({ project: { zero_datums: [] } as never, stations: [] as never });

    render(<GasPlanPanel />);

    const cta = screen.getByTestId('gas-start-entrance');
    expect(cta).toBeEnabled();
    expect(screen.getByTestId('gas-entrance-no-zero-datum-hint')).toBeInTheDocument();

    fireEvent.click(cta);
    expect(useGasPlanStore.getState().originChosen).toBe(true);
  });

  it('shows the hint when the only zero datum is dangling (its station no longer resolves)', () => {
    useGasPlanStore.setState({ originChosen: false, originPickArmed: false });
    useProjectStore.setState({
      // One entry, so `zero_datums.length > 0` - the guard must not stop there.
      project: { zero_datums: [{ station_ref: DANGLING_STATION_REF }] } as never,
      stations: [{ station_ref: RESOLVABLE_STATION_REF }] as never,
    });

    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-entrance-no-zero-datum-hint')).toBeInTheDocument();
  });

  it('shows no hint once a RESOLVABLE zero datum exists', () => {
    useGasPlanStore.setState({ originChosen: false, originPickArmed: false });
    useProjectStore.setState({
      project: { zero_datums: [{ station_ref: RESOLVABLE_STATION_REF }] } as never,
      stations: [{ station_ref: RESOLVABLE_STATION_REF }] as never,
    });

    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-start-entrance')).toBeEnabled();
    expect(screen.queryByTestId('gas-entrance-no-zero-datum-hint')).toBeNull();

    fireEvent.click(screen.getByTestId('gas-start-entrance'));
    expect(useGasPlanStore.getState().originChosen).toBe(true);
  });

  it('warns on the entrance origin readout after clicking through with no zero datum (hint-to-warning handoff)', () => {
    useGasPlanStore.setState({ originChosen: false, originPickArmed: false });
    useProjectStore.setState({ project: { zero_datums: [] } as never, stations: [] as never });

    render(<GasPlanPanel />);
    fireEvent.click(screen.getByTestId('gas-start-entrance'));

    expect(screen.getByTestId('gas-origin-no-zero-datum-warning')).toBeInTheDocument();
  });

  it('warns on the entrance origin readout when a loaded plan lands with no zero datum', () => {
    useGasPlanStore.setState({ originChosen: true, origin: null });
    useProjectStore.setState({ project: { zero_datums: [] } as never, stations: [] as never });

    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-origin-no-zero-datum-warning')).toBeInTheDocument();
  });

  it('does not warn on the entrance origin readout when a resolvable zero datum is set', () => {
    useGasPlanStore.setState({ originChosen: true, origin: null });
    useProjectStore.setState({
      project: { zero_datums: [{ station_ref: RESOLVABLE_STATION_REF }] } as never,
      stations: [{ station_ref: RESOLVABLE_STATION_REF }] as never,
    });

    render(<GasPlanPanel />);

    expect(screen.queryByTestId('gas-origin-no-zero-datum-warning')).toBeNull();
  });

  it('does not warn on the origin readout once a real station origin is picked', () => {
    useGasPlanStore.setState({
      originChosen: true,
      origin: { line_id: 1, station_index: 0 },
    });
    useProjectStore.setState({ project: { zero_datums: [] } as never, stations: [] as never });

    render(<GasPlanPanel />);

    expect(screen.queryByTestId('gas-origin-no-zero-datum-warning')).toBeNull();
  });
});

/**
 * Stations in the panel (origin, endpoint, way points, marked points and the turn station the
 * engine falls back to) read by name with their line (`Main Line / 5`, SPEC 4.1): station names
 * are only line-unique. Never the positional `line:index` handle the store addresses them by.
 */
describe('GasPlanPanel station labels', () => {
  const at = (station_index: number) => ({ line_id: 1, station_index });

  beforeEach(() => {
    useProjectStore.setState({
      surveyData: null,
      project: {
        zero_datums: [{ station_ref: RESOLVABLE_STATION_REF }],
        lines: [{ id: 1, name: 'Main Line' }],
      } as never,
      // Names that differ from the positions, so a positional label would show.
      stations: [0, 1, 2, 3, 4].map((i) => ({ station_ref: at(i), name: `A${i + 10}`, id: 100 + i })) as never,
    });
  });

  it('names the origin, endpoint, way points and marked points by line and name', () => {
    const plan = useGasPlanStore.getState();
    plan.setOrigin(at(0));
    plan.setEndpoint(at(4));
    plan.addWaypoint(at(2));
    plan.addMarkedPoint(at(3));
    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-origin-readout')).toHaveTextContent('Main Line / A10');
    expect(screen.getByTestId('gas-endpoint-readout')).toHaveTextContent('Main Line / A14');
    expect(screen.getByTestId('gas-waypoint-row-0')).toHaveTextContent('Main Line / A12');
    expect(screen.getByTestId('gas-marked-row-0')).toHaveTextContent('Main Line / A13');
    for (const id of ['gas-origin-readout', 'gas-endpoint-readout', 'gas-waypoint-row-0', 'gas-marked-row-0']) {
      expect(screen.getByTestId(id).textContent).not.toMatch(/\d+:\d+/);
    }
  });

  it('names the station the plan turns at when the endpoint is out of reach', () => {
    const plan = useGasPlanStore.getState();
    plan.setEndpoint(at(4));
    plan.setReachField(makeReachField({ final_turn_ref: at(1), turn_toward_endpoint: true }));
    render(<GasPlanPanel />);

    expect(screen.getByTestId('gas-endpoint-out-of-reach')).toHaveTextContent(
      'Out of reach on this gas. Turning at Main Line / A11, on the way there.'
    );
  });
});
