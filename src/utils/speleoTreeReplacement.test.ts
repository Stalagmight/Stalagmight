import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useGasPlanStore } from '../store/gasPlanStore';
import { useProjectStore } from '../store/projectStore';
import { speleoProjectSessionIssuedFor, useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { runProjectSwitch } from './projectTransition';
import { applySpeleoTreeReplacement } from './speleoTreeReplacement';

let refreshProjectState: ReturnType<typeof vi.fn>;
let checkProjectIntegrity: ReturnType<typeof vi.fn>;
let checkHistoryDiscarded: ReturnType<typeof vi.fn>;
let refreshLinkStatus: ReturnType<typeof vi.fn>;

beforeEach(() => {
  refreshProjectState = vi.fn(async () => {});
  checkProjectIntegrity = vi.fn(async () => []);
  checkHistoryDiscarded = vi.fn(async () => {});
  refreshLinkStatus = vi.fn(async () => {});
  useProjectStore.setState({
    refreshProjectState,
    checkProjectIntegrity,
    checkHistoryDiscarded,
    historyDiscardedDismissed: true,
    isDirty: true,
  });
  useSpeleoStore.setState({
    session: null,
    sessionGeneration: 0,
    remoteProjectId: 'p1',
    refreshLinkStatus,
  });
  useUIStore.setState({
    view: 'global',
    activeTool: 'merge',
    selection: { type: 'station', stationRef: { section_id: 1, station_index: 2 } },
    multiSelection: [
      { type: 'station', stationRef: { section_id: 1, station_index: 2 } },
    ],
    mergeStationA: { section_id: 1, station_index: 2 },
    mergeStationB: { section_id: 1, station_index: 3 },
    commentTarget: {
      type: 'shot',
      sectionId: 1,
      shotId: 7,
      screenPos: { x: 10, y: 20 },
      currentComment: 'old tree',
    },
    measureStart: { kind: 'point', x: 1, y: 2, z: 3 },
    distanceGradientOrigin: { section_id: 1, station_index: 2 },
    distanceField: {} as never,
  });
  useGasPlanStore.getState().reset();
  useGasPlanStore.getState().setEndpoint({ section_id: 1, station_index: 2 });
});

describe('applySpeleoTreeReplacement', () => {
  it('reconciles every store that owns per-tree state', async () => {
    const applied = await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);

    expect(applied).toBe(true);
    expect(useUIStore.getState()).toMatchObject({
      view: 'single',
      activeTool: 'select',
      selection: null,
      multiSelection: [],
      mergeStationA: null,
      mergeStationB: null,
      commentTarget: null,
      measureStart: null,
      distanceGradientOrigin: null,
      distanceField: null,
    });
    expect(useGasPlanStore.getState().endpoint).toBeNull();
    expect(refreshProjectState).toHaveBeenCalledOnce();
    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(checkProjectIntegrity).toHaveBeenCalledOnce();
    expect(refreshLinkStatus).toHaveBeenCalledOnce();
  });

  it('does not touch the new project when the captured identity is stale', async () => {
    const issuedFor = speleoProjectSessionIssuedFor('p1');
    useSpeleoStore.setState({ remoteProjectId: 'p2' });

    const applied = await applySpeleoTreeReplacement(issuedFor, true);

    expect(applied).toBe(false);
    expect(useUIStore.getState().selection).not.toBeNull();
    expect(useGasPlanStore.getState().endpoint).not.toBeNull();
    expect(refreshProjectState).not.toHaveBeenCalled();
    expect(checkProjectIntegrity).not.toHaveBeenCalled();
    expect(refreshLinkStatus).not.toHaveBeenCalled();
    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('holds project switches until replacement reconciliation finishes', async () => {
    let finishRefresh!: () => void;
    refreshProjectState = vi.fn(
      () => new Promise<void>((resolve) => {
        finishRefresh = resolve;
      }),
    );
    useProjectStore.setState({ refreshProjectState });

    const replacement = applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);
    await vi.waitFor(() => expect(refreshProjectState).toHaveBeenCalledOnce());

    const switchWork = vi.fn(async () => {});
    const projectSwitch = runProjectSwitch(switchWork);
    await Promise.resolve();
    expect(switchWork).not.toHaveBeenCalled();

    finishRefresh();
    await replacement;
    await projectSwitch;
    expect(switchWork).toHaveBeenCalledOnce();
  });

  /**
   * [#473] The swapped tree is a DIFFERENT file, so its history has to be re-read.
   *
   * This seam is the only place that can do it. The App effect that normally drives the
   * check keys on `file_path` + `isLoading`, and an in-place revision swap changes
   * neither - the backend reopens different bytes at the same path
   * (`speleodb_revisions.rs`) and the frontend deliberately skips `loadProject`, which
   * would short-circuit on the already-open project. Without this, a restored revision
   * whose history is unreadable showed NO banner: silent loss of undo and redo, which is
   * the one outcome #473 exists to prevent.
   */
  it('re-reads the discarded-history flag for the swapped tree', async () => {
    useProjectStore.setState({ historyDiscarded: true });

    const applied = await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);

    expect(applied).toBe(true);
    expect(checkHistoryDiscarded).toHaveBeenCalledOnce();
    // ...and the previous tree's verdict AND dismissal are both cleared, in one
    // synchronous write. A dismissal is per-project and `checkHistoryDiscarded` only
    // clears it when the condition CLEARS, so a swap from one broken tree to another
    // would otherwise stay silent; and leaving `historyDiscarded` set until the async
    // reply lands flashes a false data-loss banner over a healthy project.
    expect(useProjectStore.getState()).toMatchObject({
      historyDiscarded: false,
      historyDiscardedDismissed: false,
    });
  });

  /**
   * ...and does NOT touch it when nothing was swapped.
   *
   * Not every caller of this seam replaces a tree: `discardChanges` runs it even when the
   * backend returned `null`, meaning the lock was released and the file is untouched. The
   * rest of the reconciliation is harmless there; clearing the dismissal is not, because
   * the backend flag still reads `true` for the same unchanged project and the banner the
   * user dismissed came straight back.
   */
  it('leaves the dismissal alone when no tree was replaced', async () => {
    useProjectStore.setState({ historyDiscarded: true, historyDiscardedDismissed: true });

    const applied = await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), false);

    expect(applied).toBe(true);
    expect(checkHistoryDiscarded).not.toHaveBeenCalled();
    expect(useProjectStore.getState()).toMatchObject({
      historyDiscarded: true,
      historyDiscardedDismissed: true,
    });
  });
});
