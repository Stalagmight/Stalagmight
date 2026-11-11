/**
 * #674: the post-add sequence reuses the existing import tail - dirty via
 * saveAfterImport, orphan placement keyed on `section_ids`, receipt via
 * finishImportWithoutOrphanSuggestions - and drops that tail when the project it
 * was issued against is no longer the open one.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const invokeIpc = vi.fn();
vi.mock('../ipc', () => ({ invokeIpc: (...a: unknown[]) => invokeIpc(...a), getErrorMessage: (e: unknown) => String(e) }));

const finish = vi.fn();
const finalizeDeferred = vi.fn();
vi.mock('../saveFeedback', () => ({
  finishImportWithoutOrphanSuggestions: (...a: unknown[]) => finish(...a),
  finalizeDeferredSaveWarnings: (...a: unknown[]) => finalizeDeferred(...a),
}));

import { runAddProjectFile } from '../addProjectFile';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

const addProjectFile = vi.fn();
const saveAfterImport = vi.fn();
const receipt = { source_format: 'x', warnings: [], info: [] };

beforeEach(() => {
  vi.clearAllMocks();
  saveAfterImport.mockResolvedValue(true);
  finalizeDeferred.mockReturnValue(false);
  useProjectStore.setState({ project: { created_at: 'cave-a' }, addProjectFile, saveAfterImport } as never);
  useUIStore.setState({ activeDialog: null, dialogData: {}, importContext: null });
});

describe('runAddProjectFile', () => {
  it('stops at a failed command: no save, no orphan lookup, context dropped', async () => {
    addProjectFile.mockResolvedValue({ success: false, section_ids: [] });
    await runAddProjectFile('/cave/a.stalx');
    expect(saveAfterImport).not.toHaveBeenCalled();
    expect(invokeIpc).not.toHaveBeenCalled();
    expect(finish).not.toHaveBeenCalled();
    expect(useUIStore.getState().importContext).toBeNull();
  });

  it('opens orphan placement with the receipt when suggestions exist for the new section ids', async () => {
    addProjectFile.mockResolvedValue({ success: true, section_ids: [7, 8], import_receipt: receipt });
    invokeIpc.mockResolvedValue([{ orphanSectionId: 7 }]);
    await runAddProjectFile('/cave/a.stalx');
    expect(invokeIpc).toHaveBeenCalledWith(
      'suggest_orphan_placements',
      { newlyImportedSectionIds: [7, 8] },
      expect.anything(),
    );
    expect(useUIStore.getState().activeDialog).toBe('orphan-placement');
    expect(useUIStore.getState().dialogData).toMatchObject({ receipt });
    expect(finish).not.toHaveBeenCalled();
  });

  it('finishes with the receipt when there are no suggestions', async () => {
    addProjectFile.mockResolvedValue({ success: true, section_ids: [7], import_receipt: receipt });
    invokeIpc.mockResolvedValue([]);
    await runAddProjectFile('/cave/a.stalx');
    expect(finish).toHaveBeenCalledWith('add-or-open-project', expect.anything(), [7], receipt, expect.any(Function));
  });

  it('skips the orphan lookup when the add created no sections', async () => {
    addProjectFile.mockResolvedValue({ success: true, section_ids: [], import_receipt: receipt });
    await runAddProjectFile('/cave/a.stalx');
    expect(invokeIpc).not.toHaveBeenCalled();
    expect(finish).toHaveBeenCalledWith('add-or-open-project', expect.anything(), [], receipt, expect.any(Function));
  });

  it('a failed orphan lookup is non-fatal and still finishes', async () => {
    addProjectFile.mockResolvedValue({ success: true, section_ids: [7], import_receipt: receipt });
    invokeIpc.mockRejectedValue(new Error('boom'));
    await runAddProjectFile('/cave/a.stalx');
    expect(finish).toHaveBeenCalledTimes(1);
  });

  it('a transport rejection comes back as a failed result, context dropped', async () => {
    addProjectFile.mockRejectedValue(new Error('ipc gone'));
    const result = await runAddProjectFile('/cave/a.stalx');
    expect(result.success).toBe(false);
    expect(result.error).toContain('ipc gone');
    expect(saveAfterImport).not.toHaveBeenCalled();
    expect(useUIStore.getState().importContext).toBeNull();
  });

  it('stops after reporting a failed deferred save, like every other import route', async () => {
    addProjectFile.mockResolvedValue({ success: true, section_ids: [7], import_receipt: receipt, warnings: [] });
    finalizeDeferred.mockReturnValue(true);
    await runAddProjectFile('/cave/a.stalx');
    expect(finalizeDeferred).toHaveBeenCalledWith([], 'add-or-open-project', expect.anything(), receipt);
    expect(saveAfterImport).not.toHaveBeenCalled();
    expect(invokeIpc).not.toHaveBeenCalled();
    expect(finish).not.toHaveBeenCalled();
  });

  /**
   * The add copies and loads a whole project, and its tail writes save state and
   * raises orphan placement; none of it may land on a project opened meanwhile.
   *
   * Injection: drop the `stillOpen()` check after `addProjectFile` and
   * `saveAfterImport` plus the orphan lookup run against the replacement.
   */
  it('drops the tail when the project changed while the add ran', async () => {
    addProjectFile.mockImplementation(async () => {
      useProjectStore.setState({ project: { created_at: 'cave-b' } } as never);
      return { success: true, section_ids: [7], import_receipt: receipt };
    });
    await runAddProjectFile('/cave/a.stalx');
    expect(saveAfterImport).not.toHaveBeenCalled();
    expect(invokeIpc).not.toHaveBeenCalled();
    expect(finish).not.toHaveBeenCalled();
    expect(useUIStore.getState().importContext).toBeNull();
  });

  /** Injection: drop the `stillOpen()` check after the orphan lookup and the
   *  placement dialog opens over the replacement project. */
  it('drops orphan placement when the project changed during the lookup', async () => {
    addProjectFile.mockResolvedValue({ success: true, section_ids: [7], import_receipt: receipt });
    invokeIpc.mockImplementation(async () => {
      useProjectStore.setState({ project: null } as never);
      return [{ orphanSectionId: 7 }];
    });
    await runAddProjectFile('/cave/a.stalx');
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(finish).not.toHaveBeenCalled();
    expect(useUIStore.getState().importContext).toBeNull();
  });

  /** The final helper awaits its own count IPC; the add hands it the identity
   *  predicate so a switch during THAT await drops the completion too. */
  it('hands the finish helper a predicate that goes false when the project changes', async () => {
    addProjectFile.mockResolvedValue({ success: true, section_ids: [7], import_receipt: receipt });
    invokeIpc.mockResolvedValue([]);
    await runAddProjectFile('/cave/a.stalx');
    const stillCurrent = finish.mock.calls[0]?.[4] as () => boolean;
    useUIStore.getState().setImportContext('existing-project');
    expect(stillCurrent()).toBe(true);
    useProjectStore.setState({ project: { created_at: 'cave-b' } } as never);
    expect(stillCurrent()).toBe(false);
    expect(useUIStore.getState().importContext).toBeNull();
  });
});
