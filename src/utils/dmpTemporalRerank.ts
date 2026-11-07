import type { CrossSectionSuggestion, DmpSection, SuggestionConfidence } from '../types/commands';
import type { DistanceField, StationRef } from '../types/project';
import type { VirtualConnection } from './segmentContext';

const MINUTE_MS = 60_000;

export const TEMPORAL_CLUSTER_BREAK_MINUTES = 8 * 60;
export const TEMPORAL_SOFT_SPEED_M_PER_MIN = 12;
export const TEMPORAL_HARD_SPEED_M_PER_MIN = 25;
export const TEMPORAL_BEAM_WIDTH = 100;
export const TEMPORAL_CANDIDATES_PER_SECTION = 10;

/**
 * Confidence tiers, best first. Used to surface the highest-confidence feasible
 * candidate first *within* each import section, so temporal planning can't demote
 * a feasible higher-confidence suggestion below a lower one (#101). Typed to the
 * generated union so tsc flags it if the IPC enum gains/renames a tier.
 */
const CONFIDENCE_RANK: Record<SuggestionConfidence, number> = { high: 0, medium: 1, low: 2 };
/** Tier for an unrecognized confidence value - treat as lowest so a malformed
 *  input can never displace a valid higher-confidence candidate. */
const CONFIDENCE_RANK_FALLBACK = 3;

type DmpTiming = {
  sectionIndex: number;
  startedAtMs: number;
  endedAtMs: number;
  startStationIndex: number;
  endStationIndex: number;
};

type TemporalPair = {
  fromSectionIndex: number;
  toSectionIndex: number;
  minutes: number;
};

type Graph = Map<string, Array<{ to: string; weight: number }>>;

type PlanState = {
  connections: VirtualConnection[];
  placed: Set<number>;
  firstSuggestion: CrossSectionSuggestion | null;
  geometryScore: number;
  penalty: number;
  checkedPairs: number;
};

export type TemporalRerankResult = {
  suggestions: CrossSectionSuggestion[];
  rejectedCount: number;
};

function standardShots(section: DmpSection) {
  return section.shots.filter((s) => s.shot_type === 'Standard');
}

function parseSectionDate(section: DmpSection): Date | null {
  const d = new Date(section.date);
  return Number.isFinite(d.getTime()) ? d : null;
}

export function deriveDmpTiming(section: DmpSection, sectionIndex: number): DmpTiming | null {
  const base = parseSectionDate(section);
  if (!base) return null;

  const shots = standardShots(section);
  if (shots.length === 0) return null;

  const times = shots
    .filter((s) => s.hour !== 0 || s.minute !== 0 || s.second !== 0)
    .map((s) => ({ hour: s.hour, minute: s.minute, second: s.second }));

  if (times.length !== shots.length) return null;

  let dayOffset = 0;
  let previous: number | null = null;
  const unrolled = times.map((t) => {
    let d = new Date(base.getTime());
    d.setHours(t.hour, t.minute, t.second, 0);
    d = new Date(d.getTime() + dayOffset * 24 * 60 * MINUTE_MS);
    if (previous != null && d.getTime() < previous) {
      dayOffset += 1;
      d = new Date(d.getTime() + 24 * 60 * MINUTE_MS);
    }
    previous = d.getTime();
    return d.getTime();
  });

  const startedAtMs = unrolled[0];
  const endedAtMs = unrolled[unrolled.length - 1];
  if (startedAtMs == null || endedAtMs == null || endedAtMs < startedAtMs) return null;

  return {
    sectionIndex,
    startedAtMs,
    endedAtMs,
    startStationIndex: 0,
    endStationIndex: shots.length,
  };
}

export function buildTemporalPairs(sections: DmpSection[]): TemporalPair[] {
  const timings = sections
    .map((section, idx) => deriveDmpTiming(section, idx))
    .filter((t): t is DmpTiming => t != null)
    .sort((a, b) => a.startedAtMs - b.startedAtMs);

  const pairs: TemporalPair[] = [];
  for (let i = 0; i < timings.length - 1; i++) {
    const from = timings[i]!;
    const to = timings[i + 1]!;
    const minutes = (to.startedAtMs - from.endedAtMs) / MINUTE_MS;
    if (minutes < 0) continue;
    if (minutes > TEMPORAL_CLUSTER_BREAK_MINUTES) continue;
    pairs.push({
      fromSectionIndex: from.sectionIndex,
      toSectionIndex: to.sectionIndex,
      minutes,
    });
  }
  return pairs;
}

