/**
 * [NF4] The loading window's error state is a modal alert dialog; the same
 * component shown inline (the export dialog's progress) claims no modality.
 *
 * In the loading window the error holds the app until Retry or Dismiss, yet it
 * rendered as a bare `role="alert"` with an unlabelled heading and nothing
 * `aria-modal`. With `modal` (set by ProjectLoadingOverlay only) it is a
 * `role="alertdialog"`, `aria-modal`, labelled by its heading and described by
 * its message. Real i18n, `.tsx` so vitest runs it under jsdom.
 */
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import '../../../i18n';
import { LoadingProgress } from '../LoadingProgress';

const error = { message: 'Could not read the project file.', details: 'ENOENT: /caves/CaveB.stalx' };

describe('LoadingProgress modality (NF4)', () => {
  it('in the loading window, the error state is a modal alert dialog named by its heading and described by its message', () => {
    render(<LoadingProgress modal title="Loading failed" phase="" overallProgress={0} error={error} onCancel={() => {}} />);

    const dialog = screen.getByRole('alertdialog', { name: 'Loading error' });
    expect(dialog).toBe(screen.getByTestId('loading-progress'));
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(dialog).toHaveAccessibleDescription('Could not read the project file.');
    // Focus goes into it (the loading window moves it), so it is no live region as well.
    expect(dialog).not.toHaveAttribute('aria-live');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('in the loading window, the progress state is modal too', () => {
    render(<LoadingProgress modal title="Loading CaveB..." phase="Loading project data" overallProgress={25} onCancel={() => {}} />);

    const dialog = screen.getByRole('dialog', { name: 'Loading CaveB...' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
  });

  // Guards (pass before and after): the inline use is unchanged.
  it('inline, the error state stays an alert: no alert dialog, nothing aria-modal', () => {
    render(<LoadingProgress title="Exporting..." phase="" overallProgress={0} error={error} onCancel={() => {}} />);

    const alert = screen.getByRole('alert');
    expect(alert).toBe(screen.getByTestId('loading-progress'));
    expect(alert).toHaveAttribute('aria-live', 'assertive');
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(document.querySelector('[aria-modal]')).toBeNull();
  });

  it('inline, the progress state is not aria-modal', () => {
    render(<LoadingProgress title="Exporting..." phase="Preparing" overallProgress={10} />);

    expect(screen.getByRole('dialog', { name: 'Exporting...' })).not.toHaveAttribute('aria-modal');
  });
});
