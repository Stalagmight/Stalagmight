/**
 * RS-PROSE: the Warnings tab draws a survey warning's opening `Shot <label> `
 * as its shot pill and the rest of the message from the message's parts, cut
 * where the body starts. A cut that would split a name, or parts that do not
 * say the body's words, give no parts: the body then reads as its words.
 */
import { describe, expect, it } from 'vitest';
import { messageBodyParts } from '../warningProse';
import { partsToPlain } from '../prose';
import type { ProsePart } from '../../types/ipc/ProsePart';

const shot: ProsePart = {
  kind: 'shot',
  text: 'A: 1 → 2',
  line_id: 1,
  context: 'A',
  from: { name: '1' },
  to: { name: '2' },
};
const end = (name: string): ProsePart => ({ kind: 'station', text: `A / ${name}`, line_id: 1, context: 'A', name });
const parts: ProsePart[] = [
  { kind: 'text', text: 'Shot ' },
  shot,
  { kind: 'text', text: ' has both ends joined by loop closure ' },
  end('1'),
  { kind: 'text', text: ' to ' },
  end('2'),
  { kind: 'text', text: ', which would squeeze the shot to zero length.' },
];
const message = partsToPlain(parts);
const body = message.slice('Shot A: 1 → 2 '.length);

describe('messageBodyParts', () => {
  it('cuts the parts where the body starts, keeping the names inside it', () => {
    const cut = messageBodyParts(message, parts, body)!;
    expect(partsToPlain(cut)).toBe(body);
    expect(cut[0]).toEqual({ kind: 'text', text: 'has both ends joined by loop closure ' });
    expect(cut.filter((p) => p.kind === 'station').map((p) => p.text)).toEqual(['A / 1', 'A / 2']);
  });

  it('gives the whole list for the whole message', () => {
    expect(messageBodyParts(message, parts, message)).toEqual(parts);
  });

  it('gives nothing for a cut inside a name, no parts, or other words', () => {
    expect(messageBodyParts(message, parts, message.slice('Shot A: 1'.length))).toBeUndefined();
    expect(messageBodyParts(message, undefined, body)).toBeUndefined();
    expect(messageBodyParts(message, [], body)).toBeUndefined();
    expect(messageBodyParts(message, parts, 'not a tail')).toBeUndefined();
    expect(messageBodyParts(`${message}!`, parts, `${body}!`)).toBeUndefined();
  });
});
