import { describe, it, expect } from 'vitest';
import { dialogMapPreview } from '../useStationHighlights';

describe('dialogMapPreview', () => {
  it('rings the station Split Section Here cuts at', () => {
    expect(
      dialogMapPreview('split-section-here', { sectionId: 4, at: { type: 'station', station_index: 2 } })
    ).toEqual({ stations: [{ section_id: 4, station_index: 2 }], sections: [] });
    // A cut along a shot makes a new station: nothing to ring yet.
    expect(
      dialogMapPreview('split-section-here', { sectionId: 4, at: { type: 'shot', shot_id: 9, distance: 1 } })
    ).toEqual({ stations: [], sections: [] });
  });

  it('highlights both sections and rings the join for Combine Sections', () => {
    const preview = { sections: [1, 5], stations: [{ section_id: 1, station_index: 3 }] };
    expect(dialogMapPreview('combine-sections', { preview })).toEqual(preview);
  });

  it('previews nothing for any other dialog', () => {
    expect(dialogMapPreview('edit-section', { sectionId: 1 })).toEqual({ stations: [], sections: [] });
    expect(dialogMapPreview(null, {})).toEqual({ stations: [], sections: [] });
  });
});
