/**
 * The conversion banner (demo issue I-5): one short lead, then only the
 * changes that apply, then ONE sentence on saving that is true for the file's
 * state, and a long list of renamed stations behind Details. The old banner
 * repeated "converted from the old file format" and kept saying "the new
 * format is written the first time the project is saved" after that save.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../i18n';
import { ConversionNoticeBanner } from '../ConversionNoticeBanner';
import type { ConversionNoticeView } from '../../types/ipc/ConversionNoticeView';

const notice = (extra: Partial<ConversionNoticeView> = {}): ConversionNoticeView => ({
  changes: ['3 stations moved by up to 2.50 m', 'a merged station\'s two comments were joined'],
  details: [],
  file: 'not_yet_saved',
  ...extra,
});

describe('ConversionNoticeBanner', () => {
  it('says the lead once, then the changes as one sentence, then how it is saved', () => {
    render(<ConversionNoticeBanner notice={notice()} onDismiss={() => {}} />);
    const banner = screen.getByTestId('conversion-notice-banner');
    expect(banner.textContent?.match(/old file format/g)).toHaveLength(1);
    expect(banner).toHaveTextContent(/^Converted from the old file format\./);
    expect(screen.getByTestId('conversion-notice-text')).toHaveTextContent(
      "3 stations moved by up to 2.50 m; a merged station's two comments were joined.",
    );
    expect(screen.getByTestId('conversion-notice-save')).toHaveTextContent(
      'It is saved in the new format the next time you save; the original file is kept and can be restored from Recent Projects (right-click, Restore an earlier copy).',
    );
    expect(screen.queryByTestId('conversion-notice-details-button')).toBeNull();
  });

  it('capitalises a first change that starts with a word', () => {
    render(<ConversionNoticeBanner notice={notice({ changes: ["a merged station's two comments were joined"] })} onDismiss={() => {}} />);
    expect(screen.getByTestId('conversion-notice-text')).toHaveTextContent("A merged station's two comments were joined.");
  });

  it.each([
    ['saved', 'Saved in the new format; the original file is kept and can be restored from Recent Projects'],
    ['saved_as_new_file', 'Saved in the new format as a new file; the original file is unchanged.'],
    ['downloaded', 'the original revision is unchanged on SpeleoDB'],
  ] as const)('says the truth about saving once the file is %s', (file, text) => {
    render(<ConversionNoticeBanner notice={notice({ file })} onDismiss={() => {}} />);
    const save = screen.getByTestId('conversion-notice-save');
    expect(save).toHaveTextContent(text);
    expect(save).not.toHaveTextContent('next time you save');
  });

  it('keeps a long list of renamed stations behind Details', async () => {
    const details = Array.from({ length: 30 }, (_, i) => `Branch ${i + 1} / 0 is now shown as ${(i + 1) * 19}`);
    render(
      <ConversionNoticeBanner
        notice={notice({ changes: ['30 stations are now shown under another name'], details })}
        onDismiss={() => {}}
      />,
    );
    expect(screen.queryByTestId('conversion-notice-details')).toBeNull();
    const button = screen.getByTestId('conversion-notice-details-button');
    expect(button).toHaveTextContent('Details');
    await userEvent.click(button);
    const list = screen.getByTestId('conversion-notice-details');
    expect(list.querySelectorAll('li')).toHaveLength(30);
    expect(list).toHaveTextContent('Branch 1 / 0 is now shown as 19');
    expect(button).toHaveTextContent('Hide details');
  });

  it('dismisses', async () => {
    const onDismiss = vi.fn();
    render(<ConversionNoticeBanner notice={notice()} onDismiss={onDismiss} />);
    await userEvent.click(screen.getByTestId('conversion-notice-dismiss-button'));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
