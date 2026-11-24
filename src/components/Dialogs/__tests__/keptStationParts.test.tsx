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
import { answeredName, collisionAnswerProse, collisionAnswerText, recordIdentity, StickyDialogFooter } from '../keptStationParts';
import { keptProse } from '../keptStationProse';
import type { NameCollision } from '../../../types/ipc/NameCollision';
import { proseParts, proseToPlain } from '../../../utils/prose';

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
      "Rename the 0 that is on Side now (Station ID 29) to 0a",
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

/**
 * RS-PROSE: the prose variant words an answer exactly as the plain one, its
 * station and the collision's line as parts; the Station ID the words say is
 * not drawn again, and the new name stays words. Injection: word the prose
 * variant apart from `answerWords` (a copy that drifts) fails the first case.
 */
describe('collisionAnswerProse', () => {
  const src = { lines: [{ id: MAIN, name: 'Main' }, { id: SIDE, name: 'Side' }] };
  const prose = keptProse(src);
  const subjects = [undefined, { id: 15, role: 'comesBack' as const }, { id: 29, role: 'kept' as const }];

  it('says the same words as collisionAnswerText', () => {
    for (const subject of subjects) {
      for (const which of ['letter', 'alternative'] as const) {
        expect(proseToPlain(collisionAnswerProse(collision[which], collision, subject, prose))).toBe(
          collisionAnswerText(collision[which], collision, subject, lineName),
        );
      }
    }
  });

  it("names the station and the line as parts, without the Station ID the words say", () => {
    const own = proseParts(collisionAnswerProse(collision.alternative, collision, { id: 15, role: 'comesBack' }, prose));
    expect(own.filter((p) => p.kind !== 'text')).toEqual([
      { kind: 'station', text: '0', name: '0', line_id: SIDE },
      { kind: 'line', text: 'Side', name: 'Side', line_id: SIDE },
    ]);
    const role = proseParts(collisionAnswerProse(collision.letter, collision, { id: 15, role: 'comesBack' }, prose));
    expect(role.filter((p) => p.kind !== 'text')).toEqual([
      { kind: 'station', text: 'Side / 0', name: '0', line_id: SIDE, context: 'Side' },
    ]);
    // The new name is words.
    expect(role[role.length - 1]).toEqual({ kind: 'text', text: ', Station ID 15) to 0a' });
  });
});

describe('keptProse', () => {
  it('keeps an identity\'s Station ID unless the words say it, and names a slot by the words given', () => {
    const src = {
      lines: [{ id: MAIN, name: 'Main' }, { id: SIDE, name: 'Side' }],
      stations: [
        {
          station_ref: { line_id: SIDE, station_index: 0 },
          coordinates: { x: 0, y: 0, z: 0 },
          is_zero_datum: false,
          gps_coordinates: null,
          merged_with: [],
          id: 11,
          name: '12',
          shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false },
        },
      ],
    };
    const prose = keptProse(src);
    const identity = { id: 46, name: '0', home_line: MAIN };
    expect(proseParts(prose.identity(identity))[0]).toMatchObject({ text: 'Main / 0', station_id: 46 });
    expect(proseParts(prose.identityNoId(identity))[0]).not.toHaveProperty('station_id');
    expect(proseParts(prose.identityNoId(identity))[0]).toMatchObject({ text: 'Main / 0', context: 'Main' });
    // A foreign slot by its bare name: the words stay "12", the pill draws
    // the slot's home-line dot and Station ID (the plain label is "12 (Main)").
    expect(proseParts(prose.slotName({ line_id: SIDE, station_index: 0 }, '12'))[0]).toEqual({
      kind: 'station',
      text: '12',
      name: '12',
      line_id: SIDE,
      slot: { line_id: SIDE, station_index: 0 },
      home_line: MAIN,
      station_id: 11,
    });
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
