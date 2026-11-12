/**
 * #707: the "Combine Sections" dialog.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { CombineCandidate } from '../../../types/ipc/CombineCandidate';

const closeDialog = vi.fn();
const setSelection = vi.fn();
const combineSections = vi.fn();

let dialogData: Record<string, unknown> = {};
const uiState = () => ({ closeDialog, setSelection, dialogData });
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign(() => uiState(), { getState: () => uiState() }),
}));

let sections: Array<{ id: number; name: string | null; color: string }> = [];
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({ combineSections, config: null, project: { sections } }),
}));

const combineCandidate = vi.fn();
const combineCandidateAtStation = vi.fn();
vi.mock('../../../utils/combineSections', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/combineSections')>()),
  combineCandidate: (a: number, b: number) => combineCandidate(a, b),
  combineCandidateAtStation: (station: unknown) => combineCandidateAtStation(station),
}));

let blocked = false;
vi.mock('../../../hooks/useEditingBlocked', () => ({
  isEditingBlockedNow: () => blocked,
}));

vi.mock('../../../utils/telemetry', () => ({ trackDialogComplete: vi.fn() }));

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import CombineSectionsDialog from '../CombineSectionsDialog';

const candidate = (extra: Partial<CombineCandidate> = {}): CombineCandidate => ({
  kept: 1,
  other: 2,
  pairing: 'kept_end_other_start',
  reverse_other: false,
  blocked: null,
  comment_conflict: false,
  name_conflict: false,
  offset_conflict: false,
  credit: [
    { surveyor: 'Surveyor A', date: '2026-03-02', length_m: 212 },
    { surveyor: null, date: null, length_m: 96 },
  ],
  ...extra,
});

const confirm = () => screen.getByTestId('combine-confirm');

describe('CombineSectionsDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    sections = [
      { id: 1, name: 'Main', color: '#111111' },
      { id: 2, name: 'Side lead', color: '#222222' },
    ];
    dialogData = { a: 1, b: 2 };
    combineCandidate.mockResolvedValue(candidate());
    combineCandidateAtStation.mockResolvedValue(candidate());
    combineSections.mockResolvedValue({ success: true });
  });

  it('shows a loading state, then the plan and the credit', async () => {
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-loading')).toBeInTheDocument();
    expect(confirm()).toBeDisabled();
    expect(await screen.findByTestId('combine-plan')).toHaveTextContent('Side lead will be added after Main.');
    expect(combineCandidate).toHaveBeenCalledWith(1, 2);
    expect(screen.getByTestId('combine-credit-0')).toHaveTextContent('Surveyor A · 2026-03-02 · 212');
    expect(screen.getByTestId('combine-credit-1')).toHaveTextContent('Unknown · Unknown · 96');
    expect(screen.getByTestId('combine-name')).toHaveValue('Main');
    expect(screen.getByTestId('combine-color')).toHaveValue('#111111');
  });

  it('asks the station query when opened from a station', async () => {
    dialogData = { station: { section_id: 2, station_index: 0 } };
    render(<CombineSectionsDialog />);
    await screen.findByTestId('combine-plan');
    expect(combineCandidateAtStation).toHaveBeenCalledWith({ section_id: 2, station_index: 0 });
    expect(combineCandidate).not.toHaveBeenCalled();
  });

  it('disables confirm when the sections do not meet', async () => {
    combineCandidate.mockResolvedValue(null);
    render(<CombineSectionsDialog />);
    expect(await screen.findByTestId('combine-not-joined')).toHaveTextContent("don't meet end to end");
    expect(confirm()).toBeDisabled();
  });

  it('shows the reason and disables confirm when blocked', async () => {
    combineCandidate.mockResolvedValue(candidate({ blocked: 'The sections use different azimuth references' }));
    render(<CombineSectionsDialog />);
    expect(await screen.findByTestId('combine-blocked')).toHaveTextContent('different azimuth references');
    expect(confirm()).toBeDisabled();
  });

  it('defaults the name to X for a split pair X-A and X-B', async () => {
    sections = [
      { id: 1, name: 'Main-A', color: '#111111' },
      { id: 2, name: 'Main-B', color: '#222222' },
    ];
    render(<CombineSectionsDialog />);
    await waitFor(() => expect(screen.getByTestId('combine-name')).toHaveValue('Main'));
  });

  it('describes a reversed prepend', async () => {
    combineCandidate.mockResolvedValue(candidate({ pairing: 'kept_start_other_start', reverse_other: true }));
    render(<CombineSectionsDialog />);
    expect(await screen.findByTestId('combine-plan')).toHaveTextContent(
      'Side lead will be reversed and added before Main.'
    );
  });

  it('offers the joined-station choices only when they conflict', async () => {
    const { unmount } = render(<CombineSectionsDialog />);
    await screen.findByTestId('combine-plan');
    expect(screen.queryByTestId('combine-comment-side')).toBeNull();
    expect(screen.queryByTestId('combine-name-side')).toBeNull();
    expect(screen.queryByTestId('combine-offset-side')).toBeNull();
    unmount();

    combineCandidate.mockResolvedValue(candidate({ comment_conflict: true, offset_conflict: true }));
    render(<CombineSectionsDialog />);
    await screen.findByTestId('combine-comment-side');
    expect(screen.getByTestId('combine-comment-side-both')).toBeInTheDocument();
    expect(screen.queryByTestId('combine-name-side')).toBeNull();
    expect(screen.queryByTestId('combine-offset-side-both')).toBeNull();
    fireEvent.click(screen.getByTestId('combine-comment-side-both'));
    fireEvent.click(screen.getByTestId('combine-offset-side-b'));
    fireEvent.click(confirm());
    await waitFor(() => expect(combineSections).toHaveBeenCalled());
    expect(combineSections.mock.calls[0]?.[2]).toMatchObject({
      comment_side: 'both',
      name_side: 'a',
      offset_side: 'b',
    });
  });

  it('sends the payload, selects the kept section and closes', async () => {
    render(<CombineSectionsDialog />);
    await screen.findByTestId('combine-plan');
    fireEvent.change(screen.getByTestId('combine-name'), { target: { value: ' Joined ' } });
    fireEvent.change(screen.getByTestId('combine-color'), { target: { value: '#abcdef' } });
    fireEvent.click(confirm());
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(combineSections).toHaveBeenCalledWith(1, 2, {
      name: 'Joined',
      color: '#abcdef',
      comment_side: 'a',
      name_side: 'a',
      offset_side: 'a',
    });
    expect(setSelection).toHaveBeenCalledWith({ type: 'section', sectionId: 1 });
  });

  it('swaps the kept and the other section', async () => {
    render(<CombineSectionsDialog />);
    await screen.findByTestId('combine-plan');
    fireEvent.click(screen.getByTestId('combine-swap'));
    expect(screen.getByTestId('combine-plan')).toHaveTextContent('Main will be added before Side lead.');
    expect(screen.getByTestId('combine-name')).toHaveValue('Side lead');
    expect(screen.getByTestId('combine-color')).toHaveValue('#222222');
    fireEvent.click(confirm());
    await waitFor(() => expect(combineSections).toHaveBeenCalled());
    expect(combineSections.mock.calls[0]?.slice(0, 2)).toEqual([2, 1]);
    expect(combineSections.mock.calls[0]?.[2]).toMatchObject({ name: 'Side lead', color: '#222222' });
    expect(setSelection).toHaveBeenCalledWith({ type: 'section', sectionId: 2 });
  });

  it('refuses to combine in a read-only view', async () => {
    blocked = true;
    render(<CombineSectionsDialog />);
    await screen.findByTestId('combine-plan');
    fireEvent.click(confirm());
    expect(combineSections).not.toHaveBeenCalled();
    expect(screen.getByTestId('combine-error')).toHaveTextContent('Editing is disabled');
  });
});
