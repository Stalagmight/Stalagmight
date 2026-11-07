/**
 * The Ariane export's azimuth notes reach the user (#438).
 *
 * `export_tml` returns note CODES for the two cases where the single-flag
 * `.tml` format cannot carry what the project knows about its north references:
 * either every bearing was rotated on the way out, or the document could not be
 * made consistent at all. Both change what the file MEANS relative to the
 * project, and the export is the only moment the user is looking at it.
 *
 * Before this, the only surface was a `tracing::warn` in the backend - which is
 * to say, none. The assertions below are about the wiring, not the copy: that a
 * code becomes a toast, that it is translated rather than shown raw, and that an
 * ordinary export stays silent.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const showToastMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc');
stubGetErrorMessage(stringOfError);

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { invokeIpcMock, stubGetErrorMessage, stringOfError } from '../../test/factories/ipc';

describe('exportTml surfaces the backend azimuth notes', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(useUIStore.getState(), 'showToast').mockImplementation(showToastMock);
  });

  it('says nothing for an ordinary project', async () => {
    invokeIpcMock.mockResolvedValue([]);

    await useProjectStore.getState().exportTml('/tmp/cave.tml');

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'export_tml',
      { path: '/tmp/cave.tml' },
      expect.anything(),
    );
    expect(showToastMock).not.toHaveBeenCalled();
  });

  it('raises a warning toast per note, translated, keyed per code', async () => {
    invokeIpcMock.mockResolvedValue(['azimuth-rotated-to-true']);

    await useProjectStore.getState().exportTml('/tmp/cave.tml');

    expect(showToastMock).toHaveBeenCalledTimes(1);
    const [message, key, kind] = showToastMock.mock.calls[0] as [string, string, string];
    // Per CODE, not the bare shared key: two notes from one export must not
    // dedup into a single toast (#378 added a second possible code).
    expect(key).toBe(`${TOAST_KEYS.EXPORT_NOTE}:azimuth-rotated-to-true`);
    // `warning`, per docs/notifications.md: the export SUCCEEDED, with a caveat.
    expect(kind).toBe('warning');
    // Translated, not the raw code - a code on screen is the defect this asserts
    // against, and the i18n key has to exist for the message to differ from it.
    expect(message).not.toBe('azimuth-rotated-to-true');
    expect(message.length).toBeGreaterThan(0);
  });

  it('translates the unresolved-mix note too', async () => {
    invokeIpcMock.mockResolvedValue(['azimuth-mixed-unresolved']);

    await useProjectStore.getState().exportTml('/tmp/cave.tml');

    const [message] = showToastMock.mock.calls[0] as [string];
    expect(message).not.toBe('azimuth-mixed-unresolved');
  });

  /**
   * #576 B11: text the file could not carry was replaced with U+FFFD. The
   * export succeeded, so this is a translated warning, never the raw code.
   */
  it('translates the replaced-characters note', async () => {
    invokeIpcMock.mockResolvedValue(['xml-chars-replaced']);

    await useProjectStore.getState().exportTml('/tmp/cave.tml');

    expect(showToastMock).toHaveBeenCalledTimes(1);
    const [message, key, kind] = showToastMock.mock.calls[0] as [string, string, string];
    expect(message).not.toBe('xml-chars-replaced');
    expect(message).toContain('�');
    expect(key).toBe(`${TOAST_KEYS.EXPORT_NOTE}:xml-chars-replaced`);
    expect(kind).toBe('warning');
  });

  /**
   * A backend that grows a third note must not crash the export: an unknown
   * code falls back to itself rather than rendering the i18n key path.
   */
  it('falls back to the code for a note this build does not know', async () => {
    invokeIpcMock.mockResolvedValue(['azimuth-something-new']);

    await useProjectStore.getState().exportTml('/tmp/cave.tml');

    const [message] = showToastMock.mock.calls[0] as [string];
    expect(message).toBe('azimuth-something-new');
  });

  /** Two notes from ONE export keep distinct keys, so neither replaces the other. */
  it('keeps two simultaneous notes on distinct toast keys', async () => {
    invokeIpcMock.mockResolvedValue(['azimuth-rotated-to-true', 'gps-anchors-not-written']);

    await useProjectStore.getState().exportTml('/tmp/cave.tml');

    expect(showToastMock).toHaveBeenCalledTimes(2);
    const keys = showToastMock.mock.calls.map((call) => call[1] as string);
    expect(new Set(keys).size).toBe(2);
  });

  /** A command that returns nothing at all (an older backend) is not a crash. */
  it('tolerates a missing notes array', async () => {
    invokeIpcMock.mockResolvedValue(undefined);

    await expect(useProjectStore.getState().exportTml('/tmp/cave.tml')).resolves.toBeUndefined();
    expect(showToastMock).not.toHaveBeenCalled();
  });
});
