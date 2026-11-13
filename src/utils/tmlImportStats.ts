import type { TmlFile, TmlSection, TmlShot } from '../types/commands';
import type { VirtualShotMode } from '../types/project';

/**
 * The shots the backend will actually import under the current toggles: CLOSURE
 * rows are never imported (they are loop markers, not measurements) and excluded
 * rows only when the user opts in - mirrors `convert_shots_to_shot_data`
 * (src-tauri/src/parsers/tml.rs).
 */
export function importableShots(section: TmlSection, includeExcluded: boolean): TmlShot[] {
  return section.shots.filter(
    (shot) => shot.shot_type !== 'Closure' && (includeExcluded || !shot.excluded)
  );
}

export interface TmlImportStats {
  totalSections: number;
  totalShots: number;
  virtualCount: number;
  /**
   * The survey length the project will show once imported (#689): every
   * imported shot but the virtual ones the import makes References
   * ({@link jumpShots}, `virtualShotMode`).
   */
  totalLength: number;
  depthRange: string | null;
  excludedCount: number;
  commentCount: number;
}

/**
 * Section/shot counts under the given toggles - the single source of truth for
 * every screen that quotes a TML file's size, so the create-project promise and
 * the import dialog's own numbers cannot drift apart (#411).
 *
 * `selected` (#425) narrows every figure to the sections the user kept ticked;
 * `null` - what the pre-dialog screens pass, having no selection to speak of -
 * counts the whole file. Note `excludedCount` and `commentCount` narrow too:
 * they drive the dialog's two option toggles, which are pointless when the only
 * rows they would affect are in sections that are not being imported.
 *
 * `virtualShotMode` decides which virtual shots count toward the length: a
 * Reference is left out of every survey length after import (#689), a Jump
 * counts. All jumps counts them all, All references none, and Auto runs the
 * import's chain rule ({@link jumpShots}).
 */
export function computeTmlImportStats(
  tmlFile: TmlFile,
  includeExcluded: boolean,
  selected: ReadonlySet<number> | null = null,
  virtualShotMode: VirtualShotMode = 'Auto'
): TmlImportStats {
  let totalSections = 0;
  let totalShots = 0;
  let totalLength = 0;
  let minDepth = Infinity;
  let maxDepth = -Infinity;
  let excludedCount = 0;
  let commentCount = 0;
  let virtualCount = 0;

  const kept = tmlFile.sections.filter((_, index) => selected === null || selected.has(index));
  const imported = kept.flatMap((section) => importableShots(section, includeExcluded));
  const jumps =
    virtualShotMode === 'Auto'
      ? jumpShots(imported, kept.flatMap((section) => section.shots.filter((shot) => shot.shot_type === 'Closure')))
      : null;
  const counts = (shot: TmlShot): boolean =>
    shot.shot_type !== 'Virtual' || virtualShotMode === 'AllJumps' || (jumps?.has(shot) ?? false);

  for (const [index, section] of tmlFile.sections.entries()) {
    if (selected !== null && !selected.has(index)) continue;
    // A section left with no shots by the toggles is dropped by the converter
    // ("no shots after filtering"), so it must not be counted as arriving.
    let sectionShots = 0;
    for (const shot of section.shots) {
      if (shot.shot_type === 'Closure') continue;
      if (shot.excluded) excludedCount++;
      if (!includeExcluded && shot.excluded) continue;
      sectionShots++;
      totalShots++;
      if (counts(shot)) totalLength += shot.length;
      if (shot.depth < minDepth) minDepth = shot.depth;
      if (shot.depth > maxDepth) maxDepth = shot.depth;
      if (shot.comment) commentCount++;
      if (shot.shot_type === 'Virtual') virtualCount++;
    }
    if (sectionShots > 0) totalSections++;
  }

  return {
    totalSections,
    totalShots,
    virtualCount,
    totalLength: Math.round(totalLength * 10) / 10,
    depthRange:
      minDepth === Infinity
        ? null
        : `${Math.round(minDepth * 10) / 10} - ${Math.round(maxDepth * 10) / 10}`,
    excludedCount,
    commentCount,
  };
}

/**
 * The virtual shots among `shots` that the import's Auto mode makes Jumps,
 * by its chain rule (`classify_virtual_shots`, #27): a connected run of
 * virtual shots that reaches real survey at two or more distinct points -
 * through loop closures, which join two stations into one point - is a Jump;
 * any other run is a Reference. Evaluated on the file's own stations, its
 * Ariane ids (a section starts at its parent's station, so they connect the
 * way the imported sections are merged). An import into a project that
 * already holds survey can reach more real points through it, so there this
 * can only undercount the Jumps.
 */
export function jumpShots(shots: readonly TmlShot[], closures: readonly TmlShot[]): Set<TmlShot> {
  const parent = new Map<string, string>();
  const find = (id: string): string => {
    let root = id;
    while (parent.has(root) && parent.get(root) !== root) root = parent.get(root)!;
    let at = id;
    while (at !== root) {
      const next = parent.get(at) ?? root;
      parent.set(at, root);
      at = next;
    }
    return root;
  };
  const union = (a: string, b: string): void => {
    const ra = find(a);
    const rb = find(b);
    if (ra !== rb) parent.set(ra, rb);
  };
  const key = (id: bigint | number): string => String(id);

  for (const closure of closures) union(key(closure.from_id), key(closure.closure_to_id));

  const real = new Set<string>();
  const virtual: TmlShot[] = [];
  for (const shot of shots) {
    if (shot.shot_type === 'Virtual') virtual.push(shot);
    else if (shot.shot_type === 'Real') {
      real.add(find(key(shot.from_id)));
      real.add(find(key(shot.id)));
    }
  }
  // Runs: virtual shots that share a station (after the closures).
  const run = new Map<string, string>();
  const findRun = (id: string): string => {
    let root = id;
    while (run.has(root) && run.get(root) !== root) root = run.get(root)!;
    return root;
  };
  for (const shot of virtual) {
    const a = findRun(find(key(shot.from_id)));
    const b = findRun(find(key(shot.id)));
    if (a !== b) run.set(a, b);
  }
  const realPoints = new Map<string, Set<string>>();
  for (const shot of virtual) {
    for (const end of [find(key(shot.from_id)), find(key(shot.id))]) {
      if (!real.has(end)) continue;
      const root = findRun(end);
      if (!realPoints.has(root)) realPoints.set(root, new Set());
      realPoints.get(root)!.add(end);
    }
  }
  return new Set(virtual.filter((shot) => (realPoints.get(findRun(find(key(shot.from_id))))?.size ?? 0) >= 2));
}
