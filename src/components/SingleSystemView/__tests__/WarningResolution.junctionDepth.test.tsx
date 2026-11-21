/**
 * Demo issue I-4: a junction whose depth readings disagree by more than the
 * threshold is an advisory warning (`JUNCTION_DEPTH_SPREAD`, on the shot
 * arriving at the junction; `_START` on a leaving shot). The reading to fix
 * can be on any line through the junction, so its resolution is the way
 * there: Go to station lands on the junction the backend names
 * (`datumStationIndex`), and Ignore is offered. Injection: drop the case from
 * WarningResolution and only Ignore is offered.
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

const spread = (code: string, shotIndex: number, datumStationIndex: number) =>
  ({
    code,
    message:
      "The depth readings at Loose / 2, where 2 lines meet, differ by 11.0 m (4.0 m to 15.0 m). The station's depth is their mean, 9.5 m. Check the depths of the shots that meet here.",
    severity: 'warning',
    lineId: 7,
    shotId: 700,
    shotIndex,
    isResolved: false,
    datumStationIndex,
  }) as ProjectWarning;

describe.each([
  ['JUNCTION_DEPTH_SPREAD', 1, 2],
  ['JUNCTION_DEPTH_SPREAD_START', 0, 0],
] as const)('WarningResolution - a junction whose depths disagree (%s)', (code, shotIndex, stationIndex) => {
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
        warning={spread(code, shotIndex, stationIndex)}
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

  it('goes to the junction', async () => {
    renderResolution();
    await userEvent.click(screen.getByTestId('warning-resolution-apply'));
    await waitFor(() =>
      expect(setSelection).toHaveBeenCalledWith({
        type: 'station',
        stationRef: { line_id: 7, station_index: stationIndex },
      })
    );
  });
});
