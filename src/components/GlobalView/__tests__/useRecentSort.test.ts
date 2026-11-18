import { describe, it, expect } from 'vitest';
import { sortRecentProjects } from '../useRecentSort';

interface Row {
  name: string;
  file_path: string;
  modified_at: string;
  line_count: number;
  total_length: number;
}

// In backend (and store) order: index 0 is the most-recently-opened project.
const projects: Row[] = [
  { name: 'Bravo', file_path: '/p/bravo', modified_at: '2026-01-02T00:00:00Z', line_count: 3, total_length: 800 },
  { name: 'Alpha', file_path: '/p/alpha', modified_at: '2026-01-04T00:00:00Z', line_count: 5, total_length: 1200 },
  { name: 'Delta', file_path: '/p/delta', modified_at: '2026-01-01T00:00:00Z', line_count: 8, total_length: 2100 },
  { name: 'Charlie', file_path: '/p/charlie', modified_at: '2026-01-03T00:00:00Z', line_count: 2, total_length: 350 },
];

const paths = (rows: Row[]) => rows.map((r) => r.file_path);

describe('sortRecentProjects', () => {
  it('last_opened desc preserves the backend recency order (most recent first)', () => {
    const result = sortRecentProjects(projects, 'last_opened', 'desc');
    expect(paths(result)).toEqual(['/p/bravo', '/p/alpha', '/p/delta', '/p/charlie']);
  });

  it('last_opened asc reverses to oldest-opened first', () => {
    const result = sortRecentProjects(projects, 'last_opened', 'asc');
    expect(paths(result)).toEqual(['/p/charlie', '/p/delta', '/p/alpha', '/p/bravo']);
  });

  it('last_edited desc orders by modified_at, newest first', () => {
    const result = sortRecentProjects(projects, 'last_edited', 'desc');
    expect(paths(result)).toEqual(['/p/alpha', '/p/charlie', '/p/bravo', '/p/delta']);
  });

  it('name asc is alphabetical', () => {
    const result = sortRecentProjects(projects, 'name', 'asc');
    expect(result.map((r) => r.name)).toEqual(['Alpha', 'Bravo', 'Charlie', 'Delta']);
  });

  it('lines desc orders by line count, largest first', () => {
    const result = sortRecentProjects(projects, 'lines', 'desc');
    expect(result.map((r) => r.line_count)).toEqual([8, 5, 3, 2]);
  });

  it('length asc orders by total length, smallest first', () => {
    const result = sortRecentProjects(projects, 'length', 'asc');
    expect(result.map((r) => r.total_length)).toEqual([350, 800, 1200, 2100]);
  });

  it('does not mutate the input array', () => {
    const before = paths(projects);
    sortRecentProjects(projects, 'name', 'asc');
    expect(paths(projects)).toEqual(before);
  });
});
