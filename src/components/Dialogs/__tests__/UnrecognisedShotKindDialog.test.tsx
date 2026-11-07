/**
 * #27: the unreadable-shot-kind prompt.
 *
 * The backend fails an unrecognised `kind` byte closed to `Reference` so nothing
 * can route through a leg it could not parse. That is safe but it is a
 * substitution the user never agreed to, so the dialog surfaces it. The
 * behaviour that matters here: the safe answer is preselected, and confirming
 * without touching anything must issue NO edits.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

// jsdom does not implement scrollIntoView; the Select component calls it on open.
Element.prototype.scrollIntoView = vi.fn();

/** Pick `value` from a portal-based `Select` (src/components/common/Select.tsx):
 *  it isn't a native `<select>`, so this clicks the trigger, then the option
 *  rendered at `data-testid="${testId}-option-${value}"`. */
function pickOption(testId: string, value: string) {
  fireEvent.click(screen.getByTestId(testId));
  fireEvent.click(screen.getByTestId(`${testId}-option-${encodeURIComponent(value)}`));
}

const executeBatch = vi.fn().mockResolvedValue({ success: true });
const closeDialog = vi.fn();

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).translationStub());

const dialogData: { shots?: unknown } = {};

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) =>
    sel({ closeDialog, dialogData }),
}));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ executeBatch }),
}));

vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryStubs());

vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

import UnrecognisedShotKindDialog from '../UnrecognisedShotKindDialog';

const shot = (shotId: number, raw: string) => ({
  section_id: 0,
  shot_id: shotId,
  shot_number: shotId + 1,
  section_name: 'Main',
  raw_value: raw,
});

describe('UnrecognisedShotKindDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    dialogData.shots = [shot(1, '7'), shot(2, "'not-a-number'")];
  });

  it('renders nothing when there is nothing to resolve', () => {
    dialogData.shots = [];
    const { container } = render(<UnrecognisedShotKindDialog />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the raw stored value so corruption is distinguishable from a newer file', () => {
    render(<UnrecognisedShotKindDialog />);
    expect(screen.getByText('7')).toBeInTheDocument();
    // Rendered via SQLite's quote(), so a non-integer storage class - the case
    // an integer read would have failed on entirely - shows up legibly.
    expect(screen.getByText("'not-a-number'")).toBeInTheDocument();
  });

  it('preselects Reference - the fail-closed answer the backend already applied', () => {
    render(<UnrecognisedShotKindDialog />);
    expect(screen.getByTestId('unrecognised-kind-select-1')).toHaveTextContent('shotKind.reference');
    expect(screen.getByTestId('unrecognised-kind-select-2')).toHaveTextContent('shotKind.reference');
  });

  it('issues NO edits when the user accepts the defaults', async () => {
    render(<UnrecognisedShotKindDialog />);
    fireEvent.click(screen.getByTestId('unrecognised-kind-apply'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(executeBatch).not.toHaveBeenCalled();
  });

  it('issues an edit only for the shots the user actually changed', async () => {
    render(<UnrecognisedShotKindDialog />);
    pickOption('unrecognised-kind-select-1', 'Jump');
    fireEvent.click(screen.getByTestId('unrecognised-kind-apply'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(executeBatch).toHaveBeenCalledTimes(1);
    expect(executeBatch).toHaveBeenCalledWith([
      { type: 'update_shot', section_id: 0, shot_id: 1, updates: { kind: 'Jump' } },
    ]);
  });

  it('stays open and explains when an edit is rejected', async () => {
    // setShotKind REPORTS failure rather than throwing (read-only SpeleoDB
    // project, or a lock re-verify that rejects). Closing anyway would lose the
    // user's choices for good: the backend list is already consumed and cannot
    // be re-offered.
    executeBatch.mockResolvedValueOnce({ success: false, error: 'Project is read-only' });
    render(<UnrecognisedShotKindDialog />);
    pickOption('unrecognised-kind-select-1', 'Jump');
    fireEvent.click(screen.getByTestId('unrecognised-kind-apply'));

    await waitFor(() =>
      expect(screen.getByTestId('unrecognised-kind-error')).toHaveTextContent(
        'Project is read-only',
      ),
    );
    expect(closeDialog).not.toHaveBeenCalled();
    // The user's selection survives, so they can retry once the lock is held.
    expect(screen.getByTestId('unrecognised-kind-select-1')).toHaveTextContent('shotKind.jump');
  });

  it('sends every change as ONE batch, not a call per shot', async () => {
    // A newer file can use an unknown discriminant on every shot in the survey.
    // Per-shot commands would each refetch the whole project state, undo state,
    // warnings and the integrity check.
    render(<UnrecognisedShotKindDialog />);
    for (const id of [1, 2]) {
      pickOption(`unrecognised-kind-select-${id}`, 'Jump');
    }
    fireEvent.click(screen.getByTestId('unrecognised-kind-apply'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(executeBatch).toHaveBeenCalledTimes(1);
    expect(executeBatch.mock.calls[0]![0]).toHaveLength(2);
  });

  it('dismissing leaves everything as Reference', () => {
    render(<UnrecognisedShotKindDialog />);
    fireEvent.click(screen.getByText('dialogs.unrecognisedShotKind.keepAsReference'));
    expect(executeBatch).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
  });
});
