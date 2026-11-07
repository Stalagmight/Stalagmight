/**
 * #576 NF-W3-2 (decided: "Add a Default north reference control to Project Settings
 * (lifts the unread state)"): the reference a section that declares no north of its
 * own inherits could not be set anywhere in the app, so an unread one kept saving off
 * until the project was reopened. Project Settings now has the control; it saves with
 * the name and the thresholds (`update_project_settings`).
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../../i18n';
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

// jsdom does not implement scrollIntoView; the Select component calls it on open.
Element.prototype.scrollIntoView = vi.fn();

let updateProjectSettings: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

function openWith(reference: AzimuthReference | null) {
  useProjectStore.setState({
    project: {
      name: 'CaveB',
      location: null,
      validation: defaultConfig.validation,
      surface_elevation: null,
      default_azimuth_reference: reference,
    } as unknown as Project,
  });
  render(<Dialog />);
}

async function choose(north: string) {
  await userEvent.click(screen.getByTestId('project-settings-north-reference'));
  await userEvent.click(screen.getByTestId(`project-settings-north-reference-option-${north}`));
}

const save = () => userEvent.click(screen.getByTestId('project-settings-save'));

beforeEach(() => {
  updateProjectSettings = vi.fn(async (): Promise<CommandOutcome<EditOutcome>> => EDITED());
  closeDialog = vi.fn();
  useProjectStore.setState({
    config: defaultConfig,
    updateProjectSettings,
    setSurfaceElevation: vi.fn(async () => EDITED()),
    newerSchemaVersion: null,
    unreadableProjectSettings: [],
  });
  useUIStore.setState({ closeDialog, showToast: vi.fn(), geometryMode: 'measured', appMode: 'survey' });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('Project Settings: Default north reference (#576 NF-W3-2)', () => {
  it('sets magnetic north with a declination, as a hand-entered correction', async () => {
    openWith(null);
    await choose('magnetic');
    fireEvent.change(screen.getByTestId('project-settings-north-correction'), { target: { value: '-3.5' } });
    await save();

    expect(updateProjectSettings).toHaveBeenCalledWith('CaveB', defaultConfig.validation, {
      north: 'magnetic',
      correction: -3.5,
      source: 'manual',
    });
    expect(closeDialog).toHaveBeenCalled();
  });

  it('true north carries no correction, and shows no field for one', async () => {
    openWith(null);
    await choose('true');
    expect(screen.queryByTestId('project-settings-north-correction')).toBeNull();
    await save();

    expect(updateProjectSettings).toHaveBeenCalledWith('CaveB', defaultConfig.validation, {
      north: 'true',
      correction: 0,
      source: 'manual',
    });
  });

  it('back to "Not recorded" clears it', async () => {
    openWith({ north: 'grid', correction: 1.25, source: 'manual' });
    expect(screen.getByTestId('project-settings-north-correction')).toHaveProperty('value', '1.25');
    await choose('none');
    await save();

    expect(updateProjectSettings).toHaveBeenCalledWith('CaveB', defaultConfig.validation, null);
  });

  it('an untouched Save changes nothing, and keeps where a stored correction came from', async () => {
    const stored: AzimuthReference = { north: 'magnetic', correction: 7, source: 'file_declared' };
    openWith(stored);
    await save();
    expect(updateProjectSettings, 'nothing changed, nothing unread').not.toHaveBeenCalled();

    // With the thresholds changed, the stored reference rides along exactly as it was.
    fireEvent.change(screen.getByTestId('project-settings-name'), { target: { value: 'CaveC' } });
    await save();
    expect(updateProjectSettings).toHaveBeenCalledWith('CaveC', defaultConfig.validation, stored);
  });

  it('a correction that is not a number within a turn blocks Save', async () => {
    openWith(null);
    await choose('magnetic');
    fireEvent.change(screen.getByTestId('project-settings-north-correction'), { target: { value: '400' } });

    expect(screen.getByTestId('project-settings-save')).toBeDisabled();
  });

  it('is disabled, with the reason, on a project whose settings cannot change', () => {
    useProjectStore.setState({ newerSchemaVersion: 99 });
    openWith(null);

    expect(screen.getByTestId('project-settings-north-reference')).toBeDisabled();
  });
});
