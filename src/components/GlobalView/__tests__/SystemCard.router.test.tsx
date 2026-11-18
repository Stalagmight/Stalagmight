/**
 * #450: a recents card delegates opening to its `onOpen` prop (wired to
 * GlobalView's `handleOpenFromList` / `openRecentEntry`, which owns the
 * telemetry attribution and the `.stalx`-only routing gate - see
 * openRecentEntry.test.ts) rather than routing or attributing itself.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/react';

import SystemCard from '../SystemCard';
import { systemCardTestId } from '../../../utils/testId';

const FILE_PATH = '/projects/cave-alpha.stalx';

describe('SystemCard - onOpen delegation (#450)', () => {
  it('calls onOpen with the card path on double-click', () => {
    const onOpen = vi.fn();
    const { getByTestId } = render(
      <SystemCard
        name="Alpha"
        location={null}
        filePath={FILE_PATH}
        modifiedAt="2026-07-26T00:00:00Z"
        lineCount={3}
        totalLength={120}
        onOpen={onOpen}
      />,
    );

    fireEvent.doubleClick(getByTestId(systemCardTestId(FILE_PATH)));

    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenCalledWith(FILE_PATH);
  });

  it('does not throw when double-clicked with no onOpen prop', () => {
    const { getByTestId } = render(
      <SystemCard
        name="Alpha"
        location={null}
        filePath={FILE_PATH}
        modifiedAt="2026-07-26T00:00:00Z"
        lineCount={3}
        totalLength={120}
      />,
    );

    expect(() => fireEvent.doubleClick(getByTestId(systemCardTestId(FILE_PATH)))).not.toThrow();
  });
});
