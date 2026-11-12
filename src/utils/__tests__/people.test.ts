import { describe, expect, it } from 'vitest';
import {
  dedupePeople,
  disambiguationLabels,
  foldForMatch,
  normalizeName,
  personKey,
  rankSuggestions,
  samePerson,
  splitTypedNames,
  type PersonLike,
  type Suggestion,
} from '../people';

const project = (name: string, email?: string | null): Suggestion => ({
  person: email === undefined ? { name } : { name, email },
  source: 'project',
});
const speleo = (name: string, email?: string | null): Suggestion => ({
  person: email === undefined ? { name } : { name, email },
  source: 'speleodb',
});
const recent = (name: string, email?: string | null): Suggestion => ({
  person: email === undefined ? { name } : { name, email },
  source: 'recent',
});
const names = (list: Suggestion[]) => list.map((s) => s.person.name);

describe('normalizeName', () => {
  it('trims and collapses internal whitespace, including tabs and non-breaking spaces', () => {
    expect(normalizeName('  John \t  Smith  Jr ')).toBe('John Smith Jr');
  });

  it('composes decomposed accents so both spellings display the same', () => {
    expect(normalizeName('José')).toBe('José');
  });

  it('keeps case and punctuation as typed', () => {
    expect(normalizeName("Doe, john O'Brien")).toBe("Doe, john O'Brien");
  });
});

describe('personKey / samePerson', () => {
  it('ignores name case and whitespace, and email case and surrounding space', () => {
    expect(samePerson({ name: ' john  SMITH', email: ' JS@Caves.org ' }, { name: 'John Smith', email: 'js@caves.org' })).toBe(true);
  });

  it('treats a missing, null, empty and blank email alike', () => {
    const keys = new Set([
      personKey({ name: 'Ann' }),
      personKey({ name: 'Ann', email: null }),
      personKey({ name: 'Ann', email: '' }),
      personKey({ name: 'Ann', email: '   ' }),
    ]);
    expect(keys.size).toBe(1);
  });

  it('a person without an email never equals the same name with an email', () => {
    expect(samePerson({ name: 'Ann' }, { name: 'Ann', email: 'ann@x.org' })).toBe(false);
  });

  it('different emails are different people even with the same name', () => {
    expect(samePerson({ name: 'Ann', email: 'a@x.org' }, { name: 'Ann', email: 'b@x.org' })).toBe(false);
  });

  it('accents are part of identity (matching ignores them, identity does not)', () => {
    expect(samePerson({ name: 'José' }, { name: 'Jose' })).toBe(false);
  });

  // Identity is exactly the backend's `Person::same_as` (name split on Unicode
  // whitespace and lower-cased; email trimmed and lower-cased): anything looser
  // would merge two people the backend keeps apart.
  it('compares like Rust: no Unicode normalization, no sharp-s folding', () => {
    expect(samePerson({ name: 'Jose\u0301' }, { name: 'Jos\u00e9' })).toBe(false);
    expect(samePerson({ name: 'Strau\u00df' }, { name: 'STRAUSS' })).toBe(false);
    expect(samePerson({ name: 'STRAU\u00df' }, { name: 'strau\u00df' })).toBe(true);
  });

  it('collapses the whitespace Rust collapses, and only that', () => {
    // NEL (U+0085) and NBSP are White_Space in Rust; U+FEFF is not.
    expect(samePerson({ name: 'Ann\u0085Lee' }, { name: 'ann lee' })).toBe(true);
    expect(samePerson({ name: '\u00a0Ann \u3000 Lee\u2003' }, { name: 'ann lee' })).toBe(true);
    expect(samePerson({ name: 'Ann\ufeffLee' }, { name: 'ann lee' })).toBe(false);
  });

  it('trims and lower-cases the email but keeps inner text', () => {
    expect(samePerson({ name: 'Ann', email: '\u00a0A@X.org\n' }, { name: 'ann', email: 'a@x.org' })).toBe(true);
    expect(samePerson({ name: 'Ann', email: 'a @x.org' }, { name: 'ann', email: 'a@x.org' })).toBe(false);
  });

  it('cannot be confused by a name that ends where an email would start', () => {
    expect(personKey({ name: 'a', email: 'b' })).not.toBe(personKey({ name: 'a b' }));
  });
});

