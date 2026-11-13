/**
 * Undo/redo of a section reverse must move the frontend's positional refs back
 * with it (#24).
 *
 * The hazard: a reverse renumbers station `i` to `N - i`, and the
 * distance-gradient origin is a plain `StationRef`. Undo restores the old
 * numbering but leaves it where the forward mirror put it - still IN RANGE, now
 * naming the opposite physical station, so the penetration ramp silently
 * re-roots from the far end while still looking correct. No error is raised.
 *
 * The GAS PLAN's pins used to ride along here and no longer do (#310): they hold
 * backend anchors and are re-resolved around every mutation, which is
 * reverse-proof by construction. One test below pins that they are NOT mirrored
 * a second time; the anchor round trip itself is covered in
 * `gasPlanAnchors.test.ts`.
 *
 * `executeUndoRedo` therefore re-mirrors the sections the backend reports on the
 * result as renumbered (`CommandResult.reversed_sections`, populated for a
 * forward reverse and for the undo/redo that re-reverses it). These tests drive
 * the real store action with the IPC mocked, so deleting that wiring fails here.
 *
 * From `undo_to`/`redo_to` the field is an ORDERED PER-STEP LOG - one entry per
 * reversing step, repeats intact, each carrying the shot count in force at ITS
 * step - so the store composes the entries into one transform per section and
 * applies it once. The Playwright IPC mock stubs undo/redo as no-ops that never
 * emit the field, so this path is only reachable at the store level, here.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useGasPlanStore } from '../gasPlanStore';
import { useUIStore } from '../uiStore';
import type { ReversedSection } from '../../types/ipc/ReversedSection';
import type { Station } from '../../types/project';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

const SECTION = 4;

/** A 3-shot (4-station) section laid out along +x, in station order. */
function line(xs: number[]): Station[] {
  return xs.map((x, i) => ({
    station_ref: { section_id: SECTION, station_index: i },
    coordinates: { x, y: 0, z: 0 },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [],
    closures_with: [],
  }));
}

const FORWARD = [0, 10, 25, 40];
/** The same four places, numbered from the other end - i.e. after a reverse. */
const REVERSED = [40, 25, 10, 0];
/** Reversed AND translated, as one batch that reverses and moves produces. */
const REVERSED_AND_MOVED = REVERSED.map((x) => x + 1000);

/** One `reversed_sections` entry: the section, and the count in force at that step. */
const reversed = (shot_count: number): ReversedSection => ({
  section_id: SECTION,
  shot_count,
});

function projectWith(stations: Station[]) {
  return {
    name: 'Reverse',
    location: null,
    created_at: '2026-01-01T00:00:00Z',
    modified_at: '2026-01-01T00:00:01Z',
    sections: [
      {
        id: SECTION,
        name: 'Main',
        surveyor: null,
        date: null,
        color: '#fff',
        direction: 'in',
        quality: 1,
        shots: stations.slice(1).map((_s, i) => ({ id: i, azimuth_in: 0, azimuth_out: 0 })),
        next_shot_id: stations.length,
      },
    ],
    station_merges: [],
    zero_datums: [],
    loop_closures: [],
    file_path: '/tmp/reverse.stalx',
    next_section_id: 5,
    section_offsets: {},
    imported_file_hashes: [],
    imported_section_hashes: [],
  };
}

/**
 * Point every read-only refresh IPC at a project whose stations are `stations`,
 * and have undo/redo report `reversedSections` as renumbered. Omit it for an
 * operation that reversed nothing: the field is then absent from the wire, which
 * is what the backend does for every other command. `error` turns the undo into
 * a FAILED one that still reports what it managed to apply.
 */
function serveState(
  stations: Station[],
  reversedSections?: ReversedSection[],
  error?: string
) {
  invokeIpcMock.mockImplementation(async (command: string) => {
    switch (command) {
      case 'undo':
      case 'redo':
      case 'undo_to':
        return {
          success: error === undefined,
          warnings: [],
          error: error ?? null,
          section_ids: [],
          ...(reversedSections === undefined
            ? {}
            : { reversed_sections: reversedSections }),
        };
      case 'get_project_state':
        return {
          project: projectWith(stations),
          stations,
          stats: null,
          frozen_shots: [],
        };
      case 'can_undo':
      case 'can_redo':
        return false;
      case 'get_project_warnings':
        return [];
      case 'check_project_integrity':
        return { messages: [], save_blocked: false };
      default:
        return undefined;
    }
  });
}

