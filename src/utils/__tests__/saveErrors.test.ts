import { describe, it, expect } from 'vitest';
import {
  EXTERNALLY_MODIFIED,
  FILE_MISSING,
  READ_ONLY_FILE,
  FS_PATH_NOT_FOUND,
  FS_PATH_TOO_LONG,
  FS_PERMISSION_DENIED,
  IGNORES_UNREADABLE,
  classifySaveError,
  isFileMissingError,
  isPathError,
  isPermissionError,
  stripSaveErrorMarker,
} from '../saveErrors';

describe('isPermissionError', () => {
  it('detects the backend permission marker prefix', () => {
    expect(
      isPermissionError(`${FS_PERMISSION_DENIED}: Failed to create save file: ...`)
    ).toBe(true);
  });

  it('does not flag unrelated save errors', () => {
    // The crux: a real corruption / generic failure must not be treated as a
    // permission problem (which would show the wrong remediation).
    expect(isPermissionError('Failed to save project: disk full')).toBe(false);
    expect(isPermissionError('Database integrity check failed')).toBe(false);
  });

  it('does not flag a destination failure', () => {
    // #429: the missing-folder and over-MAX_PATH markers used to arrive as
    // FS_PERMISSION_DENIED, which is what put Windows users in front of the
    // macOS folder-access dialog.
    expect(isPermissionError(`${FS_PATH_NOT_FOUND}: Cannot save to Z:\\cave.stalx`)).toBe(false);
    expect(isPermissionError(`${FS_PATH_TOO_LONG}: Failed to create save file`)).toBe(false);
  });

  it('handles null/undefined safely', () => {
    expect(isPermissionError(null)).toBe(false);
    expect(isPermissionError(undefined)).toBe(false);
    expect(isPermissionError('')).toBe(false);
  });
});

describe('classifySaveError', () => {
  it('maps each backend marker to its own kind', () => {
    expect(classifySaveError(`${FS_PERMISSION_DENIED}: denied`)).toBe('permission');
    expect(classifySaveError(`${FS_PATH_NOT_FOUND}: the folder no longer exists`)).toBe(
      'path-not-found'
    );
    expect(classifySaveError(`${FS_PATH_TOO_LONG}: too long`)).toBe('path-too-long');
    expect(classifySaveError(`${IGNORES_UNREADABLE}: could not read the ignore list`)).toBe(
      'ignores-unreadable'
    );
    // The `.stalx` v2 refusals (plan 6.4, 6.5).
    expect(classifySaveError(`${EXTERNALLY_MODIFIED}: cave.stalx changed on disk`)).toBe('externally-modified');
    expect(classifySaveError(`${READ_ONLY_FILE}: made with a newer Stalagmight`)).toBe('read-only-file');
  });

  // Pre-PR review finding 1: a file that is GONE is still the guard's refusal
  // (the save prompt), but marked so the prompt offers no reopen of it.
  it('tells a moved or deleted file from a changed one', () => {
    const missing = `${EXTERNALLY_MODIFIED}: ${FILE_MISSING}: /c/cave.stalx was moved or deleted by another program`;
    expect(classifySaveError(missing)).toBe('externally-modified');
    expect(isFileMissingError(missing)).toBe(true);
    expect(isFileMissingError(`${EXTERNALLY_MODIFIED}: /c/cave.stalx was changed by another program`)).toBe(false);
    // Only right after the guard's code: a path that happens to contain it is not.
    expect(isFileMissingError(`${EXTERNALLY_MODIFIED}: /c/${FILE_MISSING}: cave.stalx was changed`)).toBe(false);
    expect(isFileMissingError(`${FS_PATH_NOT_FOUND}: ${FILE_MISSING}: x`)).toBe(false);
    expect(isFileMissingError(null)).toBe(false);
  });

  // [#517] The marker exists to keep this refusal out of the Review & Repair
  // route; it must not be mistaken for part of the sentence.
  it('takes the marker off a message that is already user-ready', () => {
    expect(stripSaveErrorMarker(`${IGNORES_UNREADABLE}: Can't save: nothing was written.`, IGNORES_UNREADABLE)).toBe(
      "Can't save: nothing was written."
    );
    expect(stripSaveErrorMarker('Disk full', IGNORES_UNREADABLE)).toBe('Disk full');
  });

  it('treats an unmarked error - and no error at all - as other', () => {
    expect(classifySaveError('Failed to save project: no space left on device')).toBe('other');
    expect(classifySaveError(null)).toBe('other');
    expect(classifySaveError(undefined)).toBe('other');
  });
});

describe('isPathError', () => {
  it('covers both destination failures and nothing else', () => {
    expect(isPathError(`${FS_PATH_NOT_FOUND}: gone`)).toBe(true);
    expect(isPathError(`${FS_PATH_TOO_LONG}: 335 chars`)).toBe(true);
    expect(isPathError(`${FS_PERMISSION_DENIED}: denied`)).toBe(false);
    expect(isPathError('Failed to save project')).toBe(false);
    expect(isPathError(null)).toBe(false);
  });
});