function dmpNode(sectionIndex: number, stationIndex: number): string {
  return `d:${sectionIndex}:${stationIndex}`;
}

function existingNode(ref: StationRef): string {
  return `e:${ref.section_id}:${ref.station_index}`;
}

export function existingDistanceKey(from: StationRef, to: StationRef): string {
  return `${from.section_id}:${from.station_index}->${to.section_id}:${to.station_index}`;
}

/**
 * The distances from `origin` that `get_station_distances_from` answered, keyed for
 * the rerank. The command answers null when no project is open (#576 NF-W5I-2): there
 * are no existing stations to measure from, so that is an empty set, not a field to
 * read.
 */
export function distanceMapFromField(origin: StationRef, field: DistanceField | null): Map<string, number> {
  const map = new Map<string, number>();
  if (!field) return map;
  field.station_refs.forEach((ref, idx) => {
    const distance = field.distances[idx];
    if (distance != null && Number.isFinite(distance)) {
      map.set(existingDistanceKey(origin, ref), distance);
    }
  });
  return map;
}

function addEdge(graph: Graph, a: string, b: string, weight: number) {
  if (!Number.isFinite(weight) || weight < 0) return;
  if (!graph.has(a)) graph.set(a, []);
  if (!graph.has(b)) graph.set(b, []);
  graph.get(a)!.push({ to: b, weight });
  graph.get(b)!.push({ to: a, weight });
}

function defaultMergePair(s: CrossSectionSuggestion, sections: DmpSection[]) {
  const section = sections[s.importSectionIndex];
  const importShotCount = section ? standardShots(section).length : 0;
  const maxImportOffset = s.isPartial ? s.matchedShots - 1 : s.matchedShots;
  const offsets = Array.from({ length: maxImportOffset + 1 }, (_, idx) => maxImportOffset - idx);
  const orderedOffsets = s.preferredMergeOffset != null
    ? [
      ...offsets.filter((importOffset) => importOffset === s.preferredMergeOffset),
      ...offsets.filter((importOffset) => importOffset !== s.preferredMergeOffset),
    ]
    : offsets;

  for (const offset of orderedOffsets) {
    const importStationIndex = s.importStationIndex === 0 ? offset : s.importStationIndex - offset;
    const existingStationIndex = s.direction === 'forward'
      ? s.existingStationIndex + offset
      : s.existingStationIndex - offset;

    if (importStationIndex < 0 || importStationIndex > importShotCount) continue;
    if (existingStationIndex < 0) continue;

    if (s.targetType === 'importing' && s.targetImportIndex != null) {
      const target = sections[s.targetImportIndex];
      const targetShotCount = target ? standardShots(target).length : 0;
      if (existingStationIndex > targetShotCount) continue;
    }

    return { importStationIndex, existingStationIndex };
  }

  return {
    importStationIndex: s.importStationIndex,
    existingStationIndex: s.existingStationIndex,
  };
}

function suggestionToConnection(s: CrossSectionSuggestion, sections: DmpSection[]): VirtualConnection {
  const merge = defaultMergePair(s, sections);
  return {
    importSectionIndex: s.importSectionIndex,
    importStationIndex: merge.importStationIndex,
    targetType: s.targetType,
    ...(s.targetType === 'existing' && s.existingSectionId != null
      ? { existingSectionId: s.existingSectionId, existingStationIndex: merge.existingStationIndex }
      : {}),
    ...(s.targetType === 'importing' && s.targetImportIndex != null
      ? { targetImportIndex: s.targetImportIndex, targetImportStationIndex: merge.existingStationIndex }
      : {}),
  };
}

function connectionKey(c: VirtualConnection): string {
  if (c.targetType === 'existing') {
    return `${c.importSectionIndex}:${c.importStationIndex}->e:${c.existingSectionId}:${c.existingStationIndex}`;
  }
  return `${c.importSectionIndex}:${c.importStationIndex}->d:${c.targetImportIndex}:${c.targetImportStationIndex}`;
}

