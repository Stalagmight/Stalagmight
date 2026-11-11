/**
 * Component test (#465) - entrance elevation display + edit in the Project
 * Settings dialog's General tab.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../i18n';
import { ProjectSettingsDialog as Dialog } from './ProjectSettingsDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { defaultConfig } from '../../types/config';
import type { Project } from '../../types/project';
import type { CommandResult } from '../../types/commands';

let updateProjectSettings: ReturnType<typeof vi.fn>;
let setSurfaceElevation: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

function baseProject(overrides: Partial<Project> = {}): Project {
  return {
    name: 'Test Project',
    location: null,
    validation: defaultConfig.validation,
    surface_elevation: null,
    ...overrides,
  } as unknown as Project;
}

beforeEach(() => {
  updateProjectSettings = vi.fn(async (): Promise<CommandResult> => ({ success: true, error: null }) as CommandResult);
  setSurfaceElevation = vi.fn(async (): Promise<CommandResult> => ({ success: true, error: null }) as CommandResult);
  closeDialog = vi.fn();
  useProjectStore.setState({
    project: baseProject(),
    config: defaultConfig,
    updateProjectSettings,
    setSurfaceElevation,
  });
  useUIStore.setState({ closeDialog, showToast: vi.fn(), geometryMode: 'measured', appMode: 'survey' });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('ProjectSettingsDialog entrance elevation (#465)', () => {
  it('shows the not-recorded state explicitly, never 0.0', async () => {
    render(<Dialog />);
    const input = await screen.findByTestId('project-settings-elevation');
    // jest-dom reports an empty `type="number"` input's value as `null`.
    expect(input).toHaveValue(null);
    expect(input).toHaveAttribute('placeholder', 'Not recorded');
  });

  it('displays a previously recorded elevation', async () => {
    useProjectStore.setState({ project: baseProject({ surface_elevation: 42.5 }) });
    render(<Dialog />);
    const input = await screen.findByTestId('project-settings-elevation');
    expect(input).toHaveValue(42.5);
  });

  it('saves a typed elevation via setSurfaceElevation, in meters', async () => {
    render(<Dialog />);
    const input = await screen.findByTestId('project-settings-elevation');
    // fireEvent sets the value atomically - `userEvent.type` on a
    // `type="number"` input hits a jsdom quirk where intermediate strings
    // like "42." are treated as invalid and reset to "".
    fireEvent.change(input, { target: { value: '42.5' } });
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(setSurfaceElevation).toHaveBeenCalledWith(42.5);
    expect(closeDialog).toHaveBeenCalled();
  });

  it('clears a recorded elevation back to null (not 0.0)', async () => {
    useProjectStore.setState({ project: baseProject({ surface_elevation: 42.5 }) });
    render(<Dialog />);
    const input = await screen.findByTestId('project-settings-elevation');
    fireEvent.change(input, { target: { value: '' } });
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(setSurfaceElevation).toHaveBeenCalledWith(null);
  });

  it('does not call setSurfaceElevation when the value is unchanged', async () => {
    useProjectStore.setState({ project: baseProject({ surface_elevation: 42.5 }) });
    render(<Dialog />);
    await screen.findByTestId('project-settings-elevation');
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(setSurfaceElevation).not.toHaveBeenCalled();
  });

  it('scopes the read-only gate to the elevation field only: Save stays live and saves the name, elevation input is disabled and never written', async () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    useProjectStore.setState({ project: baseProject({ surface_elevation: 10 }) });
    render(<Dialog />);
    const elevationInput = await screen.findByTestId('project-settings-elevation');
    expect(elevationInput).toBeDisabled();
    const saveButton = screen.getByTestId('project-settings-save');
    expect(saveButton).toBeEnabled();

    fireEvent.change(screen.getByTestId('project-settings-name'), { target: { value: 'Renamed' } });
    await userEvent.click(saveButton);

    expect(updateProjectSettings).toHaveBeenCalledWith('Renamed', defaultConfig.validation);
    expect(setSurfaceElevation).not.toHaveBeenCalled();
  });

  it('does not rewrite an unchanged elevation finer than display precision (metric)', async () => {
    // 1234.56m rounds to "1234.6" at precision 1 on display; re-parsing and
    // re-converting THAT back to meters must not be compared against the
    // stored 1234.56 - the draft-string comparison must recognise "untouched".
    useProjectStore.setState({
      project: baseProject({ surface_elevation: 1234.56 }),
      config: {
        ...defaultConfig,
        units: { ...defaultConfig.units, depth: 'meters', decimal_precision: { ...defaultConfig.units.decimal_precision, depth: 1 } },
      },
    });
    render(<Dialog />);
    await screen.findByTestId('project-settings-elevation');
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(setSurfaceElevation).not.toHaveBeenCalled();
  });

  it('does not rewrite an unchanged elevation in a feet-unit project', async () => {
    // 42.5m -> "139.4" ft on display; converting "139.4"ft back to meters
    // lands at ~42.4894m, off the stored 42.5m by more than float noise.
    useProjectStore.setState({
      project: baseProject({ surface_elevation: 42.5 }),
      config: { ...defaultConfig, units: { ...defaultConfig.units, depth: 'feet' } },
    });
    render(<Dialog />);
    await screen.findByTestId('project-settings-elevation');
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(setSurfaceElevation).not.toHaveBeenCalled();
  });

  it('a changed draft is attempted even when editingBlocked flips true before Save; on EDIT_DISABLED it keeps the dialog open, shows the error, and keeps the staged draft', async () => {
    setSurfaceElevation = vi.fn(
      async (): Promise<CommandResult> =>
        ({ success: false, error: 'EDIT_DISABLED: Set entrance elevation is disabled: this SpeleoDB project is open read-only. Acquire the lock to edit.' }) as CommandResult,
    );
    useProjectStore.setState({ setSurfaceElevation });
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    render(<Dialog />);
    const input = await screen.findByTestId('project-settings-elevation');
    fireEvent.change(input, { target: { value: '42.5' } });

    // Read-only flips true AFTER the draft was typed, BEFORE Save is clicked.
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(setSurfaceElevation).toHaveBeenCalledWith(42.5);
    expect(closeDialog).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('EDIT_DISABLED'), expect.anything(), 'error');
    expect(screen.getByTestId('project-settings-elevation')).toHaveValue(42.5);
  });

  it('a changed draft still saves when editingBlocked is (stale-)true but the backend accepts it', async () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    render(<Dialog />);
    const input = await screen.findByTestId('project-settings-elevation');
    expect(input).toBeDisabled();
    // The disabled input can't be typed into directly by a user while blocked,
    // but a value staged just before the flip (previous test) must still save;
    // simulate that staged state by changing the draft via fireEvent (bypasses
    // the DOM disabled attribute, mirroring "typed, then blocked").
    fireEvent.change(input, { target: { value: '42.5' } });
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(setSurfaceElevation).toHaveBeenCalledWith(42.5);
    expect(closeDialog).toHaveBeenCalled();
  });
});

it('Local settings retains local name and elevation controls without SpeleoDB forms', async () => {
  render(<Dialog />);
  expect(await screen.findByRole('heading', { name: 'Local settings' })).toBeVisible();
  expect(screen.getByTestId('project-settings-name')).toBeEnabled();
  expect(screen.getByTestId('project-settings-elevation')).toBeEnabled();
  expect(screen.queryByTestId('project-settings-tab-speleodb')).not.toBeInTheDocument();
  expect(screen.queryByTestId('speleodb-account-login-form')).not.toBeInTheDocument();
});

it('Local settings keeps linked names immutable and respects the remote edit gate', async () => {
  useSpeleoStore.setState({ link: { editState: 'read-only' } as never, projectLinkKind: 'remote' });
  render(<Dialog />);
  expect(await screen.findByRole('heading', { name: 'Local settings' })).toBeVisible();
  expect(screen.getByTestId('project-settings-name')).toBeDisabled();
  expect(screen.getByTestId('project-settings-elevation')).toBeDisabled();
  expect(screen.queryByTestId('speleodb-settings-name')).not.toBeInTheDocument();
});
