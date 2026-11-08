/**
 * [#576 W84 x FZ-FZ-INT-1] The project-exit ticket: latest wins among the exits that
 * wait for queued edits before they request their switch.
 *
 * An exit takes a ticket when it is asked for and checks it once its wait is over;
 * any newer exit intent, or any switch request, made in between overtakes it. The
 * behaviour it buys (Close vs a `.stalx` opened during the wait, and so on) is pinned
 * in `hooks/__tests__/handBackExits.waitRaces.test.tsx`.
 */
import { describe, it, expect } from 'vitest';

import { noteProjectExitRequested, noteSwitchRequested, projectExitSuperseded } from '../switchGate';

describe('project-exit ticket', () => {
  it('a ticket nothing overtook is current', () => {
    const ticket = noteProjectExitRequested();
    expect(projectExitSuperseded(ticket)).toBe(false);
  });

  it('a newer exit intent overtakes the older one, and not the other way round', () => {
    const older = noteProjectExitRequested();
    const newer = noteProjectExitRequested();
    expect(projectExitSuperseded(older)).toBe(true);
    expect(projectExitSuperseded(newer)).toBe(false);
  });

  it('a switch request overtakes every exit still waiting', () => {
    const first = noteProjectExitRequested();
    const second = noteProjectExitRequested();
    const release = noteSwitchRequested();
    try {
      expect(projectExitSuperseded(first)).toBe(true);
      expect(projectExitSuperseded(second)).toBe(true);
    } finally {
      release();
    }
  });

  it('a switch settling does not overtake anything: an exit asked for while it ran stays current', () => {
    const release = noteSwitchRequested();
    const ticket = noteProjectExitRequested();
    release();
    expect(projectExitSuperseded(ticket)).toBe(false);
  });
});
