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
import type { ConversionNoticeView } from '../../types/ipc/ConversionNoticeView';

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
    let notice: ConversionNoticeView | null = {
      changes: ['3 stations moved by up to 0.42 m'],
      details: [],
      file: 'not_yet_saved',
    };
    ipc.mockImplementation(async (command: string) => {
      if (command !== 'take_conversion_notice') return null;
      const taken = notice;
      notice = null;
      return taken;
    });

    await useProjectStore.getState().checkFileStatus();
    await useProjectStore.getState().checkFileStatus();

    expect(useProjectStore.getState().conversionNotice?.changes).toEqual(['3 stations moved by up to 0.42 m']);
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

/**
 * The save that converts a project opened from a 0.6.0 file (demo issue I-5):
 * the banner stops promising that the next save will, and says it was saved;
 * with no banner showing, a toast says so instead (one event, one surface).
 * Injection: drop the `legacy_converted` branch from the save action and the
 * banner keeps "the next time you save" after the save, and no toast shows.
 */
describe('the converting save', () => {
  const notice: ConversionNoticeView = {
    changes: ['1 station moved by 0.40 m'],
    details: [],
    file: 'not_yet_saved',
  };
  const saving = (legacy_converted?: 'saved' | 'saved_as_new_file') =>
    ipc.mockImplementation(async (command: string) =>
      command === 'save_project'
        ? { success: true, saved_path: '/a.stalx', warnings: [], error: null, line_ids: [], legacy_converted }
        : null,
    );

  it('turns the banner to saved, with no toast', async () => {
    const toast = vi.spyOn(useUIStore.getState(), 'showToast');
    useProjectStore.setState({ conversionNotice: notice });
    saving('saved');
    await useProjectStore.getState().saveProject();
    expect(useProjectStore.getState().conversionNotice?.file).toBe('saved');
    expect(toast).not.toHaveBeenCalledWith(expect.anything(), 'legacy-converted', expect.anything());
  });

  it('says a Save As elsewhere left the original as it was', async () => {
    useProjectStore.setState({ conversionNotice: notice });
    saving('saved_as_new_file');
    await useProjectStore.getState().saveProject('/b.stalx');
    expect(useProjectStore.getState().conversionNotice?.file).toBe('saved_as_new_file');
  });

  it('confirms with a toast when no banner shows', async () => {
    const toast = vi.fn();
    useUIStore.setState({ showToast: toast } as never);
    saving('saved');
    await useProjectStore.getState().saveProject();
    expect(useProjectStore.getState().conversionNotice).toBeNull();
    expect(toast).toHaveBeenCalledWith(
      'Saved in the new format; the original is kept in Recent Projects',
      'legacy-converted',
      'success',
    );
  });

  it('says nothing for a save that converted nothing', async () => {
    const toast = vi.fn();
    useUIStore.setState({ showToast: toast } as never);
    useProjectStore.setState({ conversionNotice: { ...notice, file: 'saved' } });
    saving(undefined);
    await useProjectStore.getState().saveProject();
    expect(useProjectStore.getState().conversionNotice?.file).toBe('saved');
    expect(toast).not.toHaveBeenCalled();
  });
});

/**
 * Demo issue N-6: the status bar said "Saved" while the file on disk was still
 * the 0.6.0 file, including for a conversion that raised no notice at all. The
 * file status now reads whether the file is still a 0.6.0 file (a peek), and
 * the converting save (to the file or a Save As elsewhere) clears it.
 * Injection: dropping the `project_file_is_legacy` read from checkFileStatus,
 * or the clear from noteLegacyConverted, fails these.
 */
describe('the old format on disk', () => {
  const legacyOpen = (legacy: boolean) =>
    ipc.mockImplementation(async (command: string) => {
      if (command === 'project_file_is_legacy') return legacy;
      if (command === 'save_project') {
        return { success: true, saved_path: '/a.stalx', warnings: [], error: null, line_ids: [], legacy_converted: 'saved' };
      }
      return null;
    });

  it('is read with the file status, even when the conversion raised no notice', async () => {
    legacyOpen(true);
    await useProjectStore.getState().checkFileStatus();
    expect(useProjectStore.getState().legacyFileOnDisk).toBe(true);
    expect(useProjectStore.getState().conversionNotice).toBeNull();
    expect(ipc).toHaveBeenCalledWith('project_file_is_legacy', undefined, expect.anything());
  });

  it('is false for a version 2 file', async () => {
    legacyOpen(false);
    await useProjectStore.getState().checkFileStatus();
    expect(useProjectStore.getState().legacyFileOnDisk).toBe(false);
  });

  it('ends with the converting save', async () => {
    legacyOpen(true);
    await useProjectStore.getState().checkFileStatus();
    await useProjectStore.getState().saveProject();
    expect(useProjectStore.getState().legacyFileOnDisk).toBe(false);
  });

  it('is cleared with the rest of the file status on load, create and close', () => {
    expect(FILE_STATUS_CLEARED.legacyFileOnDisk).toBe(false);
  });
});
