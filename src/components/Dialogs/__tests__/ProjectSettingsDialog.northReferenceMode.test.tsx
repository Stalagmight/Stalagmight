/**
 * #576 NF-W3-2-PLAN (decided: "Default north reference is a survey edit: Survey only
 * (refused in Plan/Export like elevation)"). The reference turns every section that
 * declares none of its own, so - like the entrance elevation - the Project Settings
 * control is disabled in Plan and Export and says why, while the name and the
 * thresholds stay editable there. An unread reference is not set from Plan either.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../../i18n';
import i18n from '../../../i18n';
import { ProjectSettingsDialog as Dialog } from '../ProjectSettingsDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { defaultConfig } from '../../../types/config';
import type { AzimuthReference } from '../../../types/ipc/AzimuthReference';
import type { Project } from '../../../types/project';
import { accepted, type CommandOutcome } from '../../../store/commandResult';
import type { EditOutcome } from '../../../types/ipc/EditOutcome';

const EDITED = (): CommandOutcome<EditOutcome> => accepted<EditOutcome>({ warnings: [], remote_link: null });

let updateProjectSettings: ReturnType<typeof vi.fn>;

const stored: AzimuthReference = { north: 'magnetic', correction: 7, source: 'file_declared' };

function open() {
  useProjectStore.setState({
    project: {
      name: 'CaveB',
      location: null,
      validation: defaultConfig.validation,
      surface_elevation: null,
      default_azimuth_reference: stored,
    } as unknown as Project,
  });
  render(<Dialog />);
}

beforeEach(() => {
  updateProjectSettings = vi.fn(async (): Promise<CommandOutcome<EditOutcome>> => EDITED());
  useProjectStore.setState({
    config: defaultConfig,
    updateProjectSettings,
    setSurfaceElevation: vi.fn(async () => EDITED()),
    newerSchemaVersion: null,
    unreadableProjectSettings: [],
  });
  useUIStore.setState({ closeDialog: vi.fn(), showToast: vi.fn(), geometryMode: 'measured', appMode: 'survey' });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('Project Settings: the north reference is Survey only (#576 NF-W3-2-PLAN)', () => {
  it.each(['plan', 'export'] as const)('is disabled in %s, with the read-only reason shown', (appMode) => {
    useUIStore.setState({ appMode });
    open();

    expect(screen.getByTestId('project-settings-north-reference')).toBeDisabled();
    expect(screen.getByTestId('project-settings-north-correction')).toBeDisabled();
    expect(screen.getByTestId('project-settings-north-reference-blocked')).toHaveTextContent(
      i18n.t('dialogs.projectSettings.northReference.blocked.workspace'),
    );
    // The name stays editable there (it is on the backend's allowlist).
    expect(screen.getByTestId('project-settings-name')).toBeEnabled();
  });

  it('is enabled in Survey, with no reason shown', () => {
    open();

    expect(screen.getByTestId('project-settings-north-reference')).toBeEnabled();
    expect(screen.queryByTestId('project-settings-north-reference-blocked')).toBeNull();
  });

  it('a rename in Plan sends the stored reference unchanged', async () => {
    useUIStore.setState({ appMode: 'plan' });
    open();
    fireEvent.change(screen.getByTestId('project-settings-name'), { target: { value: 'CaveC' } });
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(updateProjectSettings).toHaveBeenCalledWith('CaveC', defaultConfig.validation, stored);
  });

  it('an unread north reference alone is not set from Plan', async () => {
    useUIStore.setState({ appMode: 'plan' });
    useProjectStore.setState({ unreadableProjectSettings: ['default_azimuth_reference'] });
    open();
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(updateProjectSettings, 'nothing the user may change there').not.toHaveBeenCalled();
    expect(useProjectStore.getState().unreadableProjectSettings).toEqual(['default_azimuth_reference']);
  });

  it('unread thresholds are still set from Plan, but the unread reference stays unread', async () => {
    useUIStore.setState({ appMode: 'plan' });
    useProjectStore.setState({ unreadableProjectSettings: ['validation', 'default_azimuth_reference'] });
    open();
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(updateProjectSettings).toHaveBeenCalledWith('CaveB', defaultConfig.validation, stored);
    expect(useProjectStore.getState().unreadableProjectSettings).toEqual(['default_azimuth_reference']);
  });
});