function buildGraph(
  sections: DmpSection[],
  connections: VirtualConnection[],
  existingDistances: Map<string, number>,
): Graph {
  const graph: Graph = new Map();

  sections.forEach((section, sectionIndex) => {
    const shots = standardShots(section);
    shots.forEach((shot, shotIndex) => {
      addEdge(
        graph,
        dmpNode(sectionIndex, shotIndex),
        dmpNode(sectionIndex, shotIndex + 1),
        shot.length,
      );
    });
  });

  const existingRefs: StationRef[] = [];
  for (const c of connections) {
    if (c.targetType === 'existing' && c.existingSectionId != null && c.existingStationIndex != null) {
      const ref = { section_id: c.existingSectionId, station_index: c.existingStationIndex };
      existingRefs.push(ref);
      addEdge(graph, dmpNode(c.importSectionIndex, c.importStationIndex), existingNode(ref), 0);
    } else if (c.targetType === 'importing' && c.targetImportIndex != null && c.targetImportStationIndex != null) {
      addEdge(
        graph,
        dmpNode(c.importSectionIndex, c.importStationIndex),
        dmpNode(c.targetImportIndex, c.targetImportStationIndex),
        0,
      );
    }
  }

  for (let i = 0; i < existingRefs.length; i++) {
    for (let j = i + 1; j < existingRefs.length; j++) {
      const a = existingRefs[i]!;
      const b = existingRefs[j]!;
      const dist = existingDistances.get(existingDistanceKey(a, b))
        ?? existingDistances.get(existingDistanceKey(b, a));
      if (dist != null) addEdge(graph, existingNode(a), existingNode(b), dist);
    }
  }

  return graph;
}

function shortestPath(graph: Graph, start: string, goal: string): number | null {
  if (start === goal) return 0;
  if (!graph.has(start) || !graph.has(goal)) return null;

  const dist = new Map<string, number>([[start, 0]]);
  const visited = new Set<string>();

  for (;;) {
    let bestNode: string | null = null;
    let bestDist = Infinity;
    for (const [node, d] of dist) {
      if (!visited.has(node) && d < bestDist) {
        bestNode = node;
        bestDist = d;
      }
    }
    if (bestNode == null) return null;
    if (bestNode === goal) return bestDist;
    visited.add(bestNode);

    for (const edge of graph.get(bestNode) ?? []) {
      if (visited.has(edge.to)) continue;
      const next = bestDist + edge.weight;
      if (next < (dist.get(edge.to) ?? Infinity)) {
        dist.set(edge.to, next);
      }
    }
  }
}

function evaluateConnections(
  sections: DmpSection[],
  pairs: TemporalPair[],
  connections: VirtualConnection[],
  existingDistances: Map<string, number>,
): { hardRejected: boolean; penalty: number; checkedPairs: number } {
  const graph = buildGraph(sections, connections, existingDistances);
  let penalty = 0;
  let checkedPairs = 0;

  for (const pair of pairs) {
    if (pair.minutes <= 0) continue;
    const fromTiming = deriveDmpTiming(sections[pair.fromSectionIndex]!, pair.fromSectionIndex);
    const toTiming = deriveDmpTiming(sections[pair.toSectionIndex]!, pair.toSectionIndex);
    if (!fromTiming || !toTiming) continue;

    const distance = shortestPath(
      graph,
      dmpNode(pair.fromSectionIndex, fromTiming.endStationIndex),
      dmpNode(pair.toSectionIndex, toTiming.startStationIndex),
    );
    if (distance == null) continue;

    checkedPairs += 1;
    const speed = distance / pair.minutes;
    if (speed > TEMPORAL_HARD_SPEED_M_PER_MIN) {
      return { hardRejected: true, penalty: Infinity, checkedPairs };
    }
    if (speed > TEMPORAL_SOFT_SPEED_M_PER_MIN) {
      const ratio = (speed - TEMPORAL_SOFT_SPEED_M_PER_MIN)
        / (TEMPORAL_HARD_SPEED_M_PER_MIN - TEMPORAL_SOFT_SPEED_M_PER_MIN);
      penalty += ratio * 0.75;
    }
  }

  return { hardRejected: false, penalty, checkedPairs };
}

