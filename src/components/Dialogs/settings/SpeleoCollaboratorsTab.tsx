import { useCallback, useEffect, useState } from 'react';
import type { Dispatch, SetStateAction } from 'react';
import { useTranslation } from 'react-i18next';
import { SettingsSection } from './SettingsSection';
import { Button } from '../../common/Button';
import { Select } from '../../common/Select';
import { SpeleoUserAutocomplete } from '../../common/SpeleoUserAutocomplete';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { expectedOpenProject } from '../../../store/expectedOpenProject';
import { useSpeleoWebLink } from '../../../hooks/useSpeleoWebLink';
import { invokeIpc } from '../../../utils/ipc';
import { humanizeSpeleoError } from '../../../utils/speleoError';
import { toastProjectChanged } from '../../../utils/projectChangedNotice';
import { permissionAtLeast, USER_LEVELS, TEAM_LEVELS } from '../../../utils/speleoPermission';
import type { Permission, SpeleoCollaborators, SpeleoTeamOption } from '../../../types/ipc.generated';
import { TOAST_KEYS } from '../../../utils/toastKeys';

const inputClass =
  'w-full px-2 py-1 bg-background border border-border rounded text-sm disabled:opacity-60';

/** Wire tier -> the i18n key of its localized label. */
const levelKey = (p: Permission) => `speleodb.collaborators.levels.${p}`;

/**
 * The ONE roster step currently armed, anywhere in the tab.
 *
 * `row` is the `<kind>:<id>` key of the row it belongs to, so a step armed on one
 * collaborator can never be confirmed by a press on another: arming or staging
 * anywhere overwrites this single slot, which disarms whatever held it before.
 * That is also why removal and re-permission share the slot - they are competing
 * actions on the same roster, and only one of them can be half-committed at a time.
 */
type ArmedAction =
  | { kind: 'remove'; row: string }
  | { kind: 'level'; row: string; permission: Permission };

/**
 * The per-row access-level picker + Remove control, shared by the user and the team
 * list (identical mechanics; only the ids and the offered tiers differ).
 *
 * Both verbs mutate the SERVER the instant they fire and neither has an undo, so
 * both are two-step, in the same arm-then-confirm shape the revert/restore dialogs
 * use for their destructive routes:
 * - Remove offers itself first and only removes on a second, differently labelled
 *   press against a distinct `data-testid`.
 * - The picker only STAGES a level; the re-permission IPC waits for an explicit
 *   Apply, with Cancel as the way back out. A `<select>` re-permissioned on change
 *   turned a stray click or an arrow key - the two things a `<select>` is most
 *   likely to receive by accident - into an irreversible server-side grant.
 *
 * A staged level is what the picker SHOWS, because the alternative (showing the
 * server value while holding a different pending one) hides the user's own choice.
 * It is shown next to a badge that says it has not been applied, so the control
 * never silently claims a tier the server does not actually have.
 */
function RosterRowActions({
  kind,
  id,
  testKey,
  name,
  current,
  levels,
  busy,
  armed,
  setArmed,
  onSetLevel,
  onRemove,
}: {
  kind: 'user' | 'team';
  /** Row identity: the email for a user, the team id for a team. */
  id: string;
  /** What this row's test ids are keyed by: the position for a user, never the
   *  email (a test id is printed where a run reports what it clicked; #576
   *  W47-NF1), the team id for a team. */
  testKey: string;
  /** Human-readable subject of this row, for the per-row aria labels. */
  name: string;
  /** The tier the SERVER currently has for this row. */
  current: Permission;
  levels: readonly Permission[];
  busy: boolean;
  armed: ArmedAction | null;
  setArmed: Dispatch<SetStateAction<ArmedAction | null>>;
  onSetLevel: (permission: Permission) => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation();
  const row = `${kind}:${id}`;
  const staged = armed?.kind === 'level' && armed.row === row ? armed.permission : null;
  const removeArmed = armed?.kind === 'remove' && armed.row === row;
  const testId = (part: string) => `speleodb-collab-${kind}-${part}-${testKey}`;

  return (
    <>
      <Select
        value={staged ?? current}
        disabled={busy}
        onChange={(v) => {
          const next = v as Permission;
          // Landing back on the tier the server already has is a back-out, not a
          // change: there is nothing left to apply, so drop the staged step.
          setArmed(next === current ? null : { kind: 'level', row, permission: next });
        }}
        options={[
          // [#576 B16] A tier the server added after this build shipped is shown as
          // the current value, but cannot be picked: it is never sent back.
          ...(levels.includes(current)
            ? []
            : [{ value: current, label: t(levelKey(current)), disabled: true }]),
          ...levels.map((p) => ({ value: p, label: t(levelKey(p)) })),
        ]}
        size="sm"
        pending={staged !== null}
        aria-label={t('speleodb.collaborators.levelFor', { name })}
        data-testid={testId('level')}
      />
      {staged !== null && (
        <>
          <span className="text-warning text-xs" data-testid={testId('level-pending')}>
            {t('speleodb.collaborators.levelPending')}
          </span>
          <button
            type="button"
            disabled={busy}
            onClick={() => onSetLevel(staged)}
            className="text-accent text-xs hover:underline disabled:opacity-60"
            aria-label={t('speleodb.collaborators.applyLevelFor', { name })}
            data-testid={testId('level-apply')}
          >
            {t('common.actions.apply')}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => setArmed(null)}
            className="text-text-secondary text-xs hover:underline disabled:opacity-60"
            aria-label={t('speleodb.collaborators.cancelLevelFor', { name })}
            data-testid={testId('level-cancel')}
          >
            {t('common.actions.cancel')}
          </button>
        </>
      )}
      <button
        type="button"
        disabled={busy}
        onClick={() => {
          if (!removeArmed) {
            setArmed({ kind: 'remove', row });
            return;
          }
          onRemove();
        }}
        className={`text-error text-xs hover:underline disabled:opacity-60 ${
          removeArmed ? 'font-semibold underline' : ''
        }`}
        aria-label={
          removeArmed
            ? t('speleodb.collaborators.removeConfirmFor', { name })
            : t('speleodb.collaborators.removeFor', { name })
        }
        data-testid={removeArmed ? testId('remove-confirm') : testId('remove-offer')}
      >
        {removeArmed
          ? t('speleodb.collaborators.removeConfirm')
          : t('speleodb.collaborators.remove')}
      </button>
    </>
  );
}

