/**
 * A warning's message body as prose parts (RS-PROSE): the Warnings tab draws
 * a survey warning's opening `Shot <label> ` as its own shot pill
 * (`warningMessageBody` strips it), and the rest of the message, the body,
 * names further stations and lines (`Shot A: 1 → 2 has both ends joined by
 * loop closure A / 1 to A / 2, ...`). The backend sends the whole message's
 * parts (`ProjectWarning.messageParts`); these are the body's.
 */
import { partsToPlain, type ProsePart } from './prose';

/**
 * The parts of `body`, a tail of `message`, cut from `parts` (the whole
 * message's): every part before the body dropped, a words part the body
 * starts inside of cut. `undefined` (the body then reads as its words) when
 * there are no parts, `body` is not a tail of `message`, the cut falls inside
 * a name, or the parts do not say the body's words.
 */
export function messageBodyParts(
  message: string,
  parts: readonly ProsePart[] | null | undefined,
  body: string,
): ProsePart[] | undefined {
  if (!parts || parts.length === 0 || !message.endsWith(body)) return undefined;
  let skip = message.length - body.length;
  const out: ProsePart[] = [];
  for (const part of parts) {
    if (skip === 0) {
      out.push(part);
      continue;
    }
    if (part.text.length <= skip) {
      skip -= part.text.length;
      continue;
    }
    // The body starts inside this part: only words can be cut.
    if (part.kind !== 'text') return undefined;
    out.push({ kind: 'text', text: part.text.slice(skip) });
    skip = 0;
  }
  return partsToPlain(out) === body ? out : undefined;
}
