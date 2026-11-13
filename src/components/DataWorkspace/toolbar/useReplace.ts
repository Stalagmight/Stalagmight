/**
 * Replace / Replace all for the find bar. Every replace is ONE batch through
 * `buildEditCommands` -> `executeBatch` (one undo step), locked cells are
 * skipped and counted, and values the column refuses are reported.
 */
import { useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import { buildEditCommands } from '../model/edits';
import { planReplace, type ReplacePlan, type ReplaceSpec } from '../model/replace';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { isEditingBlockedNow } from '../../../hooks/useEditingBlocked';
import { TOAST_KEYS } from '../../../utils/toastKeys';
import { formatNumber } from '../../../i18n/format';
import type { DataModel } from '../DataModelContext';
import type { DataCellEdit, DataLockReason, DataView } from '../types';
import { columnLabel, lockReasonText } from './labels';
import type { FindNavigation } from './useFindNavigation';

/** What "Replace all" would do, for the confirmation. */
export interface ReplaceAllPreview {
  matches: number;
  cells: number;
  lockedMatches: number;
  invalidCells: number;
  /** Number cells shown rounded, written as shown (see ReplacePlan.roundedCells). */
  roundedCells: number;
  /** The search hit its match cap: only the first matches are included. */
  limited: boolean;
}

export interface ReplaceOutcome {
  replaced: number;
  locked: number;
  invalid: number;
}

/** Fold `buildEditCommands`' own skips back into match counts. */
export function settleOutcome(plan: ReplacePlan, skipped: readonly { edit: DataCellEdit; reason: string }[]): ReplaceOutcome {
  const idx = new Map(plan.edits.map((e, i) => [e, i]));
  let replaced = plan.replacedMatches;
  let locked = plan.lockedMatches;
  let invalid = plan.invalid.length;
  for (const s of skipped) {
    const n = plan.editMatches[idx.get(s.edit) ?? -1] ?? 0;
    replaced -= n;
    if (s.reason === 'invalid') invalid += 1;
    else locked += n;
  }
  return { replaced, locked, invalid };
}

export function useReplace(view: DataView, model: DataModel, nav: FindNavigation) {
  const { t, i18n } = useTranslation();
  const [busy, setBusy] = useState(false);
  const viewRef = useRef(view);
  viewRef.current = view;
  const modelRef = useRef(model);
  modelRef.current = model;

  const toast = useCallback(
    (message: string, kind: 'success' | 'warning' | 'error' | 'info') =>
      useUIStore.getState().showToast(message, TOAST_KEYS.DATA_WORKSPACE_REPLACE, kind),
    [],
  );

  const blocked = useCallback(() => {
    if (!modelRef.current.ctx.editingBlocked && !isEditingBlockedNow()) return false;
    toast(t('dataToolbar.replace.blocked'), 'warning');
    return true;
  }, [t, toast]);

  const spec = (): ReplaceSpec => {
    const s = useDataWorkspaceStore.getState().search;
    return { query: s.query, replacement: s.replacement, matchCase: s.matchCase, wholeWord: s.wholeWord, regex: s.regex };
  };

  /** Execute a plan as one batch. null = the batch failed (already toasted). */
  const run = useCallback(
    async (plan: ReplacePlan): Promise<{ outcome: ReplaceOutcome; skipReason: string | null } | null> => {
      const { ctx } = modelRef.current;
      const editPlan = buildEditCommands(plan.edits, ctx);
      const outcome = settleOutcome(plan, editPlan.skipped);
      const skipReason = editPlan.skipped[0]?.reason ?? null;
      if (editPlan.commands.length === 0) return { outcome, skipReason };
      setBusy(true);
      try {
        const result = await useProjectStore.getState().executeBatch(editPlan.commands);
        if (!result.success) {
          toast(t('dataToolbar.replace.failed', { error: result.error ?? '' }), 'error');
          return null;
        }
        return { outcome, skipReason };
      } catch (e) {
        toast(t('dataToolbar.replace.failed', { error: e instanceof Error ? e.message : String(e) }), 'error');
        return null;
      } finally {
        setBusy(false);
      }
    },
    [t, toast],
  );

  /** Replace the current match only, then move to the next one. */
  const replaceOne = useCallback(async () => {
    if (busy || blocked()) return;
    const { matches } = viewRef.current;
    const m = matches[useDataWorkspaceStore.getState().currentMatch];
    if (!m) {
      nav.next();
      return;
    }
    const { ctx, rowByKey, columnByKey } = modelRef.current;
    if (m.lockReason !== null) {
      toast(t('dataToolbar.replace.lockedOne', { reason: lockReasonText(t, m.lockReason) }), 'info');
      nav.next();
      return;
    }
    const plan = planReplace([m], { rowByKey, columnByKey }, ctx, spec());
    const bad = plan.invalid[0];
    if (bad) {
      const col = columnByKey.get(bad.colKey);
      toast(
        t('dataToolbar.replace.invalidOne', { text: bad.text, column: col ? columnLabel(t, i18n, col) : bad.colKey }),
        'warning',
      );
      return;
    }
    const texts = plan.editTexts[0];
    if (!texts) {
      nav.next();
      return;
    }
    // Resume right after the replaced text once the matches are recomputed, so
    // a replacement that still contains the query ("a" -> "aa") is not hit again.
    // An empty replacement resumes AT the old start (the next match may now begin there).
    const replacedLen = m.end - m.start + (texts.after.length - texts.before.length);
    nav.setAnchor({ rowKey: m.rowKey, colKey: m.colKey, start: m.start + Math.max(0, replacedLen) });
    const done = await run(plan);
    if (!done) {
      nav.setAnchor(null);
      return;
    }
    if (done.outcome.replaced === 0) {
      nav.setAnchor(null);
      const reason = done.skipReason;
      if (reason && reason !== 'invalid') {
        toast(t('dataToolbar.replace.lockedOne', { reason: lockReasonText(t, reason as DataLockReason) }), 'info');
      }
      nav.next();
    }
  }, [busy, blocked, nav, run, t, i18n, toast]);

  /** Count what Replace all would do (null + a toast when nothing can be replaced). */
  const previewAll = useCallback((): ReplaceAllPreview | null => {
    if (blocked()) return null;
    const { ctx, rowByKey, columnByKey } = modelRef.current;
    const v = viewRef.current;
    const plan = planReplace(v.matches, { rowByKey, columnByKey }, ctx, spec());
    if (plan.edits.length === 0) {
      toast(t('dataToolbar.replace.nothingReplaceable'), 'info');
      return null;
    }
    return {
      matches: plan.replacedMatches,
      cells: plan.edits.length,
      lockedMatches: plan.lockedMatches,
      invalidCells: plan.invalid.length,
      roundedCells: plan.roundedCells,
      limited: v.matchLimitReached === true,
    };
  }, [blocked, t, toast]);

  const replaceAll = useCallback(async () => {
    if (busy || blocked()) return;
    const { ctx, rowByKey, columnByKey } = modelRef.current;
    const plan = planReplace(viewRef.current.matches, { rowByKey, columnByKey }, ctx, spec());
    const done = await run(plan);
    if (!done) return;
    const { outcome } = done;
    const parts: string[] = [];
    parts.push(
      outcome.replaced > 0
        ? t('dataToolbar.replace.summaryReplaced', { n: formatNumber(outcome.replaced) })
        : t('dataToolbar.replace.summaryNone'),
    );
    if (outcome.locked > 0) parts.push(t('dataToolbar.replace.summaryLocked', { n: formatNumber(outcome.locked) }));
    if (outcome.invalid > 0) parts.push(t('dataToolbar.replace.summaryInvalid', { n: formatNumber(outcome.invalid) }));
    const kind = outcome.replaced > 0 && outcome.locked === 0 && outcome.invalid === 0 ? 'success' : 'warning';
    toast(parts.join(' · '), kind);
  }, [busy, blocked, run, t, toast]);

  return { replaceOne, previewAll, replaceAll, busy };
}
