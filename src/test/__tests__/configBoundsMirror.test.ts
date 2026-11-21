import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { MIN_COL_WIDTH, MAX_COL_WIDTH } from '../../components/common/SpreadsheetGrid/useColumnWidths';
import { PANEL_SIZE_BOUNDS } from '../../components/SingleSystemView/layoutConstants';
import { MAX_DEPTH_REFERENCE_M } from '../../utils/gpsBounds';
import { MAP_PANE_MIN_WIDTH } from '../../components/DataWorkspace/store';
import { TINT_MODES } from '../../components/DataWorkspace/types';
import { MAP_PANE_MAX_WIDTH } from '../../components/DataWorkspace/useDataWorkspaceViewPersistence';

/**
 * Cross-language numeric-bounds mirror (issue #357 item 18).
 *
 * Two Rust/TS pairs are mirrored by hand with only a comment holding them
 * together:
 *
 *   - column widths: `MIN_COLUMN_WIDTH`/`MAX_COLUMN_WIDTH` in
 *     src-tauri/crates/core/src/config/column_widths.rs vs
 *     `MIN_COL_WIDTH`/`MAX_COL_WIDTH`
 *     in src/components/common/SpreadsheetGrid/useColumnWidths.ts.
 *   - zero-datum depth reference: `MAX_DEPTH_REFERENCE_M` in
 *     src-tauri/crates/models/src/station.rs vs the same name in
 *     src/utils/gpsBounds.ts.
 *   - panel sizes: the four `.clamp(a, b)` calls in
 *     `PanelsConfig::normalize` (src-tauri/crates/core/src/config/panels.rs) vs
 *     `PANEL_SIZE_BOUNDS` in src/components/SingleSystemView/layoutConstants.ts.
 *   - the Data workspace's map pane width: its `.clamp(a, b)` in the same
 *     `normalize` vs `MAP_PANE_MIN_WIDTH` (DataWorkspace/store.ts) and
 *     `MAP_PANE_MAX_WIDTH` (DataWorkspace/useDataWorkspaceViewPersistence.ts),
 *     and its tint modes, `DATA_WORKSPACE_TINT_MODES` vs `TINT_MODES`
 *     (DataWorkspace/types.ts) - a mode only one side knows is refused by the
 *     backend's partial door and repaired to the default on load.
 *
 * A one-sided bump gives a SILENT SNAP-BACK: the frontend accepts and saves a
 * value the Rust `normalize()` then clamps back down, and the subsequent
 * `loadConfig()` feeds the clamped value back to the UI with no error
 * anywhere. This test parses the Rust source and compares the numbers
 * against the TS constants (imported normally, not re-parsed) so drift fails
 * loudly here instead of silently in the running app.
 */

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, '../../../');
const COLUMN_WIDTHS_RS = path.join(REPO_ROOT, 'src-tauri/crates/core/src/config/column_widths.rs');
const PANELS_RS = path.join(REPO_ROOT, 'src-tauri/crates/core/src/config/panels.rs');
const STATION_RS = path.join(REPO_ROOT, 'src-tauri/crates/models/src/station.rs');

function parseRustConst(source: string, name: string, file: string): number {
  const re = new RegExp(`const ${name}:\\s*u32\\s*=\\s*(\\d+)\\s*;`);
  const match = re.exec(source);
  if (!match) {
    throw new Error(
      `Could not find \`const ${name}: u32 = N;\` in ${file}. ` +
        'If it was renamed, retyped, or reformatted, update this tripwire to match.',
    );
  }
  return Number(match[1]);
}

function parseRustClamp(source: string, field: string, file: string): { min: number; max: number } {
  const re = new RegExp(
    `self\\.${field}\\s*=\\s*self\\.${field}\\.clamp\\((\\d+),\\s*(\\d+)\\)`,
  );
  const match = re.exec(source);
  if (!match) {
    throw new Error(
      `Could not find \`self.${field} = self.${field}.clamp(MIN, MAX)\` in ${file}. ` +
        'If PanelsConfig::normalize was renamed or restructured, update this tripwire to match.',
    );
  }
  return { min: Number(match[1]), max: Number(match[2]) };
}

