/**
 * Step 1 of the mNemo import: the default Surveyors / Explorers for every
 * section are pill fields, prefilled with the people the last section was
 * created or imported with this session (`uiStore.lastUsedCredit`), handed to
 * the wizard as `defaultSurveyors` / `defaultExplorers`, and remembered again
 * on Continue.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (k: string) => k }),
}));

const dirtySpy = vi.fn<(dirty: boolean | undefined) => void>();
vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children, isDirty }: { children: React.ReactNode; isDirty?: boolean }) => {
    dirtySpy(isDirty);
    return <div>{children}</div>;
  },
}));

import DmpSelectionDialog, { type DmpSelectionConfig } from '../DmpSelectionDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import type { DmpFile } from '../../../types/commands';

const shot = {
  shot_type: 'Standard',
  heading_in: 10,
  heading_out: 10,
  length: 5,
  depth_in: 0,
  depth_out: 1,
  pitch_in: 0,
  pitch_out: 0,
  left: 0,
  right: 0,
  up: 0,
  down: 0,
  temperature: 0,
  hour: 0,
  minute: 0,
  second: 0,
  marker_index: 0,
  comment: null,
};

const dmpFile = {
  version: 6,
  sections: [
    { name: 'A', date: '2026-01-01T00:00:00', direction: 'in', shots: [shot, shot, shot], content_hash: 'h-a' },
  ],
} as unknown as DmpFile;

const lastDirty = () => dirtySpy.mock.calls[dirtySpy.mock.calls.length - 1]?.[0] ?? false;

function renderDialog(onContinue = vi.fn<(config: DmpSelectionConfig) => void>()) {
  render(<DmpSelectionDialog dmpFile={dmpFile} filePath="/tmp/a.dmp" onCancel={vi.fn()} onContinue={onContinue} />);
  return onContinue;
}

describe('DmpSelectionDialog default people', () => {
  beforeEach(() => {
    dirtySpy.mockClear();
    useProjectStore.setState({
      calculateFileHash: vi.fn(async () => 'hash'),
      checkDuplicateImport: vi.fn(async () => false),
      checkSectionHashes: vi.fn(async (hashes: string[]) => hashes.map(() => false)),
    } as never);
  });

  afterEach(() => {
    useUIStore.getState().setLastUsedCredit({ surveyors: [], explorers: [] });
  });

  it('prefills the pills from the last used people, and opens clean', async () => {
    useUIStore.getState().setLastUsedCredit({
      surveyors: [{ name: 'Ann' }],
      explorers: [{ name: 'Bo', email: 'bo@example.org' }],
    });
    renderDialog();
    await act(async () => {});

    expect(screen.getByTestId('default-surveyors-pill-0-label')).toHaveTextContent('Ann');
    expect(screen.getByTestId('default-explorers-pill-0-label')).toHaveTextContent('Bo');
    expect(lastDirty()).toBe(false);
  });

  it('hands the pills to the wizard and remembers them on Continue', async () => {
    // Injection: drop the `setLastUsedCredit` call in handleContinue and the
    // next import (and New Section) opens with nobody.
    const user = userEvent.setup();
    const onContinue = renderDialog();
    await act(async () => {});

    await user.type(screen.getByTestId('default-surveyors-input'), 'Ann{Enter}Cy;');
    await user.type(screen.getByTestId('default-explorers-input'), 'Dee{Enter}');
    expect(lastDirty()).toBe(true);

    fireEvent.click(screen.getByTestId('continue-button'));

    expect(onContinue).toHaveBeenCalledTimes(1);
    const config = onContinue.mock.calls[0]![0];
    expect(config.defaultSurveyors).toEqual([{ name: 'Ann' }, { name: 'Cy' }]);
    expect(config.defaultExplorers).toEqual([{ name: 'Dee' }]);
    expect(useUIStore.getState().lastUsedCredit).toEqual({
      surveyors: [{ name: 'Ann' }, { name: 'Cy' }],
      explorers: [{ name: 'Dee' }],
    });
  });

  it('counts a name typed but not added yet as unsaved', async () => {
    renderDialog();
    await act(async () => {});
    expect(lastDirty()).toBe(false);

    fireEvent.change(screen.getByTestId('default-surveyors-input'), { target: { value: 'Half typed' } });
    expect(lastDirty()).toBe(true);
  });

  it('is clean again once a removed prefilled person is added back', async () => {
    useUIStore.getState().setLastUsedCredit({ surveyors: [{ name: 'Ann' }], explorers: [] });
    const user = userEvent.setup();
    renderDialog();
    await act(async () => {});

    fireEvent.click(screen.getByTestId('default-surveyors-pill-0-remove'));
    expect(lastDirty()).toBe(true);
    await user.type(screen.getByTestId('default-surveyors-input'), 'Ann{Enter}');
    expect(lastDirty()).toBe(false);
  });
});