describe('dedupePeople', () => {
  it('keeps the first of each identity, in order', () => {
    const list: PersonLike[] = [
      { name: 'Ann' },
      { name: 'Bob', email: 'bob@x.org' },
      { name: 'ann ' },
      { name: 'Bob' },
      { name: 'BOB', email: 'Bob@X.org' },
    ];
    expect(dedupePeople(list)).toEqual([{ name: 'Ann' }, { name: 'Bob', email: 'bob@x.org' }, { name: 'Bob' }]);
  });

  it('returns an empty list for an empty input', () => {
    expect(dedupePeople([])).toEqual([]);
  });

  it('drops people without a name, like the backend', () => {
    expect(dedupePeople([{ name: '  ' }, { name: 'Ann' }, { name: '\u3000' }])).toEqual([{ name: 'Ann' }]);
  });
});

describe('disambiguationLabels', () => {
  const label = (labels: Map<string, string>, person: PersonLike) => labels.get(personKey(person));

  it('shows a unique name as the plain (normalized) name', () => {
    const ann = { name: '  Ann   Lee ', email: 'ann@x.org' };
    expect(label(disambiguationLabels([ann, { name: 'Bob' }]), ann)).toBe('Ann Lee');
  });

  it('uses the email local part when it tells same-named people apart', () => {
    const a = { name: 'John Smith', email: 'jsmith@caves.org' };
    const b = { name: 'John Smith', email: 'john.smith@caves.org' };
    const labels = disambiguationLabels([a, b]);
    expect(label(labels, a)).toBe('John Smith (jsmith)');
    expect(label(labels, b)).toBe('John Smith (john.smith)');
  });

  it('falls back to the domain when local parts collide', () => {
    const a = { name: 'John Smith', email: 'jsmith@caves.org' };
    const b = { name: 'John Smith', email: 'jsmith@diving.net' };
    const labels = disambiguationLabels([a, b]);
    expect(label(labels, a)).toBe('John Smith (@caves.org)');
    expect(label(labels, b)).toBe('John Smith (@diving.net)');
  });

  it('falls back to the full email when neither part is unique in the group', () => {
    const a = { name: 'John Smith', email: 'jsmith@caves.org' };
    const b = { name: 'John Smith', email: 'jsmith@diving.net' };
    const c = { name: 'John Smith', email: 'john@caves.org' };
    const labels = disambiguationLabels([a, b, c]);
    // a's local part collides with b and its domain with c: only the full email is unique.
    expect(label(labels, a)).toBe('John Smith (jsmith@caves.org)');
    expect(label(labels, b)).toBe('John Smith (@diving.net)');
    expect(label(labels, c)).toBe('John Smith (john)');
    expect(new Set(labels.values()).size).toBe(3);
  });

  it('labels a same-named person without an email "(no email)"', () => {
    const a = { name: 'John Smith', email: 'jsmith@caves.org' };
    const b = { name: 'john  smith' };
    const labels = disambiguationLabels([a, b]);
    expect(label(labels, a)).toBe('John Smith (jsmith)');
    expect(label(labels, b)).toBe('john smith (no email)');
  });

  it('accepts a translated "no email" text', () => {
    const labels = disambiguationLabels([{ name: 'A', email: 'a@x' }, { name: 'A' }], { noEmail: 'sans e-mail' });
    expect(label(labels, { name: 'A' })).toBe('A (sans e-mail)');
  });

  it('compares local parts and domains case-insensitively but shows them as written', () => {
    const a = { name: 'Eve', email: 'Eve@Caves.org' };
    const b = { name: 'Eve', email: 'eve@Diving.net' };
    const labels = disambiguationLabels([a, b]);
    expect(label(labels, a)).toBe('Eve (@Caves.org)');
    expect(label(labels, b)).toBe('Eve (@Diving.net)');
  });

  it('collapses duplicate identities before deciding whether a name is shared', () => {
    const a = { name: 'Ann', email: 'ann@x.org' };
    const labels = disambiguationLabels([a, { name: 'ANN', email: 'Ann@X.org' }]);
    expect(label(labels, a)).toBe('Ann');
    expect(labels.size).toBe(1);
  });

  it('only disambiguates within a same-name group', () => {
    const a = { name: 'John Smith', email: 'j@x.org' };
    const b = { name: 'John Smith' };
    const c = { name: 'Jane Smith', email: 'j@x.org' };
    const labels = disambiguationLabels([a, b, c]);
    expect(label(labels, c)).toBe('Jane Smith');
    expect(label(labels, a)).toBe('John Smith (j)');
  });

  it('handles an email without an @', () => {
    const a = { name: 'Kim', email: 'kim' };
    const b = { name: 'Kim', email: 'kim@x.org' };
    const labels = disambiguationLabels([a, b]);
    // Both local parts are "kim"; the malformed one has no domain, so it shows whole.
    expect(label(labels, a)).toBe('Kim (kim)');
    expect(label(labels, b)).toBe('Kim (@x.org)');
  });
});

