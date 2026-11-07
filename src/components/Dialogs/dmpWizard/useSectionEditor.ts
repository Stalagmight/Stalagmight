/**
 * The DMP wizard's section editor (split out of DmpNewImportDialog with #576
 * D4): the rows of the section on screen, the overlap view drawn over them, and
 * every change a user makes to them.
 *
 * The section on screen is the suggestion's import section while suggesting
 * and the segment anchor's section while manual placing. Its edits live in the
 * run's `sectionEdits` map (the dialog's ref, which the imports and the saved
 * state read); `editState` is the copy React renders, re-seeded from that map
 * whenever the section on screen changes and written back after every edit.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from 'react';
import { useTranslation } from 'react-i18next';
import type { CrossSectionSuggestion, DmpFile } from '../../../types/commands';
import type { OverlapMode } from '../../../types/config';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import type { EditableShot } from '../../common/ShotTableEditor';
import { useLatest } from '../../../hooks/useLatest';
import { useLiveShots } from './useLiveShots';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { convertToEditableShots } from '../../../utils/shotConversion';
import {
  computeOverlapIds,
  computeActionableIds,
  computeTargetShotIndex,
  needsDirectionReversal,
  reverseTargetShot,
  applyTargetData,
  interpolateShots,
  type TargetShotLike,
} from '../../../utils/overlapHelpers';
import type { SectionEditState } from '../../../utils/batchImportHelpers';
import { isEmittedShot, visibleBefore, visibleShotCount } from '../../common/emittedShots';
import { findIncompleteSections } from '../../../utils/importPayloadGuard';
import { blankEditableShot } from '../../common/editableShot';
import { debugLog } from '../../../utils/debugLog';
import { TOAST_KEYS } from '../../../utils/toastKeys';

/** A change of `sectionIndex`'s visible row count: `delta` rows at visible row `at`. */
export interface RowCountChange {
  sectionIndex: number;
  at: number;
  delta: number;
  survivorsAfter: number;
}

