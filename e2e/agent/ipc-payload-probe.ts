/**
 * Where does a Tauri IPC argument stop arriving intact?
 *
 * A one-off experiment, kept because its answer is a number this repo's import
 * path depends on and nothing else measures it. `import-dialog-fuzz` found
 * `import_sections` failing with serde's `premature end of input` - an
 * ARGUMENT-DESERIALIZATION failure, i.e. the JSON body the webview sent was not
 * the JSON body Rust received - and the question is what makes that happen.
 *
 * The trick is that the payload does NOT have to be valid. A well-formed body
 * that is missing a field fails with `missing field ...`; a body that was
 * TRUNCATED fails with `premature end of input`. So the error message alone
 * distinguishes "arrived and was rejected" from "did not arrive", and a
 * deliberately-invalid config is enough to binary-search the boundary without
 * mutating a project.
 *
 *   pnpm tsx e2e/agent/ipc-payload-probe.ts [--char x] [--max 33554432]
 *
 * Prints the largest string argument that arrives intact and the smallest that
 * does not.
 */
import { AppSession } from './session';
import { evalInApp } from './lib/page';
import { shareableErrorText, valueShape, withheldAppText } from './lib/shareableFailure';

interface Probe {
  /** serde's message, or null when the call unexpectedly succeeded. */
  error: string | null;
  /** Did the body reach the handler at all? */
  arrived: boolean;
}

/**
 * Characters worth one call each, with the reason they are suspects.
 *
 * Two families. The CONTROL characters are what a fixed-width binary format
 * pads and separates records with, so they reach a comment by surviving a
 * parse rather than by being typed. The FORMAT characters are what this
 * repo's hostile-text corpus already carries (`HOSTILE_COMMENTS` in
 * `lib/surveyFileSpecs.ts`).
 *
 * The lone surrogates are the ones the corpus deliberately CANNOT carry: a
 * `.dmp` is written through `TextEncoder`, which substitutes U+FFFD, so a
 * generated file can never hold one. They are probed here anyway, because
 * this boundary is reached by typed input too and "the writer cannot emit it"
 * is not "the app never sees it".
 */
const SUSPECTS: readonly { label: string; text: string }[] = [
  { label: 'U+0000 NUL', text: '\u0000' },
  { label: 'U+0001 SOH', text: '\u0001' },
  { label: 'U+0008 BS', text: '\u0008' },
  { label: 'U+0009 TAB', text: '\u0009' },
  { label: 'U+000A LF', text: '\u000a' },
  { label: 'U+000D CR', text: '\u000d' },
  { label: 'U+001B ESC', text: '\u001b' },
  { label: 'U+007F DEL', text: '\u007f' },
  { label: 'U+200B zero width space', text: '\u200b' },
  { label: 'U+202E right-to-left override', text: '\u202e' },
  { label: 'U+FFFD replacement', text: '\ufffd' },
  { label: 'lone high surrogate U+D83D', text: '\ud83d' },
  { label: 'lone low surrogate U+DE00', text: '\ude00' },
  { label: 'well-formed pair U+1F600', text: '\ud83d\ude00' },
  { label: 'quote and backslash', text: '"\\' },
];

/** The flags this probe takes. An error names one of them, and anything else by its shape (#576 W51). */
const FLAGS: ReadonlySet<string> = new Set(['--classes', '--shapes', '--char', '--max']);
const shownFlag = (flag: string | undefined): string => (flag !== undefined && FLAGS.has(flag) ? flag : valueShape(flag));

const DEFAULT_CHAR = 'x';
/**
 * The filler as printed (#576 W51): `--char` is the user's text, so it prints
 * as the default, as a suspect's label, or by its shape.
 */
const shownChar = (text: string): string =>
  text === DEFAULT_CHAR
    ? JSON.stringify(DEFAULT_CHAR)
    : (SUSPECTS.find((suspect) => suspect.text === text)?.label ?? valueShape(text));

