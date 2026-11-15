/**
 * #746: Combine Sections removes the ties it would leave as loop closures
 * across one shot, and the store action tells the user once, through the
 * shared removed-closure toast, passing the command result through.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createSectionStructureActions } from '../sectionStructureActions';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { CLOSURE_REMOVED_ACROSS_ONE_SHOT } from '../../utils/closureRemovalToast';
import type { CommandResult } from '../../types/commands';
import type { CombineSectionsOptions } from '../../types/ipc/CombineSectionsOptions';

const options: CombineSectionsOptions = {
  name: 'Joined',
  color: '#abcdef',
  swap_lr: true,
  comment_pick: 'kept',
  name_pick: 'kept',
};

const message =
  'Removed the merge of 1:1 and 2:0: combining the sections left its two stations at the two ends of one shot.';

describe('combineSections', () => {
  const showToast = vi.fn();
  beforeEach(() => {
    showToast.mockReset();
    useUIStore.setState({ showToast } as never);
  });

  it('runs combine_sections and toasts the ties it removed across one shot', async () => {
    const result = {
      success: true,
      section_id: 0,
      warnings: [{ code: CLOSURE_REMOVED_ACROSS_ONE_SHOT, message, severity: 'warning' }],
    } as unknown as CommandResult;
    const execute = vi.fn().mockResolvedValue(result);
    const { combineSections } = createSectionStructureActions(execute);

    await expect(combineSections(0, 2, options)).resolves.toBe(result);
    expect(execute).toHaveBeenCalledWith(
      'combine_sections',
      { kept: 0, other: 2, options },
      { purpose: 'combine sections', fields: { kept: 0, other: 2 } }
    );
    expect(showToast).toHaveBeenCalledWith(message, TOAST_KEYS.CLOSURE_REMOVED, 'warning');
  });

  it('stays silent for a combine that removed nothing', async () => {
    const execute = vi.fn().mockResolvedValue({ success: true, warnings: [] } as unknown as CommandResult);
    const { combineSections } = createSectionStructureActions(execute);
    await combineSections(0, 2, options);
    expect(showToast).not.toHaveBeenCalled();
  });
});
