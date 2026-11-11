/**
 * Component test: the Display menu during a Warnings-tab visit. The Survey
 * Issues and Errors pills show what the map paints (on by default while the
 * Warnings tab is open), flagged as automatic, with a note saying the changes
 * last until the user leaves the tab.
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
import { emptySurveyData } from '../../../types/survey-data';

const ISSUES = 'display-toggle-showIssueSeverity';
const ERRORS = 'display-toggle-showShotErrors';
const SECTION = 'display-color-section';
const NOTE = 'display-warnings-visit-note';

async function openMenu() {
  render(<DisplayMenu />);
  await userEvent.click(screen.getByTestId('display-menu-button'));
}

const pressed = (id: string) => screen.getByTestId(id).getAttribute('aria-pressed');
const auto = (id: string) => screen.getByTestId(id).getAttribute('data-auto');

const saveDisplaySetting = vi.fn(async () => undefined);

beforeEach(() => {
  saveDisplaySetting.mockClear();
  useUIStore.setState({
    view: 'single',
    appMode: 'survey',
    geometryMode: 'measured',
    syntacticLevel: 'off',
    detailPanelOpen: true,
    detailPanelTab: 'details',
    dataTableFullScreen: false,
    warningsAutoDisplay: true,
    warningsVisitOverride: null,
    showIssueSeverity: false,
    showDepthGradient: false,
    showDistanceGradient: false,
    showShotErrors: false,
    saveDisplaySetting,
  });
  useProjectStore.setState({ project: null, surveyData: emptySurveyData() });
});

describe('DisplayMenu: Errors pill', () => {
  it('sits with the labels and saves like any toggle outside a visit', async () => {
    await openMenu();
    expect(screen.getByTestId(ERRORS)).toHaveTextContent('displayMenu.items.shotErrors');
    expect(pressed(ERRORS)).toBe('false');
    await userEvent.click(screen.getByTestId(ERRORS));
    expect(pressed(ERRORS)).toBe('true');
    expect(auto(ERRORS)).toBeNull();
    expect(saveDisplaySetting).toHaveBeenCalledWith('showShotErrors', true);
    expect(screen.queryByTestId(NOTE)).toBeNull();
  });
});

describe('DisplayMenu during a Warnings-tab visit', () => {
  beforeEach(() => useUIStore.setState({ detailPanelTab: 'warnings' }));

  it('shows Survey Issues and Errors on, marked automatic, with the note', async () => {
    await openMenu();
    expect(pressed(ISSUES)).toBe('true');
    expect(pressed(SECTION)).toBe('false');
    expect(auto(ISSUES)).toBe('true');
    expect(pressed(ERRORS)).toBe('true');
    expect(auto(ERRORS)).toBe('true');
    expect(screen.getByTestId(ISSUES).getAttribute('title')).toContain('displayMenu.warningsVisit.pillHint');
    expect(screen.getByTestId(NOTE)).toHaveTextContent('displayMenu.warningsVisit.note');
  });

  it('a pick during the visit moves the pill, drops the auto mark and saves nothing', async () => {
    await openMenu();
    await userEvent.click(screen.getByTestId(SECTION));
    expect(pressed(SECTION)).toBe('true');
    expect(pressed(ISSUES)).toBe('false');
    expect(auto(ISSUES)).toBeNull();
    await userEvent.click(screen.getByTestId(ERRORS));
    expect(pressed(ERRORS)).toBe('false');
    expect(auto(ERRORS)).toBeNull();
    expect(saveDisplaySetting).not.toHaveBeenCalled();
    // The note stays: the changes still end with the visit.
    expect(screen.getByTestId(NOTE)).toBeInTheDocument();
  });

  it('with the setting off the menu shows the saved settings', async () => {
    useUIStore.setState({ warningsAutoDisplay: false });
    await openMenu();
    expect(pressed(SECTION)).toBe('true');
    expect(pressed(ERRORS)).toBe('false');
    expect(screen.queryByTestId(NOTE)).toBeNull();
  });
});
