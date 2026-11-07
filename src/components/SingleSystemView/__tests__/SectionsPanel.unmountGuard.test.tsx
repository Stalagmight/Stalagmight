/**
 * [#576 RV9b-1] SectionsPanel's own wiring of the virtualizer unmount guard
 * (`useVirtualizerUnmountGuard(rowVirtualizer)`, #576 NF-W8E3-1).
 *
 * The hook's test drives a list of its own; nothing pinned that the panel calls
 * it, and removing the call failed no test. Here the real panel and the real
 * TanStack Virtual run in jsdom: a selection far down the list makes the panel
 * reveal its row (`scrollToIndex`), the panel unmounts (Plan mode) before the
 * reveal's frame runs, and the frame then runs - with the animation frames held
 * so the order is the losing one every time.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, render } from '@testing-library/react';

vi.mock('../../../utils/ipc');
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

import SectionsPanel from '../SectionsPanel';
import { useUIStore } from '../../../store/uiStore';
import { makeSection } from '../../../test/factories/survey';
import { invokeIpcMock } from '../../../test/factories/ipc';

const SECTIONS = Array.from({ length: 200 }, (_, i) => makeSection({ id: i + 1, name: `Section ${i + 1}` }));

let frames: FrameRequestCallback[];

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  useUIStore.setState({ selection: null, multiSelection: [], hoveredSectionId: null });
  frames = [];
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => frames.push(cb));
  // jsdom lays nothing out. The virtualizer sizes its scroller, and measures each
  // row, from offsetHeight: give both one, so rows render and are measured - the
  // dynamic-size mode the Sections panel runs in.
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(30);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(300);
});

afterEach(() => vi.restoreAllMocks());

const runFrames = () => {
  while (frames.length > 0) frames.shift()!(performance.now());
};

describe('SectionsPanel: a reveal pending when the panel unmounts (#576 RV9b-1)', () => {
  it('throws nothing from its frame', () => {
    const view = render(<SectionsPanel sections={SECTIONS} />);
    runFrames();

    // Select a section far down: the selection effect reveals its row.
    act(() => useUIStore.getState().setSelection({ type: 'section', sectionId: 180 }));
    expect(frames.length, 'the reveal scheduled its frame').toBeGreaterThan(0);

    // Plan mode: the panel unmounts before the frame runs.
    view.unmount();
    expect(runFrames).not.toThrow();
  });
});
