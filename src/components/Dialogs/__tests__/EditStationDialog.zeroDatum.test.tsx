/**
 * #741: the Edit Station dialog with several zero datums in one connected
 * survey - the accuracy field, the live `preview_zero_datum` readout (role, gap,
 * sanity warning), and its stale-reply guard.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import type { ZeroDatumPreview } from '../../../types/ipc/ZeroDatumPreview';

const closeDialog = vi.fn();
let dialogData: Record<string, unknown> = {};
let activeDialog: string | null = 'edit-station';
const uiState = () => ({ closeDialog, dialogData, activeDialog });
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

let blocked = false;
vi.mock('../../../hooks/useEditingBlocked', () => ({
  useEditingBlocked: () => blocked,
  isEditingBlockedNow: () => blocked,
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

const REF = { section_id: 2, station_index: 3 };
const GPS = { latitude: 20.5, longitude: -87.4, depth_reference: 0 };

function station(extra: Record<string, unknown> = {}) {
  return {
    station_ref: REF,
    coordinates: { x: 10, y: 20, z: 4 },
    is_zero_datum: false,
    gps_coordinates: null,
    comment: null,
    merged_with: [],
    ...extra,
  };
}

function setProject(opts: { datum?: Record<string, unknown>; modified?: string } = {}) {
  projectState = {
    setZeroDatum,
    removeZeroDatum,
    updateStationComment,
    stations: [station(opts.datum ? { is_zero_datum: true, gps_coordinates: GPS } : {})],
    project: {
      created_at: 'c',
      modified_at: opts.modified ?? 'm1',
      file_path: '/p.stalx',
      sections: [{ id: 2, name: 'B', shots: [] }],
      zero_datums: opts.datum ? [{ station_ref: REF, gps: GPS, ...opts.datum }] : [],
    },
  };
}

const fix = (gap: Partial<NonNullable<ZeroDatumPreview['gap']>> = {}): ZeroDatumPreview => ({
  role: 'GpsFix',
  gap: { gap_m: 40, bearing_deg: 45, survey_distance_m: 1400, threshold_m: 43, exceeds_threshold: false, ...gap },
  is_entrance: false,
});

/** A promise resolved by hand, to land replies out of order. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

function typeGps(value: string) {
  fireEvent.change(screen.getByTestId('station-gps-input'), { target: { value } });
}

describe('EditStationDialog zero datum (#741)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    activeDialog = 'edit-station';
    dialogData = { stationRef: REF, forceEnableZeroDatum: true };
    setProject();
    setZeroDatum.mockResolvedValue({ success: true });
    previewZeroDatum.mockResolvedValue(fix());
  });

  it('previews the datum once valid GPS is entered, with the station depth', async () => {
    render(<EditStationDialog />);
    expect(previewZeroDatum).not.toHaveBeenCalled();
    typeGps('20.5, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalledTimes(1));
    expect(previewZeroDatum).toHaveBeenCalledWith(REF, { latitude: 20.5, longitude: -87.4, depth_reference: 4 });
  });

  it('says a GPS fix bends the computed view, not the raw one, and shows the gap', async () => {
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    expect(await screen.findByTestId('station-datum-fix-notice')).toHaveTextContent(
      'This survey already has a zero datum. This one will bend the Closures Computed View toward its GPS. It does not move the Raw Data View.'
    );
    expect(screen.getByTestId('station-datum-gap')).toHaveTextContent('GPS is 40 m NE of the surveyed position.');
    expect(screen.queryByTestId('station-datum-gap-warning')).not.toBeInTheDocument();
  });

  it('warns, without blocking, when the gap exceeds the threshold', async () => {
    previewZeroDatum.mockResolvedValue(fix({ gap_m: 60, bearing_deg: 180, exceeds_threshold: true }));
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    const warning = await screen.findByTestId('station-datum-gap-warning');
    expect(warning).toHaveTextContent(/more than the 43 m expected over 1,?400 m of survey/);
    expect(screen.getByTestId('station-datum-gap')).toHaveTextContent('GPS is 60 m S of');
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalledTimes(1));
  });

  it('shows nothing extra for a datum that would place its piece', async () => {
    previewZeroDatum.mockResolvedValue({ role: 'Placement' });
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId('station-datum-preview')).not.toBeInTheDocument();
  });

  it('defaults the accuracy to a 0 m hard pin, and stores none for the default', async () => {
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    expect(await screen.findByTestId('station-datum-accuracy-input')).toHaveValue(0);
    expect(screen.getByText(/0 puts the computed survey exactly through this GPS\. A larger number .* trusts the GPS less, and the survey wins most of the gap\./)).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('edit-station-save'));
    // The user never set an accuracy, so none is sent: the backend reads an
    // absent one as the default without writing a side-table row for it.
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalledWith(REF, expect.anything(), {}));
  });

  it('re-saves a datum stored without an accuracy without writing one', async () => {
    dialogData = { stationRef: REF };
    setProject({ datum: { role: 'GpsFix' } });
    render(<EditStationDialog />);
    expect(screen.getByTestId('station-datum-accuracy-input')).toHaveValue(0);
    typeGps('20.6, -87.4');
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(setZeroDatum).toHaveBeenCalledTimes(1);
    expect(setZeroDatum).toHaveBeenCalledWith(REF, { latitude: 20.6, longitude: -87.4, depth_reference: 4 }, {});
  });

  it('keeps a stored accuracy through a GPS-only edit', async () => {
    dialogData = { stationRef: REF };
    setProject({ datum: { accuracy_m: 12, role: 'GpsFix' } });
    render(<EditStationDialog />);
    typeGps('20.6, -87.4');
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    // Omitting it would clear the stored 12 m back to the default.
    expect(setZeroDatum).toHaveBeenCalledWith(REF, expect.anything(), { accuracyM: 12 });
  });

  it('passes the entered accuracy on save', async () => {
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    fireEvent.change(await screen.findByTestId('station-datum-accuracy-input'), { target: { value: '2.5' } });
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(setZeroDatum).toHaveBeenCalledWith(
      REF,
      { latitude: 20.5, longitude: -87.4, depth_reference: 4 },
      { accuracyM: 2.5 }
    );
  });

  it('refuses a negative, empty or over-1000 m accuracy until it is fixed', async () => {
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    const field = await screen.findByTestId('station-datum-accuracy-input');
    // 5000 is what the backend refuses on save; the field must refuse it first.
    for (const bad of ['-1', '', '5000', '1000.5']) {
      fireEvent.change(field, { target: { value: bad } });
      expect(screen.getByTestId('station-datum-accuracy-error')).toHaveTextContent(
        'Enter an accuracy from 0 to 1000 m.'
      );
      fireEvent.click(screen.getByTestId('edit-station-save'));
      await act(async () => {});
      expect(setZeroDatum).not.toHaveBeenCalled();
    }
    fireEvent.change(field, { target: { value: '4' } });
    expect(screen.queryByTestId('station-datum-accuracy-error')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalledWith(REF, expect.anything(), { accuracyM: 4 }));
  });

  it('prefills an existing datum accuracy and role, and saves an accuracy-only change', async () => {
    dialogData = { stationRef: REF };
    setProject({ datum: { accuracy_m: 12, role: 'GpsFix' } });
    render(<EditStationDialog />);
    expect(screen.getByTestId('station-datum-accuracy-input')).toHaveValue(12);
    expect(screen.getByTestId('station-datum-role')).toHaveTextContent(
      'Role: GPS fix - bends the Closures Computed View toward its GPS and does not move the Raw Data View'
    );
    // The stored datum explains itself through its role line, not the "already
    // has a zero datum" notice meant for a new one.
    expect(await screen.findByTestId('station-datum-gap')).toBeInTheDocument();
    expect(screen.queryByTestId('station-datum-fix-notice')).not.toBeInTheDocument();
    fireEvent.change(screen.getByTestId('station-datum-accuracy-input'), { target: { value: '3' } });
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalledWith(REF, expect.anything(), { accuracyM: 3 }));
  });

  it('hides the accuracy of a placement datum and sends its stored value back', async () => {
    dialogData = { stationRef: REF };
    previewZeroDatum.mockResolvedValue({ role: 'Placement' });
    setProject({ datum: { accuracy_m: 7, role: 'Placement' } });
    render(<EditStationDialog />);
    expect(screen.queryByTestId('station-datum-accuracy-input')).not.toBeInTheDocument();
    typeGps('20.6, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId('station-datum-accuracy-input')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    // The GPS changed, so the datum is re-saved with its stored 7 m: the
    // backend stores what it is sent, so sending none would wipe it.
    expect(setZeroDatum).toHaveBeenCalledWith(
      REF,
      { latitude: 20.6, longitude: -87.4, depth_reference: 4 },
      { accuracyM: 7 }
    );
  });

  it('sends no accuracy for a placement datum that never stored one', async () => {
    dialogData = { stationRef: REF };
    previewZeroDatum.mockResolvedValue({ role: 'Placement' });
    setProject({ datum: { role: 'Placement' } });
    render(<EditStationDialog />);
    typeGps('20.6, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalled());
    await act(async () => {});
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(setZeroDatum).toHaveBeenCalledWith(REF, { latitude: 20.6, longitude: -87.4, depth_reference: 4 }, {});
  });

  it('hides the accuracy of a new datum until the preview says it will be a fix', async () => {
    previewZeroDatum.mockResolvedValue({ role: 'Placement' });
    render(<EditStationDialog />);
    expect(screen.queryByTestId('station-datum-accuracy-input')).not.toBeInTheDocument();
    typeGps('20.5, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId('station-datum-accuracy-input')).not.toBeInTheDocument();
    previewZeroDatum.mockResolvedValue(fix());
    typeGps('20.6, -87.4');
    expect(await screen.findByTestId('station-datum-accuracy-input')).toHaveValue(0);
  });

  it('does not touch an existing datum when neither GPS nor accuracy changed', async () => {
    dialogData = { stationRef: REF };
    setProject({ datum: { role: 'Placement' } });
    render(<EditStationDialog />);
    expect(screen.getByTestId('station-datum-role')).toHaveTextContent('Role: places this part of the survey on its GPS');
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(setZeroDatum).not.toHaveBeenCalled();
  });

  it('never paints a reply that a newer request superseded', async () => {
    const first = deferred<ZeroDatumPreview>();
    const second = deferred<ZeroDatumPreview>();
    previewZeroDatum.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalledTimes(1));
    typeGps('20.6, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalledTimes(2));

    await act(async () => first.resolve(fix({ gap_m: 99 })));
    expect(screen.queryByTestId('station-datum-gap')).not.toBeInTheDocument();
    await act(async () => second.resolve(fix({ gap_m: 40 })));
    expect(screen.getByTestId('station-datum-gap')).toHaveTextContent('GPS is 40 m');
  });

  // What this pins is the end state: no answer about the old project survives.
  // It does NOT isolate the projectIdentityKey prong - deleting it stays green,
  // because the re-render re-runs the effect, which clears the preview too. The
  // prong covers the window between a store write and React's commit.
  it('shows no answer about a project state that changed under the request', async () => {
    const reply = deferred<ZeroDatumPreview>();
    previewZeroDatum.mockReturnValueOnce(reply.promise);
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    await waitFor(() => expect(previewZeroDatum).toHaveBeenCalledTimes(1));
    // An edit lands (modified_at moves) before the reply does.
    setProject({ modified: 'm2' });
    await act(async () => reply.resolve(fix()));
    expect(screen.queryByTestId('station-datum-preview')).not.toBeInTheDocument();
  });

  it('disables Save, with the reason, while editing is blocked', () => {
    blocked = true;
    render(<EditStationDialog />);
    const save = screen.getByTestId('edit-station-save');
    expect(save).toBeDisabled();
    expect(save).toHaveAttribute('title', 'Editing is disabled in the current view');
  });

  it('leaves another dialog alone when this one was dismissed mid-save', async () => {
    activeDialog = 'something-else';
    render(<EditStationDialog />);
    typeGps('20.5, -87.4');
    fireEvent.click(screen.getByTestId('edit-station-save'));
    await waitFor(() => expect(setZeroDatum).toHaveBeenCalled());
    await act(async () => {});
    expect(closeDialog).not.toHaveBeenCalled();
  });
});
