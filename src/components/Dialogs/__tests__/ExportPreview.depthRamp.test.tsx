import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, waitFor } from '@testing-library/react';

import ExportPreview from '../ExportPreview';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { DEFAULT_IMAGE_OPTIONS } from '../../../store/exportStore';
import type { ExportImageOptions } from '../../../types/commands';
import { invokeIpcMock } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');

/**
 * #360, second half. Stamping the theme's depth endpoints onto `export_image`
 * alone would have made this component LIE: its contract is that the preview is
 * "pixel-for-pixel what the backend will write", and `preview_svg_export` is a
 * separate command invoked straight from here, bypassing the store.
 *
 * It is not saved by SVG being monochrome either - what suppresses colour is
 * `options.format == Svg`, and the preview is asked for the REAL format, so
 * previewing a PNG/PDF export in depth mode does emit the ramp. Without this
 * the light theme showed a dark-ramp preview beside a themed file.
 */
describe('ExportPreview depth endpoints (#360)', () => {
  /** The options of the MOST RECENT preview request. Deliberately not `find` -
   *  the theme-change case turns on a SECOND request, which the first match
   *  would hide. */
  const previewOptions = (): ExportImageOptions => {
    const calls = invokeIpcMock.mock.calls.filter((c) => c[0] === 'preview_svg_export');
    return (calls[calls.length - 1]![1] as { options: ExportImageOptions }).options;
  };

  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockResolvedValue('<svg></svg>');
    document.documentElement.style.setProperty('--cc-depth-shallow', '#14487f');
    document.documentElement.style.setProperty('--cc-depth-deep', '#a52a1a');
    // The preview only fires for an open project.
    useProjectStore.setState({ project: { name: 'T', sections: [] } as never });
  });

  it('asks the backend for the LIVE theme ramp, matching what export writes', async () => {
    vi.useFakeTimers();
    try {
      render(<ExportPreview options={{ ...DEFAULT_IMAGE_OPTIONS, color_mode: 'depth' }} />);
      // The request is debounced behind a timer.
      await vi.advanceTimersByTimeAsync(500);
    } finally {
      vi.useRealTimers();
    }

    await waitFor(() => {
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'preview_svg_export')).toBe(true);
    });
    expect(previewOptions().depth_shallow).toBe('#14487f');
    expect(previewOptions().depth_deep).toBe('#a52a1a');
  });

  it('re-asks when the theme changes under an open preview', async () => {
    // The theme reaches the payload off the live document, not through
    // `options`, so it has to be an explicit dependency of the request. Without
    // it the Export-mode panel keeps the old ramp after an in-place theme
    // switch while the FILE follows the new one - the same preview-vs-file
    // drift, just moved. Asserting the first fetch alone cannot see this.
    const opts = { ...DEFAULT_IMAGE_OPTIONS, color_mode: 'depth' } as ExportImageOptions;
    render(<ExportPreview options={opts} />);
    await waitFor(() => expect(previewOptions().depth_shallow).toBe('#14487f'));

    // Switch to the E-Ink Kaleido ramp the way the app does: the CSS vars
    // change, and `resolvedTheme` in the store is what announces it.
    document.documentElement.style.setProperty('--cc-depth-shallow', '#0050a0');
    document.documentElement.style.setProperty('--cc-depth-deep', '#cc0000');
    useUIStore.setState({ resolvedTheme: 'kaleido_eink' as never });

    await waitFor(() => expect(previewOptions().depth_shallow).toBe('#0050a0'));
    expect(previewOptions().depth_deep).toBe('#cc0000');
  });
});
