/**
 * #741: the Entrance checkbox in Edit Station's zero-datum line. The
 * backend resolves whether a datum is an entrance (`is_entrance` on the stored
 * datum and on the preview); the dialog shows that answer and sends the
 * user's click only when it differs.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { ZeroDatumPreview } from '../../../types/ipc/ZeroDatumPreview';

const closeDialog = vi.fn();
let dialogData: Record<string, unknown> = {};
const uiState = () => ({ closeDialog, dialogData, activeDialog: 'edit-station' });
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign(() => uiState(), { getState: () => uiState() }),
}));

const setZeroDatum = vi.fn();
const removeZeroDatum = vi.fn();
const updateStationComment = vi.fn();
let projectState: Record<string, unknown> = {};
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: Object.assign(
    (selector?: (s: Record<string, unknown>) => unknown) => (selector ? selector(projectState) : projectState),
    { getState: () => projectState }
  ),
}));

vi.mock('../../../hooks/useEditingBlocked', () => ({
  useEditingBlocked: () => false,
  isEditingBlockedNow: () => false,
}));

vi.mock('../../../utils/telemetry', () => ({ trackDialogComplete: vi.fn() }));

const previewZeroDatum = vi.fn();
vi.mock('../../../utils/zeroDatumPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/zeroDatumPreview')>()),
  previewZeroDatum: (...args: unknown[]) => previewZeroDatum(...args),
}));

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import '../../../i18n';
import EditStationDialog from '../EditStationDialog';

const REF = { line_id: 2, station_index: 3 };
const GPS = { latitude: 20.5, longitude: -87.4, depth_reference: 0 };
const HINT =
  'Plan mode starts dives and measures penetration from entrances. A GPS fix in the middle of a passage, such as a radiolocation point, is not an entrance.';

/** A project whose station 2:3 holds `datum` (or nothing). */
function setProject(datum?: Record<string, unknown>) {
  projectState = {
    setZeroDatum,
    removeZeroDatum,
    updateStationComment,
    stations: [
      {
        station_ref: REF,
        coordinates: { x: 10, y: 20, z: 4 },
        is_zero_datum: datum !== undefined,
        gps_coordinates: datum ? GPS : null,
        comment: null,
        merged_with: [],
      },
    ],
    project: {
      created_at: 'c',
      modified_at: 'm1',
      file_path: '/p.stalx',
      lines: [{ id: 2, name: 'B', shots: [] }],
      zero_datums: datum ? [{ station_ref: REF, gps: GPS, ...datum }] : [],
    },
  };
}

const preview = (role: 'Placement' | 'GpsFix', is_entrance: boolean): ZeroDatumPreview => ({
  role,
  is_entrance,
  ...(role === 'GpsFix'
    ? { gap: { gap_m: 3, bearing_deg: 315, survey_distance_m: 120, threshold_m: 17.4, exceeds_threshold: false } }
    : {}),
});

const checkbox = () => screen.getByTestId('station-datum-entrance-input') as HTMLInputElement;

function typeGps(value: string) {
  fireEvent.change(screen.getByTestId('station-gps-input'), { target: { value } });
}

describe('EditStationDialog Entrance checkbox (#741)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setZeroDatum.mockResolvedValue({ success: true });
  });

  it('starts a new mid-passage fix unticked, as the backend resolves it, with its hint', async () => {
    dialogData = { stationRef: REF, forceEnableZeroDatum: true };
    setProject();
    previewZeroDatum.mockResolvedValue(preview('GpsFix', false));
    render(<EditStationDialog />);
    expect(screen.queryByTestId('station-datum-entrance-input')).not.toBeInTheDocument();
    typeGps('20.5, -87.4');
    await screen.findByTestId('station-datum-entrance-input');
    expect(checkbox().checked).toBe(false);
    expect(checkbox().disabled).toBe(false);
    expect(screen.getByTestId('station-datum-entrance-hint')).toHaveTextContent(HINT);
  });

  it('starts a new fix at a line end ticked, and sends nothing when left alone', async () => {
    dialogData = { stationRef: REF, forceEnableZeroDatum: true };
    setProject();
    previewZeroDatum.mockResolvedValue(preview('GpsFix', true));
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    await screen.findByTestId('station-datum-entrance-input');
    expect(checkbox().checked).toBe(true);
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalledTimes(1));
    expect(setZeroDatum.mock.calls[0]![2]).not.toHaveProperty('entrance');
  });

  it('sends the answer when the user ticks a mid-passage fix', async () => {
    dialogData = { stationRef: REF, forceEnableZeroDatum: true };
    setProject();
    previewZeroDatum.mockResolvedValue(preview('GpsFix', false));
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    await screen.findByTestId('station-datum-entrance-input');
    fireEvent.click(checkbox());
    expect(checkbox().checked).toBe(true);
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalledTimes(1));
    expect(setZeroDatum.mock.calls[0]![2]).toMatchObject({ entrance: true });
  });

  it('saves an entrance change alone on a stored fix, even with the GPS unchanged', async () => {
    dialogData = { stationRef: REF };
    setProject({ role: 'GpsFix', is_entrance: true });
    previewZeroDatum.mockResolvedValue(preview('GpsFix', true));
    render(<EditStationDialog />);
    expect(checkbox().checked).toBe(true);
    fireEvent.click(checkbox());
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalledTimes(1));
    expect(setZeroDatum.mock.calls[0]![2]).toMatchObject({ entrance: false });
  });

  it('shows a placement datum as an entrance, ticked and disabled', () => {
    dialogData = { stationRef: REF };
    setProject({ role: 'Placement', is_entrance: true });
    previewZeroDatum.mockResolvedValue(preview('Placement', true));
    render(<EditStationDialog />);
    expect(checkbox().checked).toBe(true);
    expect(checkbox().disabled).toBe(true);
    expect(screen.getByTestId('station-datum-entrance-hint')).toHaveTextContent(
      'The zero datum that places this part of the survey is always an entrance.'
    );
  });
});
