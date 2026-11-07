/**
 * CRC-32 (IEEE 802.3, reflected polynomial 0xEDB88320).
 *
 * Two callers need the same routine and neither may pull an npm package for it:
 * `surveyFileWriters.ts` puts it in a STORED ZIP entry's header, and
 * `fixtureCorpus.ts` folds it down to the 20-bit id that stands in for a private
 * fixture's real name. Sharing one implementation means the ZIP byte-pin test
 * also pins the id function's arithmetic.
 *
 * Node 24 does expose `zlib.crc32`, but this stays hand-rolled: the harness is
 * run from checkouts whose Node is only guaranteed by `.nvmrc`, and a 20-line
 * table is cheaper than a version-gated fallback.
 */

/** Built once, on first use - 256 entries is not worth doing at import time. */
let cachedTable: Uint32Array | null = null;

function crcTable(): Uint32Array {
  if (cachedTable !== null) return cachedTable;
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let bit = 0; bit < 8; bit++) {
      c = (c & 1) !== 0 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  cachedTable = table;
  return table;
}

/** CRC-32 of `bytes`, as an unsigned 32-bit number. */
export function crc32(bytes: Uint8Array): number {
  const table = crcTable();
  let crc = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) {
    // The table has 256 entries and its index is masked to 0..255.
    crc = table[(crc ^ bytes[i]!) & 0xff]! ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** CRC-32 of `text`'s UTF-8 bytes. */
export function crc32Text(text: string): number {
  return crc32(new TextEncoder().encode(text));
}

/**
 * `text` folded to a 5-hex-character tag: CRC-32, low 20 bits.
 *
 * The publishable stand-in for a name that must not be published - see
 * `fixtureCorpus.corpusFileId`, which is this function under a domain name, and
 * `fuzzTrace.describeSteps`, which prints it for a corpus step. It lives here so
 * the two agree by construction rather than by two copies staying in step: a
 * trace line that named a different id from the metric key would be worse than
 * either alone.
 *
 * 20 bits is ~1M values, so it is a LABEL and not a key: over a corpus of a few
 * thousand files the birthday bound says a colliding pair is more likely than
 * not. That is fine for what this is for - telling two files apart in one run's
 * output, where the real path is still on the operator's own disk - and it is
 * not fine for RESOLVING one, which is what a replay does when it reads an id
 * out of a trace. `fixtureCorpus.corpusPathHash` is the resolution key; this is
 * the one that gets printed. Anything that looks a file up by this tag must
 * treat two matches as no match, and `fixtureCorpus.corpusLookup` does.
 */
export function shortId(text: string): string {
  return (crc32Text(text) & 0xfffff).toString(16).padStart(5, '0');
}