function hasDirectCycle(connections: VirtualConnection[], candidate: CrossSectionSuggestion): boolean {
  if (candidate.targetType !== 'importing' || candidate.targetImportIndex == null) return false;
  return connections.some((c) =>
    c.targetType === 'importing'
    && c.importSectionIndex === candidate.targetImportIndex
    && c.targetImportIndex === candidate.importSectionIndex
  );
}

function uniqueTopCandidates(pool: CrossSectionSuggestion[]): CrossSectionSuggestion[] {
  const perSection = new Map<number, CrossSectionSuggestion[]>();
  const byGeometryScore = [...pool].sort((a, b) => b.score - a.score);
  for (const suggestion of byGeometryScore) {
    const arr = perSection.get(suggestion.importSectionIndex) ?? [];
    if (arr.length < TEMPORAL_CANDIDATES_PER_SECTION) {
      arr.push(suggestion);
      perSection.set(suggestion.importSectionIndex, arr);
    }
  }
  return Array.from(perSection.values()).flat();
}

function rankStates(a: PlanState, b: PlanState): number {
  return b.placed.size - a.placed.size
    || b.checkedPairs - a.checkedPairs
    || (b.geometryScore - b.penalty) - (a.geometryScore - a.penalty)
    || b.geometryScore - a.geometryScore;
}

