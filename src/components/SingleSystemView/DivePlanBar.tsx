import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import {
  selectHasUnresolvedPins,
  selectIsPlanDirty,
  useGasPlanStore,
} from '../../store/gasPlanStore';
import { useProjectStore } from '../../store/projectStore';
import { expectedOpenProject } from '../../store/expectedOpenProject';
import { projectSessionEpoch } from '../../store/projectSession';
import { Select } from '../common/Select';
import { divePlanSnapshotOf } from '../../utils/divePlan';
import {
  deleteDivePlan,
  listDivePlans,
  loadDivePlan,
  planAnchorsOf,
  saveDivePlan,
} from '../../utils/divePlanApi';
import { projectChangedMessage } from '../../utils/projectChangedNotice';
import type { DivePlanMeta } from '../../types/ipc.generated';

/**
 * The open project's identity, read when a request is made and again when its answer
 * arrives: an answer made for another project is dropped, success or failure (#244).
 * The gas store mutations (apply/markSaved/clear) survive an unmount, so a reply that
 * resolves after the user opened ANOTHER project would corrupt the new project's store.
 * Composite key (mirrors prevProjectKeyRef below): created_at alone doesn't change on a
 * Save-As, but the .plnx sibling path is derived from file_path, so a Save-As mid-flight
 * must also be treated as a project-identity change here.
 *
 * [#576 W60-NF2] Plus the project session (`projectSession.ts`, moved by every load or
 * create), and checked on failures and the plan list too: the bar stays mounted while a
 * load answered ALREADY_OPEN re-reads another project (`loadProject`), and the previous
 * project's error or plan list landed on the new project's bar. Not `modified_at` (as
 * useMapTools' key has): a project-settings save, allowed in Plan, moves it.
 */
function projectKey(): string {
  const p = useProjectStore.getState().project;
  return `${projectSessionEpoch()}|${p?.created_at ?? ''}|${p?.file_path ?? ''}`;
}

/** Inline confirm/prompt state for the bar (kept inline to avoid registering a new
 *  dialog type + its telemetry events; #244). */
type Pending =
  | { kind: 'none' }
  | { kind: 'saveAs' }
  | { kind: 'delete' }
  | { kind: 'switch'; id: number };

/**
 * "Previous dive plans" bar (#244) at the top of the Plan-mode gas panel: a
 * dropdown to load a saved plan plus Save / Save as... / Delete. A plan is the
 * `compute_gas_reach_field` input tuple; loading applies it to `gasPlanStore` and
 * the existing MapCanvas effect recomputes the overlay. Dirty tracking + the
 * app-close and mode-switch guards live in the store / App / ModeCarousel.
 */
