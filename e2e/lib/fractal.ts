/**
 * L-system fractal generation helpers for creating complex cave survey test data.
 *
 * Implements the Fractal Plant L-system:
 *   Variables: X F
 *   Constants: + - [ ]
 *   Start: -X
 *   Rules: X -> F+[[X]-X]-F[-FX]+X, F -> FF
 *   Angle: 25 degrees
 */

// ============================================================================
// Type Definitions (local to avoid dependency on src/types)
// ============================================================================

export interface ShotData {
  azimuth_in: number;
  azimuth_out: number;
  length: number;
  depth_in: number;
  depth_out: number;
}

export interface LineData {
  name: string;
  color: string;
  shots: ShotData[];
}

export interface StationRef {
  line_id: number;
  station_index: number;
}

export interface MergeData {
  stationA: StationRef;
  stationB: StationRef;
}

export interface FractalResult {
  lines: LineData[];
  merges: MergeData[];
}

// ============================================================================
// Constants
// ============================================================================

const COLORS = [
  '#FF6B6B', '#4ECDC4', '#45B7D1', '#96CEB4',
  '#FFEAA7', '#DDA0DD', '#98D8C8', '#F7DC6F',
];

const L_SYSTEM_RULES: Record<string, string> = {
  'X': 'F+[[X]-X]-F[-FX]+X',
  'F': 'FF',
};

const AXIOM = '-X';
const ANGLE = 25; // degrees
const SHOT_LENGTH = 5.0; // meters
const SHOT_DEPTH = 5.0; // meters

// ============================================================================
// L-System Generation
// ============================================================================

/**
 * Generate L-system string by expanding rules for the given number of iterations.
 *
 * @param iterations - Number of times to apply the production rules
 * @returns The expanded L-system string
 */
export function generateLSystem(iterations: number): string {
  let result = AXIOM;

  for (let i = 0; i < iterations; i++) {
    result = result
      .split('')
      .map((c) => L_SYSTEM_RULES[c] || c)
      .join('');
  }

  return result;
}

// ============================================================================
// L-System Interpretation (Turtle Graphics)
// ============================================================================

interface TurtleState {
  lineId: number;
  stationIndex: number;
  azimuth: number;
}

/**
 * Execute L-system string and produce line/merge data.
 *
 * The turtle interprets the L-system:
 * - F: Draw forward (add a shot to current line)
 * - +: Turn left 25 degrees
 * - -: Turn right 25 degrees
 * - [: Push state (save position, start new line/branch)
 * - ]: Pop state (return to saved position)
 * - X: No drawing action (control variable)
 *
 * @param lSystem - The expanded L-system string
 * @returns Lines and merges to create in the backend
 */
export function executeLSystem(lSystem: string): FractalResult {
  const lines: LineData[] = [{ name: 'S0', color: COLORS[0], shots: [] }];
  const merges: MergeData[] = [];
  const stack: TurtleState[] = [];

  let currentLineId = 0;
  let currentAzimuth = 270; // Start pointing up (north in map coordinates)

  for (const char of lSystem) {
    switch (char) {
      case 'F': {
        // Draw forward: add shot to current line
        lines[currentLineId].shots.push({
          azimuth_in: currentAzimuth,
          azimuth_out: (currentAzimuth + 180) % 360,
          length: SHOT_LENGTH,
          depth_in: SHOT_DEPTH,
          depth_out: SHOT_DEPTH,
        });
        break;
      }

      case '+': {
        // Turn left (counterclockwise)
        currentAzimuth = (currentAzimuth - ANGLE + 360) % 360;
        break;
      }

      case '-': {
        // Turn right (clockwise)
        currentAzimuth = (currentAzimuth + ANGLE) % 360;
        break;
      }

      case '[': {
        // Push state: save current position and start a new branch
        const stationIndex = lines[currentLineId].shots.length;
        stack.push({
          lineId: currentLineId,
          stationIndex,
          azimuth: currentAzimuth,
        });

        // Create new line for the branch
        const newLineId = lines.length;
        lines.push({
          name: `S${newLineId}`,
          color: COLORS[newLineId % COLORS.length],
          shots: [],
        });

        // Record merge: new line's station 0 connects to current position
        merges.push({
          stationA: { line_id: newLineId, station_index: 0 },
          stationB: { line_id: currentLineId, station_index: stationIndex },
        });

        currentLineId = newLineId;
        break;
      }

      case ']': {
        // Pop state: return to saved position
        const saved = stack.pop();
        if (saved) {
          currentLineId = saved.lineId;
          currentAzimuth = saved.azimuth;
        }
        break;
      }

      // X and other characters: no action (control variables)
    }
  }

  // Filter out empty lines and remap merge indices
  return filterEmptyLines(lines, merges);
}

