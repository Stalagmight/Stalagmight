/**
 * The conversion banner (demo issue I-5; kept stations, plan 2026-11-23 RS
 * 8.2 and 8.3): one short lead for the kind of file converted, then only the
 * changes that apply, then what brings a station or comment back when one is
 * now hidden, then ONE sentence on saving that is true for the file's state,
 * and a long list behind Details. The old banner repeated "converted from the
 * old file format" and kept saying "the new format is written the first time
 * the project is saved" after that save.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../i18n';
import { ConversionNoticeBanner } from '../ConversionNoticeBanner';
import type { ConversionNoticeView } from '../../types/ipc/ConversionNoticeView';

const notice = (extra: Partial<ConversionNoticeView> = {}): ConversionNoticeView => ({
  source: 'old_format',
  changes: ['3 stations moved by up to 2.50 m', 'the comment of Side / 0 is hidden'],
  details: [],
  brings_back: false,
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
      '3 stations moved by up to 2.50 m; the comment of Side / 0 is hidden.',
    );
    expect(screen.queryByTestId('conversion-notice-brings-back')).toBeNull();
    expect(screen.getByTestId('conversion-notice-save')).toHaveTextContent(
      'It is saved in the new format the next time you save; the original file is kept and can be restored from Recent Projects (right-click, Restore an earlier copy).',
    );
    expect(screen.queryByTestId('conversion-notice-details-button')).toBeNull();
  });

  it('capitalises a first change that starts with a word', () => {
    render(<ConversionNoticeBanner notice={notice({ changes: ['the comment of Side / 0 is hidden'] })} onDismiss={() => {}} />);
    expect(screen.getByTestId('conversion-notice-text')).toHaveTextContent('The comment of Side / 0 is hidden.');
  });

  it('leads an earlier version 2 file with its own lead', () => {
    render(<ConversionNoticeBanner notice={notice({ source: 'earlier_version_2' })} onDismiss={() => {}} />);
    const lead = screen.getByTestId('conversion-notice-lead');
    expect(lead).toHaveTextContent('Upgraded from an earlier version 2 file.');
    expect(screen.getByTestId('conversion-notice-banner')).not.toHaveTextContent('old file format');
  });

  it('says what brings a station shown as another, or a hidden comment, back', () => {
    render(
      <ConversionNoticeBanner
        notice={notice({ changes: ['Side / 0 is now shown as Main / 12'], brings_back: true })}
        onDismiss={() => {}}
      />,
    );
    expect(screen.getByTestId('conversion-notice-brings-back')).toHaveTextContent(
      'Detach Line from Station or Remove Closure brings each one back.',
    );
  });

  it.each([
    ['saved', 'Saved in the new format; the original file is kept and can be restored from Recent Projects'],
    ['saved_as_new_file', 'Saved in the new format as a new file; the original file is unchanged.'],
    ['downloaded', 'the original revision is unchanged on SpeleoDB'],
    ['read_only', 'It is only shown converted: the file is open read-only, so nothing is saved'],
  ] as const)('says the truth about saving once the file is %s', (file, text) => {
    render(<ConversionNoticeBanner notice={notice({ file })} onDismiss={() => {}} />);
    const save = screen.getByTestId('conversion-notice-save');
    expect(save).toHaveTextContent(text);
    expect(save).not.toHaveTextContent('next time you save');
  });

  it('keeps a long list of renamed stations behind Details', async () => {
    const details = Array.from({ length: 30 }, (_, i) => `Branch ${i + 1} / 0 is now shown as Main / ${(i + 1) * 19}`);
    render(
      <ConversionNoticeBanner
        notice={notice({ changes: ["30 stations are now shown under another station's name"], details })}
        onDismiss={() => {}}
      />,
    );
    expect(screen.queryByTestId('conversion-notice-details')).toBeNull();
    const button = screen.getByTestId('conversion-notice-details-button');
    expect(button).toHaveTextContent('Details');
    await userEvent.click(button);
    const list = screen.getByTestId('conversion-notice-details');
    expect(list.querySelectorAll('li')).toHaveLength(30);
    expect(list).toHaveTextContent('Branch 1 / 0 is now shown as Main / 19');
    expect(button).toHaveTextContent('Hide details');
  });

  it('dismisses', async () => {
    const onDismiss = vi.fn();
    render(<ConversionNoticeBanner notice={notice()} onDismiss={onDismiss} />);
    await userEvent.click(screen.getByTestId('conversion-notice-dismiss-button'));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
