/**
 * Component test - the "which of these is the survey?" prompt (#258).
 *
 * The properties pinned here are the ones a user would be harmed by if they
 * broke. The choice made in this dialog decides which file BECOMES the project,
 * and the next push replaces the project file on SpeleoDB with it, so a wrong
 * answer is discovered one push too late:
 *
 *   - the file SpeleoDB already serves is the pre-selection, and confirming
 *     without touching anything sends THAT path (the answer that changes nothing);
 *   - picking a different row sends the row that was picked, not the default it
 *     replaced - a confirm keyed on the pre-selection would quietly overwrite the
 *     server copy with a file the user explicitly did not choose;
 *   - with nothing canonical there is no safe default, so the dialog refuses to
 *     confirm until a human names a file rather than guessing at one;
 *   - cancelling resolves NOTHING, so backing out of an irreversible choice is
 *     genuinely backing out;
 *   - the Modified column appears only when the timestamps actually differ. The
 *     dump is a `git archive`, which stamps every entry with the same commit
 *     time, so an always-on column is a row of identical dates presented as if it
 *     were evidence.
 */
import { describe, it, expect, vi, beforeEach, type Mock } from 'vitest';
import { act, render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

// Real copy, so the assertions below are about what the user actually reads.
import '../../i18n';
import SpeleoDumpPickerDialog, { shouldShowModified, visiblePath } from './SpeleoDumpPickerDialog';
import { useSpeleoStore, type DumpPickerKind } from '../../store/speleoStore';
import type { SpeleoDumpCandidate } from '../../types/ipc/SpeleoDumpCandidate';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

const candidate = (over: Partial<SpeleoDumpCandidate> = {}): SpeleoDumpCandidate => ({
  entry_path: 'survey.stalx',
  size_bytes: 4096,
  modified: '2026-05-01T10:00:00Z',
  ...over,
});

/** The ordinary dump: one entry SpeleoDB serves, plus a stray sibling. */
const CANONICAL_SECOND: SpeleoDumpCandidate[] = [
  candidate({ entry_path: 'backup/old-survey.stalx', size_bytes: 2048 }),
  candidate({ entry_path: 'main/survey.stalx', size_bytes: 8192 }),
];

/** The store's own signatures: the real resolve is async, so the stub must be too. */
let resolveMock: Mock<(entryPath: string) => Promise<void>>;
let closeMock: Mock<() => void>;

type PickerSeed = {
  dumpPickerOpen: boolean;
  dumpPickerCandidates: SpeleoDumpCandidate[];
  dumpPickerKind: DumpPickerKind;
  dumpPickerBusy: boolean;
};

function seed(over: Partial<PickerSeed> = {}): void {
  useSpeleoStore.setState({
    dumpPickerOpen: true,
    dumpPickerCandidates: CANONICAL_SECOND,
    dumpPickerKind: 'stalx',
    dumpPickerBusy: false,
    resolveDumpPicker: resolveMock,
    closeDumpPicker: closeMock,
    ...over,
  });
}

/**
 * The mount gate App.tsx puts this dialog behind. Mirrored rather than rendering
 * the component directly so the busy-dismissal assertions can read the DOM and
 * fail the way a user meets the bug - the prompt gone from in front of them.
 */
function Host() {
  const open = useSpeleoStore((s) => s.dumpPickerOpen);
  return open ? <SpeleoDumpPickerDialog /> : null;
}

const dialog = () => screen.queryByTestId('speleodb-dump-picker-dialog');
const confirm = () => screen.getByTestId('speleodb-dump-picker-confirm');
const option = (index: number) => screen.getByTestId(`speleodb-dump-picker-option-${index}`);

beforeEach(() => {
  invokeIpcMock.mockReset();
  resolveMock = vi.fn(async () => {});
  closeMock = vi.fn();
  seed();
});

describe('SpeleoDumpPickerDialog - choosing', () => {
  it('[#258 review] nominates nothing: no row checked, no row badged', () => {
    // Both used to be keyed on a `project.stalx` BASENAME, which asserts something no
    // dump records: the push writes `<TARGET_FOLDER>/project.stalx` for a folder the
    // archive never names. So with `backup/project.stalx` alongside `misc/current.stalx`
    // under a target of `misc/`, the badge landed on the BACKUP, sorted it first,
    // pre-selected it and took initial focus - a stray Enter accepted the one file the
    // next push would overwrite.
    render(<Host />);

    expect(option(0)).not.toBeChecked();
    expect(option(1)).not.toBeChecked();
    expect(screen.queryByText('current')).not.toBeInTheDocument();
  });

  it('[#258 review] confirming without choosing is impossible, not merely discouraged', async () => {
    render(<Host />);

    // Disabled rather than defaulted. The app has no candidate it can honestly
    // nominate, so it asks instead of guessing on the user's behalf.
    expect(confirm()).toBeDisabled();
    await userEvent.click(confirm());
    expect(resolveMock).not.toHaveBeenCalled();
  });

  it('[#258 review] a re-offer clears the answer, so Enter cannot carry it into a new tree', async () => {
    // The backend refuses a reply that answers a different revision and re-asks with
    // the new tree's metadata. This component stays mounted across that, and a path
    // that survives the push - a backup routinely does - left the old pick selected,
    // Confirm re-enabled and still focused. A second Enter then accepted bytes from a
    // tree the user never looked at, which is the deliberate re-choice the refusal was
    // demanding.
    render(<Host />);
    await userEvent.click(option(0));
    expect(confirm()).toBeEnabled();

    // The tree moved: same path still present, different metadata.
    act(() => {
      seed({
        dumpPickerCandidates: [
          candidate({ entry_path: 'backup/old-survey.stalx', size_bytes: 999 }),
          candidate({ entry_path: 'main/survey.stalx', size_bytes: 111 }),
        ],
      });
    });

    expect(option(0)).not.toBeChecked();
    expect(option(1)).not.toBeChecked();
    expect(confirm()).toBeDisabled();
  });

  it('[#258 review] renders bidi and invisible controls visibly, but submits the raw path', async () => {
    // Entry names come from the archive, which a collaborator writes. A bidi override
    // can make a decoy row READ as the trusted path while the radio's value stays the
    // attacker's original string - the user picks what they see and the app opens what
    // was sent. Escaped on the way to the screen, byte-identical on the way back, since
    // the value has to match the archive's own membership test.
    const hostile = 'backup/\u202Excod.stalx';
    seed({
      dumpPickerCandidates: [
        candidate({ entry_path: hostile }),
        candidate({ entry_path: 'main/survey.stalx' }),
      ],
    });
    render(<Host />);

    expect(screen.getByText(/<U\+202E>/)).toBeInTheDocument();
    expect(screen.queryByText(hostile)).not.toBeInTheDocument();

    await userEvent.click(option(0));
    await userEvent.click(confirm());
    expect(resolveMock).toHaveBeenCalledWith(hostile);
  });

  it('[#258 review] escapes by Unicode property, not by an enumerated range', async () => {
    // The first version of this listed the ranges it could think of and missed a soft
    // hyphen, a combining grapheme joiner, a Mongolian vowel separator and every
    // variation selector - each of which still lets two byte-distinct paths render
    // identically, which is the whole attack. An allowlist of invisibles is a list you
    // are always one codepoint behind on.
    const sneaky = 'a\u00ADb\u034Fc\u180Ed\uFE00e.stalx';
    seed({ dumpPickerCandidates: [candidate({ entry_path: sneaky })] });
    render(<Host />);

    for (const code of ['00AD', '034F', '180E', 'FE00']) {
      expect(screen.getByText(new RegExp(`<U\\+${code}>`))).toBeInTheDocument();
    }
    // ...while an ordinary space stays an ordinary space: it is legal in a path and
    // visible between other characters.
    expect(visiblePath('misc/my survey.stalx')).toBe('misc/my survey.stalx');
  });

  it('picking a row confirms THAT file', async () => {
    render(<Host />);
    await userEvent.click(option(0));
    await userEvent.click(confirm());

    expect(resolveMock).toHaveBeenCalledTimes(1);
    expect(resolveMock).toHaveBeenCalledWith('backup/old-survey.stalx');
  });

  it('refuses to confirm when no candidate is canonical until a file is named', async () => {
    seed({
      dumpPickerCandidates: [
        candidate({ entry_path: 'a/survey.stalx' }),
        candidate({ entry_path: 'b/survey.stalx' }),
      ],
    });
    render(<Host />);

    // No safe default exists, so there is nothing to fall back on - guessing one
    // would pick the file that overwrites the server copy on the next push.
    expect(confirm()).toBeDisabled();
    expect(option(0)).not.toBeChecked();
    expect(option(1)).not.toBeChecked();

    await userEvent.click(option(1));
    expect(confirm()).toBeEnabled();
    await userEvent.click(confirm());
    expect(resolveMock).toHaveBeenCalledWith('b/survey.stalx');
  });

  it('cancelling resolves nothing and closes only this overlay', async () => {
    render(<Host />);
    await userEvent.click(screen.getByTestId('speleodb-dump-picker-cancel'));

    // The negative half is the point: backing out of an irreversible choice must
    // not quietly commit the pre-selected file on the way out.
    expect(resolveMock).not.toHaveBeenCalled();
    expect(closeMock).toHaveBeenCalledTimes(1);
  });

  it('renders nothing when there are no candidates to choose between', () => {
    seed({ dumpPickerCandidates: [] });
    render(<Host />);

    // An empty list with a dead confirm button is a dialog asking a question that
    // has no answers.
    expect(dialog()).not.toBeInTheDocument();
  });

  // Adopting converts an Ariane project into a Stalagmight one, so a prompt that
  // promised "open" for both would understate what half of them do.
  const kinds: [DumpPickerKind, string, string][] = [
    ['stalx', 'Open', 'Choose which file to open'],
    ['ariane', 'Adopt', 'Choose which file to adopt'],
  ];

  it.each(kinds)('names the verb it is about to perform for a %s dump', (kind, action, title) => {
    seed({ dumpPickerKind: kind });
    render(<Host />);

    expect(confirm()).toHaveTextContent(action);
    expect(screen.getByText(title)).toBeInTheDocument();
  });

  it('states the consequence above the list, not after it', () => {
    render(<Host />);

    const consequence = screen.getByTestId('speleodb-dump-picker-consequence');
    // What the choice DOES has to be read before the choice is made; below the
    // rows it is an explanation of a click that already happened.
    expect(consequence.textContent).toContain('replaces the project file on SpeleoDB');
    expect(
      consequence.compareDocumentPosition(option(0)) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it('shows a human size per row', () => {
    render(<Host />);

    expect(option(0).closest('label')?.textContent).toContain('2.0 KB');
    expect(option(1).closest('label')?.textContent).toContain('8.0 KB');
  });
});

describe('SpeleoDumpPickerDialog - the Modified column', () => {
  const withModified = (values: (string | null)[]): SpeleoDumpCandidate[] =>
    values.map((modified, index) =>
      candidate({ entry_path: `f${index}.stalx`, modified }),
    );

  it('is hidden when every entry carries the same timestamp', () => {
    seed({ dumpPickerCandidates: withModified(['2026-05-01T10:00:00Z', '2026-05-01T10:00:00Z']) });
    render(<Host />);

    // A `git archive` stamps every entry with the commit time, so this is the
    // ORDINARY case - and a column of identical dates reads as a broken clock,
    // or worse, as a reason to pick one row over the other.
    expect(screen.queryByText('Modified')).not.toBeInTheDocument();
  });

  it('is shown when the timestamps actually differ', () => {
    seed({ dumpPickerCandidates: withModified(['2026-05-01T10:00:00Z', '2026-06-02T11:00:00Z']) });
    render(<Host />);

    expect(screen.getByText('Modified')).toBeInTheDocument();
  });

  it('shouldShowModified needs two entries that genuinely disagree', () => {
    const at = (modified: string | null) => candidate({ modified });

    expect(shouldShowModified([])).toBe(false);
    expect(shouldShowModified([at('2026-05-01T10:00:00Z')])).toBe(false);
    expect(shouldShowModified([at('2026-05-01T10:00:00Z'), at('2026-05-01T10:00:00Z')])).toBe(
      false,
    );
    expect(shouldShowModified([at('2026-05-01T10:00:00Z'), at('2026-06-02T11:00:00Z')])).toBe(true);
    // An entry that declares no timestamp is an absence, not a disagreement: a
    // column of one date and one blank says nothing about which file to take.
    expect(shouldShowModified([at(null), at(null)])).toBe(false);
    expect(shouldShowModified([at('2026-05-01T10:00:00Z'), at(null)])).toBe(false);
  });
});

describe('SpeleoDumpPickerDialog - while the resolve is in flight', () => {
  it('disables confirm, cancel and the rows', () => {
    seed({ dumpPickerBusy: true });
    render(<Host />);

    // A second confirm would re-invoke the backend for a different file while the
    // first open is still running.
    expect(confirm()).toBeDisabled();
    expect(screen.getByTestId('speleodb-dump-picker-cancel')).toBeDisabled();
    expect(option(0)).toBeDisabled();
    expect(option(1)).toBeDisabled();
  });

  it('DO NOT REGRESS: Escape cannot take the prompt away mid-resolve', () => {
    // Inherited from #270, where the dismissal routes were the hole: Escape, the
    // title-bar X and the backdrop all reach `onClose` and NONE of them is gated
    // on a button's `disabled`. Unmounting here leaves an open still running with
    // nothing on screen that says so, and no surface for its failure to land on.
    seed({ dumpPickerBusy: true });
    render(<Host />);

    fireEvent.keyDown(screen.getByTestId('speleodb-dump-picker-dialog'), { key: 'Escape' });

    expect(closeMock).not.toHaveBeenCalled();
    expect(dialog()).toBeInTheDocument();
  });

  it('DO NOT REGRESS: the backdrop cannot take the prompt away mid-resolve', () => {
    seed({ dumpPickerBusy: true });
    render(<Host />);

    const backdrop = document.querySelector('#dialog-root .bg-overlay');
    // Thrown rather than asserted away: if the wrapper renames this element the
    // dismissal silently stops happening, and this test would "pass" by never
    // exercising the route it is named for.
    if (!backdrop) throw new Error('no dialog backdrop to click');
    fireEvent.mouseDown(backdrop);
    fireEvent.click(backdrop);

    expect(closeMock).not.toHaveBeenCalled();
    expect(dialog()).toBeInTheDocument();
  });
});
