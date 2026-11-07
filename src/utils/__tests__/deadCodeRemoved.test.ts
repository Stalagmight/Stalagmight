/**
 * #576 D27-32(e): unreachable code stays deleted.
 *
 * Three pieces had no production caller: the `useFpsMonitor` hook (re-exported
 * by the hooks barrel, consumed nowhere), the `src/utils/index.ts` barrel
 * (imported nowhere), and the client-side DMP shot validator in `validation.ts`
 * (`validateShot`, `validateSection`, `getHighestSeverity`, `countIssues`),
 * reachable only from its own tests - the shot warnings users see come from the
 * backend (`get_project_warnings`). Code that never runs invites fixes to, and
 * reliance on, behaviour nothing exercises, so it was removed; this pins that.
 */
import { describe, expect, it } from 'vitest';
import * as validation from '../validation';
import en from '../../i18n/locales/en.json';

const files = import.meta.glob(['../../hooks/useFpsMonitor.ts', '../index.ts']);

describe('dead code stays removed (D27-32(e))', () => {
  it('has no useFpsMonitor hook and no utils barrel', () => {
    expect(Object.keys(files)).toEqual([]);
  });

  it('validation.ts no longer carries the unused client-side shot validator', () => {
    for (const name of ['validateShot', 'validateSection', 'getHighestSeverity', 'countIssues']) {
      expect(validation, name).not.toHaveProperty(name);
    }
    // The live part of the module stays.
    expect(validation).toHaveProperty('validateEditableShot');
    expect(validation).toHaveProperty('hasAzimuthDiscrepancy');
  });

  it('drops the strings only that validator used', () => {
    const v = (en as unknown as { validation: Record<string, unknown> }).validation;
    expect(v).not.toHaveProperty('shot');
    expect(v).not.toHaveProperty('section');
  });

  // #576 NF-W8D4-2: the DMP wizard's "no more automatic connections" frame
  // (`wizard-no-suggestions`) was its only render site, and it is gone.
  it('drops the DMP wizard string only the removed no-suggestions frame used', () => {
    const wizard = (en as unknown as { dialogs: { dmpNewImport: Record<string, unknown> } }).dialogs.dmpNewImport;
    expect(wizard).not.toHaveProperty('noSuggestions');
    expect(wizard).toHaveProperty('suggestionsFailed');
  });
});
