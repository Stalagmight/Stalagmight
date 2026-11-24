/**
 * The conversion banner (demo issue I-5; kept stations, plan 2026-11-23 RS
 * 8.2 and 8.3): one short lead for the kind of file converted, then only the
 * changes that apply, then what brings a station or comment back when one is
 * now hidden, then ONE sentence on saving that is true for the file's state,
 * and a long list behind Details. The old banner repeated "converted from the
 * old file format" and kept saying "the new format is written the first time
 * the project is saved" after that save.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../i18n';
import { ConversionNoticeBanner } from '../ConversionNoticeBanner';
import type { ConversionNoticeView } from '../../types/ipc/ConversionNoticeView';
import type { ProsePart } from '../../types/ipc/ProsePart';
import { useUIStore } from '../../store/uiStore';
import { partsToPlain } from '../../utils/prose';
import { accessibleText } from '../../test/accessibleText';

const notice = (extra: Partial<ConversionNoticeView> = {}): ConversionNoticeView => ({
  source: 'old_format',
  changes: ['3 stations moved by up to 2.50 m', 'the comment of Side / 0 is hidden'],
  details: [],
  change_parts: [],
  detail_parts: [],
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

  describe('prose pills (RS-PROSE)', () => {
    const station = (line_id: number, line: string, name: string, station_id: number): ProsePart => ({
      kind: 'station', text: `${line} / ${name}`, line_id, context: line, name, station_id,
    });
    const shownAs: ProsePart[] = [
      station(2, 'Side', '0', 20),
      { kind: 'text', text: ' is now shown as ' },
      station(1, 'Main', '12', 11),
    ];
    const hidden: ProsePart[] = [
      { kind: 'text', text: 'the comment of ' },
      station(2, 'Side', '0', 20),
      { kind: 'text', text: ' is hidden' },
    ];

    beforeEach(() => {
      useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
    });

    it('draws the stations a clause names and reads the sentence plain text wrote', () => {
      const changes = ['3 stations moved by up to 2.50 m', partsToPlain(shownAs), partsToPlain(hidden)];
      render(
        <ConversionNoticeBanner
          notice={notice({ changes, change_parts: [[], shownAs, hidden], brings_back: true })}
          onDismiss={() => {}}
        />,
      );
      const text = screen.getByTestId('conversion-notice-text');
      const pills = [...text.querySelectorAll('.sh-ppill')];
      expect(pills.map((p) => p.getAttribute('data-prose-kind'))).toEqual(['station', 'station', 'station']);
      expect(pills[0]!.querySelector('[aria-hidden="true"]')!.textContent).toBe('Side: 0#20');
      expect(accessibleText(text)).toBe(
        '3 stations moved by up to 2.50 m; Side / 0 is now shown as Main / 12; the comment of Side / 0 is hidden.',
      );
    });

    it('capitalises the sentence as the plain one does; an opening name is drawn as it is', () => {
      const { unmount } = render(
        <ConversionNoticeBanner
          notice={notice({ changes: [partsToPlain(hidden)], change_parts: [hidden] })}
          onDismiss={() => {}}
        />,
      );
      expect(accessibleText(screen.getByTestId('conversion-notice-text'))).toBe('The comment of Side / 0 is hidden.');
      unmount();
      const lower: ProsePart[] = [station(3, 'annex', '2', 30), { kind: 'text', text: ' is now shown as 2a' }];
      render(
        <ConversionNoticeBanner
          notice={notice({ changes: [partsToPlain(lower)], change_parts: [lower] })}
          onDismiss={() => {}}
        />,
      );
      // RS-PROSE fix 7: the words read capitalised, as the plain sentence
      // (changesSentence) writes them; the pill draws the line's own name.
      const text = screen.getByTestId('conversion-notice-text');
      expect(accessibleText(text)).toBe('Annex / 2 is now shown as 2a.');
      expect(text.querySelector('.sh-ppill [aria-hidden="true"]')!.textContent).toMatch(/^annex: 2/);
    });

    it('draws the names of each detail, and a clause without parts as its words', async () => {
      const details = [partsToPlain(shownAs), 'Branch 1 / 0 is now shown as Main / 19'];
      render(
        <ConversionNoticeBanner
          notice={notice({
            changes: ["2 stations are now shown under another station's name"],
            change_parts: [[]],
            details,
            detail_parts: [shownAs, []],
          })}
          onDismiss={() => {}}
        />,
      );
      expect(screen.getByTestId('conversion-notice-text').querySelector('.sh-ppill')).toBeNull();
      await userEvent.click(screen.getByTestId('conversion-notice-details-button'));
      const items = screen.getByTestId('conversion-notice-details').querySelectorAll('li');
      expect(items[0]!.querySelectorAll('.sh-ppill')).toHaveLength(2);
      expect(accessibleText(items[0]!)).toBe(details[0]);
      expect(items[1]!.querySelector('.sh-ppill')).toBeNull();
      expect(items[1]!.textContent).toBe(details[1]);
    });

    it('is the plain sentence below Full', () => {
      useUIStore.setState({ syntacticLevel: 'off' });
      render(
        <ConversionNoticeBanner
          notice={notice({ changes: [partsToPlain(shownAs)], change_parts: [shownAs] })}
          onDismiss={() => {}}
        />,
      );
      const text = screen.getByTestId('conversion-notice-text');
      expect(text.querySelector('.sh-ppill')).toBeNull();
      expect(text.textContent).toBe('Side / 0 is now shown as Main / 12.');
    });
  });

  it('dismisses', async () => {
    const onDismiss = vi.fn();
    render(<ConversionNoticeBanner notice={notice()} onDismiss={onDismiss} />);
    await userEvent.click(screen.getByTestId('conversion-notice-dismiss-button'));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
