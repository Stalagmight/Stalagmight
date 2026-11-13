import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { formatFixed, getNumberFormatLocale } from '../../i18n/format';
import { defaultConfig } from '../../types/config';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useGridLabels } from '../../hooks/useGridLabels';
import {
  hasAzimuthDiscrepancy,
  hasPitchDiscrepancy,
  validateEditableShot,
} from '../../utils/validation';
import { nextShotKind } from '../../utils/shotKind';
import { SpreadsheetGrid } from './SpreadsheetGrid';
import { ExpandIcon, ShrinkIcon } from './icons';
import {
  useEditSectionViewPersistence,
  useImportShotsViewPersistence,
} from '../Dialogs/useEditSectionViewPersistence';
import type { CellEdit } from './SpreadsheetGrid/types';
import {
  buildEditableShotColumns,
  EDITABLE_AZIMUTH_AVG_KEY,
  EDITABLE_PITCH_AVG_KEY,
} from './editableShotColumns';
import { ShotColumnsMenu } from './ShotColumnsMenu';
import { buildShotUpdates } from '../SingleSystemView/dataTableEdits';
import {
  ANGLE_FAMILY_KEYS,
  applyAngleMode,
  deriveAngleMode,
  normalizeAngleFamilies,
  reorderForAngleMode,
  type AzimuthMode,
} from '../Dialogs/newSectionRows';
import type { EditableShot } from './editableShot';
import { missingRequiredCells } from './editableShotStatus';
import { TOAST_KEYS } from '../../utils/toastKeys';

export type { EditableShot };

/**
 * The shared editable shot table, on the app's spreadsheet grid.
 *
 * Rendered by EditSectionDialog, ZeroDatumConfigDialog and (twice) the mNemo
 * import wizard. It used to be a hand-rolled `<table>` of `<input>`s with its
 * own draft/blur/Escape bookkeeping; that predated `SpreadsheetGrid` and carried
 * the defects that came with it - locale-naive `parseFloat` (a comma decimal
 * truncated), a controlled value re-formatted at display precision on every
 * keystroke, no clipboard, no keyboard cell navigation, no virtualization.
 *
 * The props are UNCHANGED, so its hosts did not have to move: it still takes
 * `shots`, hands `onShotChange` an index into the FULL array (soft-deleted rows
 * included), and keeps the add/insert/delete callbacks. Only the internals and
 * the per-cell test IDs differ.
 */
interface ValidationConfig {
  azimuth_diff_threshold: number;
  pitch_diff_threshold: number;
}

export interface ShotTableEditorProps {
  /** Array of editable shots to display */
  shots: EditableShot[];
  /** Callback when a shot field changes */
  onShotChange: (
    index: number,
    field: keyof EditableShot,
    value: number | string | boolean | null
  ) => void;
  /** Callback to add a new shot at the end */
  onAddShot: () => void;
  /** Append `count` blank shots in ONE update. Supply this and a paste that runs
   *  past the trailing blank arrives as a single call, which is the only way a
   *  host can append safely: called N times in one React event, a host that
   *  rebuilds its array from a closed-over snapshot overwrites its own earlier
   *  appends, and one that stamps rows from a captured temp id gives them all the
   *  same identity. Falls back to N calls of {@link onAddShot} when absent. */
  onAddShots?: (count: number) => void;
  /** Insert `count` blank shots at a full-array index, in ONE update. Same reason
   *  as {@link onAddShots}: called N times in one event, a host that rebuilds from
   *  a captured array or a captured temp id loses rows or duplicates identities.
   *  It is how a host services an insert at all - without it only the empty-table
   *  case, which is an append, has anywhere to go. */
  onInsertShots?: (index: number, count: number, position: 'before' | 'after') => void;
  /** Callback to delete a shot at the given index */
  onDeleteShot: (index: number) => void;
  /** Delete several shots at once (full-array indices, descending). Supply this
   *  and a multi-row delete arrives as ONE call, which is the only way a host can
   *  judge the selection as a whole - asked one shot at a time, each answer would
   *  treat the others as still present. Falls back to a loop over
   *  {@link onDeleteShot} when absent. */
  onDeleteShots?: (indices: number[]) => boolean | void;
  /** Validation configuration for warnings */
  validationConfig?: ValidationConfig;
  /** Whether the table is read-only */
  readOnly?: boolean;
  /** Maximum height for the table container (CSS value) */
  maxHeight?: string;
  /** Currently selected row index (into the FULL shots array) */
  selectedRowIndex?: number | null;
  /** Callback when a row is selected (index into the FULL shots array) */
  onRowSelect?: (index: number) => void;
  /** Set of shot indices whose geometry fields should be disabled (frozen shots) */
  frozenShotIndices?: Set<number>;
  /** The section these shots belong to. Supply it and the warning column shows
   *  the project's warnings for each shot, exactly as the data table does; the
   *  editor's own threshold checks are layered on top for edits the backend has
   *  not seen yet. Omit it for shots that are not in a section yet (the import
   *  wizard, zero-datum config), where there is nothing to look up. */
  sectionId?: number;
  /** Whether the host dialog is currently enlarged. Supply this together with
   *  {@link onToggleExpand} to get the Expand control; the HOST owns the size,
   *  because this editor is a child of three different dialogs and cannot
   *  resize any of them itself. */
  expanded?: boolean;
  onToggleExpand?: () => void;
  /** Which persisted layout this instance reads and writes. `edit-section` is
   *  the saved-survey review grid; `import` covers the wizard and zero-datum,
   *  which edit shots that are not in the project yet. Separate because sharing
   *  one slice meant reshaping columns mid-import rewrote the review layout. */
  layoutScope?: 'edit-section' | 'import';
}