export function useSectionEditor({
  activeEditableSectionIndex,
  currentSuggestion,
  selectedMergePair,
  overlapMode,
  dmpFile,
  selectionConfig,
  selectedIndices,
  sectionPalette,
  sectionEditsRef,
  onRowsChanged,
}: {
  /** The section on screen, or null when none is. */
  activeEditableSectionIndex: number | null;
  /** The suggestion on screen: its overlap rows are frozen and drawn treated. */
  currentSuggestion: CrossSectionSuggestion | null;
  selectedMergePair: { importStation: number; existingStation: number } | null;
  overlapMode: OverlapMode;
  dmpFile: DmpFile;
  selectionConfig: DmpSelectionConfig;
  selectedIndices: number[];
  sectionPalette: readonly string[];
  /** The run's edits per DMP section index. */
  sectionEditsRef: MutableRefObject<Map<number, SectionEditState>>;
  /** Rows were added or removed: the segment anchor renumbers with them. */
  onRowsChanged: (change: RowCountChange) => void;
}) {
  const { t } = useTranslation();
  // Section editing state
  const [editState, setEditState] = useState<SectionEditState | null>(null);
  /** The live edit state and rows, for actions that must not read a captured
   *  copy - see refuseIncompleteShots and applyShotEdit. */
  const { editStateRef, currentShots, stageShots } = useLiveShots(editState);
  // Tracks which import section index the current editState was loaded for.
  // Used to prevent the sync effect from writing stale editState to the wrong section
  // when currentSuggestion changes before editState catches up.
  const [editStateForSection, setEditStateForSection] = useState<number | null>(null);
  /** Which section `editStateRef` currently holds, as a ref: the gate below has
   *  to pair the two, and a captured pairing is the stale-closure bug again. */
  const editStateForSectionRef = useLatest<number | null>(editStateForSection);
  const nextTempIdRef = useRef(1);

  // Initialize / restore section edit state when active editable section changes.
  useEffect(() => {
    if (activeEditableSectionIndex == null) {
      setEditStateForSection(null);
      setEditState(null);
      return;
    }

    const idx = activeEditableSectionIndex;

    // Restore from ref if previously edited
    const existing = sectionEditsRef.current.get(idx);
    if (existing) {
      // Re-wrapped in the DEV guard (unlike the trivial-argument sites nearby):
      // `debugLog` itself no-ops in production, but JS evaluates a function's
      // arguments before the call, so the `.filter(...).length` scan below ran
      // on every restore in production regardless - the guard is what actually
      // skips the work, not the no-op.
      if (import.meta.env.DEV) {
        debugLog(
          `[init-effect] RESTORE section=${idx} from sectionEditsRef`,
          `shots.length=${existing.shots.length}`,
          `deleted=${existing.shots.filter(s => s.isDeleted).length}`,
        );
      }
      // The counter is per section, and only the initialize branch below used to
      // seed it: coming back to a section after editing a SHORTER one handed out
      // temp ids this section had already used, so two rows shared a key and the
      // grid painted one row's marks on the other.
      nextTempIdRef.current =
        existing.shots.reduce((max, s) => Math.max(max, s.tempId ?? 0), existing.shots.length) + 1;
      setEditStateForSection(idx);
      setEditState(existing);
      return;
    }

    // Initialize from DMP data + selectionConfig defaults
    const section = dmpFile.sections[idx];
    if (!section) {
      setEditStateForSection(null);
      setEditState(null);
      return;
    }

    const shots = convertToEditableShots(section.shots, section.version);
    nextTempIdRef.current = shots.length + 1;
    debugLog(
      `[init-effect] INITIALIZE section=${idx} "${section.name}"`,
      `dmpShots=${section.shots.length} standardShots=${shots.length}`,
    );

    // Color based on position in selection order (matches zero datum dialog)
    const selectedPos = selectedIndices.indexOf(idx);
    const newState: SectionEditState = {
      name: section.name,
      surveyor: selectionConfig.defaultSurveyor || '',
      date: selectionConfig.defaultDate ?? section.date?.split(/[T ]/)[0] ?? '',
      color: sectionPalette[(selectedPos >= 0 ? selectedPos : 0) % sectionPalette.length] ?? '#4ecdc4',
      direction: section.direction === 'out' ? 'out' : 'in',
      quality: 1.0,
      reverse: false,
      shots,
    };

    sectionEditsRef.current.set(idx, newState);
    setEditStateForSection(idx);
    setEditState(newState);
  // eslint-disable-next-line react-hooks/exhaustive-deps -- (re)initialise only when the active section changes; the other inputs are read at that moment
  }, [activeEditableSectionIndex]);

  // Keep sectionEditsRef in sync with editState changes (user edits to shots/metadata).
  // Guard: only write when editState actually belongs to the active edit section.
  // Without this guard, when the active section changes, this effect can briefly fire
  // with stale editState from the previous section before setEditState takes effect.
  useEffect(() => {
    if (!editState || activeEditableSectionIndex == null) return;
    if (editStateForSection !== activeEditableSectionIndex) {
      debugLog(
        `[sync-effect] SKIPPED stale write: editStateFor=${editStateForSection} !== active=${activeEditableSectionIndex}`,
      );
      return;
    }
    debugLog(
      `[sync-effect] WRITING sectionEditsRef[${activeEditableSectionIndex}]`,
      `editState.shots.length=${editState.shots.length}`,
    );
    sectionEditsRef.current.set(activeEditableSectionIndex, editState);
  }, [editState, activeEditableSectionIndex, editStateForSection, sectionEditsRef]);

  // Compute frozen shot indices - matched overlap shots that must not be deleted
  const frozenShotIndices = useMemo(() => {
    if (!currentSuggestion || !editState) return new Set<number>();

    const { importStationIndex, matchedShots } = currentSuggestion;
    const frozenIds = computeOverlapIds(importStationIndex, matchedShots);

    // Map original IDs to current indices in editState.shots
    const indices = new Set<number>();
    editState.shots.forEach((shot, idx) => {
      if (!shot.isNew && frozenIds.has(shot.id)) indices.add(idx);
    });
    return indices;
  }, [currentSuggestion, editState]);

  // Compute effective shots: applies overlap mode transformations as a derived view.
  // Raw editState is never mutated by overlap settings - effectiveShots is computed.
  const effectiveShots = useMemo(() => {
    if (!editState || !currentSuggestion || currentSuggestion.matchedShots === 0) {
      return editState?.shots ?? [];
    }

    const { importStationIndex, matchedShots, existingMatchedShots, direction } = currentSuggestion;
    const mergeImportStation = selectedMergePair?.importStation ?? importStationIndex;

    const allOverlapIds = computeOverlapIds(importStationIndex, matchedShots);
    const actionableIds = computeActionableIds(importStationIndex, matchedShots, mergeImportStation);

    // Get target shots for data replacement
    let targetShots: TargetShotLike[] | null = null;
    if (overlapMode !== 'import_data' && overlapMode !== 'keep_both') {
      const { targetType, existingSectionId, targetImportIndex, existingStationIndex } = currentSuggestion;

      if (targetType === 'existing' && existingSectionId != null) {
        const currentProject = useProjectStore.getState().project;
        const targetSection = currentProject?.sections.find((s) => s.id === existingSectionId);
        if (targetSection) targetShots = targetSection.shots;
      } else if (targetType === 'importing' && targetImportIndex != null) {
        const targetEdits = sectionEditsRef.current.get(targetImportIndex);
        if (targetEdits) {
          // Station space - see the note on the other target-shot build.
          targetShots = targetEdits.shots.filter(isEmittedShot);
        } else {
          const targetDmpSection = dmpFile.sections[targetImportIndex];
          if (targetDmpSection) {
            targetShots = convertToEditableShots(targetDmpSection.shots, targetDmpSection.version);
          }
        }
      }

      // Build a mapping: import overlap shot ID → target shot.
      // The mapping is anchored by existingStationIndex (the suggestion's base),
      // NOT the merge-adjusted station - the merge picker only controls which
      // shots are actionable, not the overlap-to-target correspondence.
      if (targetShots) {
        const shouldReverse = needsDirectionReversal(importStationIndex, direction);
        const overlapParams = { importStationIndex, matchedShots, existingMatchedShots, existingStationIndex, direction };
        const targetMap = new Map<number, TargetShotLike>();
        for (const overlapId of allOverlapIds) {
          const targetIdx = computeTargetShotIndex(overlapId, overlapParams);
          if (targetIdx >= 0 && targetIdx < targetShots.length) {
            const raw = targetShots[targetIdx]!;
            targetMap.set(overlapId, shouldReverse ? reverseTargetShot(raw) : raw);
          }
        }
        // Replace targetShots reference with the map for efficient lookup
        return editState.shots.map((shot) => {
          if (shot.isNew || shot.isDeleted) return shot;
          const isOverlap = !shot.isNew && allOverlapIds.has(shot.id);
          if (!isOverlap) return shot;

          const isActionable = actionableIds.has(shot.id);
          if (isActionable) {
            const target = targetMap.get(shot.id);
            if (target) {
              if (overlapMode === 'existing_data') {
                return applyTargetData(shot, target);
              } else if (overlapMode === 'interpolate') {
                return interpolateShots(shot, target);
              }
            }
            return shot;
          }
          // Past merge station: leave unchanged to keep geometry continuous in preview.
          // Deletion happens at import time in handleConfirm.
          return shot;
        });
      }
    }

    // For import_data / keep_both / no target data available:
    return editState.shots.map((shot) => {
      if (shot.isNew || shot.isDeleted) return shot;
      const isOverlap = !shot.isNew && allOverlapIds.has(shot.id);
      if (!isOverlap) return shot;

      const isActionable = actionableIds.has(shot.id);
      if (overlapMode === 'keep_both') {
        return { ...shot, kind: 'Reference' as const };
      }
      if (!isActionable) {
        // Past merge station: leave unchanged to keep geometry continuous in preview.
        return shot;
      }
      // import_data: keep import values as-is
      return shot;
    });
  }, [editState, currentSuggestion, selectedMergePair, overlapMode, dmpFile.sections, sectionEditsRef]);

  /**
   * Rows the keep-both view DERIVES as Reference.
   *
   * Their glyph is computed, not read back from `editState`, so the kind cell on
   * these rows never showed what a click on it wrote: the display stayed
   * Reference while the write destroyed the Jump the user had set underneath,
   * and the import forces Reference on them again regardless. Nothing to edit,
   * so nothing pretends to be editable.
   */
  const derivedKindIndices = useMemo(() => {
    if (
      overlapMode !== 'keep_both' ||
      !editState ||
      !currentSuggestion ||
      currentSuggestion.matchedShots === 0
    ) {
      return new Set<number>();
    }
    const overlapIds = computeOverlapIds(
      currentSuggestion.importStationIndex,
      currentSuggestion.matchedShots,
    );
    const indices = new Set<number>();
    editState.shots.forEach((shot, idx) => {
      if (!shot.isNew && !shot.isDeleted && overlapIds.has(shot.id)) indices.add(idx);
    });
    return indices;
  }, [overlapMode, editState, currentSuggestion]);

  // Section edit callbacks
  const handleMetadataChange = useCallback((field: keyof SectionEditState, value: string | number | boolean) => {
    setEditState((prev) => prev ? { ...prev, [field]: value } : prev);
  }, []);

  const handleShotChange = useCallback(
    (index: number, field: keyof EditableShot, value: number | string | boolean | null) => {
      if (field === 'kind' && derivedKindIndices.has(index)) return;
      setEditState((prev) => {
        if (!prev) return prev;
        const newShots = [...prev.shots];
        const shot = newShots[index];
        if (shot) {
          newShots[index] = { ...shot, [field]: value };
        }
        return { ...prev, shots: newShots };
      });
    },
    [derivedKindIndices]
  );

  /**
   * The one path a row-count change takes.
   *
   * `arrayIndex` is where the change lands in the FULL array and `delta` how
   * many VISIBLE rows it adds (positive) or removes (negative) there. The
   * segment anchor is a station index in that same visible space, so it is
   * renumbered here, in the same step as the rows: a mutator that wrote only the
   * rows left the pick addressing a station its own dropdown no longer offered.
   *
   * The transition is read from the rows as they stand now and committed OUTSIDE
   * the state updater, which stays pure - StrictMode invokes updaters twice on
   * purpose, and a commit in there re-anchored the pick twice for one edit.
   */
  const applyShotEdit = useCallback(
    (arrayIndex: number, delta: number, mutate: (shots: EditableShot[]) => EditableShot[]) => {
      const before = currentShots();
      stageShots(mutate(before));
      setEditState((prev) => (prev ? { ...prev, shots: mutate(prev.shots) } : prev));

      // The segment anchor is renumbered in the same step (the wizard's reducer,
      // 'rowsChanged'); only a manual-placing anchor on this section moves.
      if (delta === 0 || activeEditableSectionIndex == null) return;
      onRowsChanged({
        sectionIndex: activeEditableSectionIndex,
        at: visibleBefore(before, arrayIndex),
        delta,
        survivorsAfter: visibleShotCount(before) + delta,
      });
    },
    [activeEditableSectionIndex, currentShots, onRowsChanged, stageShots]
  );

  /** Blank rows to insert, each with its own temp id. */
  const blankRows = useCallback(
    (count: number): EditableShot[] =>
      Array.from({ length: count }, () => {
        const tempId = nextTempIdRef.current++;
        return { ...blankEditableShot(-tempId), tempId };
      }),
    []
  );

  /** Append `count` blank shots in ONE update, each with its own temp id. */
  const handleAddShots = useCallback((count: number) => {
    const rows = blankRows(Math.max(1, count));
    // Past every row: an append cannot renumber a station that already exists.
    applyShotEdit(Number.POSITIVE_INFINITY, rows.length, (shots) => [...shots, ...rows]);
  }, [applyShotEdit, blankRows]);

  const handleAddShot = useCallback(() => handleAddShots(1), [handleAddShots]);

  /** Insert `count` blank shots at `index`, in ONE update with an id per row. */
  const handleInsertShots = useCallback(
    (index: number, count: number, position: 'before' | 'after') => {
      const at = position === 'after' ? index + 1 : index;
      const rows = blankRows(Math.max(1, count));
      applyShotEdit(at, rows.length, (shots) => {
        const next = [...shots];
        next.splice(at, 0, ...rows);
        return next;
      });
    },
    [applyShotEdit, blankRows]
  );

  /** Say so when a row cannot go: the wizard freezes the matched overlap rows,
   *  and refusing them in silence left them on screen while the grid cleared the
   *  selection as though they had been deleted. Same wording as the edit-section
   *  grid, which refuses for the same reason. */
  const refuseFrozenDelete = useCallback((count: number) => {
    useUIStore
      .getState()
      .showToast(t('editSection.frozenNotDeleted', { count }), TOAST_KEYS.DMP_WIZARD_FROZEN_DELETE, 'warning');
  }, [t]);

  const handleDeleteShot = useCallback((index: number) => {
    const shot = currentShots()[index];
    if (!shot || shot.isDeleted === true) return;
    if (frozenShotIndices.has(index)) {
      refuseFrozenDelete(1);
      return;
    }
    applyShotEdit(index, -1, (shots) => {
      const next = [...shots];
      const row = next[index];
      if (row) next[index] = { ...row, isDeleted: true };
      return next;
    });
  }, [applyShotEdit, currentShots, frozenShotIndices, refuseFrozenDelete]);

  /** Delete a selection as ONE decision. Asked one row at a time, the frozen
   *  rows were dropped without a word and the grid collapsed the selection as
   *  though the whole range had gone; the anchor was also re-anchored once per
   *  row, each time against a count that no longer held. */
  const handleDeleteShots = useCallback((indices: number[]): boolean => {
    const shots = currentShots();
    const requested = indices.filter((i) => shots[i] !== undefined && shots[i]!.isDeleted !== true);
    if (requested.length === 0) return false;
    const targets = requested.filter((i) => !frozenShotIndices.has(i));
    if (targets.length < requested.length) refuseFrozenDelete(requested.length - targets.length);
    if (targets.length === 0) return false;

    const doomed = new Set(targets);
    applyShotEdit(Math.min(...targets), -targets.length, (rows) =>
      rows.map((row, i) => (doomed.has(i) ? { ...row, isDeleted: true } : row)),
    );
    return true;
  }, [applyShotEdit, currentShots, frozenShotIndices, refuseFrozenDelete]);

  /**
   * Refuse while ANY section that will be sent holds a half-filled row.
   *
   * Every selected section's edits are serialized, not just the one on screen,
   * so a gate on the active section alone let a cleared cell in section B walk
   * past it (switching the active section asks nothing) and die at Rust's
   * deserializer with an error that named nothing. The section is named here for
   * the same reason.
   *
   * Read FRESH from the refs: the wizard's actions suppress exhaustive-deps, so
   * a captured count is stale exactly when it matters - clear a cell, click
   * Confirm without touching anything else, and the old callback still remembers
   * zero.
   */
  const refuseIncompleteShots = useCallback(() => {
    const live = new Map(sectionEditsRef.current);
    // The active section's newest rows reach `sectionEditsRef` an effect later.
    const active = editStateForSectionRef.current;
    if (active != null && editStateRef.current) live.set(active, editStateRef.current);
    const blocking = findIncompleteSections(selectedIndices, live)[0];
    if (!blocking) return false;
    useUIStore.getState().showToast(
      t('dmpWizard.toasts.sectionIncomplete', { name: blocking.name, count: blocking.count }),
      TOAST_KEYS.DMP_WIZARD_INCOMPLETE,
      'warning',
    );
    return true;
  }, [selectedIndices, t, editStateForSectionRef, editStateRef, sectionEditsRef]);

  return {
    editState,
    effectiveShots,
    frozenShotIndices,
    handleMetadataChange,
    handleShotChange,
    handleAddShot,
    handleAddShots,
    handleInsertShots,
    handleDeleteShot,
    handleDeleteShots,
    refuseIncompleteShots,
  };
}
