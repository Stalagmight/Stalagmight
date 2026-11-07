/**
 * DRAFT-1 (#576): the map comment overlay keeps what the user typed when the
 * save fails.
 *
 * `commit` awaited updateStationComment / updateShot and then closed the
 * overlay whatever the CommandResult said, so a refused save (a backend
 * rejection, a read-only gate, or GHOST-2's refusal while a move is carried)
 * discarded the comment: the failure notice appeared and the text was gone.
 * A failed save now leaves the overlay open with the text, so Enter or the
 * check button retries; Escape still discards. Success is unchanged.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc');
invokeIpcMock.mockImplementation(() => Promise.resolve(undefined));

import CommentOverlay from '../CommentOverlay';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore, type CommentTarget } from '../../../store/uiStore';
import type { CommandResult } from '../../../types/commands';
import { invokeIpcMock } from '../../../test/factories/ipc';

const result = (success: boolean): CommandResult => ({
  success,
  warnings: [],
  error: success ? null : 'refused',
  section_ids: [],
});

const stationTarget: CommentTarget = {
  type: 'station',
  stationRef: { section_id: 1, station_index: 2 },
  screenPos: { x: 100, y: 100 },
  currentComment: '',
};

const shotTarget: CommentTarget = {
  type: 'shot',
  sectionId: 1,
  shotId: 3,
  screenPos: { x: 100, y: 100 },
  currentComment: 'old',
};

let updateStationComment: ReturnType<typeof vi.fn>;
let updateShot: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateStationComment = vi.fn();
  updateShot = vi.fn();
  useProjectStore.setState({ updateStationComment, updateShot });
  useUIStore.setState({ commentTarget: null });
});

async function openAndType(target: CommentTarget, text: string) {
  render(<CommentOverlay />);
  act(() => useUIStore.getState().setCommentTarget(target));
  const textarea = screen.getByTestId('comment-overlay-textarea');
  fireEvent.change(textarea, { target: { value: text } });
  return textarea as HTMLTextAreaElement;
}

async function pressEnter(textarea: HTMLElement) {
  await act(async () => {
    fireEvent.keyDown(textarea, { key: 'Enter' });
    await Promise.resolve();
  });
}

describe('CommentOverlay save failure (DRAFT-1)', () => {
  it('keeps the overlay open with the typed station comment when the save is refused', async () => {
    updateStationComment.mockResolvedValue(result(false));
    const textarea = await openAndType(stationTarget, 'Squeeze at the restriction');

    await pressEnter(textarea);

    expect(updateStationComment).toHaveBeenCalledWith(stationTarget.stationRef, 'Squeeze at the restriction');
    expect(useUIStore.getState().commentTarget).not.toBeNull();
    expect((screen.getByTestId('comment-overlay-textarea') as HTMLTextAreaElement).value).toBe(
      'Squeeze at the restriction'
    );
  });

  it('keeps the typed shot comment too, and a retry that succeeds closes the overlay', async () => {
    updateShot.mockResolvedValue(result(false));
    const textarea = await openAndType(shotTarget, 'Low airspace');

    await pressEnter(textarea);
    expect(useUIStore.getState().commentTarget).not.toBeNull();
    expect((screen.getByTestId('comment-overlay-textarea') as HTMLTextAreaElement).value).toBe('Low airspace');

    updateShot.mockResolvedValue(result(true));
    await pressEnter(screen.getByTestId('comment-overlay-textarea'));
    expect(updateShot).toHaveBeenCalledTimes(2);
    expect(updateShot).toHaveBeenLastCalledWith(1, 3, { comment: 'Low airspace' });
    expect(useUIStore.getState().commentTarget).toBeNull();
    expect(screen.queryByTestId('comment-overlay')).toBeNull();
  });

  it('closes the overlay after a successful save, as before', async () => {
    updateStationComment.mockResolvedValue(result(true));
    const textarea = await openAndType(stationTarget, 'Restriction');

    await pressEnter(textarea);

    expect(useUIStore.getState().commentTarget).toBeNull();
    expect(screen.queryByTestId('comment-overlay')).toBeNull();
  });
});
