import { render, screen } from '@testing-library/react';
import { describe, expect, it, afterEach, beforeEach, vi } from 'vitest';
import OnboardingOverlay from '../OnboardingOverlay';
import type { ActiveOnboardingStep } from '../../../hooks/useOnboarding';
import { useProjectStore } from '../../../store/projectStore';
import { defaultConfig, type Config } from '../../../types/config';

// Partial mock, same as OnboardingOverlay.unresolvableTarget.test.tsx: the
// overlay reads `map.controls.show_navigation_tools` off the project store, and
// `src/i18n/index.ts` (pulled in by that store) needs the real `initReactI18next`.
// `t` returning the key is what makes the assertions below readable - a tool's
// label renders as its i18n key.
vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (key: string) => key }),
}));

/**
 * #409 - the tour must teach the toolbar the user actually has. The Navigate
 * group (Pan / Zoom) is off the toolbar unless
 * `map.controls.show_navigation_tools` is on, and the pan card (placement mode
 * only) teaches CTRL+drag and middle-drag, never a plain left-drag.
 *
 * The steps carry no `targetSelector`: these cases are about the illustration,
 * and an unresolvable anchor would only add the two-second give-up wait the
 * sibling spec exists for.
 */
function stepWith(illustration: 'tools-overview' | 'mouse-pan-zoom'): ActiveOnboardingStep {
  return {
    id: `nav-gating-${illustration}`,
    flowId: 'orientation',
    flowTitleKey: 'onboarding.flows.orientation.title',
    titleKey: 'onboarding.flows.orientation.steps.tools.title',
    descriptionKey: 'onboarding.flows.orientation.steps.tools.description',
    placement: 'center',
    interaction: 'locked',
    illustration,
  };
}

const props = {
  stepIndex: 0,
  stepCount: 1,
  canGoBack: false,
  onBack: vi.fn(),
  onNext: vi.fn(),
  onSkipCurrent: vi.fn(),
  onSkipAll: vi.fn(),
};

function withNavigationTools(show: boolean): Config {
  return {
    ...defaultConfig,
    map: {
      ...defaultConfig.map,
      controls: { ...defaultConfig.map.controls, show_navigation_tools: show },
    },
  };
}

describe('OnboardingOverlay navigation-tool gating (#409)', () => {
  beforeEach(() => {
    // The store is a module singleton; every case states its own config.
    useProjectStore.setState({ config: null });
  });

  afterEach(() => {
    useProjectStore.setState({ config: null });
  });

  it('omits the Navigate group while the config has not loaded', () => {
    render(<OnboardingOverlay {...props} step={stepWith('tools-overview')} />);

    expect(screen.getByTestId('onboarding-illustration-tools-overview')).toBeTruthy();
    // `navigationToolsEnabled(null)` is false, which is the right answer here:
    // the tour cannot start before the config loads, so a null config must not
    // advertise a group the toolbar may not have.
    expect(screen.queryByText('toolbar.groups.navigate')).toBeNull();
    expect(screen.queryByText('toolbar.tools.pan')).toBeNull();
    expect(screen.queryByText('toolbar.tools.zoom')).toBeNull();
    // The rest of the reference is untouched - this is a filter, not a rewrite.
    expect(screen.getByText('toolbar.tools.merge')).toBeTruthy();
    expect(screen.getByText('toolbar.tools.measure')).toBeTruthy();
  });

  it('omits the Navigate group when the setting is off', () => {
    useProjectStore.setState({ config: withNavigationTools(false) });
    render(<OnboardingOverlay {...props} step={stepWith('tools-overview')} />);

    expect(screen.queryByText('toolbar.groups.navigate')).toBeNull();
    expect(screen.queryByText('toolbar.tools.pan')).toBeNull();
    expect(screen.queryByText('toolbar.tools.zoom')).toBeNull();
    expect(screen.getByText('toolbar.tools.merge')).toBeTruthy();
  });

  it('shows the Navigate group with Pan and Zoom when the setting is on', () => {
    useProjectStore.setState({ config: withNavigationTools(true) });
    render(<OnboardingOverlay {...props} step={stepWith('tools-overview')} />);

    expect(screen.getByText('toolbar.groups.navigate')).toBeTruthy();
    expect(screen.getByText('toolbar.tools.pan')).toBeTruthy();
    expect(screen.getByText('toolbar.tools.zoom')).toBeTruthy();
    expect(screen.getByText('toolbar.tools.merge')).toBeTruthy();
  });

  it('teaches only the pan gestures that work during placement', () => {
    render(<OnboardingOverlay {...props} step={stepWith('mouse-pan-zoom')} />);

    const illustration = screen.getByTestId('onboarding-illustration-mouse-pan-zoom');
    // The illustration is shown only in placement mode, where a plain drag
    // places instead of panning (mapGestureMode), so no plain-drag row: the one
    // "Left click" is the CTRL+left gesture.
    expect(screen.queryByText('+ drag')).toBeNull();
    expect(screen.getAllByText('Left click').length).toBe(1);
    expect(illustration.textContent).toContain('CTRL');
    expect(illustration.textContent).toContain('Middle click');
  });
});
