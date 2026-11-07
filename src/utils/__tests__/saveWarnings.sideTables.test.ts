/**
 * #576 RV2-R-7: a Save As away from an open file that could not be read now saves
 * with a SIDE_TABLES_NOT_CARRIED warning; it is shown in its own words, like the
 * other save warnings.
 */
import { describe, it, expect, vi } from 'vitest';

import { surfaceSaveWarnings } from '../saveFeedback';
import { useUIStore } from '../../store/uiStore';
import i18n from '../../i18n';

describe('the SIDE_TABLES_NOT_CARRIED save warning (#576 RV2-R-7)', () => {
  it('is shown in its own words', () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });

    surfaceSaveWarnings([
      { code: 'SIDE_TABLES_NOT_CARRIED', message: 'backend wording', severity: 'warning' },
    ]);

    expect(showToast).toHaveBeenCalledWith(
      i18n.t('saveWarnings.sideTablesNotCarried'),
      'save-warning:SIDE_TABLES_NOT_CARRIED',
      'warning',
    );
    expect(i18n.t('saveWarnings.sideTablesNotCarried')).toContain('ignored warnings');
  });
});
