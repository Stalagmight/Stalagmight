import { describe, it, expect } from 'vitest';
import { dialogMapPreview } from '../useStationHighlights';

describe('dialogMapPreview', () => {
  it('rings the station Split Line Here cuts at', () => {
    expect(
      dialogMapPreview('split-line-here', { lineId: 4, at: { type: 'station', station_index: 2 } })
    ).toEqual({ stations: [{ line_id: 4, station_index: 2 }], lines: [] });
    // A cut along a shot makes a new station: nothing to ring yet.
    expect(
      dialogMapPreview('split-line-here', { lineId: 4, at: { type: 'shot', shot_id: 9, distance: 1 } })
    ).toEqual({ stations: [], lines: [] });
  });

  it('highlights both lines and rings the join for Combine Lines', () => {
    const preview = { lines: [1, 5], stations: [{ line_id: 1, station_index: 3 }] };
    expect(dialogMapPreview('combine-lines', { preview })).toEqual(preview);
  });

  it('previews nothing for any other dialog', () => {
    expect(dialogMapPreview('edit-line', { lineId: 1 })).toEqual({ stations: [], lines: [] });
    expect(dialogMapPreview(null, {})).toEqual({ stations: [], lines: [] });
  });
});
