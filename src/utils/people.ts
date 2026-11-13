/**
 * People (surveyors / explorers): identity, labels and autocomplete ranking.
 *
 * Identity is the pair (name, email): the same name AND the same email is one
 * person, and a person without an email only equals another person without an
 * email. It is EXACTLY the backend's `Person::same_as`
 * (`src-tauri/crates/models/src/credit.rs`), so the frontend never merges two
 * people the backend keeps apart or the other way round: the name is split on
 * Unicode whitespace, re-joined with one space and lower-cased; the email is
 * trimmed and lower-cased. Matching (autocomplete) is looser than identity: it
 * also ignores accents, so typing "jose" finds "José" - but "José" and "Jose"
 * stay two different people.
 *
 * Pure functions only, so the pill input and any table that lists people can
 * share them.
 */

/** The minimal shape of a person. The backend's generated `Person` fits it. */
export interface PersonLike {
  name: string;
  email?: string | null;
}

/** Where an autocomplete candidate came from. */
export type SuggestionSource = 'project' | 'recent' | 'speleodb';

export interface Suggestion {
  person: PersonLike;
  source: SuggestionSource;
}

/**
 * How a ranked suggestion matched the query, best first. `any` is the empty
 * query, which matches everyone.
 */
export type MatchKind = 'name-prefix' | 'name-word' | 'email-prefix' | 'substring' | 'any';

/** A `[start, end)` range of UTF-16 offsets. */
export type TextRange = [number, number];

export interface RankedSuggestion extends Suggestion {
  match: MatchKind;
  /** The matched span of `normalizeName(person.name)`, or null. */
  nameRange: TextRange | null;
  /** The matched span of the trimmed email, or null. */
  emailRange: TextRange | null;
}

/** Tie-break order for equally good matches: project people first. */
export const SOURCE_ORDER: readonly SuggestionSource[] = ['project', 'speleodb', 'recent'];

const MATCH_ORDER: readonly MatchKind[] = ['name-prefix', 'name-word', 'email-prefix', 'substring', 'any'];

/**
 * Display form of a name: trimmed, every run of whitespace collapsed to one
 * space, and NFC-composed so a name typed on a keyboard that emits decomposed
 * accents ("e" + U+0301) is stored the same as the precomposed one.
 */
export function normalizeName(name: string): string {
  return name.normalize('NFC').trim().replace(/\s+/g, ' ');
}

/**
 * Case-fold for MATCHING only: upper-casing first turns "ß" into "SS" (and so
 * "ss"), which a plain `toLowerCase` leaves alone. Locale-independent on
 * purpose. Identity does not use it (see {@link personKey}).
 */
function caseFold(text: string): string {
  return text.toUpperCase().toLowerCase();
}

/**
 * Rust's `char::is_whitespace` (the Unicode White_Space property), which
 * `str::split_whitespace` and `str::trim` use. JavaScript's `\s` differs in
 * two code points (it adds U+FEFF and lacks U+0085), so it is spelled out.
 */
const RUST_WHITESPACE_CLASS = '\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000';
const RUST_WHITESPACE_RUN = new RegExp(`[${RUST_WHITESPACE_CLASS}]+`);
const RUST_TRIM = new RegExp(`^[${RUST_WHITESPACE_CLASS}]+|[${RUST_WHITESPACE_CLASS}]+$`, 'g');

/** `Person::name_key`: whitespace runs collapsed, ends trimmed, lower-cased. */
function nameKey(name: string): string {
  return name
    .split(RUST_WHITESPACE_RUN)
    .filter((part) => part !== '')
    .join(' ')
    .toLowerCase();
}

/** `Person::email_key`: trimmed and lower-cased; an empty email is none (''). */
function emailKey(email: string | null | undefined): string {
  return (email ?? '').replace(RUST_TRIM, '').toLowerCase();
}

/**
 * Identity key: `nameKey`, U+0000, `emailKey` (empty when there is none).
 * U+0000 cannot appear in a typed name, so the join is unambiguous. Equal keys
 * are exactly the backend's `Person::same_as`.
 */
export function personKey(person: PersonLike): string {
  return `${nameKey(person.name)}\u0000${emailKey(person.email)}`;
}

export function samePerson(a: PersonLike, b: PersonLike): boolean {
  return personKey(a) === personKey(b);
}

/** `Person::is_valid`: false for a name that is empty after trimming. */
export function isValidPerson(person: PersonLike): boolean {
  return nameKey(person.name) !== '';
}

/**
 * Whether typed text names `person` exactly: the same name, or the same email
 * (both compared as identity compares them). What a typed name resolves
 * through when it is committed as text rather than picked from the list.
 */
