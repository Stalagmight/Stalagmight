/**
 * RS 7 "Notices": what an action did to station names on its own (rule L
 * letters, restored archived stations, closure ends that show their own name
 * again, comments a promotion hid) raises ONE notice per action naming each,
 * under one dedup key. A failed action and an action that did nothing to
 * names raise none. Injection: toasting per letter fails "one notice per
 * action"; dropping the hidden loop fails "names every hidden comment".
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
        restored: [
          {
            old_id: 11,
            old_name: '3',
            old_home_line: 1,
            station: { id: 40, name: '0', home_line: 2 },
            comment_moved: false,
            comment_hidden: 'Silt',
          },
        ],
        unarchived: [5n],
        hidden: [],
      },
      lineName,
    );
    expect(message).toBe(
      'Renamed Side / 12 to 12a, Side / 13 to 13a so no line shows a name twice. ' +
        'Side / 0 (Station ID 40) is shown again. ' +
        'Main / 3\'s comment ("Silt") is hidden: Undo brings it back. ' +
        "A loop closure's hidden end shows its own name again.",
    );
  });

  it('says nothing for an action that did nothing to names', () => {
    expect(identityNoticeMessage(undefined, lineName)).toBeNull();
    expect(identityNoticeMessage({ lettered: [], restored: [], unarchived: [], hidden: [] }, lineName)).toBeNull();
  });

  it('names every hidden comment by the station that showed it (RS 12)', () => {
    const message = identityNoticeMessage(
      {
        lettered: [],
        restored: [],
        unarchived: [],
        hidden: [
          { id: 28, name: '3', home_line: 2, comment: 'Low air' },
          { id: 7, name: '0', home_line: 1, comment: 'Sump' },
        ],
      },
      lineName,
    );
    expect(message).toBe(
      'Side / 3\'s comment ("Low air") is hidden: Undo brings it back. ' +
        'Main / 0\'s comment ("Sump") is hidden: Undo brings it back.',
    );
  });

  // RS-FX6, P2-7: a restore's hidden comment was named "Station ID 54"
  // only; it is named by its line, its name and its text. A notice from an
  // older backend without the old name still says the Station ID.
  // Injection: name it by `old_id` again and the first expectation fails.
  it("names a restore's hidden comment by line, name and text", () => {
    const restored = (old_name: string) =>
      identityNoticeMessage(
        {
          lettered: [],
          restored: [
            {
              old_id: 54,
              old_name,
              old_home_line: 2,
              station: { id: 55, name: '0', home_line: 1 },
              comment_moved: false,
              comment_hidden: 'Annex end: belay on a flake',
            },
          ],
          unarchived: [],
          hidden: [],
        },
        lineName,
      );
    expect(restored('3')).toContain('Side / 3\'s comment ("Annex end: belay on a flake") is hidden: Undo brings it back.');
    expect(restored('')).toContain('Station ID 54\'s comment ("Annex end: belay on a flake") is hidden');
  });

  it('raises one warning toast per action, under its key', () => {
    notifyIdentityChanges(
      result({ lettered: [{ station_id: 41, line_id: 2, other_id: 12, from: '12', to: '12a' }], restored: [], unarchived: [], hidden: [] }),
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
      result({ lettered: [{ station_id: 41, line_id: 2, other_id: 12, from: '12', to: '12a' }], restored: [], unarchived: [], hidden: [] }, false),
      lineName,
    );
    expect(showToast).not.toHaveBeenCalled();
  });
});
