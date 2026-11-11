import { describe, it, expect } from 'vitest';
import {
  fileNameFromPath,
  deriveMissingPaths,
  deriveRemoteItems,
  buildRecentListItems,
  filterRecentListItems,
  hideOtherAccountsCopies,
  type RecentListItem,
} from '../recentListItems';
import type { ProjectMetadata } from '../../../types/project';
import type { SpeleoCachedProject, SpeleoCacheOwnership, SpeleoProjectSummary } from '../../../types/ipc.generated';

function copy(id: string, name: string, ownership: SpeleoCacheOwnership): SpeleoCachedProject {
  return {
    project_id: id, project_name: name, local_path: `/cache/${id}/project.stalx`, ownership,
    has_unpushed_edits: false, state_unknown: false, is_open: false,
  };
}

function summary(partial: Partial<SpeleoProjectSummary> & { id: string }): SpeleoProjectSummary {
  return {
    name: `Project ${partial.id}`,
    permission: 'READ_AND_WRITE',
    lock_holder: null,
    type_tag: 'stalx',
    latest_commit_sha: null,
    ...partial,
  } as SpeleoProjectSummary;
}

function project(partial: Partial<ProjectMetadata> & { file_path: string }): ProjectMetadata {
  return {
    name: 'Project',
    location: null,
    modified_at: '2026-01-01T00:00:00Z',
    section_count: 0,
    total_length: 0,
    station_count: 0,
    gps_center: null,
    gps_bounds: null,
    ...partial,
  } as ProjectMetadata;
}

describe('fileNameFromPath', () => {
  it('returns the last path segment', () => {
    expect(fileNameFromPath('/home/user/caves/spring.stalx')).toBe('spring.stalx');
    expect(fileNameFromPath('C:\\caves\\spring.stalx')).toBe('spring.stalx');
    expect(fileNameFromPath('spring.stalx')).toBe('spring.stalx');
  });
});

describe('deriveMissingPaths', () => {
  it('does not offer local missing-file actions for managed SpeleoDB cache paths', () => {
    expect(deriveMissingPaths([
      '/cache/speleodb/11111111-1111-1111-1111-111111111111/project.stalx',
      '/projects/local.stalx',
    ], [])).toEqual(['/projects/local.stalx']);
  });
  it('returns config paths that have no loaded metadata, in config order', () => {
    const recentPaths = ['/a', '/gone', '/b', '/also-gone'];
    const available = [project({ file_path: '/a' }), project({ file_path: '/b' })];
    expect(deriveMissingPaths(recentPaths, available)).toEqual(['/gone', '/also-gone']);
  });

  it('returns nothing when all paths are available', () => {
    const available = [project({ file_path: '/a' })];
    expect(deriveMissingPaths(['/a'], available)).toEqual([]);
  });
});

describe('deriveRemoteItems', () => {
  it('lists each remote once, whether cached or not, sorted by name', () => {
    const summaries = {
      p2: summary({ id: 'p2', name: 'Zeta' }),
      p1: summary({ id: 'p1', name: 'Alpha' }),
      p3: summary({ id: 'p3', name: 'Mu' }),
    };
    // p3 is cached, but still uses the same remote row.
    const available = [project({ file_path: '/a', speleodb_project_id: 'p3' })];
    const items = deriveRemoteItems(summaries, available);
    expect(items.map((i) => (i.kind === 'remote' ? i.summary.name : ''))).toEqual(['Alpha', 'Mu', 'Zeta']);
    expect(items.every((i) => i.kind === 'remote')).toBe(true);
  });

  it('retains remote entries when every project is cached', () => {
    const summaries = { p1: summary({ id: 'p1' }) };
    const available = [project({ file_path: '/a', speleodb_project_id: 'p1' })];
    expect(deriveRemoteItems(summaries, available)).toEqual([{ kind: 'remote', projectId: 'p1', summary: summaries.p1 }]);
  });

  it('uses cached names with unknown access before an offline refresh can resolve them', () => {
    const cached = project({ file_path: '/cache/p1.stalx', is_remote: true, speleodb_project_id: 'p1', name: 'Cached cave' });
    const remote = deriveRemoteItems({}, [cached]);
    expect(remote).toMatchObject([{ kind: 'remote', projectId: 'p1', summary: { name: 'Cached cave', permission: null } }]);
    expect(buildRecentListItems([cached], [], remote)).toEqual(remote);
  });

  it('never emits a cached SpeleoDB project as an available local row', () => {
    const cached = project({ file_path: '/cache/p1.stalx', is_remote: true, speleodb_project_id: 'p1' });
    const summaries = { p1: summary({ id: 'p1', name: 'Server name' }) };
    expect(buildRecentListItems([cached], [], deriveRemoteItems(summaries, [cached])))
      .toEqual([{ kind: 'remote', projectId: 'p1', summary: summaries.p1 }]);
  });

  it('returns nothing when there are no scanned summaries', () => {
    expect(deriveRemoteItems({}, [project({ file_path: '/a' })])).toEqual([]);
  });

  it('lists every cached copy the session may see, even one that is not in recents', () => {
    const items = deriveRemoteItems({}, [], [
      copy('p1', 'Mine', 'mine'),
      copy('p2', 'Unknown owner', 'unknown'),
      copy('p3', 'Signed out', 'signed_out'),
      copy('p4', 'Theirs', 'other_account'),
    ]);
    expect(items.map((i) => (i.kind === 'remote' ? i.projectId : ''))).toEqual(['p1', 'p3', 'p2']);
    expect(items.every((i) => i.kind === 'remote' && i.cached?.project_id === i.projectId)).toBe(true);
    expect(items[0]).toMatchObject({ summary: { name: 'Mine', permission: null } });
  });

  it('keeps the server summary for a cached copy the listing knows', () => {
    const summaries = { p1: summary({ id: 'p1', name: 'Server name' }) };
    const [item] = deriveRemoteItems(summaries, [], [copy('p1', 'Downloaded name', 'mine')]);
    expect(item).toMatchObject({ kind: 'remote', summary: summaries.p1, cached: { project_id: 'p1' } });
  });

  it('never attaches another account\'s copy to the server row', () => {
    const summaries = { p1: summary({ id: 'p1' }) };
    expect(deriveRemoteItems(summaries, [], [copy('p1', 'Theirs', 'other_account')]))
      .toEqual([{ kind: 'remote', projectId: 'p1', summary: summaries.p1 }]);
  });
});

