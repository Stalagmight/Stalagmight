import { useState, useRef, useCallback, useMemo, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore, type PlacementResult } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { Button } from '../common/Button';
import { ExpandIcon, ShrinkIcon } from '../common/icons';
import { DialogWrapper } from './DialogWrapper';
import type { Shot, SectionData } from '../../types/project';
import { SpreadsheetGrid } from '../common/SpreadsheetGrid';
import { useBulkEditReportToast } from '../common/useBulkEditReportToast';
import type { CellEdit } from '../common/SpreadsheetGrid/types';
import {
  type ShotDraft,
  materializeDraft,
  draftMissing,
  isBlankDraft,
} from '../SingleSystemView/dataTableDrafts';
import {
  ANGLE_FAMILY_KEYS,
  applyAngleMode,
  applyEdits,
  type AzimuthMode,
  blankRow,
  canSubmit,
  collectShots,
  countEditsPastCap,
  deleteRows,
  deriveAngleMode,
  deriveAzimuthMode,
  insertBlankRows,
  moveRows,
  normalizeAngleFamilies,
  reorderForAngleMode,
  summarizeSubmit,
} from './newSectionRows';
import {
  AZIMUTH_AVG_KEY,
  buildNewSectionShotColumns,
  DEFAULT_NEW_SECTION_HIDDEN_COLUMNS,
  MISSING_PROBLEM_COLUMNS,
  PITCH_AVG_KEY,
} from './newSectionShotColumns';
import { useNewSectionViewPersistence } from './useNewSectionViewPersistence';
import { getNextSectionColor } from '../../utils/colors';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useGridLabels } from '../../hooks/useGridLabels';
import { ShotColumnsMenu } from '../common/ShotColumnsMenu';
import { getNumberFormatLocale } from '../../i18n/format';
import { nextShotKind } from '../../utils/shotKind';
import { trackDialogComplete } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useShallow } from 'zustand/react/shallow';

/** Grid height to assume before the first measurement, and under jsdom (which
 *  has no ResizeObserver). Tall enough that virtualization renders rows. */
const FALLBACK_GRID_HEIGHT = 360;
/** Never size the row area below this, so the grid stays usable on a short
 *  window rather than collapsing to the header. */
const ROW_AREA_MIN = 120;

interface SavedFormState {
  name: string;
  surveyor: string;
  date: string;
  color: string;
  /** The full grid contents, including partially-filled rows, so a cancelled
   *  placement restores exactly what was on screen. */
  drafts: ShotDraft[];
  /** Continues the monotonic key counter, so restored rows can never collide
   *  with rows added afterwards. */
  nextKey: number;
}