export function rerankSuggestionsByTemporalPlan(
  pool: CrossSectionSuggestion[],
  sections: DmpSection[],
  fixedConnections: VirtualConnection[],
  existingDistances: Map<string, number> = new Map(),
): TemporalRerankResult {
  if (pool.length === 0) return { suggestions: [], rejectedCount: 0 };

  const temporalPairs = buildTemporalPairs(sections);
  if (temporalPairs.length === 0) {
    return { suggestions: pool, rejectedCount: 0 };
  }

  const candidates = uniqueTopCandidates(pool);
  const fixedPlaced = new Set(fixedConnections.map((c) => c.importSectionIndex));
  const fixedKeys = new Set(fixedConnections.map(connectionKey));
  let states: PlanState[] = [{
    connections: [...fixedConnections],
    placed: fixedPlaced,
    firstSuggestion: null,
    geometryScore: 0,
    penalty: 0,
    checkedPairs: 0,
  }];
  let rejectedCount = 0;

  const maxDepth = new Set(candidates.map((c) => c.importSectionIndex)).size;
  for (let depth = 0; depth < maxDepth; depth++) {
    const nextStates: PlanState[] = [];

    for (const state of states) {
      for (const candidate of candidates) {
        if (state.placed.has(candidate.importSectionIndex)) continue;
        if (hasDirectCycle(state.connections, candidate)) continue;

        const connection = suggestionToConnection(candidate, sections);
        if (fixedKeys.has(connectionKey(connection))) continue;

        const connections = [...state.connections, connection];
        const evalResult = evaluateConnections(sections, temporalPairs, connections, existingDistances);
        if (evalResult.hardRejected) {
          rejectedCount += 1;
          continue;
        }

        const placed = new Set(state.placed);
        placed.add(candidate.importSectionIndex);
        nextStates.push({
          connections,
          placed,
          firstSuggestion: state.firstSuggestion ?? candidate,
          geometryScore: state.geometryScore + candidate.score,
          penalty: evalResult.penalty,
          checkedPairs: evalResult.checkedPairs,
        });
      }
    }

    if (nextStates.length === 0) break;
    nextStates.sort(rankStates);
    states = nextStates.slice(0, TEMPORAL_BEAM_WIDTH);
  }

  const bestFirstKeys: string[] = [];
  for (const state of [...states].sort(rankStates)) {
    if (state.firstSuggestion) {
      const key = `${state.firstSuggestion.importSectionIndex}:${state.firstSuggestion.importStationIndex}:${state.firstSuggestion.targetType}:${state.firstSuggestion.existingSectionId ?? ''}:${state.firstSuggestion.targetImportIndex ?? ''}:${state.firstSuggestion.existingStationIndex}:${state.firstSuggestion.direction}`;
      if (!bestFirstKeys.includes(key)) bestFirstKeys.push(key);
    }
  }

  const keyFor = (s: CrossSectionSuggestion) =>
    `${s.importSectionIndex}:${s.importStationIndex}:${s.targetType}:${s.existingSectionId ?? ''}:${s.targetImportIndex ?? ''}:${s.existingStationIndex}:${s.direction}`;

  const feasible = pool.filter((suggestion) => {
    if (hasDirectCycle(fixedConnections, suggestion)) return false;
    const evalResult = evaluateConnections(
      sections,
      temporalPairs,
      [...fixedConnections, suggestionToConnection(suggestion, sections)],
      existingDistances,
    );
    if (evalResult.hardRejected) {
      rejectedCount += 1;
      return false;
    }
    return true;
  });

  feasible.sort((a, b) => {
    const ai = bestFirstKeys.indexOf(keyFor(a));
    const bi = bestFirstKeys.indexOf(keyFor(b));
    if (ai !== -1 || bi !== -1) {
      if (ai === -1) return 1;
      if (bi === -1) return -1;
      return ai - bi;
    }
    return b.score - a.score;
  });

  // #101: within each import section, surface the highest-confidence feasible
  // candidate first - but do NOT disturb the temporal plan's cross-section order
  // (the plan still chooses which section leads). The plan's `rankStates` weighs
  // `checkedPairs` above geometry, so a weak import-to-import link (e.g. AC3→AC2,
  // Low) that "explains" a temporal adjacency can become a section's first
  // suggestion over a strong existing-section match (AC3→Mainline 2, High). A
  // *global* confidence sort would over-correct, letting an unrelated
  // high-confidence section jump ahead of a low/medium link the plan chose to
  // satisfy a temporal pair. Grouping by section and stably reordering within
  // each group fixes the same-section High→Low inversion without touching section
  // order. Array.sort is stable, so same-tier candidates keep their plan/score order.
  // Fall back to the lowest tier for any unrecognized confidence so a malformed
  // value yields a finite comparator (never NaN) and can't displace a valid one.
  const tierOf = (s: CrossSectionSuggestion): number =>
    (CONFIDENCE_RANK as Record<string, number>)[s.confidence] ?? CONFIDENCE_RANK_FALLBACK;
  // `feasible` is already in plan/score order; capture each index so the within-
  // group sort below is deterministic without relying on Array.sort stability.
  const planOrder = new Map<CrossSectionSuggestion, number>();
  feasible.forEach((s, i) => planOrder.set(s, i));

  const groups = new Map<number, CrossSectionSuggestion[]>();
  const sectionOrder: number[] = [];
  for (const s of feasible) {
    let group = groups.get(s.importSectionIndex);
    if (!group) {
      group = [];
      groups.set(s.importSectionIndex, group);
      sectionOrder.push(s.importSectionIndex);
    }
    group.push(s);
  }
  const ordered = sectionOrder.flatMap((idx) => {
    const group = groups.get(idx)!;
    group.sort((a, b) => tierOf(a) - tierOf(b) || planOrder.get(a)! - planOrder.get(b)!);
    return group;
  });

  return { suggestions: ordered, rejectedCount };
}

export function collectExistingAnchorsForTemporalRerank(
  suggestions: CrossSectionSuggestion[],
  connections: VirtualConnection[],
  sections: DmpSection[] = [],
): StationRef[] {
  const byKey = new Map<string, StationRef>();
  const add = (ref: StationRef) => {
    byKey.set(`${ref.section_id}:${ref.station_index}`, ref);
  };

  for (const s of suggestions) {
    if (s.targetType === 'existing' && s.existingSectionId != null) {
      add({
        section_id: s.existingSectionId,
        station_index: defaultMergePair(s, sections).existingStationIndex,
      });
    }
  }
  for (const c of connections) {
    if (c.targetType === 'existing' && c.existingSectionId != null && c.existingStationIndex != null) {
      add({ section_id: c.existingSectionId, station_index: c.existingStationIndex });
    }
  }

  return Array.from(byKey.values());
}
