import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

import { useExportStore, DEFAULT_IMAGE_OPTIONS } from '../exportStore';
import { useUIStore } from '../uiStore';
import { DISPLAY_DEFAULTS } from '../displaySelectors';

describe('exportStore', () => {
  beforeEach(() => {
    useExportStore.setState({
      exportType: 'image',
      dataFormat: 'csv',
      imageOptions: DEFAULT_IMAGE_OPTIONS,
    });
  });

  it('defaults to the image type so Export mode shows the preview immediately', () => {
    expect(useExportStore.getState().exportType).toBe('image');
  });

  it('default image options are monochrome PNG with labels on (colors opt-in)', () => {
    expect(DEFAULT_IMAGE_OPTIONS).toEqual({
      format: 'png',
      dpi: 300,
      branching_threshold: 2,
      show_penetration_labels: true,
      color_mode: 'none',
      show_station_labels: false,
      show_station_names: false,
      show_section_names: false,
      show_station_comments: false,
      show_shot_comments: false,
      show_shot_data: false,
    });
  });

  it('updates each slice independently', () => {
    const s = useExportStore.getState();
    s.setExportType('data');
    s.setDataFormat('tml');
    s.setImageOptions({ ...DEFAULT_IMAGE_OPTIONS, color_mode: 'depth' });
    const next = useExportStore.getState();
    expect(next.exportType).toBe('data');
    expect(next.dataFormat).toBe('tml');
    expect(next.imageOptions.color_mode).toBe('depth');
  });

  /**
   * The store outlives the project its options were chosen for. Nothing used to
   * clear it - not `loadProject`, not `closeProject`, and `setAppMode` says
   * explicitly that it does not - so a 600-DPI PNG framed for one survey stayed
   * selected when the next one opened, with the Export-mode preview rendering a
   * different cave at settings nobody chose for it.
   */
  describe('reset on project transitions', () => {
    const dirty = () => {
      const s = useExportStore.getState();
      s.setExportType('data');
      s.setDataFormat('tml');
      s.setImageOptions({ ...DEFAULT_IMAGE_OPTIONS, format: 'pdf', dpi: 600, branching_threshold: 9 });
    };
    const expectDefaults = () => {
      const s = useExportStore.getState();
      expect(s.exportType).toBe('image');
      expect(s.dataFormat).toBe('csv');
      expect(s.imageOptions).toEqual(DEFAULT_IMAGE_OPTIONS);
    };

    it('reset() restores every slice', () => {
      dirty();
      useExportStore.getState().reset();
      expectDefaults();
    });

    it('hands back a FRESH imageOptions object, not the shared default', () => {
      // `reset` exists to un-do a user's edits; a caller that then mutated what
      // it got back would corrupt the defaults for the rest of the session.
      useExportStore.getState().reset();
      expect(useExportStore.getState().imageOptions).not.toBe(DEFAULT_IMAGE_OPTIONS);
    });

    it('applyDisplaySettings resets it - the load/create funnel', () => {
      dirty();
      // Same call `projectStore.loadProject` / `createProject` make, and the same
      // funnel `useGasPlanStore.getState().reset()` already rides on.
      useUIStore.getState().applyDisplaySettings({ ...DISPLAY_DEFAULTS, geometryMode: 'measured' });
      expectDefaults();
    });
  });
});
