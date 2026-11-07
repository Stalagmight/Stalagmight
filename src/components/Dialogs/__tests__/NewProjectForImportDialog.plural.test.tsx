/**
 * #447 item 1: the create-for-import summary used to interpolate a raw count
 * into a hardcoded plural ("1 sections - 12 shots"). Renders through the real
 * i18next instance (unlike the sibling suite's stubbed `t`) so the `_one`/
 * `_other` plural forms in en.json are actually exercised.
 *
 * Also covers the #447 review's follow-up: every format's summary line (not
 * just DMP's), and that the trailing cave/track/project-name suffix survives
 * the switch from one interpolated sentence to composed pluralised clauses.
 *
 * `useUIStore` is mocked here as a bare hook return value (not `.setState` on
 * the real store, unlike the sibling `ErrorDialog.test.tsx`) because
 * `NewProjectForImportDialog` only ever calls `useUIStore` as a hook (with a
 * selector, which the mock applies) - it never reaches for
 * `useUIStore.getState()` - so a hook-shaped mock is a faithful stand-in for
 * every code path this suite exercises.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

import '../../../i18n';

const dialogData: {
  dmpFile: { sections: Array<{ shots: Array<{ shot_type: string }> }> } | undefined;
  tmlFile: { sections: unknown[]; cave_name?: string } | undefined;
  compassFile:
    | {
        survey_count: number;
        shot_count: number;
        excluded_shot_count?: number;
        invalid_shot_counts?: {
          non_excluded_corrected: number;
          non_excluded_uncorrected: number;
          excluded_corrected: number;
          excluded_uncorrected: number;
        };
        cave_name?: string;
      }
    | undefined;
  wallsFile: { resolved_survey_count: number; shot_count: number; project_name?: string } | undefined;
  gpxFile: { waypoints: unknown[]; tracks: unknown[]; routes: unknown[]; name?: string | null } | undefined;
  filePath?: string;
  importType?: string;
  defaultName?: string;
} = {
  dmpFile: undefined,
  tmlFile: undefined,
  compassFile: undefined,
  wallsFile: undefined,
  gpxFile: undefined,
};

const closeDialog = vi.fn();
const openDialog = vi.fn();
const setImportContext = vi.fn();

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData, openDialog, setImportContext }),
}));

const createProject = vi.fn();
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ createProject }),
}));

vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({ title: 'testid' }),
);

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

import NewProjectForImportDialog from '../NewProjectForImportDialog';

function dmpFileWithSections(count: number): { sections: Array<{ shots: Array<{ shot_type: string }> }> } {
  return {
    sections: Array.from({ length: count }, () => ({ shots: [{ shot_type: 'Standard' }] })),
  };
}

function resetDialogData(): void {
  dialogData.dmpFile = undefined;
  dialogData.tmlFile = undefined;
  dialogData.compassFile = undefined;
  dialogData.wallsFile = undefined;
  dialogData.gpxFile = undefined;
  dialogData.filePath = '/cave/exports/survey.dmp';
  dialogData.importType = 'dmp';
  dialogData.defaultName = 'Survey';
}

describe('NewProjectForImportDialog summary plurals (#447)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetDialogData();
  });

  it('uses the singular form for exactly one section and one shot (DMP)', () => {
    dialogData.dmpFile = dmpFileWithSections(1);

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('1 section - 1 shot')).toBeInTheDocument();
  });

  it('uses the plural form for multiple sections and shots (DMP)', () => {
    dialogData.dmpFile = dmpFileWithSections(3);

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('3 sections - 3 shots')).toBeInTheDocument();
  });

  it('uses the plural form for zero sections/shots (DMP)', () => {
    dialogData.dmpFile = dmpFileWithSections(0);

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('0 sections - 0 shots')).toBeInTheDocument();
  });

  it('pluralizes the TML summary and keeps the cave-name suffix', () => {
    dialogData.tmlFile = { sections: [], cave_name: 'Naharon' };
    dialogData.importType = 'tml';

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('0 sections - 0 shots - Naharon')).toBeInTheDocument();
  });

  it('pluralizes the Compass summary (surveys_one) and keeps the cave-name suffix', () => {
    dialogData.compassFile = {
      survey_count: 1,
      shot_count: 1,
      excluded_shot_count: 0,
      invalid_shot_counts: {
        non_excluded_corrected: 0,
        non_excluded_uncorrected: 0,
        excluded_corrected: 0,
        excluded_uncorrected: 0,
      },
      cave_name: 'Naharon',
    };
    dialogData.importType = 'compass';

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('1 survey - 1 shot - Naharon')).toBeInTheDocument();
  });

  it('pluralizes the Compass summary (surveys_other) with no cave name', () => {
    dialogData.compassFile = {
      survey_count: 2,
      shot_count: 5,
      excluded_shot_count: 0,
      invalid_shot_counts: {
        non_excluded_corrected: 0,
        non_excluded_uncorrected: 0,
        excluded_corrected: 0,
        excluded_uncorrected: 0,
      },
      cave_name: '',
    };
    dialogData.importType = 'compass';

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('2 surveys - 5 shots')).toBeInTheDocument();
  });

  it('pluralizes the Walls summary and keeps the project-name suffix', () => {
    dialogData.wallsFile = { resolved_survey_count: 1, shot_count: 1, project_name: 'Deep System' };
    dialogData.importType = 'walls';

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('1 survey - 1 shot - Deep System')).toBeInTheDocument();
  });

  it('pluralizes the GPX summary (waypoints/tracks/routes) and keeps the track-name suffix', () => {
    dialogData.gpxFile = {
      waypoints: [1],
      tracks: [1, 2],
      routes: [],
      name: 'Surface Trail',
    };
    dialogData.importType = 'gpx';

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('1 waypoint - 2 tracks - 0 routes - Surface Trail')).toBeInTheDocument();
  });

  it('pluralizes the GPX summary with no track name (null)', () => {
    dialogData.gpxFile = { waypoints: [], tracks: [], routes: [1], name: null };
    dialogData.importType = 'gpx';

    render(<NewProjectForImportDialog />);

    expect(screen.getByText('0 waypoints - 0 tracks - 1 route')).toBeInTheDocument();
  });
});