const ref = (station_index: number) => ({ section_id: SECTION, station_index });

describe('projectStore undo/redo re-mirrors positional refs (#24)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useGasPlanStore.getState().reset();
    useUIStore.setState({ distanceGradientOrigin: null, selection: null, multiSelection: [] });
    // Pre-undo state: the section is currently REVERSED and every pin was
    // already mirrored onto it by the Reverse dialog.
    useProjectStore.setState({ stations: line(REVERSED), project: null });
  });

  it('mirrors the distance-gradient origin and clears armed tool state', async () => {
    useUIStore.setState({ distanceGradientOrigin: ref(3) });
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(3) });
    serveState(line(FORWARD), [reversed(3)]);
    await useProjectStore.getState().undo();

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(0));
    expect(useUIStore.getState().selection).toBeNull();
  });

  it('leaves the GAS PLAN pins to the anchor round trip instead (#310)', async () => {
    // Mirroring them HERE as well would apply the shift twice, landing each pin back on the
    // station the reverse moved it off. `executeUndoRedo` re-resolves them from backend anchors
    // before this runs - see `gasPlanAnchors.test.ts`, which drives that half end to end.
    useGasPlanStore.getState().setOrigin(ref(3));
    useGasPlanStore.getState().addWaypoint(ref(1));

    serveState(line(FORWARD), [reversed(3)]);
    await useProjectStore.getState().undo();

    const after = useGasPlanStore.getState();
    expect(after.origin).toEqual(ref(3));
    expect(after.waypoints).toEqual([ref(1)]);
  });

  it('re-mirrors on redo too, and on a multi-step undo_to', async () => {
    useUIStore.setState({ distanceGradientOrigin: ref(3) });

    serveState(line(FORWARD), [reversed(3)]);
    await useProjectStore.getState().redo();
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(0));

    // And back again through the history dropdown's multi-step path, which
    // reports one entry per reversing step it walked past.
    useProjectStore.setState({ stations: line(FORWARD) });
    serveState(line(REVERSED), [reversed(3)]);
    await useProjectStore.getState().undoTo(2);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(3));
  });

  it('leaves the origin alone on an undo that did not renumber anything', async () => {
    useUIStore.setState({ distanceGradientOrigin: ref(3) });

    // No `reversed_sections` on the wire: an ordinary undo must not churn the
    // distance ramp.
    serveState(line(REVERSED));
    await useProjectStore.getState().undo();

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(3));
    expect(useUIStore.getState().selection).toBeNull();
  });

  it('mirrors a section that was reversed AND moved in the same operation', async () => {
    // The case that defeats any position-diffing inference: undoing a batch that
    // both reversed and moved the section leaves its stations matching NEITHER
    // the old numbering nor its mirror, so the geometry reports nothing while
    // the numbering really did flip. The backend says so outright instead.
    useUIStore.setState({ distanceGradientOrigin: ref(1) });

    serveState(line(REVERSED_AND_MOVED), [reversed(3)]);
    await useProjectStore.getState().undo();

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(2));
  });

  it('treats an equal-count double reverse as a TOTAL no-op', async () => {
    // Two reverses of one section at the same count, undone as one range: the
    // composed transform is exactly `i -> i`, so nothing may move and nothing may
    // be cleared. The out-of-range origin is the discriminator - it is what a
    // per-entry replay would drag onto an end station (`9 -> 0 -> 7`), because
    // each replayed mirror clamps, and it is what the identity map would clamp to
    // `maxIndex` if `applyReverseAftermath` ran at all.
    useUIStore.setState({ distanceGradientOrigin: ref(9) });
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(2) });

    serveState(line(REVERSED), [reversed(7), reversed(7)]);
    await useProjectStore.getState().undoTo(1);

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(9));
    // The four UI clears are gated on the same "did anything actually renumber"
    // question, and a cancelled-out range did not: the user's selection and
    // half-staged merge are still valid.
    expect(useUIStore.getState().selection).not.toBeNull();
  });

  it('composes two reverses at DIFFERENT counts instead of cancelling them', async () => {
    // `[Reverse, CreateShot, Reverse]` walked back by one `undo_to`: the steps
    // mirrored against 4 shots then 3, so the net map is `i -> 3 - (4 - i)`, i.e.
    // one station down. Cancelling the repeat (parity) would leave the pin at 3;
    // dividing by the post-range count alone would send it to 0.
    useUIStore.setState({ distanceGradientOrigin: ref(3) });

    serveState(line(FORWARD), [reversed(4), reversed(3)]);
    await useProjectStore.getState().undoTo(2);

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(2));
  });

  it('mirrors what a PARTIALLY failed undo_to already applied', async () => {
    // The earlier steps flipped their sections and cannot be rolled back, so the
    // backend attaches them to the failure return. Ignoring the field on
    // `success === false` leaves the pins on the opposite physical stations with
    // only a step-N-of-M toast to explain it.
    useUIStore.setState({ distanceGradientOrigin: ref(1) });

    serveState(line(FORWARD), [reversed(3)], 'Undo failed at step 2 of 3: boom');
    const result = await useProjectStore.getState().undoTo(2);

    expect(result.success).toBe(false);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(2));
  });

  it('ignores the field on a failure that mutated nothing', async () => {
    // Step 1 of N failing means no step landed, so there is nothing to mirror -
    // and `executeUndoRedo` only reads the field for the partial case.
    useUIStore.setState({ distanceGradientOrigin: ref(3) });

    serveState(line(REVERSED), [reversed(3)], 'Undo failed at step 1 of 3: boom');
    await useProjectStore.getState().undoTo(2);

    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(3));
  });
});

