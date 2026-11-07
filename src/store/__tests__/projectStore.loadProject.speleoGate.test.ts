/**
 * #270 render-order pin: `loadProject` must settle the SpeleoDB read-only gate
 * BEFORE `setView('single')` puts the project on screen.
 *
 * The original bug was ordering, not just defaulting: the load fired
 * `refreshLinkStatus()` unawaited AFTER the view switch, so a remote project
 * rendered with `link === null` for as long as that round trip took (and it is
 * not a cheap local read - for a remote project `speleodb_link_status` also makes
 * best-effort `get_project` / `get_revisions` HTTP calls). These tests sample the
 * gate at the exact instant of the view switch, so re-introducing the gap fails
 * here rather than only in the app.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { useSpeleoStore, selectSpeleoReadOnly } from '../speleoStore';
import type { DisplaySettings } from '../../types/loading';
import { invokeIpcMock } from '../../test/factories/ipc';

const loadBinaryMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

const displaySettings: DisplaySettings = {
  showStations: true,
  showShots: true,
  showSectionNames: true,
  showStationLabels: false,
  showShotData: false,
  showReferenceShots: true,
  showJumps: true,
  showZeroDatum: true,
  showGrid: false,
  showSatellite: false,
  showDepthGradient: false,
  showStationComments: false,
  showShotComments: false,
  gridSpacing: 10,
  geometryMode: 'measured',
};

/** Resolves the load with the backend's `is_remote` verdict for this project. */
const mockLoad = (isRemote: boolean) =>
  loadBinaryMock.mockResolvedValue({
    loadingId: 1,
    isRemote,
    project: {
      name: 'Loaded Cave',
      location: null,
      created_at: new Date().toISOString(),
      modified_at: new Date().toISOString(),
      sections: [],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      file_path: '/mock/loaded.stalx',
      next_section_id: 1,
      section_offsets: {},
      imported_file_hashes: [],
      imported_section_hashes: [],
      next_closure_id: 1,
    },
    stations: [],
    surveyData: { shots: [], stations: [] },
    stats: null,
    warnings: [],
    frozen_shots: [],
    displaySettings,
  });

/** The read-only verdict sampled at the moment the project view is switched in. */
let readOnlyAtViewSwitch: boolean | null;
/** Whether the SpeleoDB shell had a link by then (only the synchronous
 *  `syncProjectLink` can have seeded one this early). */
let hadLinkAtViewSwitch: boolean | null;

beforeEach(() => {
  vi.clearAllMocks();
  readOnlyAtViewSwitch = null;
  hadLinkAtViewSwitch = null;

  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects') return [];
    // Never answers: stands in for the slow/hanging status probe the gate must
    // no longer depend on. (`refreshLinkStatus` swallows the rejection.)
    if (command === 'speleodb_link_status') return new Promise(() => {});
    return undefined;
  });

  useSpeleoStore.setState({
    link: null,
    projectLinkKind: 'unknown',
    remoteProjectId: null,
    divergence: null,
    banner: null,
    session: null,
  });

  const realSetView = useUIStore.getState().setView;
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    setView: (view) => {
      if (view === 'single') {
        readOnlyAtViewSwitch = selectSpeleoReadOnly(useSpeleoStore.getState());
        hadLinkAtViewSwitch = useSpeleoStore.getState().link !== null;
      }
      realSetView(view);
    },
  });

  useProjectStore.setState({
    project: null,
    stations: [],
    stats: null,
    frozenShots: [],
    warnings: [],
    recentProjects: [],
    autosaveFailure: null,
    isDirty: false,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
  });
});

describe('loadProject resolves the SpeleoDB gate before the view renders (#270)', () => {
  it('a remote project is already read-only when the project view appears', async () => {
    mockLoad(true);

    await useProjectStore.getState().loadProject('/mock/loaded.stalx');

    expect(readOnlyAtViewSwitch).toBe(true);
    // Proves the gate was RESOLVED at that point rather than merely unresolved
    // (the pessimistic default would also read `true`): only the synchronous
    // open-time resolution can have seeded the shell this early.
    expect(hadLinkAtViewSwitch).toBe(true);
    // ... and stays read-only afterwards: the status probe never answered.
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(true);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
  });

  it('a local project is already editable when the project view appears (no read-only flash)', async () => {
    mockLoad(false);

    await useProjectStore.getState().loadProject('/mock/loaded.stalx');

    expect(readOnlyAtViewSwitch).toBe(false);
    expect(hadLinkAtViewSwitch).toBe(false);
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
  });
});
