/**
 * The DMP import wizard's state machine (#576 D4, second slice).
 *
 * The wizard used to keep its step as a phase LABEL beside a dozen independent
 * flags and fields - the suggestion on screen, its alternatives and merge
 * offset, whether suggestions were being computed, the segment queue, its
 * anchor and preview anchor, the placement mode, the From Existing pick, the
 * hidden auto-start window, two confirmation flags and the count they quote -
 * and moved between steps four different ways, effects watching what other
 * effects wrote among them. Any combination of those could be written, and
 * several meaningless ones were reachable.
 *
 * Here each phase carries exactly the data that phase has, so "a suggestion
 * while placing", "a segment anchor with no segment", "both confirmations open"
 * or "hidden outside manual placing" cannot be represented. Every transition is
 * an action of `wizardReducer`, a pure function; the dialog dispatches them
 * from the calls that decide them (an import resolving, a click), not from
 * effects that react to a flag.
 *
 * What is NOT here, on purpose: the run (`useDmpImportRun`: its imports, the
 * busy flag that blocks the exits, the typed failures, the withdrawal) and the
 * run's plan data (connections, skipped sections, edits, placements, pools),
 * which async steps read synchronously through refs.
 */
import { useCallback, useRef, useState } from 'react';
import type { CrossSectionSuggestion } from '../../../types/commands';
import type { SegmentPlacementMode } from '../SegmentPlacementModes';
import { reanchorAfterRunDelete, reanchorStation } from '../../../utils/stationReanchor';
import type { SavedManualPlacingState } from './wizardTypes';

/** A station of a DMP section, numbered over its VISIBLE rows. */
export interface SegmentAnchor {
  sectionIndex: number;
  stationIndex: number;
}

/**
 * The file's section `anchor` names, or undefined when the file has none at that
 * index. Map placement draws the segment from this section, so without it no
 * placement payload can be built - the one reason a segment cannot go on the map,
 * which the manual-placing screen then states (#576 NF-W9FE-3-D1).
 */
export function anchorSectionIn<S>(sections: readonly S[], anchor: SegmentAnchor): S | undefined {
  return sections[anchor.sectionIndex];
}

/** A queue of segments (each a list of DMP section indices) that is never empty. */
export type SegmentQueue = [number[], ...number[][]];

/** Starting: the anchor import (the run's busy flag) and the suggestion call. */
export interface InitPhase {
  kind: 'init';
  /** `suggest_all_import_placements` is on the wire. */
  calculating: boolean;
}

/** One suggestion on screen, waiting for Confirm or Skip. */
export interface SuggestingPhase {
  kind: 'suggesting';
  suggestion: CrossSectionSuggestion;
  /** Other suggestions for the same import section, best first, at most four. */
  alternatives: CrossSectionSuggestion[];
  /** The merge-station offset picked (the backend's preferred one to start with). */
  mergeOffset: number;
}

/** What was left unreachable, offered one segment at a time. */
export interface ManualPlacingPhase {
  kind: 'manual_placing';
  /** The segment on screen first. */
  queue: SegmentQueue;
  /** The station the segment is attached by. */
  anchor: SegmentAnchor;
  /** The station the preview highlights: follows a dropdown hover, else the anchor. */
  previewAnchor: SegmentAnchor;
  mode: SegmentPlacementMode;
  /** The existing station picked in From Existing mode. */
  fromExisting: { sectionId: number | null; stationIndex: number };
  /**
   * The auto-start handoff to map placement has not been ATTEMPTED yet. The
   * dialog is hidden for exactly this window, so every attempt ends it,
   * whatever the attempt decides (#476).
   */
  handoffPending: boolean;
}

/** The late import: whatever manual placement added. */
export interface PlacingPhase {
  kind: 'placing';
}

/** Nothing left to suggest or place. */
export interface DonePhase {
  kind: 'done';
}

export type WizardPhase = InitPhase | SuggestingPhase | ManualPlacingPhase | PlacingPhase | DonePhase;

/** The Cancel or Back confirmation, with the section count its copy quotes. The
 *  count is taken when it opens: the live one lives in the run's refs. */
export interface ExitConfirmation {
  kind: 'cancel' | 'back';
  count: number;
}

export interface WizardState {
  phase: WizardPhase;
  exit: ExitConfirmation | null;
}

/** What decides where a segment opens: its anchor starts on a section the user
 *  skipped in the suggestion phase, and a segment holding a skipped suggestion
 *  opens on reusing it. */
export interface SegmentChoices {
  skipped: ReadonlySet<number>;
  withSuggestion: ReadonlySet<number>;
}

