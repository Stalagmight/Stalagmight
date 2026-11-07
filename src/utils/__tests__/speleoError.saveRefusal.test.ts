/**
 * [#576 NF-W2SAVE-1] A Done/Push whose pre-push save refused says why, in the
 * words a refused save gets - never with the save's routing marker.
 *
 * The backend used to answer every refused pre-push flush with "failed to flush
 * project before push". It now passes the save's own refusal through - its code
 * and its sentence (#576 A6) - and the push-failure banner renders it through
 * `humanizeSpeleoError`.
 */
import { describe, expect, it } from 'vitest';

import i18n from '../../i18n';
import { humanizeSpeleoError } from '../speleoError';
import type { CommandErrorCode } from '../../types/ipc/CommandErrorCode';

const dto = (code: CommandErrorCode, message: string) => ({ code, message });

describe('humanizeSpeleoError and a refused pre-push save (#576 NF-W2SAVE-1)', () => {
  it.each([
    [
      'PROJECT_SETTINGS_UNREADABLE',
      'Can\'t save: this project\'s stored validation thresholds could not be read when it was opened.',
      () => i18n.t('banners.settingsUnreadable.saveRefused'),
    ],
    [
      'NEWER_FILE_READ_ONLY',
      'Saving is disabled: this project was saved by a newer version of Stalagmight.',
      () => i18n.t('banners.newerFile.saveRefused'),
    ],
    [
      'FS_PATH_NOT_FOUND',
      'Failed to save project: the folder is gone',
      () => i18n.t('banners.autosave.pathNotFound'),
    ],
  ] as const)('names the save refusal by its code: %s', (code, message, expected) => {
    const shown = humanizeSpeleoError(dto(code, message), 'Could not push changes', { saves: true });
    // The operation frame drops a trailing period, as for every SpeleoDB error.
    expect(shown).toBe(`Could not push changes: ${expected().replace(/\.\s*$/, '')}`);
    expect(shown).not.toMatch(/[A-Z_]{8,}:/);
  });

  it('relays an uncoded save failure as the backend wrote it', () => {
    expect(humanizeSpeleoError(dto('FAILED', 'Failed to save project: disk full.'), 'Could not push changes', { saves: true })).toBe(
      'Could not push changes: Failed to save project: disk full',
    );
  });
});
