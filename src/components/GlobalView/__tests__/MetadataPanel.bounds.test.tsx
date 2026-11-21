/**
 * Demo finding I-14: a version 2 file's `summary.bounds` is around its datums
 * only, so a survey with one datum has a box of one point. Project details
 * showed it as "Bounds: N 20.410000, S 20.410000, E -87.310000, W
 * -87.310000", which reads as a broken box; the GPS center already shows that
 * point. The bounds line is left out when the box is one point as shown, and
 * kept for a box that spans something. Injection: rendering the bounds
 * whenever `gps_bounds` is set fails the first case.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(async () => undefined), getErrorMessage: String }));

import '../../../i18n';
import MetadataPanel from '../MetadataPanel';
import type { ProjectMetadata } from '../../../types/project';

const project = (bounds: ProjectMetadata['gps_bounds']): ProjectMetadata => ({
  name: 'Demo Cave',
  location: null,
  file_path: '/surveys/demo-cave.stalx',
  modified_at: '2026-11-21T02:50:05Z',
  line_count: 6,
  total_length: 250,
  station_count: 25,
  gps_center: [-87.31, 20.41],
  gps_bounds: bounds,
  is_remote: false,
});

function renderPanel(meta: ProjectMetadata) {
  render(<MetadataPanel project={meta} onOpenProject={vi.fn()} onRemoveFromRecent={vi.fn()} />);
}

afterEach(() => cleanup());

describe('Project details bounds', () => {
  it('leaves out a box of one point (one datum)', () => {
    renderPanel(project({ north: 20.41, south: 20.41, east: -87.31, west: -87.31 }));
    expect(screen.getByTestId('metadata-gps')).toHaveTextContent('20.410000N');
    expect(screen.queryByTestId('metadata-bounds')).toBeNull();
    expect(screen.queryByText(/Bounds/)).toBeNull();
  });

  it('leaves out a box that is one point at the shown precision', () => {
    renderPanel(project({ north: 20.4100000001, south: 20.41, east: -87.31, west: -87.3100000001 }));
    expect(screen.queryByTestId('metadata-bounds')).toBeNull();
  });

  it('keeps a box that spans something (two datums)', () => {
    renderPanel(project({ north: 20.42, south: 20.41, east: -87.3, west: -87.31 }));
    expect(screen.getByTestId('metadata-bounds')).toHaveTextContent('N 20.420000, S 20.410000');
  });
});
