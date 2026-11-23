/**
 * Name parts for sentences that word a name their own way (RS-PROSE), where
 * the builders of `utils/prose.ts` give the right pill but not the right
 * plain words: the Station details' "3a belongs to Main." names the slot by
 * its bare name (its plain label is "3a (Main)"), and a sentence that states a
 * Station ID itself ("0 (Station ID 20), Side's own station") must not draw it
 * a second time as the pill's superscript.
 */
import { isNamePart, proseParts, proseToken, type ProsePart } from './prose';

/**
 * A station's own name in a sentence that states its Station ID itself: its
 * bare name, coloured by its home line (neutral without one, as a record no
 * longer on any line is), with no line context and no superscript ID.
 */
export function ownStationPart(name: string, homeLine?: number): ProsePart {
  const part: ProsePart = { kind: 'station', text: name, name };
  if (homeLine !== undefined) part.line_id = homeLine;
  return part;
}

/**
 * The name `token` holds, read as `text` (the words the plain sentence has in
 * its place); the pill draws what it drew. A token that holds no name reads
 * as `text`.
 */
export function reworded(token: string, text: string): string {
  const part = proseParts(token).find(isNamePart);
  return part ? proseToken({ ...part, text }) : text;
}
