/**
 * The one-click kept-station actions re-check the read-only gate after their
 * preview's round trip (review finding ui-5). The app-mode gate (Plan,
 * Export, the Closures Computed View) lives only in the frontend, so a mode
 * switch during the preview must stop the edit: the helper acts on nothing,
 * says editing is disabled and reports 'failed'. The whose-station prompt's
 * Remove the closure and Detach buttons follow the gate too.
 *
 * Injection (333e24ac5): the helpers mutated after the preview without the
 * check, and the prompt's buttons were gated on `busy` only, so every case
 * fails there.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

const invokeIpc = vi.fn();
vi.mock('../ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../i18n';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { detachLineAsking, removeClosureAsking } from '../keptStationActions';
import { TOAST_KEYS } from '../toastKeys';
import { ShownIdentityPrompt } from '../../components/Dialogs/ShownIdentityPrompt';

const MAIN = { line_id: 1, station_index: 3 };
const SIDE = { line_id: 2, station_index: 0 };
const unmergeStationPair = vi.fn();
const removeLoopClosure = vi.fn();
const showToast = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  unmergeStationPair.mockResolvedValue({ success: true });
  removeLoopClosure.mockResolvedValue({ success: true });
  useProjectStore.setState({
    unmergeStationPair,
    removeLoopClosure,
    fileReadOnlyReason: null,
    project: {
      lines: [
        { id: 1, name: 'Main', shots: [] },
        { id: 2, name: 'Side', shots: [] },
      ],
      loop_closures: [{ id: 5n, station_a: MAIN, station_b: SIDE }],
    },
    surveyData: null,
    stations: [],
  } as never);
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', showToast } as never);
});

/** The preview answers after the user switched to Plan mode. */
function previewThenPlan(answer: unknown) {
  invokeIpc.mockImplementation(async () => {
    useUIStore.setState({ appMode: 'plan' } as never);
    return answer;
  });
}

describe('the one-click actions after their preview', () => {
  it('Detach does nothing once editing is blocked', async () => {
    previewThenPlan({
      name: '3',
      station_id: 60,
      restored: false,
      kept: false,
      lines: [],
      together: [],
      rest_choices: [],
      collisions: [],
    });
    await expect(detachLineAsking(MAIN, SIDE, [SIDE])).resolves.toBe('failed');
    expect(unmergeStationPair).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith('Editing is disabled in the current view.', TOAST_KEYS.EDIT_DISABLED, 'warning');
  });

  it('Detach does nothing once editing is blocked, also when its preview failed', async () => {
    invokeIpc.mockImplementation(async () => {
      useUIStore.setState({ appMode: 'export' } as never);
      throw new Error('gone');
    });
    await expect(detachLineAsking(MAIN, SIDE, [SIDE])).resolves.toBe('failed');
    expect(unmergeStationPair).not.toHaveBeenCalled();
  });

  it('Remove Closure does nothing once editing is blocked', async () => {
    previewThenPlan({ closure_id: 5n, renamed_lines: [], collisions: [] });
    await expect(removeClosureAsking(5n, MAIN)).resolves.toBe('failed');
    expect(removeLoopClosure).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith('Editing is disabled in the current view.', TOAST_KEYS.EDIT_DISABLED, 'warning');
  });
});

describe("the whose-station prompt's ways back", () => {
  it('Remove the closure and Detach follow the read-only gate', async () => {
    invokeIpc.mockResolvedValue({
      station: SIDE,
      node: 20,
      shown: { id: 3, name: '3', home_line: 1 },
      lines: [1, 2],
      foreign: true,
      loop_closure: 5n,
    });
    useUIStore.setState({ appMode: 'plan' } as never);
    const { unmount } = render(
      <ShownIdentityPrompt stationRef={SIDE} edit="name" onProceed={vi.fn()} onCancel={vi.fn()} />,
    );
    const remove = await screen.findByTestId('shown-identity-prompt-remove-closure');
    expect(remove).toBeDisabled();
    expect(screen.getByTestId('shown-identity-prompt-proceed')).toBeDisabled();
    unmount();

    invokeIpc.mockResolvedValue({
      station: SIDE,
      node: 20,
      shown: { id: 3, name: '3', home_line: 1 },
      lines: [1, 2],
      foreign: true,
      own_station: { id: 40, name: '0', home_line: 2 },
    });
    render(<ShownIdentityPrompt stationRef={SIDE} edit="name" onProceed={vi.fn()} onCancel={vi.fn()} />);
    await waitFor(() => expect(screen.getByTestId('shown-identity-prompt-detach')).toBeDisabled());
    expect(screen.getByTestId('shown-identity-prompt-cancel')).not.toBeDisabled();
  });
});
