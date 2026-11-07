/**
 * Edit Station: the GPS field with a value that is already stored (#576, R4
 * review F8/F9).
 *
 * The stored datum is shown as decimal degrees. It goes through the same parse
 * path as typed text, so the format badge, the note and the swap hint always
 * describe the text in the box - including after the station is refreshed
 * while the dialog is open, which used to leave the note of the text typed
 * before it. A stored value was confirmed when it was saved, so the advisory
 * "may be swapped" hint is not shown for it, only once the text is edited.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
// The real i18next instance, so the assertions below read the rendered English.
import '../../../i18n';

type Gps = { latitude: number; longitude: number; depth_reference: number } | null;

const closeDialog = vi.fn();
const dialogData: Record<string, unknown> = {};
const storeState: Record<string, unknown> = {};

function station(gps: Gps) {
  return {
    station_ref: { section_id: 1, station_index: 0 },
    coordinates: { x: 0, y: 0, z: -3 },
    is_zero_datum: gps !== null,
    gps_coordinates: gps,
    comment: null,
    merged_with: [],
  };
}

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData }),
}));

// Called with a selector by the dialog and by `useDecimalPrecision` (reads `config`).
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel?: (s: unknown) => unknown) => (sel ? sel(storeState) : storeState),
}));

vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryStubs());

vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());

import EditStationDialog from '../EditStationDialog';
import { lazyStationArray, surveyDataFromLegacy } from '../../../types/survey-data';

/** Put one station (section 1, index 0) in the store. A new object each time,
 *  as a store refresh produces. */
function setStation(gps: Gps) {
  Object.assign(storeState, {
    stations: [station(gps)],
    project: { sections: [{ id: 1, name: 'CaveA Section' }] },
    config: null,
    setZeroDatum: vi.fn().mockResolvedValue({ success: true }),
    removeZeroDatum: vi.fn().mockResolvedValue({ success: true }),
    updateStationComment: vi.fn().mockResolvedValue({ success: true }),
  });
}

const q = (id: string) => screen.queryByTestId(id);

