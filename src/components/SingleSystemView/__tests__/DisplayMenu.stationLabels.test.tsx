/**
 * Component test: the Station IDs / Station names pills in the Display menu.
 *
 * The two pills are one choice - the map's station label is either the ID or
 * the name - so clicking one turns the other off. The Station names pill is
 * offered only when the project has names: with none it would only switch the
 * IDs off and label nothing. If it is already on (a saved choice), it stays
 * clickable so it can be turned off.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// Keys, not copy: the strings are pinned by the i18n/e2e tiers.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
  initReactI18next: { type: '3rdParty', init: () => undefined },
}));

import DisplayMenu from '../DisplayMenu';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { emptySurveyData, type SurveyData } from '../../../types/survey-data';

const IDS = 'display-toggle-showStationLabels';
const NAMES = 'display-toggle-showStationNames';

function surveyWithNames(names: Array<[number, string]>): SurveyData {
  const d = emptySurveyData();
  d.stationNames = new Map(names);
  return d;
}

async function openMenu() {
  render(<DisplayMenu />);
  await userEvent.click(screen.getByTestId('display-menu-button'));
}

const pressed = (testId: string): string | null => screen.getByTestId(testId).getAttribute('aria-pressed');

beforeEach(() => {
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    syntacticLevel: 'off',
    showGrid: false,
    showStationLabels: false,
    showStationNames: false,
    // No IPC in a component test; the save order is pinned in the store tests.
    saveDisplaySetting: vi.fn(async () => undefined),
  });
  useProjectStore.setState({ project: null, surveyData: surveyWithNames([[0, '7']]) });
});

describe('DisplayMenu station label pills', () => {
  it('renders Station names right after Station IDs, with their own labels', async () => {
    await openMenu();
    const ids = screen.getByTestId(IDS);
    const names = screen.getByTestId(NAMES);
    expect(ids).toHaveTextContent('displayMenu.items.stationLabels');
    expect(names).toHaveTextContent('displayMenu.items.stationNames');
    expect(names.getAttribute('title')).toBe('displayMenu.descriptions.stationNames');
    expect(ids.nextElementSibling).toBe(names);
  });

  it('turning one on turns the other off; both can be off', async () => {
    await openMenu();
    await userEvent.click(screen.getByTestId(IDS));
    expect([pressed(IDS), pressed(NAMES)]).toEqual(['true', 'false']);
    await userEvent.click(screen.getByTestId(NAMES));
    expect([pressed(IDS), pressed(NAMES)]).toEqual(['false', 'true']);
    await userEvent.click(screen.getByTestId(IDS));
    expect([pressed(IDS), pressed(NAMES)]).toEqual(['true', 'false']);
    await userEvent.click(screen.getByTestId(IDS));
    expect([pressed(IDS), pressed(NAMES)]).toEqual(['false', 'false']);
  });

  it('is disabled, saying why, when the project has no station names', async () => {
    useProjectStore.setState({ surveyData: surveyWithNames([]) });
    useUIStore.setState({ showStationLabels: true });
    await openMenu();
    const names = screen.getByTestId(NAMES);
    expect(names).toBeDisabled();
    expect(names.getAttribute('title')).toBe('displayMenu.descriptions.stationNamesUnavailable');
    await userEvent.click(names);
    // The click did nothing: the IDs stay on.
    expect([pressed(IDS), pressed(NAMES)]).toEqual(['true', 'false']);
  });

  it('is disabled with no project data at all', async () => {
    useProjectStore.setState({ surveyData: null });
    await openMenu();
    expect(screen.getByTestId(NAMES)).toBeDisabled();
  });

  it('stays clickable while on even without names, so it can be turned off', async () => {
    useProjectStore.setState({ surveyData: surveyWithNames([]) });
    useUIStore.setState({ showStationNames: true });
    await openMenu();
    const names = screen.getByTestId(NAMES);
    expect(names).toBeEnabled();
    await userEvent.click(names);
    expect(pressed(NAMES)).toBe('false');
    // Now off with no names: offered no more.
    expect(screen.getByTestId(NAMES)).toBeDisabled();
  });
});