export function DivePlanBar() {
  const { t } = useTranslation();
  const { loadedPlanId, applyLoadedPlan, markPlanSaved, clearLoadedPlan } = useGasPlanStore(
    useShallow((s) => ({
      loadedPlanId: s.loadedPlanId,
      applyLoadedPlan: s.applyLoadedPlan,
      markPlanSaved: s.markPlanSaved,
      clearLoadedPlan: s.clearLoadedPlan,
    }))
  );
  const isDirty = useGasPlanStore(selectIsPlanDirty);
  // #310: a pin the backend can no longer place must not be written into a `.plnx`. The backend's
  // own `to_persisted` refuses it too, but that refusal arrives as an error string after the diver
  // has already named the plan; blocking here says which plan is unsavable and why, up front.
  const pinsUnresolved = useGasPlanStore(selectHasUnresolvedPins);
  // The .plnx sits next to the .stalx, so plans need a saved project (a file path).
  // A deferred new project HAS a reserved file_path that was never written,
  // and the backend refuses .plnx persistence until the first real save
  // (#275 B3). databaseOpen is the purpose-built signal for that state
  // (false while the initial save is deferred), so gate on BOTH: path
  // reserved AND database actually on disk.
  const hasPath = useProjectStore((s) => Boolean(s.project?.file_path) && s.databaseOpen);
  // created_at is the stable project identity; changing project => refetch.
  const projectCreatedAt = useProjectStore((s) => s.project?.created_at ?? null);
  // The backend targets the .plnx sibling of file_path; a project Save-As moves it in place,
  // so the loaded plan + dropdown must re-key to the new location (#244).
  const projectFilePath = useProjectStore((s) => s.project?.file_path ?? null);

  const [plans, setPlans] = useState<DivePlanMeta[]>([]);
  const [pending, setPending] = useState<Pending>({ kind: 'none' });
  const [saveAsName, setSaveAsName] = useState('');
  const [staleCount, setStaleCount] = useState(0);
  // #576 NF-UI-2: span speeds the last load dropped as out of range (one notice).
  const [droppedSpeedCount, setDroppedSpeedCount] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    const key = projectKey();
    listDivePlans()
      .then((list) => {
        if (projectKey() === key) setPlans(list);
      })
      .catch((e: unknown) => {
        if (projectKey() === key) setError(String(e));
      });
  }, []);

  // Refetch on mount (panel mounts when Plan mode opens) and whenever the project's identity
  // or .plnx location changes. On an actual CHANGE (project switch / Save-As) the loaded plan is
  // stale, so drop it - but NOT on a fresh mount (a fresh ref reads null), so the plan survives
  // a Plan<->Survey<->Export re-entry (#244). project switch also resets the store upstream.
  const prevProjectKeyRef = useRef<string | null>(null);
  useEffect(() => {
    const key = `${projectCreatedAt ?? ''}|${projectFilePath ?? ''}`;
    if (prevProjectKeyRef.current !== null && prevProjectKeyRef.current !== key) {
      clearLoadedPlan();
      setStaleCount(0);
      setDroppedSpeedCount(0);
    }
    prevProjectKeyRef.current = key;
    refresh();
  }, [refresh, clearLoadedPlan, projectCreatedAt, projectFilePath]);

  const loadedName = plans.find((p) => p.id === loadedPlanId)?.name ?? null;

  // Each request captures the project identity (`projectKey`) and drops its answer if it
  // moved, success or failure (#244, #576 W60-NF2).
  const doLoad = useCallback(
    async (id: number) => {
      setError(null);
      setBusy(true);
      const key = projectKey();
      try {
        const plan = await loadDivePlan(id);
        if (projectKey() !== key) return; // project switched mid-flight
        const { droppedSpeeds } = applyLoadedPlan(id, plan.snapshot, planAnchorsOf(plan));
        setStaleCount(plan.stale_ref_count);
        setDroppedSpeedCount(droppedSpeeds);
      } catch (e) {
        if (projectKey() !== key) return; // [#576 W60-NF2] not this project's error
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [applyLoadedPlan]
  );

  const onSelect = (value: string) => {
    setError(null);
    setPending({ kind: 'none' });
    if (value === '') {
      setStaleCount(0);
      setDroppedSpeedCount(0);
      return;
    }
    const id = Number(value);
    if (id === loadedPlanId) return;
    // Guard losing unsaved edits to the currently-loaded plan (#244).
    if (isDirty) {
      setPending({ kind: 'switch', id });
    } else {
      void doLoad(id);
    }
  };

  const doSave = useCallback(async () => {
    if (loadedPlanId === null || loadedName === null) return;
    setError(null);
    setBusy(true);
    const key = projectKey();
    // [#576 RVF-c-C1] The project the plan is saved for, named on the IPC.
    const madeFor = expectedOpenProject();
    // Capture the exact snapshot once so the persisted bytes and the new dirty-baseline agree
    // even if the user edits a field during the async save (#244).
    const snap = divePlanSnapshotOf(useGasPlanStore.getState());
    try {
      const meta = await saveDivePlan(loadedPlanId, loadedName, snap, madeFor);
      if (projectKey() !== key) return;
      markPlanSaved(meta.id, snap);
      refresh();
    } catch (e) {
      if (projectKey() !== key) return; // [#576 W60-NF2] not this project's error
      // [#576 Q-W44-7-D1] Refused because the project changed: the short line instead.
      setError(projectChangedMessage(e) ?? String(e));
    } finally {
      setBusy(false);
    }
  }, [loadedPlanId, loadedName, markPlanSaved, refresh]);

  const doSaveAs = useCallback(async () => {
    const name = saveAsName.trim();
    if (!name) return;
    setError(null);
    setBusy(true);
    const key = projectKey();
    const madeFor = expectedOpenProject();
    const snap = divePlanSnapshotOf(useGasPlanStore.getState());
    try {
      const meta = await saveDivePlan(null, name, snap, madeFor);
      if (projectKey() !== key) return;
      markPlanSaved(meta.id, snap);
      setSaveAsName('');
      setPending({ kind: 'none' });
      refresh();
    } catch (e) {
      if (projectKey() !== key) return; // [#576 W60-NF2] not this project's error
      setError(projectChangedMessage(e) ?? String(e));
    } finally {
      setBusy(false);
    }
  }, [saveAsName, markPlanSaved, refresh]);

  const doDelete = useCallback(async () => {
    if (loadedPlanId === null) return;
    setError(null);
    setBusy(true);
    const key = projectKey();
    const madeFor = expectedOpenProject();
    try {
      await deleteDivePlan(loadedPlanId, madeFor);
      if (projectKey() !== key) return;
      clearLoadedPlan();
      setPending({ kind: 'none' });
      setStaleCount(0);
      setDroppedSpeedCount(0);
      refresh();
    } catch (e) {
      if (projectKey() !== key) return; // [#576 W60-NF2] not this project's error
      setError(projectChangedMessage(e) ?? String(e));
    } finally {
      setBusy(false);
    }
  }, [loadedPlanId, clearLoadedPlan, refresh]);

  const canSave =
    loadedPlanId !== null && loadedName !== null && isDirty && hasPath && !busy && !pinsUnresolved;
  const canSaveAs = hasPath && !busy && !pinsUnresolved;
  const canDelete = loadedPlanId !== null && !busy;

  return (
    <section className="space-y-1" data-testid="dive-plan-bar">
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium opacity-80" id="dive-plan-select-label">
          {t('divePlanBar.label')}
        </span>
        {loadedPlanId !== null && isDirty && (
          <span className="text-xs text-warning" data-testid="dive-plan-dirty">
            {t('divePlanBar.unsavedBadge')}
          </span>
        )}
      </div>

      <div className="flex items-center gap-1">
        <Select
          data-testid="dive-plan-select"
          className="flex-1 min-w-0 text-sm"
          value={loadedPlanId !== null ? String(loadedPlanId) : ''}
          onChange={(v) => onSelect(v)}
          options={[
            { value: '', label: plans.length === 0 ? t('divePlanBar.empty') : t('divePlanBar.choose') },
            ...plans.map((p) => ({ value: String(p.id), label: p.name })),
          ]}
          disabled={busy}
          aria-labelledby="dive-plan-select-label"
        />
      </div>

      <div className="flex items-center gap-1">
        <button
          type="button"
          data-testid="dive-plan-save"
          className="px-2 py-1 text-xs rounded border border-border hover:bg-panel disabled:opacity-40 disabled:cursor-not-allowed"
          onClick={() => void doSave()}
          disabled={!canSave}
          title={hasPath ? undefined : t('divePlanBar.needsSavedProject')}
        >
          {t('divePlanBar.save')}
        </button>
        <button
          type="button"
          data-testid="dive-plan-save-as"
          className="px-2 py-1 text-xs rounded border border-border hover:bg-panel disabled:opacity-40 disabled:cursor-not-allowed"
          onClick={() => {
            setError(null);
            setPending({ kind: 'saveAs' });
            setSaveAsName('');
          }}
          disabled={!canSaveAs}
          title={hasPath ? undefined : t('divePlanBar.needsSavedProject')}
        >
          {t('divePlanBar.saveAs')}
        </button>
        <button
          type="button"
          data-testid="dive-plan-delete"
          className="px-2 py-1 text-xs rounded border border-border hover:bg-panel disabled:opacity-40 disabled:cursor-not-allowed"
          onClick={() => {
            setError(null);
            setPending({ kind: 'delete' });
          }}
          disabled={!canDelete}
        >
          {t('divePlanBar.delete')}
        </button>
      </div>

      {!hasPath && (
        <p className="text-xs opacity-70" data-testid="dive-plan-needs-save">
          {t('divePlanBar.needsSavedProject')}
        </p>
      )}

      {pinsUnresolved && (
        <p
          className="text-xs p-2 rounded bg-error-10 border border-error-30 text-error"
          data-testid="dive-plan-unresolved-pins"
        >
          {t('divePlanBar.unresolvedPins')}
        </p>
      )}

      {pending.kind === 'saveAs' && (
        <div className="flex items-center gap-1" data-testid="dive-plan-save-as-row">
          <input
            type="text"
            data-testid="dive-plan-name-input"
            className="flex-1 min-w-0 bg-input border border-border rounded px-2 py-1 text-sm"
            placeholder={t('divePlanBar.namePlaceholder')}
            value={saveAsName}
            onChange={(e) => setSaveAsName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void doSaveAs();
              if (e.key === 'Escape') setPending({ kind: 'none' });
            }}
            autoFocus
          />
          <button
            type="button"
            data-testid="dive-plan-save-as-confirm"
            className="px-2 py-1 text-xs rounded border border-border hover:bg-panel disabled:opacity-40"
            onClick={() => void doSaveAs()}
            disabled={saveAsName.trim() === '' || busy}
          >
            {t('divePlanBar.save')}
          </button>
          <button
            type="button"
            data-testid="dive-plan-save-as-cancel"
            className="px-2 py-1 text-xs rounded border border-border hover:bg-panel"
            onClick={() => setPending({ kind: 'none' })}
          >
            {t('common.actions.cancel')}
          </button>
        </div>
      )}

      {pending.kind === 'delete' && (
        <div
          className="flex items-center justify-between gap-2 p-2 rounded bg-warning-10 border border-warning-30 text-warning text-xs"
          data-testid="dive-plan-delete-confirm"
        >
          <span>{t('divePlanBar.deleteConfirm', { name: loadedName ?? '' })}</span>
          <span className="flex gap-1 shrink-0">
            <button
              type="button"
              data-testid="dive-plan-delete-confirm-yes"
              className="px-2 py-0.5 rounded border border-warning-30 hover:bg-warning-10"
              onClick={() => void doDelete()}
            >
              {t('divePlanBar.delete')}
            </button>
            <button
              type="button"
              data-testid="dive-plan-delete-confirm-cancel"
              className="px-2 py-0.5 rounded border border-border hover:bg-panel"
              onClick={() => setPending({ kind: 'none' })}
            >
              {t('common.actions.cancel')}
            </button>
          </span>
        </div>
      )}

      {pending.kind === 'switch' && (
        <div
          className="flex items-center justify-between gap-2 p-2 rounded bg-warning-10 border border-warning-30 text-warning text-xs"
          data-testid="dive-plan-switch-confirm"
        >
          <span>{t('divePlanBar.discardConfirm', { name: loadedName ?? '' })}</span>
          <span className="flex gap-1 shrink-0">
            <button
              type="button"
              data-testid="dive-plan-switch-confirm-yes"
              className="px-2 py-0.5 rounded border border-warning-30 hover:bg-warning-10"
              onClick={() => {
                const { id } = pending;
                setPending({ kind: 'none' });
                void doLoad(id);
              }}
            >
              {t('divePlanBar.discard')}
            </button>
            <button
              type="button"
              data-testid="dive-plan-switch-confirm-cancel"
              className="px-2 py-0.5 rounded border border-border hover:bg-panel"
              onClick={() => setPending({ kind: 'none' })}
            >
              {t('common.actions.cancel')}
            </button>
          </span>
        </div>
      )}

      {staleCount > 0 && (
        <p
          className="text-xs p-2 rounded bg-warning-10 border border-warning-30 text-warning"
          data-testid="dive-plan-stale-warning"
        >
          {t('divePlanBar.staleRefs', { count: staleCount })}
        </p>
      )}

      {droppedSpeedCount > 0 && (
        <p
          className="text-xs p-2 rounded bg-warning-10 border border-warning-30 text-warning"
          data-testid="dive-plan-speeds-dropped-warning"
        >
          {t('divePlanBar.speedsDropped', { count: droppedSpeedCount })}
        </p>
      )}

      {error && (
        <p className="text-xs text-error" data-testid="dive-plan-error">
          {error}
        </p>
      )}
    </section>
  );
}
