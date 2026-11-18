import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

import { useUIStore } from '../uiStore';
import { DISPLAY_DEFAULTS } from '../displaySelectors';
import { HIDDEN_SECTIONS_SETTING, UNGROUPED_GROUP_ID, prunedHiddenSections } from '../sectionsViewSlice';

const saves = () =>
  vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === 'save_display_setting').map(([, args]) => args);

describe('Sections view slice', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockClear();
    useUIStore.setState({ hiddenSections: [], collapsedSections: {}, pendingSectionRename: null, requestZoomToLine: null });
  });

  it('hides and shows a Section, saving the JSON array each time', () => {
    useUIStore.getState().setSectionHidden(4, true);
    useUIStore.getState().setSectionHidden(9, true);
    expect(useUIStore.getState().hiddenSections).toEqual([4, 9]);
    useUIStore.getState().setSectionHidden(4, false);
    expect(useUIStore.getState().hiddenSections).toEqual([9]);
    expect(saves()).toEqual([
      { key: HIDDEN_SECTIONS_SETTING, value: '[4]' },
      { key: HIDDEN_SECTIONS_SETTING, value: '[4,9]' },
      { key: HIDDEN_SECTIONS_SETTING, value: '[9]' },
    ]);
  });

  it('a toggle to the state it already has saves nothing', () => {
    useUIStore.getState().setSectionHidden(4, false);
    expect(saves()).toEqual([]);
  });

  it('a load restores the hidden Sections, and a project without the setting hides none', () => {
    useUIStore.setState({ hiddenSections: [1] });
    useUIStore.getState().applyDisplaySettings({ ...DISPLAY_DEFAULTS, geometryMode: 'measured', hiddenSections: [3, 5] });
    expect(useUIStore.getState().hiddenSections).toEqual([3, 5]);
    const legacy = { ...DISPLAY_DEFAULTS, geometryMode: 'measured' } as unknown as Parameters<
      ReturnType<typeof useUIStore.getState>['applyDisplaySettings']
    >[0];
    useUIStore.getState().applyDisplaySettings(legacy);
    expect(useUIStore.getState().hiddenSections).toEqual([]);
  });

  it('forgets hidden ids that name no Section, and saves the pruned list', () => {
    useUIStore.setState({ hiddenSections: [3, 7, 9] });
    useUIStore.getState().pruneHiddenSections([{ id: 3 }, { id: 9 }, { id: 12 }]);
    expect(useUIStore.getState().hiddenSections).toEqual([3, 9]);
    expect(saves()).toEqual([{ key: HIDDEN_SECTIONS_SETTING, value: '[3,9]' }]);
    // A later Section reusing id 7 does not start hidden.
    useUIStore.getState().pruneHiddenSections([{ id: 3 }, { id: 7 }, { id: 9 }]);
    expect(useUIStore.getState().hiddenSections).toEqual([3, 9]);
  });

  it('pruning that changes nothing saves nothing, and an unknown Sections list prunes nothing', () => {
    useUIStore.setState({ hiddenSections: [3] });
    useUIStore.getState().pruneHiddenSections([{ id: 3 }]);
    useUIStore.getState().pruneHiddenSections(undefined);
    expect(useUIStore.getState().hiddenSections).toEqual([3]);
    expect(saves()).toEqual([]);
    expect(prunedHiddenSections([], [])).toBeNull();
    expect(prunedHiddenSections([1, 2], [])).toEqual([]);
  });

  it('keeps collapse state per project', () => {
    const { setSectionCollapsed } = useUIStore.getState();
    setSectionCollapsed('a', 2, true);
    setSectionCollapsed('a', UNGROUPED_GROUP_ID, true);
    setSectionCollapsed('b', 2, true);
    setSectionCollapsed('a', 2, false);
    expect(useUIStore.getState().collapsedSections).toEqual({ a: [UNGROUPED_GROUP_ID], b: [2] });
  });

  it('a several-lines zoom shares the zoom-to-line request; an empty one asks nothing', () => {
    useUIStore.getState().triggerZoomToLines([]);
    expect(useUIStore.getState().requestZoomToLine).toBeNull();
    useUIStore.getState().triggerZoomToLines([3, 4]);
    expect(useUIStore.getState().requestZoomToLine).toEqual([3, 4]);
  });
});