describe('EditStationDialog GPS field with a stored value (#576 R4 F8, F9)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    for (const key of Object.keys(dialogData)) delete dialogData[key];
    dialogData.stationRef = { section_id: 1, station_index: 0 };
  });

  it('opens a stored value parsed like typed text: badge and preview, no note', () => {
    setStation({ latitude: 20.5, longitude: -87.3, depth_reference: -3 });
    render(<EditStationDialog />);

    expect(screen.getByTestId('station-gps-input')).toHaveValue('20.5, -87.3');
    expect(screen.getByTestId('station-gps-format')).toHaveTextContent('DD');
    expect(screen.getByTestId('station-gps-parsed')).toHaveTextContent('Lat: 20.500000, Lon: -87.300000');
    expect(q('station-gps-note')).not.toBeInTheDocument();
    expect(q('station-gps-error')).not.toBeInTheDocument();
  });

  it('shows no swap hint for a stored far-north value until the text is edited', () => {
    // Svalbard (78.9N, 15.5E) stored as decimal degrees: confirmed when saved.
    setStation({ latitude: 78.9, longitude: 15.5, depth_reference: 0 });
    render(<EditStationDialog />);

    expect(screen.getByTestId('station-gps-parsed')).toHaveTextContent('Lat: 78.900000');
    expect(q('station-gps-transposed-warning')).not.toBeInTheDocument();

    fireEvent.change(screen.getByTestId('station-gps-input'), { target: { value: '78.9, 15.4' } });
    expect(screen.getByTestId('station-gps-transposed-warning')).toBeInTheDocument();
  });

  it('a station refresh while open re-reads the stored value and drops the old note', () => {
    setStation(null);
    dialogData.forceEnableZeroDatum = true;
    const { rerender } = render(<EditStationDialog />);

    fireEvent.change(screen.getByTestId('station-gps-input'), { target: { value: '33S 512345 8765432' } });
    expect(screen.getByTestId('station-gps-note')).toHaveTextContent('southern hemisphere');
    expect(screen.getByTestId('station-gps-format')).toHaveTextContent('UTM');

    // The datum was stored (set_zero_datum succeeded) and the stations
    // refreshed, but the dialog stayed open (e.g. the comment update failed).
    setStation({ latitude: -11.168093, longitude: 15.113069, depth_reference: -3 });
    rerender(<EditStationDialog />);

    expect(screen.getByTestId('station-gps-input')).toHaveValue('-11.168093, 15.113069');
    expect(screen.getByTestId('station-gps-format')).toHaveTextContent('DD');
    expect(screen.getByTestId('station-gps-parsed')).toHaveTextContent('Lat: -11.168093, Lon: 15.113069');
    expect(q('station-gps-note')).not.toBeInTheDocument();
  });

  it('keeps a stored value whose plain text form does not read back (1e-7)', () => {
    // `${1e-7}` is "1e-7", which is not coordinate text; the stored value is
    // still valid and must stay usable (no error, Save not blocked).
    setStation({ latitude: 1e-7, longitude: -87.3, depth_reference: 0 });
    render(<EditStationDialog />);

    expect(screen.getByTestId('station-gps-parsed')).toHaveTextContent('Lat: 0.000000, Lon: -87.300000');
    expect(q('station-gps-error')).not.toBeInTheDocument();
  });

  // [#576 B35(g)] A stored tie point can now be finite but impossible: the load
  // keeps it and the integrity banner sends the user here to correct it.
  it('shows a stored out-of-range value as the error it is, not as parsed', () => {
    setStation({ latitude: 95, longitude: 10, depth_reference: 0 });
    render(<EditStationDialog />);

    expect(screen.getByTestId('station-gps-input')).toHaveValue('95, 10');
    expect(screen.getByTestId('station-gps-error')).toHaveTextContent('Latitude');
    expect(q('station-gps-parsed')).not.toBeInTheDocument();
  });

  it('Save rewrites a stored point whose only problem is its depth reference', async () => {
    // Latitude and longitude are fine; the depth reference (20 km, from a build
    // before the bound) is not. Unchanged coordinates used to skip the write, so
    // the flagged point could not be fixed here at all.
    setStation({ latitude: 20.5, longitude: -87.3, depth_reference: 20000 });
    render(<EditStationDialog />);

    fireEvent.click(screen.getByTestId('edit-station-save'));
    await vi.waitFor(() => {
      expect(storeState.setZeroDatum).toHaveBeenCalledWith(
        { section_id: 1, station_index: 0 },
        { latitude: 20.5, longitude: -87.3, depth_reference: -3 },
      );
    });
  });
});

/**
 * #576 A9-D2: the store's `stations` is the binary decoder's lazy view after a load
 * and, now, after every refresh. Each index read of that view builds a NEW Station
 * object, so a station looked up on every render is a new object on every render.
 * The form must still keep what the user sets.
 */
describe('EditStationDialog over the lazy station array (#576 A9-D2)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    for (const key of Object.keys(dialogData)) delete dialogData[key];
    dialogData.stationRef = { section_id: 1, station_index: 0 };
  });

  it('keeps the zero-datum toggle and the comment the user set', () => {
    setStation(null);
    const section = { id: 1, name: 'CaveA Section', color: '#fff', direction: 'in', quality: 1, next_shot_id: 1, shots: [] };
    const data = surveyDataFromLegacy([section] as never, storeState.stations as never);
    Object.assign(storeState, { stations: lazyStationArray(data) });
    render(<EditStationDialog />);

    const toggle = screen.getByTestId('station-zero-datum-input');
    fireEvent.click(toggle);
    expect(toggle).toBeChecked();
    expect(screen.getByTestId('station-gps-input')).toBeInTheDocument();

    fireEvent.change(screen.getByTestId('station-comment-input'), { target: { value: 'sump' } });
    expect(screen.getByTestId('station-comment-input')).toHaveValue('sump');
    expect(toggle).toBeChecked();
  });
});