const DEFAULT_VALIDATION_CONFIG: ValidationConfig = {
  azimuth_diff_threshold: defaultConfig.validation.azimuth_diff_threshold,
  pitch_diff_threshold: defaultConfig.validation.pitch_diff_threshold,
};

/** Fallback grid height before the first measurement, and under jsdom. */
const DEFAULT_GRID_HEIGHT = 260;
/** Never size the row area below this on a short window. */
const MIN_GRID_HEIGHT = 120;

/** Parse a CSS length like "16rem" / "320px" into px for the grid, which needs
 *  a number (virtualization cannot work from an indefinite height). */
function cssHeightToPx(value: string | undefined): number {
  if (!value) return DEFAULT_GRID_HEIGHT;
  const n = parseFloat(value);
  if (!Number.isFinite(n)) return DEFAULT_GRID_HEIGHT;
  if (value.endsWith('rem')) return Math.round(n * 16);
  return Math.round(n);
}

/** The editor's own live threshold checks, for edits the backend's analysis has
 *  not seen yet. `covered` names the warning codes the project already reports
 *  for this shot, so the tooltip does not say the same thing twice in two
 *  wordings - the backend's version names the shot and the measured value, so it
 *  is the one worth keeping. */
function warningsFor(
  shot: EditableShot,
  config: ValidationConfig,
  t: TFunction,
  covered: ReadonlySet<string> = new Set()
): string[] {
  const out: string[] = [];
  if (
    !covered.has('AZIMUTH_DEVIATION') &&
    hasAzimuthDiscrepancy(shot.azimuth_in, shot.azimuth_out, config.azimuth_diff_threshold)
  ) {
    out.push(
      t('shotTable.warnings.azimuthDiff', {
        threshold: `${formatFixed(config.azimuth_diff_threshold, undefined, 0)}°`,
      })
    );
  }
  if (
    !covered.has('PITCH_DEVIATION') &&
    hasPitchDiscrepancy(shot.pitch_in, shot.pitch_out, config.pitch_diff_threshold)
  ) {
    out.push(
      t('shotTable.warnings.pitchDiff', {
        threshold: `${formatFixed(config.pitch_diff_threshold, undefined, 0)}°`,
      })
    );
  }
  return out;
}

/** Angle legs for a row that exists on screen but not yet in `shots`. */
const UNSET_ANGLE_LEGS = {
  azimuth_in: NaN,
  azimuth_out: NaN,
  pitch_in: null,
  pitch_out: null,
} as const;

/** The blank trailing row. Rendered, never stored: `onAddShot` turns it into a
 *  real shot the first time a value lands in it. */
const PHANTOM_ROW: EditableShot = {
  id: Number.NEGATIVE_INFINITY,
  azimuth_in: NaN,
  azimuth_out: NaN,
  length: NaN,
  depth_in: NaN,
  depth_out: NaN,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  comment: null,
  kind: 'Normal',
};

/** The azimuth legs and the whole inclination family start hidden: the averages
 *  are what this editor has always shown. Everything else stays visible - it
 *  edits a survey that already exists, so there is no "start simple" case the
 *  way there is for a brand-new section.
 *
 *  ONLY reaches a config that has never carried `edit_section_hidden_columns`.
 *  That key shipped before these columns did, so an existing install has it
 *  stored as `[]` and the stored value wins - such a user gets the inclination
 *  AVERAGE column on upgrade (the legs are hidden anyway, by the repair in
 *  `normalizeAngleFamilies`). Deliberate: one new column is a smaller cost than
 *  a migration that cannot tell a stale setting from a deliberate choice.
 *
 *  Mirrors `default_edit_section_hidden_columns` in src-tauri/src/config/panels.rs. */
