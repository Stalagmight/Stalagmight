import type { ProjectMetadata } from '../../types/project';
import type { RecentListItem } from './recentListItems';
import type { RecentSortDirection, RecentSortField } from './useRecentSort';

/** Sort the displayed list as a whole, using only server dates for remote entries.
 * Missing values stay last in either direction; names break ties predictably.
 */
export function sortRecentListItems(
  items: RecentListItem[],
  projects: ProjectMetadata[],
  field: RecentSortField,
  direction: RecentSortDirection,
  recentPaths: string[] = projects.map((p) => p.file_path),
): RecentListItem[] {
  const remotePaths = new Map(projects.filter((p) => p.is_remote)
    .map((p) => [p.speleodb_project_id, p.file_path]));
  const opened = new Map(recentPaths.map((path, index) => [path, -index]));
  const name = (item: RecentListItem) => item.kind === 'remote' ? item.summary.name
    : item.kind === 'available' ? item.project.name : item.name;
  const value = (item: RecentListItem): string | number | undefined => {
    if (field === 'name') return name(item);
    if (field === 'last_opened') {
      const path = item.kind === 'remote' ? remotePaths.get(item.projectId) : item.filePath;
      return path === undefined ? undefined : opened.get(path);
    }
    if (field === 'last_edited') {
      const date = item.kind === 'remote' ? item.summary.modified_date
        : item.kind === 'available' ? item.project.modified_at : undefined;
      const timestamp = date ? Date.parse(date) : NaN;
      return Number.isFinite(timestamp) ? timestamp : undefined;
    }
    if (item.kind !== 'available') return undefined;
    return field === 'sections' ? item.project.section_count : item.project.total_length;
  };
  const sign = direction === 'asc' ? 1 : -1;
  return [...items].sort((a, b) => {
    const av = value(a);
    const bv = value(b);
    if (av === undefined && bv !== undefined) return 1;
    if (av !== undefined && bv === undefined) return -1;
    const comparison = typeof av === 'string' && typeof bv === 'string'
      ? av.localeCompare(bv)
      : typeof av === 'number' && typeof bv === 'number' ? av - bv : 0;
    return comparison * sign || name(a).localeCompare(name(b));
  });
}