describe('frontend mutation ordering (#24)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useGasPlanStore.getState().reset();
    useUIStore.setState({ distanceGradientOrigin: null });
    useProjectStore.setState({ project: null });
  });

  it('applies aftermath in ISSUE order when two operations overlap', async () => {
    // The transforms do not commute when the counts differ: reverse at N, a shot-count edit,
    // reverse at N+1. `executeCommand` and `executeUndoRedo` are separate async chains, each
    // awaiting its own IPC call and its own refresh, so without a shared queue the SECOND
    // operation can finish first and its aftermath lands before the first one's - walking the
    // pins onto a different station, in range and silent.
    const stations = line([0, 1, 2, 3, 4, 5, 6, 7]);
    useUIStore.setState({ distanceGradientOrigin: ref(1) });

    // The FIRST call resolves slowly, the second immediately - so completion order would be
    // reversed if nothing serialised them.
    let releaseFirst: (() => void) | undefined;
    const firstHeld = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    let call = 0;
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'get_project_state') {
        return { project: projectWith(stations), stations, stats: null, frozen_shots: [] };
      }
      if (command === 'can_undo' || command === 'can_redo') return false;
      if (command === 'get_project_warnings') return [];
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      // No anchor for the origin, so the reverse mirror places it (the path under test).
      if (command === 'plan_anchors_for_refs') return [null];
      // Both mutations report a reverse, at DIFFERENT counts.
      call += 1;
      const shot_count = call === 1 ? 7 : 6;
      if (call === 1) await firstHeld;
      return {
        success: true,
        warnings: [],
        error: null,
        section_ids: [],
        reversed_sections: [{ section_id: SECTION, shot_count }],
      };
    });

    const store = useProjectStore.getState();
    const first = store.reverseSection(SECTION, false);
    const second = store.reverseSection(SECTION, false);
    releaseFirst?.();
    await Promise.all([first, second]);

    // Composed in issue order: i -> 7 - i then i -> 6 - i, i.e. 1 -> 6 -> 0.
    // The reverse order would give 1 -> 5 -> 2, a different station.
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(0));
  });

  it('never sends a queued command against a project that opened while it waited', async () => {
    // Project load/create/close do NOT go through the mutation queue, so a command can be
    // issued against A, wait behind another entry, and reach the front after B has opened.
    // Section ids are small per-project integers, so running A's ids against B would mostly
    // RESOLVE - mutating the wrong survey rather than failing.
    const stations = line([0, 1, 2, 3, 4, 5, 6, 7]);
    let releaseFirst: (() => void) | undefined;
    const firstHeld = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    const reverseCalls: unknown[] = [];
    // The refresh has to reflect the switch too, or the first command's own
    // `refreshProjectState` would put project A back and mask the boundary entirely.
    let openProjectCreatedAt = '2026-01-01T00:00:00Z';
    invokeIpcMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'get_project_state') {
        return {
          project: { ...projectWith(stations), created_at: openProjectCreatedAt },
          stations,
          stats: null,
          frozen_shots: [],
        };
      }
      if (command === 'can_undo' || command === 'can_redo') return false;
      if (command === 'get_project_warnings') return [];
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      reverseCalls.push(args);
      if (reverseCalls.length === 1) await firstHeld;
      return { success: true, warnings: [], error: null, section_ids: [] };
    });

    useProjectStore.setState({ project: projectWith(stations) as never });
    const store = useProjectStore.getState();

    // Let the first entry actually reach its IPC and park there, so the second is genuinely
    // WAITING behind it rather than both being rejected before either starts.
    const first = store.reverseSection(SECTION, false);
    await new Promise((resolve) => setTimeout(resolve, 0));
    const second = store.reverseSection(SECTION, false);

    // Project B opens while the second entry is still queued behind the first.
    openProjectCreatedAt = '2099-12-31T00:00:00Z';
    useProjectStore.setState({
      project: { ...projectWith(stations), created_at: openProjectCreatedAt } as never,
    });
    releaseFirst?.();
    const [firstResult, secondResult] = await Promise.all([first, second]);

    expect(firstResult.success).toBe(true);
    expect(reverseCalls).toHaveLength(1);
    expect(secondResult.success).toBe(false);
    expect(secondResult.error).toContain('project changed');
  });
});

