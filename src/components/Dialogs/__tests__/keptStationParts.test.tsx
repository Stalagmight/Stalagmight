/**
 * RS-FX6: the shared parts of the four kept-station dialogs.
 * - P1-1: a collision's two stations share a name, so each answer names the
 *   one it renames by role and Station ID.
 * - P2-2: the name a station has once the chosen answers apply.
 * - P3-18: the action row stays pinned at the bottom of a scrolling body.
 * Injections: word an answer by `identityLabelOf` alone; return `fallback`
 * from `answeredName` for an answered station; drop `sticky` from the
 * footer.
 */
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import '../../../i18n';
import { answeredName, collisionAnswerText, recordIdentity, StickyDialogFooter } from '../keptStationParts';
import type { NameCollision } from '../../../types/ipc/NameCollision';

const MAIN = 1;
const SIDE = 2;
const lineName = (id: number) => (id === MAIN ? 'Main' : 'Side');

/** Side / 0 comes back (Station ID 15) where Side's own 0 (29) stands. */
const collision: NameCollision = {
  line_id: SIDE,
  name: '0',
  letter: { station: { id: 15, name: '0', home_line: SIDE }, to: '0a' },
  alternative: { station: { id: 29, name: '0', home_line: SIDE }, to: '0a' },
};

describe('collisionAnswerText', () => {
  it('names the dialog station by its role and the other by its line, both with Station IDs', () => {
    const comesBack = { id: 15, role: 'comesBack' as const };
    expect(collisionAnswerText(collision.letter, collision, comesBack, lineName)).toBe(
      'Rename the station that comes back (Side / 0, Station ID 15) to 0a',
    );
    expect(collisionAnswerText(collision.alternative, collision, comesBack, lineName)).toBe(
      "Rename Side's own 0 (Station ID 29) to 0a",
    );
  });

  it('names the kept station by its role, and a station of another line by its home line', () => {
    const merge: NameCollision = {
      line_id: SIDE,
      name: '12',
      letter: { station: { id: 27, name: '12', home_line: SIDE }, to: '12a' },
      alternative: { station: { id: 12, name: '12', home_line: MAIN }, to: '12a' },
    };
    expect(collisionAnswerText(merge.alternative, merge, { id: 12, role: 'kept' }, lineName)).toBe(
      'Rename the kept station (Main / 12, Station ID 12) to 12a',
    );
    expect(collisionAnswerText(merge.alternative, merge, undefined, lineName)).toBe('Rename Main / 12 (Station ID 12) to 12a');
  });

  it('never words the two answers of one collision alike', () => {
    for (const subject of [undefined, { id: 15, role: 'comesBack' as const }, { id: 29, role: 'kept' as const }]) {
      expect(collisionAnswerText(collision.letter, collision, subject, lineName)).not.toBe(
        collisionAnswerText(collision.alternative, collision, subject, lineName),
      );
    }
  });
});

describe('answeredName', () => {
  it('follows the answer: the letter, or the own name when the other station takes it', () => {
    expect(answeredName(15, '0a', [collision], ['letter'])).toBe('0a');
    expect(answeredName(15, '0a', [collision], ['alternative'])).toBe('0');
    expect(answeredName(29, '0', [collision], ['alternative'])).toBe('0a');
    // A station in no collision keeps the preview's name.
    expect(answeredName(40, '7', [collision], ['letter'])).toBe('7');
  });
});

describe('recordIdentity', () => {
  it("takes the record's first line as its home line", () => {
    expect(recordIdentity({ id: 46, name: '0', lines: [SIDE, MAIN] })).toEqual({ id: 46, name: '0', home_line: SIDE });
    expect(recordIdentity({ id: 47, name: '9', lines: [] })).toEqual({ id: 47, name: '9' });
  });
});

describe('StickyDialogFooter', () => {
  it('pins its row to the bottom of the scrolling body', () => {
    render(
      <StickyDialogFooter testId="footer">
        <button type="button">Detach</button>
      </StickyDialogFooter>,
    );
    const footer = screen.getByTestId('footer');
    expect(footer.className).toMatch(/\bsticky\b/);
    expect(footer.className).toMatch(/-bottom-4/);
    expect(footer.className).toMatch(/bg-panel/);
  });
});
