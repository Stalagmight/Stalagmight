import type { CrossLineSuggestion, DmpSection, SuggestionConfidence } from '../types/commands';
import type { DistanceField, StationRef } from '../types/project';
import type { VirtualConnection } from './segmentContext';

const MINUTE_MS = 60_000;

export const TEMPORAL_CLUSTER_BREAK_MINUTES = 8 * 60;
export const TEMPORAL_SOFT_SPEED_M_PER_MIN = 12;
export const TEMPORAL_HARD_SPEED_M_PER_MIN = 25;
export const TEMPORAL_BEAM_WIDTH = 100;
export const TEMPORAL_CANDIDATES_PER_LINE = 10;

/**
 * Confidence tiers, best first. Used to surface the highest-confidence feasible
 * candidate first *within* each import line, so temporal planning can't demote
 * a feasible higher-confidence suggestion below a lower one (#101). Typed to the
 * generated union so tsc flags it if the IPC enum gains/renames a tier.
 */
const CONFIDENCE_RANK: Record<SuggestionConfidence, number> = { high: 0, medium: 1, low: 2 };
/** Tier for an unrecognized confidence value - treat as lowest so a malformed
 *  input can never displace a valid higher-confidence candidate. */
const CONFIDENCE_RANK_FALLBACK = 3;

type DmpTiming = {
  lineIndex: number;
  startedAtMs: number;
  endedAtMs: number;
  startStationIndex: number;
  endStationIndex: number;
};

type TemporalPair = {
  fromLineIndex: number;
  toLineIndex: number;
  minutes: number;
};

type Graph = Map<string, Array<{ to: string; weight: number }>>;

type PlanState = {
  connections: VirtualConnection[];
  placed: Set<number>;
  firstSuggestion: CrossLineSuggestion | null;
  geometryScore: number;
  penalty: number;
  checkedPairs: number;
};

export type TemporalRerankResult = {
  suggestions: CrossLineSuggestion[];
  rejectedCount: number;
};

function standardShots(line: DmpSection) {
  return line.shots.filter((s) => s.shot_type === 'Standard');
}

function parseLineDate(line: DmpSection): Date | null {
  const d = new Date(line.date);
  return Number.isFinite(d.getTime()) ? d : null;
}

export function deriveDmpTiming(line: DmpSection, lineIndex: number): DmpTiming | null {
  const base = parseLineDate(line);
  if (!base) return null;

  const shots = standardShots(line);
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
    lineIndex,
    startedAtMs,
    endedAtMs,
    startStationIndex: 0,
    endStationIndex: shots.length,
  };
}

export function buildTemporalPairs(lines: DmpSection[]): TemporalPair[] {
  const timings = lines
    .map((line, idx) => deriveDmpTiming(line, idx))
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
      fromLineIndex: from.lineIndex,
      toLineIndex: to.lineIndex,
      minutes,
    });
  }
  return pairs;
}

function dmpNode(lineIndex: number, stationIndex: number): string {
  return `d:${lineIndex}:${stationIndex}`;
}

function existingNode(ref: StationRef): string {
  return `e:${ref.line_id}:${ref.station_index}`;
}

export function existingDistanceKey(from: StationRef, to: StationRef): string {
  return `${from.line_id}:${from.station_index}->${to.line_id}:${to.station_index}`;
}