const DEFAULT_EDIT_SECTION_HIDDEN_COLUMNS: readonly string[] = [
  'azimuth_in',
  'azimuth_out',
  EDITABLE_PITCH_AVG_KEY,
  'pitch_in',
  'pitch_out',
];

export function ShotTableEditor({
  shots,
  onShotChange,
  onAddShot,
  onAddShots,
  onInsertShots,
  onDeleteShot,
  onDeleteShots,
  validationConfig,
  readOnly = false,
  maxHeight,
  selectedRowIndex,
  onRowSelect,
  frozenShotIndices,
  sectionId,
  layoutScope = 'import',
  expanded = false,
  onToggleExpand,
}: ShotTableEditorProps) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const gridLabels = useGridLabels('dataTable.shotNoun');
  const showToast = useUIStore((st) => st.showToast);
  const configValidation = useProjectStore((s) => s.config?.validation);
  // Per-project thresholds (#57). Between the host's prop and the global config
  // on purpose: a host that passes nothing - the import wizard, zero-datum -
  // must still warn by the OPEN PROJECT's rules, or its inline warnings disagree
  // with the Warnings panel and with the backend's own analysis of the same shots.
  const projectValidation = useProjectStore((s) => s.project?.validation);
  const projectWarnings = useProjectStore((s) => s.warnings);

  /** The project's unresolved warnings for one shot, split by severity. The data
   *  table's warning column reads exactly this; without it the editor showed only
   *  its own two threshold checks and the column sat empty on shots the data
   *  table flagged. A row added here has no backend warnings yet - it does not
   *  exist as far as the last analysis is concerned. */
  const backendWarningsFor = useCallback(
    (shot: EditableShot): { errors: string[]; warnings: string[]; codes: Set<string> } => {
      const empty = { errors: [], warnings: [], codes: new Set<string>() };
      if (sectionId === undefined || shot.isNew === true || shot.isDeleted === true) return empty;
      const errors: string[] = [];
      const warnings: string[] = [];
      const codes = new Set<string>();
      for (const w of projectWarnings) {
        if (w.sectionId !== sectionId || w.shotId !== shot.id || w.isResolved) continue;
        (w.severity === 'error' ? errors : warnings).push(w.message);
        codes.add(w.code);
      }
      return { errors, warnings, codes };
    },
    [projectWarnings, sectionId]
  );

  const effectiveValidation: ValidationConfig = {
    azimuth_diff_threshold:
      validationConfig?.azimuth_diff_threshold ??
      projectValidation?.azimuth_diff_threshold ??
      configValidation?.azimuth_diff_threshold ??
      DEFAULT_VALIDATION_CONFIG.azimuth_diff_threshold,
    pitch_diff_threshold:
      validationConfig?.pitch_diff_threshold ??
      projectValidation?.pitch_diff_threshold ??
      configValidation?.pitch_diff_threshold ??
      DEFAULT_VALIDATION_CONFIG.pitch_diff_threshold,
  };

  // Soft-deleted rows stay in `shots` so the host's indices never shift, but are
  // not shown. Everything below works in VISIBLE indices and translates on the
  // way out, because the callbacks are documented in full-array indices.
  const { visibleShots, actualIndexes } = useMemo(() => {
    const rows: EditableShot[] = [];
    const map: number[] = [];
    shots.forEach((shot, i) => {
      if (shot.isDeleted) return;
      rows.push(shot);
      map.push(i);
    });
    return { visibleShots: rows, actualIndexes: map };
  }, [shots]);

  const actualIndex = useCallback(
    (visibleIndex: number): number => actualIndexes[visibleIndex] ?? -1,
    [actualIndexes]
  );

  /** The always-present empty row at the end. Typing in it is what adds a shot -
   *  the same affordance the data table and the new-section dialog use, instead
   *  of a separate Add button. It is a render-only phantom: it becomes real via
   *  `onAddShot` the moment a value lands in it. */
  const isPhantomAt = useCallback(
    (visibleIndex: number): boolean => visibleIndex === visibleShots.length,
    [visibleShots.length]
  );
  const gridRows = useMemo<EditableShot[]>(
    () => [...visibleShots, { ...PHANTOM_ROW }],
    [visibleShots]
  );

  const errorsAt = useCallback(
    (visibleIndex: number): string[] => {
      const shot = visibleShots[visibleIndex];
      if (!shot) return []; // the trailing blank is an invitation, not an error
      const local = validateEditableShot(
        {
          length: shot.length,
          azimuth_in: shot.azimuth_in,
          azimuth_out: shot.azimuth_out,
          isDeleted: shot.isDeleted ?? false,
        },
        actualIndex(visibleIndex),
        0
      ).map((e) => e.message);
      // Deduped by message: a live check and the backend's analysis can name the
      // same problem, and the tooltip should say it once.
      return [...new Set([...local, ...backendWarningsFor(shot).errors])];
    },
    [visibleShots, actualIndex, backendWarningsFor]
  );

  const warningsAt = useCallback(
    (visibleIndex: number): string[] => {
      const shot = visibleShots[visibleIndex];
      if (!shot) return [];
      const backend = backendWarningsFor(shot);
      return [
        ...new Set([
          ...warningsFor(shot, effectiveValidation, t, backend.codes),
          ...backend.warnings,
        ]),
      ];
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps -- thresholds are primitives
    [visibleShots, effectiveValidation.azimuth_diff_threshold, effectiveValidation.pitch_diff_threshold, t, backendWarningsFor]
  );

  const isFrozenAt = useCallback(
    (visibleIndex: number): boolean => frozenShotIndices?.has(actualIndex(visibleIndex)) ?? false,
    [frozenShotIndices, actualIndex]
  );

  const onCycleKindAt = useCallback(
    (visibleIndex: number, direction: 1 | -1) => {
      if (readOnly) return;
      const index = actualIndex(visibleIndex);
      const shot = visibleShots[visibleIndex];
      if (index < 0 || !shot) return;
      onShotChange(index, 'kind', nextShotKind(shot.kind, direction));
    },
    [readOnly, actualIndex, visibleShots, onShotChange]
  );

  // --- Column layout, persisted under the dialog's own `edit_section_*` keys --
  const [columnOrder, setColumnOrder] = useState<string[]>([]);
  // Seeded from the default, not empty: the persisted config overrides this as
  // soon as it loads, but before it does the grid must not flash the five columns
  // that ship hidden.
  const [hiddenColumns, setHiddenColumns] = useState<ReadonlySet<string>>(
    () => new Set(DEFAULT_EDIT_SECTION_HIDDEN_COLUMNS)
  );
  const [columnWidths, setColumnWidths] = useState<Record<string, number>>({});
  const viewSetters = useMemo(() => ({ setColumnOrder, setHiddenColumns, setColumnWidths }), []);
  const usePersistence =
    layoutScope === 'edit-section' ? useEditSectionViewPersistence : useImportShotsViewPersistence;
  usePersistence(
    { columnOrder, hiddenColumns, columnWidths },
    viewSetters,
    DEFAULT_EDIT_SECTION_HIDDEN_COLUMNS
  );
  // What is ACTUALLY hidden on screen. A set stored before these columns existed
  // can leave an average AND its legs visible at once, so it is repaired on the
  // way out; the stored value stays the user's. Everything that asks "is this
  // column showing?" - the grid, the missing-cell tint, the eye menu's problem
  // marks - reads this one value, because two of them reading different sets is
  // how the tint ended up pointing at a column the grid was not drawing.
  const effectiveHidden = useMemo(() => normalizeAngleFamilies(hiddenColumns), [hiddenColumns]);
  const hiddenColumnList = useMemo(() => [...effectiveHidden], [effectiveHidden]);


  /** Which of avg / in-out / off an angle family is currently showing, read back
   *  out of the hidden set - no separate mode state to keep in sync. */
  const azimuthMode = useMemo(
    () => deriveAngleMode(hiddenColumns, EDITABLE_AZIMUTH_AVG_KEY, 'azimuth_in', 'azimuth_out'),
    [hiddenColumns]
  );
  const inclinationMode = useMemo(
    () => deriveAngleMode(hiddenColumns, EDITABLE_PITCH_AVG_KEY, 'pitch_in', 'pitch_out'),
    [hiddenColumns]
  );

  /** Show/hide a group of columns together (LRUD is one row, four keys). */
  const toggleCols = useCallback((keys: string[]) => {
    setHiddenColumns((prev) => {
      const next = new Set(prev);
      const allVisible = keys.every((k) => !next.has(k));
      for (const k of keys) {
        if (allVisible) next.add(k);
        else next.delete(k);
      }
      return next;
    });
  }, []);


  const columns = useMemo(
    () =>
      buildEditableShotColumns({
        t,
        precision,
        readOnly,
        errorsAt,
        warningsAt,
        isFrozenAt,
        onCycleKindAt,
        isPhantomAt,
      }),
    [
      t,
      precision,
      readOnly,
      errorsAt,
      warningsAt,
      isFrozenAt,
      onCycleKindAt,
      isPhantomAt,
    ]
  );

  const setAngleMode = useCallback(
    (family: 'azimuth' | 'pitch', mode: AzimuthMode) => {
      const avgKey = family === 'azimuth' ? EDITABLE_AZIMUTH_AVG_KEY : EDITABLE_PITCH_AVG_KEY;
      const legIn = `${family}_in`;
      const legOut = `${family}_out`;
      const current = deriveAngleMode(hiddenColumns, avgKey, legIn, legOut);
      if (current === mode) return;
      // The incoming columns take the slot the outgoing ones held, so the azimuth
      // stays put in the table whichever way it is shown. Off has no columns to
      // anchor to, so it leaves the order alone and the next switch back finds
      // the slot where it was.
      if (current !== 'off' && mode !== 'off') {
        const keys = ANGLE_FAMILY_KEYS[family === 'azimuth' ? 'azimuth' : 'inclination'];
        setColumnOrder((prev) =>
          reorderForAngleMode(prev, columns.map((c) => c.key), keys[current], keys[mode])
        );
      }
      setHiddenColumns((prev) => applyAngleMode(prev, mode, avgKey, legIn, legOut));
    },
    [hiddenColumns, columns]
  );

  /** One coerced cell value -> the host's field callback(s), at a resolved
   *  full-array index. */
  const applyEditAt = useCallback(
    (index: number, colKey: string, value: unknown) => {
      // The trailing blank has just been materialized by `resolveRowForEdit`, so
      // its index is one past the `shots` this render was given. Fall back to an
      // unset row rather than skipping the averaged-column branch, which would
      // write the VIRTUAL key ('_azimuth_avg') onto the shot as a real field and
      // drop the value - on the primary way of adding a shot.
      const shot = shots[index] ?? UNSET_ANGLE_LEGS;
      // An averaged column fans back out to its two legs. Routed through the data
      // table's `buildShotUpdates` rather than a local rule, so both grids offset
      // the legs by (new mean - old mean) and PRESERVE the spread between them.
      // The editor used to set both legs to the typed value, which was invisible
      // while the average was the only azimuth column on screen and is plainly
      // wrong now that the legs can be shown next to it.
      if (colKey === EDITABLE_AZIMUTH_AVG_KEY || colKey === EDITABLE_PITCH_AVG_KEY) {
        for (const [field, next] of Object.entries(buildShotUpdates(shot, [{ colKey, value }]))) {
          onShotChange(index, field as keyof EditableShot, next as number | string | null);
        }
        return;
      }
      onShotChange(index, colKey as keyof EditableShot, value as number | string | null);
    },
    [onShotChange, shots]
  );

  /** Resolve a grid row to a full-array index, materializing the trailing blank
   *  on the way if that is where the edit landed. `onAddShot` appends, so the
   *  new shot's index is the pre-append length - both hosts update `shots`
   *  functionally, so the append and the edit apply in order within one tick. */
  const resolveRowForEdit = useCallback(
    (visibleRow: number): number => {
      if (!isPhantomAt(visibleRow)) return actualIndex(visibleRow);
      onAddShot();
      return shots.length;
    },
    [isPhantomAt, actualIndex, onAddShot, shots.length]
  );

  const onEdit = useCallback(
    (rowIndex: number, colKey: string, value: unknown) => {
      if (readOnly) return;
      // The kind cell is a badge, not a value: on the trailing blank it renders
      // nothing to click and answers no Space, because a shot whose only content
      // is "Jump" is an incomplete row that blocks Save. Typing `j` reached the
      // grid's other keyboard route into a click-only column and materialized
      // exactly that row, so this closes it - the two deliberate refusals and
      // this one now say the same thing. Only the interactive path comes through
      // here; a paste carrying a kind still grows the table via onBatchEdit.
      if (colKey === 'kind' && isPhantomAt(rowIndex)) return;
      const index = resolveRowForEdit(rowIndex);
      if (index >= 0) applyEditAt(index, colKey, value);
    },
    [readOnly, isPhantomAt, resolveRowForEdit, applyEditAt]
  );

  /**
   * A required cell cleared back to empty. The grid marks it missing and writes
   * NOTHING (it will never put a 0 in a survey), so without this the host's shot
   * keeps its old value and a row emptied by the user still looks filled - and
   * still blocks Save. Writing NaN is how "no measurement" is spelled here.
   */
  const onClearMissing = useCallback(
    (rowIndex: number, colKey: string) => {
      if (readOnly || isPhantomAt(rowIndex)) return;
      const index = actualIndex(rowIndex);
      if (index < 0) return;
      if (colKey === EDITABLE_AZIMUTH_AVG_KEY) {
        onShotChange(index, 'azimuth_in', NaN);
        onShotChange(index, 'azimuth_out', NaN);
        return;
      }
      onShotChange(index, colKey as keyof EditableShot, NaN);
    },
    [readOnly, isPhantomAt, actualIndex, onShotChange]
  );

  const onBatchEdit = useCallback(
    (edits: CellEdit[]) => {
      if (readOnly) return;
      // Group by row so the trailing blank is materialized ONCE however many of
      // its cells a paste fills.
      const byRow = new Map<number, CellEdit[]>();
      for (const e of edits) {
        const arr = byRow.get(e.row);
        if (arr) arr.push(e);
        else byRow.set(e.row, [e]);
      }
      // Ascending, and counting the rows we have appended so far: a paste that
      // runs past the trailing blank needs ONE new shot per row, each at its own
      // index. `resolveRowForEdit` can only ever answer for the first of them -
      // it returns `shots.length`, which is the same index for every row in the
      // block - so a multi-row paste used to pile every row onto one shot.
      const rows = [...byRow.keys()].sort((a, b) => a - b);
      // Append ALL the new rows first, in one call where the host supports it -
      // see `onAddShots`. Only then hand out indices, so every appended row gets
      // its own slot and its own identity.
      const newRows = rows.filter((r) => isPhantomAt(r) || r >= visibleShots.length);
      if (newRows.length > 0) {
        if (onAddShots) onAddShots(newRows.length);
        else for (let n = 0; n < newRows.length; n++) onAddShot();
      }
      let appended = 0;
      for (const row of rows) {
        const rowEdits = byRow.get(row) ?? [];
        const isNewRow = isPhantomAt(row) || row >= visibleShots.length;
        const index = isNewRow ? shots.length + appended++ : actualIndex(row);
        if (index < 0) continue;
        for (const e of rowEdits) applyEditAt(index, e.colKey, e.value);
      }
    },
    [readOnly, applyEditAt, isPhantomAt, visibleShots.length, onAddShot, onAddShots, shots.length, actualIndex]
  );

  /** Report what a paste actually did, so an over-cap block cannot vanish
   *  quietly. Rows are grown to fit (below), so this normally only speaks when
   *  the grid's own ceiling is hit. */
  const onPasteReport = useCallback(
    ({ written, clipped, skipped }: { written: number; clipped: number; skipped: number }) => {
      const parts: string[] = [];
      if (written > 0) parts.push(t('dataTable.pasteReport.written', { count: written }));
      if (clipped > 0) parts.push(t('dataTable.pasteReport.clipped', { count: clipped }));
      if (skipped > 0) parts.push(t('dataTable.pasteReport.skipped', { count: skipped }));
      if (parts.length > 0) {
        const kind = clipped > 0 || skipped > 0 ? 'warning' : 'success';
        showToast(parts.join(' \u00b7 '), TOAST_KEYS.SHOT_EDITOR_PASTE, kind);
      }
    },
    [t, showToast]
  );

  /** Let a paste grow the table past the single trailing blank. Without this the
   *  grid clips every target at `rows.length`, so pasting a block of shots wrote
   *  the first row and silently dropped the rest. */
  const growRows = useMemo(() => ({ growRowTemplate: () => ({ ...PHANTOM_ROW }) }), []);

  // Insert: the grid reports an insertion BOUNDARY, which is "after the last
  // row" at the end and "before this row" everywhere else.
  const onInsertRows = useCallback(
    (beforeIndex: number, count = 1) => {
      if (readOnly) return;
      // The menu says "Insert N shots" and passes N. Dropping it inserted one,
      // so the label was a promise the table did not keep; the data table and
      // the new-section dialog have always honoured it.
      const n = Math.max(1, count);
      const last = visibleShots.length - 1;
      // An empty table has no row to insert relative to, so the only thing an
      // insert can mean there is an append. `onAddShots?.(n) ?? onAddShot()` ran
      // BOTH: a void return is `undefined`, which `??` reads as "nothing
      // happened", so an empty table got n + 1 rows. Branch on existence.
      if (last < 0) {
        if (onAddShots) onAddShots(n);
        else for (let i = 0; i < n; i++) onAddShot();
        return;
      }
      // Every host that services inserts hands over the batch callback - there
      // is no per-row path left to spend the request on.
      if (!onInsertShots) return;
      if (beforeIndex > last) onInsertShots(actualIndex(last), n, 'after');
      else onInsertShots(actualIndex(beforeIndex), n, 'before');
    },
    [readOnly, visibleShots.length, onAddShot, onAddShots, onInsertShots, actualIndex]
  );

  // Delete high-to-low so each removal cannot shift the indices still to come.
  const onDeleteRows = useCallback(
    (fromRow: number, toRow: number): boolean | void => {
      if (readOnly) return false;
      const indices: number[] = [];
      for (let r = fromRow; r <= Math.min(toRow, visibleShots.length - 1); r++) {
        const i = actualIndex(r);
        if (i >= 0) indices.push(i);
      }
      const descending = indices.sort((a, b) => b - a);
      // A host with the batch callback answers for the whole run; the one-at-a-
      // time fallback has no way to refuse, so it always counts as done - and it
      // is unsafe for a host that rebuilds from a captured array, which is why
      // every host in this repo supplies the batch form.
      if (onDeleteShots) return onDeleteShots(descending);
      for (const i of descending) onDeleteShot(i);
    },
    [readOnly, actualIndex, onDeleteShot, onDeleteShots, visibleShots.length]
  );

  /** Missing values whose column is HIDDEN, as the eye menu's row identities.
   *  Without this, turning an angle family OFF (a first-class menu choice) hides
   *  every column the mark could land on, so an incomplete row shows nothing at
   *  all - the blocker names the row but never the field. */
  const hiddenProblems = useMemo(() => {
    const problems = new Set<string>();
    const rowIdentity: Record<string, string> = {
      length: 'length',
      depth_in: 'depthIn',
      depth_out: 'depthOut',
      [EDITABLE_AZIMUTH_AVG_KEY]: 'azimuth',
      azimuth_in: 'azimuth',
      azimuth_out: 'azimuth',
    };
    for (const shot of visibleShots) {
      for (const key of missingRequiredCells(shot, EDITABLE_AZIMUTH_AVG_KEY, effectiveHidden)) {
        if (effectiveHidden.has(key) && rowIdentity[key]) problems.add(rowIdentity[key]);
      }
    }
    return problems;
  }, [visibleShots, effectiveHidden]);

  // Required-but-empty cells get the grid's red missing tint, so an incomplete
  // row points at the cells to fill rather than only naming the row.
  const cellClassName = useCallback(
    (shot: EditableShot, rowIndex: number, colKey: string): string | undefined => {
      if (isPhantomAt(rowIndex)) return undefined;
      return missingRequiredCells(shot, EDITABLE_AZIMUTH_AVG_KEY, effectiveHidden).has(colKey)
        ? 'sg-cell-missing'
        : undefined;
    },
    [isPhantomAt, effectiveHidden]
  );

  const rowClassName = useCallback(
    (shot: EditableShot, rowIndex: number): string | undefined => {
      // A virtual shot (jump/reference) is indicated by the J/R column, exactly
      // as in the data table and the new-section dialog. It is deliberately NOT
      // dimmed: the old table faded the whole row, which read as "disabled" on
      // a row that is perfectly editable.
      if (isPhantomAt(rowIndex)) return 'sg-row-draft';
      // Frozen BEFORE the error tint, and both when a row is both: the error
      // tint used to win outright, so a frozen row that also failed validation
      // lost the one signal that explained why half its cells would not accept
      // an edit. The error tint is a 5% wash and rides on top of the amber.
      if (isFrozenAt(rowIndex)) {
        return errorsAt(rowIndex).length > 0 ? 'sg-row-frozen bg-error-5' : 'sg-row-frozen';
      }
      if (errorsAt(rowIndex).length > 0) return 'bg-error-5';
      // Not-yet-saved rows borrow the grid's own uncommitted-row tint rather
      // than inventing a second one.
      if (shot.isNew) return 'sg-row-draft';
      return undefined;
    },
    [errorsAt, isPhantomAt, isFrozenAt]
  );

  // The host tracks selection in FULL-array indices; the grid works in visible
  // ones, so translate in both directions.
  const selectedVisibleRow = useMemo(() => {
    if (selectedRowIndex === null || selectedRowIndex === undefined) return null;
    const found = actualIndexes.indexOf(selectedRowIndex);
    return found === -1 ? null : found;
  }, [selectedRowIndex, actualIndexes]);

  // With an explicit `maxHeight` the host is dictating the size. Without one
  // (the expanded case) the box grows with the dialog, so it has to be measured
  // - virtualization needs a number, not an indefinite height. Guarded for
  // jsdom, which has no ResizeObserver, per the convention in ProfileChart/Dock.
  const gridBoxRef = useRef<HTMLDivElement | null>(null);
  const [measuredHeight, setMeasuredHeight] = useState(DEFAULT_GRID_HEIGHT);
  useEffect(() => {
    const el = gridBoxRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const measure = () => {
      const h = Math.max(MIN_GRID_HEIGHT, el.clientHeight);
      setMeasuredHeight((prev) => (Math.abs(prev - h) > 1 ? h : prev));
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);

  const onActiveCellChange = useCallback(
    (rowIndex: number) => {
      const index = actualIndex(rowIndex);
      if (index >= 0) onRowSelect?.(index);
    },
    [actualIndex, onRowSelect]
  );

  return (
    <div data-testid="shot-table-container" className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex shrink-0 items-center justify-end gap-3">
        {/* No section-id / shot-id rows: those columns are the data table's way
            of pointing at a shot across sections, and this grid edits one
            section's shots in place. The warnings column IS here, so it gets a
            toggle like the data table's. */}
        <ShotColumnsMenu
          hidden={hiddenColumns}
          onToggle={toggleCols}
          testIdPrefix="shot-editor-"
          buttonTestId="shot-editor-columns-menu"
          dropdownTestId="shot-editor-columns-dropdown"
          extraRows={{ warnings: true }}
          problems={hiddenProblems}
          angleModes={{
            azimuth: { mode: azimuthMode, onPick: (m) => setAngleMode('azimuth', m) },
            inclination: { mode: inclinationMode, onPick: (m) => setAngleMode('pitch', m) },
          }}
        />
        {onToggleExpand && (
          <button
            type="button"
            onClick={onToggleExpand}
            className="text-text-secondary hover:text-text"
            title={expanded ? t('newSection.collapse') : t('newSection.expand')}
            aria-label={expanded ? t('newSection.collapse') : t('newSection.expand')}
            data-testid="shot-editor-expand"
          >
            {expanded ? <ShrinkIcon size={14} /> : <ExpandIcon size={14} />}
          </button>
        )}
      </div>
      <div
        ref={gridBoxRef}
        className="min-h-0 flex-1 overflow-hidden rounded border border-border"
        data-testid="shot-table"
      >
        <SpreadsheetGrid<EditableShot>
          columns={columns}
          rows={readOnly ? visibleShots : gridRows}
          // Namespaced like EditSectionDialog's own shot key: temp ids and
          // imported ids are both small integers drawn from separate counters,
          // so a bare `tempId ?? id` collides the moment the mNemo wizard shows
          // a fresh row beside an imported one. React then reuses one row's DOM
          // for the other, and the grid's missing-cell marks - keyed off this
          // very value - paint on the twin.
          rowKey={(s, i) =>
            s.tempId != null ? `t:${s.tempId}` : s.id != null ? `i:${s.id}` : `x:${i}`
          }
          rowTestId={(_s, i) => `shot-row-${i}`}
          cellTestId={(_s, i, colKey) => `shot-cell-${i}-${colKey}`}
          testIdPrefix="shot-editor"
          locale={getNumberFormatLocale()}
          height={maxHeight === undefined ? measuredHeight : cssHeightToPx(maxHeight)}
          onEdit={onEdit}
          onBatchEdit={onBatchEdit}
          onClearMissing={onClearMissing}
          rowClassName={rowClassName}
          cellClassName={cellClassName}
          labels={gridLabels}
          selectedRow={selectedVisibleRow}
          onActiveCellChange={onActiveCellChange}
          hiddenColumns={hiddenColumnList}
          columnOrder={columnOrder}
          onColumnOrderChange={setColumnOrder}
          columnWidths={columnWidths}
          onColumnWidthsChange={setColumnWidths}
          onPasteReport={onPasteReport}
          {...(readOnly
            ? {}
            : {
                onInsertRows,
                onDeleteRows,
                pasteGrowsRows: growRows,
                // The trailing blank is rendered, never stored, so the row menu
                // must not count it: it used to say "3 shots" over a gutter drag
                // that ended on it, then insert three and delete two.
                trailingPlaceholderRows: 1,
              })}
        />
      </div>
    </div>
  );
}