export function typedNames(typed: string, person: PersonLike): boolean {
  const name = nameKey(typed);
  if (name === '') return false;
  return name === nameKey(person.name) || emailKey(typed) === emailKey(person.email);
}

/**
 * Drop later duplicates (by {@link personKey}), keeping the first of each, and
 * people without a name - the backend's `dedupe_people`.
 */
export function dedupePeople<T extends PersonLike>(people: readonly T[]): T[] {
  const seen = new Set<string>();
  const out: T[] = [];
  for (const person of people) {
    if (!isValidPerson(person)) continue;
    const key = personKey(person);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(person);
  }
  return out;
}

function splitEmail(email: string): { local: string; domain: string } {
  const at = email.lastIndexOf('@');
  return at < 0 ? { local: email, domain: '' } : { local: email.slice(0, at), domain: email.slice(at + 1) };
}

function tally(values: readonly string[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1);
  return counts;
}

/**
 * The label each person should be shown with, keyed by {@link personKey}.
 *
 * A name that only one known person carries is shown as is. When two or more
 * share it (case-insensitively), each gets the shortest email hint that is
 * unique within that same-name group: the local part ("John Smith (jsmith)"),
 * else the domain ("John Smith (@caves.org)"), else the whole email. The
 * group-wide uniqueness is deliberate: "(@caves.org)" is only shown when no
 * other John Smith is at caves.org, so a hint never points at two people. A
 * same-named person without an email is "(no email)" (`options.noEmail`
 * overrides the text for translation).
 */
export function disambiguationLabels(
  people: readonly PersonLike[],
  options: { noEmail?: string } = {},
): Map<string, string> {
  const noEmail = options.noEmail ?? 'no email';
  const groups = new Map<string, PersonLike[]>();
  for (const person of dedupePeople(people)) {
    const key = nameKey(person.name);
    const group = groups.get(key);
    if (group) group.push(person);
    else groups.set(key, [person]);
  }

  const labels = new Map<string, string>();
  for (const group of groups.values()) {
    if (group.length === 1) {
      const only = group[0]!;
      labels.set(personKey(only), normalizeName(only.name));
      continue;
    }
    const emails = group.map((p) => emailKey(p.email)).filter((e) => e !== '');
    const locals = tally(emails.map((e) => splitEmail(e).local));
    const domains = tally(emails.map((e) => splitEmail(e).domain));
    for (const person of group) {
      const email = (person.email ?? '').trim();
      let hint = noEmail;
      if (email !== '') {
        const { local, domain } = splitEmail(email);
        if (local !== '' && locals.get(local.toLowerCase()) === 1) hint = local;
        else if (domain !== '' && domains.get(domain.toLowerCase()) === 1) hint = `@${domain}`;
        else hint = email;
      }
      labels.set(personKey(person), `${normalizeName(person.name)} (${hint})`);
    }
  }
  return labels;
}

/**
 * Latin letters NFD does not decompose, so stripping marks alone would leave
 * "Łukasz" unmatched by "lukasz". Keys are already case-folded.
 */
const EXTRA_FOLDS: Readonly<Record<string, string>> = {
  'ł': 'l',
  'ø': 'o',
  'đ': 'd',
  'ð': 'd',
  'ħ': 'h',
  'ı': 'i',
  'æ': 'ae',
  'œ': 'oe',
  'þ': 'th',
};

const COMBINING_MARK = /\p{M}/u;
const COMBINING_MARKS = /\p{M}/gu;
const WORD_CHAR = /[\p{L}\p{N}]/u;

function foldChar(ch: string): string {
  const folded = caseFold(ch.normalize('NFD').replace(COMBINING_MARKS, ''));
  return EXTRA_FOLDS[folded] ?? folded;
}

/** Case- and accent-insensitive form of `text`, for matching only. */
export function foldForMatch(text: string): string {
  let out = '';
  for (const ch of text) out += foldChar(ch);
  return out;
}

interface FoldedText {
  text: string;
  /** For each UTF-16 unit of `text`: where its source code point starts / ends in the original. */
  starts: number[];
  ends: number[];
}

function foldMapped(original: string): FoldedText {
  let text = '';
  const starts: number[] = [];
  const ends: number[] = [];
  let at = 0;
  for (const ch of original) {
    const folded = foldChar(ch);
    for (let i = 0; i < folded.length; i++) {
      starts.push(at);
      ends.push(at + ch.length);
    }
    text += folded;
    at += ch.length;
  }
  return { text, starts, ends };
}

/** Map a match at `[index, index + length)` of the folded text back to the original. */
function originalRange(original: string, folded: FoldedText, index: number, length: number): TextRange {
  const start = folded.starts[index] ?? 0;
  let end = folded.ends[index + length - 1] ?? original.length;
  // Keep a trailing combining accent with its letter, so bolding never splits
  // "e" from its U+0301.
  while (end < original.length && COMBINING_MARK.test(original[end]!)) end++;
  return [start, end];
}

