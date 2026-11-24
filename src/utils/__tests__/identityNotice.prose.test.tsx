/**
 * RS-PROSE: the identity notice names each station as a prose token, so the
 * `station-identities` toast draws a pill for it, while the live region (and
 * the dismiss label) read the sentence plain text wrote. The burst join of
 * `notifyIdentityChanges` (a later notice joins a fresh one, the same words
 * twice are said once) works on the token strings as it did on plain ones.
 *
 * Injections: wording the station with `formatStationLabel` again fails "holds
 * tokens" and "draws a pill"; announcing `item.message` raw fails "reads
 * plain"; comparing the join on plain words of one side only fails "said once".
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import '../../i18n';
import { ToastStack } from '../../components/common/ToastStack';
import { useUIStore } from '../../store/uiStore';
import { identityNoticeMessage, notifyIdentityChanges } from '../identityNotice';
import { hasProse, proseParts, proseToPlain } from '../prose';
import { accessibleText } from '../../test/accessibleText';
import { TOAST_KEYS } from '../toastKeys';
import type { CommandResult } from '../../types/commands';
import type { IdentityNotice } from '../../types/ipc/IdentityNotice';

const ANNEX = 1;
const TRUNK = 2;
const lineName = (id: number) => (id === ANNEX ? 'Annex' : 'Trunk');

const notice = (over: Partial<IdentityNotice>): IdentityNotice => ({
  lettered: [],
  restored: [],
  unarchived: [],
  hidden: [],
  ...over,
});

const result = (identities: IdentityNotice): CommandResult =>
  ({ success: true, warnings: [], error: null, line_ids: [], identities }) as CommandResult;

const lettered = (from: string, to: string) =>
  notice({ lettered: [{ station_id: 41, line_id: TRUNK, other_id: 7, from, to }] });

const PLAIN_LETTER = 'Renamed Trunk / A7 to A7a so no line shows a name twice.';

const notices = () =>
  useUIStore.getState().toasts.filter((t) => t.key === TOAST_KEYS.STATION_IDENTITIES && !t.leaving);

beforeEach(() => {
  vi.useFakeTimers();
  useUIStore.setState({ toasts: [], syntacticLevel: 'full' });
});
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  useUIStore.setState({ toasts: [] });
});

describe('the identity notice as prose (RS-PROSE)', () => {
  it('names each station as a token whose words are the plain label', () => {
    const message = identityNoticeMessage(
      notice({
        lettered: [{ station_id: 41, line_id: TRUNK, other_id: 7, from: 'A7', to: 'A7a' }],
        restored: [
          {
            old_id: 54,
            old_name: 'B3',
            old_home_line: ANNEX,
            station: { id: 55, name: 'B0', home_line: TRUNK },
            comment_moved: false,
            comment_hidden: 'Low air',
          },
        ],
        hidden: [{ id: 28, name: 'B9', home_line: ANNEX, comment: 'Sump' }],
      }),
      lineName,
    )!;
    expect(hasProse(message)).toBe(true);
    const stations = proseParts(message).filter((p) => p.kind === 'station');
    expect(stations.map((p) => p.text)).toEqual(['Trunk / A7', 'Trunk / B0', 'Annex / B3', 'Annex / B9']);
    // Coloured by the home line, with the Station ID; the restored station's
    // sentence says its Station ID in words, so its pill draws none.
    expect(stations.map((p) => (p.kind === 'station' ? [p.line_id, p.station_id] : null))).toEqual([
      [TRUNK, 41],
      [TRUNK, undefined],
      [ANNEX, 54],
      [ANNEX, 28],
    ]);
    expect(proseToPlain(message)).toBe(
      'Renamed Trunk / A7 to A7a so no line shows a name twice. ' +
        'Trunk / B0 (Station ID 55) is shown again. ' +
        'Annex / B3\'s comment ("Low air") is hidden: Undo brings it back. ' +
        'Annex / B9\'s comment ("Sump") is hidden: Undo brings it back.',
    );
  });

  it('draws the station as a pill in the toast and reads plain in the live region', () => {
    render(<ToastStack />);
    act(() => {
      notifyIdentityChanges(result(lettered('A7', 'A7a')), lineName);
    });
    const toast = screen.getByTestId('toast');
    const pill = toast.querySelector('.sh-ppill');
    expect(pill).not.toBeNull();
    expect(pill!.getAttribute('data-prose-kind')).toBe('station');
    expect(pill!.querySelector('[aria-hidden="true"]')!.textContent).toBe('Trunk: A7#41');
    // The visible message is aria-hidden (the live regions announce it); its
    // words, read without the drawing, are the plain sentence.
    const message = pill!.closest('span.text-sm')!;
    expect(message.textContent).not.toMatch(/[\u0001\u0002]/);
    const sr = [...message.querySelectorAll('.sr-only')].map((el) => el.textContent);
    expect(sr).toEqual(['Trunk / A7']);
    // A warning is announced assertively, in plain words, with no marker.
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toBe(PLAIN_LETTER);
    expect(accessibleText(alert)).toBe(PLAIN_LETTER);
    // The dismiss button names the plain message too.
    expect(toast.querySelector('button')!.getAttribute('aria-label')).toContain(PLAIN_LETTER);
  });

  it('joins a second notice in the burst window and says the same words once', () => {
    notifyIdentityChanges(result(lettered('A7', 'A7a')), lineName);
    notifyIdentityChanges(result(lettered('A8', 'A8a')), lineName);
    expect(notices()).toHaveLength(1);
    const joined = notices()[0]!.message;
    expect(proseParts(joined).filter((p) => p.kind === 'station')).toHaveLength(2);
    expect(proseToPlain(joined)).toBe(
      `${PLAIN_LETTER} Renamed Trunk / A8 to A8a so no line shows a name twice.`,
    );
    // The first notice again: already in the joined message, so not repeated.
    notifyIdentityChanges(result(lettered('A7', 'A7a')), lineName);
    expect(notices()).toHaveLength(1);
    expect(notices()[0]!.message).toBe(joined);
  });
});
