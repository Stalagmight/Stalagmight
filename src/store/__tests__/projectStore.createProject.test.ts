import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import type { DisplaySettings } from '../../types/loading';
import { invokeIpcMock } from '../../test/factories/ipc';
import { runProjectSwitch } from '../../utils/projectTransition';

vi.mock('../../utils/ipc');

describe('projectStore.createProject', () => {
  beforeEach(() => {
    vi.clearAllMocks();

    invokeIpcMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === 'cancel_loading') return undefined;
      if (command === 'list_recent_projects') return [];
      if (command === 'create_project_full') {
        return {
          loadingId: 1,
          project: {
            name: args?.name ?? 'Untitled Project',
            location: args?.projectLocation ?? null,
            created_at: new Date().toISOString(),
            modified_at: new Date().toISOString(),
            sections: [],
            station_merges: [],
            zero_datums: [],
            loop_closures: [],
            file_path: args?.savePath ?? '/mock/Documents/stalagmight/Untitled_Project.stalx',
            next_section_id: 1,
            section_offsets: {},
            imported_file_hashes: [],
            imported_section_hashes: [],
            next_closure_id: 1,
          },
          // Complete payload (satisfies pins it): applyDisplaySettings merges
          // with ??, so an omitted field would silently keep prior in-memory
          // state and under-exercise the real create path.
          displaySettings: {
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
          } satisfies DisplaySettings,
        };
      }
      return undefined;
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
      loading: {
        loadingId: 0,
        clientOpId: 0,
        isLoading: false,
        overallProgress: 0,
      },
    });
  });

  it('passes explicit savePath and projectLocation to create_project_full', async () => {
    await useProjectStore.getState().createProject('SaveNow Cave', 'Florida Keys', {
      savePath: '/tmp/custom-save.stalx',
    });

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'create_project_full',
      expect.objectContaining({
        name: 'SaveNow Cave',
        projectLocation: 'Florida Keys',
        savePath: '/tmp/custom-save.stalx',
        deferInitialSave: false,
      }),
      expect.any(Object)
    );
  });

  it('defers initial disk write for default autosave path', async () => {
    await useProjectStore.getState().createProject('Default Docs Cave');

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'create_project_full',
      expect.objectContaining({
        name: 'Default Docs Cave',
        projectLocation: undefined,
        savePath: undefined,
        deferInitialSave: true,
      }),
      expect.any(Object)
    );
  });

  // #54: opening a project must reset the Details panel to the project-level view,
  // i.e. drop any selection carried over from a previous project/session.
  it('clears a stale selection on open', async () => {
    useUIStore.setState({ selection: { type: 'section', sectionId: 1 } });
    expect(useUIStore.getState().selection).not.toBeNull();

    await useProjectStore.getState().createProject('Fresh Cave');

    expect(useUIStore.getState().selection).toBeNull();
  });

  // #275 B1 wiring: the real createProject action (not just the uiStore method)
  // must clear armed tool transients - deleting the reset call site would
  // otherwise pass every unit test while the bug ships.
  it('clears armed tool transients on create (end-to-end wiring)', async () => {
    useUIStore.setState({
      activeTool: 'move_comment',
      moveCommentSource: {
        type: 'shot',
        sectionId: 2,
        shotId: 7,
        comment: 'stale comment from the previous project',
      },
      mergeStationA: { section_id: 1, station_index: 3 },
    });

    await useProjectStore.getState().createProject('Fresh Cave');

    const ui = useUIStore.getState();
    expect(ui.activeTool).toBe('select');
    expect(ui.moveCommentSource).toBeNull();
    expect(ui.mergeStationA).toBeNull();
  });

  /**
   * #576 RV15-2: the backend judges a create that names its file before it
   * cancels any open or load (RV14-1), but the store used to cancel the load in
   * progress (`cancel_loading`) and show its own loading state first, so a New
   * Project the policy refused still stopped an open and flashed the loading
   * modal before its error.
   */
  describe('a create the write-path policy refuses (#576 RV15-2)', () => {
    const refusal = { code: 'INTERNAL', message: 'Refused to change Cave.stalx: inside a system folder' };
    const inProgress = {
      loadingId: 7,
      clientOpId: 3,
      isLoading: true,
      overallProgress: 40,
      projectName: 'Other Cave',
      projectPath: '/caves/Other.stalx',
    };

    beforeEach(() => {
      const answer = invokeIpcMock.getMockImplementation();
      // The backend's write-path check refuses everything under /refused/, in the
      // pre-check and in the create alike.
      invokeIpcMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
        const target = args?.savePath;
        if (
          (command === 'check_create_target' || command === 'create_project_full') &&
          typeof target === 'string' &&
          target.startsWith('/refused/')
        ) {
          throw refusal;
        }
        return answer?.(command, args);
      });
    });

    // The create's own commands, in order (a store subscriber may read the config
    // on its own schedule).
    const CREATE_COMMANDS = ['check_create_target', 'cancel_loading', 'create_project_full'];
    const commandsSent = () =>
      invokeIpcMock.mock.calls.map(([command]) => command as string).filter((command) => CREATE_COMMANDS.includes(command));

    it('leaves a load in progress alone and shows no loading state', async () => {
      useProjectStore.setState({ loading: { ...inProgress } });

      const result = await useProjectStore.getState().createProject('Refused Cave', undefined, {
        savePath: '/refused/Cave.stalx',
      });

      expect({
        success: result.success,
        loadCancelled: commandsSent().includes('cancel_loading'),
        loading: useProjectStore.getState().loading,
      }).toEqual({ success: false, loadCancelled: false, loading: inProgress });
      // The dialog shows the refusal; nothing was created.
      expect(result.error).toBe(refusal.message);
      expect(commandsSent()).not.toContain('create_project_full');
    });

    // Codex review on #630: the check ran inside the switch, and entering the
    // switch supersedes the one in flight (a load's onSuperseded cancels it with
    // cancel_loading), so a refused create still stopped a valid open. A real
    // in-flight switch stands in for that load here; it must not be superseded.
    it('a refused create does not supersede a switch already in flight', async () => {
      let release: () => void = () => undefined;
      const held = new Promise<void>((resolve) => {
        release = resolve;
      });
      const superseded = vi.fn();
      const inFlight = runProjectSwitch(
        async (turn) => {
          await held;
          return turn.superseded;
        },
        { onSuperseded: superseded }
      );
      for (let i = 0; i < 5; i += 1) await Promise.resolve();

      const refused = await useProjectStore.getState().createProject('Refused Cave', undefined, {
        savePath: '/refused/Cave.stalx',
      });
      expect(refused.success).toBe(false);
      expect(refused.error).toBe(refusal.message);
      expect(superseded).not.toHaveBeenCalled();
      expect(commandsSent()).not.toContain('cancel_loading');

      release();
      expect(await inFlight).toBe(false);
    });

    it('a create the policy accepts is checked first, then runs as before', async () => {
      const result = await useProjectStore.getState().createProject('Fine Cave', undefined, {
        savePath: '/fine/Cave.stalx',
      });

      expect(result.success).toBe(true);
      expect(commandsSent()).toEqual(['check_create_target', 'cancel_loading', 'create_project_full']);
      expect(invokeIpcMock).toHaveBeenCalledWith(
        'check_create_target',
        { name: 'Fine Cave', saveDirectory: undefined, savePath: '/fine/Cave.stalx' },
        expect.any(Object)
      );
      expect(useProjectStore.getState().loading.isLoading).toBe(false);
    });

    it('a create at the default location is not checked (the backend does not check it either)', async () => {
      await useProjectStore.getState().createProject('Default Cave');

      expect(commandsSent()).toEqual(['cancel_loading', 'create_project_full']);
    });
  });
});