interface PersonMatch {
  match: MatchKind;
  nameRange: TextRange | null;
  emailRange: TextRange | null;
}

function matchPerson(query: string, person: PersonLike): PersonMatch | null {
  if (query === '') return { match: 'any', nameRange: null, emailRange: null };
  const name = normalizeName(person.name);
  const email = (person.email ?? '').trim();
  const foldedName = foldMapped(name);
  const foldedEmail = foldMapped(email);

  if (foldedName.text.startsWith(query)) {
    return { match: 'name-prefix', nameRange: originalRange(name, foldedName, 0, query.length), emailRange: null };
  }
  for (let at = foldedName.text.indexOf(query, 1); at > 0; at = foldedName.text.indexOf(query, at + 1)) {
    if (!WORD_CHAR.test(foldedName.text[at - 1]!)) {
      return { match: 'name-word', nameRange: originalRange(name, foldedName, at, query.length), emailRange: null };
    }
  }
  if (foldedEmail.text.startsWith(query)) {
    return { match: 'email-prefix', nameRange: null, emailRange: originalRange(email, foldedEmail, 0, query.length) };
  }
  const inName = foldedName.text.indexOf(query);
  if (inName >= 0) {
    return { match: 'substring', nameRange: originalRange(name, foldedName, inName, query.length), emailRange: null };
  }
  const inEmail = foldedEmail.text.indexOf(query);
  if (inEmail >= 0) {
    return { match: 'substring', nameRange: null, emailRange: originalRange(email, foldedEmail, inEmail, query.length) };
  }
  return null;
}

const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });

function sourceRank(source: SuggestionSource): number {
  const rank = SOURCE_ORDER.indexOf(source);
  return rank < 0 ? SOURCE_ORDER.length : rank;
}

/**
 * Autocomplete: the candidates matching `query`, best first, at most `limit`.
 *
 * The query matches the name and the email, ignoring case and accents. Rank:
 * name prefix, then the start of a later word of the name ("smi" in "John
 * Smith"), then email prefix, then a substring of either. Equal matches order
 * by source (project, SpeleoDB, recent), then alphabetically by name.
 *
 * Candidates sharing a {@link personKey} are one person and appear once: the
 * SpeleoDB entry is kept for its metadata (so it carries the SpeleoDB badge),
 * ranked at the best source any of its copies came from. Two people with the
 * same name but different (or missing) emails are different people and both
 * stay. People in `exclude` (already selected) are dropped. An empty query
 * matches everyone, ordered by source then name.
 */
export function rankSuggestions(
  query: string,
  candidates: readonly Suggestion[],
  exclude: readonly PersonLike[] = [],
  limit = 8,
): RankedSuggestion[] {
  const excluded = new Set(exclude.map(personKey));
  const merged = new Map<string, { suggestion: Suggestion; rank: number }>();
  for (const candidate of candidates) {
    const key = personKey(candidate.person);
    if (excluded.has(key)) continue;
    const rank = sourceRank(candidate.source);
    const existing = merged.get(key);
    if (!existing) {
      merged.set(key, { suggestion: candidate, rank });
      continue;
    }
    existing.rank = Math.min(existing.rank, rank);
    if (candidate.source === 'speleodb' && existing.suggestion.source !== 'speleodb') {
      existing.suggestion = candidate;
    }
  }

  const folded = foldForMatch(normalizeName(query));
  const ranked: Array<RankedSuggestion & { rank: number; key: string }> = [];
  for (const [key, { suggestion, rank }] of merged) {
    const found = matchPerson(folded, suggestion.person);
    if (found) ranked.push({ ...suggestion, ...found, rank, key });
  }
  ranked.sort(
    (a, b) =>
      MATCH_ORDER.indexOf(a.match) - MATCH_ORDER.indexOf(b.match) ||
      a.rank - b.rank ||
      collator.compare(normalizeName(a.person.name), normalizeName(b.person.name)) ||
      collator.compare(emailKey(a.person.email), emailKey(b.person.email)) ||
      (a.key < b.key ? -1 : a.key > b.key ? 1 : 0),
  );
  return ranked
    .slice(0, Math.max(0, limit))
    .map(({ person, source, match, nameRange, emailRange }) => ({ person, source, match, nameRange, emailRange }));
}

/**
 * Split typed or pasted text into names. A semicolon always separates, and so
 * does a line break (a column pasted from a spreadsheet); a comma never does,
 * because "Doe, John" is typed on purpose. Names come back normalized, empty
 * ones dropped.
 */
export function splitTypedNames(text: string): string[] {
  return text
    .split(/[;\r\n]/)
    .map(normalizeName)
    .filter((name) => name !== '');
}
