/**
 * Render smoke for the component-test tier (issue #107).
 *
 * Proves the jsdom + Testing Library harness end-to-end: a real component
 * mounts to the DOM, jest-dom matchers resolve, and user-event drives a
 * handler. `Button` is deliberately dependency-free (no store / i18n / IPC), so
 * this asserts the *harness*, not app wiring - the bug-prone-wiring regression
 * tests (e.g. wizard onComplete → save) land in a follow-up PR.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { Button } from './Button';

describe('Button (component-tier harness smoke)', () => {
  it('renders its children into the DOM', () => {
    render(<Button>Save project</Button>);
    expect(screen.getByRole('button', { name: 'Save project' })).toBeInTheDocument();
  });

  it('fires onClick when activated via user-event', async () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Import</Button>);

    await userEvent.click(screen.getByRole('button', { name: 'Import' }));

    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('does not fire onClick while disabled', async () => {
    const onClick = vi.fn();
    render(
      <Button onClick={onClick} disabled>
        Import
      </Button>,
    );

    await userEvent.click(screen.getByRole('button', { name: 'Import' }));

    expect(screen.getByRole('button', { name: 'Import' })).toBeDisabled();
    expect(onClick).not.toHaveBeenCalled();
  });

  it('tags the primary variant with data-primary-action (dialog focus heuristic)', () => {
    render(<Button variant="primary">Confirm</Button>);
    expect(screen.getByRole('button', { name: 'Confirm' })).toHaveAttribute('data-primary-action', '');
  });

  it('has no yellow highlight variant: a main action is primary', () => {
    render(
      // The warning-yellow `highlight` variant is gone: the type check fails if it returns.
      // @ts-expect-error highlight is not a Button variant
      <Button variant="highlight">Open project</Button>,
    );
    expect(screen.getByRole('button', { name: 'Open project' }).className).not.toMatch(/warning/);
  });

  it('tints its icon with iconColor on the unfilled variants only', () => {
    render(
      <>
        <Button icon={<svg data-testid="i-secondary" />} iconColor="#3fb6b6" data-testid="b-secondary">Save</Button>
        <Button variant="ghost" icon={<svg />} iconColor="#3fb6b6" data-testid="b-ghost">Save</Button>
        <Button variant="primary" icon={<svg />} iconColor="#3fb6b6" data-testid="b-primary">Save</Button>
        <Button variant="danger" icon={<svg />} iconColor="#3fb6b6" data-testid="b-danger">Save</Button>
        <Button icon={<svg />} data-testid="b-plain">Save</Button>
      </>,
    );
    const iconSpan = (id: string) => screen.getByTestId(id).querySelector('span');
    expect(iconSpan('b-secondary')).toHaveStyle({ color: '#3fb6b6' });
    expect(iconSpan('b-ghost')).toHaveStyle({ color: '#3fb6b6' });
    // A filled button keeps its own text colour on the icon: a tint could vanish on the fill.
    expect(iconSpan('b-primary')?.getAttribute('style')).toBeNull();
    expect(iconSpan('b-danger')?.getAttribute('style')).toBeNull();
    expect(iconSpan('b-plain')?.getAttribute('style')).toBeNull();
  });

  it('fills the secondary variant with its own colour, not the panel it sits on', () => {
    render(<Button data-testid="b">Reload</Button>);
    const cls = screen.getByTestId('b').className;
    expect(cls).toContain('bg-button-secondary');
    expect(cls).toContain('hover:bg-button-secondary-hover');
    expect(cls).not.toMatch(/(^|\s)bg-panel(\s|$)/);
  });
});
