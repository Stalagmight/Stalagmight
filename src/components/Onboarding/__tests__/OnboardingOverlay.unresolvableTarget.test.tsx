import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import OnboardingOverlay from '../OnboardingOverlay';
import type { ActiveOnboardingStep } from '../../../hooks/useOnboarding';

// Partial mock: the overlay reads `map.controls.show_navigation_tools` off the
// project store, and `src/i18n/index.ts` (pulled in by that store) needs the
// real `initReactI18next`.
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys());

const step: ActiveOnboardingStep = {
  id: 'segment-placement',
  flowId: 'dmp-segment-placement',
  flowTitleKey: 'onboarding.flows.dmpImport.title',
  titleKey: 'onboarding.flows.dmpImport.steps.segmentPlacement.title',
  descriptionKey: 'onboarding.flows.dmpImport.steps.segmentPlacement.description',
  // Deliberately absent from the DOM: the case where a step out-lives its
  // anchor (the wizard hid itself for on-map placement).
  targetSelector: '[data-testid="a-target-that-does-not-exist"]',
  placement: 'left',
  interaction: 'locked',
};

const props = {
  step,
  stepIndex: 0,
  stepCount: 1,
  canGoBack: false,
  onBack: vi.fn(),
  onNext: vi.fn(),
  onSkipCurrent: vi.fn(),
  onSkipAll: vi.fn(),
};

describe('OnboardingOverlay with an unresolvable target', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('stops waiting and shows the step card instead of an undismissable dim', () => {
    render(<OnboardingOverlay {...props} />);

    // `hidden: true` because the card starts out visibility:hidden, which is
    // exactly the state under test.
    const card = screen.getByRole('dialog', { hidden: true });
    // Before giving up it is legitimately hidden while it waits to anchor.
    expect(card).toHaveStyle({ visibility: 'hidden' });

    act(() => {
      vi.advanceTimersByTime(2000);
    });

    // The trap this guards against: staying hidden forever under a full-screen
    // dim, with `locked` swallowing the Escape that renders its confirmation
    // inside this same hidden card (#331).
    expect(card).toHaveStyle({ visibility: 'visible' });
    expect(card).toHaveStyle({ pointerEvents: 'auto' });
  });
});
