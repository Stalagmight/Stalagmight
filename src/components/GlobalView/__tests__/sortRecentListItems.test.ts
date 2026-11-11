import { describe, expect, it } from 'vitest';
import { buildRecentListItems, deriveRemoteItems } from '../recentListItems';
import { sortRecentListItems } from '../sortRecentListItems';
import type { ProjectMetadata } from '../../../types/project';
import type { SpeleoProjectSummary } from '../../../types/ipc.generated';

const local = { name: 'Bravo', file_path: '/local', modified_at: '2026-02-01T00:00:00Z',
  section_count: 3, total_length: 100 } as ProjectMetadata;
const cached = { ...local, name: 'Alpha', file_path: '/cache', is_remote: true,
  speleodb_project_id: 'remote', modified_at: '2099-01-01T00:00:00Z' };
const remote = (id: string, name: string, modified_date?: string): SpeleoProjectSummary => ({
  id, name, ...(modified_date === undefined ? {} : { modified_date }), permission: null, lock_holder: null, type_tag: 'stalagmight', latest_commit_sha: null,
});
const projects = [cached, local];
const summaries = { remote: remote('remote', 'Alpha', '2026-01-01T00:00:00Z'),
  new: remote('new', 'Zulu', '2026-03-01T00:00:00+00:00'),
  unknown: remote('unknown', 'Unknown'), invalid: remote('invalid', 'Invalid', 'bad-date') };
const items = buildRecentListItems(projects, [], deriveRemoteItems(summaries, projects));
const names = (rows: typeof items) => rows.map((r) => r.kind === 'remote' ? r.summary.name
  : r.kind === 'available' ? r.project.name : r.name);

describe('combined recent sorting', () => {
  it('mixes local and remote dates newest first, ignoring cache modification time', () => {
    expect(names(sortRecentListItems(items, projects, 'last_edited', 'desc')))
      .toEqual(['Zulu', 'Bravo', 'Alpha', 'Invalid', 'Unknown']);
  });
  it('reverses known dates while keeping missing and invalid dates at the bottom', () => {
    expect(names(sortRecentListItems(items, projects, 'last_edited', 'asc')))
      .toEqual(['Alpha', 'Bravo', 'Zulu', 'Invalid', 'Unknown']);
  });
  it('sorts names in both directions across all row kinds', () => {
    const ascending = names(sortRecentListItems(items, projects, 'name', 'asc'));
    expect(ascending).toEqual(['Alpha', 'Bravo', 'Invalid', 'Unknown', 'Zulu']);
    expect(names(sortRecentListItems(items, projects, 'name', 'desc'))).toEqual([...ascending].reverse());
  });
  it('uses backend recency for cached remote projects and leaves unopened entries last', () => {
    expect(names(sortRecentListItems(items, projects, 'last_opened', 'desc')).slice(0, 2)).toEqual(['Alpha', 'Bravo']);
    expect(names(sortRecentListItems(items, projects, 'last_opened', 'asc')).slice(0, 2)).toEqual(['Bravo', 'Alpha']);
  });
  it('never substitutes cached dates when remote metadata is unavailable', () => {
    const offline = buildRecentListItems(projects, [], deriveRemoteItems({}, projects));
    expect(names(sortRecentListItems(offline, projects, 'last_edited', 'desc'))).toEqual(['Bravo', 'Alpha']);
  });
});
