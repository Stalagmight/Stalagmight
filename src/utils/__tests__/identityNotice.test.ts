/**
 * RS 7 "Notices": what an action did to station names on its own (rule L
 * letters, restored archived stations, closure ends that show their own name
 * again) raises ONE notice per action naming each, under one dedup key. A
 * failed action and an action that did nothing to names raise none.
 * Injection: toasting per letter fails "one notice per action".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import '../../i18n';
import { useUIStore } from '../../store/uiStore';
import { identityNoticeMessage, notifyIdentityChanges } from '../identityNotice';
import { TOAST_KEYS } from '../toastKeys';
import type { CommandResult } from '../../types/commands';

const lineName = (id: number) => (id === 1 ? 'Main' : 'Side');
const showToast = vi.fn();

const result = (identities: CommandResult['identities'], success = true): CommandResult =>
  ({ success, warnings: [], error: null, line_ids: [], identities }) as CommandResult;

beforeEach(() => {
  showToast.mockReset();
  useUIStore.setState({ showToast } as never);
});

describe('the identity notice', () => {
  it('names every letter, restore and unarchived closure in one message', () => {
    const message = identityNoticeMessage(
      {
        lettered: [
          { station_id: 41, line_id: 2, other_id: 12, from: '12', to: '12a' },
          { station_id: 42, line_id: 2, other_id: 13, from: '13', to: '13a' },
        ],
        restored: [{ old_id: 11, station: { id: 40, name: '0', home_line: 2 }, comment_moved: false, comment_hidden: 'Silt' }],
        unarchived: [5n],
      },
      lineName,
    );
    expect(message).toBe(
      'Renamed Side / 12 to 12a, Side / 13 to 13a so no line shows a name twice. ' +
        'Side / 0 (Station ID 40) is shown again. ' +
        'The comment of Station ID 11 is hidden: Undo brings it back. ' +
        "A loop closure's hidden end shows its own name again.",
    );
  });

  it('says nothing for an action that did nothing to names', () => {
    expect(identityNoticeMessage(undefined, lineName)).toBeNull();
    expect(identityNoticeMessage({ lettered: [], restored: [], unarchived: [] }, lineName)).toBeNull();
  });

  it('raises one warning toast per action, under its key', () => {
    notifyIdentityChanges(
      result({ lettered: [{ station_id: 41, line_id: 2, other_id: 12, from: '12', to: '12a' }], restored: [], unarchived: [] }),
      lineName,
    );
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      'Renamed Side / 12 to 12a so no line shows a name twice.',
      TOAST_KEYS.STATION_IDENTITIES,
      'warning',
    );
  });

  it('raises nothing for a failed action', () => {
    notifyIdentityChanges(
      result({ lettered: [{ station_id: 41, line_id: 2, other_id: 12, from: '12', to: '12a' }], restored: [], unarchived: [] }, false),
      lineName,
    );
    expect(showToast).not.toHaveBeenCalled();
  });
});
