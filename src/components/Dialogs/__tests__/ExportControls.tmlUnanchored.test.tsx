/**
 * The TML export's "lines without a zero datum connection" notice lists each
 * line by its name alone: line names are project-unique, so the internal line
 * id (`Main Line (#3)`, `Line 3`) is never shown (SPEC 4.1).
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
  initReactI18next: { type: '3rdParty', init: () => undefined },
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: vi.fn(async () => undefined),
}));

import ExportControls from '../ExportControls';
import { DEFAULT_IMAGE_OPTIONS } from '../../../store/exportStore';
import { useProjectStore } from '../../../store/projectStore';

describe('ExportControls TML unanchored-lines notice', () => {
  it('lists each unanchored line by its name, never its id', async () => {
    useProjectStore.setState({
      project: {
        lines: [
          { id: 3, name: 'Main Line', shots: [] },
          { id: 7, name: 'Side', shots: [] },
          { id: 9, name: null, shots: [] },
        ],
      },
      // Only line 7 reaches a zero datum.
      getAnchoredLineIds: vi.fn(async () => [7]),
    } as never);
    render(
      <ExportControls
        variant="panel"
        exportType="data"
        onExportTypeChange={() => undefined}
        dataFormat="tml"
        onDataFormatChange={() => undefined}
        imageOptions={DEFAULT_IMAGE_OPTIONS}
        onImageOptionsChange={() => undefined}
      />
    );
    await userEvent.click(screen.getByTestId('export-button'));

    await waitFor(() => expect(screen.getByTestId('tml-unanchored-section-item-3')).toBeInTheDocument());
    expect(screen.getByTestId('tml-unanchored-section-item-3')).toHaveTextContent(/^Main Line$/);
    // A line without a name (an older file) reads as "Unnamed line", not `Line 9`.
    expect(screen.getByTestId('tml-unanchored-section-item-9')).not.toHaveTextContent('9');
    expect(screen.queryByTestId('tml-unanchored-section-item-7')).toBeNull();
  });
});
