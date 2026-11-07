/**
 * #576 NF-AN4-N4: the DMP selection step's duplicate checks swallowed a failed
 * backend call ("graceful degradation"). The check exists to deselect sections the
 * project already holds and to warn about a re-imported file, so a failed check
 * silently let the user import duplicates believing there were none. The dialog
 * still lets them continue - the check is advisory - but now says it could not run.
 *
 * #576 DMP-1: the two checks shared one notice (`fileCheckError ??
 * sectionCheckError`) whose text always said sections already in the project are
 * not flagged - also when only the FILE check failed and the section check ran and
 * flagged (and deselected) them. Each check now has its own notice; both can show.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys('keyAndOptions'));

const store = {
  config: null,
  calculateFileHash: vi.fn(async (_path: string) => 'hash'),
  checkDuplicateImport: vi.fn(async (_hash: string) => false),
  checkSectionHashes: vi.fn(async (hashes: string[]) => hashes.map(() => false)),
};
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel(store),
}));

vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({ testId: true }),
);

import DmpSelectionDialog from '../DmpSelectionDialog';
import type { DmpSection } from '../../../types/commands';
import { makeDmpFile, makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';
import en from '../../../i18n/locales/en.json';

const FILE_NOTICE = 'file-duplicate-check-failed';
const SECTION_NOTICE = 'section-duplicate-check-failed';

const section = (name: string): DmpSection =>
  makeDmpSection({ version: 5, name, shots: makeDmpShots(3), content_hash: `hash-${name}` });

const file = makeDmpFile({ version: 5, sections: [section('AAA'), section('BBB')] });

const renderDialog = () =>
  render(<DmpSelectionDialog dmpFile={file} filePath="/tmp/dive.dmp" onCancel={vi.fn()} onContinue={vi.fn()} />);

const failFileCheck = (reason: string) =>
  store.checkDuplicateImport.mockImplementation(async () => {
    throw new Error(reason);
  });
const failSectionCheck = (reason: string) =>
  store.checkSectionHashes.mockImplementation(async () => {
    throw reason;
  });

describe('DMP selection: a failed duplicate check is said, not swallowed (#576 NF-AN4-N4, DMP-1)', () => {
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    store.calculateFileHash.mockReset().mockImplementation(async () => 'hash');
    store.checkDuplicateImport.mockReset().mockImplementation(async () => false);
    store.checkSectionHashes.mockReset().mockImplementation(async (hashes: string[]) => hashes.map(() => false));
    warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    warn.mockRestore();
  });

  it('section check failed, file check ran: only the section notice, and the user may still continue', async () => {
    failSectionCheck('database is locked (test)');
    renderDialog();

    const notice = await screen.findByTestId(SECTION_NOTICE);
    expect(notice).toHaveTextContent('dialogs.dmpSelection.sectionCheckFailed');
    expect(notice).toHaveTextContent('database is locked (test)');
    await waitFor(() => expect(screen.getByTestId('continue-button')).toBeEnabled());
    expect(screen.queryByTestId(FILE_NOTICE)).not.toBeInTheDocument();
    expect(warn).toHaveBeenCalled();
  });

  it('file check failed, section check ran and flagged: only the file notice, next to the flagged sections', async () => {
    failFileCheck('no such table (test)');
    store.checkSectionHashes.mockImplementation(async (hashes: string[]) => hashes.map((_h, i) => i === 0));
    renderDialog();

    const notice = await screen.findByTestId(FILE_NOTICE);
    expect(notice).toHaveTextContent('dialogs.dmpSelection.fileCheckFailed');
    expect(notice).toHaveTextContent('no such table (test)');
    // The section check ran and flagged one of two: that warning stands, and no
    // notice claims sections are not flagged.
    expect(await screen.findByTestId('duplicate-sections-warning')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId('continue-button')).toBeEnabled());
    expect(screen.queryByTestId(SECTION_NOTICE)).not.toBeInTheDocument();
  });

  it('both checks failed: both notices, each with its own reason', async () => {
    failFileCheck('no such table (test)');
    failSectionCheck('database is locked (test)');
    renderDialog();

    const fileNotice = await screen.findByTestId(FILE_NOTICE);
    const sectionNotice = await screen.findByTestId(SECTION_NOTICE);
    expect(fileNotice).toHaveTextContent('no such table (test)');
    expect(fileNotice).not.toHaveTextContent('database is locked (test)');
    expect(sectionNotice).toHaveTextContent('database is locked (test)');
    expect(sectionNotice).not.toHaveTextContent('no such table (test)');
  });

  it('pin: says nothing when both checks ran', async () => {
    renderDialog();

    await waitFor(() => expect(screen.getByTestId('continue-button')).toBeEnabled());
    expect(screen.queryByTestId(FILE_NOTICE)).not.toBeInTheDocument();
    expect(screen.queryByTestId(SECTION_NOTICE)).not.toBeInTheDocument();
  });

  it('copy: only the section notice says duplicates are not flagged', () => {
    const copy = en.dialogs.dmpSelection;
    expect(copy.fileCheckFailed).toBe('Could not check whether this file was imported before ({{reason}}).');
    expect(copy.sectionCheckFailed).toBe(
      'Could not check for sections already in the project ({{reason}}) - duplicates are not flagged.',
    );
    expect('duplicateCheckFailed' in copy).toBe(false);
  });
});
