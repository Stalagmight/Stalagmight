/**
 * #741: the zero-datum part of the station Details names the datum's role and,
 * for a GPS fix, shows the backend's gap and labels its lat/lon as the GPS
 * reading rather than where the station is drawn.
 */
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import '../../../i18n';
import { StationDatumDetail } from '../StationDatumDetail';
import type { ZeroDatum } from '../../../types/ipc/ZeroDatum';

const REF = { section_id: 1, station_index: 5 };
const GPS = { latitude: 20.5, longitude: -87.25, depth_reference: 0 };
const GAP = { gap_m: 12.4, bearing_deg: 44, survey_distance_m: 500, threshold_m: 25, exceeds_threshold: false };

const datum = (extra: Partial<ZeroDatum>): ZeroDatum => ({ station_ref: REF, gps: GPS, role: 'Placement', is_entrance: true, ...extra });

describe('StationDatumDetail (#741)', () => {
  it('names a placement datum and shows its GPS plainly', () => {
    render(<StationDatumDetail datum={datum({})} gps={GPS} gpsPrecision={6} />);
    expect(screen.getByTestId('station-datum-detail-role')).toHaveTextContent('Placement datum');
    expect(screen.queryByTestId('station-datum-detail-gap')).not.toBeInTheDocument();
    expect(screen.queryByTestId('station-datum-detail-gps-reading')).not.toBeInTheDocument();
    expect(screen.getByText('20.500000')).toBeInTheDocument();
  });

  it('names a GPS fix, shows its gap, and labels its lat/lon as the GPS reading', () => {
    render(<StationDatumDetail datum={datum({ role: 'GpsFix', gap: GAP })} gps={GPS} gpsPrecision={6} />);
    expect(screen.getByTestId('station-datum-detail-role')).toHaveTextContent('GPS fix');
    expect(screen.getByTestId('station-datum-detail-gap')).toHaveTextContent(
      'GPS is 12 m NE of the surveyed position.'
    );
    expect(screen.getByTestId('station-datum-detail-gps-reading')).toHaveTextContent('GPS reading');
    expect(screen.queryByTestId('station-datum-detail-gap-warning')).not.toBeInTheDocument();
  });

  it('warns when the backend says the fix fails the gap check', () => {
    render(
      <StationDatumDetail
        datum={datum({ role: 'GpsFix', gap: { ...GAP, gap_m: 43, exceeds_threshold: true } })}
        gps={GPS}
        gpsPrecision={6}
      />
    );
    expect(screen.getByTestId('station-datum-detail-gap-warning')).toHaveTextContent(
      'more than the 25 m expected'
    );
  });

  it('falls back to the plain label when the wire has no datum for the station', () => {
    render(<StationDatumDetail datum={undefined} gps={GPS} gpsPrecision={6} />);
    expect(screen.getByTestId('station-datum-detail-role')).toHaveTextContent('Zero datum station');
  });
});
