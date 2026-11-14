/**
 * #741: `datum_gps_gap` - a GPS fix whose GPS is further from its surveyed
 * position than the sanity threshold. Advisory: offer a jump to the fix station
 * and the Edit Station dialog for its datum, plus Ignore.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

let blocked = false;
vi.mock('../../../hooks/useEditingBlocked', () => ({
  useEditingBlocked: () => blocked,
  isEditingBlockedNow: () => blocked,
}));

import '../../../i18n';
import { WarningResolution } from '../WarningResolution';
import { useUIStore } from '../../../store/uiStore';
import type { ProjectWarning } from '../../../types/commands';
import type { Section, Shot } from '../../../types/project';

const section = { id: 4, name: 'B', shots: [] } as unknown as Section;
const shot = {
  id: 41,
  length: 10,
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

const warning: ProjectWarning = {
  code: 'datum_gps_gap',
  message: 'The GPS of zero datum 4:3 is 60 m from where the survey puts it.',
  severity: 'warning',
  sectionId: 4,
  shotId: 41,
  shotIndex: 2,
  isResolved: false,
  datumStationIndex: 3,
};

const openDialog = vi.fn();
const onExit = vi.fn();

function renderIt() {
  return render(
    <WarningResolution warning={warning} section={section} shot={shot} onExit={onExit} onResolved={vi.fn()} onSkip={vi.fn()} />
  );
}

describe('WarningResolution datum_gps_gap (#741)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    useUIStore.setState({ openDialog });
  });

  it('shows the backend message and offers go-to-station, edit datum and ignore', () => {
    renderIt();
    expect(screen.getByText(warning.message)).toBeInTheDocument();
    expect(screen.getByTestId('warning-resolution-option-go-to-station')).toBeEnabled();
    expect(screen.getByTestId('warning-resolution-option-edit-datum')).toBeEnabled();
    expect(screen.getByTestId('warning-resolution-option-ignore')).toBeInTheDocument();
    expect(screen.getByText('Edit zero datum')).toBeInTheDocument();
  });

  it('opens Edit Station on the fix station, focused on its GPS', async () => {
    renderIt();
    fireEvent.click(screen.getByTestId('warning-resolution-option-edit-datum'));
    fireEvent.click(screen.getByTestId('warning-resolution-apply'));
    await waitFor(() => expect(onExit).toHaveBeenCalled());
    expect(openDialog).toHaveBeenCalledWith('edit-station', {
      stationRef: { section_id: 4, station_index: 3 },
      focusGpsInput: true,
    });
  });

  it('go-to-station selects the fix station without opening a dialog', async () => {
    renderIt();
    fireEvent.click(screen.getByTestId('warning-resolution-apply'));
    await waitFor(() => expect(onExit).toHaveBeenCalled());
    expect(openDialog).not.toHaveBeenCalled();
    expect(useUIStore.getState().selection).toEqual({ type: 'station', stationRef: { section_id: 4, station_index: 3 } });
  });

  it('disables editing the datum while editing is blocked', () => {
    blocked = true;
    renderIt();
    expect(screen.getByTestId('warning-resolution-option-edit-datum')).toBeDisabled();
    expect(screen.getByTestId('warning-resolution-option-go-to-station')).toBeEnabled();
  });
});
