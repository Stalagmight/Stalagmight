/**
 * The DMP import wizard's RUN: every import the wizard sends, what each one
 * banks, and the withdrawal that undoes them (#576 D4, first slice).
 *
 * The wizard imports EAGERLY - the zero-datum anchor on mount, the reachable
 * sections on the way out of the suggestion phase (early), a segment the user
 * drops on the map, and whatever manual placement added at the end (late) -
 * because the map has to render what it is placing. Those four imports, the
 * per-run accumulators they bank into, and the withdrawal behind Cancel/Back
 * live here, headless; the dialog decides WHAT to send (`buildBatchImport`, the
 * overlap treatment) and what comes NEXT (its phases), and renders the outcome.
 *
 * A failure is typed by the step that failed (`DmpRunFailure.kind`), so the
 * dialog can put each one where the user is looking instead of sharing one
 * error string between every step.
 */
import { useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18n from '../../../i18n';
import type { ImportSectionConfig } from '../../../types/commands';
import type { ImportReceipt } from '../../../types/ipc/ImportReceipt';
import type { ImportOutcome } from '../../../types/ipc/ImportOutcome';
import type { CommandRefused } from '../../../store/commandResult';
import type { DmpFile } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { useProjectStore, projectSwitched, type ImportReceiptProvenance } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { extractFilename } from '../../../utils/path';
import { aggregateImportReceipts } from '../../../utils/importReceipt';
import { isEditGateCode, READ_ONLY_EDIT_NOTICE } from '../../../utils/editErrors';
import { NonFiniteShotError } from '../../../utils/importPayloadGuard';
import {
  backendExistingShotUpdatesForSections,
  removeExistingShotUpdatesForSections,
  type ExistingShotUpdateLocal,
} from '../../../utils/overlapTreatment';

/** The step of the run a failure belongs to. */
export type DmpRunStep = 'anchor' | 'early' | 'late' | 'segment' | 'suggestions';

export interface DmpRunFailure {
  kind: DmpRunStep;
  message: string;
}

/**
 * Why the Back/Cancel withdrawal did not complete: the backend refused the
 * batch (or it threw), the write-through save failed, or the run's project is
 * no longer the open one. Kept apart from `DmpRunFailure` (#576 NF-AN4-N2): it
 * is shown inside the confirmation that asked for the removal, and it must not
 * replace the import failure the wizard's panel (and its Retry) is about.
 */
export interface DmpWithdrawalFailure {
  kind: 'withdraw' | 'withdrawSave' | 'wrongProject';
  message: string;
}

/**
 * What a batch import sends: the configs and, position for position, the DMP
 * section each one was built from. `existingShotUpdates`, when present, is the
 * overlap treatment computed with this plan and replaces the run's pending one.
 */
export interface DmpImportPlan {
  configs: ImportSectionConfig[];
  emittedDmpIndices: number[];
  existingShotUpdates?: ExistingShotUpdateLocal[];
}

/**
 * The run's accumulators as they ride `SavedManualPlacingState` across the
 * manual-placement round trip: map placement CLOSES the wizard and reopens a
 * fresh instance, so anything not carried here dies with the old one.
 */
export interface DmpRunCarry {
  existingShotUpdates?: ExistingShotUpdateLocal[];
  /** DMP section index -> project `section_id`, for every section already imported. */
  earlyImported?: Array<[number, number]>;
  /**
   * Every `import_receipt` this wizard run has banked so far (#487).
   *
   * Without the carry the reopened instance completes the import with an empty
   * list, which aggregates to `undefined` and silently suppresses the receipt
   * the other import paths all show. Optional because a state saved by an older
   * build (or a test) has none; absent reads as "nothing banked yet".
   */
  importedReceipts?: ImportReceipt[];
  /**
   * Every project `section_id` this wizard run has created so far (#487
   * residue 2).
   *
   * Same carry, same reason, as `importedReceipts` above - and the same
   * consequence when it is missing: the reopened instance completes with an
   * empty list, App.tsx's `if (newSectionIds?.length)` is false, and
   * `suggest_orphan_placements` is never called. It is also what the withdrawal
   * needs to undo a run that crossed the handoff. Optional for the same reason.
   */
  importedSectionIds?: number[];
  /**
   * The `created_at` of the project those ids were created in.
   *
   * Nothing clears `dialogData` when the open project changes, so a saved state
   * can outlive the project it describes - and section ids are small
   * per-project integers, so every one of them would RESOLVE against whatever
   * is open instead. Carried so the imports and the withdrawal can refuse
   * rather than write into, or delete same-numbered sections from, the wrong
   * survey. Optional: absent means "unknown", which never refuses.
   */
  projectKey?: string | null;
}

/** A refusal from the payload guard is already written for the user; anything
 *  else reaching a catch here is a raw failure with no better wording. */
export function importErrorMessage(e: unknown): string {
  return e instanceof NonFiniteShotError ? e.message : String(e);
}

/**
 * [#576 RV2-FE-D3] The inline text for a wizard import the backend refused: a read-only
 * refusal (an edit-gate code - the lock lost mid-wizard, the Plan/Export workspace)
 * reads as the friendly read-only notice; anything else as the backend said it, or
 * `fallback` when it said nothing. The store's edit path is told not to toast the same
 * notice on top (`importSections`' `readOnlyNoticeInline`).
 */
function importFailureText(result: CommandRefused, fallback: string): string {
  return isEditGateCode(result.code) ? READ_ONLY_EDIT_NOTICE(result.code) : (result.error || fallback);
}

/**
 * [#576 RV2-FE-D1] Whether the project this run imports into (`runKey`, its
 * `created_at`) has been replaced by another. Checked before EVERY import, not only the
 * withdrawal: the store's own staleness guard captures the project when the import is
 * called, which is already the replacement once the run (its saved state rides
 * `dialogData`, which a project switch does not clear) has outlived a switch.
 */
function runProjectReplaced(runKey: string | null): boolean {
  return projectSwitched(runKey, useProjectStore.getState().project?.created_at ?? null);
}

/** What the user is told when the backend gives no reason of its own: the en.json
 *  key per step, rendered through `t()` (#576 RV5-LS-1). */
const FALLBACK_TEXT_KEY = {
  anchor: 'dialogs.dmpNewImport.anchorImportFailed',
  early: 'dialogs.dmpNewImport.importFailed',
  late: 'dialogs.dmpNewImport.lateImportFailed',
  segment: 'dialogs.dmpNewImport.segmentImportFailed',
} as const satisfies Record<'anchor' | 'early' | 'late' | 'segment', string>;

export function useDmpImportRun({
  dmpFile,
  selectionConfig,
  carried,
  importsOnMount = false,
}: {
  dmpFile: DmpFile;
  selectionConfig: DmpSelectionConfig;
  /** The run so far, when this instance reopens one (`SavedManualPlacingState`). */
  carried: DmpRunCarry | undefined;
  /** This instance starts by importing (a segment dropped on the map, #576 D13):
   *  busy from the first render, so neither exit nor the auto-start handoff can
   *  act before that import has answered. */
  importsOnMount?: boolean;
}) {
  const { t } = useTranslation();
  const importSections = useProjectStore((s) => s.importSections);
  const executeBatch = useProjectStore((s) => s.executeBatch);

  const [isImporting, setIsImporting] = useState(importsOnMount);
  const [failure, setFailure] = useState<DmpRunFailure | null>(null);
  const fail = useCallback((kind: DmpRunStep, message: string) => setFailure({ kind, message }), []);
  const clearFailure = useCallback(() => setFailure(null), []);

  const existingShotUpdatesRef = useRef<ExistingShotUpdateLocal[]>(carried?.existingShotUpdates ?? []);
  const earlyImportedRef = useRef<Map<number, number>>(new Map(carried?.earlyImported ?? []));
  // Seeded from the carry, with a sharper failure when it is not: an empty list
  // reaches App.tsx as "this import created nothing", which skips
  // `suggest_orphan_placements` altogether.
  const importedSectionIdsRef = useRef<number[]>([...(carried?.importedSectionIds ?? [])]);
  // `created_at` is the identity `executeCommand`'s own staleness guard uses:
  // immutable for the life of a project, unlike `file_path`. Read once.
  const projectKeyRef = useRef<string | null>(
    carried?.projectKey ?? useProjectStore.getState().project?.created_at ?? null
  );
  // One #391 summary for the whole run, aggregated at completion.
  const importedReceiptsRef = useRef<ImportReceipt[]>([...(carried?.importedReceipts ?? [])]);
  const filePathRef = useRef(
    (useUIStore.getState().dialogData as Record<string, unknown>).filePath as string | undefined
  );

  // Called per import, not built once: the file's parse warnings (#576 B13 - a
  // section cut short, a damaged record) lead the receipt, and a run's receipts
  // are concatenated at completion, so they ride the run's FIRST import only.
  // Nothing banked yet means this is it; the banked list survives the
  // map-placement reopen, so a reopened instance does not send them again.
  const provenance = useCallback((): ImportReceiptProvenance => ({
    sourceLabel: 'mNemo DMP',
    sourceFileName: extractFilename(filePathRef.current ?? ''),
    sectionsFound: dmpFile.sections.length,
    ...(importedReceiptsRef.current.length === 0 && (dmpFile.warnings?.length ?? 0) > 0
      ? { sourceWarnings: dmpFile.warnings }
      : {}),
  }), [dmpFile]);

  /** Bank what one successful `import_sections` created. */
  const bank = useCallback((result: ImportOutcome) => {
    if (result.section_ids.length > 0) importedSectionIdsRef.current.push(...result.section_ids);
    if (result.import_receipt) importedReceiptsRef.current.push(result.import_receipt);
  }, []);

  /** Record which project section each emitted DMP section became, and drop the
   *  overlap updates that went with them. Every batch records - the late one too
   *  (#576 NF-DMPS-2): a late import can leave a section unreachable, which sends
   *  the run back to manual placing and then into ANOTHER late import, whose
   *  batch is rebuilt from every placement so far and skips only what is
   *  recorded here - so an unrecorded late section was imported twice. */
  const settle = useCallback((emittedDmpIndices: number[], sectionIds: number[]) => {
    // Backend response ordering matches the submitted configs.
    emittedDmpIndices.forEach((dmpIndex, i) => {
      const sectionId = sectionIds[i];
      if (sectionId != null) earlyImportedRef.current.set(dmpIndex, sectionId);
    });
    existingShotUpdatesRef.current = removeExistingShotUpdatesForSections(
      existingShotUpdatesRef.current,
      new Set(emittedDmpIndices),
    );
  }, []);

  /**
   * Send one batch through the store's edit path (#576 D6): it refuses during a
   * move carry, serializes behind other edits, refreshes, marks dirty (local and
   * SpeleoDB), resolves a deferred first save and re-checks integrity. Refused
   * outright when the run's project has been replaced (D1). Returns the result
   * on success; on failure records it and returns null.
   */
  const send = useCallback(
    async (kind: 'anchor' | 'early' | 'late' | 'segment', configs: ImportSectionConfig[], emittedDmpIndices: number[] | null) => {
      if (runProjectReplaced(projectKeyRef.current)) {
        fail(kind, t('dialogs.dmpNewImport.importWrongProject'));
        return null;
      }
      const updates = emittedDmpIndices
        ? backendExistingShotUpdatesForSections(existingShotUpdatesRef.current, new Set(emittedDmpIndices))
        : [];
      // 'AllReferences', not Auto (#27): the DMP parser only ever emits Normal
      // shots, so every virtual shot reaching an import is one the user marked
      // Reference - the "Keep both (reference)" overlap treatment. Auto would
      // re-judge them by the chain rule and promote the bounded ones to Jump,
      // putting the duplicated geometry back into gas and penetration.
      const result = await importSections(
        configs, selectionConfig.fileHash, selectionConfig.sectionHashes, 'AllReferences', provenance(),
        updates.length > 0 ? updates : null, true,
      );
      if (!result.success) {
        fail(kind, importFailureText(result, t(FALLBACK_TEXT_KEY[kind])));
        return null;
      }
      bank(result);
      return result;
    },
    [bank, fail, importSections, provenance, selectionConfig.fileHash, selectionConfig.sectionHashes, t],
  );

  /**
   * Run one import step with the busy flag held and every throw - the payload
   * guard's refusal included - recorded as that step's failure. `isImporting`
   * is what blocks both exits (an import banks its ids only when it resolves),
   * so it must never be left set: try/finally on every path.
   */
  const step = useCallback(
    async <R>(kind: DmpRunStep, work: () => Promise<R | null>): Promise<R | null> => {
      setIsImporting(true);
      setFailure(null);
      try {
        return await work();
      } catch (e) {
        console.error(`[dmpImport] ${kind} import failed:`, e);
        fail(kind, importErrorMessage(e));
        return null;
      } finally {
        setIsImporting(false);
      }
    },
    [fail],
  );

  /** Import the zero-datum section, the anchor everything else hangs from. */
  const importAnchor = useCallback(
    (build: () => ImportSectionConfig) =>
      step('anchor', async () => ((await send('anchor', [build()], null)) ? true : null)),
    [send, step],
  );

  /**
   * The early import (everything reachable, on the way out of the suggestion
   * phase) or the late one (whatever manual placement added). Resolves to the
   * plan on success - the caller reads what was left unreachable from it - and
   * to null on failure.
   */
  const importBatch = useCallback(
    <P extends DmpImportPlan>(kind: 'early' | 'late', plan: () => P) =>
      step(kind, async () => {
        const p = plan();
        if (p.existingShotUpdates) existingShotUpdatesRef.current = p.existingShotUpdates;
        if (p.configs.length === 0) return p;
        const result = await send(kind, p.configs, p.emittedDmpIndices);
        if (!result) return null;
        settle(p.emittedDmpIndices, result.section_ids);
        return p;
      }),
    [send, settle, step],
  );

  /**
   * Import the segment just placed on the map. `plan` is the whole batch as it
   * stands with that placement added; only the segment's own sections are sent,
   * with a relative target that is not part of the segment resolved to the
   * project section it already became.
   */
  const importSegment = useCallback(
    (segment: number[], plan: () => DmpImportPlan) =>
      step('segment', async () => {
        const { configs, emittedDmpIndices } = plan();
        const pending = new Set(segment);
        const segmentConfigs: ImportSectionConfig[] = [];
        const segmentDmpIndices: number[] = [];
        // Original config position -> position in segmentConfigs.
        const posRemap = new Map<number, number>();
        configs.forEach((config, i) => {
          const dmpIndex = emittedDmpIndices[i];
          if (dmpIndex != null && pending.has(dmpIndex)) {
            posRemap.set(i, segmentConfigs.length);
            segmentConfigs.push({ ...config, section_index: segmentConfigs.length });
            segmentDmpIndices.push(dmpIndex);
          }
        });

        for (const cfg of segmentConfigs) {
          if (cfg.placement.type !== 'from_importing') continue;
          const placement = cfg.placement;
          const newTarget = posRemap.get(placement.target_section_index);
          if (newTarget != null) {
            cfg.placement = { ...placement, target_section_index: newTarget };
            continue;
          }
          const targetDmpIndex = emittedDmpIndices[placement.target_section_index];
          const existingTargetId = targetDmpIndex != null ? earlyImportedRef.current.get(targetDmpIndex) : undefined;
          if (existingTargetId == null) {
            // The module's i18n, not the hook's `t`: as a dependency it would rebuild
            // this callback whenever `t` changes identity (#576 RV5-LS-1).
            fail('segment', i18n.t('dialogs.dmpNewImport.segmentTargetUnresolved'));
            return null;
          }
          cfg.placement = {
            type: 'from_existing',
            import_station: placement.import_station,
            existing_station: { section_id: existingTargetId, station_index: placement.target_station_index },
          };
        }

        if (segmentConfigs.length === 0) return true;
        // Through the store's edit path like the other three (#576 D6/D13): the
        // wizard is mounted while it runs, so its failure is shown inline and
        // the store's catch-all toast yields to the open dialog.
        const result = await send('segment', segmentConfigs, segmentDmpIndices);
        if (!result) return null;
        settle(segmentDmpIndices, result.section_ids);
        return true;
      }),
    [fail, send, settle, step],
  );

  // ─── Withdrawal (Cancel / Back) ─────────────────────────────────────────────
  //
  // By the time either exit is reached the project may already hold sections
  // the user never agreed to keep (#394 F2), so both offer to WITHDRAW what was
  // imported, as one batched `delete_section` - one undo entry for one action.

  /** A withdrawal already running. A REF, not just the state mirror below: two
   *  clicks in the same tick both read the pre-`setState` value, and the second
   *  must not send a second batch for ids the first one is already deleting. */
  const withdrawInFlightRef = useRef(false);
  const [withdrawInFlight, setWithdrawInFlight] = useState(false);
  const [withdrawalFailure, setWithdrawalFailure] = useState<DmpWithdrawalFailure | null>(null);
  const clearWithdrawalFailure = useCallback(() => setWithdrawalFailure(null), []);

  /** The removal landed in memory but is not yet on disk. Survives a failed
   *  save so a second Remove click RETRIES THE SAVE instead of finding an empty
   *  id list, returning true, and closing over the stale file it just refused to
   *  close over. */
  const withdrawalNeedsSaveRef = useRef(false);

  /** Remove every section this wizard run imported, as ONE undoable step, and
   *  leave the result on disk.
   *  Returns false (leaving the accumulators untouched) if it refused, the
   *  backend did, or the follow-up save did, so the caller can keep the wizard
   *  open on a visible error instead of closing over a project it failed to
   *  clean up. */
  const withdrawImportedSections = useCallback(async (): Promise<boolean> => {
    if (withdrawInFlightRef.current) return false;
    // DEDUPLICATED: this accumulator is seeded from the saved state AND appended
    // to by live imports, so one id can appear twice across the map-placement
    // handoff. `execute_batch` is all-or-nothing and the second delete of an id
    // fails with "not found", which rolls the WHOLE batch back - the withdrawal
    // would remove nothing at all and report an error for it.
    const sectionIds = [...new Set(importedSectionIdsRef.current)];
    if (sectionIds.length === 0 && !withdrawalNeedsSaveRef.current) return true;
    // Section ids are small per-project integers, so a withdrawal aimed at a
    // project that is no longer open would delete same-numbered sections in
    // whatever replaced it - and every id would RESOLVE, so nothing downstream
    // fails safe. Nothing clears `dialogData` on a project switch
    // (`resetToolTransientsForProjectSwitch` leaves the dialog slot alone), so
    // a saved state really can outlive its project. This is the refusal.
    const openProjectKey = useProjectStore.getState().project?.created_at ?? null;
    if (projectSwitched(projectKeyRef.current, openProjectKey)) {
      setWithdrawalFailure({ kind: 'wrongProject', message: t('dialogs.dmpNewImport.removeImportedWrongProject') });
      return false;
    }
    setWithdrawalFailure(null);
    withdrawInFlightRef.current = true;
    setWithdrawInFlight(true);
    setIsImporting(true);
    try {
      if (sectionIds.length > 0) {
        const fileHashes = selectionConfig.fileHash ? [selectionConfig.fileHash] : [];
        const sectionHashes = selectionConfig.sectionHashes ?? [];
        const result = await executeBatch([
          ...sectionIds.map((sectionId) => ({ type: 'delete_section', section_id: sectionId }) as const),
          // In the SAME batch, and that is the point of batching it: deleting a
          // section does not touch the import-dedup hashes `import_sections`
          // recorded (they are project-level bookkeeping, not references), so
          // without this the project still claims this file was imported while
          // none of it remains - and `DmpSelectionDialog` DESELECTS every section
          // it believes is a duplicate, so the obvious retry silently offers
          // nothing to import. One batch, so one undo restores the sections and
          // their tracking together.
          //
          // Only the hashes THIS run sent to `import_sections`: `sectionHashes`
          // holds the selected sections alone (duplicates were auto-deselected
          // before the wizard opened), so an earlier session's tracking is out of
          // reach. The file hash is coarser - one per file - so a file imported
          // in an earlier session too loses its file-level duplicate flag. That
          // is the intended direction: after a withdrawal the user may well want
          // to re-import it, and the per-section flags still stand.
          ...(fileHashes.length > 0 || sectionHashes.length > 0
            ? [{
                type: 'untrack_import',
                file_hashes: fileHashes,
                section_hashes: sectionHashes,
              } as const]
            : []),
        ]);
        if (!result.success) {
          setWithdrawalFailure({ kind: 'withdraw', message: result.error ?? t('dialogs.dmpNewImport.removeImportedFailed') });
          return false;
        }

        // DIFFED, not cleared: an import that resolved while the batch was in
        // flight has pushed an id the batch never named, and that section is
        // still in the project. Wiping the list would drop it from tracking
        // while it survives on disk - the very shape F2 exists to prevent. (The
        // handlers refuse to start a withdrawal mid-import, so this is the
        // second line of defence, not the first.)
        const removed = new Set(sectionIds);
        importedSectionIdsRef.current = importedSectionIdsRef.current.filter((id) => !removed.has(id));
        for (const dmpIndex of [...earlyImportedRef.current.keys()]) {
          const sectionId = earlyImportedRef.current.get(dmpIndex);
          if (sectionId != null && removed.has(sectionId)) earlyImportedRef.current.delete(dmpIndex);
        }
        // Receipts are per-`import_sections` CALL and carry no section id, so
        // they cannot be diffed - only safe to drop once nothing survives.
        if (importedSectionIdsRef.current.length === 0) importedReceiptsRef.current = [];

        // #394 F14. The eager anchor import already RESOLVED this project's
        // deferred first save (`finalize_deferred_import`), so the file on disk
        // still holds the sections just withdrawn. It has to be written through.
        //
        // Gated as `saveAfterImport` gates its own save, for its reason:
        // importing INTO AN EXISTING project the user may have unrelated pending
        // edits, and committing those behind their back is not this action's to
        // do. There the withdrawal leaves the project dirty and autosave / the
        // close guard carry it, as for any other edit.
        withdrawalNeedsSaveRef.current = useUIStore.getState().importContext === 'new-project';
      }

      // The save is NOT best-effort: closing here is what makes the removal
      // final from the user's side, and a `.stalx` still holding the sections
      // they chose to remove is the exact F2 shape one layer out. So a failed
      // write keeps the wizard open with the reason on screen, and
      // `withdrawalNeedsSaveRef` makes the next Remove click retry the SAVE -
      // without it the retry would find an empty id list, return true, and close
      // over the very file the first attempt refused to close over.
      if (withdrawalNeedsSaveRef.current) {
        const saved = await useProjectStore.getState().saveProject();
        if (!saved.success) {
          setWithdrawalFailure({
            kind: 'withdrawSave',
            message: saved.error ?? t('dialogs.dmpNewImport.removeImportedSaveFailed'),
          });
          return false;
        }
        withdrawalNeedsSaveRef.current = false;
      }
      return true;
    } catch (e) {
      setWithdrawalFailure({ kind: 'withdraw', message: importErrorMessage(e) });
      return false;
    } finally {
      withdrawInFlightRef.current = false;
      setWithdrawInFlight(false);
      setIsImporting(false);
    }
  }, [executeBatch, selectionConfig.fileHash, selectionConfig.sectionHashes, t]);

  /** How many sections the run has put in the project so far. */
  const importedCount = useCallback(() => importedSectionIdsRef.current.length, []);

  /** `onComplete`'s arguments: every section the run created, and one receipt for the run. */
  const completion = useCallback(
    (): [number[], ImportReceipt | undefined] => [
      importedSectionIdsRef.current,
      aggregateImportReceipts(importedReceiptsRef.current),
    ],
    [],
  );

  /** The run's share of `SavedManualPlacingState`, read live. */
  const carry = useCallback(
    (): DmpRunCarry => ({
      existingShotUpdates: [...existingShotUpdatesRef.current],
      earlyImported: Array.from(earlyImportedRef.current.entries()),
      importedReceipts: [...importedReceiptsRef.current],
      importedSectionIds: [...importedSectionIdsRef.current],
      projectKey: projectKeyRef.current,
    }),
    [],
  );

  return {
    isImporting,
    failure,
    fail,
    clearFailure,
    withdrawInFlight,
    withdrawalFailure,
    clearWithdrawalFailure,
    /** DMP section index -> project section id, live (`buildBatchImport`'s `alreadyImported`). */
    earlyImported: earlyImportedRef.current,
    importedCount,
    importAnchor,
    importBatch,
    importSegment,
    withdrawImportedSections,
    completion,
    carry,
  };
}

export type DmpImportRun = ReturnType<typeof useDmpImportRun>;