/**
 * [#215] The Collaborators tab of the SpeleoDB project dialog: individual users +
 * teams (incl. the WEB_VIEWER tier). Viewing the roster needs READ_ONLY+; every
 * mutation needs ADMIN (the server 401/403 is the real gate - the UI only mirrors
 * the level and surfaces the error).
 */
export function SpeleoCollaboratorsTab() {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  const link = useSpeleoStore((s) => s.link);
  const web = useSpeleoWebLink('permissions/user');

  const canManage = permissionAtLeast(link?.permission, 'ADMIN');

  const [roster, setRoster] = useState<SpeleoCollaborators | null>(null);
  const [teams, setTeams] = useState<SpeleoTeamOption[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The single armed slot every row's controls share - see `ArmedAction`.
  const [armed, setArmed] = useState<ArmedAction | null>(null);

  const [newEmail, setNewEmail] = useState('');
  const [newLevel, setNewLevel] = useState<Permission>('READ_ONLY');
  const [newTeamId, setNewTeamId] = useState('');
  const [newTeamLevel, setNewTeamLevel] = useState<Permission>('READ_ONLY');

  // Retry re-runs the load effect by bumping this rather than by calling a loader
  // callback, so the fetch can live INSIDE the effect and be cancelled by that
  // effect's own cleanup (same shape as `useSpeleoLocalStates`).
  const [reloadNonce, setReloadNonce] = useState(0);
  const reload = useCallback(() => setReloadNonce((n) => n + 1), []);

  useEffect(() => {
    // The permission genuinely changes under a MOUNTED tab - `syncProjectLink`
    // seeds the link with `permission: null` and the first status seed resolves it
    // - so this effect re-runs while a request is still in flight. Without the
    // cancellation token the two replies raced and whichever answered LAST won: a
    // stale failure could `setLoadError` over a roster that had already loaded
    // fine, and a stale `finally` cleared the loading flag the NEWER request had
    // just raised, dropping the tab out of its spinner before it had an answer.
    let cancelled = false;
    setLoading(true);
    setLoadError(null);
    void (async () => {
      try {
        const list = await invokeIpc('speleodb_list_collaborators');
        // Only an admin can add, so only an admin needs the team picker.
        const options = canManage
          ? await invokeIpc('speleodb_list_teams')
          : [];
        if (cancelled) return;
        setRoster(list);
        setTeams(options);
      } catch (e) {
        if (!cancelled)
          setLoadError(humanizeSpeleoError(e, t('speleodb.collaborators.loadFailed')));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // Keyed on the ADMIN-ness, not on the raw permission string: the roster fetch
    // does not depend on the tier at all and only the team-picker fetch does, so
    // this re-runs when that gate actually flips instead of on every tier change.
  }, [canManage, t, reloadNonce]);

  // Run a roster mutation, folding the returned server-truth roster back in. All
  // errors surface as a toast; the roster is left untouched on failure. A refusal
  // because the project changed toasts the short line instead (#576 Q-W44-7-D1).
  const mutate = useCallback(
    async (op: () => Promise<SpeleoCollaborators>, failKey: string) => {
      setBusy(true);
      try {
        setRoster(await op());
      } catch (e) {
        if (!toastProjectChanged(e)) {
          showToast(humanizeSpeleoError(e, t(failKey)), TOAST_KEYS.SPELEODB_COLLABORATORS, 'error');
        }
      } finally {
        setBusy(false);
        // Disarm on completion, either way, and for every mutation (an add is just
        // as much a competing action as a remove). On success the returned roster
        // already carries the new truth; on FAILURE dropping the staged level snaps
        // the picker back to what the server still says, so a change that did not
        // take is never left on screen looking applied.
        setArmed(null);
      }
    },
    [showToast, t],
  );

  const addUser = useCallback(() => {
    const email = newEmail.trim();
    if (!email) return;
    void mutate(
      () =>
        invokeIpc('speleodb_add_collaborator', {
          email,
          permission: newLevel,
          // [#576 RVF-c-C1] Each roster change names its project (refused once
          // another is open); `mutate` runs the call at once, so this is the click.
          expectedProject: expectedOpenProject(),
        }).then((r) => {
          setNewEmail('');
          return r;
        }),
      'speleodb.collaborators.addFailed',
    );
  }, [mutate, newEmail, newLevel]);

  const addTeam = useCallback(() => {
    if (!newTeamId) return;
    void mutate(
      () =>
        invokeIpc('speleodb_add_team', {
          teamId: newTeamId,
          permission: newTeamLevel,
          expectedProject: expectedOpenProject(),
        }).then((r) => {
          setNewTeamId('');
          return r;
        }),
      'speleodb.collaborators.addTeamFailed',
    );
  }, [mutate, newTeamId, newTeamLevel]);

  const levelLabel = (p: Permission) => t(levelKey(p));

  return (
    <div data-testid="speleodb-collaborators-tab">
      {loading ? (
        <p className="text-text-secondary text-sm" data-testid="speleodb-collaborators-loading">
          {t('speleodb.collaborators.loading')}
        </p>
      ) : loadError || !roster ? (
        <div className="space-y-3" data-testid="speleodb-collaborators-error">
          <p className="text-error text-sm">{loadError ?? t('speleodb.collaborators.loadFailed')}</p>
          <Button
            variant="secondary"
            onClick={reload}
            data-testid="speleodb-collaborators-retry"
          >
            {t('speleodb.collaborators.retry')}
          </Button>
        </div>
      ) : (
        <SettingsSection title={t('speleodb.collaborators.title')}>
          <div className="flex justify-end">
            <button
              type="button"
              disabled={web.url === null}
              title={web.url === null ? t('speleodb.web.unavailable') : undefined}
              onClick={web.open}
              className="text-xs text-accent hover:underline disabled:opacity-50 disabled:no-underline"
              data-testid="speleodb-web-collaborators"
            >
              {t('speleodb.web.openCollaborators')}
            </button>
          </div>

          {!canManage && (
            <div
              className="mb-3 inline-block text-xs px-1.5 py-0.5 bg-warning-20 text-warning rounded"
              data-testid="speleodb-collaborators-readonly-badge"
            >
              {t('speleodb.collaborators.readOnlyBadge')}
            </div>
          )}

          {/* There is still no save step - each confirmed change goes straight to
              SpeleoDB - so the note says WHEN a change lands, now that the landing
              is gated on a second press rather than on the first click. */}
          <p className="text-text-secondary text-xs mb-2" data-testid="speleodb-collaborators-changes-note">
            {t('speleodb.collaborators.changesConfirmNote')}
          </p>

          <ul className="space-y-1" data-testid="speleodb-collaborators-users">
            {/* Rows are keyed by position in their test ids: a test id is printed where a
                run reports what it clicked, and must not carry an email (#576 W47-NF1). */}
            {roster.users.map((u, i) => (
              <li key={u.email} className="flex items-center gap-2" data-testid={`speleodb-collab-user-${i}`}>
                <span className="flex-1 min-w-0 truncate text-sm">
                  {u.email}
                  {u.is_self && (
                    <span className="text-text-secondary text-xs"> {t('speleodb.collaborators.you')}</span>
                  )}
                </span>
                {canManage && !u.is_self ? (
                  <RosterRowActions
                    kind="user"
                    id={u.email}
                    testKey={String(i)}
                    name={u.email}
                    current={u.permission}
                    levels={USER_LEVELS}
                    busy={busy}
                    armed={armed}
                    setArmed={setArmed}
                    onSetLevel={(permission) =>
                      void mutate(
                        () =>
                          invokeIpc('speleodb_set_collaborator', {
                            email: u.email,
                            permission,
                            expectedProject: expectedOpenProject(),
                          }),
                        'speleodb.collaborators.changeFailed',
                      )
                    }
                    onRemove={() =>
                      void mutate(
                        () =>
                          invokeIpc('speleodb_remove_collaborator', {
                            email: u.email,
                            expectedProject: expectedOpenProject(),
                          }),
                        'speleodb.collaborators.removeFailed',
                      )
                    }
                  />
                ) : (
                  <span className="text-text-secondary text-xs">{levelLabel(u.permission)}</span>
                )}
              </li>
            ))}
          </ul>

          {roster.teams.length > 0 && (
            <ul className="space-y-1 mt-2 pt-2 border-t border-border" data-testid="speleodb-collaborators-teams">
              {roster.teams.map((g) => (
                <li key={g.team_id} className="flex items-center gap-2" data-testid={`speleodb-collab-team-${g.team_id}`}>
                  <span className="flex-1 min-w-0 truncate text-sm">
                    {g.team_name}
                    <span className="text-text-secondary text-xs"> {t('speleodb.collaborators.teamTag')}</span>
                  </span>
                  {canManage ? (
                    <RosterRowActions
                      kind="team"
                      id={g.team_id}
                      testKey={g.team_id}
                      name={g.team_name}
                      current={g.permission}
                      levels={TEAM_LEVELS}
                      busy={busy}
                      armed={armed}
                      setArmed={setArmed}
                      onSetLevel={(permission) =>
                        void mutate(
                          () =>
                            invokeIpc('speleodb_set_team', {
                              teamId: g.team_id,
                              permission,
                              expectedProject: expectedOpenProject(),
                            }),
                          'speleodb.collaborators.changeFailed',
                        )
                      }
                      onRemove={() =>
                        void mutate(
                          () =>
                            invokeIpc('speleodb_remove_team', {
                              teamId: g.team_id,
                              expectedProject: expectedOpenProject(),
                            }),
                          'speleodb.collaborators.removeFailed',
                        )
                      }
                    />
                  ) : (
                    <span className="text-text-secondary text-xs">{levelLabel(g.permission)}</span>
                  )}
                </li>
              ))}
            </ul>
          )}

          {canManage && (
            <div className="mt-3 pt-3 border-t border-border space-y-2">
              <div className="flex items-center gap-2">
                <SpeleoUserAutocomplete
                  value={newEmail}
                  onChange={setNewEmail}
                  disabled={busy}
                  enabled={canManage}
                  className={inputClass}
                  placeholder={t('speleodb.collaborators.emailPlaceholder')}
                  ariaLabel={t('speleodb.collaborators.addUser')}
                  inputTestId="speleodb-collaborators-add-email"
                  listTestId="speleodb-collaborators-suggestions"
                  suggestionTestIdPrefix="speleodb-collaborators-suggestion"
                />
                <Select
                  value={newLevel}
                  disabled={busy}
                  onChange={(v) => setNewLevel(v as Permission)}
                  options={USER_LEVELS.map((p) => ({ value: p, label: levelLabel(p) }))}
                  aria-label={t('speleodb.collaborators.addLevel')}
                  data-testid="speleodb-collaborators-add-level"
                />
                <Button
                  variant="secondary"
                  disabled={busy || newEmail.trim() === ''}
                  onClick={addUser}
                  data-testid="speleodb-collaborators-add-user"
                >
                  {t('speleodb.collaborators.add')}
                </Button>
              </div>

              {teams.length > 0 && (
                <div className="flex items-center gap-2">
                  <Select
                    value={newTeamId}
                    disabled={busy}
                    onChange={setNewTeamId}
                    options={[
                      { value: '', label: t('speleodb.collaborators.teamPlaceholder'), disabled: true },
                      ...teams.map((tm) => ({ value: tm.id, label: tm.name })),
                    ]}
                    className="w-full"
                    aria-label={t('speleodb.collaborators.addTeam')}
                    data-testid="speleodb-collaborators-add-team-id"
                  />
                  <Select
                    value={newTeamLevel}
                    disabled={busy}
                    onChange={(v) => setNewTeamLevel(v as Permission)}
                    options={TEAM_LEVELS.map((p) => ({ value: p, label: levelLabel(p) }))}
                    aria-label={t('speleodb.collaborators.addTeamLevel')}
                    data-testid="speleodb-collaborators-add-team-level"
                  />
                  <Button
                    variant="secondary"
                    disabled={busy || newTeamId === ''}
                    onClick={addTeam}
                    data-testid="speleodb-collaborators-add-team"
                  >
                    {t('speleodb.collaborators.add')}
                  </Button>
                </div>
              )}
            </div>
          )}
        </SettingsSection>
      )}
    </div>
  );
}
