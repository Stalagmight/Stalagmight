/**
 * Component test: the recents list accenting a SpeleoDB project's WRITE-LOCK state
 * on the same leading edge that #270 uses for un-pushed work.
 *
 * Three states compete for one 3px bar, so the interesting behaviour is the
 * precedence (un-pushed work outranks lock state - it is the only one of the three
 * that can be lost) and the fact that a local project never picks up a lock accent.
 *
 * The colours are theme TOKENS rather than hex values on purpose: a literal white
 * bar does not exist on the four light themes, three of which have a near-white
 * `--cc-panel`. The token check below is the guard for that, and for the dead-class
 * trap next door - `text-text-tertiary` is used 54 times in this tree but no
 * `--cc-text-tertiary` token or Tailwind entry defines it, so as a colour it would
 * yield an invalid `box-shadow` that the browser drops whole.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import SystemTable from '../SystemTable';
import SystemCard from '../SystemCard';
import {
  LOCK_MINE_ACCENT,
  LOCK_OTHER_ACCENT,
  recentAccent,
  recentAccentStyle,
  resolveSpeleoLock,
  type SpeleoLockInfo,
} from '../SpeleoLockBadge';
import type { RecentListItem } from '../recentListItems';
import type { ProjectMetadata } from '../../../types/project';
import { UNPUSHED_ACCENT, type SpeleoLocalStateMap } from '../../../utils/speleoLocalState';
import { themeTokens } from '../../../theme/tokens';
// Test IDs are built by these helpers, which URL-encode the path - do not hand-
// assemble `system-card-${path}`, it silently stops matching for any path with a
// character encodeURIComponent touches.
import { systemCardTestId, systemRowTestId } from '../../../utils/testId';
import { stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';

const MINE = 'aaaaaaaa-1111-1111-1111-111111111111';
const THEIRS = 'bbbbbbbb-2222-2222-2222-222222222222';
const FREE = 'cccccccc-3333-3333-3333-333333333333';
const ME = 'me@example.com';
const THEM = 'buddy@example.com';

/** `speleodb_project_id` is optional, not nullable - a local project OMITS it. */
const project = (id: string | undefined, name: string): ProjectMetadata => ({
  name,
  location: null,
  file_path: `/p/${name}.stalx`,
  modified_at: '2026-07-26T00:00:00Z',
  section_count: 1,
  total_length: 10,
  station_count: 2,
  gps_center: null,
  gps_bounds: null,
  is_remote: id !== undefined,
  ...(id !== undefined ? { speleodb_project_id: id } : {}),
});

const MINE_PROJECT = project(MINE, 'Mine');
const THEIRS_PROJECT = project(THEIRS, 'Theirs');
const FREE_PROJECT = project(FREE, 'Free');
const LOCAL_PROJECT = project(undefined, 'Local');

const REMOTE_LOCKS = { [MINE]: ME, [THEIRS]: THEM, [FREE]: null };

const ITEMS: RecentListItem[] = [MINE_PROJECT, THEIRS_PROJECT, FREE_PROJECT, LOCAL_PROJECT].map(
  (p) => ({ kind: 'available', filePath: p.file_path, project: p }),
);

const lock = (over: Partial<SpeleoLockInfo> = {}): SpeleoLockInfo => ({
  isRemote: true,
  lockHolder: null,
  lockedByMe: false,
  speleoName: undefined,
  hasLocalEdits: false,
  ...over,
});

function renderTable(localStates: SpeleoLocalStateMap = {}) {
  render(
    <SystemTable
      items={ITEMS}
      selectedProjectId={null}
      multiSelectedIds={new Set()}
      sortField="name"
      sortDirection="asc"
      remoteLocks={REMOTE_LOCKS}
      remoteProjects={{ [MINE]: 'Mine', [THEIRS]: 'Theirs', [FREE]: 'Free' }}
      myEmail={ME}
      localStates={localStates}
      onSort={vi.fn()}
      onSelect={vi.fn()}
      onOpen={vi.fn()}
      onContextMenu={vi.fn()}
      onRemoveMissing={vi.fn()}
      onOpenRemote={vi.fn()}
    />,
  );
}

