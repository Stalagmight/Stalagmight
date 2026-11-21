/**
 * The open FILE's status on the `.stalx` v2 model (plan 6.4, 6.5, 7.2.10):
 * - a file made with a newer Stalagmight is read-only, and the editing gate
 *   says so (`'project'`);
 * - the old-format conversion notice is taken once and kept until dismissed;
 * - a warning ignore rides the save, so it dirties the project.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { editingBlockedReasonNow, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { FILE_STATUS_CLEARED } from '../projectFileStatusActions';

const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: ipc }));

beforeEach(() => {
  ipc.mockReset();
  vi.spyOn(console, 'error').mockImplementation(() => {});
  useProjectStore.setState({
    ...FILE_STATUS_CLEARED,
    project: { file_path: '/a.stalx', created_at: 't0' } as never,
    isDirty: false,
  });
  // A confirmed-local project in Survey: nothing else blocks editing.
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' } as never);
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' } as never);
});

describe('checkFileStatus', () => {
  it('reads why a file is read-only and blocks editing on it', async () => {
    ipc.mockImplementation(async (command: string) =>
      command === 'project_read_only_reason' ? 'made with a newer Stalagmight; it requires features this version does not know: lofts' : null,
    );
    expect(isEditingBlockedNow()).toBe(false);

    await useProjectStore.getState().checkFileStatus();

    expect(useProjectStore.getState().fileReadOnlyReason).toContain('lofts');
    expect(isEditingBlockedNow()).toBe(true);
    expect(editingBlockedReasonNow()).toBe('project');
  });

  it('keeps the conversion notice a second (StrictMode) check cannot take again', async () => {
    let notice: string | null = 'Converted from the old file format; 3 stations moved by up to 0.42 m. The new format is written the first time the project is saved, or already was if you renamed it in Recent Projects or removed its SpeleoDB link; the original file is kept: to go back to it, right-click the project in Recent Projects and choose Restore an earlier copy.';
    ipc.mockImplementation(async (command: string) => {
      if (command !== 'take_conversion_notice') return null;
      const taken = notice;
      notice = null;
      return taken;
    });

    await useProjectStore.getState().checkFileStatus();
    await useProjectStore.getState().checkFileStatus();

    expect(useProjectStore.getState().conversionNotice).toContain('3 stations moved');
    useProjectStore.getState().dismissConversionNotice();
    expect(useProjectStore.getState().conversionNotice).toBeNull();
  });

  it("does not paint one file's status over the next one opened meanwhile", async () => {
    let release: (v: string) => void = () => {};
    ipc.mockImplementation((command: string) =>
      command === 'project_read_only_reason' ? new Promise<string>((r) => (release = r)) : Promise.resolve(null),
    );
    const pending = useProjectStore.getState().checkFileStatus();
    useProjectStore.setState({ project: { file_path: '/b.stalx', created_at: 't1' } as never });
    release('made with a newer Stalagmight');
    await pending;

    expect(useProjectStore.getState().fileReadOnlyReason).toBeNull();
  });
});

describe('a warning ignore rides the save', () => {
  it('dirties the project when the backend says the ignore is unsaved', async () => {
    ipc.mockImplementation(async (command: string) => {
      if (command === 'has_unsaved_changes') return true;
      if (command === 'get_project_warnings') return [];
      return undefined;
    });

    await useProjectStore.getState().resolveWarning(1, 100000, 'LONG_SHOT');

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(ipc).toHaveBeenCalledWith('has_unsaved_changes', undefined, expect.anything());
  });

  it('leaves a clean project clean when the backend has nothing unsaved', async () => {
    ipc.mockImplementation(async (command: string) => {
      if (command === 'has_unsaved_changes') return false;
      if (command === 'get_project_warnings') return [];
      return undefined;
    });

    await useProjectStore.getState().unresolveWarning(1, 100000, 'LONG_SHOT');

    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('assumes unsaved when the flag cannot be read', async () => {
    ipc.mockImplementation(async (command: string) => {
      if (command === 'has_unsaved_changes') throw new Error('gone');
      if (command === 'get_project_warnings') return [];
      return undefined;
    });

    await useProjectStore.getState().resolveWarning(1, 100000, 'LONG_SHOT');

    expect(useProjectStore.getState().isDirty).toBe(true);
  });
});
