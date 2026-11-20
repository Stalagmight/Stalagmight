/**
 * Detach a line from a station several lines share (plan 5.2, 9.3): the
 * dialog lists every line through the station, the clicked one first and
 * preselected, and sends the command that detaches the CHOSEN line.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import UnmergeStationsDialog from '../UnmergeStationsDialog';

const MAIN = 3;
const SIDE = 8;
const BRANCH = 9;
const unmergeStations = vi.fn();
const unmergeStationPair = vi.fn();
const closeDialog = vi.fn();

const slot = (line_id: number, station_index: number) => ({ line_id, station_index });

beforeEach(() => {
  vi.clearAllMocks();
  unmergeStations.mockResolvedValue({ success: true });
  unmergeStationPair.mockResolvedValue({ success: true });
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main Line', shots: [] },
        { id: SIDE, name: 'Side', shots: [] },
        { id: BRANCH, name: 'Branch', shots: [] },
      ],
    },
    surveyData: null,
    // One station (Station ID 40, "J") on three lines.
    stations: [
      { station_ref: slot(MAIN, 2), id: 40, name: 'J', merged_with: [slot(SIDE, 0), slot(BRANCH, 0)] },
      { station_ref: slot(SIDE, 0), id: 40, name: 'J', merged_with: [slot(MAIN, 2), slot(BRANCH, 0)] },
      { station_ref: slot(BRANCH, 0), id: 40, name: 'J', merged_with: [slot(MAIN, 2), slot(SIDE, 0)] },
    ],
    unmergeStations,
    unmergeStationPair,
  } as never);
  useUIStore.setState({
    closeDialog,
    dialogData: { stationRef: slot(SIDE, 0), mergedStations: [slot(MAIN, 2), slot(BRANCH, 0)] },
  } as never);
});

describe('Detach a Line from This Station', () => {
  it('lists every line through the station, the clicked one first and chosen', () => {
    render(<UnmergeStationsDialog />);
    expect(screen.getByTestId('unmerge-stations-lead')).toHaveTextContent('J is where 3 lines meet');
    expect(screen.getByTestId(`unmerge-station-option-${SIDE}-0`)).toBeChecked();
    expect(screen.getByText('Side (the line you clicked)')).toBeInTheDocument();
    expect(screen.getByText('Main Line')).toBeInTheDocument();
    expect(screen.getByText('Branch')).toBeInTheDocument();
    expect(document.body.textContent).toContain('The other lines stay joined');
  });

  it("detaches the clicked slot's own line with unmerge_stations", async () => {
    render(<UnmergeStationsDialog />);
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(unmergeStations).toHaveBeenCalledWith(slot(SIDE, 0));
    expect(unmergeStationPair).not.toHaveBeenCalled();
  });

  it('detaches another line with the pair form, which detaches the partner', async () => {
    render(<UnmergeStationsDialog />);
    fireEvent.click(screen.getByTestId(`unmerge-station-option-${BRANCH}-0`));
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(unmergeStationPair).toHaveBeenCalledWith(slot(SIDE, 0), slot(BRANCH, 0));
    expect(unmergeStations).not.toHaveBeenCalled();
  });

  it('keeps the dialog open with the reason when the backend refuses', async () => {
    unmergeStations.mockResolvedValue({ success: false, error: 'Stations are not directly merged' });
    render(<UnmergeStationsDialog />);
    fireEvent.click(screen.getByTestId('unmerge-stations-confirm'));
    expect(await screen.findByText('Stations are not directly merged')).toBeInTheDocument();
    expect(closeDialog).not.toHaveBeenCalled();
  });
});