/**
 * The window the #310 anchor flush opened in both mutation seams.
 *
 * `flushPlanAnchors` is a real IPC round trip, and it sits BETWEEN the project-switch guard and
 * the `invokeIpc` that dispatches the edit. Project load/create/close do not go through the
 * mutation queue, so one can land while the flush is in flight - and `args` were captured against
 * the OUTGOING project. Section ids are small per-project integers, so they would mostly RESOLVE
 * in whatever is now open: the wrong survey is edited, silently, and the guard that exists to stop
 * exactly that already ran before the window existed.
 *
 * Both tests park the flush on a deferred `plan_anchors_for_refs` and open a project across it.
 */
describe('a project that opens during the anchor flush cancels the mutation (#310)', () => {
  const stations = line([0, 1, 2, 3, 4, 5, 6, 7]);

  beforeEach(() => {
    vi.clearAllMocks();
    useGasPlanStore.getState().reset();
    useUIStore.setState({ distanceGradientOrigin: null });
    useProjectStore.setState({ project: null });
  });

  /**
   * Serve the refresh IPCs, hold `plan_anchors_for_refs` until released, and record every command
   * that is not part of the anchor/refresh chatter - i.e. the mutations.
   */
  function serveHeldFlush() {
    let release: (() => void) | undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const mutations: string[] = [];
    const openProject = { createdAt: '2026-01-01T00:00:00Z' };
    invokeIpcMock.mockImplementation(async (command: string) => {
      switch (command) {
        case 'plan_anchors_for_refs':
          await held;
          return [{ id: 'endpoint' }];
        case 'plan_refs_for_anchors':
          return [ref(3)];
        case 'get_project_state':
          return {
            project: { ...projectWith(stations), created_at: openProject.createdAt },
            stations,
            stats: null,
            frozen_shots: [],
          };
        case 'can_undo':
        case 'can_redo':
          return false;
        case 'get_project_warnings':
          return [];
        case 'check_project_integrity':
          return { messages: [], save_blocked: false };
        default:
          mutations.push(command);
          return { success: true, warnings: [], error: null, section_ids: [] };
      }
    });
    return { release: () => release?.(), mutations, openProject };
  }

  /** Open project B while the entry is parked on its flush. */
  function switchProjectTo(createdAt: string, openProject: { createdAt: string }) {
    openProject.createdAt = createdAt;
    useProjectStore.setState({
      project: { ...projectWith(stations), created_at: createdAt } as never,
    });
  }

  it.each([
    ['executeCommand', () => useProjectStore.getState().reverseSection(SECTION, false), 'reverse_section'],
    ['executeUndoRedo', () => useProjectStore.getState().undo(), 'undo'],
  ])('%s never dispatches once the flush loses its project', async (_seam, run, command) => {
    const { release, mutations, openProject } = serveHeldFlush();
    // A pin placed moments ago is still sitting on the debounce, so the seam's flush is a genuine
    // round trip rather than the usual no-op.
    useGasPlanStore.getState().setEndpoint(ref(3));
    useProjectStore.setState({ project: projectWith(stations) as never });

    const pending = run();
    // Let the entry reach its flush and park there, rather than being refused before it starts.
    await new Promise((resolve) => setTimeout(resolve, 0));
    switchProjectTo('2099-12-31T00:00:00Z', openProject);
    release();
    const result = await pending;

    expect(mutations).not.toContain(command);
    expect(mutations).toHaveLength(0);
    expect(result.success).toBe(false);
    expect(result.error).toContain('project changed');
  });
});