export default function ImportDialog() {
  const { t } = useTranslation();
  const { closeDialog, openDialog, startPlacementMode, dialogData, resolvedTheme, showToast } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      openDialog: s.openDialog,
      startPlacementMode: s.startPlacementMode,
      dialogData: s.dialogData,
      resolvedTheme: s.resolvedTheme,
      showToast: s.showToast,
    })),
  );
  const { createSection, createSectionAndMerge, moveSection, project } = useProjectStore(
    useShallow((s) => ({
      createSection: s.createSection,
      createSectionAndMerge: s.createSectionAndMerge,
      moveSection: s.moveSection,
      project: s.project,
    })),
  );
  const precision = useDecimalPrecision();
  const editingBlocked = useEditingBlocked();
  const gridLabels = useGridLabels('dataTable.shotNoun');
  const numberLocale = getNumberFormatLocale();

  // Restore form state when reopening after placement cancel
  const saved = dialogData as { savedFormState?: SavedFormState } | undefined;
  const restored = saved?.savedFormState;

  // Form state
  const [name, setName] = useState<string>(restored?.name ?? '');
  const [surveyor, setSurveyor] = useState<string>(restored?.surveyor ?? '');
  const [date, setDate] = useState<string>(restored?.date ?? '');

  // Row keys are handed out from a monotonic counter and NEVER reused: the grid
  // keys per-cell state (missing-cell marks) by row identity, so a recycled key
  // would leak one row's red cells onto a later one.
  const keyCounterRef = useRef<number>(restored?.nextKey ?? 0);
  const nextKey = useCallback(() => keyCounterRef.current++, []);
  const [drafts, setDrafts] = useState<ShotDraft[]>(
    () => restored?.drafts ?? [blankRow(keyCounterRef.current++)]
  );

  // Get next available color
  const usedColors = project?.sections.map((s) => s.color) ?? [];
  const [color, setColor] = useState<string>(
    restored?.color ?? getNextSectionColor(usedColors, resolvedTheme)
  );

  // --- Grid layout (persisted under its own config keys) --------------------
  const [columnOrder, setColumnOrder] = useState<string[]>([]);
  const [hiddenColumns, setHiddenColumns] = useState<ReadonlySet<string>>(
    () => new Set(DEFAULT_NEW_SECTION_HIDDEN_COLUMNS)
  );
  const [columnWidths, setColumnWidths] = useState<Record<string, number>>({});
  const viewSetters = useMemo(
    () => ({ setColumnOrder, setHiddenColumns, setColumnWidths }),
    []
  );
  useNewSectionViewPersistence(
    { columnOrder, hiddenColumns, columnWidths },
    viewSetters,
    DEFAULT_NEW_SECTION_HIDDEN_COLUMNS
  );
  // Normalized on the way to the grid: a set stored before these columns existed
  // can leave an average AND its legs visible at once. The stored value stays the
  // user's; only what renders is repaired.
  const hiddenColumnList = useMemo(
    () => [...normalizeAngleFamilies(hiddenColumns)],
    [hiddenColumns]
  );

  // --- Chrome: column-visibility menu + expand ------------------------------
  // The dialog draws these itself (the grid's built-in toolbar is off), so the
  // column control is the same eye menu the data table uses rather than a second
  // idiom for the same job.
  const [expanded, setExpanded] = useState(false);

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



  // --- Grid height ----------------------------------------------------------
  // Virtualization needs an explicit pixel height, and the box is either a fixed
  // 300px or (expanded) whatever the window leaves, so it has to be measured.
  // The box holds nothing but the grid - the chrome row is its sibling.
  const gridBoxRef = useRef<HTMLDivElement | null>(null);
  const [gridHeight, setGridHeight] = useState(FALLBACK_GRID_HEIGHT);
  useEffect(() => {
    const el = gridBoxRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const measure = () => {
      const h = Math.max(ROW_AREA_MIN, el.clientHeight);
      setGridHeight((prev) => (Math.abs(prev - h) > 1 ? h : prev));
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);

  // --- Rows -----------------------------------------------------------------
  // Materialized per DRAFT, not per render: the row mutators hand back the same
  // draft object for every row an edit did not touch, so one keystroke rebuilds
  // one row instead of all of them - this grid takes 5000-row pastes.
  const rowCache = useMemo(() => new WeakMap<ShotDraft, Shot>(), []);
  const rows = useMemo<Shot[]>(
    () =>
      drafts.map((d) => {
        const cached = rowCache.get(d);
        if (cached) return cached;
        const row = materializeDraft(d);
        rowCache.set(d, row);
        return row;
      }),
    [drafts, rowCache]
  );

  // One funnel for every edit path, so the row cap has a single place to report
  // from: `applyEdits` refuses edits that would grow the list past MAX_PASTE_ROWS
  // and says how many, which would otherwise vanish without a word. It is also
  // where the structural invariants live, so nothing may write `drafts` around
  // it (see onCycleShotKind).
  const applyGridEdits = useCallback(
    (edits: CellEdit[]) => {
      if (editingBlocked) return;
      // Decide the capped-paste toast BEFORE scheduling state. Reading the drop
      // count back out of the updater used to lose it: the updater can run later
      // than this function returns (so the read saw a stale 0 and the toast
      // vanished) or twice under StrictMode (which would double it). The cap is
      // only ever reached by a paste, which arrives as one batch against the
      // rows on screen, so `drafts.length` here is the same count `applyEdits`
      // would see - no updater `prev` needed.
      const droppedPastCap = countEditsPastCap(drafts.length, edits);
      // Still functional: the grid calls this once PER cleared cell when several
      // required cells are emptied at once, and each call computing from the same
      // captured `drafts` meant only the last one survived - while the grid
      // showed them all cleared, so Create could go green with a measurement the
      // user believed they had removed.
      setDrafts((current) => applyEdits(current, edits, nextKey).rows);
      if (droppedPastCap > 0) {
        showToast(
          t('newSection.pasteCapped', { count: droppedPastCap }),
          TOAST_KEYS.NEW_SECTION_PASTE_CAP,
          'error'
        );
      }
    },
    [editingBlocked, drafts, nextKey, showToast, t]
  );

  // The J/R cell goes through that same funnel, addressing its row by grid
  // position like every other edit. Writing `kind` straight into the drafts
  // array skipped `normalizeRows`, so cycling the kind of the trailing blank row
  // consumed the invitation row - no fresh blank below it, nowhere left to type,
  // and Create disabled over a row the user had only tagged.
  const onCycleShotKind = useCallback(
    (shot: Shot, direction: 1 | -1, rowIndex: number) => {
      applyGridEdits([
        { row: rowIndex, colKey: 'kind', value: nextShotKind(shot.kind, direction) },
      ]);
    },
    [applyGridEdits]
  );

  const columns = useMemo(
    () =>
      buildNewSectionShotColumns({
        t,
        precision,
        readOnly: editingBlocked,
        onCycleShotKind,
      }),
    [t, precision, editingBlocked, onCycleShotKind]
  );

  const setAngleMode = useCallback(
    (family: 'azimuth' | 'pitch', mode: AzimuthMode) => {
      const avgKey = family === 'azimuth' ? AZIMUTH_AVG_KEY : PITCH_AVG_KEY;
      const legIn = `${family}_in`;
      const legOut = `${family}_out`;
      const current = deriveAngleMode(hiddenColumns, avgKey, legIn, legOut);
      if (current === mode) return;
      // Incoming columns take the outgoing ones' slot - see reorderForAngleMode.
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

  const onEdit = useCallback(
    (rowIndex: number, colKey: string, value: unknown) =>
      applyGridEdits([{ row: rowIndex, colKey, value }]),
    [applyGridEdits]
  );

  const onBatchEdit = applyGridEdits;

  // A required cell cleared back to "missing": drop the field so it reads as
  // absent to draftMissing/completeDraftData rather than keeping a stale value.
  const onClearMissing = useCallback(
    (rowIndex: number, colKey: string) =>
      applyGridEdits([{ row: rowIndex, colKey, value: undefined }]),
    [applyGridEdits]
  );

  // What the grid could not fully apply (columns clipped past the last one,
  // cells rejected as read-only or out of range) - the same report the data
  // table surfaces, rather than a paste that silently lands short.
  const onPasteReport = useBulkEditReportToast(TOAST_KEYS.NEW_SECTION_PASTE);

  const onInsertRows = useCallback(
    (beforeIndex: number, count: number) => {
      if (editingBlocked) return;
      setDrafts((prev) => insertBlankRows(prev, beforeIndex, count, nextKey));
    },
    [editingBlocked, nextKey]
  );

  const onDeleteRows = useCallback(
    (fromRow: number, toRow: number) => {
      if (editingBlocked) return;
      setDrafts((prev) => deleteRows(prev, fromRow, toRow, nextKey));
    },
    [editingBlocked, nextKey]
  );

  const onMoveRows = useCallback(
    (fromStart: number, fromEnd: number, toIndex: number) => {
      if (editingBlocked) return;
      setDrafts((prev) => moveRows(prev, fromStart, fromEnd, toIndex, nextKey));
    },
    [editingBlocked, nextKey]
  );

  // --- Missing-cell marking and the Create gate -----------------------------
  const azimuthMode = useMemo(() => deriveAzimuthMode(hiddenColumns), [hiddenColumns]);
  const inclinationMode = useMemo(
    () => deriveAngleMode(hiddenColumns, PITCH_AVG_KEY, 'pitch_in', 'pitch_out'),
    [hiddenColumns]
  );
  // Validated per DRAFT like `rows` above, so a keystroke re-checks the row it
  // changed rather than the whole grid. The cache is thrown away whenever the
  // column layout it was computed against changes.
  const missingCache = useMemo(
    () => new WeakMap<ShotDraft, ReturnType<typeof draftMissing>>(),
    [hiddenColumns, azimuthMode]
  );
  const missingByRow = useMemo(
    () =>
      drafts.map((d) => {
        const cached = missingCache.get(d);
        if (cached) return cached;
        const missing = draftMissing(d, hiddenColumns, azimuthMode);
        missingCache.set(d, missing);
        return missing;
      }),
    [drafts, missingCache, hiddenColumns, azimuthMode]
  );
  const cellClassName = useCallback(
    (_row: Shot, rowIndex: number, colKey: string) =>
      missingByRow[rowIndex]?.cells.has(colKey) ? 'sg-cell-missing' : undefined,
    [missingByRow]
  );

  // A required field whose column is hidden would block Create with nothing on
  // screen to point at: the blocker names the row, but the cell that would go
  // red is off-screen and there is no way to find it. `draftMissing` reports
  // those separately from the visible ones precisely so a host can act, so pull
  // the column back into view. This is a real unhide and persists like any
  // other - a temporary one would be undone by the layout restore on reopen,
  // putting the user straight back into the dead end.
  useEffect(() => {
    const reveal = new Set<string>();
    for (const { hiddenProblems } of missingByRow) {
      for (const problem of hiddenProblems) {
        for (const key of MISSING_PROBLEM_COLUMNS[problem] ?? []) {
          if (hiddenColumns.has(key)) reveal.add(key);
        }
      }
    }
    if (reveal.size === 0) return;
    setHiddenColumns((prev) => {
      const next = new Set(prev);
      for (const key of reveal) next.delete(key);
      return next;
    });
  }, [missingByRow, hiddenColumns]);

  // One blank row for the grid to parse and validate pasted-in rows against,
  // before this host has created them. Only read, never written.
  const growRows = useMemo(
    () => ({ growRowTemplate: () => materializeDraft(blankRow(-1)) }),
    []
  );

  const submit = useMemo(() => summarizeSubmit(drafts), [drafts]);
  const submitAllowed = canSubmit(submit) && !editingBlocked;

  const handleSubmit = () => {
    if (!submitAllowed) return;
    const sectionData: SectionData = {
      name: name || null,
      surveyor: surveyor || null,
      date: date || null,
      color,
      direction: null,
      quality: 1.0,
      shots: collectShots(drafts),
    };

    // Save form state so we can restore on cancel
    const formState: SavedFormState = {
      name,
      surveyor,
      date,
      color,
      drafts,
      nextKey: keyCounterRef.current,
    };

    // Close dialog and enter placement mode
    trackDialogComplete('import');
    closeDialog();
    startPlacementMode(
      sectionData,
      0, // align station 0 to cursor
      // onComplete: create section, merge if snapped or move to position
      async (result: PlacementResult) => {
        if (result.snappedToStation) {
          await createSectionAndMerge(sectionData, 0, result.snappedToStation, 'Move');
        } else {
          const cmdResult = await createSection(sectionData);
          if (cmdResult.success && cmdResult.section_id != null) {
            await moveSection(cmdResult.section_id, result.position);
          }
        }
      },
      // onCancel: reopen dialog with saved form state
      () => {
        openDialog('import', { savedFormState: formState });
      },
    );
  };

  // Track if form has been modified. Any row carrying data counts, including an
  // incomplete one - discarding it still loses typing. Judged by value, the same
  // rule the grid and the Create gate use: a cleared optional leaves its key
  // behind, so counting keys offered to discard an empty grid.
  const isDirty =
    name !== '' || surveyor !== '' || date !== '' || drafts.some((d) => !isBlankDraft(d));

  return (
    <DialogWrapper
      title={t('newSection.title')}
      onClose={closeDialog}
      size={expanded ? 'fullscreen' : '2xl'}
      overflow={expanded ? 'hidden' : 'auto'}
      isDirty={isDirty}
      data-testid="import-section-dialog"
    >
      <div className={`flex flex-col gap-4 ${expanded ? 'h-full' : ''}`}>
        {/* Metadata */}
        <div className="grid shrink-0 grid-cols-2 gap-4">
          <div>
            <label className="block text-sm text-text-secondary mb-1">
              {t('newSection.labels.name')}
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full"
              placeholder={t('newSection.placeholders.name')}
              data-testid="section-name-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">
              {t('newSection.labels.surveyor')}
            </label>
            <input
              type="text"
              value={surveyor}
              onChange={(e) => setSurveyor(e.target.value)}
              className="w-full"
              data-testid="import-section-surveyor-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">
              {t('newSection.labels.date')}
            </label>
            <input
              type="date"
              value={date}
              onChange={(e) => setDate(e.target.value)}
              className="w-full"
              data-testid="import-section-date-input"
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">
              {t('newSection.labels.color')}
            </label>
            <input
              type="color"
              value={color}
              onChange={(e) => setColor(e.target.value)}
              className="w-full h-9"
              data-testid="import-section-color-input"
            />
          </div>
        </div>

        {/* Shots grid, under the dialog's own chrome rather than the grid's
            built-in toolbar - so the column control matches the data table's eye
            menu, and Expand can sit beside it. */}
        <div
          className={`flex flex-col overflow-hidden rounded border border-border ${
            expanded ? 'min-h-0 flex-1' : ''
          }`}
          data-testid="import-section-grid"
        >
          <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border px-2 py-1">
            <span className="text-sm font-medium">{t('newSection.shots')}</span>
            <div className="flex items-center gap-2">
              {/* No warnings / section-id / shot-id rows: this section does not
                  exist yet, so the grid has none of those columns to toggle. */}
              <ShotColumnsMenu
                hidden={hiddenColumns}
                onToggle={toggleCols}
                testIdPrefix="new-section-"
                buttonTestId="new-section-columns-menu"
                dropdownTestId="new-section-columns-dropdown"
                angleModes={{
                  azimuth: { mode: azimuthMode, onPick: (m) => setAngleMode('azimuth', m) },
                  inclination: {
                    mode: inclinationMode,
                    onPick: (m) => setAngleMode('pitch', m),
                  },
                }}
              />
              <button
                type="button"
                onClick={() => setExpanded((v) => !v)}
                className="text-text-secondary hover:text-text"
                title={expanded ? t('newSection.collapse') : t('newSection.expand')}
                aria-label={expanded ? t('newSection.collapse') : t('newSection.expand')}
                data-testid="import-section-expand"
              >
                {expanded ? <ShrinkIcon size={14} /> : <ExpandIcon size={14} />}
              </button>
            </div>
          </div>
          <div
            ref={gridBoxRef}
            className={expanded ? 'min-h-0 flex-1 overflow-hidden' : 'h-[300px] overflow-hidden'}
          >
          <SpreadsheetGrid<Shot>
            columns={columns}
            rows={rows}
            rowKey={(r) => r.id}
            locale={numberLocale}
            height={gridHeight}
            onEdit={onEdit}
            onBatchEdit={onBatchEdit}
            onClearMissing={onClearMissing}
            onPasteReport={onPasteReport}
            cellClassName={cellClassName}
            labels={gridLabels}
            testIdPrefix="new-section"
            hiddenColumns={hiddenColumnList}
            columnOrder={columnOrder}
            onColumnOrderChange={setColumnOrder}
            columnWidths={columnWidths}
            onColumnWidthsChange={setColumnWidths}
            // Every row here is provisional, so a paste may run past the last
            // one: applyEdits grows the list to fit rather than clipping a
            // spreadsheet block to the single blank row the dialog opens with.
            // The template is what those not-yet-created rows are parsed and
            // validated against, so a pasted block obeys the same column rules
            // wherever in it a given line happens to fall.
            //
            // All the row affordances are withheld together when editing is
            // blocked: offering a Delete entry or a gutter drag that silently
            // does nothing is worse than not offering it at all.
            {...(editingBlocked
              ? {}
              : { pasteGrowsRows: growRows, onInsertRows, onDeleteRows, onMoveRows })}
          />
          </div>
        </div>

        {/* Actions */}
        <div className="flex shrink-0 items-center justify-between gap-4 border-t border-border pt-4">
          <p className="text-sm text-error" data-testid="import-section-blocker">
            {editingBlocked
              ? t('newSection.readOnly')
              : submit.incompleteRows.length > 0
                ? t('newSection.incomplete', {
                    count: submit.incompleteRows.length,
                    rows: submit.incompleteRows.join(', '),
                  })
                : submit.completeCount === 0
                  ? t('newSection.empty')
                  : ''}
          </p>
          <div className="flex gap-2">
            <Button onClick={closeDialog} data-testid="import-section-cancel">
              {t('common.actions.cancel')}
            </Button>
            <Button
              variant="primary"
              onClick={handleSubmit}
              disabled={!submitAllowed}
              data-testid="import-section-create"
            >
              {t('newSection.create')}
            </Button>
          </div>
        </div>
      </div>
    </DialogWrapper>
  );
}
