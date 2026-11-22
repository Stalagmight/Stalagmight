/**
 * The read-only notice is titled for what happened (demo item 7): an open whose
 * automatic Start Editing did not get the lock says the project opened read-only;
 * a Resume editing on a project already open that left it read-only does not say
 * it opened.
 */
import { beforeEach, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: (e: unknown) => String(e) }));

import { SpeleoReadOnlyNotice } from './SpeleoReadOnlyNotice';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { showSpeleoReadOnlyNotice } from '../../utils/autoAcquireSpeleoLock';

beforeEach(() => {
  useProjectStore.setState({ project: { name: 'Synthetic cave', file_path: '/synthetic/p.stalx' } as never });
  useSpeleoStore.setState({ sessionGeneration: 0 });
});

it.each([
  ['opened', 'Project opened read-only'],
  ['stillReadOnly', 'Still read-only'],
] as const)('a %s notice is titled "%s"', (kind, title) => {
  showSpeleoReadOnlyNotice('Synthetic reason.', kind);
  render(<SpeleoReadOnlyNotice />);
  expect(screen.getByTestId('speleodb-read-only-dialog')).toHaveTextContent(title);
  expect(screen.getByText('Synthetic reason.')).toBeVisible();
  if (kind === 'stillReadOnly') expect(screen.queryByText('Project opened read-only')).toBeNull();
});
