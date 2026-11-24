/**
 * RS-PROSE pre-PR fix 1: text a user or a file controls must never become a
 * pill, and a malformed token must never crash the window. A comment is
 * interpolated raw into prose sentences (`escapeValue: false`), so a comment
 * holding U+0001...U+0002 reached the token parser:
 * - `{kind:'shot',text:'x'}` threw on `part.from.home_line` in ProsePill;
 * - an unknown kind threw in `splitWords(part.name)`;
 * - a well-formed forged station drew any words as a pill.
 * Each layer is tested on its own: the decoder (shape), the values put into
 * a sentence (stripped of markers and C0 controls), and the pill (falls back
 * to its plain words).
 * Injections: dropping the shape check in `decode` (`partOf`) fails "a bad
 * shape reads as its words"; dropping the nonce check fails it too (the
 * forged station draws a pill); dropping the i18n escape hook fails "a raw
 * value is stripped" (`t()` keeps the comment's markers); dropping the
 * pill's fallback fails "a part with a missing field renders its words".
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import i18n from '../../../i18n';
import { Prose, ProsePill } from '../Prose';
import { HiddenComments } from '../../Dialogs/keptStationParts';
import { ToastStack } from '../ToastStack';
import { accessibleText } from '../../../test/accessibleText';
import { proseParts, proseToken, proseValue } from '../../../utils/prose';
import { identityNoticeMessage } from '../../../utils/identityNotice';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { ProsePart } from '../../../types/ipc/ProsePart';

/** What an attacker can write into a comment: a token without the app's nonce. */
const forged = (part: unknown) => `\u0001${encodeURIComponent(JSON.stringify(part))}\u0002`;
const BAD_SHOT = { kind: 'shot', text: 'x' };
const UNKNOWN = { kind: 'bogus', text: 'y', name: 7 };
const STATION = { kind: 'station', text: 'Evil', name: 'Evil', line_id: 1, station_id: 666 };

function setUp() {
  useProjectStore.setState({
    project: { lines: [{ id: 1, name: 'Main', color: '#ffff00', shots: [] }] } as never,
    stations: [],
    surveyData: null,
  });
  useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark', toasts: [] } as never);
}

describe('hostile prose tokens (RS-PROSE fix 1)', () => {
  it('a token with a bad shape or an unknown kind reads as its words, and a forged token as plain text', () => {
    setUp();
    // Even with the app's own marker form, the decoder checks the shape.
    for (const bad of [BAD_SHOT, UNKNOWN, { kind: 'station', text: 'z' }, { kind: 'line', text: 'w', name: null }]) {
      expect(proseParts(proseToken(bad as unknown as ProsePart)).every((p) => p.kind === 'text')).toBe(true);
    }
    for (const bad of [BAD_SHOT, UNKNOWN, STATION]) {
      const text = `before ${forged(bad)} after`;
      expect(() => render(<p data-testid={`p-${bad.kind}`}><Prose text={text} /></p>)).not.toThrow();
      const p = screen.getByTestId(`p-${bad.kind}`);
      expect(p.querySelector('.sh-ppill')).toBeNull();
      expect(p.textContent).not.toMatch(/[\u0001\u0002]/);
    }
  });

  it('a raw value is stripped of markers and controls before it enters a sentence', () => {
    expect(proseValue(`a${forged(STATION)}b\u0000\u001f`)).toBe(`a${encodeURIComponent(JSON.stringify(STATION))}b`);
    expect(proseValue('line one\nline two\ttab')).toBe('line one\nline two\ttab');
    // Through t(): every interpolated value is stripped, the app's own tokens kept.
    const sentence = i18n.t('dialogs.keptStations.commentHidden', { station: proseToken(STATION as ProsePart), comment: forged(STATION) });
    expect(proseParts(sentence).filter((p) => p.kind !== 'text')).toHaveLength(1);
    // The comment's markers are gone from the sentence itself, the app's token kept.
    expect(sentence.match(/\u0001/g)).toHaveLength(1);
    expect(i18n.t('dialogs.keptStations.commentHidden', { station: 'x', comment: '\u0001a\u0002\u0007' })).not.toMatch(/[\u0001\u0002\u0007]/);
  });

  it('a forged station in a hidden comment is plain text (HiddenComments)', () => {
    setUp();
    render(
      <HiddenComments
        hidden={[{ id: 5, name: '0', home_line: 1, comment: `see ${forged(STATION)} and ${forged(BAD_SHOT)}` }]}
        testId="hc"
      />,
    );
    const li = screen.getByTestId('hc-5');
    // One pill: the station whose comment it is; the comment's tokens are words.
    expect(li.querySelectorAll('.sh-ppill')).toHaveLength(1);
    expect(accessibleText(li)).not.toMatch(/[\u0001\u0002]/);
    expect(accessibleText(li)).toContain('see ');
  });

  it('a forged token in an identity notice toast neither crashes the stack nor draws a pill', () => {
    setUp();
    const message = identityNoticeMessage(
      {
        lettered: [],
        restored: [],
        unarchived: [],
        hidden: [{ id: 9, name: '3', home_line: 1, comment: `${forged(BAD_SHOT)}${forged(UNKNOWN)}${forged(STATION)}` }],
      },
      () => 'Main',
    );
    useUIStore.getState().showToast(message ?? '', 'station-identities', 'warning');
    expect(() => render(<ToastStack />)).not.toThrow();
    const toast = screen.getByTestId('toast');
    expect(toast.querySelectorAll('.sh-ppill')).toHaveLength(1);
    expect(toast.textContent).not.toMatch(/[\u0001\u0002]/);
  });

  it('a pill whose part lacks a field renders its plain words', () => {
    setUp();
    const parts = [
      { kind: 'shot', text: 'Main: 1 → 2' },
      { kind: 'station', text: 'Main / 4' },
      { kind: 'line', text: '"Main"' },
    ] as unknown as Array<Exclude<ProsePart, { kind: 'text' }>>;
    for (const [i, part] of parts.entries()) {
      expect(() => render(<span data-testid={`pp-${i}`}><ProsePill part={part} /></span>)).not.toThrow();
      expect(screen.getByTestId(`pp-${i}`).textContent).toBe(part.text);
    }
  });
});