/**
 * Filter out empty lines and remap all merge references.
 * This handles the case where `[[` is followed by `]` without any `F`.
 */
function filterEmptyLines(
  lines: LineData[],
  merges: MergeData[]
): FractalResult {
  // Build mapping from old indices to new indices (for non-empty lines)
  const nonEmptyIndices: number[] = [];
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].shots.length > 0) {
      nonEmptyIndices.push(i);
    }
  }

  const indexMap = new Map<number, number>();
  nonEmptyIndices.forEach((oldIdx, newIdx) => {
    indexMap.set(oldIdx, newIdx);
  });

  // Filter lines
  const filteredLines = lines.filter((s) => s.shots.length > 0);

  // Assert S0 (root line) has shots - the L-system guarantees this
  if (filteredLines.length === 0 || !indexMap.has(0)) {
    throw new Error('L-system error: root line S0 has no shots');
  }

  // Filter and remap merges
  const filteredMerges = merges
    .filter((m) =>
      indexMap.has(m.stationA.line_id) && indexMap.has(m.stationB.line_id)
    )
    .map((m) => ({
      stationA: {
        line_id: indexMap.get(m.stationA.line_id)!,
        station_index: m.stationA.station_index,
      },
      stationB: {
        line_id: indexMap.get(m.stationB.line_id)!,
        station_index: m.stationB.station_index,
      },
    }));

  return { lines: filteredLines, merges: filteredMerges };
}

// ============================================================================
// Merge Verification Helpers
// ============================================================================

/**
 * Normalize a merge tuple to have the lower line_id first.
 * This allows comparing merges regardless of order (A->B vs B->A).
 *
 * @returns Normalized string key for set comparison
 */
export function normalizeMergeKey(
  lineA: number,
  stationA: number,
  lineB: number,
  stationB: number
): string {
  // Put lower line_id first for consistent comparison
  if (lineA < lineB || (lineA === lineB && stationA <= stationB)) {
    return `${lineA}-${stationA}-${lineB}-${stationB}`;
  }
  return `${lineB}-${stationB}-${lineA}-${stationA}`;
}

/**
 * Convert expected merges (using local indices) to a Set of normalized keys.
 * Used for comparing against actual backend merges.
 *
 * @param merges - Merges with local line indices
 * @param lineIdMap - Mapping from local index to backend line ID
 * @returns Set of normalized merge keys
 */
export function buildExpectedMergeSet(
  merges: MergeData[],
  lineIdMap: Map<number, number>
): Set<string> {
  const keys = new Set<string>();

  for (const merge of merges) {
    const actualA = lineIdMap.get(merge.stationA.line_id);
    const actualB = lineIdMap.get(merge.stationB.line_id);

    if (actualA !== undefined && actualB !== undefined) {
      const key = normalizeMergeKey(
        actualA,
        merge.stationA.station_index,
        actualB,
        merge.stationB.station_index
      );
      keys.add(key);
    }
  }

  return keys;
}

/**
 * Convert actual backend merges to a Set of normalized keys.
 *
 * @param stationMerges - Array of station merges from project.station_merges
 * @returns Set of normalized merge keys
 */
export function buildActualMergeSet(
  stationMerges: Array<{
    station_a: { line_id: number; station_index: number };
    station_b: { line_id: number; station_index: number };
  }>
): Set<string> {
  const keys = new Set<string>();

  for (const merge of stationMerges) {
    const key = normalizeMergeKey(
      merge.station_a.line_id,
      merge.station_a.station_index,
      merge.station_b.line_id,
      merge.station_b.station_index
    );
    keys.add(key);
  }

  return keys;
}

/**
 * Compare two merge sets and return differences.
 */
export function compareMergeSets(
  expected: Set<string>,
  actual: Set<string>
): { missing: string[]; extra: string[] } {
  const missing: string[] = [];
  const extra: string[] = [];

  for (const key of expected) {
    if (!actual.has(key)) {
      missing.push(key);
    }
  }

  for (const key of actual) {
    if (!expected.has(key)) {
      extra.push(key);
    }
  }

  return { missing, extra };
}
