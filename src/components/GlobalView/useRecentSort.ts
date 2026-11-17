import { useCallback, useState } from 'react';

/** Field the recent-projects list is sorted by. */
export type RecentSortField =
  | 'last_opened'
  | 'last_edited'
  | 'name'
  | 'lines'
  | 'length';

/** Sort direction. */
export type RecentSortDirection = 'asc' | 'desc';

const FIELD_KEY = 'recentProjectsSortField';
const DIRECTION_KEY = 'recentProjectsSortDirection';

const FIELDS: RecentSortField[] = [
  'last_opened',
  'last_edited',
  'name',
  'lines',
  'length',
];

const DEFAULT_FIELD: RecentSortField = 'last_edited';
const DEFAULT_DIRECTION: RecentSortDirection = 'desc';

/**
 * Sensible initial direction when switching to a field: alphabetical fields
 * ascend (A→Z); numeric/date fields descend (largest/newest first). Used when a
 * column header is clicked for a not-yet-active column.
 */
export function defaultDirectionForField(field: RecentSortField): RecentSortDirection {
  return field === 'name' ? 'asc' : 'desc';
}

function readField(): RecentSortField {
  try {
    const stored = localStorage.getItem(FIELD_KEY);
    if (stored && (FIELDS as string[]).includes(stored)) {
      return stored as RecentSortField;
    }
  } catch {
    // localStorage may be unavailable
  }
  return DEFAULT_FIELD;
}

function readDirection(): RecentSortDirection {
  try {
    const stored = localStorage.getItem(DIRECTION_KEY);
    if (stored === 'asc' || stored === 'desc') return stored;
  } catch {
    // localStorage may be unavailable
  }
  return DEFAULT_DIRECTION;
}

export interface RecentSort {
  field: RecentSortField;
  direction: RecentSortDirection;
  setField: (field: RecentSortField) => void;
  setDirection: (direction: RecentSortDirection) => void;
  toggleDirection: () => void;
}

/**
 * Recent-projects sort preference, persisted to localStorage so it survives
 * across sessions (mirrors {@link useViewMode}). Field and direction are stored
 * independently. Defaults to last-edited, newest first.
 */
export function useRecentSort(): RecentSort {
  const [field, setFieldState] = useState<RecentSortField>(readField);
  const [direction, setDirectionState] =
    useState<RecentSortDirection>(readDirection);

  const setField = useCallback((next: RecentSortField) => {
    setFieldState(next);
    try {
      localStorage.setItem(FIELD_KEY, next);
    } catch {
      // localStorage may be unavailable
    }
  }, []);

  const setDirection = useCallback((next: RecentSortDirection) => {
    setDirectionState(next);
    try {
      localStorage.setItem(DIRECTION_KEY, next);
    } catch {
      // localStorage may be unavailable
    }
  }, []);

  const toggleDirection = useCallback(() => {
    setDirectionState((current) => {
      const next = current === 'asc' ? 'desc' : 'asc';
      try {
        localStorage.setItem(DIRECTION_KEY, next);
      } catch {
        // localStorage may be unavailable
      }
      return next;
    });
  }, []);

  return { field, direction, setField, setDirection, toggleDirection };
}

/**
 * Returns a new array of projects sorted by the given field and direction.
 * `last_edited`/`lines`/`length` sort numerically; `name` uses a
 * locale-aware comparison; `last_opened` preserves the backend's recency order
 * (the input array is already most-recently-opened first - there is no
 * per-project "opened at" timestamp, so the original position is the signal).
 * Direction flips the comparator: `desc` puts the largest/most-recent first.
 */
export function sortRecentProjects<
  T extends {
    name: string;
    file_path: string;
    modified_at: string;
    line_count: number;
    total_length: number;
  }
>(projects: T[], field: RecentSortField, direction: RecentSortDirection): T[] {
  const sign = direction === 'asc' ? 1 : -1;
  // Original positions encode last-opened recency (index 0 = most recent).
  const openedIndex = new Map(projects.map((p, i) => [p.file_path, i]));
  const sorted = [...projects];
  sorted.sort((a, b) => {
    let cmp: number;
    switch (field) {
      case 'last_opened': {
        // Treat a more-recent (lower) index as the "larger" value so that
        // `desc` surfaces the most-recently-opened project first.
        const ia = openedIndex.get(a.file_path) ?? 0;
        const ib = openedIndex.get(b.file_path) ?? 0;
        cmp = ib - ia;
        break;
      }
      case 'name':
        cmp = a.name.localeCompare(b.name);
        break;
      case 'lines':
        cmp = a.line_count - b.line_count;
        break;
      case 'length':
        cmp = a.total_length - b.total_length;
        break;
      case 'last_edited':
      default:
        cmp =
          new Date(a.modified_at).getTime() - new Date(b.modified_at).getTime();
        break;
    }
    // Stable tie-break by name so equal keys keep a deterministic order.
    if (cmp === 0) cmp = a.name.localeCompare(b.name);
    return sign * cmp;
  });
  return sorted;
}
