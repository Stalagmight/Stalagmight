/**
 * #814: a piece of the survey nothing places is an advisory warning. Its code
 * keeps master's spelling, SECTION_NOT_CONNECTED (the backend's
 * `LINE_NOT_CONNECTED` constant), and its resolution is the way to the piece:
 * Go to station lands on the first station of the line the warning names, and
 * Ignore is offered, since a separate piece can be kept on purpose.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import { WarningResolution } from '../WarningResolution';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { ProjectWarning } from '../../../types/commands';
import type { Line, Shot } from '../../../types/project';

const line = { id: 7, name: 'Loose', shots: [] } as unknown as Line;
const shot = {
  id: 700,
  length: 3,
  azimuth_in: 90,
  azimuth_out: 90,
  depth_in: 0,
  depth_out: 0,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  kind: 'Normal' as const,
} as unknown as Shot;

const notConnected = {
  code: 'SECTION_NOT_CONNECTED',
  message:
    'Line "Loose" is not connected to the rest of the survey and has no zero datum, so nothing places it: merge a station onto the survey or give it a zero datum',
  severity: 'warning',
  lineId: 7,
  shotId: 700,
  shotIndex: 0,
  isResolved: false,
} as ProjectWarning;

describe('WarningResolution - a line nothing places (#814)', () => {
  const setSelection = vi.fn();

  beforeEach(() => {
    setSelection.mockReset();
    useProjectStore.setState({
      databaseOpen: true,
      surveyData: null,
      stations: [],
      project: { loop_closures: [], lines: [{ id: 7, name: 'Loose', shots: [] }] },
    } as never);
    useUIStore.setState({ geometryMode: 'measured', appMode: 'survey', setSelection } as never);
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' } as never);
  });

  function renderResolution() {
    render(
      <WarningResolution
        warning={notConnected}
        line={line}
        shot={shot}
        onExit={vi.fn()}
        onResolved={vi.fn()}
        onSkip={vi.fn()}
      />
    );
  }

  it('offers Go to station first, and Ignore', () => {
    renderResolution();
    expect(screen.getByTestId('warning-resolution-option-go-to-station')).toBeChecked();
    expect(screen.getByTestId('warning-resolution-option-ignore')).toBeEnabled();
  });

  it("goes to the first station of the warning's line", async () => {
    renderResolution();
    await userEvent.click(screen.getByTestId('warning-resolution-apply'));
    await waitFor(() =>
      expect(setSelection).toHaveBeenCalledWith({
        type: 'station',
        stationRef: { line_id: 7, station_index: 0 },
      })
    );
  });
});