export type WizardAction =
  | { type: 'calculating'; calculating: boolean }
  | { type: 'suggest'; suggestion: CrossSectionSuggestion; alternatives: CrossSectionSuggestion[]; mergeOffset: number }
  | { type: 'chooseAlternative'; alternative: CrossSectionSuggestion; mergeOffset: number }
  | { type: 'setMergeOffset'; mergeOffset: number }
  /** Enter manual placing with the handoff pending. `mode: 'manual'` opens the
   *  first segment on map placement whatever it holds (the early import's
   *  way in); 'bySegment' lets a skipped suggestion choose. */
  | { type: 'placeManually'; queue: SegmentQueue; choices: SegmentChoices; mode: 'manual' | 'bySegment' }
  /** The segment on screen is placed (or discarded): the next one, or the late import. */
  | { type: 'segmentDone'; choices: SegmentChoices }
  /** The segment on screen goes to the back of the queue. */
  | { type: 'segmentSkipped'; choices: SegmentChoices }
  | { type: 'commitAnchor'; anchor: SegmentAnchor }
  /** A section dropdown hovers an option (null: it closed). */
  | { type: 'hoverAnchorSection'; sectionIndex: number | null }
  /** A station dropdown hovers an option (null: it closed). */
  | { type: 'hoverAnchorStation'; stationIndex: number | null }
  /** Rows of `sectionIndex` changed count: renumber the anchor in the same step. */
  | { type: 'rowsChanged'; sectionIndex: number; at: number; delta: number; survivorsAfter: number }
  | { type: 'setMode'; mode: SegmentPlacementMode }
  | { type: 'setFromExistingSection'; sectionId: number | null }
  | { type: 'setFromExistingStation'; stationIndex: number }
  /** The auto-start handoff was attempted (or will not be): the dialog shows. */
  | { type: 'handoffSettled' }
  | { type: 'done' }
  | { type: 'openExit'; exit: ExitConfirmation }
  | { type: 'closeExit' };

const NO_FROM_EXISTING = { sectionId: null, stationIndex: 0 } as const;

/** The anchor a segment opens on: station 0 of a section the user skipped, else of its first. */
export function segmentEntryAnchor(segment: number[], choices: SegmentChoices): SegmentAnchor {
  return { sectionIndex: segment.find((idx) => choices.skipped.has(idx)) ?? segment[0]!, stationIndex: 0 };
}

function segmentEntryMode(segment: number[], choices: SegmentChoices): SegmentPlacementMode {
  return segment.some((idx) => choices.withSuggestion.has(idx)) ? 'suggestion' : 'manual';
}

/** `phase` showing `queue`'s head as it opens: its entry anchor and mode, no From Existing pick. */
function onSegment(phase: ManualPlacingPhase, queue: SegmentQueue, choices: SegmentChoices): ManualPlacingPhase {
  const anchor = segmentEntryAnchor(queue[0], choices);
  return {
    ...phase,
    queue,
    anchor,
    previewAnchor: anchor,
    mode: segmentEntryMode(queue[0], choices),
    fromExisting: NO_FROM_EXISTING,
  };
}

/** A non-empty queue as a `SegmentQueue`, or null. */
export function segmentQueue(segments: number[][]): SegmentQueue | null {
  const [head, ...rest] = segments;
  return head ? [head, ...rest] : null;
}

/** Apply `update` when the wizard is manual placing; any other phase has no segment to change. */
function inManualPlacing(
  state: WizardState,
  update: (phase: ManualPlacingPhase) => WizardPhase,
): WizardState {
  if (state.phase.kind !== 'manual_placing') return state;
  const phase = update(state.phase);
  return phase === state.phase ? state : { ...state, phase };
}

