/**
 * [#576 B15 follow-up] Stored settings the open could not read refuse every save
 * until the user sets them again. Saving Project Settings is that act: while the
 * thresholds, the default north reference or the elevation are unread, Save sends
 * them even when unchanged (the values shown are the defaults standing in), and each
 * one the backend accepts leaves the unread list - so the banner and autosave follow.
 * The north reference got its control in #576 NF-W3-2; until then it stayed unread
 * until the project was reopened, and saving stayed off with it.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../../i18n';
import { ProjectSettingsDialog as Dialog } from '../ProjectSettingsDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { defaultConfig } from '../../../types/config';
import type { Project } from '../../../types/project';
import type { CommandResult } from '../../../types/commands';

let updateProjectSettings: ReturnType<typeof vi.fn>;
let setSurfaceElevation: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateProjectSettings = vi.fn(async (): Promise<CommandResult> => ({ success: true, error: null }) as CommandResult);
  setSurfaceElevation = vi.fn(async (): Promise<CommandResult> => ({ success: true, error: null }) as CommandResult);
  useProjectStore.setState({
    project: {
      name: 'CaveB',
      location: null,
      validation: defaultConfig.validation,
      surface_elevation: null,
    } as unknown as Project,
    config: defaultConfig,
    updateProjectSettings,
    setSurfaceElevation,
    unreadableProjectSettings: ['validation', 'surface_elevation', 'default_azimuth_reference'],
  });
  useUIStore.setState({ closeDialog: vi.fn(), showToast: vi.fn(), geometryMode: 'measured', appMode: 'survey' });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('ProjectSettingsDialog with unread stored settings (#576 B15 follow-up)', () => {
  it('Save sets the unread thresholds, north reference and elevation even unchanged, and lifts them', async () => {
    render(<Dialog />);
    await userEvent.click(await screen.findByTestId('project-settings-save'));

    expect(updateProjectSettings).toHaveBeenCalledWith('CaveB', defaultConfig.validation, null);
    expect(setSurfaceElevation).toHaveBeenCalledWith(null);
    expect(useProjectStore.getState().unreadableProjectSettings, 'still unread: saving stays off').toEqual([]);
  });

  it('an unread north reference alone is set by Save too', async () => {
    useProjectStore.setState({ unreadableProjectSettings: ['default_azimuth_reference'] });
    render(<Dialog />);
    await userEvent.click(await screen.findByTestId('project-settings-save'));

    expect(updateProjectSettings).toHaveBeenCalledWith('CaveB', defaultConfig.validation, null);
    expect(useProjectStore.getState().unreadableProjectSettings).toEqual([]);
  });

  it('a refused write leaves the setting unread', async () => {
    setSurfaceElevation.mockResolvedValueOnce({ success: false, error: 'EDIT_DISABLED: no' } as CommandResult);
    render(<Dialog />);
    await userEvent.click(await screen.findByTestId('project-settings-save'));

    expect(useProjectStore.getState().unreadableProjectSettings).toContain('surface_elevation');
  });

  it('with nothing unread, an untouched Save still sends nothing', async () => {
    useProjectStore.setState({ unreadableProjectSettings: [] });
    render(<Dialog />);
    await userEvent.click(await screen.findByTestId('project-settings-save'));

    expect(updateProjectSettings).not.toHaveBeenCalled();
    expect(setSurfaceElevation).not.toHaveBeenCalled();
  });
});
