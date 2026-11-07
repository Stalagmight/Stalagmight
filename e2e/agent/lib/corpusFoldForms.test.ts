/**
 * #576 RV18-W23-5: the fold finds a corpus name in its other plain spellings.
 *
 * The fold matched a name as it is on disk and in every escaped form JSON and
 * Rust's `{:?}` write it in. Two spellings that are not escapes left it whole:
 * a `file://` URL percent-encodes a name (`Caf%C3%A9%20Nord.srv`), and the
 * same name in the other Unicode normal form - a macOS file name comes
 * decomposed, `e` then a combining acute - is different text. Both now fold.
 * Not attempted (and not folded): a change of case, base64, or a part of a
 * name. (A form encoder's `+` and an HTML reference fold since RV19b-P5, see
 * corpusFoldShortNames.test.ts.)
 *
 * Everything here is synthetic: token names and a copy directory that is only a
 * string.
 */
import * as path from 'path';
import { pathToFileURL } from 'url';
import { describe, expect, it } from 'vitest';

import { redactCorpusText, registerCorpusRedactions } from './fixtureCorpus';

const DIR = '/tmp/qzw23five/sessions/run-1/fuzz-files/corpus-0';
/** Composed (NFC): the acute is one character, U+00E9. */
const NORD = 'Caf\u00e9 Nord.srv';
/** Decomposed (NFD): `e`, then the combining acute U+0301 - as macOS hands it out. */
const SUD = 'Cafe\u0301 Sud.srv';
const NORD_TAG = '<corpus #form1>/file';
const SUD_TAG = '<corpus #form2>/file';

registerCorpusRedactions('form1', DIR, [NORD]);
registerCorpusRedactions('form2', '', [SUD]);

describe('the fold finds a corpus name percent-encoded and in either normal form (#576 RV18-W23-5)', () => {
  it('the inputs are what they say', () => {
    expect(NORD.normalize('NFC')).toBe(NORD);
    expect(SUD.normalize('NFD')).toBe(SUD);
    expect(SUD.normalize('NFC')).not.toBe(SUD);
    expect(pathToFileURL(path.join(DIR, NORD)).href).toBe(`file://${DIR}/Caf%C3%A9%20Nord.srv`);
  });

  it('folds a name percent-encoded as a file:// URL writes it, in either hex case', () => {
    expect(redactCorpusText(pathToFileURL(path.join(DIR, NORD)).href)).toBe(`file://<corpus #form1>/${NORD_TAG}`);
    // The reviewer's probe: the copy directory folded, the name left.
    expect(redactCorpusText('file://<corpus #abcde>/Caf%C3%A9%20Nord.srv')).toBe(`file://<corpus #abcde>/${NORD_TAG}`);
    expect(redactCorpusText('open file:///x/Caf%c3%a9%20Nord.srv failed')).toBe(`open file:///x/${NORD_TAG} failed`);
    // encodeURIComponent of the whole path, as an asset URL carries it: the directory's
    // `/` is `%2F` too.
    expect(redactCorpusText(`asset://localhost/${encodeURIComponent(path.join(DIR, NORD))}`)).toBe(
      `asset://localhost/<corpus #form1>%2F${NORD_TAG}`,
    );
    // Decomposed, then percent-encoded: `e` and the acute's two bytes.
    expect(redactCorpusText('file:///x/Cafe%CC%81%20Nord.srv')).toBe(`file:///x/${NORD_TAG}`);
  });

  it('folds a composed name written decomposed, as itself and escaped', () => {
    const nfd = NORD.normalize('NFD');
    expect(nfd).not.toBe(NORD);
    expect(redactCorpusText(`could not read ${nfd}`)).toBe(`could not read ${NORD_TAG}`);
    // `{:?}` writes the combining mark as `\u{301}`; an ASCII-only JSON encoder as `\u0301`.
    expect(redactCorpusText('path="Cafe\\u{301} Nord.srv"')).toBe(`path="${NORD_TAG}"`);
    expect(redactCorpusText('{"path":"Cafe\\u0301 Nord.srv"}')).toBe(`{"path":"${NORD_TAG}"}`);
  });

  it('folds a decomposed name written composed, as itself, escaped and percent-encoded', () => {
    const nfc = SUD.normalize('NFC');
    expect(redactCorpusText(`could not read ${nfc}`)).toBe(`could not read ${SUD_TAG}`);
    expect(redactCorpusText('{"path":"Caf\\u00e9 Sud.srv"}')).toBe(`{"path":"${SUD_TAG}"}`);
    expect(redactCorpusText('file:///x/Caf%C3%A9%20Sud.srv')).toBe(`file:///x/${SUD_TAG}`);
    // ...and still as it is on disk.
    expect(redactCorpusText(`could not read ${SUD}`)).toBe(`could not read ${SUD_TAG}`);
  });
});