/** The accent lives on the row's first CELL - a <tr> generates no box under
    border-collapse, so a box-shadow set on it never paints. */
const accentOf = (row: HTMLElement) => row.querySelector('td')?.getAttribute('style') ?? '';

describe('recents write-lock accent', () => {
  it('picks one accent per state, and lets un-pushed work outrank the lock', () => {
    expect(recentAccent(lock({ lockedByMe: true, lockHolder: ME }))).toBe('lock-mine');
    expect(recentAccent(lock({ lockHolder: THEM }))).toBe('lock-other');
    expect(recentAccent(lock())).toBe('none');
    expect(recentAccent(undefined)).toBe('none');

    // The lock is recoverable (ask, or wait); work that exists only on this computer
    // is not. Both lock states yield to it, and the row still says which lock it is
    // in words via SpeleoRecentInfo.
    expect(recentAccent(lock({ lockedByMe: true, hasLocalEdits: true }))).toBe('unpushed');
    expect(recentAccent(lock({ lockHolder: THEM, hasLocalEdits: true }))).toBe('unpushed');
  });

  it('never gives a local project a lock accent', () => {
    // resolveSpeleoLock derives lockedByMe without consulting is_remote, so the
    // gate has to live in the accent itself.
    expect(recentAccent(lock({ isRemote: false, lockedByMe: true }))).toBe('none');
    expect(recentAccent(lock({ isRemote: false, lockHolder: THEM }))).toBe('none');
    expect(recentAccent(resolveSpeleoLock(LOCAL_PROJECT, REMOTE_LOCKS, ME))).toBe('none');
  });

  it('paints the bright neutral for your lock and blue for another holder', () => {
    expect(recentAccentStyle(lock({ lockedByMe: true }))?.boxShadow).toBe(
      `inset 3px 0 0 0 ${LOCK_MINE_ACCENT}`,
    );
    expect(recentAccentStyle(lock({ lockHolder: THEM }))?.boxShadow).toBe(
      `inset 3px 0 0 0 ${LOCK_OTHER_ACCENT}`,
    );
    expect(recentAccentStyle(lock())).toBeUndefined();
  });

  it('accents table rows by lock holder', () => {
    renderTable();
    const row = (p: ProjectMetadata) => screen.getByTestId(systemRowTestId(p.file_path));

    expect(row(MINE_PROJECT)).toHaveAttribute('data-recent-accent', 'lock-mine');
    expect(accentOf(row(MINE_PROJECT))).toContain(LOCK_MINE_ACCENT);

    expect(row(THEIRS_PROJECT)).toHaveAttribute('data-recent-accent', 'lock-other');
    expect(accentOf(row(THEIRS_PROJECT))).toContain(LOCK_OTHER_ACCENT);

    // A free remote project and a local one earn no bar at all.
    for (const p of [FREE_PROJECT, LOCAL_PROJECT]) {
      expect(row(p)).toHaveAttribute('data-recent-accent', 'none');
      expect(accentOf(row(p))).toBe('');
    }
  });

  it('keeps the yellow bar on a locked row that also holds un-pushed work', () => {
    renderTable({
      [MINE]: {
        has_local_copy: true,
        has_unpushed_edits: true,
        last_edit_at: '2026-07-26T00:00:00Z',
        is_open: false,
        state_unknown: false,
      },
    });
    const row = screen.getByTestId(systemRowTestId(MINE_PROJECT.file_path));
    expect(row).toHaveAttribute('data-recent-accent', 'unpushed');
    expect(row).toHaveAttribute('data-local-edits', 'true');
    expect(accentOf(row)).toContain(UNPUSHED_ACCENT);
    expect(accentOf(row)).not.toContain(LOCK_MINE_ACCENT);
  });

  it('keeps the selection ring on a card that is both selected and accented', () => {
    // Tailwind compiles `ring-1` to a box-shadow, so the inline accent shadow used to
    // replace it and a selected accented card lost its ring, leaving only the border
    // colour. Rare while only un-pushed rows had a bar; common once lock state does.
    render(
      <SystemCard
        name={MINE_PROJECT.name}
        location={null}
        filePath={MINE_PROJECT.file_path}
        modifiedAt={MINE_PROJECT.modified_at}
        sectionCount={1}
        totalLength={10}
        isSelected
        speleoLock={resolveSpeleoLock(MINE_PROJECT, REMOTE_LOCKS, ME)}
      />,
    );
    const style = screen.getByTestId(systemCardTestId(MINE_PROJECT.file_path)).getAttribute('style');
    expect(style).toContain(LOCK_MINE_ACCENT);
    // The ring is the non-inset half; without composition only the inset bar survives.
    expect(style).toContain('0 0 0 1px var(--cc-accent)');
  });

  it('leaves the ring class alone on a selected card with no accent', () => {
    // The untouched case must stay untouched: no inline box-shadow at all, so the
    // `ring-1` class paints exactly as it did before this composition existed.
    render(
      <SystemCard
        name={FREE_PROJECT.name}
        location={null}
        filePath={FREE_PROJECT.file_path}
        modifiedAt={FREE_PROJECT.modified_at}
        sectionCount={1}
        totalLength={10}
        isSelected
        speleoLock={resolveSpeleoLock(FREE_PROJECT, REMOTE_LOCKS, ME)}
      />,
    );
    const card = screen.getByTestId(systemCardTestId(FREE_PROJECT.file_path));
    expect(card).toHaveAttribute('data-recent-accent', 'none');
    expect(card.getAttribute('style') ?? '').not.toContain('box-shadow');
  });

  it('accents the card grid on the same signal', () => {
    render(
      <SystemCard
        name={THEIRS_PROJECT.name}
        location={null}
        filePath={THEIRS_PROJECT.file_path}
        modifiedAt={THEIRS_PROJECT.modified_at}
        sectionCount={1}
        totalLength={10}
        speleoLock={resolveSpeleoLock(THEIRS_PROJECT, REMOTE_LOCKS, ME)}
      />,
    );
    const card = screen.getByTestId(systemCardTestId(THEIRS_PROJECT.file_path));
    expect(card).toHaveAttribute('data-recent-accent', 'lock-other');
    expect(card.getAttribute('style')).toContain(LOCK_OTHER_ACCENT);
  });

  it('references tokens that every theme actually defines', () => {
    // The trap this guards: `text-text-tertiary` is used throughout this list, but
    // no --cc-text-tertiary token exists - the class emits nothing, and as a colour
    // it would produce an invalid box-shadow the browser discards silently. It also
    // catches a token being dropped from one theme, which is how a bar goes missing
    // on exactly one theme nobody screenshots.
    const named = (accent: string) => /^var\((--cc-[a-z0-9-]+)\)$/.exec(accent)?.[1];
    const mine = named(LOCK_MINE_ACCENT);
    const other = named(LOCK_OTHER_ACCENT);
    expect(mine).toBeDefined();
    expect(other).toBeDefined();

    // Exact, not a floor: a floor would let a whole theme be dropped and still pass.
    const themes = Object.entries(themeTokens);
    expect(themes.length).toBe(10);
    for (const [theme, tokens] of themes) {
      const defined = tokens as Record<string, string | undefined>;
      expect(defined[mine!], `${mine} missing on ${theme}`).toBeTruthy();
      expect(defined[other!], `${other} missing on ${theme}`).toBeTruthy();
      // Bright neutral vs blue: on no theme may the two collapse to one bar.
      expect(defined[mine!], `${theme} accents are identical`).not.toBe(defined[other!]);
    }
  });
});
