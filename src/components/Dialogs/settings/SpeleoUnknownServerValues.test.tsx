/**
 * [#576 B16] A permission tier or visibility SpeleoDB adds after this build shipped.
 *
 * The backend now parses an unrecognised label as `UNKNOWN` instead of failing the
 * whole list. The UI must show it as such, grant nothing on it, and never offer it
 * back to the server.
 */
import { describe, it, expect, vi, beforeEach, beforeAll } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

beforeAll(() => {
  // jsdom does not implement scrollIntoView; the Select component calls it on open.
  Element.prototype.scrollIntoView = vi.fn();
});

vi.mock('../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { SpeleoCollaboratorsTab } from './SpeleoCollaboratorsTab';
import { SpeleoDbGeneralTab } from './SpeleoDbGeneralTab';
import { useSpeleoProjectMetadata } from '../../../hooks/useSpeleoProjectMetadata';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { permissionAtLeast } from '../../../utils/speleoPermission';
import type {
  Permission,
  SpeleoCollaborators,
  SpeleoProjectMetadata,
} from '../../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../test/factories/ipc';

/** The value the backend sends for a label it does not know. */
const UNKNOWN = 'UNKNOWN' as Permission;

const seed = (permission: Permission) =>
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

const ROSTER: SpeleoCollaborators = {
  users: [
    { email: 'me@x.com', permission: 'ADMIN', is_self: true },
    { email: 'bob@x.com', permission: UNKNOWN, is_self: false },
  ],
  teams: [{ team_id: 't1', team_name: 'Divers', permission: UNKNOWN }],
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  useUIStore.setState({ showToast: vi.fn() });
  useSpeleoStore.setState({ session: null, remoteProjectId: null });
});

describe('unknown SpeleoDB permission tiers (#576 B16)', () => {
  it('grants nothing on an unknown tier', () => {
    for (const min of ['WEB_VIEWER', 'READ_ONLY', 'READ_AND_WRITE', 'ADMIN'] as Permission[]) {
      expect(permissionAtLeast(UNKNOWN, min)).toBe(false);
    }
  });

  it('shows an unknown collaborator tier as Unknown', async () => {
    seed('READ_ONLY');
    invokeIpcMock.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === 'speleodb_list_teams' ? [] : ROSTER),
    );
    render(<SpeleoCollaboratorsTab />);
    const bob = await screen.findByTestId('speleodb-collab-user-1');
    expect(bob).toHaveTextContent('Unknown');
    expect(bob).not.toHaveTextContent('speleodb.collaborators.levels');
    expect(screen.getByTestId('speleodb-collab-team-t1')).toHaveTextContent('Unknown');
  });

  it("keeps an unknown tier as the admin picker's current value without offering it", async () => {
    seed('ADMIN');
    invokeIpcMock.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === 'speleodb_list_teams' ? [] : ROSTER),
    );
    render(<SpeleoCollaboratorsTab />);
    const picker = await screen.findByTestId('speleodb-collab-user-level-1');
    expect(picker).toHaveTextContent('Unknown');
    await userEvent.click(picker);
    const unknownOption = screen.getByTestId(
      'speleodb-collab-user-level-1-option-UNKNOWN',
    );
    // Shown as the current value, but not selectable: it is never sent back.
    expect(unknownOption).toHaveAttribute('aria-disabled', 'true');
  });
});

describe('unknown SpeleoDB visibility (#576 B16)', () => {
  function Harness() {
    const metadata = useSpeleoProjectMetadata();
    return <SpeleoDbGeneralTab metadata={metadata} />;
  }

  it('shows an unknown visibility as Unknown and leaves it unchanged', async () => {
    seed('READ_AND_WRITE');
    const meta: SpeleoProjectMetadata = {
      name: 'CaveA',
      description: 'desc',
      country: 'US',
      visibility: 'UNKNOWN' as SpeleoProjectMetadata['visibility'],
      latitude: null,
      longitude: null,
      exclude_geojson: false,
    };
    invokeIpcMock.mockResolvedValue(meta);
    render(<Harness />);
    const select = await screen.findByTestId('speleodb-settings-visibility');
    await waitFor(() => expect(select).toHaveTextContent('Unknown'));
  });
});
