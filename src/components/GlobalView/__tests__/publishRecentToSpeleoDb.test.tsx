/**
 * "Publish to SpeleoDB" from the GlobalView recents context menu (#270).
 *
 * Two things are pinned here:
 *
 *  - GATING. The item is offered only for a LOCAL recent, only with a signed-in
 *    SpeleoDB session, and only on a single selection. A remote row has nothing
 *    to publish and a logged-out user would only get a failing click, so both
 *    must yield NO item rather than a present-but-broken one.
 *
 *  - ORDERING. The backend publishes the OPEN project and the dialog prefills
 *    from it in `useState` initializers (read once, at mount). So the project has
 *    to be opened AND that load has to settle BEFORE the dialog is raised.
 *    Reversed, the form would carry the previously open project's name and
 *    publish under it. The last test mounts the REAL dialog off the real UI store
 *    and asserts the prefill, with the reversed order run as a control so the
 *    assertion is known to have teeth.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import { canPublishRecent, publishRecentToSpeleoDb } from '../publishRecentToSpeleoDb';
import PublishToSpeleoDbDialog from '../../Dialogs/PublishToSpeleoDbDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { ProjectMetadata } from '../../../types/project';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';

const localProject = (over: Partial<ProjectMetadata> = {}): ProjectMetadata => ({
  name: 'Local Cave',
  location: null,
  file_path: '/p/local.stalx',
  modified_at: '2026-07-26T00:00:00Z',
  section_count: 1,
  total_length: 10,
  station_count: 2,
  gps_center: null,
  gps_bounds: null,
  is_remote: false,
  ...over,
});

describe('canPublishRecent (context-menu gating)', () => {
  it('offers the item for a local recent with a signed-in session', () => {
    expect(canPublishRecent(localProject(), true, 1)).toBe(true);
  });

  it('hides it for a project already on SpeleoDB', () => {
    expect(canPublishRecent(localProject({ is_remote: true }), true, 1)).toBe(false);
    // Belt and braces: an id present but `is_remote` stale still reads as remote.
    expect(
      canPublishRecent(localProject({ speleodb_project_id: 'abc' }), true, 1),
    ).toBe(false);
  });

  it('hides it when logged out (no failing click)', () => {
    expect(canPublishRecent(localProject(), false, 1)).toBe(false);
  });

  it('hides it under a multi-selection (publishing is per project)', () => {
    expect(canPublishRecent(localProject(), true, 3)).toBe(false);
  });

  it('hides it when the right-clicked path is not a known recent', () => {
    expect(canPublishRecent(undefined, true, 1)).toBe(false);
    expect(canPublishRecent(null, true, 1)).toBe(false);
  });
});

describe('publishRecentToSpeleoDb (open, THEN dialog)', () => {
  it('does not raise the dialog until the open has settled', async () => {
    const order: string[] = [];
    let settle!: (r: { success: boolean }) => void;
    const loadProject = vi.fn(
      () =>
        new Promise<{ success: boolean }>((res) => {
          order.push('load');
          settle = res;
        }),
    );
    const openDialog = vi.fn(() => { order.push('dialog'); });

    const done = publishRecentToSpeleoDb('/p/local.stalx', { loadProject, openDialog });
    // The load is in flight - nothing may have been raised yet.
    await Promise.resolve();
    expect(loadProject).toHaveBeenCalledWith('/p/local.stalx');
    expect(openDialog).not.toHaveBeenCalled();

    settle({ success: true });
    await expect(done).resolves.toBe(true);
    expect(openDialog).toHaveBeenCalledWith('publish-to-speleodb');
    expect(order).toEqual(['load', 'dialog']);
  });

  it('tags the telemetry source before opening', async () => {
    const order: string[] = [];
    await publishRecentToSpeleoDb('/p/local.stalx', {
      loadProject: vi.fn(async () => { order.push('load'); return { success: true }; }),
      openDialog: vi.fn(() => { order.push('dialog'); }),
      markSource: () => { order.push('source'); },
    });
    expect(order).toEqual(['source', 'load', 'dialog']);
  });

  it('raises no dialog when the open fails (loadProject surfaced it)', async () => {
    const openDialog = vi.fn();
    const opened = await publishRecentToSpeleoDb('/p/local.stalx', {
      loadProject: vi.fn(async () => ({ success: false })),
      openDialog,
    });
    expect(opened).toBe(false);
    expect(openDialog).not.toHaveBeenCalled();
  });
});

/** Mirrors App.tsx: the dialog mounts as a RESULT of `openDialog`. */
function DialogHost() {
  const activeDialog = useUIStore((s) => s.activeDialog);
  return activeDialog === 'publish-to-speleodb' ? <PublishToSpeleoDbDialog /> : null;
}

describe('the dialog prefills from the NEWLY opened project', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    useUIStore.setState({ activeDialog: null, dialogData: {} });
    useProjectStore.setState({ project: { name: 'Previously Open', zero_datums: [] } as never });
  });

  /** Fake open: commits the new project to the store, like the real loadProject. */
  const openNewProject = async () => {
    useProjectStore.setState({ project: { name: 'Newly Opened', zero_datums: [] } as never });
    return { success: true };
  };

  it('shows the newly opened project name, not the previously open one', async () => {
    render(<DialogHost />);
    // A load that stays in flight, so React gets a chance to paint mid-open: a
    // reversed implementation would mount the dialog HERE, against the old
    // project, and both assertions below would catch it.
    let settle!: (r: { success: boolean }) => void;
    let done!: Promise<boolean>;
    await act(async () => {
      done = publishRecentToSpeleoDb('/p/local.stalx', {
        loadProject: () => new Promise<{ success: boolean }>((res) => { settle = res; }),
        openDialog: (d) => useUIStore.getState().openDialog(d),
      });
    });
    expect(screen.queryByTestId('publish-to-speleodb-dialog-name')).toBeNull();

    await act(async () => {
      await openNewProject();
      settle({ success: true });
      await done;
    });
    expect(screen.getByTestId('publish-to-speleodb-dialog-name')).toHaveValue('Newly Opened');
  });

  it('control: raising the dialog BEFORE the open prefills the wrong project', async () => {
    // Same inputs, reversed order - this is what the guarded sequencing prevents,
    // and it proves the assertion above is not vacuous.
    render(<DialogHost />);
    // Separate acts, because that is exactly the real hazard: the dialog gets to
    // MOUNT (and run its prefill initializers) while the open is still in flight.
    await act(async () => { useUIStore.getState().openDialog('publish-to-speleodb'); });
    await act(async () => { await openNewProject(); });
    expect(screen.getByTestId('publish-to-speleodb-dialog-name')).toHaveValue('Previously Open');
  });
});