describe('foldForMatch', () => {
  it('strips accents and folds case', () => {
    expect(foldForMatch('Jörg Émile')).toBe('jorg emile');
  });

  it('folds letters NFD does not decompose', () => {
    expect(foldForMatch('Łukasz Sørensen Æsir')).toBe('lukasz sorensen aesir');
  });
});

describe('rankSuggestions', () => {
  it('ranks name prefix > word start > email prefix > substring', () => {
    const candidates = [
      project('Aldo Castillo', 'aldo@x.org'), // word start
      project('Bocassi', 'b@x.org'), // substring
      project('Zed', 'cass@x.org'), // email prefix
      project('Cassandra', null), // name prefix
    ];
    const ranked = rankSuggestions('cas', candidates);
    expect(names(ranked)).toEqual(['Cassandra', 'Aldo Castillo', 'Zed', 'Bocassi']);
    expect(ranked.map((r) => r.match)).toEqual(['name-prefix', 'name-word', 'email-prefix', 'substring']);
  });

  it('treats hyphens and apostrophes as word starts', () => {
    const ranked = rankSuggestions('luc', [project('Bluc Ann'), project('Jean-Luc Picard')]);
    expect(names(ranked)).toEqual(['Jean-Luc Picard', 'Bluc Ann']);
    expect(rankSuggestions('brien', [project("Pat O'Brien")])[0]?.match).toBe('name-word');
  });

  it('breaks ties by source (project, speleodb, recent) then alphabetically', () => {
    const ranked = rankSuggestions('a', [
      recent('Abe'),
      speleo('Ava'),
      project('Amy'),
      project('Al'),
      speleo('Ada'),
    ]);
    expect(names(ranked)).toEqual(['Al', 'Amy', 'Ada', 'Ava', 'Abe']);
  });

  it('matches without case or accents and maps the highlight back onto the original name', () => {
    const [hit] = rankSuggestions('jose', [project('José María')]);
    expect(hit?.match).toBe('name-prefix');
    expect(hit?.nameRange).toEqual([0, 4]);
    const [inner] = rankSuggestions('maria', [project('José María')]);
    expect(inner?.nameRange).toEqual([5, 10]);
  });

  it('keeps a trailing combining accent inside the highlighted range', () => {
    // A decomposed name normalizes to NFC for display, so the range is over the NFC form.
    const [hit] = rankSuggestions('jose', [project('José')]);
    expect(hit?.nameRange).toEqual([0, 4]);
  });

  it('maps a range across a letter that folds to two (sharp s)', () => {
    const [hit] = rankSuggestions('strauss', [project('Johann Strauß')]);
    expect(hit?.match).toBe('name-word');
    expect(hit?.nameRange).toEqual([7, 13]);
  });

  it('reports the email range for email matches', () => {
    const [hit] = rankSuggestions('jsm', [project('John Smith', 'JSmith@caves.org')]);
    expect(hit?.match).toBe('email-prefix');
    expect(hit?.emailRange).toEqual([0, 3]);
    expect(hit?.nameRange).toBeNull();
    const [domain] = rankSuggestions('caves', [project('John Smith', 'jsmith@caves.org')]);
    expect(domain?.match).toBe('substring');
    expect(domain?.emailRange).toEqual([7, 12]);
  });

  it('collapses whitespace in the query', () => {
    expect(names(rankSuggestions('  john   sm ', [project('John Smith')]))).toEqual(['John Smith']);
  });

  it('drops candidates that do not match', () => {
    expect(rankSuggestions('xyz', [project('John Smith', 'js@x.org')])).toEqual([]);
  });

  it('dedupes identical identities, keeping the SpeleoDB entry but the best source rank', () => {
    const ranked = rankSuggestions('j', [
      recent('Jo Zed'),
      project('Jo Ann', 'jo@x.org'),
      speleo('JO ANN', 'Jo@X.org'),
      speleo('Jo Bee'),
    ]);
    expect(ranked).toHaveLength(3);
    // Jo Ann is a project person (rank 0) shown with its SpeleoDB metadata.
    expect(ranked[0]?.person).toEqual({ name: 'JO ANN', email: 'Jo@X.org' });
    expect(ranked[0]?.source).toBe('speleodb');
    expect(names(ranked)).toEqual(['JO ANN', 'Jo Bee', 'Jo Zed']);
  });

  it('keeps same-named people with different or missing emails as separate entries', () => {
    const ranked = rankSuggestions('john', [
      project('John Smith', 'a@x.org'),
      project('John Smith', 'b@x.org'),
      project('John Smith'),
    ]);
    expect(ranked).toHaveLength(3);
  });

  it('excludes people already selected, by identity', () => {
    const ranked = rankSuggestions(
      'j',
      [project('John Smith', 'a@x.org'), project('John Smith'), project('Jane')],
      [{ name: 'john smith', email: 'A@x.org' }],
    );
    expect(ranked.map((r) => r.person)).toEqual([{ name: 'Jane' }, { name: 'John Smith' }]);
  });

  it('honours the limit (default 8)', () => {
    const many = Array.from({ length: 12 }, (_, i) => project(`Person ${String(i).padStart(2, '0')}`));
    expect(rankSuggestions('p', many)).toHaveLength(8);
    expect(rankSuggestions('p', many, [], 3)).toHaveLength(3);
    expect(rankSuggestions('p', many, [], 0)).toHaveLength(0);
  });

  it('returns everyone for an empty query, ordered by source then name', () => {
    const ranked = rankSuggestions('  ', [recent('Abe'), project('Zoe'), speleo('Bea')]);
    expect(names(ranked)).toEqual(['Zoe', 'Bea', 'Abe']);
    expect(ranked.every((r) => r.match === 'any' && r.nameRange === null)).toBe(true);
  });

  it('sorts numerically inside names', () => {
    expect(names(rankSuggestions('diver', [project('Diver 10'), project('Diver 9')]))).toEqual(['Diver 9', 'Diver 10']);
  });
});

describe('splitTypedNames', () => {
  it('splits on semicolons and trims', () => {
    expect(splitTypedNames(' Ann ; Bob;Cy ')).toEqual(['Ann', 'Bob', 'Cy']);
  });

  it('never splits on commas', () => {
    expect(splitTypedNames('Doe, John; Roe, Jane')).toEqual(['Doe, John', 'Roe, Jane']);
  });

  it('splits on line breaks (a pasted column)', () => {
    expect(splitTypedNames('Ann\r\nBob\nCy')).toEqual(['Ann', 'Bob', 'Cy']);
  });

  it('drops empty names and collapses whitespace', () => {
    expect(splitTypedNames(';;  ;Ann   Lee;; ')).toEqual(['Ann Lee']);
    expect(splitTypedNames('')).toEqual([]);
  });
});