export function distanceMapFromField(origin: StationRef, field: DistanceField): Map<string, number> {
  const map = new Map<string, number>();
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

function defaultMergePair(s: CrossLineSuggestion, lines: DmpSection[]) {
  const line = lines[s.importLineIndex];
  const importShotCount = line ? standardShots(line).length : 0;
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
      const target = lines[s.targetImportIndex];
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

function suggestionToConnection(s: CrossLineSuggestion, lines: DmpSection[]): VirtualConnection {
  const merge = defaultMergePair(s, lines);
  return {
    importLineIndex: s.importLineIndex,
    importStationIndex: merge.importStationIndex,
    targetType: s.targetType,
    ...(s.targetType === 'existing' && s.existingLineId != null
      ? { existingLineId: s.existingLineId, existingStationIndex: merge.existingStationIndex }
      : {}),
    ...(s.targetType === 'importing' && s.targetImportIndex != null
      ? { targetImportIndex: s.targetImportIndex, targetImportStationIndex: merge.existingStationIndex }
      : {}),
  };
}

function connectionKey(c: VirtualConnection): string {
  if (c.targetType === 'existing') {
    return `${c.importLineIndex}:${c.importStationIndex}->e:${c.existingLineId}:${c.existingStationIndex}`;
  }
  return `${c.importLineIndex}:${c.importStationIndex}->d:${c.targetImportIndex}:${c.targetImportStationIndex}`;
}

function buildGraph(
  lines: DmpSection[],
  connections: VirtualConnection[],
  existingDistances: Map<string, number>,
): Graph {
  const graph: Graph = new Map();

  lines.forEach((line, lineIndex) => {
    const shots = standardShots(line);
    shots.forEach((shot, shotIndex) => {
      addEdge(
        graph,
        dmpNode(lineIndex, shotIndex),
        dmpNode(lineIndex, shotIndex + 1),
        shot.length,
      );
    });
  });

  const existingRefs: StationRef[] = [];
  for (const c of connections) {
    if (c.targetType === 'existing' && c.existingLineId != null && c.existingStationIndex != null) {
      const ref = { line_id: c.existingLineId, station_index: c.existingStationIndex };
      existingRefs.push(ref);
      addEdge(graph, dmpNode(c.importLineIndex, c.importStationIndex), existingNode(ref), 0);
    } else if (c.targetType === 'importing' && c.targetImportIndex != null && c.targetImportStationIndex != null) {
      addEdge(
        graph,
        dmpNode(c.importLineIndex, c.importStationIndex),
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
  lines: DmpSection[],
  pairs: TemporalPair[],
  connections: VirtualConnection[],
  existingDistances: Map<string, number>,
): { hardRejected: boolean; penalty: number; checkedPairs: number } {
  const graph = buildGraph(lines, connections, existingDistances);
  let penalty = 0;
  let checkedPairs = 0;

  for (const pair of pairs) {
    if (pair.minutes <= 0) continue;
    const fromTiming = deriveDmpTiming(lines[pair.fromLineIndex]!, pair.fromLineIndex);
    const toTiming = deriveDmpTiming(lines[pair.toLineIndex]!, pair.toLineIndex);
    if (!fromTiming || !toTiming) continue;

    const distance = shortestPath(
      graph,
      dmpNode(pair.fromLineIndex, fromTiming.endStationIndex),
      dmpNode(pair.toLineIndex, toTiming.startStationIndex),
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

function hasDirectCycle(connections: VirtualConnection[], candidate: CrossLineSuggestion): boolean {
  if (candidate.targetType !== 'importing' || candidate.targetImportIndex == null) return false;
  return connections.some((c) =>
    c.targetType === 'importing'
    && c.importLineIndex === candidate.targetImportIndex
    && c.targetImportIndex === candidate.importLineIndex
  );
}

function uniqueTopCandidates(pool: CrossLineSuggestion[]): CrossLineSuggestion[] {
  const perLine = new Map<number, CrossLineSuggestion[]>();
  const byGeometryScore = [...pool].sort((a, b) => b.score - a.score);
  for (const suggestion of byGeometryScore) {
    const arr = perLine.get(suggestion.importLineIndex) ?? [];
    if (arr.length < TEMPORAL_CANDIDATES_PER_LINE) {
      arr.push(suggestion);
      perLine.set(suggestion.importLineIndex, arr);
    }
  }
  return Array.from(perLine.values()).flat();
}

function rankStates(a: PlanState, b: PlanState): number {
  return b.placed.size - a.placed.size
    || b.checkedPairs - a.checkedPairs
    || (b.geometryScore - b.penalty) - (a.geometryScore - a.penalty)
    || b.geometryScore - a.geometryScore;
}

export function rerankSuggestionsByTemporalPlan(
  pool: CrossLineSuggestion[],
  lines: DmpSection[],
  fixedConnections: VirtualConnection[],
  existingDistances: Map<string, number> = new Map(),
): TemporalRerankResult {
  if (pool.length === 0) return { suggestions: [], rejectedCount: 0 };

  const temporalPairs = buildTemporalPairs(lines);
  if (temporalPairs.length === 0) {
    return { suggestions: pool, rejectedCount: 0 };
  }

  const candidates = uniqueTopCandidates(pool);
  const fixedPlaced = new Set(fixedConnections.map((c) => c.importLineIndex));
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

  const maxDepth = new Set(candidates.map((c) => c.importLineIndex)).size;
  for (let depth = 0; depth < maxDepth; depth++) {
    const nextStates: PlanState[] = [];

    for (const state of states) {
      for (const candidate of candidates) {
        if (state.placed.has(candidate.importLineIndex)) continue;
        if (hasDirectCycle(state.connections, candidate)) continue;

        const connection = suggestionToConnection(candidate, lines);
        if (fixedKeys.has(connectionKey(connection))) continue;

        const connections = [...state.connections, connection];
        const evalResult = evaluateConnections(lines, temporalPairs, connections, existingDistances);
        if (evalResult.hardRejected) {
          rejectedCount += 1;
          continue;
        }

        const placed = new Set(state.placed);
        placed.add(candidate.importLineIndex);
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
      const key = `${state.firstSuggestion.importLineIndex}:${state.firstSuggestion.importStationIndex}:${state.firstSuggestion.targetType}:${state.firstSuggestion.existingLineId ?? ''}:${state.firstSuggestion.targetImportIndex ?? ''}:${state.firstSuggestion.existingStationIndex}:${state.firstSuggestion.direction}`;
      if (!bestFirstKeys.includes(key)) bestFirstKeys.push(key);
    }
  }

  const keyFor = (s: CrossLineSuggestion) =>
    `${s.importLineIndex}:${s.importStationIndex}:${s.targetType}:${s.existingLineId ?? ''}:${s.targetImportIndex ?? ''}:${s.existingStationIndex}:${s.direction}`;

  const feasible = pool.filter((suggestion) => {
    if (hasDirectCycle(fixedConnections, suggestion)) return false;
    const evalResult = evaluateConnections(
      lines,
      temporalPairs,
      [...fixedConnections, suggestionToConnection(suggestion, lines)],
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

  // #101: within each import line, surface the highest-confidence feasible
  // candidate first - but do NOT disturb the temporal plan's cross-line order
  // (the plan still chooses which line leads). The plan's `rankStates` weighs
  // `checkedPairs` above geometry, so a weak import-to-import link (e.g. AC3→AC2,
  // Low) that "explains" a temporal adjacency can become a line's first
  // suggestion over a strong existing-line match (AC3→Mainline 2, High). A
  // *global* confidence sort would over-correct, letting an unrelated
  // high-confidence line jump ahead of a low/medium link the plan chose to
  // satisfy a temporal pair. Grouping by line and stably reordering within
  // each group fixes the same-line High→Low inversion without touching line
  // order. Array.sort is stable, so same-tier candidates keep their plan/score order.
  // Fall back to the lowest tier for any unrecognized confidence so a malformed
  // value yields a finite comparator (never NaN) and can't displace a valid one.
  const tierOf = (s: CrossLineSuggestion): number =>
    (CONFIDENCE_RANK as Record<string, number>)[s.confidence] ?? CONFIDENCE_RANK_FALLBACK;
  // `feasible` is already in plan/score order; capture each index so the within-
  // group sort below is deterministic without relying on Array.sort stability.
  const planOrder = new Map<CrossLineSuggestion, number>();
  feasible.forEach((s, i) => planOrder.set(s, i));

  const groups = new Map<number, CrossLineSuggestion[]>();
  const lineOrder: number[] = [];
  for (const s of feasible) {
    let group = groups.get(s.importLineIndex);
    if (!group) {
      group = [];
      groups.set(s.importLineIndex, group);
      lineOrder.push(s.importLineIndex);
    }
    group.push(s);
  }
  const ordered = lineOrder.flatMap((idx) => {
    const group = groups.get(idx)!;
    group.sort((a, b) => tierOf(a) - tierOf(b) || planOrder.get(a)! - planOrder.get(b)!);
    return group;
  });

  return { suggestions: ordered, rejectedCount };
}

export function collectExistingAnchorsForTemporalRerank(
  suggestions: CrossLineSuggestion[],
  connections: VirtualConnection[],
  lines: DmpSection[] = [],
): StationRef[] {
  const byKey = new Map<string, StationRef>();
  const add = (ref: StationRef) => {
    byKey.set(`${ref.line_id}:${ref.station_index}`, ref);
  };

  for (const s of suggestions) {
    if (s.targetType === 'existing' && s.existingLineId != null) {
      add({
        line_id: s.existingLineId,
        station_index: defaultMergePair(s, lines).existingStationIndex,
      });
    }
  }
  for (const c of connections) {
    if (c.targetType === 'existing' && c.existingLineId != null && c.existingStationIndex != null) {
      add({ line_id: c.existingLineId, station_index: c.existingStationIndex });
    }
  }

  return Array.from(byKey.values());
}
