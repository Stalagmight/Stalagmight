/**
 * #737: a delete with shift that removed loop closures (they now span a single
 * shot) tells the user once, in the backend's words, as a warning toast, and
 * passes the command result through untouched. #738: so does an edit whose
 * orphan sweep removed closures between merged stations, in the same toast.
 * #746: and Combine Lines, for the ties it removed across one shot.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  CLOSURE_REMOVED_ACROSS_ONE_SHOT,
  CLOSURE_REMOVED_MERGED_ENDS,
  notifyRemovedClosures,
} from '../closureRemovalToast';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../toastKeys';
import type { CommandResult } from '../../types/commands';
import type { ProsePart } from '../../types/ipc/ProsePart';
import { partsToPlain, proseParts, proseToPlain } from '../prose';

const removed = (message: string) => ({
  code: CLOSURE_REMOVED_ACROSS_ONE_SHOT,
  message,
  severity: 'warning' as const,
});

function result(over: Partial<CommandResult>): CommandResult {
  return { success: true, warnings: [], error: null, line_ids: [], ...over } as CommandResult;
}

describe('notifyRemovedClosures', () => {
  const showToast = vi.fn();
  beforeEach(() => {
    showToast.mockReset();
    useUIStore.setState({ showToast } as never);
  });

  it('shows one warning toast naming every removed closure', () => {
    const r = result({
      warnings: [
        removed('Removed loop closure 0:1 to 0:5: closing the gap left its two stations at the two ends of one shot.'),
        { code: 'SYSTEM', message: 'unrelated', severity: 'warning' },
        removed('Removed loop closure 1:0 to 0:4: closing the gap left its two stations at the two ends of one shot.'),
      ],
    });
    expect(notifyRemovedClosures(r)).toBe(r);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      'Removed loop closure 0:1 to 0:5: closing the gap left its two stations at the two ends of one shot. ' +
        'Removed loop closure 1:0 to 0:4: closing the gap left its two stations at the two ends of one shot.',
      TOAST_KEYS.CLOSURE_REMOVED,
      'warning',
    );
  });

  it('shows closures removed between merged stations in the same toast (#738)', () => {
    const merged = (message: string) => ({ code: CLOSURE_REMOVED_MERGED_ENDS, message, severity: 'warning' as const });
    const r = result({
      warnings: [
        merged(
          'Removed loop closure 1:0 to 0:3: another loop closure became a merge, which made its two stations one point.'
        ),
        removed('Removed loop closure 0:1 to 0:5: closing the gap left its two stations at the two ends of one shot.'),
        merged('Removed loop closure 2:1 to 3:0: its two stations were already one point.'),
      ],
    });
    expect(notifyRemovedClosures(r)).toBe(r);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      'Removed loop closure 1:0 to 0:3: another loop closure became a merge, which made its two stations one point. ' +
        'Removed loop closure 0:1 to 0:5: closing the gap left its two stations at the two ends of one shot. ' +
        'Removed loop closure 2:1 to 3:0: its two stations were already one point.',
      TOAST_KEYS.CLOSURE_REMOVED,
      'warning',
    );
  });

  it('shows the ties a combine removed across one shot in one toast (#746)', () => {
    const r = result({
      warnings: [
        removed(
          'Removed the merge of Squeeze 1:1 and Beyond 2:0: combining the lines left its two stations at the two ends of one shot.'
        ),
        removed('Removed loop closure Main 0:1 to Beyond 2:0: combining the lines left its two stations at the two ends of one shot.'),
      ],
    });
    expect(notifyRemovedClosures(r)).toBe(r);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      'Removed the merge of Squeeze 1:1 and Beyond 2:0: combining the lines left its two stations at the two ends of one shot. ' +
        'Removed loop closure Main 0:1 to Beyond 2:0: combining the lines left its two stations at the two ends of one shot.',
      TOAST_KEYS.CLOSURE_REMOVED,
      'warning',
    );
  });

  it('names the stations a message carries as parts with prose tokens (RS-PROSE)', () => {
    const parts: ProsePart[] = [
      { kind: 'text', text: 'Removed loop closure ' },
      { kind: 'station', text: 'Main / 1', line_id: 1, context: 'Main', name: '1', station_id: 4 },
      { kind: 'text', text: ' to ' },
      { kind: 'station', text: 'Main / 5', line_id: 1, context: 'Main', name: '5', station_id: 8 },
      { kind: 'text', text: ': closing the gap left its two stations at the two ends of one shot.' },
    ];
    const message = partsToPlain(parts);
    const r = result({
      warnings: [
        { ...removed(message), parts },
        removed('Removed loop closure a station to a station: closing the gap left its two stations at the two ends of one shot.'),
      ],
    });
    notifyRemovedClosures(r);
    expect(showToast).toHaveBeenCalledTimes(1);
    const shown = showToast.mock.calls[0]![0] as string;
    // The toast draws the tokens as pills; its words are the messages, joined.
    expect(proseParts(shown).filter((p) => p.kind === 'station').map((p) => p.text)).toEqual(['Main / 1', 'Main / 5']);
    expect(proseToPlain(shown)).toBe(
      `${message} Removed loop closure a station to a station: closing the gap left its two stations at the two ends of one shot.`,
    );
  });

  it('stays silent when the delete removed no closure', () => {
    const r = result({ warnings: [{ code: 'SYSTEM', message: 'x', severity: 'warning' }] });
    expect(notifyRemovedClosures(r)).toBe(r);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('stays silent on a failed command, whatever it carries', () => {
    const r = result({ success: false, error: 'refused', warnings: [removed('Removed loop closure 0:1 to 0:2.')] });
    expect(notifyRemovedClosures(r)).toBe(r);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('tolerates a result without a warnings array (an older mock)', () => {
    const r = { success: true } as CommandResult;
    expect(notifyRemovedClosures(r)).toBe(r);
    expect(showToast).not.toHaveBeenCalled();
  });
});
