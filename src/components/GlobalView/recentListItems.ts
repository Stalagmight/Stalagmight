import type { ProjectMetadata } from '../../types/project';
import type { SpeleoCachedProject, SpeleoProjectSummary } from '../../types/ipc.generated';
import { isListedCachedCopy } from '../../utils/speleoCachedCopies';

/** Local projects, unavailable local paths, and SpeleoDB projects.
 * Every SpeleoDB project is remote here, regardless of whether it has a cached copy.
 */
export type RecentListItem =
  | { kind: 'available'; filePath: string; project: ProjectMetadata }
  | { kind: 'missing'; filePath: string; name: string }
  | {
      kind: 'remote';
      projectId: string;
      summary: SpeleoProjectSummary;
      /** This computer's cached copy of the project, when the session may see it. */
      cached?: SpeleoCachedProject;
    };

/** Last path segment (filename with extension), for naming missing entries. */
export function fileNameFromPath(path: string): string {
  const parts = path.split(/[/\\]/).filter((p) => p.length > 0);
  return parts[parts.length - 1] ?? path;
}

/**
 * Recent paths that have no loaded metadata (file missing, moved, or
 * unreadable), preserving the order they appear in the config (most-recently
 * opened first).
 */
export function deriveMissingPaths(
  recentPaths: string[],
  available: Pick<ProjectMetadata, 'file_path'>[]
): string[] {
  const have = new Set(available.map((p) => p.file_path));
  // Managed cache files are represented by their remote project, never by local
  // missing-file actions. Their cache path is fixed by the backend.
  return recentPaths.filter((p) => !have.has(p) && !isManagedCachePath(p));
}

/** `<cache>/speleodb/<project uuid>/project.stalx`: the one place a managed copy lives. */
function isManagedCachePath(path: string): boolean {
  return /[/\\]speleodb[/\\][0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}[/\\]project\.stalx$/i.test(path);
}

/** All SpeleoDB projects use the same remote entry, including cached projects.
 * A cached name keeps the project discoverable when the startup refresh is offline;
 * permission and lock state stay unknown until the server answers.
 */
export function remoteSummaryForProject(
  project: ProjectMetadata,
  summaries: Record<string, SpeleoProjectSummary>,
): SpeleoProjectSummary | null {
  if (!project.is_remote || !project.speleodb_project_id) return null;
  return summaries[project.speleodb_project_id] ?? {
    id: project.speleodb_project_id,
    name: project.name,
    permission: null,
    lock_holder: null,
    type_tag: 'stalx',
    latest_commit_sha: null,
  };
}

/** The summary a cached copy shows when the server listing does not (yet) have it. */
export function remoteSummaryForCachedCopy(
  copy: SpeleoCachedProject,
  summaries: Record<string, SpeleoProjectSummary>,
): SpeleoProjectSummary {
  return summaries[copy.project_id] ?? {
    id: copy.project_id,
    name: copy.project_name,
    permission: null,
    lock_holder: null,
    type_tag: 'stalx',
    latest_commit_sha: null,
  };
}

/**
 * Recents without the cached copies that belong to another SpeleoDB account (or
 * instance) than the signed-in one. Signed out, the backend lists every copy, so
 * nothing is hidden. A recent whose project has no cached copy is left alone.
 */
export function hideOtherAccountsCopies(
  recents: ProjectMetadata[],
  copies: SpeleoCachedProject[],
): ProjectMetadata[] {
  const hidden = new Set(copies.filter((copy) => !isListedCachedCopy(copy)).map((copy) => copy.project_id));
  if (hidden.size === 0) return recents;
  // Only the cached file itself: a stray linked copy elsewhere is not a cached copy
  // (opening it routes to the managed copy), and hiding it would turn its recents
  // entry into an "unavailable" row.
  return recents.filter((p) => !(
    p.is_remote && p.speleodb_project_id && hidden.has(p.speleodb_project_id) && isManagedCachePath(p.file_path)
  ));
}

export function deriveRemoteItems(
  summaries: Record<string, SpeleoProjectSummary>,
  available: ProjectMetadata[],
  copies: SpeleoCachedProject[] = [],
): RecentListItem[] {
  const projects = new Map(Object.values(summaries).map((s) => [s.id, s]));
  // Every cached copy this session may see, whether or not it is in recents (a
  // sign-out used to forget clean ones, and a copy can be downloaded from the
  // browser without ever being opened from here).
  for (const copy of copies) {
    if (isListedCachedCopy(copy) && !projects.has(copy.project_id)) {
      projects.set(copy.project_id, remoteSummaryForCachedCopy(copy, summaries));
    }
  }
  for (const project of available) {
    const summary = remoteSummaryForProject(project, summaries);
    if (summary && !projects.has(summary.id)) projects.set(summary.id, summary);
  }
  const cached = new Map(copies.filter(isListedCachedCopy).map((copy) => [copy.project_id, copy]));
  return [...projects.values()]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((summary): RecentListItem => {
      const copy = cached.get(summary.id);
      return copy
        ? { kind: 'remote', projectId: summary.id, summary, cached: copy }
        : { kind: 'remote', projectId: summary.id, summary };
    });
}

/**
 * Build the combined list before applying the selected sort to all row kinds.
 */
export function buildRecentListItems(
  availableProjects: ProjectMetadata[],
  missingPaths: string[],
  remoteItems: RecentListItem[] = []
): RecentListItem[] {
  const available: RecentListItem[] = availableProjects.filter((project) => !project.is_remote).map((project) => ({
    kind: 'available',
    filePath: project.file_path,
    project,
  }));
  const missing: RecentListItem[] = missingPaths.map((filePath) => ({
    kind: 'missing',
    filePath,
    name: fileNameFromPath(filePath),
  }));
  return [...available, ...remoteItems, ...missing];
}

/**
 * Case-insensitive filter over name, location, and path. Available rows match
 * on any of the three; missing rows match on name or path. An empty/whitespace
 * term returns the list unchanged.
 */
export function filterRecentListItems(
  items: RecentListItem[],
  term: string
): RecentListItem[] {
  const needle = term.trim().toLowerCase();
  if (!needle) return items;
  return items.filter((item) => {
    if (item.kind === 'available') {
      const { name, location, file_path } = item.project;
      return (
        name.toLowerCase().includes(needle) ||
        (location?.toLowerCase().includes(needle) ?? false) ||
        file_path.toLowerCase().includes(needle)
      );
    }
    if (item.kind === 'remote') {
      return item.summary.name.toLowerCase().includes(needle);
    }
    return (
      item.name.toLowerCase().includes(needle) ||
      item.filePath.toLowerCase().includes(needle)
    );
  });
}
