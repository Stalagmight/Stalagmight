/**
 * The workspace carousel is Survey | Data | Plan. Export left it: it opens from
 * the project menu (or Ctrl+E), and while it is open no carousel button is
 * pressed - clicking one is how the user leaves Export for that mode.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue({}) }));

import '../../../i18n';
import { ModeCarousel } from '../ModeCarousel';
import { useUIStore } from '../../../store/uiStore';

const buttons = () => screen.getAllByRole('button');
const pressed = () => buttons().filter((b) => b.getAttribute('aria-pressed') === 'true');

beforeEach(() => {
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
});

describe('ModeCarousel', () => {
  it('shows exactly Survey, Data and Plan, in that order', () => {
    render(<ModeCarousel />);
    expect(buttons().map((b) => b.getAttribute('data-testid'))).toEqual(['mode-survey', 'mode-data', 'mode-plan']);
    expect(buttons().map((b) => b.textContent)).toEqual(['Survey', 'Data', 'Plan']);
  });

  it('names every mode in the compact (icon-only) variant', () => {
    render(<ModeCarousel compact />);
    expect(screen.getByTestId('mode-data')).toHaveAccessibleName('Data');
  });

  it('clicking Data enters Data and presses only its button', () => {
    render(<ModeCarousel />);
    fireEvent.click(screen.getByTestId('mode-data'));
    expect(useUIStore.getState().appMode).toBe('data');
    expect(pressed().map((b) => b.getAttribute('data-testid'))).toEqual(['mode-data']);
  });

  it('presses nothing while Export is open, and a click leaves Export for that mode', () => {
    useUIStore.getState().setAppMode('export');
    render(<ModeCarousel />);
    expect(pressed()).toHaveLength(0);
    fireEvent.click(screen.getByTestId('mode-data'));
    expect(useUIStore.getState().appMode).toBe('data');
    // Leaving the read-only mode restored the editable view it had forced away.
    expect(useUIStore.getState().geometryMode).toBe('measured');
  });

  it('is disabled during a placement', () => {
    render(<ModeCarousel disabled />);
    for (const b of buttons()) expect(b).toBeDisabled();
  });
});
