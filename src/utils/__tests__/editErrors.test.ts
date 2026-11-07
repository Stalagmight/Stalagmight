import { describe, it, expect } from 'vitest';
import i18n from '../../i18n';
import { READ_ONLY_EDIT_NOTICE, isEditGateCode, isReverifyDivergedCode } from '../editErrors';
import { errorCode } from '../commandError';
import type { CommandErrorCode } from '../../types/ipc/CommandErrorCode';

describe('isEditGateCode', () => {
  it('matches every edit-gate refusal code (#576 A6)', () => {
    // The three the old `startsWith('EDIT_DISABLED')` matched: the SpeleoDB
    // read-only gate, the read-only workspace, and a newer file's edit refusal.
    expect(isEditGateCode('EDIT_DISABLED')).toBe(true);
    expect(isEditGateCode('WORKSPACE_READ_ONLY')).toBe(true);
    expect(isEditGateCode('NEWER_FILE_EDIT_DISABLED')).toBe(true);
    // [#576 RV5-F4-5] And the adoption hand-over, which the old marker put behind
    // EDIT_DISABLED ("EDIT_DISABLED: ADOPTION_IN_PROGRESS: ...").
    expect(isEditGateCode('ADOPTION_IN_PROGRESS')).toBe(true);
  });

  it('is false for unrelated codes, null, and undefined', () => {
    expect(isEditGateCode('FAILED')).toBe(false);
    expect(isEditGateCode('INTERNAL_ERROR')).toBe(false);
    expect(isEditGateCode(null)).toBe(false);
    expect(isEditGateCode(undefined)).toBe(false);
  });

  it('does not take the re-verify divergence or a refused SAVE of a newer file', () => {
    // The re-verify divergence routes to the divergence prompt, and a newer file's
    // SAVE refusal (NEWER_FILE_READ_ONLY) to the save router, not the edit notice.
    expect(isEditGateCode('EDIT_REVERIFY_DIVERGED')).toBe(false);
    expect(isEditGateCode('NEWER_FILE_READ_ONLY')).toBe(false);
  });

  it('reads the code off the failure, never its message', () => {
    // A message that merely MENTIONS the old marker is not a refusal of the gate.
    expect(isEditGateCode(errorCode({ code: 'FAILED', message: 'EDIT_DISABLED: looks coded' }))).toBe(false);
    expect(isEditGateCode(errorCode({ code: 'EDIT_DISABLED', message: 'Move station is disabled.' }))).toBe(true);
  });
});

describe('isReverifyDivergedCode', () => {
  it('matches the #202 re-verify divergence', () => {
    expect(isReverifyDivergedCode('EDIT_REVERIFY_DIVERGED')).toBe(true);
  });

  it('is distinct from the plain read-only gate and unrelated codes', () => {
    // A plain read-only rejection must NOT read as a divergence (different routing).
    expect(isReverifyDivergedCode('EDIT_DISABLED')).toBe(false);
    expect(isReverifyDivergedCode('FAILED')).toBe(false);
    expect(isReverifyDivergedCode(null)).toBe(false);
    expect(isReverifyDivergedCode(undefined)).toBe(false);
  });
});

describe('READ_ONLY_EDIT_NOTICE', () => {
  it('names the read-only workspace for a Plan/Export refusal (#576 C5)', () => {
    const code: CommandErrorCode = 'WORKSPACE_READ_ONLY';
    expect(isEditGateCode(code)).toBe(true);
    expect(READ_ONLY_EDIT_NOTICE(code)).toBe(i18n.t('editGate.workspaceReadOnlyNotice'));
    expect(READ_ONLY_EDIT_NOTICE(code)).not.toBe(i18n.t('editGate.readOnlyNotice'));
  });

  it('keeps the lock notice for a SpeleoDB refusal and the version notice for a newer file', () => {
    expect(READ_ONLY_EDIT_NOTICE('EDIT_DISABLED')).toBe(i18n.t('editGate.readOnlyNotice'));
    expect(READ_ONLY_EDIT_NOTICE('NEWER_FILE_EDIT_DISABLED')).toBe(i18n.t('editGate.newerFileNotice'));
  });
});

// [#576 RV5-F4-5] The adoption hand-over refusal carries its own code, so the
// notice says the project is being handed over instead of inviting a Start
// editing click in the middle of the adoption's own lock operations.
describe('READ_ONLY_EDIT_NOTICE during a SpeleoDB adoption', () => {
  const error = {
    code: 'ADOPTION_IN_PROGRESS' as const,
    message:
      'Move station 1:1 is disabled: this project is being handed over to ' +
      'SpeleoDB. Edit it once the upload has finished.',
  };

  it('says the project is being handed over, not "click Start editing"', () => {
    const notice = READ_ONLY_EDIT_NOTICE(errorCode(error));
    console.log(`[RV5-F4-5] notice for an adoption refusal: "${notice}"`);
    expect(isEditGateCode(errorCode(error))).toBe(true);
    expect(notice).toBe(i18n.t('editGate.adoptionInProgressNotice'));
    expect(notice).not.toBe(i18n.t('editGate.readOnlyNotice'));
    expect(notice).not.toMatch(/Start editing/);
  });
});
