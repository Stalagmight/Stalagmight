import { describe, expect, it } from 'vitest';
import '../../i18n';
import { isEditDisabledError, isReadOnlyFileError, READ_ONLY_FILE_NOTICE } from '../editErrors';

describe('READ_ONLY_FILE', () => {
  it('is recognised and worded for the user, never shown raw', () => {
    const error = 'READ_ONLY_FILE: this project was made with a newer Stalagmight and is open read-only';
    expect(isReadOnlyFileError(error)).toBe(true);
    expect(isEditDisabledError(error)).toBe(false);
    expect(isReadOnlyFileError('EDIT_DISABLED: no lock')).toBe(false);
    expect(READ_ONLY_FILE_NOTICE()).toContain('newer version of Stalagmight');
    expect(READ_ONLY_FILE_NOTICE()).not.toContain('READ_ONLY_FILE');
  });
});
