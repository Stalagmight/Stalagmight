import { useState, useEffect, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { classifyDeleteRun, isEndRun } from './editLineDeletes';
import { useProjectStore } from '../../store/projectStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { getThemeColor } from '../../utils/themeColors';
import { ShotTableEditor, type EditableShot } from '../common/ShotTableEditor';
import { blankEditableShot } from '../common/editableShot';
import { validateEditableShot } from '../../utils/validation';
import { isBlankNewShot, isShotComplete } from '../common/editableShotStatus';
import { EDITABLE_AZIMUTH_AVG_KEY } from '../common/editableShotColumns';
import { LineMetadataForm } from '../common/LineMetadataForm';
import type { Line, Shot, ShotData, ShotId } from '../../types/project';
import type { DeleteShotImpact } from '../../types/commands';
import { defaultConfig } from '../../types/config';
import DeleteShotImpactDetails from './DeleteShotImpactDetails';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useLineCreditDraft } from './useLineCreditDraft';

export default function EditLineDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, openDialog, showToast } = useUIStore();
  const { updateLine, createShot, updateShot, deleteShot, editShotCredit, executeBatch, project, isShotFrozen } = useProjectStore();

  const lineId = dialogData.lineId as number | undefined;
  const line = project?.lines.find((s) => s.id === lineId);


  // Reverse (#24) mutates the survey, and the closure-adjusted / Plan / Export
  // read-only mode has no server backstop (see useEditingBlocked's
  // backstop-asymmetry note), so its entry point has to be gated here too.
  const editingBlocked = useEditingBlocked();

  const [isLoading, setIsLoading] = useState(false);
  // Expand grows the DIALOG, not just the grid - same as the new-line dialog,
  // so the metadata fields and Save stay reachable while it is open.
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Form state
  const [name, setName] = useState('');
  // Surveyors, explorers and the date live on every shot: a pending edit of
  // all of them, applied on Save as one undo step (see useLineCreditDraft).
  const credit = useLineCreditDraft(line);
  const [color, setColor] = useState(getThemeColor('--cc-station-default'));
  const [direction, setDirection] = useState<'in' | 'out'>('in');
  const [quality, setQuality] = useState(1);

  // Shots state
  const [shots, setShots] = useState<EditableShot[]>([]);

  // Which rows the editor must lock, in the editor's OWN index space.
  //
  // `isShotFrozen` is positional against the SAVED line, but the editor
  // indexes the dialog's array, and the two stop agreeing the moment a row is
  // inserted here: everything after the insert shifts by one, so frozen-ness
  // stays on the position and slides onto the wrong shot - locking a shot that
  // is editable, and worse, unlocking one whose geometry is adjusted. Resolve
  // the saved indices to shot IDs first, then find those IDs in the dialog's
  // array, so the lock follows the shot (the identity-key rule, #157-era
  // landmine).
  const frozenShotIndices = useMemo(() => {
    if (lineId === undefined || !line) return undefined;
    const frozenIds = new Set<number>();
    line.shots.forEach((shot, i) => {
      if (isShotFrozen(lineId, i)) frozenIds.add(shot.id);
    });
    if (frozenIds.size === 0) return undefined;
    const indices = new Set<number>();
    shots.forEach((shot, i) => {
      // A row added here has no saved counterpart, so it can never be frozen -
      // and its `id` is a temp id that could collide with a real one.
      if (shot.isNew !== true && frozenIds.has(shot.id)) indices.add(i);
    });
    return indices.size > 0 ? indices : undefined;
  }, [lineId, line, isShotFrozen, shots]);
  const [nextTempId, setNextTempId] = useState(-1);
  const [selectedShotKey, setSelectedShotKey] = useState<string | null>(null);
  // `newKeys` are unsaved rows caught in the same delete range. They need no
  // backend confirm, but they must not be dropped before it either: returning
  // early used to skip them entirely, so confirming took only the saved shot and
  // left the new row sitting in the dialog. Carried here, the gesture is atomic -
  // confirm takes all of it, cancel keeps all of it.
  const [pendingMidDelete, setPendingMidDelete] = useState<
    { index: number; shotId: number; newKeys: string[] } | null
  >(null);
  const [pendingImpact, setPendingImpact] = useState<DeleteShotImpact | null>(null);
  const [pendingImpactError, setPendingImpactError] = useState<string | null>(null);
  const shiftBlocked = pendingImpact?.shift_blocked ?? false;
  const shiftBlockedReasons = pendingImpact?.shift_blocked_reasons ?? [];

  // Initialize form with line data
  useEffect(() => {
    if (line) {
      setName(line.name ?? '');
      setColor(line.color);
      setDirection(line.direction ?? 'in');
      setQuality(line.quality ?? 1);
      setShots(line.shots.map(s => ({ ...s })));
    }
  }, [line]);

  const handleShotChange = useCallback((index: number, field: keyof EditableShot, value: number | string | boolean | null) => {
    setShots(prev => {
      const updated = [...prev];
      const shot = updated[index];
      if (!shot) return prev;
      updated[index] = {
        ...shot,
        [field]: value,
      } as EditableShot;
      return updated;
    });
  }, []);

  /** Append `count` blank shots in ONE update, each with its own temp id. Called
   *  N times instead, every row would carry the same captured id - duplicate
   *  identities that the grid keys per-cell state by. */
  const handleAddShots = useCallback((count: number) => {
    const n = Math.max(1, count);
    setShots((prev) => {
      const added: EditableShot[] = [];
      for (let i = 0; i < n; i++) {
        const id = nextTempId - i;
        added.push(blankEditableShot(id));
      }
      return [...prev, ...added];
    });
    setNextTempId((prev) => prev - n);
  }, [nextTempId]);

  const handleAddShot = useCallback(() => handleAddShots(1), [handleAddShots]);

  /** Insert `count` blank shots at `index`, in ONE update with an id per row. */
  const handleInsertShots = useCallback(
    (index: number, count: number, position: 'before' | 'after') => {
      const n = Math.max(1, count);
      const at = position === 'after' ? index + 1 : index;
      setShots((prev) => {
        const next = [...prev];
        next.splice(at, 0, ...Array.from({ length: n }, (_, i) => blankEditableShot(nextTempId - i)));
        return next;
      });
      setNextTempId((prev) => prev - n);
    },
    [nextTempId]
  );

  // Rows that cannot be written: a new row starts blank, so Save has to refuse
  // until it is filled in - otherwise a NaN length would reach the backend. The
  // same gate the new-line dialog puts in front of Create.
  const incompleteRows = useMemo(() => {
    const rows: number[] = [];
    let visible = 0;
    for (const shot of shots) {
      if (shot.isDeleted) continue;
      visible += 1;
      // A row added and then left empty is nothing at all: it blocks nothing and
      // is dropped from the save below. Only a row holding SOMETHING can be
      // incomplete.
      if (isBlankNewShot(shot)) continue;
      const errors = validateEditableShot(
        {
          length: shot.length,
          azimuth_in: shot.azimuth_in,
          azimuth_out: shot.azimuth_out,
          isDeleted: false,
        },
        visible - 1,
        0
      );
      // `validateEditableShot` checks RANGES, and every comparison against NaN
      // is false - so a blank row sails through it. Completeness is the separate
      // question, and the same rule the grid tints its cells by.
      if (errors.length > 0 || !isShotComplete(shot, EDITABLE_AZIMUTH_AVG_KEY)) {
        rows.push(visible);
      }
    }
    return rows;
  }, [shots]);

  const getShotKey = useCallback((shot: EditableShot): string => {
    if (shot.tempId !== undefined) return `t:${shot.tempId}`;
    return `i:${shot.id}`;
  }, []);

  const selectedRowIndex = useMemo(() => {
    if (!selectedShotKey) return null;
    const index = shots.findIndex((shot) => getShotKey(shot) === selectedShotKey);
    return index >= 0 ? index : null;
  }, [getShotKey, selectedShotKey, shots]);

  useEffect(() => {
    if (!selectedShotKey) return;
    const selectedShot = shots.find((shot) => getShotKey(shot) === selectedShotKey);
    if (!selectedShot || selectedShot.isDeleted) {
      setSelectedShotKey(null);
    }
  }, [getShotKey, selectedShotKey, shots]);

  /**
   * Delete a set of shots as ONE decision.
   *
   * Truncating either end of the line is a plain shortening; taking shots out
   * of the middle either splits the passage or shifts everything downstream, so
   * that still asks first. The classification has to see the whole selection at
   * once: run per shot, each call would judge the others as still present, which
   * is why deleting a multi-row selection used to be refused even at the very end
   * of the line.
   */
  const handleDeleteShots = useCallback((indices: number[]): boolean => {
    // Each row is carried WITH the index it came in on: that index is already in
    // the editor's own row space, which is the space `frozenShotIndices` speaks,
    // so the frozen check below is a set lookup rather than a search of `shots`
    // per requested row.
    const requested = indices
      .map((index) => ({ index, shot: shots[index] }))
      .filter((row): row is { index: number; shot: EditableShot } =>
        row.shot !== undefined && row.shot.isDeleted !== true
      );
    if (requested.length === 0) return false;

    // A frozen shot's geometry belongs to the closure solve and the backend
    // refuses to delete it. Soft-deleting it here would take the row off screen
    // and surface the refusal at SAVE, minutes and several edits later - the old
    // table simply offered no delete control on those rows.
    const targets = requested
      .filter((row) => frozenShotIndices?.has(row.index) !== true)
      .map((row) => row.shot);
    const frozenSkipped = requested.length - targets.length;
    if (frozenSkipped > 0) {
      showToast(
        t('editSection.frozenNotDeleted', { count: frozenSkipped }),
        TOAST_KEYS.EDIT_LINE_FROZEN_DELETE,
        'warning'
      );
    }
    if (targets.length === 0) return false;

    // Rows added in this dialog are not in the line yet, so they never make a
    // delete mid-line - they are simply dropped.
    const existingTargets = targets.filter((s) => s.isNew !== true);
    if (existingTargets.length > 0 && line) {
      const surviving = shots.filter((s) => !s.isDeleted && !s.isNew);
      const kind = classifyDeleteRun(
        surviving.map(getShotKey),
        new Set(existingTargets.map(getShotKey))
      );
      if (!isEndRun(kind)) {
        if (existingTargets.length === 1) {
          const only = existingTargets[0]!;
          const existingKeys = new Set(existingTargets.map(getShotKey));
          setPendingMidDelete({
            index: shots.indexOf(only),
            shotId: only.id,
            newKeys: targets.filter((s) => !existingKeys.has(getShotKey(s))).map(getShotKey),
          });
          // Deferred behind the split/shift confirm, so the rows are still there
          // and the selection must survive until the user answers.
          return false;
        }
        // The split/shift preview answers for one shot, so a mid-line run has
        // no path here yet. Say so rather than silently dropping the gesture.
        showToast(
          t('editSection.midRunDelete'),
          TOAST_KEYS.EDIT_LINE_MID_RUN_DELETE,
          'warning'
        );
        return false;
      }
    }

    const targetKeys = new Set(targets.map(getShotKey));
    setShots((prev) =>
      prev
        // A new row leaves no trace; an existing one is soft-deleted so the
        // indices the editor was handed stay valid.
        .filter((s) => !(s.isNew === true && targetKeys.has(getShotKey(s))))
        .map((s) => (targetKeys.has(getShotKey(s)) ? ({ ...s, isDeleted: true } as EditableShot) : s))
    );
    if (selectedShotKey !== null && targetKeys.has(selectedShotKey)) setSelectedShotKey(null);
    return true;
  }, [getShotKey, line, selectedShotKey, shots, showToast, frozenShotIndices, t]);

  const handleDeleteShot = useCallback(
    (index: number) => handleDeleteShots([index]),
    [handleDeleteShots]
  );

  useEffect(() => {
    if (pendingMidDelete && lineId !== undefined) {
      invokeIpc<DeleteShotImpact>('preview_delete_shot', { lineId, shotId: pendingMidDelete.shotId }, { purpose: 'preview mid-shot deletion impact' })
        .then((result) => {
          setPendingImpact(result);
          setPendingImpactError(null);
        })
        .catch((e) => {
          setPendingImpact(null);
          setPendingImpactError(getErrorMessage(e, 'Failed to load shot impacts'));
        });
    } else {
      setPendingImpact(null);
      setPendingImpactError(null);
    }
  }, [pendingMidDelete, lineId]);

  const confirmShiftMidDelete = useCallback(() => {
    if (!pendingMidDelete) return;
    setShots((prev) => {
      const current = prev[pendingMidDelete.index];
      if (!current) return prev;
      // Unsaved rows in the same range simply go; they were never in the line.
      const dropNew = new Set(pendingMidDelete.newKeys);
      const kept =
        dropNew.size > 0 ? prev.filter((s) => !dropNew.has(getShotKey(s))) : prev;
      const index = kept.indexOf(current);
      if (index < 0) return kept;
      if (current.isNew) return kept.filter((_, i) => i !== index);
      const updated = [...kept];
      updated[index] = { ...current, isDeleted: true } as EditableShot;
      return updated;
    });
    if (selectedShotKey !== null) {
      const pendingShot = shots[pendingMidDelete.index];
      if (pendingShot && getShotKey(pendingShot) === selectedShotKey) {
        setSelectedShotKey(null);
      }
    }
    setPendingMidDelete(null);
  }, [getShotKey, pendingMidDelete, selectedShotKey, shots]);

  const confirmSplitMidDelete = useCallback(() => {
    if (!pendingMidDelete || lineId === undefined) return;
    const targetName = line?.name ?? `Line ${lineId}`;
    setSource('dialog');
    openDialog('split-line', {
      lineId,
      shotId: pendingMidDelete.shotId,
      lineName: targetName,
    });
  }, [pendingMidDelete, lineId, line, openDialog]);

  const handleSave = async () => {
    if (lineId === undefined) return;

    setIsLoading(true);
    setError(null);

    // Taken before anything awaits: it commits a name still being typed.
    const creditEdit = credit.takeEdit();
    try {
      // Line metadata and the credit of every saved shot are ONE undo step:
      // a metadata update is only sent when something in it changed (an
      // unchanged one was an invisible step a second Ctrl+Z "did nothing"
      // on), and both together go in one batch. The credit edit runs before
      // the shot edits below, so a shot added here inherits its neighbour's
      // new credit. Surveyors, explorers and the date never go through the
      // line update (its legacy surveyor/date would SET every shot).
      const metadataChanged = !line || isLineMetadataChanged({ name, color, direction, quality }, line);
      const updates = { name: name.trim() || null, color, direction, quality };
      const creditCommand = creditEdit
        ? ({ type: 'edit_shot_credit', targets: { type: 'line', line_id: lineId }, edit: creditEdit } as const)
        : null;
      const result =
        metadataChanged && creditCommand
          ? await executeBatch([{ type: 'update_line_metadata', line_id: lineId, updates }, creditCommand])
          : metadataChanged
            ? await updateLine(lineId, updates)
            : creditCommand
              ? await editShotCredit(creditCommand.targets, creditCommand.edit)
              : null;
      if (result && !result.success) {
        setError(result.error ?? 'Failed to update line');
        return;
      }
      if (creditCommand) credit.reset();

      // Process shot changes
      const originalShots = line?.shots ?? [];

      // Delete removed shots (in reverse order to avoid index issues)
      const deletedShots = shots.filter(s => s.isDeleted && !s.isNew);
      for (const shot of deletedShots.reverse()) {
        const result = await deleteShot(lineId, shot.id);
        if (!result.success) {
          setError(result.error ?? 'Failed to delete shot');
          return;
        }
      }

      // Update existing shots
      for (const shot of shots.filter(s => !s.isNew && !s.isDeleted)) {
        const original = originalShots.find(o => o.id === shot.id);
        if (original && hasChanges(original, shot)) {
          const result = await updateShot(lineId, shot.id, {
            azimuth_in: shot.azimuth_in,
            azimuth_out: shot.azimuth_out,
            length: shot.length,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
            pitch_in: shot.pitch_in,
            pitch_out: shot.pitch_out,
            left: shot.left,
            right: shot.right,
            up: shot.up,
            down: shot.down,
            comment: shot.comment,
            kind: shot.kind ?? 'Normal',
            marker_index: original.marker_index ?? null,
            temperature: original.temperature ?? null,
          });
          if (!result.success) {
            setError(result.error ?? 'Failed to update shot');
            return;
          }
        }
      }

      // Create new shots, each where the user put it (see plannedShotInserts).
      // Rows added and left empty are not shots; they never reach the backend.
      for (const { shot, beforeShotId } of plannedShotInserts(shots)) {
        const shotData: ShotData = {
          azimuth_in: shot.azimuth_in,
          azimuth_out: shot.azimuth_out,
          length: shot.length,
          depth_in: shot.depth_in,
          depth_out: shot.depth_out,
          pitch_in: shot.pitch_in,
          pitch_out: shot.pitch_out,
          left: shot.left,
          right: shot.right,
          up: shot.up,
          down: shot.down,
          comment: shot.comment,
          kind: shot.kind ?? 'Normal',
          marker_index: null,
          temperature: null,
        };
        const result = await createShot(lineId, shotData, undefined, beforeShotId);
        if (!result.success) {
          setError(result.error ?? 'Failed to create shot');
          return;
        }
      }

      trackDialogComplete('edit-line');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to update line'));
    } finally {
      setIsLoading(false);
    }
  };

  // Track if form has been modified
  const isDirty = line
    ? isEditLineDirty(
        { name, creditChanged: credit.changed || credit.hasPendingText, color, direction, quality, shots },
        line,
      )
    : false;

  // Per-project validation thresholds (#57) so inline warnings match the
  // Warnings panel. project.validation is a non-null ValidationConfig (seeded
  // from the global config on load), so read it directly; fall back to the
  // shared TS defaults only when no project is open.
  const validation = project?.validation ?? defaultConfig.validation;
  const validationConfig = {
    azimuth_diff_threshold: validation.azimuth_diff_threshold,
    pitch_diff_threshold: validation.pitch_diff_threshold,
  };

  if (!line) {
    return (
      <DialogWrapper title={t('editSection.title')} onClose={closeDialog} size="5xl" data-testid="edit-line-dialog">
        <div className="text-error">{t('editSection.notFound')}</div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper
      title={t('editSection.title')}
      onClose={closeDialog}
      size={expanded ? 'fullscreen' : '5xl'}
      {...(expanded ? { overflow: 'hidden' as const } : {})}
      isDirty={isDirty}
      data-testid="edit-line-dialog"
    >
      <div className={`space-y-4 ${expanded ? 'flex h-full flex-col' : ''}`}>
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        {/* Line metadata. Expanded, it may scroll: the point of expanding is
            to give the SHOT TABLE room, and on a short window the metadata would
            otherwise eat the whole gain and leave the table smaller than it was
            at its fixed collapsed height. */}
        <div className={expanded ? 'min-h-0 shrink overflow-y-auto' : undefined}>
        <LineMetadataForm
          name={name}
          people={credit.people}
          peopleColumns
          date=""
          dateAggregate={credit.dateAggregate}
          onDateChange={credit.onDateChange}
          {...(credit.onDateReset ? { onDateReset: credit.onDateReset } : {})}
          color={color}
          direction={direction}
          quality={quality}
          onNameChange={setName}
          onColorChange={setColor}
          onQualityChange={setQuality}
          onDirectionChange={setDirection}
          showDirection={true}
          // Closure-adjustment weight is not settable here for now. The stored
          // value is preserved on save; it is just not exposed.
          showQuality={false}
          disabled={isLoading || editingBlocked}
          creditUnavailable={!credit.available}
        />
        </div>

        {/* Shots table using ShotTableEditor. Expanded, it takes the room the
            fullscreen dialog frees up; otherwise it keeps its fixed height. */}
        <div className={`border-t border-border pt-4 ${expanded ? 'flex min-h-0 flex-1 flex-col' : ''}`}>
          <ShotTableEditor
            shots={shots}
            onShotChange={handleShotChange}
            onAddShot={handleAddShot}
            onAddShots={handleAddShots}
            onInsertShots={handleInsertShots}
            layoutScope="edit-line"
            onDeleteShot={handleDeleteShot}
            onDeleteShots={handleDeleteShots}
            readOnly={editingBlocked}
            validationConfig={validationConfig}
            selectedRowIndex={selectedRowIndex}
            onRowSelect={(rowIndex) => {
              const shot = shots[rowIndex];
              if (!shot) {
                setSelectedShotKey(null);
                return;
              }
              setSelectedShotKey(getShotKey(shot));
            }}
            {...(frozenShotIndices ? { frozenShotIndices } : {})}
            {...(lineId !== undefined ? { lineId } : {})}
            expanded={expanded}
            onToggleExpand={() => setExpanded((v) => !v)}
            {...(expanded ? {} : { maxHeight: '16rem' })}
          />
        </div>

        {pendingMidDelete && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-3">
            <p className="font-medium text-warning">{t('editSection.midShot')}</p>
            <p className="text-text-secondary">{t('editSection.midShotExplain')}</p>

            {pendingImpactError && (
              <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
                {pendingImpactError}
              </div>
            )}

            {shiftBlocked && (
              <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
                <p className="font-medium">{t('editSection.shiftUnavailable')}</p>
                {shiftBlockedReasons.length > 0 && (
                  <ul className="list-disc list-inside">
                    {shiftBlockedReasons.map((reason, index) => (
                      <li key={`shift-blocked-${index}`}>{reason}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            <DeleteShotImpactDetails
              impact={pendingImpact}
              title={t('editSection.shiftImpactTitle')}
            />

            <div className="flex justify-end gap-2">
              <Button onClick={() => setPendingMidDelete(null)} data-testid="edit-line-delete-cancel">{t('common.actions.cancel')}</Button>
              <Button onClick={confirmSplitMidDelete} data-testid="edit-line-delete-split">{t('editSection.splitSection')}</Button>
              <Button variant="danger" onClick={confirmShiftMidDelete} disabled={shiftBlocked} data-testid="edit-line-delete-shift">
                {t('editSection.shiftGeometry')}
</Button>
            </div>
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          {/*
            Reverse (#24) applies straight to the backend, so it cannot merge with
            unsaved edits held in this dialog's local copy. Rather than prompting
            to discard them, the action is simply unavailable while dirty - no
            confirm hop, and no way to lose work by misreading a dialog.
          */}
          <Button
            onClick={() => {
              openDialog('reverse-line', { lineIds: [line.id] });
            }}
            disabled={isLoading || isDirty || pendingMidDelete !== null || editingBlocked}
            title={
              editingBlocked
                ? t('editSection.editingDisabled')
                : isDirty
                  ? t('editSection.saveOrDiscardFirst')
                  : 'Renumber this line end-to-end (the cave does not move)'
            }
            data-testid="edit-line-reverse"
          >
            {t('editSection.reverseSection')}
</Button>
          {incompleteRows.length > 0 && (
            <p className="text-sm text-error" data-testid="edit-line-blocker">
              {incompleteRows.length === 1
                ? t('editSection.incompleteRow', { count: 1, rows: incompleteRows[0] })
                : t('editSection.incompleteRow', { count: incompleteRows.length, rows: incompleteRows.join(', ') })}
            </p>
          )}
          <div className="flex-1" />
          <Button onClick={closeDialog} disabled={isLoading} data-testid="edit-line-cancel">
            {t('common.actions.cancel')}
</Button>
          <Button
            variant="primary"
            onClick={handleSave}
            disabled={isLoading || editingBlocked || pendingMidDelete !== null || incompleteRows.length > 0}
            data-testid="edit-line-save"
          >
            {isLoading ? t('editSection.saving') : t('editSection.save')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

/** A row Save has to create, and the shot it must land in front of. */
export interface PlannedShotInsert {
  shot: EditableShot;
  /** Absent = append at the end of the line, which is where the row is. */
  beforeShotId?: ShotId;
}

/**
 * The rows Save has to create, in the order it must create them, each anchored
 * to where the user actually put it.
 *
 * `createShot` with NEITHER anchor appends, so every added row - including one
 * inserted between two measured shots - used to be written at the end of the
 * line, rearranging the passage on save.
 *
 * The anchor is the next SAVED shot rather than the previous one, because that
 * is the one identity that can be named for every row of a run without waiting
 * on the id the backend is about to mint: consecutive added rows all quote the
 * same anchor, and each insert lands immediately before it - i.e. after the row
 * created just before it - so a run keeps the order it has on screen. A run with
 * no saved shot after it appends, in order, which is the same position.
 *
 * Excluded from the walk, and from the anchoring:
 * - deleted rows. Their deletes run FIRST in `handleSave`, so a deleted shot's
 *   id no longer resolves by the time a create quotes it, and the backend would
 *   reject the create outright.
 * - blank added rows, which are not shots and are never written.
 */
export function plannedShotInserts(rows: readonly EditableShot[]): PlannedShotInsert[] {
  const plan: PlannedShotInsert[] = [];
  // Backwards, so every added row already knows the saved shot that follows it.
  let nextSavedId: ShotId | undefined;
  for (let i = rows.length - 1; i >= 0; i--) {
    const row = rows[i]!;
    if (row.isDeleted === true) continue;
    if (row.isNew !== true) {
      nextSavedId = row.id;
      continue;
    }
    if (isBlankNewShot(row)) continue;
    plan.push(nextSavedId === undefined ? { shot: row } : { shot: row, beforeShotId: nextSavedId });
  }
  return plan.reverse();
}

/** The form snapshot the dialog diffs against the saved line. */
export interface EditLineDirtyState {
  name: string;
  /** The pending survey-credit edit changes a shot, or a people field holds typed text. */
  creditChanged: boolean;
  color: string;
  direction: 'in' | 'out';
  quality: number;
  shots: readonly EditableShot[];
}

/** The line-level fields Save sends through `update_line_metadata`. */
export interface LineMetadataState {
  name: string;
  color: string;
  direction: 'in' | 'out';
  quality: number;
}

/** Whether any of them differs from the saved line (the rule `isEditLineDirty` uses). */
export function isLineMetadataChanged(state: LineMetadataState, line: Line): boolean {
  return (
    state.name !== (line.name ?? '') ||
    state.color !== line.color ||
    state.direction !== (line.direction ?? 'in') ||
    state.quality !== (line.quality ?? 1)
  );
}

/**
 * Whether the dialog holds unsaved changes.
 *
 * A row added and then cleared back to blank is exactly what Save skips (the
 * `isBlankNewShot` / `plannedShotInserts` predicate emits nothing for it), so it
 * does not count as a pending change either - treating every `isNew` row as
 * dirty prompted "discard changes?" on close when Save would have written
 * nothing for that row and the metadata was untouched. A real (filled-in) new
 * row, a soft-deleted existing row, an edited existing row, or changed metadata
 * all still count.
 */
export function isEditLineDirty(state: EditLineDirtyState, line: Line): boolean {
  return (
    isLineMetadataChanged(state, line) ||
    state.creditChanged ||
    state.shots.some((s) => s.isDeleted === true || (s.isNew === true && !isBlankNewShot(s))) ||
    state.shots.some((s) => {
      const original = line.shots.find((o) => o.id === s.id);
      return original !== undefined && hasChanges(original, s);
    })
  );
}

function hasChanges(original: Shot, current: EditableShot): boolean {
  return (
    original.azimuth_in !== current.azimuth_in ||
    original.azimuth_out !== current.azimuth_out ||
    original.length !== current.length ||
    original.depth_in !== current.depth_in ||
    original.depth_out !== current.depth_out ||
    original.pitch_in !== current.pitch_in ||
    original.pitch_out !== current.pitch_out ||
    original.left !== current.left ||
    original.right !== current.right ||
    original.up !== current.up ||
    original.down !== current.down ||
    original.comment !== current.comment ||
    original.kind !== current.kind
  );
}
