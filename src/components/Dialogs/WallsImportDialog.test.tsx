/**
 * Component test - WallsImportDialog after a successful import.
 *
 * #576 NF-DMPS-1: a rejected `suggest_orphan_placements` was swallowed
 * ("non-fatal"), so the step that offers to attach unanchored sections was
 * skipped without a word. The DMP wizard says so (NF-AN4-N4); TML, Compass and
 * Walls now do too, through one helper, and the import still completes.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/telemetry', async () =>
  (await import('../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);
vi.mock('../../utils/ipc');

import WallsImportDialog from './WallsImportDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandResult, WallsFile } from '../../types/commands';
import { invokeIpcMock } from '../../test/factories/ipc';

// Fully synthetic: one resolved survey with one shot, nothing georeferenced.
const wallsFile: WallsFile = {
  project_name: 'CaveA',
  book_count: 1,
  survey_count: 1,
  resolved_survey_count: 1,
  shot_count: 1,
  splay_count: 0,
  has_georeference: false,
  datum: null,
  utm_zone: null,
  fixed_station_count: 0,
  missing_file_count: 0,
  skipped_attachment_count: 0,
  any_feet_units: false,
  surveys: [
    {
      name: 'SurveyA',
      title: 'Survey A',
      date: '2026-01-01',
      segment: '',
      shot_count: 1,
      splay_count: 0,
      fixed_station_count: 0,
      resolved: true,
    },
  ],
  warnings: [],
};

let executeWallsImport: ReturnType<typeof vi.fn>;
let openDialog: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  closeDialog = vi.fn();
  showToast = vi.fn();
  executeWallsImport = vi.fn();
  invokeIpcMock.mockReset();
  useProjectStore.setState({
    executeWallsImport,
    calculateFileHash: vi.fn(async () => 'hash'),
    checkDuplicateImport: vi.fn(async () => false),
    startLoading: vi.fn(() => 0),
    setLoadingProgress: vi.fn(),
    resetLoading: vi.fn(),
  });
  useUIStore.setState({
    openDialog,
    closeDialog,
    showToast,
    setView: vi.fn(),
    triggerFitToBounds: vi.fn(),
  });
});

async function clickImport() {
  const btn = (await screen.findByTestId('walls-import-confirm')) as HTMLButtonElement;
  await waitFor(() => expect(btn.disabled).toBe(false));
  await userEvent.click(btn);
}

describe('WallsImportDialog orphan lookup failure (#576 NF-DMPS-1)', () => {
  it('says so in a warning toast and still completes the import', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    executeWallsImport.mockResolvedValue({ success: true, error: null, warnings: [], section_ids: [1] } as CommandResult);
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'suggest_orphan_placements') throw new Error('orphan matcher unavailable (test)');
      return 0; // count_unanchored_sections
    });
    render(<WallsImportDialog wallsFile={wallsFile} filePath="/tmp/cave.wpj" />);

    await clickImport();

    await waitFor(() => expect(closeDialog).toHaveBeenCalled()); // the import completed
    expect(showToast).toHaveBeenCalledWith(
      'Imported, but unattached sections could not be matched: orphan matcher unavailable (test)',
      'orphan-lookup-failed',
      'warning',
    );
    expect(openDialog).not.toHaveBeenCalledWith('orphan-placement', expect.anything());
  });

  it('opens orphan placement when the lookup answers, and raises no warning', async () => {
    const suggestion = { orphanSectionId: 1 };
    executeWallsImport.mockResolvedValue({ success: true, error: null, warnings: [], section_ids: [1] } as CommandResult);
    invokeIpcMock.mockImplementation(async (cmd: string) =>
      cmd === 'suggest_orphan_placements' ? [suggestion] : 0,
    );
    render(<WallsImportDialog wallsFile={wallsFile} filePath="/tmp/cave.wpj" />);

    await clickImport();

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('orphan-placement', expect.objectContaining({ suggestions: [suggestion] })),
    );
    expect(showToast).not.toHaveBeenCalledWith(expect.anything(), 'orphan-lookup-failed', expect.anything());
  });
});
