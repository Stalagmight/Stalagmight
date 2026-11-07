/**
 * Component test (#59) - Insert Station dialog validates the split distance
 * and only closes/selects on a genuine backend success, mirroring
 * ConfirmDialog's result-check + failure-surfacing convention.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('../../utils/telemetry', async () => (await import('../../test/factories/telemetry')).telemetryWith());

import InsertStationDialog from './InsertStationDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { MIN_SPLIT_CHILD_LENGTH_M } from '../../utils/shotSplit';
import type { Project, SectionId, ShotId } from '../../types/project';
import type { CommandResult } from '../../types/commands';

const SECTION_ID = 1 as SectionId;
const SHOT_ID = 2 as ShotId;
const SHOT_LENGTH = 10;

const baseProject = {
  sections: [{ id: SECTION_ID, shots: [{ id: SHOT_ID, length: SHOT_LENGTH }] }],
} as unknown as Project;

let splitShotAtDistance: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let setSelection: ReturnType<typeof vi.fn>;

beforeEach(() => {
  // The open project is a confirmed-local one: since #270 the SpeleoDB gate
  // fails closed, so an unresolved project would block the split too.
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  splitShotAtDistance = vi.fn();
  closeDialog = vi.fn();
  setSelection = vi.fn();
  useProjectStore.setState({
    // The dialog reads the freshest project.sections via getState() after the
    // split resolves, to find the split shot's post-split index (#59 fix D).
    project: baseProject,
    splitShotAtDistance,
  });
  useUIStore.setState({
    // Editable baseline (the read-only-guard test flips this to closureAdjusted;
    // reset here so it doesn't bleed into the other tests).
    geometryMode: 'measured',
    appMode: 'survey',
    activeDialog: 'insert-station',
    closeDialog,
    setSelection,
    dialogData: {
      sectionId: SECTION_ID,
      shotId: SHOT_ID,
      shotLength: SHOT_LENGTH,
      sectionName: 'Main Passage',
    },
  });
});

describe('InsertStationDialog (#59)', () => {
  it('refuses to split when editing is blocked, with no IPC dispatched', async () => {
    // A split has NO server backstop for the local closure-adjusted / Plan /
    // Export case, so the confirm-time isEditingBlockedNow() check is the ONLY
    // thing stopping a geometry mutation there (see useEditingBlocked). Put the
    // map in a read-only view, enter a valid distance, and confirm.
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
    fireEvent.click(screen.getByTestId('insert-station-confirm'));
    await waitFor(() => {
      expect(screen.getByText(/editing is disabled/i)).toBeInTheDocument();
    });
    expect(splitShotAtDistance).not.toHaveBeenCalled();
  });

  describe('paired metres / percent inputs', () => {
    it('typing metres updates the percent field', () => {
      render(<InsertStationDialog />);
      fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
      // 4 m of a 10 m shot.
      expect(screen.getByTestId('insert-station-percent')).toHaveValue(40);
    });

    it('typing percent updates the metres field', () => {
      render(<InsertStationDialog />);
      fireEvent.change(screen.getByTestId('insert-station-percent'), { target: { value: '25' } });
      expect(screen.getByTestId('insert-station-distance')).toHaveValue(2.5);
    });

    it('splits at the metres the percent resolved to', async () => {
      splitShotAtDistance.mockResolvedValue({ success: true } as CommandResult);
      render(<InsertStationDialog />);
      fireEvent.change(screen.getByTestId('insert-station-percent'), { target: { value: '30' } });
      fireEvent.click(screen.getByTestId('insert-station-confirm'));
      await waitFor(() => {
        // What the metres field shows is exactly what gets split - no hidden
        // extra precision from the percentage.
        expect(splitShotAtDistance).toHaveBeenCalledWith(SECTION_ID, SHOT_ID, 3);
      });
    });

    it('clearing one field clears the other', () => {
      render(<InsertStationDialog />);
      fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
      fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '' } });
      expect(screen.getByTestId('insert-station-percent')).toHaveValue(null);
      expect(screen.getByTestId('insert-station-confirm')).toBeDisabled();
    });

    it('does not fight the user by rewriting the field they are typing in', () => {
      render(<InsertStationDialog />);
      // 1/3 of 10 m is 3.3333...; the metres field must show the user's own
      // input untouched, with only the partner field rounded.
      fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '3.333' } });
      expect(screen.getByTestId('insert-station-distance')).toHaveValue(3.333);
      expect(screen.getByTestId('insert-station-percent')).toHaveValue(33.33);
    });
  });

  describe('prefill from a map right-click', () => {
    it('prefills both fields at the clicked spot', () => {
      // alongT is the fraction along the shot the pointer landed on.
      useUIStore.setState({
        dialogData: {
          sectionId: SECTION_ID,
          shotId: SHOT_ID,
          shotLength: SHOT_LENGTH,
          sectionName: 'Main Passage',
          alongT: 0.35,
        },
      });
      render(<InsertStationDialog />);
      expect(screen.getByTestId('insert-station-distance')).toHaveValue(3.5);
      expect(screen.getByTestId('insert-station-percent')).toHaveValue(35);
      // Prefilled and immediately usable.
      expect(screen.getByTestId('insert-station-confirm')).toBeEnabled();
    });

    it('splits exactly at the clicked spot without further input', async () => {
      splitShotAtDistance.mockResolvedValue({ success: true } as CommandResult);
      useUIStore.setState({
        dialogData: {
          sectionId: SECTION_ID,
          shotId: SHOT_ID,
          shotLength: SHOT_LENGTH,
          sectionName: 'Main Passage',
          alongT: 0.6,
        },
      });
      render(<InsertStationDialog />);
      fireEvent.click(screen.getByTestId('insert-station-confirm'));
      await waitFor(() => {
        expect(splitShotAtDistance).toHaveBeenCalledWith(SECTION_ID, SHOT_ID, 6);
      });
    });

    it('opens blank when there is no click position (data-table menu)', () => {
      render(<InsertStationDialog />);
      expect((screen.getByTestId('insert-station-distance') as HTMLInputElement).value).toBe('');
      expect((screen.getByTestId('insert-station-percent') as HTMLInputElement).value).toBe('');
    });
  });

  it('disables confirm when the distance field is empty', () => {
    render(<InsertStationDialog />);
    expect(screen.getByTestId('insert-station-confirm')).toBeDisabled();
  });

  it('disables confirm at distance = 0', () => {
    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '0' } });
    expect(screen.getByTestId('insert-station-confirm')).toBeDisabled();
  });

  it('disables confirm at distance = the shot length', () => {
    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), {
      target: { value: String(SHOT_LENGTH) },
    });
    expect(screen.getByTestId('insert-station-confirm')).toBeDisabled();
  });

  it('disables confirm just under the min-child-length floor', () => {
    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), {
      target: { value: String(MIN_SPLIT_CHILD_LENGTH_M / 2) },
    });
    expect(screen.getByTestId('insert-station-confirm')).toBeDisabled();
  });

  it('disables confirm when the remainder would fall under the min-child-length floor', () => {
    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), {
      target: { value: String(SHOT_LENGTH - MIN_SPLIT_CHILD_LENGTH_M / 2) },
    });
    expect(screen.getByTestId('insert-station-confirm')).toBeDisabled();
  });

  it('enables confirm exactly at the min-child-length floor', () => {
    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), {
      target: { value: String(MIN_SPLIT_CHILD_LENGTH_M) },
    });
    expect(screen.getByTestId('insert-station-confirm')).not.toBeDisabled();
  });

  it('enables confirm strictly between 0 and the shot length', () => {
    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
    expect(screen.getByTestId('insert-station-confirm')).not.toBeDisabled();
  });

  it('splits, selects the new station from the REFRESHED state, and closes on success', async () => {
    splitShotAtDistance.mockImplementation(async () => {
      // Mirrors the real backend's effect: a fresh head shot is inserted
      // before the split target, so the tail (SHOT_ID) now sits at index 1.
      useProjectStore.setState({
        project: {
          sections: [
            {
              id: SECTION_ID,
              shots: [{ id: 999 as ShotId }, { id: SHOT_ID }],
            },
          ],
        } as unknown as Project,
      });
      return { success: true, error: null } as CommandResult;
    });

    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
    fireEvent.click(screen.getByTestId('insert-station-confirm'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(splitShotAtDistance).toHaveBeenCalledWith(SECTION_ID, SHOT_ID, 4);
    expect(setSelection).toHaveBeenCalledWith({
      type: 'station',
      stationRef: { section_id: SECTION_ID, station_index: 1 },
    });
  });

  it('closes without selecting when the shot is gone from the refreshed state', async () => {
    splitShotAtDistance.mockImplementation(async () => {
      // Simulates a racing mutation / project switch: by the time the split
      // resolves, the target section no longer exists in the store.
      useProjectStore.setState({
        project: { sections: [] } as unknown as Project,
      });
      return { success: true, error: null } as CommandResult;
    });

    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
    fireEvent.click(screen.getByTestId('insert-station-confirm'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(setSelection).not.toHaveBeenCalled();
  });

  it('calls splitShotAtDistance exactly once on a rapid double-submit', async () => {
    let resolveSplit: (value: CommandResult) => void = () => {};
    splitShotAtDistance.mockImplementation(
      () =>
        new Promise<CommandResult>((resolve) => {
          resolveSplit = resolve;
        })
    );

    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
    const confirmButton = screen.getByTestId('insert-station-confirm');
    fireEvent.click(confirmButton);
    fireEvent.click(confirmButton);

    await waitFor(() => expect(confirmButton).toBeDisabled());
    expect(splitShotAtDistance).toHaveBeenCalledTimes(1);

    resolveSplit({ success: true, error: null } as CommandResult);
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(splitShotAtDistance).toHaveBeenCalledTimes(1);
  });

  it('surfaces the backend error and does not close/select on refusal', async () => {
    splitShotAtDistance.mockResolvedValue({
      success: false,
      error: 'Cannot edit geometry of a frozen shot.',
    } as CommandResult);

    render(<InsertStationDialog />);
    fireEvent.change(screen.getByTestId('insert-station-distance'), { target: { value: '4' } });
    fireEvent.click(screen.getByTestId('insert-station-confirm'));

    expect(await screen.findByText('Cannot edit geometry of a frozen shot.')).toBeInTheDocument();
    expect(closeDialog).not.toHaveBeenCalled();
    expect(setSelection).not.toHaveBeenCalled();
  });
});
