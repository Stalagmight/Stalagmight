/**
 * [#576 Q-W44-7-D1] A dive-plan Save as, Save or Delete the user started, refused
 * because the project changed under it (the backend's PROJECT_CHANGED), shows the
 * short "the project changed" line in the bar's inline error instead of the
 * backend's own text. Any other refusal keeps its text.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/divePlanApi', () => ({
  listDivePlans: vi.fn(async () => []),
  loadDivePlan: vi.fn(),
  saveDivePlan: vi.fn(),
  deleteDivePlan: vi.fn(),
  planAnchorsOf: vi.fn(() => null),
}));

import { DivePlanBar } from '../DivePlanBar';
import { useProjectStore } from '../../../store/projectStore';
import { saveDivePlan } from '../../../utils/divePlanApi';
import { IpcCommandError } from '../../../utils/commandError';

const SHORT = 'The project changed before this finished.';
const BACKEND_TEXT = 'Saving the dive plan was not applied: another project is open now.';

const saveAs = async (name: string) => {
  await userEvent.click(screen.getByTestId('dive-plan-save-as'));
  await userEvent.type(screen.getByTestId('dive-plan-name-input'), name);
  await userEvent.click(screen.getByTestId('dive-plan-save-as-confirm'));
};

describe('DivePlanBar: a refusal because the project changed (#576 Q-W44-7-D1)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useProjectStore.setState({
      project: { name: 'Cave A', created_at: '2026-07-11T00:00:00Z', file_path: '/mock/a.stalx' } as never,
      databaseOpen: true,
    });
  });

  it('Save as shows the short line, not the backend text', async () => {
    vi.mocked(saveDivePlan).mockRejectedValueOnce(
      new IpcCommandError({ code: 'PROJECT_CHANGED', message: BACKEND_TEXT }),
    );
    render(<DivePlanBar />);

    await saveAs('Plan A');

    const error = await screen.findByTestId('dive-plan-error');
    expect(error).toHaveTextContent(SHORT);
    expect(error).not.toHaveTextContent('another project is open now');
  });

  it('any other refusal keeps its own text', async () => {
    vi.mocked(saveDivePlan).mockRejectedValueOnce(
      new IpcCommandError({ code: 'IO', message: 'disk full' }),
    );
    render(<DivePlanBar />);

    await saveAs('Plan A');

    const error = await screen.findByTestId('dive-plan-error');
    expect(error).toHaveTextContent('disk full');
    expect(error).not.toHaveTextContent(SHORT);
  });
});
