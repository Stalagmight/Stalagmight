import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';
import { LeftWorkspace } from '../LeftWorkspace';
import { SpeleoNetworkBusyIndicator } from '../SpeleoNetworkBusyIndicator';

const BUSY_LABEL = 'Talking to SpeleoDB...';
import { SpeleoBadgedIcon } from '../../common/SpeleoBadgedIcon';
import { LockIcon } from '../../common/icons';
import { useSpeleoNetworkStore } from '../../../store/speleoNetworkStore';
import { useUIStore } from '../../../store/uiStore';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));
vi.mock('../SpeleoWorkspace', () => ({ SpeleoWorkspace: () => <div data-testid="workspace-test-body" /> }));

beforeEach(() => {
  useSpeleoNetworkStore.getState().reset();
  useUIStore.setState({ appMode: 'survey', leftWorkspace: 'sections', surveyLeftWorkspace: 'sections' });
});

function startRead(): () => void {
  let end!: () => void;
  act(() => {
    end = useSpeleoNetworkStore.getState().begin('read');
  });
  return () => act(() => end());
}

/** The tab icon as the SpeleoDB tab draws it while read-only: the mark with its bottom-right lock badge. */
const badgedIcon = (
  <SpeleoBadgedIcon size={16} testIdPrefix="test-tab-icon" badge={<LockIcon size={10} />} badgeColor="text-text-primary" />
);

describe('SpeleoNetworkBusyIndicator', () => {
  it('shows nothing while idle but keeps its live region mounted', () => {
    render(<SpeleoNetworkBusyIndicator />);
    expect(screen.queryByTestId('speleodb-network-busy')).toBeNull();
    const status = screen.getByRole('status');
    expect(status).toHaveAttribute('aria-live', 'polite');
    expect(status).toBeEmptyDOMElement();
  });

  it('shows a labelled spinner while a call is in flight and hides it after', () => {
    render(<SpeleoNetworkBusyIndicator />);
    const end = startRead();
    const spinner = screen.getByTestId('speleodb-network-busy');
    // The translated label (en), as the tooltip and in the live region.
    expect(spinner).toHaveAttribute('title', BUSY_LABEL);
    expect(screen.getByRole('status')).toHaveTextContent(BUSY_LABEL);
    end();
    expect(screen.queryByTestId('speleodb-network-busy')).toBeNull();
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
  });

  it('without an icon: a zero-size anchor with the spinner positioned off it', () => {
    render(<SpeleoNetworkBusyIndicator />);
    startRead();
    const slot = screen.getByTestId('speleodb-network-busy-slot');
    expect(slot.className).toMatch(/\bw-0\b/);
    expect(slot.className).toMatch(/\bh-0\b/);
    expect(slot.className).toMatch(/\brelative\b/);
    const spinner = screen.getByTestId('speleodb-network-busy');
    expect(spinner).toHaveAttribute('data-placement', 'anchor');
    expect(spinner.className).toMatch(/\babsolute\b/);
  });

  it('over an icon: overlays its top-left corner, clear of the bottom-right state badge', () => {
    render(<SpeleoNetworkBusyIndicator>{badgedIcon}</SpeleoNetworkBusyIndicator>);
    startRead();
    const slot = screen.getByTestId('speleodb-network-busy-slot');
    // The icon is the frame's only in-flow child: the frame is exactly its size.
    expect(slot.className).toMatch(/\binline-flex\b/);
    expect(slot.className).not.toMatch(/\bw-0\b/);
    expect(within(slot).getByTestId('test-tab-icon-logo')).toBeInTheDocument();
    const spinner = screen.getByTestId('speleodb-network-busy');
    expect(spinner).toHaveAttribute('data-placement', 'icon');
    expect(spinner.className).toMatch(/\babsolute\b/);
    expect(spinner.className).toMatch(/-left-1\b/);
    expect(spinner.className).toMatch(/-top-1\b/);
    // The badge owns the opposite corner, and the spinner never claims it.
    const badge = screen.getByTestId('test-tab-icon-state-badge');
    expect(badge.className).toMatch(/-bottom-1\b/);
    expect(badge.className).toMatch(/-right-1\b/);
    expect(spinner.className).not.toMatch(/bottom|right/);
  });

  it('draws the spinner filled, never stroked', () => {
    render(<SpeleoNetworkBusyIndicator />);
    startRead();
    const svg = screen.getByTestId('speleodb-network-busy').querySelector('svg')!;
    expect(svg).toHaveAttribute('aria-hidden', 'true');
    for (const shape of svg.querySelectorAll('path')) {
      expect(shape.getAttribute('fill')).toBe('currentColor');
      expect(shape.getAttribute('stroke')).toBeNull();
    }
  });
});

describe('LeftWorkspace busy indicator placement', () => {
  it('sits inside the SpeleoDB tab only', () => {
    render(<LeftWorkspace sections={<div />} />);
    startRead();
    const speleo = screen.getByTestId('workspace-tab-speleodb');
    const sections = screen.getByTestId('workspace-tab-sections');
    expect(within(speleo).getByTestId('speleodb-network-busy')).toBeInTheDocument();
    expect(within(sections).queryByTestId('speleodb-network-busy')).toBeNull();
    expect(screen.getAllByTestId('speleodb-network-busy')).toHaveLength(1);
  });

  it('is present in Plan mode, where SpeleoDB is the only tab', () => {
    useUIStore.setState({ appMode: 'plan', leftWorkspace: 'speleodb' });
    render(<LeftWorkspace sections={<div />} />);
    act(() => {
      useSpeleoNetworkStore.getState().begin('exclusive');
    });
    expect(within(screen.getByTestId('workspace-tab-speleodb')).getByTestId('speleodb-network-busy')).toBeInTheDocument();
  });
});
