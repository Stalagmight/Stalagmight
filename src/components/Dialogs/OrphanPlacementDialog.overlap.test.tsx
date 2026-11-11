/**
 * Component test (#681) - OrphanPlacementDialog resolves the WHOLE repeat, one
 * choice per repeated shot.
 *
 * Synthetic layout: the existing "Mainline" (section 2) has 5 shots; the new
 * "Orphan" (section 1) starts with two shots repeating Mainline shots 4 and 5
 * (indices 3, 4), then carries on. The repeat ends at Orphan stn 2 = Mainline
 * stn 5.
 *
 * INJECTION (tried), each seen red with the fix removed:
 * - default merge back to `preferredMergeOffset` (mid-overlap, offset 1): reds
 *   "defaults the connection to the end of the repeat";
 * - `computeOverlapPairs` pairing the whole overlap whatever the merge station
 *   (the first cut of #681): reds all four "a mid-repeat merge" cases - shots
 *   past the merge station became Reference, cutting the section off;
 * - `placeSuggestion(best, ..., new Map())` for the on-screen section in Accept
 *   All (the old code): reds "Accept All applies the choices made on screen";
 * - dropping the filter in `handleMergeChange`: reds "a row that leaves the
 *   table forgets its choice";
 * - dropping the `sameReading` no-op check: reds "an identical repeat writes
 *   nothing" and "the bulk control sets every row at once";
 * - passing `replaceDisabled={false}` to the table: reds the doubly-anchored case
 *   (Use new / Average become clickable);
 * - dropping the `isTargetDoublyAnchored` override in `effectiveChoices`: reds
 *   "a target that becomes anchored at both ends after a choice";
 * - passing `effectiveChoices` to the other sections in Accept All: reds "Accept
 *   All gives the other sections the defaults";
 * - marking each repeated shot Reference before checking the existing shot is
 *   there: reds "a repeated shot the existing section no longer has";
 * - ignoring a refused merge, in `placeSuggestion` or in Accept / Accept All:
 *   reds both "refused merge" cases;
 * - `Math.round(score * 100)` instead of `scorePercent`: reds "never shows a
 *   score above 100%";
 * - dropping the unresolved note, or always showing the plain matched count:
 *   each reds "a merge at the start of the repeat";
 * - dropping the `editingAllowed()` check from Accept, or from Accept All: each
 *   reds "refuses to place while editing is blocked".
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('./SectionPreview', () => ({ default: () => null }));
vi.mock('../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/telemetry')>()),
  trackDialogComplete: vi.fn(),
}));
// jsdom does not implement scrollIntoView; the Select component calls it on open.
Element.prototype.scrollIntoView = vi.fn();
// jsdom has no ResizeObserver; usePreviewSize sizes the preview with one.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

import OrphanPlacementDialog from './OrphanPlacementDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import type { CommandResult, OrphanPlacementSuggestion } from '../../types/commands';
import type { Project } from '../../types/ipc/Project';
import type { Shot } from '../../types/ipc/Shot';

const ok = async (): Promise<CommandResult> => ({ success: true } as CommandResult);
const refused = { success: false, error: 'Merge refused' } as CommandResult;

function shot(id: number, azimuth: number, length: number, depthIn: number, depthOut: number): Shot {
  return {
    id,
    azimuth_in: azimuth,
    azimuth_out: azimuth,
    length,
    depth_in: depthIn,
    depth_out: depthOut,
    pitch_in: null,
    pitch_out: null,
    left: 1,
    right: 1,
    up: 1,
    down: 1,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
  };
}

// Mainline shot ids 200..204; Orphan shot ids 100..103.
const MAINLINE = [
  shot(200, 10, 5, 0, 1),
  shot(201, 20, 5, 1, 2),
  shot(202, 30, 5, 2, 3),
  shot(203, 40, 6, 3, 4),
  shot(204, 50, 7, 4, 5),
];
const ORPHAN = [
  shot(100, 40, 6, 3, 4), // identical repeat of Mainline 203
  shot(101, 52, 7.5, 4, 5), // repeat of Mainline 204, re-measured: 2 deg and 0.5 m off
  shot(102, 90, 4, 5, 5),
  shot(103, 95, 4, 5, 6),
];

// A second orphan, "Second" (section 3): its first two shots repeat Mainline
// shots 1 and 2 (indices 0, 1), the second one re-measured 0.5 m longer.
const SECOND = [
  shot(300, 10, 5, 0, 1),
  shot(301, 20, 5.5, 1, 2),
  shot(302, 70, 3, 2, 2),
];

const section = (id: number, name: string, shots: Shot[]) =>
  ({ id, name, shots, direction: 'in', color: '#4a9eff' }) as unknown as Project['sections'][number];

function makeProject(over: Partial<Project> = {}): Project {
  return {
    sections: [section(1, 'Orphan', ORPHAN), section(2, 'Mainline', MAINLINE), section(3, 'Second', SECOND)],
    station_merges: [],
    zero_datums: [],
    loop_closures: [],
    ...over,
  } as unknown as Project;
}

function makeSuggestion(over: Partial<OrphanPlacementSuggestion> = {}): OrphanPlacementSuggestion {
  return {
    orphanSectionId: 1,
    orphanSectionName: 'Orphan',
    orphanStationIndex: 0,
    targetSectionId: 2,
    targetSectionName: 'Mainline',
    targetStationIndex: 3,
    score: 0.9,
    matchedShots: 2,
    existingMatchedShots: 2,
    // What the matcher really prefers for a 2-shot repeat: the mid station.
    preferredMergeOffset: 1,
    direction: 'forward',
    isPartial: false,
    confidence: 'high',
    ...over,
  };
}

async function pickOption(testId: string, value: string) {
  await userEvent.click(screen.getByTestId(testId));
  await userEvent.click(screen.getByTestId(`${testId}-option-${encodeURIComponent(value)}`));
}

let mergeStations: ReturnType<typeof vi.fn>;
let updateShot: ReturnType<typeof vi.fn>;
let setShotKind: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

function setup(project: Project = makeProject()) {
  useProjectStore.setState({ project });
}

async function acceptAndFinish() {
  await userEvent.click(screen.getByTestId('orphan-accept'));
  await waitFor(() => expect(closeDialog).toHaveBeenCalled());
}

const referenceCalls = () =>
  setShotKind.mock.calls.filter((c) => c[2] === 'Reference').map((c) => [c[0], c[1]]);

beforeEach(() => {
  // A confirmed-local project in Survey mode: editing is allowed (#270 fails
  // closed on an unknown link kind).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
  mergeStations = vi.fn(ok);
  updateShot = vi.fn(ok);
  setShotKind = vi.fn(ok);
  closeDialog = vi.fn();
  useProjectStore.setState({
    project: null,
    config: null,
    mergeStations,
    updateShot,
    setShotKind,
    refreshProjectState: vi.fn(async () => {}),
    saveAfterImport: vi.fn(async () => true),
  });
  useUIStore.setState({ closeDialog, setView: vi.fn(), triggerFitToBounds: vi.fn() });
});

describe('OrphanPlacementDialog - repeated shots table (#681)', () => {
  it('renders one row per repeated shot with both readings and the difference', () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    const row0 = screen.getByTestId('overlap-row-0');
    const row1 = screen.getByTestId('overlap-row-1');
    expect(screen.queryByTestId('overlap-row-2')).toBeNull();
    expect(screen.queryByTestId('overlap-mode-select')).toBeNull();

    expect(within(row0).getByText('Existing 2:4')).toBeTruthy();
    expect(within(row0).getByText('New 1:1')).toBeTruthy();
    expect(row0.getAttribute('data-differs')).toBe('false');

    expect(within(row1).getByText('Existing 2:5')).toBeTruthy();
    expect(within(row1).getByText('7.5')).toBeTruthy(); // new length
    expect(within(row1).getByText('0.5')).toBeTruthy(); // length difference
    expect(within(row1).getByText('2.0')).toBeTruthy(); // azimuth difference
    expect(row1.getAttribute('data-differs')).toBe('true');
  });

  it('defaults the connection to the end of the repeat', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await acceptAndFinish();

    expect(mergeStations).toHaveBeenCalledWith(
      { section_id: 1, station_index: 2 },
      { section_id: 2, station_index: 5 },
    );
  });

  it('defaults every row to Keep existing: the new copies become Reference, nothing is rewritten', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await acceptAndFinish();

    expect(referenceCalls()).toEqual([[1, 100], [1, 101]]);
    expect(updateShot).not.toHaveBeenCalled();
  });

  // A mid-repeat merge station shortens the repeat: only the shots hanging off
  // it are listed and become Reference. The shots from the merge station on to
  // the orphan's far end are new survey and stay Normal - a Reference leg costs
  // infinity in Plan mode, so marking them would cut the rest of the orphan off.
  const MID_REPEAT: Array<[string, Shot[], Partial<OrphanPlacementSuggestion>, number, number[]]> = [
    // [label, orphan shots, suggestion, the one resolved shot index, the shot ids past the merge]
    ['Front + forward', ORPHAN, {}, 0, [101, 102, 103]],
    [
      'Front + reverse',
      [shot(100, 230, 7, 5, 4), shot(101, 220, 6, 4, 3), shot(102, 300, 4, 3, 3), shot(103, 310, 4, 3, 2)],
      { targetStationIndex: 5, direction: 'reverse' },
      0,
      [101, 102, 103],
    ],
    [
      'Back + forward',
      [shot(100, 90, 4, 0, 0), shot(101, 95, 4, 0, 1), shot(102, 230, 7, 5, 4), shot(103, 220, 6, 4, 3)],
      { orphanStationIndex: 4, targetStationIndex: 3 },
      3,
      [100, 101, 102],
    ],
    [
      'Back + reverse',
      [shot(100, 90, 4, 0, 0), shot(101, 95, 4, 0, 3), shot(102, 40, 6, 3, 4), shot(103, 50, 7, 4, 5)],
      { orphanStationIndex: 4, targetStationIndex: 5, direction: 'reverse' },
      3,
      [100, 101, 102],
    ],
  ];

  for (const [label, orphanShots, over, resolvedIndex, pastMerge] of MID_REPEAT) {
    it(`a mid-repeat merge resolves only the shot before it, the rest stay Normal (${label})`, async () => {
      setup(makeProject({
        sections: [
          { id: 1, name: 'Orphan', shots: orphanShots, direction: 'in', color: '#4a9eff' },
          { id: 2, name: 'Mainline', shots: MAINLINE, direction: 'in', color: '#4a9eff' },
        ] as unknown as Project['sections'],
      }));
      render(<OrphanPlacementDialog suggestions={[makeSuggestion(over)]} />);

      // Both repeated shots are listed by default (merged at the end of the repeat).
      expect(screen.getAllByTestId(/^overlap-row-\d+$/)).toHaveLength(2);
      await pickOption('merge-station-select', '1');
      const rows = screen.getAllByTestId(/^overlap-row-\d+$/).map((row) => row.getAttribute('data-testid'));
      expect(rows).toEqual([`overlap-row-${resolvedIndex}`]);

      await userEvent.click(screen.getByTestId('overlap-set-all-use-new'));
      await acceptAndFinish();

      expect(referenceCalls()).toEqual([[1, orphanShots[resolvedIndex]!.id]]);
      for (const id of pastMerge) {
        expect(setShotKind).not.toHaveBeenCalledWith(1, id, 'Reference');
      }
    });
  }

  it('a merge at the start of the repeat says the repeat stays normal survey', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    expect(screen.getByTestId('orphan-matched-shots').textContent).toBe('2 shots matched \u2192');
    expect(screen.queryByTestId('overlap-unresolved-note')).toBeNull();

    await pickOption('merge-station-select', '1');
    expect(screen.getByTestId('orphan-matched-shots').textContent)
      .toBe('2 shots matched \u2192, 1 resolved at this merge station');
    expect(screen.queryByTestId('overlap-unresolved-note')).toBeNull();

    await pickOption('merge-station-select', '0');
    expect(screen.queryByTestId('overlap-shot-section')).toBeNull();
    expect(screen.getByTestId('orphan-matched-shots').textContent)
      .toBe('2 shots matched \u2192, 0 resolved at this merge station');
    expect(screen.getByTestId('overlap-unresolved-note').textContent).toContain(
      'keeps the 2 repeated shots as normal survey',
    );

    await acceptAndFinish();
    expect(mergeStations).toHaveBeenCalledWith(
      { section_id: 1, station_index: 0 },
      { section_id: 2, station_index: 3 },
    );
    expect(referenceCalls()).toEqual([]);
  });

  it('a row that leaves the table forgets its choice; a row that stays keeps it', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await pickOption('overlap-choice-0', 'average');
    await pickOption('overlap-choice-1', 'use_new');
    await pickOption('merge-station-select', '1');
    expect(screen.queryByTestId('overlap-row-1')).toBeNull();
    await pickOption('merge-station-select', '2');

    expect(screen.getByTestId('overlap-choice-0').textContent).toContain('Average');
    expect(screen.getByTestId('overlap-choice-1').textContent).toContain('Keep existing');
    await acceptAndFinish();
    expect(updateShot).not.toHaveBeenCalled();
  });

  it('Accept All applies the choices made on screen', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await pickOption('overlap-choice-1', 'use_new');
    await userEvent.click(screen.getByTestId('orphan-accept-all'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());

    expect(mergeStations).toHaveBeenCalledWith(
      { section_id: 1, station_index: 2 },
      { section_id: 2, station_index: 5 },
    );
    expect(updateShot).toHaveBeenCalledTimes(1);
    expect(updateShot).toHaveBeenCalledWith(2, 204, expect.objectContaining({ length: 7.5, azimuth_in: 52 }));
  });

  it('an identical repeat writes nothing, whatever the choice', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await pickOption('overlap-choice-0', 'use_new');
    await acceptAndFinish();

    expect(referenceCalls()).toEqual([[1, 100], [1, 101]]);
    expect(updateShot).not.toHaveBeenCalled();
  });

  it('Use new on one row copies the new reading into the EXISTING shot only for that row', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await pickOption('overlap-choice-1', 'use_new');
    await acceptAndFinish();

    expect(referenceCalls()).toEqual([[1, 100], [1, 101]]);
    expect(updateShot).toHaveBeenCalledTimes(1);
    expect(updateShot).toHaveBeenCalledWith(2, 204, expect.objectContaining({
      length: 7.5,
      azimuth_in: 52,
      azimuth_out: 52,
    }));
  });

  it('the bulk control sets every row at once', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await userEvent.click(screen.getByTestId('overlap-set-all-average'));
    expect(screen.getByTestId('overlap-set-all-average').getAttribute('aria-pressed')).toBe('true');
    await acceptAndFinish();

    // Row 0 repeats Mainline 203 exactly, so averaging it leaves nothing to write.
    expect(updateShot).toHaveBeenCalledTimes(1);
    expect(updateShot).toHaveBeenCalledWith(2, 204, expect.objectContaining({ length: 7.25 }));
    const az = (updateShot.mock.calls.find((c) => c[1] === 204)![2] as { azimuth_in: number }).azimuth_in;
    expect(az).toBeCloseTo(51);
  });

  it('reverses the new reading when the two surveys run opposite ways (Back + forward)', async () => {
    // The orphan's END repeats Mainline: orphan shots 2, 3 (reversed) are
    // Mainline shots 4, 3. Shot 3 is an exact repeat, so it only reads as one
    // once reversed; shot 2 was re-measured (0.2 m longer, 2 deg off). Use new
    // must write it back in Mainline's direction.
    const back = [shot(100, 90, 4, 0, 0), shot(101, 95, 4, 0, 1), shot(102, 232, 7.2, 5, 4), shot(103, 220, 6, 4, 3)];
    setup(makeProject({
      sections: [
        { id: 1, name: 'Orphan', shots: back, direction: 'in', color: '#4a9eff' },
        { id: 2, name: 'Mainline', shots: MAINLINE, direction: 'in', color: '#4a9eff' },
      ] as unknown as Project['sections'],
    }));
    render(<OrphanPlacementDialog suggestions={[makeSuggestion({ orphanStationIndex: 4, targetStationIndex: 3 })]} />);

    expect(screen.getByTestId('overlap-row-2').getAttribute('data-differs')).toBe('true');
    expect(screen.getByTestId('overlap-row-3').getAttribute('data-differs')).toBe('false');
    await userEvent.click(screen.getByTestId('overlap-set-all-use-new'));
    await acceptAndFinish();

    expect(referenceCalls()).toEqual([[1, 102], [1, 103]]);
    expect(updateShot).toHaveBeenCalledTimes(1);
    expect(updateShot).toHaveBeenCalledWith(2, 204, expect.objectContaining({
      azimuth_in: 52, length: 7.2, depth_in: 4, depth_out: 5,
    }));
  });

  it('a target anchored at both ends allows Keep existing only', async () => {
    const datum = (station_index: number) => ({ station_ref: { section_id: 2, station_index } });
    setup(makeProject({ zero_datums: [datum(0), datum(5)] as unknown as Project['zero_datums'] }));
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    expect((screen.getByTestId('overlap-set-all-use-new') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('overlap-set-all-average') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('overlap-set-all-keep-existing') as HTMLButtonElement).disabled).toBe(false);

    await userEvent.click(screen.getByTestId('overlap-choice-1'));
    expect(screen.getByTestId('overlap-choice-1-option-use_new').getAttribute('aria-disabled')).toBe('true');
    await userEvent.click(screen.getByTestId('overlap-choice-1-option-use_new'));
    await acceptAndFinish();

    expect(referenceCalls()).toEqual([[1, 100], [1, 101]]);
    expect(updateShot).not.toHaveBeenCalled();
  });

  it('a repeated shot the existing section no longer has is neither listed nor marked', async () => {
    // A stale suggestion: Mainline has lost its last shot, so the second
    // repeated shot pairs with nothing. The table and the apply step agree.
    setup(makeProject({
      sections: [
        { id: 1, name: 'Orphan', shots: ORPHAN, direction: 'in', color: '#4a9eff' },
        { id: 2, name: 'Mainline', shots: MAINLINE.slice(0, 4), direction: 'in', color: '#4a9eff' },
      ] as unknown as Project['sections'],
    }));
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    const rows = screen.getAllByTestId(/^overlap-row-\d+$/).map((row) => row.getAttribute('data-testid'));
    expect(rows).toEqual(['overlap-row-0']);
    await acceptAndFinish();

    expect(referenceCalls()).toEqual([[1, 100]]);
  });

  it('a refused merge writes nothing else and leaves the section to place', async () => {
    mergeStations.mockResolvedValueOnce(refused);
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await pickOption('overlap-choice-1', 'use_new');
    await userEvent.click(screen.getByTestId('orphan-accept'));

    const alert = await screen.findByTestId('orphan-place-error');
    expect(alert.textContent).toContain('Could not connect Orphan');
    expect(alert.textContent).toContain('Merge refused');
    expect(setShotKind).not.toHaveBeenCalled();
    expect(updateShot).not.toHaveBeenCalled();
    expect(closeDialog).not.toHaveBeenCalled();

    // Still on screen, choices intact: a second Accept places it.
    await acceptAndFinish();
    expect(mergeStations).toHaveBeenCalledTimes(2);
    expect(updateShot).toHaveBeenCalledWith(2, 204, expect.objectContaining({ length: 7.5 }));
  });

  it('Accept All stops on a refused merge, with that section on screen', async () => {
    mergeStations.mockImplementation(async (a: { section_id: number }) => (a.section_id === 3 ? refused : ok()));
    setup();
    const second = makeSuggestion({ orphanSectionId: 3, orphanSectionName: 'Second', targetStationIndex: 0 });
    render(<OrphanPlacementDialog suggestions={[makeSuggestion(), second]} />);

    await userEvent.click(screen.getByTestId('orphan-accept-all'));

    const alert = await screen.findByTestId('orphan-place-error');
    expect(alert.textContent).toContain('Could not connect Second');
    expect(referenceCalls()).toEqual([[1, 100], [1, 101]]);
    expect(closeDialog).not.toHaveBeenCalled();
    expect(screen.getByTestId('wizard-suggestion').textContent).toContain('Second');
    expect(screen.getByTestId('wizard-progress').textContent).toBe('Section 2 of 2');
  });

  it('Accept All gives the other sections the defaults, not the on-screen choices', async () => {
    setup();
    const second = makeSuggestion({ orphanSectionId: 3, orphanSectionName: 'Second', targetStationIndex: 0 });
    render(<OrphanPlacementDialog suggestions={[makeSuggestion(), second]} />);

    await pickOption('overlap-choice-1', 'use_new');
    await userEvent.click(screen.getByTestId('orphan-accept-all'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());

    // Second joins at the end of its repeat, Orphan stn 2 = Mainline stn 2.
    expect(mergeStations).toHaveBeenCalledWith(
      { section_id: 3, station_index: 2 },
      { section_id: 2, station_index: 2 },
    );
    expect(referenceCalls()).toEqual([[1, 100], [1, 101], [3, 300], [3, 301]]);
    // Only the on-screen row 1 is rewritten; Second's re-measured shot 301
    // (row 1 too) keeps Mainline 201 as it was.
    expect(updateShot).toHaveBeenCalledTimes(1);
    expect(updateShot).toHaveBeenCalledWith(2, 204, expect.objectContaining({ length: 7.5 }));
  });

  it('a target that becomes anchored at both ends after a choice still keeps the existing reading', async () => {
    setup();
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);
    await pickOption('overlap-choice-1', 'use_new');

    const datum = (station_index: number) => ({ station_ref: { section_id: 2, station_index } });
    act(() => {
      useProjectStore.setState({
        project: makeProject({ zero_datums: [datum(0), datum(5)] as unknown as Project['zero_datums'] }),
      });
    });
    expect((screen.getByTestId('overlap-set-all-use-new') as HTMLButtonElement).disabled).toBe(true);
    await acceptAndFinish();

    expect(referenceCalls()).toEqual([[1, 100], [1, 101]]);
    expect(updateShot).not.toHaveBeenCalled();
  });

  it('refuses to place while editing is blocked, and writes nothing', async () => {
    setup();
    useUIStore.setState({ appMode: 'export' });
    const second = makeSuggestion({ orphanSectionId: 3, orphanSectionName: 'Second', targetStationIndex: 0 });
    render(<OrphanPlacementDialog suggestions={[makeSuggestion(), second]} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));
    expect((await screen.findByTestId('orphan-place-error')).textContent).toBe(
      'Editing is disabled in this mode, so nothing was connected.',
    );
    await userEvent.click(screen.getByTestId('orphan-accept-all'));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(mergeStations).not.toHaveBeenCalled();
    expect(setShotKind).not.toHaveBeenCalled();
    expect(updateShot).not.toHaveBeenCalled();
    expect(closeDialog).not.toHaveBeenCalled();
    expect(screen.getByTestId('wizard-progress').textContent).toBe('Section 1 of 2');
  });

  it('never shows a score above 100%', () => {
    setup();
    const boosted = makeSuggestion({ score: 1.18 });
    const alternative = makeSuggestion({ score: 1.06, targetStationIndex: 2 });
    render(<OrphanPlacementDialog suggestions={[boosted, alternative]} />);

    expect(screen.getByText('100% score')).toBeTruthy();
    expect(screen.queryByText(/11[0-9]%/)).toBeNull();
  });
});