/** serde's message as printed (#576 W51): it is the app's error text, so its length. */
const shownError = (error: string | null): string => (error === null ? '(no error)' : withheldAppText(error));

function parseArgs(argv: string[]): { char: string; max: number; classes: boolean; shapes: boolean } {
  let char = DEFAULT_CHAR;
  let max = 32 * 1024 * 1024;
  let classes = false;
  let shapes = false;
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    if (flag === '--classes') {
      classes = true;
      continue;
    }
    if (flag === '--shapes') {
      shapes = true;
      continue;
    }
    const value = argv[i + 1];
    if (value === undefined) throw new Error(`flag ${shownFlag(flag)} needs a value`);
    i += 1;
    if (flag === '--char') char = value;
    else if (flag === '--max') max = Number(value);
    else throw new Error(`unknown flag, ${valueShape(flag)}`);
  }
  if (!Number.isInteger(max) || max < 1) throw new Error('--max needs a positive integer');
  return { char, max, classes, shapes };
}

async function main(): Promise<number> {
  const { char, max, classes, shapes } = parseArgs(process.argv.slice(2));
  const session = new AppSession();
  const started = await session.start({ build: 'auto' });
  process.stdout.write(`session ${started.state} (${started.buildMode} build ${started.buildAction})\n`);

  /**
   * One call, with a `comment` of `length` copies of `char`.
   *
   * `import_sections` is invoked with a config that is missing every required
   * field on purpose - it can never reach the app's state, so the probe is
   * read-only with respect to the project.
   */
  const probe = async (length: number): Promise<Probe> =>
    session.exclusive(() =>
      evalInApp<Probe>(
        session.browser,
        `
        const core = window.__TAURI__ && window.__TAURI__.core;
        if (!core || typeof core.invoke !== 'function') throw new Error('invoke unavailable');
        const filler = args[0].repeat(args[1]);
        try {
          await core.invoke('import_sections', { sections: [{ comment: filler }] });
          return { error: null, arrived: true };
        } catch (err) {
          const message = String(err);
          return { error: message, arrived: message.indexOf('premature end of input') < 0 };
        }
      `,
        [char, length],
      ),
    );

  let failed = false;
  try {
    if (shapes) {
      // Value KINDS rather than characters. `JSON.stringify` throwing inside
      // the serializer would send an empty body, which is exactly what serde
      // reports as `premature end of input`.
      const cases: { label: string; body: string }[] = [
        { label: 'plain object', body: '{ sections: [{ ok: 1 }] }' },
        { label: 'BigInt field', body: '{ sections: [{ section_index: BigInt(1) }] }' },
        { label: 'BigInt nested in shots', body: '{ sections: [{ shots: [{ id: BigInt(7) }] }] }' },
        { label: 'NaN field', body: '{ sections: [{ length: NaN }] }' },
        { label: 'Infinity field', body: '{ sections: [{ length: Infinity }] }' },
        { label: 'undefined in array', body: '{ sections: [undefined] }' },
        { label: 'function field', body: '{ sections: [{ f: function () {} }] }' },
        { label: 'Map field', body: '{ sections: [{ m: new Map([[1, 2]]) }] }' },
        { label: 'circular object', body: '(() => { const a = { sections: [{}] }; a.sections[0].self = a; return a; })()' },
        // `ImportSectionConfig.date` is `Option<NaiveDate>` (models/section.rs:253).
        // chrono's TOO_SHORT parse error prints as "premature end of input",
        // so an EMPTY STRING where the wizard means "no date" reads to serde as
        // a malformed argument body rather than as an absent value.
        { label: 'date empty string', body: '{ sections: [{ date: "" }] }' },
        { label: 'date null', body: '{ sections: [{ date: null }] }' },
        { label: 'date valid', body: '{ sections: [{ date: "2026-01-01" }] }' },
        { label: 'date junk', body: '{ sections: [{ date: "not-a-date" }] }' },
        { label: 'date impossible', body: '{ sections: [{ date: "2024-02-30" }] }' },
      ];
      process.stdout.write('\nvalue shape sweep:\n');
      for (const probeCase of cases) {
        const result = await session.exclusive(() =>
          evalInApp<Probe>(
            session.browser,
            `
            const core = window.__TAURI__.core;
            try {
              await core.invoke('import_sections', ${probeCase.body});
              return { error: null, arrived: true };
            } catch (err) {
              const message = String(err && err.message ? err.message : err);
              return { error: message, arrived: message.indexOf('premature end of input') < 0 };
            }
          `,
          ),
        );
        process.stdout.write(
          `  ${result.arrived ? 'arrived  ' : 'TRUNCATED'} ${probeCase.label.padEnd(24)} ${shownError(result.error)}\n`,
        );
      }
      return 0;
    }

    if (classes) {
      process.stdout.write('\ncharacter class sweep (one call each, 8 copies):\n');
      const broken: string[] = [];
      for (const suspect of SUSPECTS) {
        const result = await session.exclusive(() =>
          evalInApp<Probe>(
            session.browser,
            `
            const core = window.__TAURI__.core;
            const filler = args[0].repeat(8);
            try {
              await core.invoke('import_sections', { sections: [{ comment: filler }] });
              return { error: null, arrived: true };
            } catch (err) {
              const message = String(err);
              return { error: message, arrived: message.indexOf('premature end of input') < 0 };
            }
          `,
            [suspect.text],
          ),
        );
        process.stdout.write(
          `  ${result.arrived ? 'arrived  ' : 'TRUNCATED'} ${suspect.label.padEnd(32)} ${shownError(result.error)}\n`,
        );
        if (!result.arrived) broken.push(suspect.label);
      }
      process.stdout.write(
        broken.length > 0
          ? `\nTRUNCATES THE IPC BODY: ${broken.join(', ')}\n`
          : '\nNo probed character truncated the body.\n',
      );
      return 0;
    }

    const baseline = await probe(1);
    process.stdout.write(`  baseline (1 char): arrived=${baseline.arrived} error=${shownError(baseline.error)}\n`);
    if (!baseline.arrived) throw new Error('even a 1-character argument did not arrive; the probe is wrong');

    const ceiling = await probe(max);
    process.stdout.write(`  ceiling (${max} chars): arrived=${ceiling.arrived} error=${shownError(ceiling.error)}\n`);
    if (ceiling.arrived) {
      process.stdout.write(`\nNO TRUNCATION up to ${max} ${shownChar(char)} characters.\n`);
      return 0;
    }

    // Binary search the boundary between "arrived" and "truncated".
    let good = 1;
    let bad = max;
    while (bad - good > 1) {
      const middle = Math.floor((good + bad) / 2);
      const result = await probe(middle);
      process.stdout.write(`  ${String(middle).padStart(9)} -> ${result.arrived ? 'arrived' : 'TRUNCATED'}\n`);
      if (result.arrived) good = middle;
      else bad = middle;
    }
    process.stdout.write(
      `\nBOUNDARY for ${shownChar(char)}: ${good} arrives, ${bad} does not ` +
        `(${(good / 1024).toFixed(1)} KiB / ${(bad / 1024).toFixed(1)} KiB).\n`,
    );
  } catch (err) {
    failed = true;
    // The shareable crash form (#576 W33-NF6): a crash in session setup can quote
    // the app (an IPC rejection, a page script's error).
    process.stdout.write(`probe failed: ${shareableErrorText(err)}\n`);
  } finally {
    await session.stop('ipc-payload-probe');
  }
  return failed ? 1 : 0;
}

main().then(
  (code) => process.exit(code),
  (err: unknown) => {
    // The shareable crash form (#576 W33-NF6): a crash in session setup can quote
    // the app (an IPC rejection, a page script's error).
    process.stderr.write(`fatal: ${shareableErrorText(err)}\n`);
    process.exit(1);
  },
);
