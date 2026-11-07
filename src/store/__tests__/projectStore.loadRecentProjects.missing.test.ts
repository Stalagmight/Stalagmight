/**
 * [#576 HFI-12] A recent project moved or deleted while the app runs is shown as
 * missing as soon as the recents list is re-read, not only after a config reload.
 *
 * The Global View lists a recent the backend could not load as a "missing" row:
 * a path in the config's recents with no metadata in `list_recent_projects`
 * (`deriveMissingPaths`). The store's config copy, though, was read before every
 * project opened or created since, so such a project was in neither list and the
 * row just vanished; a config reload (or a restart) then brought it back as
 * missing. `loadRecentProjects` now re-reads the recent paths with the list.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { deriveMissingPaths } from '../../components/GlobalView/recentListItems';
import { invokeIpcMock } from '../../test/factories/ipc';
import type { Config } from '../../types/ipc/Config';
import type { ProjectMetadata } from '../../types/project';

vi.mock('../../utils/ipc');

const KEPT = '/projects/CaveA.stalx';
const MOVED = '/projects/CaveB.stalx';
const DELETED = '/projects/CaveC.stalx';

const meta = (file_path: string): ProjectMetadata => ({ file_path, name: file_path }) as ProjectMetadata;

/** A config whose recents are `recent_projects`, every other field a marker. */
const configWith = (recent_projects: string[], marker = 'store'): Config =>
  ({ general: { recent_projects, recent_projects_max: 20 }, display: { marker } }) as unknown as Config;

/** The backend now: three recents in its config, only one of them still loadable. */
function backendAfterMoveAndDelete(): void {
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects') return [meta(KEPT)];
    if (command === 'get_config') return configWith([KEPT, MOVED, DELETED], 'backend');
    return undefined;
  });
}

/** The missing rows the Global View derives (GlobalView.tsx `missingPaths`). */
function missingRows(): string[] {
  const { config, recentProjects } = useProjectStore.getState();
  return deriveMissingPaths(config?.general.recent_projects ?? [], recentProjects);
}

describe('loadRecentProjects: a recent that stopped loading is missing at once (#576 HFI-12)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(console, 'error').mockImplementation(() => {});
    // The store's config was read at startup, before the three were opened: its
    // recents do not name them. All three loaded the last time the list was read.
    useProjectStore.setState({
      config: configWith([]),
      recentProjects: [meta(KEPT), meta(MOVED), meta(DELETED)],
    });
  });

  it('shows the moved and the deleted project as missing rows, not gone', async () => {
    backendAfterMoveAndDelete();

    await useProjectStore.getState().loadRecentProjects();

    expect(useProjectStore.getState().recentProjects.map((p) => p.file_path)).toEqual([KEPT]);
    expect(missingRows()).toEqual([MOVED, DELETED]);
  });

  it('takes only the recent paths from the config it reads, and only when they changed', async () => {
    backendAfterMoveAndDelete();

    await useProjectStore.getState().loadRecentProjects();

    const config = useProjectStore.getState().config as unknown as { display: { marker: string } };
    expect(config.display.marker).toBe('store');

    const settled = useProjectStore.getState().config;
    await useProjectStore.getState().loadRecentProjects();
    expect(useProjectStore.getState().config).toBe(settled);
  });

  it('still updates the list when the config cannot be read', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'list_recent_projects') return [meta(KEPT)];
      if (command === 'get_config') throw { code: 'FAILED', message: 'config unreadable' };
      return undefined;
    });

    await useProjectStore.getState().loadRecentProjects();

    expect(useProjectStore.getState().recentProjects.map((p) => p.file_path)).toEqual([KEPT]);
    expect(useProjectStore.getState().config?.general.recent_projects).toEqual([]);
  });
});
