import { describe, expect, it } from 'vitest';
import { buildSectionLabels, cleanSectionName, elideMiddle, graphemes, SECTION_LABEL_MAX } from '../sectionLabels';

const def = (id: number) => `Section ${id}`;
const labelsOf = (names: (string | null)[], firstId = 1) =>
  buildSectionLabels(
    names.map((name, i) => ({ id: firstId + i, name })),
    def,
  );

function expectUniqueAndShort(labels: Map<number, string>) {
  const values = [...labels.values()];
  expect(new Set(values).size).toBe(values.length);
  for (const v of values) expect(graphemes(v).length).toBeLessThanOrEqual(SECTION_LABEL_MAX);
}

describe('buildSectionLabels', () => {
  it('keeps names that fit and uses the default for unnamed sections', () => {
    const labels = labelsOf(['Main', null, '   ', 'Upstream B']);
    expect(labels.get(1)).toBe('Main');
    expect(labels.get(2)).toBe('Section 2');
    expect(labels.get(3)).toBe('Section 3');
    expect(labels.get(4)).toBe('Upstream B');
  });

  it('elides long names in the middle with one ellipsis character', () => {
    const labels = labelsOf(['Downstream sump 1A']);
    const label = labels.get(1)!;
    expect(graphemes(label).length).toBeLessThanOrEqual(16);
    expect(label.split('…')).toHaveLength(2);
    expect(label.startsWith('Downstr')).toBe(true);
    expect(label.endsWith('sump 1A')).toBe(true);
  });

  it('keeps the distinguishing suffix when names share a long prefix', () => {
    const labels = labelsOf(['Downstream sump 1A', 'Downstream sump 1B']);
    expectUniqueAndShort(labels);
    expect(labels.get(1)!.endsWith('1A')).toBe(true);
    expect(labels.get(2)!.endsWith('1B')).toBe(true);
  });

  it('keeps the distinguishing prefix when names share a long suffix', () => {
    const labels = labelsOf([
      'North the very long passage to the terminal sump',
      'South the very long passage to the terminal sump',
    ]);
    expectUniqueAndShort(labels);
    expect(labels.get(1)!.startsWith('North')).toBe(true);
    expect(labels.get(2)!.startsWith('South')).toBe(true);
  });

  it('separates names that differ only in the middle', () => {
    const labels = labelsOf([
      'Common long beginning X common long ending',
      'Common long beginning Y common long ending',
      'Common long beginning Z common long ending',
    ]);
    expectUniqueAndShort(labels);
    expect(labels.get(1)).toContain('X');
    expect(labels.get(2)).toContain('Y');
    expect(labels.get(3)).toContain('Z');
  });

  it('disambiguates identical names with the section id', () => {
    const labels = buildSectionLabels(
      [
        { id: 12, name: 'Main' },
        { id: 13, name: 'Main' },
        { id: 40, name: 'Side' },
      ],
      def,
    );
    expect(labels.get(12)).toBe('Main #12');
    expect(labels.get(13)).toBe('Main #13');
    expect(labels.get(40)).toBe('Side');
  });

  it('truncates long identical names to keep the id within 16', () => {
    const labels = buildSectionLabels(
      [
        { id: 21474, name: 'A very long duplicated name' },
        { id: 21475, name: 'A very long duplicated name' },
      ],
      def,
    );
    expectUniqueAndShort(labels);
    expect(labels.get(21474)!.endsWith(' #21474')).toBe(true);
    expect(labels.get(21475)!.endsWith(' #21475')).toBe(true);
  });

  it('resolves a literal name that equals a derived label', () => {
    const labels = buildSectionLabels(
      [
        { id: 12, name: 'Main' },
        { id: 13, name: 'Main' },
        { id: 40, name: 'Main #12' },
      ],
      def,
    );
    expectUniqueAndShort(labels);
    expect(labels.get(12)).toBe('Main #12');
    expect(labels.get(40)).toBe('Main #12 #40');
  });

  it('never splits a grapheme cluster', () => {
    const accented = 'Cueva de la Peña grande del norte';
    const emoji = 'Passage \u{1F468}‍\u{1F469}‍\u{1F467} family room extension';
    const labels = labelsOf([accented, emoji]);
    expectUniqueAndShort(labels);
    for (const label of labels.values()) {
      // A lone combining tilde or a dangling joiner would mean a split cluster.
      expect(label).not.toMatch(/^̃|‍$|‍…/u);
    }
  });

  it('strips control and bidi characters and collapses whitespace', () => {
    expect(cleanSectionName('Main\tline\n 2')).toBe('Main line 2');
    expect(cleanSectionName('A‮evil\u0007')).toBe('Aevil');
    const labels = labelsOf(['\u0001\u0002', 'X‏y']);
    expect(labels.get(1)).toBe('Section 1');
    expect(labels.get(2)).toBe('Xy');
  });

  it('is deterministic', () => {
    const names = ['Downstream sump 1A', 'Downstream sump 1B', 'Main', 'Main', null, 'Upstream'];
    expect([...labelsOf(names)]).toEqual([...labelsOf(names)]);
  });

  it('stays unique across many similar long names', () => {
    const names = Array.from({ length: 60 }, (_, i) => `Underwater passage number ${i} towards the sump`);
    expectUniqueAndShort(labelsOf(names));
  });

  it('untangles chains of literal names that look like id tags', () => {
    expectUniqueAndShort(labelsOf(['A', 'A', 'A #1', 'A #1 #3', 'A #1 #3 #4']));
  });

  it('stays fast for thousands of names sharing a long prefix and suffix', () => {
    const names = Array.from({ length: 4000 }, (_, i) => `Shared long prefix of the passage ${i} shared long suffix`);
    const t0 = performance.now();
    expectUniqueAndShort(labelsOf(names));
    expect(performance.now() - t0).toBeLessThan(5000);
  });
});

describe('elideMiddle', () => {
  it('keeps a name that fits', () => {
    expect(elideMiddle('Downstream Sump Passage A', 30)).toBe('Downstream Sump Passage A');
  });

  it('elides the middle so names differing at the end stay apart', () => {
    const a = elideMiddle('Downstream Sump Passage Number One Alpha', 20);
    const b = elideMiddle('Downstream Sump Passage Number One Bravo', 20);
    expect(a).not.toBe(b);
    expect(graphemes(a).length).toBeLessThanOrEqual(20);
    expect(a.startsWith('Downstream')).toBe(true);
    expect(a.endsWith('Alpha')).toBe(true);
    expect(a).toContain('…');
  });

  it('never cuts a grapheme cluster', () => {
    const out = elideMiddle('Cueva 👩‍👩‍👧 Sistema 👩‍👩‍👧 Final', 10);
    expect(graphemes(out).length).toBeLessThanOrEqual(10);
    expect(out).not.toMatch(/\uFFFD/);
  });
});
