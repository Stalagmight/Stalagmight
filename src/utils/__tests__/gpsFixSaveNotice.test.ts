/**
 * #741: a save whose file holds GPS fixes (or stored accuracies) tells the
 * user, once per project per session, that older versions of the app drop
 * them when they save the file. The backend decides; this only shows it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  OLDER_BUILDS_DROP_GPS_FIXES,
  noticeOlderVersionsDropFixes,
  resetOlderVersionsNoticeForTests,
} from '../gpsFixSaveNotice';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../toastKeys';
import type { CommandResult } from '../../types/commands';

const holdsFixes = {
  code: OLDER_BUILDS_DROP_GPS_FIXES,
  message: 'Older versions of the app drop this project\'s GPS fixes and accuracies when they save it.',
  severity: 'warning' as const,
};

function result(over: Partial<CommandResult>): CommandResult {
  return { success: true, warnings: [], error: null, line_ids: [], ...over } as CommandResult;
}

describe('noticeOlderVersionsDropFixes', () => {
  const showToast = vi.fn();
  beforeEach(() => {
    showToast.mockReset();
    resetOlderVersionsNoticeForTests();
    useUIStore.setState({ showToast } as never);
  });

  it('shows one warning toast per project per session', () => {
    const saved = result({ warnings: [holdsFixes] });
    noticeOlderVersionsDropFixes(saved, 'A');
    noticeOlderVersionsDropFixes(saved, 'A');
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      "Older app versions drop this file's GPS fixes and accuracies on save",
      TOAST_KEYS.OLDER_VERSIONS_DROP_GPS_FIXES,
      'warning',
    );

    noticeOlderVersionsDropFixes(saved, 'B');
    expect(showToast).toHaveBeenCalledTimes(2);
  });

  it('says nothing when the save does not report fixes, failed, or has no project', () => {
    noticeOlderVersionsDropFixes(result({ warnings: [{ ...holdsFixes, code: 'UNDO_HISTORY_DROPPED' }] }), 'A');
    noticeOlderVersionsDropFixes({ success: true } as CommandResult, 'A');
    noticeOlderVersionsDropFixes(result({ success: false, warnings: [holdsFixes] }), 'A');
    noticeOlderVersionsDropFixes(result({ warnings: [holdsFixes] }), undefined);
    expect(showToast).not.toHaveBeenCalled();

    // None of those used up project A's one notice.
    noticeOlderVersionsDropFixes(result({ warnings: [holdsFixes] }), 'A');
    expect(showToast).toHaveBeenCalledTimes(1);
  });
});
