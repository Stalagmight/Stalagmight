/**
 * #576 D27-32(c): a config save sends only the settings that changed.
 */
import { describe, it, expect } from 'vitest';
import { configPatches } from '../configPatches';

describe('configPatches (#576 D27-32(c))', () => {
  const original = {
    appearance: { theme: 'dark', ui_scale: 1 },
    panels: { data_table_open: true, sections_panel_width: 300 },
    general: { recent_projects: ['/a.stalx'], undo_history_limit: 100 },
    cache: { providers: { 'osm.tiles': { ttl_days: 3 }, esri: { ttl_days: 5 } } },
  };

  it('is empty when nothing changed', () => {
    expect(configPatches(original, structuredClone(original))).toEqual([]);
  });

  it('names only the changed leaves, by field path', () => {
    const current = structuredClone(original);
    current.appearance.theme = 'light';
    current.general.undo_history_limit = 20;
    expect(configPatches(original, current)).toEqual([
      { path: ['appearance', 'theme'], value: 'light' },
      { path: ['general', 'undo_history_limit'], value: 20 },
    ]);
  });

  it('sends a changed array whole', () => {
    const current = structuredClone(original);
    current.general.recent_projects = ['/b.stalx', '/a.stalx'];
    expect(configPatches(original, current)).toEqual([
      { path: ['general', 'recent_projects'], value: ['/b.stalx', '/a.stalx'] },
    ]);
  });

  it('keeps a map key with a dot as one path segment, and sends a new entry setting by setting', () => {
    const current = structuredClone(original) as typeof original & {
      cache: { providers: Record<string, { ttl_days: number }> };
    };
    current.cache.providers['osm.tiles'] = { ttl_days: 9 };
    current.cache.providers.local = { ttl_days: 1 };
    expect(configPatches(original, current)).toEqual([
      { path: ['cache', 'providers', 'osm.tiles', 'ttl_days'], value: 9 },
      { path: ['cache', 'providers', 'local', 'ttl_days'], value: 1 },
    ]);
  });

  it('replaces a map whole when an entry was removed', () => {
    const current = structuredClone(original) as { cache: { providers: Record<string, unknown> } };
    delete current.cache.providers.esri;
    expect(configPatches(original, current as typeof original)).toEqual([
      { path: ['cache', 'providers'], value: { 'osm.tiles': { ttl_days: 3 } } },
    ]);
  });

  it('sends the settings of a table the snapshot lacked one by one, and an empty one not at all', () => {
    const base = { general: { auto_save: true } } as Record<string, unknown>;
    const current = { general: { auto_save: true }, import: {}, suggestion: { sigma: 18 } };
    expect(configPatches(base, current)).toEqual([{ path: ['suggestion', 'sigma'], value: 18 }]);
  });

  it('sends a cleared optional setting as null', () => {
    const base = { speleodb: { endpoint: 'https://x' as string | null } };
    expect(configPatches(base, { speleodb: { endpoint: null } })).toEqual([
      { path: ['speleodb', 'endpoint'], value: null },
    ]);
  });
});