describe('cross-language numeric bounds mirror (#357 item 18)', () => {
  it('column width bounds agree: MIN/MAX_COLUMN_WIDTH (Rust) vs MIN/MAX_COL_WIDTH (TS)', () => {
    const rustSource = fs.readFileSync(COLUMN_WIDTHS_RS, 'utf8');
    const rustMin = parseRustConst(rustSource, 'MIN_COLUMN_WIDTH', 'column_widths.rs');
    const rustMax = parseRustConst(rustSource, 'MAX_COLUMN_WIDTH', 'column_widths.rs');

    expect(
      rustMin,
      `Column width MIN drift: Rust MIN_COLUMN_WIDTH=${rustMin} but TS MIN_COL_WIDTH=${MIN_COL_WIDTH}. ` +
        'A one-sided bump makes the backend silently clamp a value the frontend just accepted ' +
        '(silent snap-back on the next loadConfig()). Change both together.',
    ).toBe(MIN_COL_WIDTH);
    expect(
      rustMax,
      `Column width MAX drift: Rust MAX_COLUMN_WIDTH=${rustMax} but TS MAX_COL_WIDTH=${MAX_COL_WIDTH}. ` +
        'A one-sided bump makes the backend silently clamp a value the frontend just accepted ' +
        '(silent snap-back on the next loadConfig()). Change both together.',
    ).toBe(MAX_COL_WIDTH);
  });

  it('panel size bounds agree: PanelsConfig::normalize clamps (Rust) vs PANEL_SIZE_BOUNDS (TS)', () => {
    const rustSource = fs.readFileSync(PANELS_RS, 'utf8');
    const rustSections = parseRustClamp(rustSource, 'sections_panel_width', 'panels.rs');
    const rustDetail = parseRustClamp(rustSource, 'detail_panel_width', 'panels.rs');
    const rustDataTable = parseRustClamp(rustSource, 'data_table_height', 'panels.rs');
    const rustWizardMap = parseRustClamp(rustSource, 'wizard_map_panel_width', 'panels.rs');

    expect(
      rustSections,
      `sections_panel_width bounds drift: Rust clamp=${JSON.stringify(rustSections)} but TS ` +
        `PANEL_SIZE_BOUNDS.sections=${JSON.stringify(PANEL_SIZE_BOUNDS.sections)}. Change both together.`,
    ).toEqual(PANEL_SIZE_BOUNDS.sections);
    expect(
      rustDetail,
      `detail_panel_width bounds drift: Rust clamp=${JSON.stringify(rustDetail)} but TS ` +
        `PANEL_SIZE_BOUNDS.detail=${JSON.stringify(PANEL_SIZE_BOUNDS.detail)}. Change both together.`,
    ).toEqual(PANEL_SIZE_BOUNDS.detail);
    expect(
      rustDataTable,
      `data_table_height bounds drift: Rust clamp=${JSON.stringify(rustDataTable)} but TS ` +
        `PANEL_SIZE_BOUNDS['data-table']=${JSON.stringify(PANEL_SIZE_BOUNDS['data-table'])}. Change both together.`,
    ).toEqual(PANEL_SIZE_BOUNDS['data-table']);
    expect(
      rustWizardMap,
      `wizard_map_panel_width bounds drift: Rust clamp=${JSON.stringify(rustWizardMap)} but TS ` +
        `PANEL_SIZE_BOUNDS['wizard-map']=${JSON.stringify(PANEL_SIZE_BOUNDS['wizard-map'])}. Change both together.`,
    ).toEqual(PANEL_SIZE_BOUNDS['wizard-map']);
  });

  it('Data workspace map pane bounds agree: the Rust clamp vs MAP_PANE_MIN/MAX_WIDTH (TS)', () => {
    const rustSource = fs.readFileSync(PANELS_RS, 'utf8');
    const rust = parseRustClamp(rustSource, 'data_workspace_map_pane_width', 'panels.rs');
    expect(
      rust,
      `data_workspace_map_pane_width bounds drift: Rust clamp=${JSON.stringify(rust)} but TS ` +
        `min=${MAP_PANE_MIN_WIDTH} max=${MAP_PANE_MAX_WIDTH}. Change both together.`,
    ).toEqual({ min: MAP_PANE_MIN_WIDTH, max: MAP_PANE_MAX_WIDTH });
  });

  it('Data workspace tint modes agree: DATA_WORKSPACE_TINT_MODES (Rust) vs TINT_MODES (TS)', () => {
    const rustSource = fs.readFileSync(PANELS_RS, 'utf8');
    const match = /const DATA_WORKSPACE_TINT_MODES:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]*)\]/.exec(rustSource);
    if (!match) {
      throw new Error(
        'Could not find `const DATA_WORKSPACE_TINT_MODES: [&str; N] = [...]` in panels.rs. ' +
          'If it was renamed or reformatted, update this tripwire to match.',
      );
    }
    const rustModes = [...match[1]!.matchAll(/"([^"]*)"/g)].map((m) => m[1]);
    expect(
      rustModes,
      'Tint mode drift between Rust DATA_WORKSPACE_TINT_MODES and TS TINT_MODES. Change both together.',
    ).toEqual([...TINT_MODES]);
  });

  // #524 review: both sides checked only `isFinite`, so a 1e300 depth reference
  // was accepted end to end - through the zero-datum dialog, into the geometry,
  // and into every distance derived from it.
  it('depth-reference bound agrees: MAX_DEPTH_REFERENCE_M (Rust) vs the TS constant', () => {
    const rustSource = fs.readFileSync(STATION_RS, 'utf8');
    const match = /const MAX_DEPTH_REFERENCE_M:\s*f64\s*=\s*([\d_]+(?:\.\d+)?)\s*;/.exec(
      rustSource,
    );
    if (!match) {
      throw new Error(
        'Could not find `const MAX_DEPTH_REFERENCE_M: f64 = N;` in station.rs. ' +
          'If it was renamed or reformatted, update this tripwire to match.',
      );
    }
    const rustMax = Number(match[1]!.replace(/_/g, ''));

    expect(
      rustMax,
      `Depth-reference bound drift: Rust MAX_DEPTH_REFERENCE_M=${rustMax} but TS ` +
        `MAX_DEPTH_REFERENCE_M=${MAX_DEPTH_REFERENCE_M}. A one-sided bump makes the dialog ` +
        'accept a value the backend then refuses, so Confirm fails at the far end. ' +
        'Change both together.',
    ).toBe(MAX_DEPTH_REFERENCE_M);
  });
});
