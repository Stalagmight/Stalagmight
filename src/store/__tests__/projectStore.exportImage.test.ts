// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { DEFAULT_IMAGE_OPTIONS } from '../exportStore';
import type { ExportImageOptions } from '../../types/commands';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * #360: depth-mode PNG and PDF export drew the DARK theme's ramp whatever the
 * active theme was, because the endpoints were hardcoded in Rust. That was
 * correct until #332 - `getMapThemeColors()` used to force the dark pair for
 * every theme - and silently wrong afterwards, worst on `light`, where a teal →
 * coral ramp lands on the export's white page at 1.13:1.
 *
 * The fix threads the resolved endpoints across the IPC boundary rather than
 * duplicating the per-theme table in a second language. These guard the half
 * that can regress quietly: the sending. (That the backend HONOURS them is
 * pinned in `image.rs::depth_ramp_takes_the_themes_endpoints_and_rejects_junk`.)
 */
describe('projectStore.exportImage depth endpoints (#360)', () => {
  const setTheme = (shallow: string, deep: string) => {
    document.documentElement.style.setProperty('--cc-depth-shallow', shallow);
    document.documentElement.style.setProperty('--cc-depth-deep', deep);
  };

  const sentOptions = (): ExportImageOptions => {
    const calls = invokeIpcMock.mock.calls;
    return (calls[calls.length - 1]![1] as { options: ExportImageOptions }).options;
  };

  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockResolvedValue(undefined);
  });

  it('sends the LIVE theme endpoints, not the compiled-in dark pair', async () => {
    setTheme('#14487f', '#a52a1a'); // the light theme's navy → brick

    await useProjectStore.getState().exportImage('/tmp/a.png', {
      ...DEFAULT_IMAGE_OPTIONS,
      color_mode: 'depth',
    });

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'export_image',
      expect.objectContaining({ path: '/tmp/a.png' }),
      expect.anything(),
    );
    expect(sentOptions().depth_shallow).toBe('#14487f');
    expect(sentOptions().depth_deep).toBe('#a52a1a');
  });

  it('follows a theme change rather than snapshotting the first one', async () => {
    setTheme('#4ecdc4', '#ff6b6b');
    await useProjectStore.getState().exportImage('/tmp/dark.png', DEFAULT_IMAGE_OPTIONS);
    const dark = { ...sentOptions() };

    setTheme('#0050a0', '#cc0000');
    await useProjectStore.getState().exportImage('/tmp/eink.png', DEFAULT_IMAGE_OPTIONS);

    // Two exports of the same options under two themes must not agree, or a
    // cached/snapshotted lookup would pass this suite while shipping one ramp.
    expect(sentOptions().depth_shallow).not.toBe(dark.depth_shallow);
    expect(sentOptions().depth_shallow).toBe('#0050a0');
    expect(sentOptions().depth_deep).toBe('#cc0000');
  });

  it('leaves the caller\'s own options untouched', async () => {
    setTheme('#14487f', '#a52a1a');
    const options: ExportImageOptions = { ...DEFAULT_IMAGE_OPTIONS, dpi: 600 };

    await useProjectStore.getState().exportImage('/tmp/a.pdf', options);

    expect(sentOptions().dpi).toBe(600);
    // The store must not mutate the object the export UI holds in state.
    expect(options.depth_shallow).toBeUndefined();
  });
});
