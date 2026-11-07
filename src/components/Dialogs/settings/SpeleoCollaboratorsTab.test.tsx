/**
 * Component test (#215) - the Collaborators tab of the SpeleoDB project dialog.
 *
 * Covers the ADMIN-vs-READ_ONLY gating, the exact IPC payloads the CRUD controls
 * send, the self-row lockout, the load-error retry, and (#270) the two-step
 * arming that every server-mutating verb on this tab now needs plus the
 * cancellation of a superseded roster load.
 */
import { describe, it, expect, vi, beforeEach, beforeAll } from 'vitest';
import { render, screen, waitFor, act, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// jsdom does not implement scrollIntoView; the Select component calls it on open.
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

/** Pick `value` from a portal-based `Select` (src/components/common/Select.tsx):
 *  it isn't a native `<select>`, so this clicks the trigger, then the option
 *  rendered at `data-testid="${testId}-option-${value}"`. */
async function pickOption(testId: string, value: string) {
  await userEvent.click(screen.getByTestId(testId));
  await userEvent.click(screen.getByTestId(`${testId}-option-${encodeURIComponent(value)}`));
}

vi.mock('../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { SpeleoCollaboratorsTab } from './SpeleoCollaboratorsTab';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import type { Permission, SpeleoCollaborators } from '../../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../test/factories/ipc';

const ROSTER: SpeleoCollaborators = {
  users: [
    { email: 'me@x.com', permission: 'ADMIN', is_self: true },
    { email: 'bob@x.com', permission: 'READ_ONLY', is_self: false },
    // A SECOND removable row, so the per-row arming can be told apart from a
    // single tab-wide "armed" flag.
    { email: 'carol@x.com', permission: 'READ_ONLY', is_self: false },
  ],
  teams: [{ team_id: 't1', team_name: 'Divers', permission: 'READ_AND_WRITE' }],
};

let showToast: ReturnType<typeof vi.fn>;

const seed = (permission: Permission) => {
  useSpeleoStore.setState({
    link: {
      permission,
      editState: 'read-only',
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: false,
      unwound: false,
      remoteState: 'up_to_date',
      noAccess: false,
      serverError: null,
    } as never,
  });
};

const defaultMock = () =>
  invokeIpcMock.mockImplementation((cmd: string) => {
    if (cmd === 'speleodb_list_collaborators') return Promise.resolve(ROSTER);
    if (cmd === 'speleodb_list_teams')
      return Promise.resolve([
        { id: 't1', name: 'Divers' },
        { id: 't2', name: 'Surveyors' },
      ]);
    // add/set/remove all return the (unchanged, for the test) refreshed roster.
    return Promise.resolve(ROSTER);
  });

/** True when no mutation of `cmd` has reached the backend yet. */
const notCalled = (cmd: string) => invokeIpcMock.mock.calls.every((c) => c[0] !== cmd);

beforeEach(() => {
  invokeIpcMock.mockReset();
  showToast = vi.fn();
  useUIStore.setState({ showToast });
  useSpeleoStore.setState({ session: null, remoteProjectId: null });
  defaultMock();
});

describe('SpeleoCollaboratorsTab', () => {
  it('renders the user + team roster and the instant-changes note', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    expect(await screen.findByTestId('speleodb-collab-user-bob@x.com')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-collab-team-t1')).toHaveTextContent('Divers');
    expect(screen.getByTestId('speleodb-collaborators-changes-note')).toBeInTheDocument();
  });

  it('does not offer mutation controls to a non-admin', async () => {
    seed('READ_ONLY');
    render(<SpeleoCollaboratorsTab />);
    expect(await screen.findByTestId('speleodb-collaborators-readonly-badge')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-collaborators-add-user')).toBeNull();
    expect(screen.queryByTestId('speleodb-collab-user-remove-offer-bob@x.com')).toBeNull();
  });

  it('disables its own row (no self grant/remove)', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await screen.findByTestId('speleodb-collab-user-me@x.com');
    expect(screen.queryByTestId('speleodb-collab-user-remove-offer-me@x.com')).toBeNull();
    expect(screen.queryByTestId('speleodb-collab-user-level-me@x.com')).toBeNull();
    // But another user's row IS editable.
    expect(screen.getByTestId('speleodb-collab-user-remove-offer-bob@x.com')).toBeInTheDocument();
  });

  it('adds a collaborator with the chosen email + level', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await userEvent.type(await screen.findByTestId('speleodb-collaborators-add-email'), 'new@x.com');
    await pickOption('speleodb-collaborators-add-level', 'READ_AND_WRITE');
    await userEvent.click(screen.getByTestId('speleodb-collaborators-add-user'));
    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_add_collaborator');
      expect(call).toBeTruthy();
      expect(call![1]).toEqual({ email: 'new@x.com', permission: 'READ_AND_WRITE', expectedProject: null });
    });
  });

  it('autocompletes the collaborator email and fills it on pick', async () => {
    seed('ADMIN');
    invokeIpcMock.mockImplementation((cmd: string, args?: { query?: string }) => {
      if (cmd === 'speleodb_list_collaborators') return Promise.resolve(ROSTER);
      if (cmd === 'speleodb_list_teams') return Promise.resolve([]);
      if (cmd === 'speleodb_search_users') {
        // The debounce means only the settled 3-char query reaches the server.
        expect(args?.query).toBe('ali');
        return Promise.resolve([{ email: 'alice@x.com', label: 'Alice Diver <alice@x.com>' }]);
      }
      return Promise.resolve(ROSTER);
    });
    render(<SpeleoCollaboratorsTab />);
    const input = await screen.findByTestId('speleodb-collaborators-add-email');
    await userEvent.type(input, 'ali');
    const suggestion = await screen.findByTestId('speleodb-collaborators-suggestion-alice@x.com');
    expect(suggestion).toHaveTextContent('Alice Diver <alice@x.com>');
    await userEvent.click(suggestion);
    expect(input).toHaveValue('alice@x.com');
    // Picking closes the dropdown (and does not re-open it for the filled value).
    expect(screen.queryByTestId('speleodb-collaborators-suggestions')).toBeNull();
  });

  it('does not query autocomplete below the 3-char minimum', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    const input = await screen.findByTestId('speleodb-collaborators-add-email');
    await userEvent.type(input, 'al');
    // Give the debounce window time to (not) fire.
    await new Promise((r) => window.setTimeout(r, 350));
    expect(notCalled('speleodb_search_users')).toBe(true);
  });

  it('changes and removes another user with the right payloads', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await screen.findByTestId('speleodb-collab-user-level-bob@x.com');
    await pickOption('speleodb-collab-user-level-bob@x.com', 'READ_AND_WRITE');
    await userEvent.click(screen.getByTestId('speleodb-collab-user-level-apply-bob@x.com'));
    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_set_collaborator');
      expect(call![1]).toEqual({ email: 'bob@x.com', permission: 'READ_AND_WRITE', expectedProject: null });
    });
    await userEvent.click(screen.getByTestId('speleodb-collab-user-remove-offer-bob@x.com'));
    await userEvent.click(screen.getByTestId('speleodb-collab-user-remove-confirm-bob@x.com'));
    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_remove_collaborator');
      expect(call![1]).toEqual({ email: 'bob@x.com', expectedProject: null });
    });
  });

  it('adds a team via the picker (no ADMIN option offered)', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await screen.findByTestId('speleodb-collaborators-add-team-id');
    // Teams cannot be ADMIN: the level select offers only the three non-admin tiers.
    const levelSelect = screen.getByTestId('speleodb-collaborators-add-team-level');
    await userEvent.click(levelSelect);
    const levelListbox = screen.getByTestId('speleodb-collaborators-add-team-level-listbox');
    expect(within(levelListbox).getAllByRole('option')).toHaveLength(3);
    await userEvent.click(levelSelect); // close it back before touching the other picker
    await pickOption('speleodb-collaborators-add-team-id', 't2');
    await userEvent.click(screen.getByTestId('speleodb-collaborators-add-team'));
    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_add_team');
      expect(call![1]).toEqual({ teamId: 't2', permission: 'READ_ONLY', expectedProject: null });
    });
  });

  it('changes and removes a team with the right payloads', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await screen.findByTestId('speleodb-collab-team-level-t1');
    await pickOption('speleodb-collab-team-level-t1', 'READ_ONLY');
    await userEvent.click(screen.getByTestId('speleodb-collab-team-level-apply-t1'));
    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_set_team');
      expect(call![1]).toEqual({ teamId: 't1', permission: 'READ_ONLY', expectedProject: null });
    });
    await userEvent.click(screen.getByTestId('speleodb-collab-team-remove-offer-t1'));
    await userEvent.click(screen.getByTestId('speleodb-collab-team-remove-confirm-t1'));
    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_remove_team');
      expect(call![1]).toEqual({ teamId: 't1', expectedProject: null });
    });
  });

  // ---- #270: no server mutation on a single, un-confirmed press ----

  it('does not remove a collaborator until the armed confirm', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await userEvent.click(await screen.findByTestId('speleodb-collab-user-remove-offer-bob@x.com'));
    // Armed only: the offer control is REPLACED by a distinct confirm one, and
    // nothing has reached the server.
    expect(
      screen.getByTestId('speleodb-collab-user-remove-confirm-bob@x.com'),
    ).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-collab-user-remove-offer-bob@x.com')).toBeNull();
    expect(notCalled('speleodb_remove_collaborator')).toBe(true);

    await userEvent.click(screen.getByTestId('speleodb-collab-user-remove-confirm-bob@x.com'));
    await waitFor(() =>
      expect(
        invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_remove_collaborator')![1],
      ).toEqual({ email: 'bob@x.com', expectedProject: null }),
    );
  });

  it('does not remove a team until the armed confirm', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await userEvent.click(await screen.findByTestId('speleodb-collab-team-remove-offer-t1'));
    expect(screen.getByTestId('speleodb-collab-team-remove-confirm-t1')).toBeInTheDocument();
    expect(notCalled('speleodb_remove_team')).toBe(true);
  });

  it('arms the removal per row, so another row is never armed with it', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    await userEvent.click(await screen.findByTestId('speleodb-collab-user-remove-offer-bob@x.com'));
    // Carol's row is untouched by Bob's arming...
    expect(screen.getByTestId('speleodb-collab-user-remove-offer-carol@x.com')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-collab-user-remove-confirm-carol@x.com')).toBeNull();

    // ...and arming HERS disarms Bob's, so a stray second press cannot land on a
    // row the user has already moved on from.
    await userEvent.click(screen.getByTestId('speleodb-collab-user-remove-offer-carol@x.com'));
    expect(
      screen.getByTestId('speleodb-collab-user-remove-confirm-carol@x.com'),
    ).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-collab-user-remove-offer-bob@x.com')).toBeInTheDocument();
    expect(notCalled('speleodb_remove_collaborator')).toBe(true);
  });

  it('stages a level change instead of re-permissioning on the select', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    const select = await screen.findByTestId('speleodb-collab-user-level-bob@x.com');
    await pickOption('speleodb-collab-user-level-bob@x.com', 'READ_AND_WRITE');
    // The picker shows the user's own choice, but says it is not applied, and
    // nothing has been granted server-side.
    expect(select).toHaveTextContent('Read & write');
    expect(screen.getByTestId('speleodb-collab-user-level-pending-bob@x.com')).toBeInTheDocument();
    expect(notCalled('speleodb_set_collaborator')).toBe(true);
  });

  it('backs a staged level change out without touching the server', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    const select = await screen.findByTestId('speleodb-collab-user-level-bob@x.com');
    await pickOption('speleodb-collab-user-level-bob@x.com', 'ADMIN');
    await userEvent.click(screen.getByTestId('speleodb-collab-user-level-cancel-bob@x.com'));
    // Back to server truth, with no pending badge left claiming otherwise.
    expect(select).toHaveTextContent('Read only');
    expect(screen.queryByTestId('speleodb-collab-user-level-pending-bob@x.com')).toBeNull();
    expect(notCalled('speleodb_set_collaborator')).toBe(true);
  });

  it('drops a staged level change when a removal is armed on the same row', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    const select = await screen.findByTestId('speleodb-collab-user-level-bob@x.com');
    await pickOption('speleodb-collab-user-level-bob@x.com', 'ADMIN');
    await userEvent.click(screen.getByTestId('speleodb-collab-user-remove-offer-bob@x.com'));
    // The competing action wins the single armed slot: the half-made grant is gone
    // and the picker is back on what the server has.
    expect(screen.queryByTestId('speleodb-collab-user-level-pending-bob@x.com')).toBeNull();
    expect(select).toHaveTextContent('Read only');
    expect(
      screen.getByTestId('speleodb-collab-user-remove-confirm-bob@x.com'),
    ).toBeInTheDocument();
  });

  it('surfaces a mutation failure as a toast and leaves the roster intact', async () => {
    seed('ADMIN');
    // The change call rejects; everything else (incl. the roster reload) stays fine.
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_list_collaborators') return Promise.resolve(ROSTER);
      if (cmd === 'speleodb_list_teams') return Promise.resolve([{ id: 't1', name: 'Divers' }]);
      if (cmd === 'speleodb_set_collaborator') return Promise.reject(new Error('boom'));
      return Promise.resolve(ROSTER);
    });
    render(<SpeleoCollaboratorsTab />);
    const select = await screen.findByTestId('speleodb-collab-user-level-bob@x.com');
    await pickOption('speleodb-collab-user-level-bob@x.com', 'READ_AND_WRITE');
    await userEvent.click(screen.getByTestId('speleodb-collab-user-level-apply-bob@x.com'));
    // Error surfaced, and the row is still there (roster not blanked on failure).
    await waitFor(() => expect(showToast).toHaveBeenCalled());
    expect(screen.getByTestId('speleodb-collab-user-bob@x.com')).toBeInTheDocument();
    // A change that did not take is not left on screen looking applied.
    expect(select).toHaveTextContent('Read only');
    expect(screen.queryByTestId('speleodb-collab-user-level-pending-bob@x.com')).toBeNull();
  });

  it('shows the load-error state and retries', async () => {
    seed('ADMIN');
    let attempt = 0;
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_list_collaborators') {
        attempt += 1;
        return attempt === 1 ? Promise.reject(new Error('offline')) : Promise.resolve(ROSTER);
      }
      if (cmd === 'speleodb_list_teams') return Promise.resolve([]);
      return Promise.resolve(ROSTER);
    });
    render(<SpeleoCollaboratorsTab />);
    const retry = await screen.findByTestId('speleodb-collaborators-retry');
    await userEvent.click(retry);
    expect(await screen.findByTestId('speleodb-collab-user-bob@x.com')).toBeInTheDocument();
  });

  it('ignores a superseded roster load that answers last', async () => {
    // The tab mounts before the permission is known (`syncProjectLink` seeds it
    // null / non-admin) and the status seed resolves it while mounted, so a second
    // load starts with the first still in flight.
    seed('READ_ONLY');
    const pending: Array<{
      resolve: (v: SpeleoCollaborators) => void;
      reject: (e: unknown) => void;
    }> = [];
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_list_collaborators')
        return new Promise<SpeleoCollaborators>((resolve, reject) =>
          pending.push({ resolve, reject }),
        );
      if (cmd === 'speleodb_list_teams') return Promise.resolve([]);
      return Promise.resolve(ROSTER);
    });
    render(<SpeleoCollaboratorsTab />);
    await waitFor(() => expect(pending).toHaveLength(1));

    act(() => seed('ADMIN'));
    await waitFor(() => expect(pending).toHaveLength(2));

    // The NEWER request answers first and paints the roster...
    await act(async () => {
      pending[1]!.resolve(ROSTER);
    });
    expect(await screen.findByTestId('speleodb-collab-user-bob@x.com')).toBeInTheDocument();

    // ...then the stale one fails. It must not blank the good roster with its error,
    // nor own the `loading` flag the newer request already cleared.
    await act(async () => {
      pending[0]!.reject(new Error('stale'));
    });
    expect(screen.queryByTestId('speleodb-collaborators-error')).toBeNull();
    expect(screen.queryByTestId('speleodb-collaborators-loading')).toBeNull();
    expect(screen.getByTestId('speleodb-collab-user-bob@x.com')).toBeInTheDocument();
  });

  it('disables the web link when there is no signed-in endpoint to open', async () => {
    seed('ADMIN');
    render(<SpeleoCollaboratorsTab />);
    expect(await screen.findByTestId('speleodb-web-collaborators')).toBeDisabled();

    useSpeleoStore.setState({
      session: { loggedIn: true, userEmail: 'me@x.com', endpoint: 'https://x.test', keychainBacked: true, storage: 'keychain', keychain: 'available' },
      remoteProjectId: 'p1',
    });
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-web-collaborators')).toBeEnabled(),
    );
  });
});
