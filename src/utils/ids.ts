// Shared stride for encoding line-scoped IDs into globally unique IDs.
// Keep in sync with the backend FEATURE_ID_STRIDE.
export const FEATURE_ID_STRIDE = 100000;

/**
 * Encode a line-scoped id (a shot id or a station index) into the globally
 * unique MapLibre feature id used as `promoteId: 'featureId'`. The ONE canonical
 * formula - used by both feature construction (`surveyLayer/features.ts`) and any
 * id-keyed lookup against those features, so the two never drift.
 */
export function encodeFeatureId(lineId: number, scopedId: number): number {
  return lineId * FEATURE_ID_STRIDE + scopedId;
}

/**
 * Line id out of a `"lineId:scopedId"` feature key (NaN if malformed).
 *
 * This is the ONLY supported way to recover a line id. Do NOT invert
 * {@link encodeFeatureId} with `Math.floor(fid / FEATURE_ID_STRIDE)`: that is
 * lossless only when the scoped id is line-local, which holds for a station
 * index but NOT for a shot id. The backend seeds `next_shot_id = line.id *
 * FEATURE_ID_STRIDE` (`src-tauri/src/models/line.rs`), so a shot id is
 * already line-encoded and its feature id is `2 * lineId * STRIDE + seq`
 * - the floor divide yields `2 * lineId` for every shot (#301, and #26
 * before it for the measured-path tint).
 */
export function lineIdFromKey(key: string): number {
  const head = key.split(':')[0];
  // Digits only, so a malformed head fails closed to NaN instead of coercing to a
  // real line: `Number('')` and `Number(' ')` are both 0, and line 0 exists
  // (`next_line_id` starts at 0), so coercion would mean the WRONG line.
  return head !== undefined && /^\d+$/.test(head) ? Number(head) : NaN;
}
