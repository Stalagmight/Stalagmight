import { useState, useEffect, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import i18n from '../../i18n';
import { useUIStore } from '../../store/uiStore';
import { classifyDeleteRun, isEndRun } from './editSectionDeletes';
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
import { SectionMetadataForm } from '../common/SectionMetadataForm';
import type { Section, Shot, ShotData, ShotId } from '../../types/project';
import type { DeleteShotImpact } from '../../types/commands';
import type { ShotUpdates } from '../../types/ipc/ShotUpdates';
import { changedShotFields } from '../../utils/shotUpdatesDiff';
import { defaultConfig } from '../../types/config';
import DeleteShotImpactDetails from './DeleteShotImpactDetails';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useShallow } from 'zustand/react/shallow';

export default function EditSectionDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, openDialog, showToast } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      dialogData: s.dialogData,
      openDialog: s.openDialog,
      showToast: s.showToast,
    })),
  );
  // `frozenShots` is picked but not destructured: isShotFrozen reads it through
  // get(), so a change to the frozen set must still re-render this dialog.
  const { updateSection, createShot, updateShot, deleteShot, project, isShotFrozen } = useProjectStore(
    useShallow((s) => ({
      updateSection: s.updateSection,
      createShot: s.createShot,
      updateShot: s.updateShot,
      deleteShot: s.deleteShot,
      project: s.project,
      isShotFrozen: s.isShotFrozen,
      frozenShots: s.frozenShots,
    })),
  );

  const sectionId = dialogData.sectionId as number | undefined;
  const section = project?.sections.find((s) => s.id === sectionId);


  // Reverse (#24) mutates the survey, and the closure-adjusted / Plan / Export
  // read-only mode has no server backstop (see useEditingBlocked's
  // backstop-asymmetry note), so its entry point has to be gated here too.
  const editingBlocked = useEditingBlocked();

  const [isLoading, setIsLoading] = useState(false);
  // Expand grows the DIALOG, not just the grid - same as the new-section dialog,
  // so the metadata fields and Save stay reachable while it is open.
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Form state
  const [name, setName] = useState('');
  const [surveyor, setSurveyor] = useState('');
  const [date, setDate] = useState('');
  const [color, setColor] = useState(getThemeColor('--cc-station-default'));
  const [direction, setDirection] = useState<'in' | 'out'>('in');
  const [quality, setQuality] = useState(1);

  // Shots state
  const [shots, setShots] = useState<EditableShot[]>([]);

  // Which rows the editor must lock, in the editor's OWN index space.
  //
  // `isShotFrozen` is positional against the SAVED section, but the editor
  // indexes the dialog's array, and the two stop agreeing the moment a row is
  // inserted here: everything after the insert shifts by one, so frozen-ness
  // stays on the position and slides onto the wrong shot - locking a shot that
  // is editable, and worse, unlocking one whose geometry is adjusted. Resolve
  // the saved indices to shot IDs first, then find those IDs in the dialog's
  // array, so the lock follows the shot (the identity-key rule, #157-era
  // landmine).
  const frozenShotIndices = useMemo(() => {
    if (sectionId === undefined || !section) return undefined;
    const frozenIds = new Set<number>();
    section.shots.forEach((shot, i) => {
      if (isShotFrozen(sectionId, i)) frozenIds.add(shot.id);
    });
    if (frozenIds.size === 0) return undefined;
    const indices = new Set<number>();
    shots.forEach((shot, i) => {
      // A row added here has no saved counterpart, so it can never be frozen -
      // and its `id` is a temp id that could collide with a real one.
      if (shot.isNew !== true && frozenIds.has(shot.id)) indices.add(i);
    });
    return indices.size > 0 ? indices : undefined;
  }, [sectionId, section, isShotFrozen, shots]);
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

  // Initialize form with section data
  useEffect(() => {
    if (section) {
      setName(section.name ?? '');
      setSurveyor(section.surveyor ?? '');
      setDate(section.date ?? '');
      setColor(section.color);
      setDirection(section.direction ?? 'in');
      setQuality(section.quality ?? 1);
      setShots(section.shots.map(s => ({ ...s })));
    }
  }, [section]);

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
  // same gate the new-section dialog puts in front of Create.
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
   * Truncating either end of the section is a plain shortening; taking shots out
   * of the middle either splits the passage or shifts everything downstream, so
   * that still asks first. The classification has to see the whole selection at
   * once: run per shot, each call would judge the others as still present, which
   * is why deleting a multi-row selection used to be refused even at the very end
   * of the section.
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
        TOAST_KEYS.EDIT_SECTION_FROZEN_DELETE,
        'warning'
      );
    }
    if (targets.length === 0) return false;

    // Rows added in this dialog are not in the section yet, so they never make a
    // delete mid-section - they are simply dropped.
    const existingTargets = targets.filter((s) => s.isNew !== true);
    if (existingTargets.length > 0 && section) {
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
        // The split/shift preview answers for one shot, so a mid-section run has
        // no path here yet. Say so rather than silently dropping the gesture.
        showToast(
          t('editSection.midRunDelete'),
          TOAST_KEYS.EDIT_SECTION_MID_RUN_DELETE,
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
  }, [getShotKey, section, selectedShotKey, shots, showToast, frozenShotIndices, t]);

  const handleDeleteShot = useCallback(
    (index: number) => handleDeleteShots([index]),
    [handleDeleteShots]
  );

  useEffect(() => {
    if (pendingMidDelete && sectionId !== undefined) {
      invokeIpc('preview_delete_shot', { sectionId, shotId: pendingMidDelete.shotId }, { purpose: 'preview mid-shot deletion impact' })
        .then((result) => {
          setPendingImpact(result);
          setPendingImpactError(null);
        })
        .catch((e) => {
          setPendingImpact(null);
          // i18n.t, not the hook's t: this IPC effect must not re-fetch on a language change.
          setPendingImpactError(getErrorMessage(e, i18n.t('editSection.loadImpactsFailed')));
        });
    } else {
      setPendingImpact(null);
      setPendingImpactError(null);
    }
  }, [pendingMidDelete, sectionId]);

  const confirmShiftMidDelete = useCallback(() => {
    if (!pendingMidDelete) return;
    setShots((prev) => {
      const current = prev[pendingMidDelete.index];
      if (!current) return prev;
      // Unsaved rows in the same range simply go; they were never in the section.
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
    if (!pendingMidDelete || sectionId === undefined) return;
    const targetName = section?.name ?? t('editSection.sectionFallback', { id: sectionId });
    setSource('dialog');
    openDialog('split-section', {
      sectionId,
      shotId: pendingMidDelete.shotId,
      sectionName: targetName,
    });
  }, [pendingMidDelete, sectionId, section, openDialog, t]);

  const handleSave = async () => {
    if (sectionId === undefined) return;

    setIsLoading(true);
    setError(null);

    try {
      // Update section metadata
      const sectionResult = await updateSection(sectionId, {
        name: name.trim() || null,
        surveyor: surveyor.trim() || null,
        date: date.trim() || null,
        color,
        direction,
        quality,
      });

      if (!sectionResult.success) {
        setError(sectionResult.error ?? t('editSection.updateSectionFailed'));
        return;
      }

      // Process shot changes
      const originalShots = section?.shots ?? [];

      // Delete removed shots (in reverse order to avoid index issues)
      const deletedShots = shots.filter(s => s.isDeleted && !s.isNew);
      for (const shot of deletedShots.reverse()) {
        const result = await deleteShot(sectionId, shot.id);
        if (!result.success) {
          setError(result.error ?? t('editSection.deleteShotFailed'));
          return;
        }
      }

      // Update existing shots
      for (const shot of shots.filter(s => !s.isNew && !s.isDeleted)) {
        const original = originalShots.find(o => o.id === shot.id);
        if (original && hasChanges(original, shot)) {
          const result = await updateShot(sectionId, shot.id, editedShotUpdates(original, shot));
          if (!result.success) {
            setError(result.error ?? t('editSection.updateShotFailed'));
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
        const result = await createShot(sectionId, shotData, undefined, beforeShotId);
        if (!result.success) {
          setError(result.error ?? t('editSection.createShotFailed'));
          return;
        }
      }

      trackDialogComplete('edit-section');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, t('editSection.updateSectionFailed')));
    } finally {
      setIsLoading(false);
    }
  };

  // Track if form has been modified
  const isDirty = section
    ? isEditSectionDirty({ name, surveyor, date, color, direction, quality, shots }, section)
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

  if (!section) {
    return (
      <DialogWrapper title={t('editSection.title')} onClose={closeDialog} size="5xl" data-testid="edit-section-dialog">
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
      data-testid="edit-section-dialog"
    >
      <div className={`space-y-4 ${expanded ? 'flex h-full flex-col' : ''}`}>
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        {/* Section metadata. Expanded, it may scroll: the point of expanding is
            to give the SHOT TABLE room, and on a short window the metadata would
            otherwise eat the whole gain and leave the table smaller than it was
            at its fixed collapsed height. */}
        <div className={expanded ? 'min-h-0 shrink overflow-y-auto' : undefined}>
        <SectionMetadataForm
          name={name}
          surveyor={surveyor}
          date={date}
          color={color}
          direction={direction}
          quality={quality}
          onNameChange={setName}
          onSurveyorChange={setSurveyor}
          onDateChange={setDate}
          onColorChange={setColor}
          onQualityChange={setQuality}
          onDirectionChange={setDirection}
          showDirection={true}
          // Closure-adjustment weight is not settable here for now. The stored
          // value is preserved on save; it is just not exposed.
          showQuality={false}
          disabled={isLoading}
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
            layoutScope="edit-section"
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
            {...(sectionId !== undefined ? { sectionId } : {})}
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
              <Button onClick={() => setPendingMidDelete(null)} data-testid="edit-section-delete-cancel">{t('common.actions.cancel')}</Button>
              <Button onClick={confirmSplitMidDelete} data-testid="edit-section-delete-split">{t('editSection.splitSection')}</Button>
              <Button variant="danger" onClick={confirmShiftMidDelete} disabled={shiftBlocked} data-testid="edit-section-delete-shift">
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
              openDialog('reverse-section', { sectionIds: [section.id] });
            }}
            disabled={isLoading || isDirty || pendingMidDelete !== null || editingBlocked}
            title={
              editingBlocked
                ? t('editSection.editingDisabled')
                : isDirty
                  ? t('editSection.saveOrDiscardFirst')
                  : t('editSection.reverseTitle')
            }
            data-testid="edit-section-reverse"
          >
            {t('editSection.reverseSection')}
</Button>
          {incompleteRows.length > 0 && (
            <p className="text-sm text-error" data-testid="edit-section-blocker">
              {incompleteRows.length === 1
                ? t('editSection.incompleteRow', { count: 1, rows: incompleteRows[0] })
                : t('editSection.incompleteRow', { count: incompleteRows.length, rows: incompleteRows.join(', ') })}
            </p>
          )}
          <div className="flex-1" />
          <Button onClick={closeDialog} disabled={isLoading} data-testid="edit-section-cancel">
            {t('common.actions.cancel')}
</Button>
          <Button
            variant="primary"
            onClick={handleSave}
            disabled={isLoading || editingBlocked || pendingMidDelete !== null || incompleteRows.length > 0}
            data-testid="edit-section-save"
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
  /** Absent = append at the end of the section, which is where the row is. */
  beforeShotId?: ShotId;
}

/**
 * The rows Save has to create, in the order it must create them, each anchored
 * to where the user actually put it.
 *
 * `createShot` with NEITHER anchor appends, so every added row - including one
 * inserted between two measured shots - used to be written at the end of the
 * section, rearranging the passage on save.
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

/** The form snapshot the dialog diffs against the saved section. */
export interface EditSectionDirtyState {
  name: string;
  surveyor: string;
  date: string;
  color: string;
  direction: 'in' | 'out';
  quality: number;
  shots: readonly EditableShot[];
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
export function isEditSectionDirty(state: EditSectionDirtyState, section: Section): boolean {
  return (
    state.name !== (section.name ?? '') ||
    state.surveyor !== (section.surveyor ?? '') ||
    state.date !== (section.date ?? '') ||
    state.color !== section.color ||
    state.direction !== (section.direction ?? 'in') ||
    state.quality !== (section.quality ?? 1) ||
    state.shots.some((s) => s.isDeleted === true || (s.isNew === true && !isBlankNewShot(s))) ||
    state.shots.some((s) => {
      const original = section.shots.find((o) => o.id === s.id);
      return original !== undefined && hasChanges(original, s);
    })
  );
}

/**
 * [#576 F32-EDIT] The update for one edited row: only the fields that differ from
 * the saved shot. The store holds every reading rounded through an f32 (the binary
 * envelope), so re-sending an untouched field would write that rounding over the
 * backend's exact value. Marker index and temperature have no column here and are
 * never sent.
 */
export function editedShotUpdates(original: Shot, current: EditableShot): Partial<ShotUpdates> {
  // [#576 NF-W7F-1] Compared in f32: a cell re-typed to the reading it already
  // showed (1.20 over 1.2) is not an edit.
  return changedShotFields(original, {
    azimuth_in: current.azimuth_in,
    azimuth_out: current.azimuth_out,
    length: current.length,
    depth_in: current.depth_in,
    depth_out: current.depth_out,
    pitch_in: current.pitch_in,
    pitch_out: current.pitch_out,
    left: current.left,
    right: current.right,
    up: current.up,
    down: current.down,
    comment: current.comment,
    kind: current.kind ?? 'Normal',
  }, 'f32');
}

/** Whether Save would send an update for this row (the dirty check and Save agree). */
function hasChanges(original: Shot, current: EditableShot): boolean {
  return Object.keys(editedShotUpdates(original, current)).length > 0;
}