describe('hideOtherAccountsCopies', () => {
  const P1 = '11111111-1111-1111-1111-111111111111';
  const P2 = '22222222-2222-2222-2222-222222222222';
  const cachePath = (id: string) => `/home/u/.cache/app/speleodb/${id}/project.stalx`;
  const recents = [
    project({ file_path: '/local.stalx' }),
    project({ file_path: cachePath(P1), is_remote: true, speleodb_project_id: P1 }),
    project({ file_path: cachePath(P2), is_remote: true, speleodb_project_id: P2 }),
    // A stray linked copy of the hidden project, outside the cache: not a cached copy.
    project({ file_path: '/elsewhere.stalx', is_remote: true, speleodb_project_id: P2 }),
  ];

  it('drops only the cached files that belong to another account', () => {
    const kept = hideOtherAccountsCopies(recents, [copy(P1, 'Mine', 'mine'), copy(P2, 'Theirs', 'other_account')]);
    expect(kept.map((p) => p.file_path)).toEqual(['/local.stalx', cachePath(P1), '/elsewhere.stalx']);
    // ...and the hidden cache path never resurfaces as an "unavailable" row.
    expect(deriveMissingPaths(recents.map((p) => p.file_path), kept)).toEqual([]);
  });

  it('keeps everything when nothing is hidden (signed out)', () => {
    expect(hideOtherAccountsCopies(recents, [copy(P1, 'A', 'signed_out'), copy(P2, 'B', 'signed_out')]))
      .toBe(recents);
  });
});

describe('buildRecentListItems', () => {
  it('places available rows first, then remote, then missing rows in order', () => {
    const withRemote = buildRecentListItems(
      [project({ file_path: '/a', name: 'A' })],
      ['/gone'],
      deriveRemoteItems({ p1: summary({ id: 'p1', name: 'Remote' }) }, []),
    );
    expect(withRemote.map((i) => i.kind)).toEqual(['available', 'remote', 'missing']);
  });

  it('places available rows first, then missing rows in order', () => {
    const items = buildRecentListItems(
      [project({ file_path: '/a', name: 'Alpha' })],
      ['/gone']
    );
    expect(items.map((i) => i.kind)).toEqual(['available', 'missing']);
    expect(items[1]).toMatchObject({ kind: 'missing', filePath: '/gone', name: 'gone' });
  });
});

describe('filterRecentListItems', () => {
  const items: RecentListItem[] = [
    { kind: 'available', filePath: '/caves/alpha.stalx', project: project({ file_path: '/caves/alpha.stalx', name: 'Cave Alpha', location: 'Mexico' }) },
    { kind: 'available', filePath: '/caves/beta.stalx', project: project({ file_path: '/caves/beta.stalx', name: 'Beta', location: 'Florida' }) },
    { kind: 'missing', filePath: '/old/gamma.stalx', name: 'gamma.stalx' },
  ];

  it('returns everything for an empty/whitespace term', () => {
    expect(filterRecentListItems(items, '')).toHaveLength(3);
    expect(filterRecentListItems(items, '   ')).toHaveLength(3);
  });

  // These fixture items are all available/missing (both carry filePath); the remote
  // kind (which has no filePath) is exercised in the deriveRemoteItems tests.
  const paths = (list: RecentListItem[]) => list.map((i) => ('filePath' in i ? i.filePath : ''));

  it('matches on name (case-insensitive)', () => {
    expect(paths(filterRecentListItems(items, 'alpha'))).toEqual(['/caves/alpha.stalx']);
  });

  it('matches available rows on location', () => {
    expect(paths(filterRecentListItems(items, 'florida'))).toEqual(['/caves/beta.stalx']);
  });

  it('matches on path, including missing rows', () => {
    expect(paths(filterRecentListItems(items, 'gamma'))).toEqual(['/old/gamma.stalx']);
  });

  it('returns an empty list when nothing matches', () => {
    expect(filterRecentListItems(items, 'zzz')).toEqual([]);
  });
});
