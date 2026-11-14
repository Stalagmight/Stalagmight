import { describe, it, expect } from 'vitest';
import i18n from '../index';

/**
 * The Warnings tab header (`DetailPanel`, `detailPanel.warnings.count`) is
 * plural-aware: it read "1 warnings". Pinned against the REAL i18n instance,
 * since outside dev a missing key renders empty rather than loudly.
 */
describe('detailPanel.warnings.count', () => {
  const header = (count: number) =>
    i18n.t('detailPanel.warnings.count', { count, countFormatted: String(count) });

  it('is singular for one warning', () => {
    expect(header(1)).toBe('1 warning');
  });

  it('is plural for none and for many', () => {
    expect(header(0)).toBe('0 warnings');
    expect(header(2)).toBe('2 warnings');
    expect(header(1234)).toBe('1234 warnings');
  });
});