export function wizardReducer(state: WizardState, action: WizardAction): WizardState {
  const { phase } = state;
  switch (action.type) {
    case 'calculating':
      return phase.kind === 'init' ? { ...state, phase: { kind: 'init', calculating: action.calculating } } : state;

    case 'suggest':
      // From init (the first suggestion) or suggesting (the next one).
      if (phase.kind !== 'init' && phase.kind !== 'suggesting') return state;
      return {
        ...state,
        phase: {
          kind: 'suggesting',
          suggestion: action.suggestion,
          alternatives: action.alternatives,
          mergeOffset: action.mergeOffset,
        },
      };

    case 'chooseAlternative': {
      if (phase.kind !== 'suggesting') return state;
      // The chosen one leaves the list, the one it replaces joins it.
      const alternatives = phase.alternatives.filter((a) => a !== action.alternative);
      alternatives.push(phase.suggestion);
      alternatives.sort((a, b) => b.score - a.score);
      return {
        ...state,
        phase: {
          kind: 'suggesting',
          suggestion: action.alternative,
          alternatives: alternatives.slice(0, 4),
          mergeOffset: action.mergeOffset,
        },
      };
    }

    case 'setMergeOffset':
      return phase.kind === 'suggesting' ? { ...state, phase: { ...phase, mergeOffset: action.mergeOffset } } : state;

    case 'placeManually': {
      const entry = segmentEntryAnchor(action.queue[0], action.choices);
      return {
        ...state,
        phase: {
          kind: 'manual_placing',
          queue: action.queue,
          anchor: entry,
          previewAnchor: entry,
          mode: action.mode === 'manual' ? 'manual' : segmentEntryMode(action.queue[0], action.choices),
          fromExisting: NO_FROM_EXISTING,
          handoffPending: true,
        },
      };
    }

    case 'segmentDone':
      return inManualPlacing(state, (p) => {
        const rest = segmentQueue(p.queue.slice(1));
        return rest ? onSegment(p, rest, action.choices) : { kind: 'placing' };
      });

    case 'segmentSkipped':
      return inManualPlacing(state, (p) => {
        // The only segment left stays where it is.
        const [first, ...rest] = p.queue;
        const rotated = segmentQueue([...rest, first]);
        if (rest.length === 0 || !rotated) return p;
        return onSegment(p, rotated, action.choices);
      });

    case 'commitAnchor':
      return inManualPlacing(state, (p) => ({ ...p, anchor: action.anchor, previewAnchor: action.anchor }));

    case 'hoverAnchorSection':
      return inManualPlacing(state, (p) => ({
        ...p,
        previewAnchor: action.sectionIndex === null ? p.anchor : { sectionIndex: action.sectionIndex, stationIndex: 0 },
      }));

    case 'hoverAnchorStation':
      return inManualPlacing(state, (p) => ({
        ...p,
        previewAnchor:
          action.stationIndex === null ? p.anchor : { sectionIndex: p.anchor.sectionIndex, stationIndex: action.stationIndex },
      }));

    case 'rowsChanged':
      return inManualPlacing(state, (p) => {
        if (action.delta === 0 || p.anchor.sectionIndex !== action.sectionIndex) return p;
        const stationIndex =
          action.delta < 0
            ? reanchorAfterRunDelete(p.anchor.stationIndex, action.at, -action.delta, action.survivorsAfter)
            : reanchorStation(p.anchor.stationIndex, action.at, action.delta, action.survivorsAfter);
        if (stationIndex === p.anchor.stationIndex) return p;
        const anchor = { sectionIndex: p.anchor.sectionIndex, stationIndex };
        return { ...p, anchor, previewAnchor: anchor };
      });

    case 'setMode':
      return inManualPlacing(state, (p) => ({ ...p, mode: action.mode }));

    case 'setFromExistingSection':
      return inManualPlacing(state, (p) => ({ ...p, fromExisting: { ...p.fromExisting, sectionId: action.sectionId } }));

    case 'setFromExistingStation':
      return inManualPlacing(state, (p) => ({ ...p, fromExisting: { ...p.fromExisting, stationIndex: action.stationIndex } }));

    case 'handoffSettled':
      return inManualPlacing(state, (p) => (p.handoffPending ? { ...p, handoffPending: false } : p));

    case 'done':
      return { ...state, phase: { kind: 'done' } };

    case 'openExit':
      return { ...state, exit: action.exit };

    case 'closeExit':
      return state.exit ? { ...state, exit: null } : state;
  }
}

/**
 * Where an instance starts: `init`, or - reopened after map placement - the
 * segment queue it was handed (the late import straight away if that queue is
 * empty). A reopened instance starts on its segment's map placement when
 * `autoStart` says so, hidden until it has been attempted.
 */
export function initialWizardState(
  saved: Pick<SavedManualPlacingState, 'segmentQueue' | 'segmentAnchor'> | undefined,
  autoStart: boolean,
): WizardState {
  if (!saved) return { phase: { kind: 'init', calculating: false }, exit: null };
  const queue = segmentQueue(saved.segmentQueue);
  if (!queue) return { phase: { kind: 'placing' }, exit: null };
  return {
    phase: {
      kind: 'manual_placing',
      queue,
      anchor: saved.segmentAnchor,
      previewAnchor: saved.segmentAnchor,
      mode: 'manual',
      fromExisting: NO_FROM_EXISTING,
      handoffPending: autoStart,
    },
    exit: null,
  };
}

/**
 * The reducer with its new state readable at once: `dispatch` returns it, and
 * `current()` reads the latest one dispatched. The wizard's transitions are
 * calls - entering `placing` starts the late import from the same handler that
 * emptied the queue - so the caller has to know where the action left the
 * wizard before React re-renders. One reducer application per dispatch (it is
 * pure; React does not replay it here).
 */
export function useWizardState(init: () => WizardState) {
  const [state, setState] = useState(init);
  const latest = useRef(state);
  const dispatch = useCallback((action: WizardAction): WizardState => {
    const next = wizardReducer(latest.current, action);
    if (next !== latest.current) {
      latest.current = next;
      setState(next);
    }
    return next;
  }, []);
  const current = useCallback(() => latest.current, []);
  return { state, dispatch, current };
}
