/**
 * Component test - the Save As overwrite confirm reads its body copy from the
 * locale (#576 RV2-FE-5, decision D20-D1), in both variants: the file exists, or
 * the existence probe failed and the dialog must not claim it does (E10-D1).
 *
 * [#576 RV4-FE-FEB-6] The copy cases below render with the four keys OVERRIDDEN
 * by test sentences that no hard-coded English could match, so a component that
 * stopped reading the locale fails them. Reading the real locale back proved
 * nothing: its strings equal the old hard-coded English.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';

import i18n from '../../i18n';
import OverwriteConfirmDialog from './OverwriteConfirmDialog';
import { useUIStore } from '../../store/uiStore';

vi.mock('../../hooks/useProject', () => ({ useProject: () => ({ handleCloseProject: vi.fn() }) }));

/** The translated sentence with its markup stripped, as the DOM shows it. */
const plain = (key: string, fileName?: string) =>
  i18n.t(key, { fileName }).replace(/<\/?name>/g, '');

beforeEach(() => {
  useUIStore.setState({ closeDialog: vi.fn(), dialogData: { path: '/x/CaveA.stalx', fileName: 'CaveA.stalx' } });
});

/** Test sentences in place of the four keys: only a component that reads them shows them. */
const OVERRIDES = {
  message: 'TEST exists <name>{{fileName}}</name> TEST',
  messageUnknown: 'TEST unknown <name>{{fileName}}</name> TEST',
  warning: 'TEST exists warning',
  warningUnknown: 'TEST unknown warning',
};
const ORIGINAL = Object.fromEntries(
  Object.keys(OVERRIDES).map((key) => [key, i18n.t(`dialogs.overwriteConfirm.${key}`)]),
);
const setOverwriteCopy = (copy: Record<string, string>) =>
  i18n.addResourceBundle('en', 'translation', { dialogs: { overwriteConfirm: copy } }, true, true);

describe('OverwriteConfirmDialog copy', () => {
  beforeEach(() => setOverwriteCopy(OVERRIDES));
  afterEach(() => setOverwriteCopy(ORIGINAL));

  it('the file exists: the locale sentence names it, in emphasis, and the locale warning follows', () => {
    render(<OverwriteConfirmDialog />);
    const message = screen.getByTestId('overwrite-confirm-message');
    expect(message.textContent).toBe('TEST exists CaveA.stalx TEST');
    expect(message.querySelector('span')).toHaveTextContent('CaveA.stalx');
    expect(screen.getByText('TEST exists warning')).toBeInTheDocument();
    expect(screen.queryByTestId('overwrite-confirm-unknown')).toBeNull();
  });

  it('existence unknown: the "could not check" sentence and warning come from the locale', () => {
    useUIStore.setState({
      dialogData: { path: '/x/CaveA.stalx', fileName: 'CaveA.stalx', existenceUnknown: true },
    });
    render(<OverwriteConfirmDialog />);
    const message = screen.getByTestId('overwrite-confirm-unknown');
    expect(message.textContent).toBe('TEST unknown CaveA.stalx TEST');
    expect(message.querySelector('span')).toHaveTextContent('CaveA.stalx');
    expect(screen.getByText('TEST unknown warning')).toBeInTheDocument();
    // It must not claim the file is there.
    expect(screen.queryByText('TEST exists warning')).toBeNull();
  });
});

describe('OverwriteConfirmDialog real copy', () => {

  // [#576 NF-W6-1] The name is user text. Interpolated unescaped (the global
  // escapeValue: false), a "<" in it was parsed by Trans as markup: the tail of
  // the sentence landed inside a phantom tag, or text vanished.
  it.each([
    ['CaveA <B>.stalx', false],
    ['a</name>b.stalx', false],
    ['Cave & <i>x</i>.stalx', true],
  ])('a file name with markup characters renders as typed: %s', (fileName, existenceUnknown) => {
    useUIStore.setState({ dialogData: { path: `/x/${fileName}`, fileName, existenceUnknown } });
    render(<OverwriteConfirmDialog />);
    const message = screen.getByTestId(existenceUnknown ? 'overwrite-confirm-unknown' : 'overwrite-confirm-message');
    const key = existenceUnknown ? 'dialogs.overwriteConfirm.messageUnknown' : 'dialogs.overwriteConfirm.message';
    console.log(`${JSON.stringify(fileName)} -> emphasis ${JSON.stringify(message.querySelector('span')?.textContent)}; sentence ${JSON.stringify(message.textContent)}`);
    expect(message.querySelector('span')).toHaveTextContent(fileName, { normalizeWhitespace: false });
    // The sentence around a placeholder, its tags stripped, the name put back verbatim.
    expect(message.textContent).toBe(plain(key, '\u0001').replace('\u0001', fileName));
    expect(message.querySelectorAll('span')).toHaveLength(1);
  });

  it('the four keys exist in the locale (a missing key would render its own path)', () => {
    for (const key of ['message', 'messageUnknown', 'warning', 'warningUnknown']) {
      expect(i18n.exists(`dialogs.overwriteConfirm.${key}`), key).toBe(true);
    }
  });
});
