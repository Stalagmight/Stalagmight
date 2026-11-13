import { useState, useCallback, useEffect, useMemo, useReducer, useRef, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { blockedByIncompleteShots, countIncompleteShots } from '../common/incompleteShots';
import { blankEditableShot } from '../common/editableShot';
import { isEmittedShot, emittedBefore } from '../common/emittedShots';
import { reanchorAfterRunDelete, reanchorStation } from '../../utils/stationReanchor';
import { DialogWrapper } from './DialogWrapper';
import SectionPreview from './SectionPreview';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { SectionMetadataForm, listPeopleProps, type CreditLists } from '../common/SectionMetadataForm';
import { ShotTableEditor, type EditableShot } from '../common/ShotTableEditor';
import { parseGps, detectFormat, looksTransposed, looksTransposedRawDD, type GpsFormat } from '../../utils/gps';
import { MAX_DEPTH_REFERENCE_M } from '../../utils/gpsBounds';
import { formatFixed } from '../../i18n/format';
import { convertToEditableShots } from '../../hooks/useImportState';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { editableShotsToDmpSection } from '../../utils/shotConversion';
import type { DmpFile, DmpSection } from '../../types/commands';
import type { DmpSelectionConfig } from './DmpSelectionDialog';
import type { GpsCoordinates } from '../../types/project';
import type { Person } from '../../types/ipc.generated';
import { personKey } from '../../utils/people';
import { sectionColorPalette } from '../../utils/colors';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';

/** Same people in the same order, per role (the dirty guard's comparison). */
function samePeopleLists(a: CreditLists, b: CreditLists): boolean {
  const same = (x: readonly Person[], y: readonly Person[]) =>
    x.length === y.length && x.every((p, i) => personKey(p) === personKey(y[i]!));
  return same(a.surveyors, b.surveyors) && same(a.explorers, b.explorers);
}

export interface ZeroDatumConfigResult {
  sectionIndex: number;
  stationIndex: number;
  gps: GpsCoordinates;
  metadata: {
    name: string;
    /** Every shot of the section is credited to these (none = no override). */
    surveyors: Person[];
    explorers: Person[];
    date: string | null;
    color: string;
    // Carried through the #394/#534 re-entry paths like every other field,
    // but `handleConfirm`/`buildCurrentDraft` below always re-emit this
    // hardcoded 1.0 - there is no UI control for it yet, so nothing actually
    // writes back what was carried. A future per-field quality control needs
    // to read this back in, not just carry it.
    quality: number;
    direction: 'in' | 'out';
    /** Reverse this section end-to-end on import (#24). */
    reverse: boolean;
  };
  shots: EditableShot[];
}

/**
 * A snapshot of this dialog's live state, carried on ITS OWN Back to
 * 'zero-datum-selection' and re-seeded on a same-section forward pass (#534).
 * Frontend-only - never crosses IPC - so unlike a confirmed
 * `ZeroDatumConfigResult` it can hold GPS text that never parsed: the user
 * may press Back before finishing it, and losing the text they typed on the
 * way is exactly the kind of loss this carry exists to prevent.
 */
export interface ZeroDatumConfigDraft extends Omit<ZeroDatumConfigResult, 'gps'> {
  /** Null until `gpsInputText` parses. */
  gps: GpsCoordinates | null;
  /** The raw GPS box text. Optional: a confirmed `ZeroDatumConfigResult` (the
   *  WIZARD's one-hop carry, `App.tsx`'s `previousZeroDatumConfig` set from
   *  `dialogData.zeroDatumConfig`) has no such text and is also a valid
   *  `ZeroDatumConfigDraft` - `initialGpsInputRef` below reconstructs it from
   *  `gps` in that case. */
  gpsInputText?: string;
  /** The raw depth/elevation box text (#524), carried for the same reason as
   *  `gpsInputText`: a depth typed while the GPS box has not parsed exists
   *  NOWHERE else, since `gps.depth_reference` only comes into being on a
   *  confirm. Optional for the same reason too - the wizard's one-hop carry
   *  hands back a confirmed result with no raw text, and
   *  `initialDepthReferenceRef` reconstructs it from `gps` in that case. */
  depthReferenceText?: string;
}

/** A blank row for the shot table - one 'insert' action serves appending and
 *  inserting alike. The shared blank-row contract; `id` stays negative here as
 *  this dialog keys its rows by it. */
function makeNewShot(tempId: number): EditableShot {
  return { ...blankEditableShot(-tempId), tempId };
}

/**
 * The rows that will actually be imported - `buildZeroDatumSectionConfig`
 * serializes exactly these, and `editableShotsToDmpSection` draws exactly these.
 * The dialog must count stations over the SAME set: a blank row left among the
 * shots is dropped from the payload, so counting it here made the picker, the
 * preview and the re-anchor all disagree with the backend, and the GPS anchor
 * landed on a different station than the one shown.
 */
const isCounted = isEmittedShot;

/**
 * How many counted rows precede `arrayIndex`.
 *
 * `ShotTableEditor` hands its callbacks an index into the FULL `shots` array
 * (deleted rows included), while a station index counts only emitted rows, so
 * the two spaces have to be bridged before a pick can be re-anchored.
 */
const survivorsBefore = emittedBefore;

/**
 * The shot rows and the station pick, as ONE state.
 *
 * They belong together because no action touches only one of them: a row
 * joining or leaving the payload renumbers the stations the pick addresses, so
 * the new rows and the re-anchored pick have to be derived from the same
 * snapshot. Holding them apart is what broke this dialog three times - the last
 * shape read the rows from the render closure while committing the pick through
 * a ref, and `ShotTableEditor` fans `onShotChange` out ONE CALL PER CELL (a
 * pasted row is about five, an averaged azimuth two). Every call in the event
 * saw the same pre-edit row, so one row joining the payload shifted the pick
 * once per cell, and a row cleared back to blank never shifted it back.
 *
 * As a reducer each action reads the reducer's own `prev`, which already holds
 * the earlier cells of the same event: the first write makes the row counted and
 * owes the shift, the rest see it counted and owe nothing. The fan-out is
 * idempotent without anyone having to coalesce it.
 *
 * Every case is PURE - StrictMode (dev and the Playwright build) invokes
 * reducers twice on purpose, which is precisely what the ref-based version
 * could not survive.
 */
interface ShotEditState {
  shots: EditableShot[];
  /** Committed pick, in the CURRENT emitted-station numbering: a section of `s`
   *  counted rows has stations `0..s`. */
  stationIndex: number;
  /** What the preview highlights - follows dropdown hover, reverts to the pick. */
  previewStationIndex: number;
  /** Identity for the next blank row; never reused, so rows stay distinct
   *  however many one gesture adds. */
  nextTempId: number;
  /** Set once a shot row is edited, inserted or deleted - feeds the dirty
   *  guard (#F9). A hover/pick alone does not change the payload. */
  dirty: boolean;
}

type ShotEditAction =
  /** One cell of one row, at a FULL-array index (deleted rows included). */
  | { type: 'edit'; index: number; field: keyof EditableShot; value: number | string | boolean | null }
  /** `count` blank rows at a full-array index, or appended at `'end'`. One
   *  action however many rows: asked row by row, a host that rebuilds from a
   *  snapshot overwrites its own earlier inserts. */
  | { type: 'insert'; at: number | 'end'; count: number }
  /** Soft-delete a whole selection at once, so the pick can be judged against
   *  the run rather than one row at a time. */
  | { type: 'delete'; indices: number[] }
  | { type: 'pick'; index: number }
  /** Dropdown hover; `null` when it closes without a selection. */
  | { type: 'hover'; index: number | null };

/** Commit a pick: it is the value sent, the map highlight, and what a cancelled
 *  hover reverts to. */
function withPick(state: ShotEditState, shots: EditableShot[], pick: number): ShotEditState {
  return { ...state, shots, stationIndex: pick, previewStationIndex: pick };
}

function shotEditReducer(state: ShotEditState, action: ShotEditAction): ShotEditState {
  switch (action.type) {
    case 'edit': {
      const current = state.shots[action.index];
      if (!current) return state;
      const updated = { ...current, [action.field]: action.value } as EditableShot;
      const shots = [...state.shots];
      shots[action.index] = updated;
      // Only a row crossing INTO or OUT OF the payload renumbers anything.
      // Inserting it did not move the pick (an uncounted row changes no
      // numbering), so the shift is owed here - and owed back when the row is
      // cleared to blank.
      const was = isCounted(current);
      const now = isCounted(updated);
      if (was === now) return { ...state, shots, dirty: true };
      const delta = now ? 1 : -1;
      return {
        ...withPick(
          state,
          shots,
          reanchorStation(
            state.stationIndex,
            survivorsBefore(state.shots, action.index),
            delta,
            state.shots.filter(isCounted).length + delta,
          ),
        ),
        dirty: true,
      };
    }
    case 'insert': {
      const n = Math.max(1, action.count);
      const at = action.at === 'end' ? state.shots.length : action.at;
      const shots = [...state.shots];
      shots.splice(at, 0, ...Array.from({ length: n }, (_, i) => makeNewShot(state.nextTempId + i)));
      // The inserted rows are BLANK, so `isCounted` excludes them and the
      // station numbering does not move at all. Shifting by `n` here while the
      // dropdown counted the old total pushed the pick out of step with the
      // payload - and Confirm could anchor the datum to a different station, or
      // one past the end. Rows shift the pick only once they hold data, which
      // is the 'edit' case above. The call is still made, for the clamp.
      const counted = state.shots.filter(isCounted).length;
      return {
        ...withPick(
          state,
          shots,
          reanchorStation(state.stationIndex, survivorsBefore(state.shots, at), 0, counted),
        ),
        nextTempId: state.nextTempId + n,
        dirty: true,
      };
    }
    case 'delete': {
      const targets = action.indices.filter((i) => state.shots[i] && !state.shots[i]!.isDeleted);
      if (targets.length === 0) return state;
      const shots = [...state.shots];
      for (const i of targets) shots[i] = { ...shots[i]!, isDeleted: true };
      // Anchor against the EARLIEST removal, shifted by the whole run. Only the
      // rows that were COUNTED renumber anything: a blank row in the range is
      // absent from the payload, so removing it shifts no station. Subtracting
      // every target moved the anchor a station upstream.
      const counted = state.shots.filter(isCounted).length;
      const countedGone = targets.filter((i) => isCounted(state.shots[i])).length;
      return {
        ...withPick(
          state,
          shots,
          reanchorAfterRunDelete(
            state.stationIndex,
            survivorsBefore(state.shots, Math.min(...targets)),
            countedGone,
            counted - countedGone,
          ),
        ),
        dirty: true,
      };
    }
    case 'pick':
      return withPick(state, state.shots, action.index);
    case 'hover':
      return { ...state, previewStationIndex: action.index ?? state.stationIndex };
  }
}

/**
 * Rows straight from the file, the pick at the head, temp ids past the last row
 * - UNLESS this is a re-entry, in which case the rows and the pick are the ones
 * the user left with (#394 F10).
 *
 * The pick has to come back with them or the two disagree: `stationIndex` is
 * numbered over the EDITED rows, so restoring edited rows under a pick reset to
 * 0 would silently re-anchor the survey at the head of the section. Both, or
 * neither.
 */
function initialShotEditState({
  section,
  initialConfig,
}: {
  section: DmpSection | undefined;
  initialConfig: ZeroDatumConfigDraft | undefined;
}): ShotEditState {
  if (initialConfig) {
    const shots = initialConfig.shots;
    // Rows the user ADDED carry a `tempId` (`makeNewShot`), and the counter must
    // clear every one of them - not just the file's row count - or the next
    // insert would mint an id a carried row already holds, and this grid keys
    // its rows by that id.
    const highestTempId = shots.reduce((max, shot) => Math.max(max, shot.tempId ?? 0), 0);
    return {
      shots,
      stationIndex: initialConfig.stationIndex,
      previewStationIndex: initialConfig.stationIndex,
      nextTempId: Math.max(shots.length, highestTempId) + 1,
      // NOT dirty. Carried rows are what this dialog OPENED with, so re-showing
      // them is not an edit - only `edit`/`insert`/`delete` set the flag. A
      // `true` here would make every reopened dialog prompt about discarding
      // changes the user has not made yet.
      dirty: false,
    };
  }
  const shots = section ? convertToEditableShots(section.shots) : [];
  return { shots, stationIndex: 0, previewStationIndex: 0, nextTempId: shots.length + 1, dirty: false };
}

interface ZeroDatumConfigDialogProps {
  dmpFile: DmpFile;
  selectionConfig: DmpSelectionConfig;
  sectionIndex: number;
  filePath: string;
  onCancel: () => void;
  /**
   * Back, handed this dialog's OWN live state (#534) so App.tsx can relay it
   * through 'zero-datum-selection' and re-seed it on a same-section forward
   * pass - see `buildCurrentDraft`.
   */
  onBack: (current: ZeroDatumConfigDraft) => void;
  onConfirm: (result: ZeroDatumConfigResult) => void;
  /**
   * What this dialog was confirmed with LAST time (#394 F10), or left with on
   * ITS OWN Back and re-picked the same section (#534).
   *
   * Stepping Back out of the import wizard returns here, and the wizard
   * withdraws what it imported on the way (the forward pass re-imports from
   * scratch, so keeping would duplicate). Without this the user would also lose
   * the GPS and the metadata they typed, which turns "adjust one field" into
   * "start over". Absent on the first visit.
   */
  initialConfig?: ZeroDatumConfigDraft | undefined;
}

export default function ZeroDatumConfigDialog({
  dmpFile,
  selectionConfig,
  sectionIndex,
  filePath: _filePath,
  onCancel,
  onBack,
  onConfirm,
  initialConfig,
}: ZeroDatumConfigDialogProps) {
  const { t } = useTranslation();
  const showToast = useUIStore((st) => st.showToast);
  const precision = useDecimalPrecision();

  const section = dmpFile.sections[sectionIndex];
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);

  // Figure out the color index based on position in selected indices
  const colorIndex = useMemo(() => {
    const sorted = Array.from(selectionConfig.selectedIndices).sort((a, b) => a - b);
    return sorted.indexOf(sectionIndex);
  }, [selectionConfig.selectedIndices, sectionIndex]);

  const sectionPalette = useMemo(() => sectionColorPalette(resolvedTheme), [resolvedTheme]);

  // Expand grows the DIALOG, same as the other two shot-entry surfaces; the
  // editor only asks, the host owns its size.
  const [expanded, setExpanded] = useState(false);

  // The shots and the station pick, in one reducer - see `shotEditReducer` for
  // why they cannot be separate pieces of state.
  const [{ shots, stationIndex, previewStationIndex, dirty: shotsDirty }, dispatch] = useReducer(
    shotEditReducer,
    { section, initialConfig },
    initialShotEditState,
  );
  // The reducer's OWN initial pick, captured once - not hardcoded to 0. That
  // carry has now landed (#394 F10): on a re-entry this is
  // `initialConfig.stationIndex`, so a restored non-zero pick does not read as
  // a user change.
  const initialStationIndexRef = useRef(stationIndex);

  // GPS input state. Re-entered as DECIMAL DEGREES whatever the user originally
  // typed: `gps` is the already-parsed pair, and the source text (a UTM grid
  // reference, a maps URL) cannot be re-derived from it. Interpolated plainly
  // rather than through `formatGpsCoordinates`, because this string has to PARSE
  // again on the next confirm and that formatter is locale-aware - a comma
  // decimal separator would not round-trip through `parseDD`.
  //
  // `gpsInputText` (#534) carries the raw box text straight through when it
  // never parsed - a wizard-carried `ZeroDatumConfigResult` has no such text
  // (its `gps` always parsed, or it could not have been confirmed), so that
  // path still reconstructs from `gps`.
  //
  // The ref is the dirty guard's baseline: '' on a first visit, the carried
  // text on a re-entry. `useState` reads it so the displayed value and the
  // baseline cannot drift.
  const initialGpsInputRef = useRef(
    initialConfig
      ? (initialConfig.gpsInputText ??
          (initialConfig.gps ? `${initialConfig.gps.latitude}, ${initialConfig.gps.longitude}` : ''))
      : ''
  );
  const [gpsInput, setGpsInput] = useState(initialGpsInputRef.current);
  const [gpsCoords, setGpsCoords] = useState<GpsCoordinates | null>(initialConfig?.gps ?? null);
  const [gpsFormat, setGpsFormat] = useState<GpsFormat | null>(initialConfig?.gps ? 'DD' : null);
  const [gpsError, setGpsError] = useState<string | null>(null);

  // Depth/elevation of the zero-datum point relative to the surface (#524).
  // Optional: blank keeps today's behaviour (`depth_reference: 0`, "not
  // recorded"). Meters, +down - the same `GpsCoordinates.depth_reference`
  // frame `EditStationDialog` already writes from `station.coordinates.z`
  // (docs/inclination.md), and the OPPOSITE sign from #465's project-level
  // "entrance elevation" (+up from sea level) - the two are different
  // reference points and must not be confused.
  //
  // Seeded exactly like `initialGpsInputRef`: the carried RAW TEXT first, so a
  // depth typed while the GPS box was unparsable comes back (it exists nowhere
  // else - `gps.depth_reference` only appears on a confirm), then the
  // gps-derived value for the wizard's one-hop carry, which has no raw text.
  //
  // The `!= null` rather than a falsy check is deliberate, and a behaviour
  // change: a stored depth of exactly 0 means "recorded, at the surface" and now
  // seeds as "0" instead of blank. Blank and 0 confirm to the same
  // `depth_reference`, so nothing downstream moves - but the box no longer
  // claims the user never answered.
  const initialDepthReferenceRef = useRef(
    initialConfig?.depthReferenceText ??
      (initialConfig?.gps?.depth_reference != null
        ? String(initialConfig.gps.depth_reference)
        : '')
  );
  const [depthReferenceInput, setDepthReferenceInput] = useState(initialDepthReferenceRef.current);
  const depthReferenceParsed = depthReferenceInput.trim() === '' ? 0 : Number(depthReferenceInput);
  // Finiteness is not a range: `1e300` is finite and used to travel all the way
  // into the project's geometry. The backend refuses it now
  // (`GpsCoordinates::validate`), so accepting it here would only produce a
  // confirm that fails at the far end.
  const depthReferenceValid =
    Number.isFinite(depthReferenceParsed) &&
    Math.abs(depthReferenceParsed) <= MAX_DEPTH_REFERENCE_M;

  // Metadata state. Each field keeps an initial-value ref alongside it, so the
  // dirty guard (#F9) can tell "touched" from "still what the dialog opened
  // with" without re-deriving the same expression twice - and on a re-entry
  // (#394 F10) what it opened with is the CARRIED value, not the file default,
  // or every reopened dialog would report itself dirty on arrival.
  //
  // A field the user CLEARED last time stays cleared, so each branch is chosen
  // by whether there is a previous visit at all - `??` per field would
  // resurrect the file/selection default under an emptied one.
  const initialNameRef = useRef(initialConfig?.metadata.name ?? section?.name ?? '');
  const [name, setName] = useState(initialNameRef.current);
  const initialPeopleRef = useRef<CreditLists>(
    initialConfig
      ? { surveyors: initialConfig.metadata.surveyors ?? [], explorers: initialConfig.metadata.explorers ?? [] }
      : { surveyors: selectionConfig.defaultSurveyors ?? [], explorers: selectionConfig.defaultExplorers ?? [] }
  );
  const [people, setPeople] = useState<CreditLists>(initialPeopleRef.current);
  // Typed into a people field but not added yet: unsaved, like any other edit.
  const [peopleTextPending, setPeopleTextPending] = useState(false);
  const peopleProps = listPeopleProps(
    people,
    (role, next) => setPeople((prev) => ({ ...prev, [role]: next(prev[role]) })),
    setPeopleTextPending,
  );
  const initialDateRef = useRef(
    initialConfig
      ? (initialConfig.metadata.date ?? '')
      : (selectionConfig.defaultDate ?? section?.date?.split(/[T ]/)[0] ?? '')
  );
  const [date, setDate] = useState(initialDateRef.current);
  const initialColorRef = useRef(
    initialConfig?.metadata.color ?? sectionPalette[colorIndex % sectionPalette.length] ?? '#4ecdc4'
  );
  const [color, setColor] = useState(initialColorRef.current);
  const initialDirectionRef = useRef<'in' | 'out'>(
    initialConfig?.metadata.direction ?? (section?.direction === 'out' ? 'out' : 'in')
  );
  const [direction, setDirection] = useState(initialDirectionRef.current);
  const initialReverseRef = useRef(initialConfig?.metadata.reverse ?? false);
  const [reverse, setReverse] = useState(initialReverseRef.current);

  // Station count = counted shots + 1. The dropdown, the preview, the payload
  // and the pick all count by `isCounted` - one predicate, one station space.
  const stationCount = useMemo(() => {
    return shots.filter(isCounted).length + 1;
  }, [shots]);

  // The preview is drawn from the EDITED shots, not the file's original ones,
  // so a click on a station dot and a pick in the dropdown address the same
  // station. See `editableShotsToDmpSection` for why the raw section diverges
  // even before anything is deleted.
  const previewSection = useMemo(
    () => (section ? editableShotsToDmpSection(section, shots) : null),
    [section, shots],
  );

  /** Commit a station pick: the value sent, the map highlight and the hover-revert target. */
  const commitStation = useCallback((index: number) => dispatch({ type: 'pick', index }), []);

  // Station options for the Select component
  const stationOptions = useMemo(() => {
    return Array.from({ length: stationCount }, (_, i) => ({
      value: String(i),
      label: `Station ${i}`,
    }));
  }, [stationCount]);

  // GPS origin for map preview - only set when valid GPS coordinates are entered
  const gpsOrigin = useMemo(() => {
    if (!gpsCoords) return null;
    return { latitude: gpsCoords.latitude, longitude: gpsCoords.longitude };
  }, [gpsCoords]);

  // Soft (non-blocking) heuristic (#F7): a plausibly-valid pair that still
  // looks like the fields were swapped. Coexists with `gpsError` - never
  // replaces it, and never blocks Confirm. Falls back to the raw-pair variant
  // when parsing FAILED (`gpsCoords` is null): a swapped "120, 45" never
  // produces coordinates to run the coords-based check on, so without this
  // branch it got only the generic range error and no swap hint at all.
  const gpsTransposedWarning = useMemo(() => {
    if (gpsCoords) return looksTransposed(gpsCoords.latitude, gpsCoords.longitude);
    if (gpsError) return looksTransposedRawDD(gpsInput);
    return false;
  }, [gpsCoords, gpsError, gpsInput]);

  // Dirty guard (#F9): GPS text entered, the section renamed, a metadata
  // field touched, the zero-datum STATION PICK changed, or a shot
  // edited/inserted/deleted - anything ESC (or Cancel/Back) would otherwise
  // discard silently. `stationIndex` is sent in the confirm payload just like
  // the rest, so a pick the user actually changed is losable intent too - a
  // pick that only MOVED because of a shot edit is already covered by
  // `shotsDirty` (the reducer marks that itself), so this only fires for a
  // pick made with no accompanying edit.
  //
  // Every term compares against what the dialog OPENED with, which on a re-entry
  // (#394 F10) is the carried value. The GPS term especially: `!== ''` would
  // make every reopened dialog dirty on arrival, and both this guard and
  // DialogWrapper's "Unsaved Changes" would fire over zero edits. With an empty
  // baseline it reduces to exactly `gpsInput.trim() !== ''`, so first-visit
  // behaviour (including its tolerance of whitespace-only input) is unchanged.
  const isDirty =
    gpsInput.trim() !== initialGpsInputRef.current.trim() ||
    depthReferenceInput.trim() !== initialDepthReferenceRef.current.trim() ||
    name !== initialNameRef.current ||
    peopleTextPending ||
    !samePeopleLists(people, initialPeopleRef.current) ||
    date !== initialDateRef.current ||
    color !== initialColorRef.current ||
    direction !== initialDirectionRef.current ||
    reverse !== initialReverseRef.current ||
    stationIndex !== initialStationIndexRef.current ||
    shotsDirty;

  // Back has no DialogWrapper-level guard (that machinery only ever discards
  // via `onClose`/`onCancel`), so it gets its own small, self-contained confirm
  // here. Still relevant after #534, which hands `onBack` this dialog's LIVE
  // state (`buildCurrentDraft`, not just what it opened with): the carry only
  // survives a SAME-section forward pass out of 'zero-datum-selection' - a
  // different section, Back-ing further to 'dmp-selection', or leaving the
  // wizard entirely all still lose it, so the prompt is honest about a real
  // risk, not a phantom one. The backConfirm copy spells out the same
  // distinction to the user.
  /** This dialog's live state, handed to `onBack` (#534) so App.tsx can carry
   *  it through 'zero-datum-selection' and re-seed it as `initialConfig` on a
   *  same-section forward pass. Unlike `handleConfirm`'s payload, `gps` may be
   *  null - Back does not require the GPS box to have parsed. */
  const buildCurrentDraft = useCallback(
    (): ZeroDatumConfigDraft => ({
      sectionIndex,
      stationIndex,
      gps: gpsCoords,
      gpsInputText: gpsInput,
      depthReferenceText: depthReferenceInput,
      metadata: {
        name,
        surveyors: people.surveyors,
        explorers: people.explorers,
        date: date || null,
        color,
        direction,
        quality: 1.0,
        reverse,
      },
      shots,
    }),
    [sectionIndex, stationIndex, gpsCoords, gpsInput, depthReferenceInput, name, people, date, color, direction, reverse, shots]
  );

  const [backConfirmPending, setBackConfirmPending] = useState(false);
  const handleBackClick = useCallback(() => {
    if (isDirty) {
      setBackConfirmPending(true);
    } else {
      onBack(buildCurrentDraft());
    }
  }, [isDirty, onBack, buildCurrentDraft]);

  // Intercept Escape ourselves while the Back confirm is open, so it dismisses
  // ONLY that confirm - left alone, the keydown bubbles past this (unhandled)
  // to DialogWrapper's own handler, which opens ITS "Unsaved Changes" overlay
  // on top of this one (isDirty is still true; DialogWrapper has no idea this
  // local confirm exists). Wrapping the render-prop output in a `display:
  // contents` div (rather than a Fragment) is what gives this a DOM node to
  // attach the listener to without disturbing the flex layout below.
  const handleContentKeyDown = useCallback(
    (e: ReactKeyboardEvent<HTMLDivElement>) => {
      if (backConfirmPending && (e.key === 'Escape' || e.key === 'Esc') && !e.repeat) {
        e.preventDefault();
        e.stopPropagation();
        setBackConfirmPending(false);
      }
    },
    [backConfirmPending],
  );

  // When dropdown hovers an option, show it on the map; when dropdown closes
  // without selecting, the reducer reverts to the committed station.
  const handleStationHover = useCallback((value: string | null) => {
    dispatch({ type: 'hover', index: value === null ? null : parseInt(value, 10) });
  }, []);

  // GPS input handler. Takes the depth reference explicitly (rather than
  // closing over state) so it stays correct both on a text edit and, via the
  // re-parse effect below, on a depth-only edit of already-parsed text.
  const handleGpsInputChange = useCallback((value: string, depthReference: number) => {
    setGpsInput(value);
    if (!value.trim()) {
      setGpsCoords(null);
      setGpsFormat(null);
      setGpsError(null);
      return;
    }
    const detected = detectFormat(value);
    setGpsFormat(detected || null);
    const result = parseGps(value, depthReference);
    if ('coordinates' in result) {
      setGpsCoords(result.coordinates);
      setGpsError(null);
    } else {
      setGpsCoords(null);
      setGpsError(result.message);
    }
  }, []);

  // Depth-reference input handler: just stages the text. The effect below
  // re-parses already-entered GPS text against the new value, so a datum's
  // depth can be filled in before OR after its coordinates.
  const handleDepthReferenceInputChange = useCallback((value: string) => {
    setDepthReferenceInput(value);
  }, []);

  // Re-run the GPS parse whenever the depth reference changes, so a depth
  // typed after the coordinates still lands in `gpsCoords.depth_reference`
  // instead of being stuck at whatever the coordinates parsed with.
  useEffect(() => {
    if (gpsInput.trim()) {
      handleGpsInputChange(gpsInput, depthReferenceValid ? depthReferenceParsed : 0);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [depthReferenceParsed, depthReferenceValid]);

  // Shot editing handlers. Each is one dispatch: the reducer derives the new
  // rows and the re-anchored pick from the same `prev`, so `stationIndex`
  // always addresses the current, post-edit station order - and the editor's
  // per-cell fan-out cannot shift it more than once for one row.
  const handleShotChange = useCallback(
    (index: number, field: keyof EditableShot, value: number | string | boolean | null) => {
      dispatch({ type: 'edit', index, field, value });
    },
    []
  );

  // Appending goes through the same action as an insert, so the invariant has
  // one owner. `'end'` rather than a captured length: the trailing blank row is
  // materialized and then written in ONE event, and the reducer's array is the
  // only one that has both.
  const handleAddShot = useCallback(() => dispatch({ type: 'insert', at: 'end', count: 1 }), []);

  /** Append `count` blank shots in ONE action, each with its own temp id. Asked
   *  N times in one React event instead, a host that rebuilds from a captured
   *  array overwrites its own earlier appends and one that stamps rows from a
   *  captured temp id gives them all the same identity. */
  const handleAddShots = useCallback(
    (count: number) => dispatch({ type: 'insert', at: 'end', count }),
    []
  );

  /** Insert `count` blank shots at `index`, in ONE action, re-anchoring the
   *  station pick once for the whole run rather than per row. */
  const handleInsertShots = useCallback(
    (index: number, count: number, position: 'before' | 'after') => {
      dispatch({ type: 'insert', at: position === 'after' ? index + 1 : index, count });
    },
    []
  );

  /** Soft-delete every selected index in ONE action, so the pick is judged
   *  against the whole run. The answer the grid needs is synchronous (a `false`
   *  leaves its selection alone), so whether anything is left to delete is read
   *  from the rendered rows; the reducer decides again from its own state. */
  const handleDeleteShots = useCallback(
    (indices: number[]): boolean => {
      if (!indices.some((i) => shots[i] && !shots[i]!.isDeleted)) return false;
      dispatch({ type: 'delete', indices });
      return true;
    },
    [shots]
  );

  const handleDeleteShot = useCallback(
    (index: number) => dispatch({ type: 'delete', indices: [index] }),
    []
  );

  /** Rows that hold something but are not a usable shot - see countIncompleteShots. */
  const incompleteShots = useMemo(() => countIncompleteShots(shots), [shots]);

  const handleConfirm = useCallback(() => {
    if (!gpsCoords) return;
    if (!depthReferenceValid) return;
    if (blockedByIncompleteShots(incompleteShots, t, TOAST_KEYS.ZERO_DATUM_INCOMPLETE)) return;
    onConfirm({
      sectionIndex,
      stationIndex,
      gps: gpsCoords,
      metadata: {
        name,
        surveyors: people.surveyors,
        explorers: people.explorers,
        date: date || null,
        color,
        direction,
        quality: 1.0,
        reverse,
      },
      shots,
    });
  }, [gpsCoords, depthReferenceValid, onConfirm, sectionIndex, stationIndex, name, people, date, color, direction, reverse, shots, incompleteShots, showToast, t]);

  if (!section) return null;

  return (
    <DialogWrapper
      title={t('dialogs.zeroDatumConfig.title')}
      onClose={onCancel}
      isDirty={isDirty}
      size={expanded ? 'fullscreen' : '5xl'}
      overflow="hidden"
      {...(expanded
        ? {}
        : { fixedSize: { width: 'min(1100px, 95vw)', height: 'min(800px, 90vh)' } })}
      bodyClassName="flex-1 flex flex-col p-0 min-h-0"
      closeOnBackdropClick={false}
      data-testid="zero-datum-config-dialog"
    >
      {({ requestClose }) => (
      <div className="contents" onKeyDown={handleContentKeyDown}>
      {/* Scrollable content area */}
      <div className="flex-1 flex min-h-0 overflow-hidden">
        {/* Left column: Preview (fixed) */}
        <div
          className="shrink-0 flex flex-col items-center p-4 border-r border-border"
          style={{ width: 380 }}
          data-testid="zero-datum-preview-map"
        >
          <SectionPreview
            section={previewSection}
            color={color}
            size={340}
            onStationClick={commitStation}
            selectedStationIndex={previewStationIndex}
            gpsOrigin={gpsOrigin}
            gpsStationIndex={stationIndex}
          />
        </div>

        {/* Right column: Configuration (scrollable) */}
        <div
          className={`flex-1 min-w-0 p-4 space-y-5 ${
            // Expanded, the column stops scrolling as a whole so the shot table
            // can own the leftover height; collapsed it scrolls as before.
            expanded ? 'flex min-h-0 flex-col overflow-hidden' : 'overflow-y-auto'
          }`}
          data-testid="zero-datum-config-right-pane"
        >
          {/* Zero Datum Location - prominent card */}
          <div className="rounded-lg border border-accent/30 bg-accent/5 p-4 space-y-3">
            <div>
              <h3 className="text-sm font-semibold text-text-primary">
                {t('dialogs.zeroDatumConfig.zeroDatumHeading')}
              </h3>
              <p className="text-xs text-text-secondary mt-0.5">
                {t('dialogs.zeroDatumConfig.zeroDatumDescription')}
              </p>
            </div>

            {/* Station selection */}
            <div className="flex items-center gap-2">
              <label className="text-sm text-text-secondary shrink-0">
                {t('dialogs.zeroDatumConfig.stationLabel')}:
              </label>
              <Select
                value={String(stationIndex)}
                onChange={(v) => commitStation(parseInt(v, 10))}
                options={stationOptions}
                className="flex-1"
                data-testid="zero-datum-station-select"
                onOptionHover={handleStationHover}
              />
            </div>

            {/* GPS input */}
            <div className="space-y-1">
              <div className="flex items-center gap-2">
                <label className="text-sm text-text-secondary shrink-0">
                  {t('dialogs.zeroDatumConfig.gpsLabel')}:
                </label>
                <input
                  type="text"
                  value={gpsInput}
                  onChange={(e) =>
                    handleGpsInputChange(
                      e.target.value,
                      depthReferenceValid ? depthReferenceParsed : 0,
                    )
                  }
                  placeholder={t('dialogs.zeroDatumConfig.gpsPlaceholder')}
                  className="flex-1 px-2 py-1.5 bg-background border border-border rounded text-sm"
                  data-testid="zero-datum-gps-input"
                />
                {gpsFormat && (
                  <span
                    className="text-xs px-2 py-1 bg-accent-10 text-accent rounded"
                    data-testid="zero-datum-gps-format"
                  >
                    {gpsFormat}
                  </span>
                )}
              </div>
              {!gpsInput.trim() && !gpsError && (
                <div className="text-xs text-text-secondary pl-1" data-testid="zero-datum-gps-empty-hint">
                  {t('dialogs.zeroDatumConfig.gpsEmptyHint')}
                </div>
              )}
              {gpsError && (
                <div className="text-xs text-error pl-1" data-testid="zero-datum-gps-error">
                  {gpsError}
                </div>
              )}
              {gpsCoords && (
                <div className="text-xs text-success pl-1" data-testid="zero-datum-gps-parsed">
                  Lat: {formatFixed(gpsCoords.latitude, undefined, precision.gps)}, Lon:{' '}
                  {formatFixed(gpsCoords.longitude, undefined, precision.gps)}
                </div>
              )}
              {gpsTransposedWarning && (
                <div className="text-xs text-warning pl-1" data-testid="zero-datum-gps-transposed-warning">
                  {t('dialogs.zeroDatumConfig.transposedWarning')}
                </div>
              )}
            </div>

            {/* Depth/elevation reference (#524) - optional, meters, +down
                (docs/inclination.md). Blank keeps today's behaviour: the
                datum's depth is recorded as 0 ("not recorded"). */}
            <div className="space-y-1">
              <div className="flex items-center gap-2">
                <label className="text-sm text-text-secondary shrink-0">
                  {t('dialogs.zeroDatumConfig.depthReferenceLabel')}:
                </label>
                <input
                  type="text"
                  inputMode="decimal"
                  value={depthReferenceInput}
                  onChange={(e) => handleDepthReferenceInputChange(e.target.value)}
                  placeholder={t('dialogs.zeroDatumConfig.depthReferencePlaceholder')}
                  className="flex-1 px-2 py-1.5 bg-background border border-border rounded text-sm"
                  data-testid="zero-datum-depth-reference-input"
                />
              </div>
              <div className="text-xs text-text-secondary pl-1">
                {t('dialogs.zeroDatumConfig.depthReferenceHint')}
              </div>
              {!depthReferenceValid && (
                <div className="text-xs text-error pl-1" data-testid="zero-datum-depth-reference-error">
                  {t('dialogs.zeroDatumConfig.depthReferenceError')}
                </div>
              )}
            </div>
          </div>

          {/* Section name */}
          <div>
            <h3 className="text-sm font-medium text-text-primary mb-2">
              {t('dialogs.zeroDatumConfig.metadataHeading')}
            </h3>
            <label className="block text-sm text-text-secondary mb-1">{t('common.labels.name')}</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full px-3 py-2 bg-background border border-border rounded"
              placeholder={t('sectionMetadata.placeholders.name')}
              data-testid="section-name-input"
            />
          </div>

          <div
            className={`space-y-5 ${expanded ? 'flex min-h-0 flex-1 flex-col' : ''}`}
            data-testid="zero-datum-editable-rest"
          >
            <SectionMetadataForm
              name={name}
              people={peopleProps}
              peopleColumns
              date={date}
              color={color}
              direction={direction}
              onNameChange={setName}
              onDateChange={setDate}
              onColorChange={setColor}
              onDirectionChange={setDirection}
              reverse={reverse}
              onReverseChange={setReverse}
              showName={false}
              showDirection
              showReverse
            />

            {/* Shot table */}
            <div className={expanded ? 'flex min-h-0 flex-1 flex-col' : undefined}>
              <h3 className="text-sm font-medium text-text-primary mb-2">
                {t('dialogs.zeroDatumConfig.shotsHeading')}
              </h3>
              <ShotTableEditor
                shots={shots}
                onShotChange={handleShotChange}
                onAddShot={handleAddShot}
                onAddShots={handleAddShots}
                onInsertShots={handleInsertShots}
                onDeleteShot={handleDeleteShot}
                onDeleteShots={handleDeleteShots}
                expanded={expanded}
                onToggleExpand={() => setExpanded((v) => !v)}
              />
            </div>
          </div>
        </div>
      </div>

      {/* Footer - always visible */}
      <div className="px-4 py-3 border-t border-border flex justify-between items-center shrink-0">
        <div className="flex gap-2">
          <Button variant="secondary" onClick={requestClose} data-testid="zero-datum-config-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="secondary" onClick={handleBackClick} data-testid="zero-datum-config-back">
            {t('common.actions.back')}
          </Button>
        </div>
        <Button
          variant="primary"
          onClick={handleConfirm}
          disabled={!gpsCoords || !depthReferenceValid}
          data-testid="zero-datum-config-confirm"
        >
          {t('dialogs.zeroDatumConfig.confirm')}
        </Button>
      </div>

      {/* Small, self-contained Back-discard confirm - see the `isDirty`
          comment above for why this cannot reuse DialogWrapper's own. */}
      {backConfirmPending && (
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center"
          data-testid="zero-datum-config-back-confirm"
        >
          <div
            className="absolute inset-0 bg-overlay"
            onClick={() => setBackConfirmPending(false)}
            data-testid="zero-datum-config-back-confirm-backdrop"
          />
          <div className="relative bg-panel border border-border rounded-lg shadow-xl max-w-sm w-full mx-4">
            <div className="p-4 space-y-4">
              <p>{t('dialogs.zeroDatumConfig.backConfirm.message')}</p>
              <div className="flex justify-end gap-2 pt-4 border-t border-border">
                <Button
                  onClick={() => setBackConfirmPending(false)}
                  data-testid="zero-datum-config-back-confirm-cancel"
                >
                  {t('common.actions.cancel')}
                </Button>
                <Button
                  variant="danger"
                  onClick={() => {
                    setBackConfirmPending(false);
                    onBack(buildCurrentDraft());
                  }}
                  data-testid="zero-datum-config-back-confirm-discard"
                >
                  {t('dialogs.zeroDatumConfig.backConfirm.discard')}
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}
      </div>
      )}
    </DialogWrapper>
  );
}
