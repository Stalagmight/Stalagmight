/**
 * RS-PROSE fix 7: the conversion banner's sentence reads the same with pills
 * as plain. The plain sentence capitalises its first letter; when the first
 * clause opens with a name (a line named in lower case, "side"), the pill's
 * words were left as "side / 0" while the plain sentence said "Side / 0",
 * so a screen reader read the sentence differently with pills. Injection:
 * capitalising only an opening words part again fails this test.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../i18n';
import { ConversionNoticeBanner } from '../ConversionNoticeBanner';
import { accessibleText } from '../../test/accessibleText';
import { useUIStore } from '../../store/uiStore';
import type { ProsePart } from '../../types/ipc/ProsePart';

describe('the conversion banner opening with a name (RS-PROSE fix 7)', () => {
  it('reads the capitalised plain sentence, the pill drawing the name as it is', () => {
    useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
    const station: ProsePart = { kind: 'station', text: 'side / 0', name: '0', context: 'side', line_id: 2 };
    const clause = 'side / 0 is now shown as 2a';
    render(
      <ConversionNoticeBanner
        notice={{
          source: 'old_format',
          changes: [clause],
          details: [],
          change_parts: [[station, { kind: 'text', text: ' is now shown as 2a' }]],
          detail_parts: [],
          brings_back: false,
          file: 'not_yet_saved',
        }}
        onDismiss={() => {}}
      />,
    );
    const text = screen.getByTestId('conversion-notice-text');
    expect(accessibleText(text)).toBe('Side / 0 is now shown as 2a.');
    expect(text.querySelector('.sh-ppill [aria-hidden="true"]')!.textContent).toBe('side: 0');
  });
});
