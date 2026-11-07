/**
 * [#576 W44-NF2] The project each ended session ended on (`projectSessionEndedOn`)
 * is kept for the most recent ENDED_SESSIONS_KEPT sessions only: one entry per
 * session change, never pruned, grew for as long as the app ran. An evicted session
 * reads as one that recorded nothing, so `followSession` gives its identity back
 * unchanged.
 *
 * Module tier: the session module's public functions only; the open project's
 * identity is a synthetic getter. All data is synthetic.
 */
import { describe, expect, it } from 'vitest';

import { followSession } from '../projectIssue';
import {
  ENDED_SESSIONS_KEPT,
  noteProjectSessionChanged,
  projectSessionEndedOn,
  projectSessionEpoch,
  registerOpenProjectIdentity,
} from '../projectSession';

const KEY = '2026-09-01T10:00:00Z';
/** Well past any bound worth keeping, so the test does not lean on the constant to fail. */
const CHANGES = 1000;

describe('projectSessionEndedOn keeps only the most recent ended sessions (#576 W44-NF2)', () => {
  it(`forgets the oldest once more than the bound have ended, keeping the newest`, () => {
    // Each session ends on its own file, so every kept entry is told apart.
    registerOpenProjectIdentity(() => ({ createdAt: KEY, filePath: `/caves/s${projectSessionEpoch()}.stalx` }));
    const first = projectSessionEpoch();
    for (let i = 0; i < CHANGES; i += 1) noteProjectSessionChanged();
    const newest = projectSessionEpoch() - 1;

    let kept = 0;
    for (let s = first; s <= newest; s += 1) if (projectSessionEndedOn(s) !== undefined) kept += 1;
    const oldest = projectSessionEndedOn(first);
    expect(oldest, `${kept} of ${CHANGES} ended sessions kept; session ${first} still recorded as ${JSON.stringify(oldest)}`)
      .toBeUndefined();
    expect(kept, `${kept} of ${CHANGES} ended sessions kept`).toBe(ENDED_SESSIONS_KEPT);
    expect(ENDED_SESSIONS_KEPT).toBeGreaterThanOrEqual(64);
    // The newest ENDED_SESSIONS_KEPT are the ones kept; the one before them is gone.
    expect(projectSessionEndedOn(newest)).toEqual({ createdAt: KEY, filePath: `/caves/s${newest}.stalx` });
    const oldestKept = newest - ENDED_SESSIONS_KEPT + 1;
    expect(projectSessionEndedOn(oldestKept)).toEqual({ createdAt: KEY, filePath: `/caves/s${oldestKept}.stalx` });
    expect(projectSessionEndedOn(oldestKept - 1)).toBeUndefined();
    // The current session has not ended.
    expect(projectSessionEndedOn(projectSessionEpoch())).toBeUndefined();

    // An evicted session follows nothing (the rule before #576 RVF-c-A2-2); a kept one still follows.
    const evicted = { key: KEY, file: '/caves/opened.stalx', session: first };
    expect(followSession(evicted)).toEqual(evicted);
    const recent = { key: KEY, file: '/caves/opened.stalx', session: newest };
    expect(followSession(recent)).toEqual({ ...recent, file: `/caves/s${newest}.stalx` });
  });
});
