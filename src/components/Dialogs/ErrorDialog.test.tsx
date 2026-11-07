/**
 * Component test (#447): the "report this as a bug" hint is caller-controlled
 * via `dialogData.hideReportHint`, not hardcoded for every error. Unexpected
 * failures (the default) still get the hint; callers surfacing a user-fixable
 * condition (a damaged import file, "nothing to import") set the flag to drop it.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('../../utils/telemetry', async () => (await import('../../test/factories/telemetry')).telemetryWith());

import '../../i18n';
import ErrorDialog from './ErrorDialog';
import { useUIStore } from '../../store/uiStore';

beforeEach(() => {
  useUIStore.setState({
    closeDialog: vi.fn(),
    dialogData: { title: 'Import Failed', message: 'The file is corrupt.' },
  });
});

describe('ErrorDialog report-a-bug hint (#447)', () => {
  it('shows the report-a-bug hint by default (an unexpected failure)', () => {
    render(<ErrorDialog />);

    expect(
      screen.getByText(/please report it as a bug/i),
    ).toBeInTheDocument();
  });

  it('hides the report-a-bug hint when the caller marks the failure as user-fixable', () => {
    useUIStore.setState({
      dialogData: { title: 'Import Failed', message: 'The file is corrupt.', hideReportHint: true },
    });

    render(<ErrorDialog />);

    expect(screen.queryByText(/please report it as a bug/i)).not.toBeInTheDocument();
    expect(screen.getByText('The file is corrupt.')).toBeInTheDocument();
  });
});
