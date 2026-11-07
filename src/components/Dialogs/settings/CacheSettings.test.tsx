/**
 * Component test (#576 C6) - the Clear cache button must be disabled when the
 * cache is empty. ts-rs used to type `CacheStats.total_tiles` (a Rust u64) as
 * `bigint`, but `get_cache_stats` is a JSON `invoke()`: serde writes the u64 as
 * a JSON number and the frontend receives a plain JS number. A `=== 0n` compare
 * was then `0 === 0n` (false), so the button stayed enabled on an empty cache.
 * The generated type now says `number`, which is what these tests feed.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

vi.mock('../../../utils/ipc');

import { CacheSettings } from './CacheSettings';
import { defaultConfig } from '../../../types/config';
import type { CacheStats } from '../../../types/cache';
import { invokeIpcMock } from '../../../test/factories/ipc';

/** What `get_cache_stats` actually delivers: JSON numbers, never bigint. */
function wireStats(tiles: number, bytes: number): CacheStats {
  return JSON.parse(
    JSON.stringify({
      total_tiles: tiles,
      total_bytes: bytes,
      providers: tiles > 0 ? { esri: { tiles, bytes } } : {},
    })
  ) as CacheStats;
}

async function renderWithStats(stats: CacheStats): Promise<HTMLElement> {
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'get_cache_stats') return stats;
    return null;
  });
  render(
    <CacheSettings
      config={defaultConfig}
      onChange={vi.fn()}
      changedFields={new Set()}
      onValidationError={vi.fn()}
      panelIdBase="settings"
    />
  );
  await waitFor(() => expect(screen.getByTestId('cache-tile-count')).toBeInTheDocument());
  return screen.getByTestId('clear-cache-button');
}

beforeEach(() => {
  invokeIpcMock.mockReset();
});

describe('CacheSettings Clear cache button (#576 C6)', () => {
  it('is disabled when the wire delivers total_tiles as the plain number 0', async () => {
    const stats = wireStats(0, 0);
    expect(typeof stats.total_tiles).toBe('number');
    const button = await renderWithStats(stats);
    expect(screen.getByTestId('cache-tile-count')).toHaveTextContent('0');
    expect(button).toBeDisabled();
  });

  it('is enabled when the wire delivers a nonzero plain-number tile count', async () => {
    const button = await renderWithStats(wireStats(150, 25600000));
    expect(screen.getByTestId('cache-tile-count')).toHaveTextContent('150');
    expect(button).toBeEnabled();
  });

  it('is disabled on the zero fallback a failed stats load leaves', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_cache_stats') throw new Error('cache unavailable');
      return null;
    });
    render(
      <CacheSettings
        config={defaultConfig}
        onChange={vi.fn()}
        changedFields={new Set()}
        onValidationError={vi.fn()}
        panelIdBase="settings"
      />
    );
    await waitFor(() => expect(screen.getByTestId('cache-tile-count')).toHaveTextContent('0'));
    expect(screen.getByTestId('clear-cache-button')).toBeDisabled();
  });
});
