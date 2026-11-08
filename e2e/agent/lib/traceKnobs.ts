/**
 * The knobs a fuzz trace records and a replay restores, each with the shape of
 * the value it takes (#576 W58 Q9).
 *
 * A trace is attached to issues and shrunk from wherever someone got it, and
 * `agent:scenario --trace` and `fuzz-shrink` print every knob it carries. So a
 * value is recorded, restored and printed only when it has its knob's shape - a
 * switch, a number, a stream name, a format list, a fixture name - and never as
 * whatever text the shell held: a value of any other shape is not recorded
 * (the header's `envWithheld` keeps its length, so a replay says the recording
 * had it set), a trace that carries one anyway has it refused on restore, and
 * it prints as its length. Every value shape here means the same to its reader
 * as unset does when it fails: a reader of a switch tests one word, and a
 * reader of a number falls back to its default on anything `Number` cannot
 * read - except the empty string, which `Number` reads as 0, so it is recorded.
 *
 * The registry is closed. `traceKnobs.test.ts` checks that every
 * `STALAGMIGHT_FUZZ_*` / `STALAGMIGHT_PERSIST_FUZZ_*` name the harness reads is
 * here or is one of the path knobs a trace never carries (`fuzzTrace.ts`
 * `NEVER_RESTORED`), so a new knob is registered with its shape or it fails.
 */
import { shareableFixtureName } from './privateFixtures';
import { valueShape } from './shareableFailure';
import { SURVEY_FORMAT_FAMILIES } from './surveyFileSpecs';

export interface TraceKnob {
  /** The values the knob's reader takes, as text. The empty string is always accepted. */
  readonly shape: RegExp;
  readonly what: string;
}

/** A word list of `words`, comma-separated, spaces allowed around the commas. */
function wordList(words: readonly string[], flags = ''): RegExp {
  const word = `(?:${words.join('|')})`;
  return new RegExp(`^\\s*${word}(?:\\s*,\\s*${word})*\\s*$`, flags);
}

const SWITCH: TraceKnob = { shape: /^(?:0|1|true|false|yes|no|on|off)$/i, what: 'a switch (0/1, true/false, yes/no, on/off)' };
const NUMBER: TraceKnob = { shape: /^[-+]?(?:\d{1,12}\.?\d{0,12}|\.\d{1,12})(?:e[-+]?\d{1,3})?$/i, what: 'a number' };
/** `STALAGMIGHT_FUZZ_EXPORT_FORMATS`: the export fuzzer's formats (`lib/exportOracles.ts` `ExportFormat`). */
const EXPORT_FORMATS: TraceKnob = {
  shape: wordList(['csv', 'json', 'tml', 'tmlu', 'png', 'svg', 'pdf', 'preview', 'diveProfile']),
  what: "a list of the export fuzzer's formats",
};
/** `STALAGMIGHT_FUZZ_IMPORT_FORMAT`: format families (`lib/surveyFileSpecs.ts`), read case-insensitively. */
const IMPORT_FAMILIES: TraceKnob = { shape: wordList(SURVEY_FORMAT_FAMILIES, 'i'), what: 'a list of survey format families' };

/** Every knob a trace may carry, and the shape of its value. */
export const TRACE_KNOBS: Readonly<Record<string, TraceKnob>> = {
  STALAGMIGHT_FUZZ_FIXTURE: { shape: /^[^/\\\0-\x1f]{1,200}\.stalx$/, what: 'a fixture name, a bare .stalx' },
  STALAGMIGHT_FUZZ_STREAM: { shape: /^v\d{1,2}$/, what: 'a generator stream (v1, v2, v3)' },
  STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS: SWITCH,
  STALAGMIGHT_FUZZ_DUMP_HISTORY: SWITCH,
  STALAGMIGHT_FUZZ_REPAIR: SWITCH,
  STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET: SWITCH,
  STALAGMIGHT_FUZZ_EXPORT_ROUNDTRIP: SWITCH,
  STALAGMIGHT_FUZZ_IMPORT_FORCE_STATIONS: SWITCH,
  STALAGMIGHT_FUZZ_EXPORT_FORMATS: EXPORT_FORMATS,
  STALAGMIGHT_FUZZ_IMPORT_FORMAT: IMPORT_FAMILIES,
  STALAGMIGHT_FUZZ_EXPORT_MAX_DPI: NUMBER,
  STALAGMIGHT_FUZZ_EXPORT_MOVES: NUMBER,
  STALAGMIGHT_FUZZ_EXPORT_SETTLE_MS: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_SETTLE_MS: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_FLOWS_PER_EPISODE: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_MAX_FLOW_STEPS: NUMBER,
  STALAGMIGHT_FUZZ_CORPUS_DRAW_P: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_AUX_INTERJECT_P: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_INTERJECT_P: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P: NUMBER,
  STALAGMIGHT_FUZZ_IMPORT_SUGGESTIVE_P: NUMBER,
  // The determinism mode (`fuzzKnobs.ts` fuzzDeterminism / fuzzDeterminismPlant):
  // what `fuzzDeterminism` accepts, and the plant's record number.
  STALAGMIGHT_FUZZ_DETERMINISM: { shape: /^(?:0|1|2sessions)$/, what: 'a determinism mode (0, 1, 2sessions)' },
  STALAGMIGHT_FUZZ_DETERMINISM_PLANT: NUMBER,
};

/** Is `value` one `key` takes: a registered knob, and its shape (or empty)? */
export function knobValueShaped(key: string, value: unknown): boolean {
  const knob = Object.prototype.hasOwnProperty.call(TRACE_KNOBS, key) ? TRACE_KNOBS[key] : undefined;
  return knob !== undefined && typeof value === 'string' && (value === '' || knob.shape.test(value));
}

/**
 * A knob's value as a runner prints it: the fixture by `shareableFixtureName`
 * (#576 W58 Q1), any other value of its knob's shape as itself, quoted when
 * empty, and anything else - an unregistered knob's, or one a forged trace
 * holds - as `valueShape` (its length).
 */
export function shareableKnobValue(key: string, value: unknown): string {
  if (typeof value !== 'string' || !knobValueShaped(key, value)) return valueShape(value);
  if (key === 'STALAGMIGHT_FUZZ_FIXTURE') return shareableFixtureName(value);
  return value === '' ? '""' : value;
}
