/**
 * #576 B13: what the DMP parser could not read (`DmpFile.warnings` - a last
 * section cut short, a section that stops at a damaged record, a section that
 * failed to read) is shown on the section-selection step, before the user
 * picks what to import. Until then the file looked complete.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

vi.mock('../../../store/projectStore', () => {
  const state = {
    config: null,
    calculateFileHash: async () => 'hash',
    checkDuplicateImport: async () => false,
    checkSectionHashes: async (hashes: string[]) => hashes.map(() => false),
  };
  return { useProjectStore: (sel: (s: unknown) => unknown) => sel(state) };
});

vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({ testId: true }),
);

import DmpSelectionDialog from '../DmpSelectionDialog';
import type { DmpFile, DmpSection } from '../../../types/commands';
import en from '../../../i18n/locales/en.json';
import { makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

function section(name: string): DmpSection {
  return makeDmpSection({ version: 5, name, shots: makeDmpShots(3), content_hash: `hash-${name}` });
}

const CUT_SHORT =
  'Section "BBB" (#2 in the file) has no end-of-section marker, so the download may have been cut short: 3 shots were read, the rest of the file was not.';
const DAMAGED =
  'Section "AAA" (#1 in the file) stops at a damaged record (Invalid shot type value: 9) after 2 shots; the rest of that section was not read.';

function renderDialog(file: DmpFile) {
  return render(
    <DmpSelectionDialog dmpFile={file} filePath="/tmp/dive.dmp" onCancel={vi.fn()} onContinue={vi.fn()} />,
  );
}

describe('DMP selection step: parse warnings (#576 B13)', () => {
  it('lists every warning the parser returned', async () => {
    renderDialog({ version: 5, sections: [section('AAA'), section('BBB')], warnings: [DAMAGED, CUT_SHORT] });

    const banner = await screen.findByTestId('dmp-parse-warnings');
    expect(banner).toHaveTextContent('dialogs.dmpSelection.parseWarningsTitle');
    expect(screen.getByTestId('dmp-parse-warning-0')).toHaveTextContent(DAMAGED);
    expect(screen.getByTestId('dmp-parse-warning-1')).toHaveTextContent(CUT_SHORT);
    expect(en.dialogs.dmpSelection.parseWarningsTitle).toBeTruthy();
  });

  it('shows nothing for a clean file, or one from an older backend without the field', async () => {
    const { unmount } = renderDialog({ version: 5, sections: [section('AAA')], warnings: [] });
    await waitFor(() => expect(screen.getByTestId('dmp-selection-dialog')).toBeInTheDocument());
    expect(screen.queryByTestId('dmp-parse-warnings')).toBeNull();
    unmount();

    renderDialog({ version: 5, sections: [section('AAA')] } as unknown as DmpFile);
    await waitFor(() => expect(screen.getByTestId('dmp-selection-dialog')).toBeInTheDocument());
    expect(screen.queryByTestId('dmp-parse-warnings')).toBeNull();
  });
});
