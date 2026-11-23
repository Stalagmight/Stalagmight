/**
 * A comment edit through a foreign slot or a loop end says whose station it
 * is first (RS 5.4, review finding ui-3), on the map too: the Comment tool's
 * overlay and the Move Comment tool, not only Edit Station and the data
 * table. A loop end's point is drawn and selectable, so clicking it with
 * either tool reaches the kept station's comment, which is on other lines.
 *
 * - The overlay commits a comment typed on such a slot through the
 *   whose-station prompt (`shown-identity-edit`, kind comment), not at once.
 * - Move Comment picks a comment up from, or drops one onto, such a slot only
 *   after the prompt; a native slot moves at once, as before.
 *
 * Injection (333e24ac5): the overlay called `set_station_comment` and Move
 * Comment picked up and moved the kept station's comment with no prompt, so
 * every "asks" case fails.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';
import type { MapMouseEvent } from 'maplibre-gl';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, fallback?: string) =>
    (e as { message?: string })?.message ?? fallback ?? 'unknown error',
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue({}) }));

import '../../i18n';
import { useMapTools } from '../useMapTools';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import CommentOverlay from '../../components/Map/CommentOverlay';
import ShownIdentityEditDialog from '../../components/Dialogs/ShownIdentityEditDialog';
import type { Station } from '../../types/project';

const MAIN = 1;
const SIDE = 2;
const slot = (line_id: number, station_index: number) => ({ line_id, station_index });
const LOOP_END = slot(SIDE, 3);
const PLAIN = slot(MAIN, 1);
const station = (ref: { line_id: number; station_index: number }, id: number, extra: Partial<Station> = {}): Station => ({
  station_ref: ref,
  coordinates: { x: ref.station_index * 10, y: ref.line_id, z: 0 },
  is_zero_datum: false,
  gps_coordinates: null,
  merged_with: [],
  id,
  name: String(id),
  ...extra,
});

const updateStationComment = vi.fn();
const moveComment = vi.fn();
const openDialog = vi.fn();
const clickEvent = { originalEvent: new MouseEvent('click') } as unknown as MapMouseEvent;

beforeEach(() => {
  vi.clearAllMocks();
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  updateStationComment.mockResolvedValue({ success: true });
  moveComment.mockResolvedValue({ success: true });
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
      ],
      file_path: '/a.stalx',
      created_at: 't',
      modified_at: 't',
    } as never,
    surveyData: null,
    stations: [
      station(slot(MAIN, 0), 10),
      station(PLAIN, 11, { comment: 'Plain note' }),
      station(slot(MAIN, 4), 14, { comment: 'Arch' }),
      // Side / 3 is a loop end shown as Main / 4: it shows Main / 4's comment.
      station(LOOP_END, 23, {
        name: '14',
        comment: 'Arch',
        shown: { id: 14, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 5n },
      }),
      station(slot(SIDE, 4), 24),
    ],
    stationsAdjusted: null,
    surveyDataAdjusted: null,
    updateStationComment,
    moveComment,
  } as never);
  useUIStore.setState({
    activeTool: 'comment',
    appMode: 'survey',
    geometryMode: 'measured',
    showStations: true,
    commentTarget: null,
    moveCommentSource: null,
    openDialog,
  } as never);
});

describe('the Comment tool on a loop end', () => {
  it('asks whose station it is before committing the comment', async () => {
    useUIStore.getState().setCommentTarget({
      type: 'station',
      stationRef: LOOP_END,
      screenPos: { x: 10, y: 10 },
      currentComment: 'Arch',
    });
    render(<CommentOverlay />);
    const box = await screen.findByRole('textbox');
    fireEvent.change(box, { target: { value: 'Arch, low' } });
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    expect(updateStationComment).not.toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledWith('shown-identity-edit', {
      stationRef: LOOP_END,
      kind: 'comment',
      value: 'Arch, low',
    });
    expect(useUIStore.getState().commentTarget).toBeNull();
  });

  it('commits at once on a native slot', async () => {
    useUIStore.getState().setCommentTarget({
      type: 'station',
      stationRef: PLAIN,
      screenPos: { x: 10, y: 10 },
      currentComment: 'Plain note',
    });
    render(<CommentOverlay />);
    const box = await screen.findByRole('textbox');
    fireEvent.change(box, { target: { value: 'Plain, wet' } });
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' });
    });
    expect(updateStationComment).toHaveBeenCalledWith(PLAIN, 'Plain, wet');
    expect(openDialog).not.toHaveBeenCalled();
  });
});

describe('the Move Comment tool on a loop end', () => {
  beforeEach(() => {
    useUIStore.setState({ activeTool: 'move_comment' } as never);
  });

  it('asks before picking the comment up from a loop end, and picks it up on going on', async () => {
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onStationClick?.(LOOP_END, clickEvent);
    });
    expect(useUIStore.getState().moveCommentSource).toBeNull();
    expect(openDialog).toHaveBeenCalledWith(
      'shown-identity-edit',
      expect.objectContaining({ stationRef: LOOP_END, kind: 'comment', onProceed: expect.any(Function) }),
    );
    const { onProceed } = openDialog.mock.calls[0]![1] as { onProceed: () => Promise<void> | void };
    await act(async () => {
      await onProceed();
    });
    expect(useUIStore.getState().moveCommentSource).toMatchObject({ type: 'station', stationRef: LOOP_END, comment: 'Arch' });
  });

  it('asks before dropping a comment onto a loop end, and moves it on going on', async () => {
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onStationClick?.(PLAIN, clickEvent);
    });
    expect(useUIStore.getState().moveCommentSource).toMatchObject({ stationRef: PLAIN });
    // The loop end shows no comment (the kept end has none), so the drop
    // is a plain move once the prompt is answered.
    act(() => {
      useProjectStore.setState({
        stations: useProjectStore
          .getState()
          .stations.map((s) =>
            s.station_ref.line_id === SIDE && s.station_ref.station_index === 3 ? { ...s, comment: null } : s,
          ),
      } as never);
    });
    await act(async () => {
      await result.current.onStationClick?.(LOOP_END, clickEvent);
    });
    expect(moveComment).not.toHaveBeenCalled();
    const { onProceed } = openDialog.mock.calls[openDialog.mock.calls.length - 1]![1] as { onProceed: () => Promise<void> | void };
    await act(async () => {
      await onProceed();
    });
    expect(moveComment).toHaveBeenCalledWith(
      { type: 'station', station_ref: PLAIN },
      { type: 'station', station_ref: LOOP_END },
      false,
    );
  });

  it('moves between native slots at once', async () => {
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onStationClick?.(PLAIN, clickEvent);
    });
    await act(async () => {
      await result.current.onStationClick?.(slot(SIDE, 4), clickEvent);
    });
    expect(openDialog).not.toHaveBeenCalled();
    expect(moveComment).toHaveBeenCalled();
  });

  it("the prompt's going on closes it and picks the comment up", async () => {
    invokeIpcMock.mockResolvedValue({
      station: LOOP_END,
      node: 23,
      shown: { id: 14, name: '14', home_line: MAIN },
      lines: [MAIN, SIDE],
      foreign: true,
      loop_closure: 5n,
    });
    const { result } = renderHook(() => useMapTools());
    await act(async () => {
      await result.current.onStationClick?.(LOOP_END, clickEvent);
    });
    const [dialog, data] = openDialog.mock.calls[0]!;
    const closeDialog = vi.fn();
    useUIStore.setState({ activeDialog: dialog, dialogData: data, closeDialog } as never);
    render(<ShownIdentityEditDialog />);
    expect(await screen.findByTestId('shown-identity-prompt-lead')).toHaveTextContent(
      'Shown as Main / 14 through a loop closure.',
    );
    await act(async () => {
      fireEvent.click(screen.getByTestId('shown-identity-prompt-proceed'));
    });
    expect(closeDialog).toHaveBeenCalled();
    expect(useUIStore.getState().moveCommentSource).toMatchObject({ stationRef: LOOP_END });
    expect(updateStationComment).not.toHaveBeenCalled();
  });
});
