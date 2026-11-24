import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { COMBINE_SIDE_BY_SIDE_M } from '../../utils/combineLines';

/**
 * Combine Lines side-by-side tolerance tripwire (#740).
 *
 * Two line ends within `COMBINE_TOLERANCE_M` (Rust, `combine_candidate.rs`)
 * are "side by side here", and a join moves a side by their residual. The
 * dialog lists such a move only when it tears a tie, deciding "within the
 * tolerance" with its own copy, `COMBINE_SIDE_BY_SIDE_M`
 * (`src/utils/combineLines.ts`). Nothing generated ties the two, so a
 * change to one alone would make the dialog list or hide moves at the wrong
 * size without any error. This pins that they AGREE, not what they are.
 */

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, '../../../');
const RUST_PATH = path.join(REPO_ROOT, 'src-tauri/crates/models/src/line_combine.rs');

describe('combine side-by-side tolerance', () => {
  it('matches the backend constant', () => {
    const source = fs.readFileSync(RUST_PATH, 'utf8');
    const match = /pub const COMBINE_TOLERANCE_M:\s*f64\s*=\s*([0-9.eE+-]+)\s*;/.exec(source);
    if (!match) {
      throw new Error(
        'Could not find `pub const COMBINE_TOLERANCE_M: f64 = ...;` in combine_candidate.rs. ' +
          'If it was renamed or moved, update this tripwire to match.'
      );
    }
    expect(COMBINE_SIDE_BY_SIDE_M).toBe(Number(match[1]));
  });
});
