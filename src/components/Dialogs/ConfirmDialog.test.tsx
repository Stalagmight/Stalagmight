/**
 * Component test (#241) - a refused delete does not silently close as success.
 *
 * Every branch of `handleConfirm` used to discard the delete `CommandResult`
 * (except, later, the split branch) and unconditionally select + close, so a
 * backend refusal (a frozen shot, an edge shot in an adjusted network, a
 * read-only/SpeleoDB-locked project) closed the dialog exactly as if it had
 * succeeded, with the user seeing nothing. The fix routes every path through a
 * shared `runDelete` that checks `result.success`, surfaces the error, and keeps
 * the dialog open; a thrown transport error is caught the same way. Mirrors
 * `SplitSectionDialog`'s failure handling (result-check + try/catch + in-flight guard).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('../../utils/telemetry', async () => (await import('../../test/factories/telemetry')).telemetryWith());

import ConfirmDialog from './ConfirmDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandResult } from '../../types/commands';

const SECTION_ID = 1;
const SHOT_ID = 2;

let deleteShot: ReturnType<typeof vi.fn>;
let deleteShotAndSplit: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let setSelection: ReturnType<typeof vi.fn>;
let clearSelection: ReturnType<typeof vi.fn>;

beforeEach(() => {
  deleteShot = vi.fn();
  deleteShotAndSplit = vi.fn();
  closeDialog = vi.fn();
  setSelection = vi.fn();
  clearSelection = vi.fn();
  useProjectStore.setState({
    deleteShot,
    deleteSection: vi.fn(),
    deleteLastShotAndSection: vi.fn(),
    deleteShotAndSplit,
  });
  useUIStore.setState({
    activeDialog: 'confirm-delete', // the success path only closes if still active
    closeDialog,
    clearSelection,
    setSelection,
    dialogData: {
      type: 'shot',
      sectionId: SECTION_ID,
      shotId: SHOT_ID,
      splitRequired: true,
      sectionName: 'Main Passage',
    },
  });
});

describe('ConfirmDialog delete+split refusal (#241)', () => {
  it('surfaces the backend error and does not close/select when the split is refused', async () => {
    // A read-only/SpeleoDB-locked project makes the split backend return a failure
    // CommandResult (guard_mutation); before the fix the dialog closed as success.
    deleteShotAndSplit.mockResolvedValue({
      success: false,
      error: 'Project is read-only',
    } as CommandResult);

    render(<ConfirmDialog />);
    fireEvent.click(screen.getByText('Delete & Split'));

    expect(await screen.findByText('Project is read-only')).toBeInTheDocument();
    expect(closeDialog).not.toHaveBeenCalled();
    expect(setSelection).not.toHaveBeenCalled();
  });

  it('selects section A and closes on success', async () => {
    deleteShotAndSplit.mockResolvedValue({ success: true, error: null } as CommandResult);

    render(<ConfirmDialog />);
    fireEvent.click(screen.getByText('Delete & Split'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(setSelection).toHaveBeenCalledWith({ type: 'section', sectionId: SECTION_ID });
  });
});

describe('ConfirmDialog non-split delete refusal (#241)', () => {
  beforeEach(() => {
    // A plain edge-shot delete (no split): the context menu routes a frozen edge
    // shot straight here with no pre-check, and delete_shot's backend guard refuses
    // it. This is the reachable sibling of the split-path bug.
    useUIStore.setState({
      dialogData: {
        type: 'shot',
        sectionId: SECTION_ID,
        shotId: SHOT_ID,
        splitRequired: false,
        totalShots: 3,
      },
    });
  });

  it('surfaces a refused edge-shot delete and keeps the dialog open (does not close/clear)', async () => {
    deleteShot.mockResolvedValue({
      success: false,
      error: 'Cannot delete a frozen shot. This shot is part of an adjusted survey network.',
    } as CommandResult);

    render(<ConfirmDialog />);
    fireEvent.click(screen.getByText('Delete'));

    expect(await screen.findByText(/Cannot delete a frozen shot/)).toBeInTheDocument();
    expect(closeDialog).not.toHaveBeenCalled();
    expect(clearSelection).not.toHaveBeenCalled();
  });

  it('surfaces a thrown transport error instead of leaving the click dead', async () => {
    deleteShot.mockRejectedValue(new Error('IPC channel closed'));

    render(<ConfirmDialog />);
    fireEvent.click(screen.getByText('Delete'));

    expect(await screen.findByTestId('delete-error')).toHaveTextContent('IPC channel closed');
    expect(closeDialog).not.toHaveBeenCalled();
    expect(clearSelection).not.toHaveBeenCalled();
  });

  it('closes on a successful edge-shot delete', async () => {
    deleteShot.mockResolvedValue({ success: true, error: null } as CommandResult);

    render(<ConfirmDialog />);
    fireEvent.click(screen.getByText('Delete'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(clearSelection).toHaveBeenCalled();
  });

  it('does not close a now-different dialog if this one was dismissed mid-flight', async () => {
    // The user dismisses the confirm dialog (activeDialog moves on) while the delete
    // IPC is still in flight. closeDialog is global UI state, so the late success-path
    // close must NOT fire -- otherwise it would close whatever dialog is open now.
    deleteShot.mockImplementation(async () => {
      useUIStore.setState({ activeDialog: 'edit-section' });
      return { success: true, error: null } as CommandResult;
    });

    render(<ConfirmDialog />);
    fireEvent.click(screen.getByText('Delete'));

    await waitFor(() => expect(clearSelection).toHaveBeenCalled());
    expect(closeDialog).not.toHaveBeenCalled();
  });
});
