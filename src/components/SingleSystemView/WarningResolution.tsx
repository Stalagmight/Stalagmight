import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { Button } from '../common/Button';
import { ChevronLeftIcon, CenterMapIcon } from '../common/icons';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useActiveStations } from '../../hooks/useActiveStations';
import { normalizeAzimuth, averageAzimuth } from '../../utils/azimuth';
import {
  computeGhostEnd,
  horizontalLength,
  lengthFromInclination,
  lengthFixableFromInclination,
  exceedsSlopeLength,
} from '../../utils/resolutionPreview';
import { recordedPitch as recordedPitchOf, impliedPitchDeg } from '../../utils/shotGeometry';
import { setSource } from '../../utils/telemetry';
import type { Section, Shot, ShotUpdates } from '../../types/project';
import type { GeometryPreview } from '../../store/uiStore';
import type { ProjectWarning } from '../../types/commands';
import { TOAST_KEYS } from '../../utils/toastKeys';

/** Color for the on-map resolution preview ghost (issue #57). */
const PREVIEW_COLOR = '#38bdf8';

/** Average of the recorded pitch legs (degrees), or null if none recorded. */
function recordedPitch(shot: Shot): number | null {
  return recordedPitchOf(shot.pitch_in, shot.pitch_out);
}

/** Signed inclination implied by the depth change and slope length, in the +down
 *  convention used for stored pitch and everywhere in the UI (DetailPanel's
 *  computeInclination): POSITIVE = downward (the shot descends, depth INCREASES),
 *  negative = upward. That is the frame `depth_in` / `depth_out` are already in,
 *  so this compares directly against the recorded pitch and against the backend's
 *  cross-check, with nothing negated in between (issue #57 finding #7, issue #296).
 *  Returns 0 for length<=0. */
function impliedInclination(shot: Shot): number {
  return impliedPitchDeg(shot.length, shot.depth_in, shot.depth_out);
}

/**
 * The pre-selected fix for an inclination in/out warning. Normally the average
 * of the two readings. But when exactly one leg is exactly 0° the other one is
 * kept instead: older Compass imports stored a back-sight's MISSING inclination
 * as 0°, and averaging a real 30° with that would write 15° to both legs.
 */
export function pitchDeviationDefault(shot: Shot): 'pitch-use-in' | 'pitch-use-out' | 'pitch-average' {
  if (shot.pitch_out === 0 && shot.pitch_in != null && shot.pitch_in !== 0) return 'pitch-use-in';
  if (shot.pitch_in === 0 && shot.pitch_out != null && shot.pitch_out !== 0) return 'pitch-use-out';
  return 'pitch-average';
}

/** What a manual option needs to render as an inline editable field. */
export type ManualKind =
  | 'azimuth' // single corrected azimuth, applied to both in & out
  | 'azimuthInOut' // separate in / out azimuths
  | 'length'
  | 'depth' // depth in / out
  | 'pitch' // single inclination, applied to both in & out
  | 'lrud'; // L R U D

interface ResolutionOption {
  id: string;
  /** Action label (e.g. "Average"); a computed value is appended in render. */
  label: string;
  /** Optional formatted preview value shown after the label, e.g. "(57.0°)". */
  detail?: string | undefined;
  /** Extra context shown below the option only while it is selected (like the
   *  inline manual editors), e.g. the inclination a length was derived from. */
  selectedNote?: string | undefined;
  /** Destructive option (e.g. delete the shot) - rendered in red. */
  danger?: boolean | undefined;
  /** Disabled (frozen / closure-adjusted view) - applies to data fixes. */
  disabled?: boolean;
  /** Reveals an inline editor when selected. */
  manual?: ManualKind;
  /** Selecting this navigates away instead of applying a fix. */
  navigate?: boolean;
  /** Suppress (Ignore) rather than fix the data. */
  ignore?: boolean;
}

interface WarningResolutionProps {
  warning: ProjectWarning;
  section: Section | undefined;
  shot: Shot | undefined;
  /** Present only during "Resolve all"; drives the "n of N" header + Skip. */
  position?: { index: number; total: number } | undefined;
  /** ‹ Back (single) / Exit (resolve-all). */
  onExit: () => void;
  /** A fix was applied or the warning was ignored - advance. */
  onResolved: () => void;
  /** Skip this warning without changing anything - advance. */
  onSkip: () => void;
}

const LRUD_FIELDS = ['left', 'right', 'up', 'down'] as const;
type LrudField = (typeof LRUD_FIELDS)[number];

/**
 * The proposed RAW shot fields for a resolution option - the single source the
 * on-map preview and the applied `update_shot` patch are BOTH derived from, so
 * the ghost and the committed change cannot diverge (issue #57).
 *
 * Azimuth is always carried as both legs (`azimuthIn`/`azimuthOut`) so the
 * preview can take their average - for the single-leg options (`use-in`/
 * `use-out`) both legs are set equal so `averageAzimuth(x, x) === x` reproduces
 * the old preview azimuth; the apply translation still emits only the one
 * changed leg for those. `invalid` reports a rejected manual input, mapped by
 * the caller to the same per-kind toast message as before.
 */
interface ProposedRaw {
  azimuthIn?: number;
  azimuthOut?: number;
  length?: number;
  pitchIn?: number;
  pitchOut?: number;
  depthIn?: number;
  depthOut?: number;
  lrud?: Partial<Record<LrudField, number | null>>;
  invalid?: 'azimuth' | 'length' | 'pitch' | 'invalid';
}

/** Strict numeric parse of a manual field: NaN unless the ENTIRE trimmed string
 *  is a finite number. Unlike parseFloat, this rejects trailing garbage such as
 *  "5abc" (which parseFloat would silently read as 5) (issue #57, finding #11). */
function strictNum(raw: string | undefined): number {
  const s = (raw ?? '').trim();
  if (s === '') return NaN;
  const n = Number(s);
  return Number.isFinite(n) ? n : NaN;
}

/** Strict numeric parse of a manual field (NaN when empty/non-numeric). */
function manualNum(manual: Record<string, string>, key: string): number {
  return strictNum(manual[key]);
}

/**
 * Azimuth field parse: reject NaN or values outside [-360, 360], else normalize
 * into [0, 360). Returns null on rejection (mirrors the component's parseAzimuth).
 */
function parseManualAzimuth(manual: Record<string, string>, key: string): number | null {
  const v = manualNum(manual, key);
  if (Number.isNaN(v) || v < -360 || v > 360) return null;
  return normalizeAzimuth(v);
}

/**
 * Map a resolution option id to the proposed RAW shot fields. Pure (no React,
 * no toast) and exported so the parity test can assert preview/apply derive
 * identically. Returns `null` for options that never change shot data
 * (ignore/navigate/remove-datum/delete-shot/unknown); for data fixes returns the
 * proposed fields, or `{ invalid }` for a rejected manual input.
 */
export function computeProposedRaw(
  optId: string,
  manualKind: ManualKind | undefined,
  shot: Shot,
  manual: Record<string, string>,
  negativeLrud: readonly LrudField[],
): ProposedRaw | null {
  const dz = shot.depth_out - shot.depth_in;
  switch (optId) {
    case 'use-in':
      // Apply writes only azimuth_out; both legs equal here so the preview's
      // averageAzimuth(in, out) === shot.azimuth_in (the old preview azimuth).
      return { azimuthIn: shot.azimuth_in, azimuthOut: shot.azimuth_in };
    case 'use-out':
      return { azimuthIn: shot.azimuth_out, azimuthOut: shot.azimuth_out };
    case 'average': {
      const avg = averageAzimuth(shot.azimuth_in, shot.azimuth_out);
      return { azimuthIn: avg, azimuthOut: avg };
    }
    case 'normalize':
      return { azimuthIn: normalizeAzimuth(shot.azimuth_in), azimuthOut: normalizeAzimuth(shot.azimuth_out) };
    // PITCH_DEVIATION: make the two inclination readings agree. Only offered
    // when both legs are recorded, so the non-null fallbacks never apply.
    case 'pitch-use-in': {
      const p = shot.pitch_in ?? shot.pitch_out;
      return p == null ? { invalid: 'invalid' } : { pitchIn: p, pitchOut: p };
    }
    case 'pitch-use-out': {
      const p = shot.pitch_out ?? shot.pitch_in;
      return p == null ? { invalid: 'invalid' } : { pitchIn: p, pitchOut: p };
    }
    case 'pitch-average': {
      const p = recordedPitch(shot);
      return p === null ? { invalid: 'invalid' } : { pitchIn: p, pitchOut: p };
    }
    case 'incl-from-depth': {
      const p = impliedInclination(shot);
      return { pitchIn: p, pitchOut: p };
    }
    case 'split-incl-length': {
      const recorded = recordedPitch(shot);
      if (recorded === null) return { invalid: 'invalid' };
      const mid = (recorded + impliedInclination(shot)) / 2;
      const length = lengthFromInclination(dz, mid);
      if (length === null) return { invalid: 'invalid' };
      return { pitchIn: mid, pitchOut: mid, length };
    }
    case 'length-from-depth': {
      // Setting length to the depth change makes the shot vertical, so also set
      // the inclination to the matching ±90° in the +down convention (deeper =>
      // depth increases => +90; shallower => -90), otherwise a recorded
      // non-vertical pitch would immediately trip a fresh INCLINATION_DEPTH_MISMATCH
      // (issue #57 finding #2, issue #296).
      const p = dz > 0 ? 90 : -90;
      return { length: Math.abs(dz), pitchIn: p, pitchOut: p };
    }
    case 'length-from-incl': {
      const recorded = recordedPitch(shot);
      const length = lengthFromInclination(dz, recorded);
      if (length === null) return { invalid: 'invalid' };
      return { length };
    }
    case 'lrud-abs': {
      // Return { lrud } even when nothing is negative so the empty-patch guard
      // still fires resolveInvalid (rather than a generic failure).
      const lrud: Partial<Record<LrudField, number | null>> = {};
      for (const f of negativeLrud) lrud[f] = Math.abs(shot[f] ?? 0);
      return { lrud };
    }
    case 'lrud-clear': {
      const lrud: Partial<Record<LrudField, number | null>> = {};
      for (const f of negativeLrud) lrud[f] = null;
      return { lrud };
    }
    case 'manual':
      if (manualKind === 'azimuth') {
        const n = parseManualAzimuth(manual, 'azimuth');
        if (n === null) return { invalid: 'azimuth' };
        return { azimuthIn: n, azimuthOut: n };
      } else if (manualKind === 'azimuthInOut') {
        const ni = parseManualAzimuth(manual, 'azimuthIn');
        const no = parseManualAzimuth(manual, 'azimuthOut');
        if (ni === null || no === null) return { invalid: 'azimuth' };
        return { azimuthIn: ni, azimuthOut: no };
      } else if (manualKind === 'length') {
        const v = manualNum(manual, 'length');
        if (Number.isNaN(v) || v <= 0) return { invalid: 'length' };
        return { length: v };
      } else if (manualKind === 'pitch') {
        // Inclination is only physical in [-90°, 90°] (issue #57, finding #9).
        const v = manualNum(manual, 'pitch');
        if (Number.isNaN(v) || v < -90 || v > 90) return { invalid: 'pitch' };
        return { pitchIn: v, pitchOut: v };
      } else if (manualKind === 'depth') {
        const vi = manualNum(manual, 'depthIn');
        const vo = manualNum(manual, 'depthOut');
        if (Number.isNaN(vi) || Number.isNaN(vo)) return { invalid: 'invalid' };
        return { depthIn: vi, depthOut: vo };
      } else if (manualKind === 'lrud') {
        // Iterate ALL LRUD fields (null for empty, parseFloat otherwise) so the
        // patch keys match the old apply switch byte-for-byte.
        const lrud: Partial<Record<LrudField, number | null>> = {};
        for (const f of LRUD_FIELDS) {
          const rawValue = manual[f]?.trim() ?? '';
          // Strict parse: reject "5abc" rather than silently reading it as 5.
          lrud[f] = rawValue === '' ? null : strictNum(rawValue);
        }
        if (LRUD_FIELDS.some((f) => lrud[f] !== null && Number.isNaN(lrud[f] as number))) {
          return { invalid: 'invalid' };
        }
        return { lrud };
      }
      return null;
    default:
      return null;
  }
}

export function WarningResolution({
  warning,
  section,
  shot,
  position,
  onExit,
  onResolved,
  onSkip,
}: WarningResolutionProps) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { updateShot, removeZeroDatum, deleteShot, removeLoopClosure, resolveWarning, isShotFrozen } = useProjectStore(
    useShallow((s) => ({
      updateShot: s.updateShot,
      removeZeroDatum: s.removeZeroDatum,
      deleteShot: s.deleteShot,
      removeLoopClosure: s.removeLoopClosure,
      resolveWarning: s.resolveWarning,
      isShotFrozen: s.isShotFrozen,
    })),
  );
  // The closure a CLOSURE_ACROSS_ONE_SHOT warning is about (#737), looked up by
  // the id the warning carries; its first station is where "Go to" lands.
  const closureStation = useProjectStore((s) => {
    if (warning.closureId === undefined) return undefined;
    const closure = s.project?.loop_closures.find((lc) => Number(lc.id) === warning.closureId);
    return closure?.station_a;
  });
  const { setSelection, setDetailPanelTab, triggerCenterOnStation, showToast, setGeometryPreview, triggerZoomToShot } = useUIStore(
    useShallow((s) => ({
      setSelection: s.setSelection,
      setDetailPanelTab: s.setDetailPanelTab,
      triggerCenterOnStation: s.triggerCenterOnStation,
      showToast: s.showToast,
      setGeometryPreview: s.setGeometryPreview,
      triggerZoomToShot: s.triggerZoomToShot,
    })),
  );
  const editingBlocked = useEditingBlocked();
  // "Ignore" persists to the project database immediately; a never-saved project
  // (deferred initial save) reserves file_path but has no open database yet, so the
  // suppression can't be stored. Gate Ignore on the database actually being open,
  // not on file_path (which is set even for a deferred project) (issue #57 / Codex).
  const projectSaved = useProjectStore((s) => s.databaseOpen);
  const activeStations = useActiveStations();

  const frozen = isShotFrozen(warning.sectionId, warning.shotIndex);
  const geometryDisabled = editingBlocked || frozen;

  const [selected, setSelected] = useState<string>('');
  const [busy, setBusy] = useState(false);
  // Inline manual-edit fields (string-backed like EditShotDialog).
  const [manual, setManual] = useState<Record<string, string>>({});

  const az = (v: number) => `${formatFixed(v, undefined, precision.azimuth)}°`;
  const len = (v: number) => `${formatFixed(v, undefined, precision.length)} ${t('common.units.meters')}`;
  const dep = (v: number) => `${formatFixed(v, undefined, precision.depth)} ${t('common.units.meters')}`;

  // Which LRUD fields are negative (NEGATIVE_LRUD can flag any of them).
  const negativeLrud = useMemo<LrudField[]>(
    () => (shot ? LRUD_FIELDS.filter((f) => (shot[f] ?? 0) < 0) : []),
    [shot],
  );

  const options = useMemo<ResolutionOption[]>(() => {
    if (!shot) return [];
    const ignore: ResolutionOption = {
      id: 'ignore',
      label: t('detailPanel.warnings.resolveOptions.ignore'),
      ignore: true,
      // Can't persist a suppression with no database yet (unsaved new project).
      disabled: !projectSaved,
    };
    // Errors are never ignorable (issue #57): offer Ignore only for advisory
    // (warning-severity) issues. Spread this where Ignore used to be hardcoded so
    // a single check governs every code.
    const maybeIgnore: ResolutionOption[] = warning.severity === 'error' ? [] : [ignore];
    switch (warning.code) {
      case 'AZIMUTH_DEVIATION': {
        const avg = averageAzimuth(shot.azimuth_in, shot.azimuth_out);
        return [
          { id: 'use-in', label: t('detailPanel.warnings.resolveOptions.useIn'), detail: az(shot.azimuth_in), disabled: geometryDisabled },
          { id: 'use-out', label: t('detailPanel.warnings.resolveOptions.useOut'), detail: az(shot.azimuth_out), disabled: geometryDisabled },
          { id: 'average', label: t('detailPanel.warnings.resolveOptions.average'), detail: az(avg), disabled: geometryDisabled },
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manual'), manual: 'azimuth', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      }
      case 'INVALID_AZIMUTH':
        return [
          { id: 'normalize', label: t('detailPanel.warnings.resolveOptions.normalize'), detail: `${az(normalizeAzimuth(shot.azimuth_in))} / ${az(normalizeAzimuth(shot.azimuth_out))}`, disabled: geometryDisabled },
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manual'), manual: 'azimuthInOut', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      case 'PITCH_DEVIATION': {
        // Two forward inclination readings that disagree (like AZIMUTH_DEVIATION
        // for azimuth): keep one, average them, or type the right value.
        const pitchOf = (v: number | null | undefined) => `${formatFixed(v ?? 0, undefined, precision.inclination)}°`;
        const avg = recordedPitch(shot);
        const zeroLegDefault = pitchDeviationDefault(shot);
        const zeroLegNote = (id: string) =>
          zeroLegDefault === id ? t('detailPanel.warnings.zeroInclinationLegNote') : undefined;
        return [
          { id: 'pitch-use-in', label: t('detailPanel.warnings.resolveOptions.useInclinationIn'), detail: pitchOf(shot.pitch_in), selectedNote: zeroLegNote('pitch-use-in'), disabled: geometryDisabled },
          { id: 'pitch-use-out', label: t('detailPanel.warnings.resolveOptions.useInclinationOut'), detail: pitchOf(shot.pitch_out), selectedNote: zeroLegNote('pitch-use-out'), disabled: geometryDisabled },
          { id: 'pitch-average', label: t('detailPanel.warnings.resolveOptions.average'), detail: pitchOf(avg), disabled: geometryDisabled },
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manualPitch'), manual: 'pitch', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      }
      case 'INVALID_PITCH':
        // An out-of-range inclination is an error (not ignorable). Correct it
        // with a manual in-range pitch (issue #57, finding #9).
        return [
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manualPitch'), manual: 'pitch', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      case 'NON_FINITE_VALUE': {
        // Backstop for persisted non-finite geometry (#275): parsers coerce at
        // ingestion, so this fires mostly on projects saved before the guards
        // existed. Error severity, never ignorable - without an explicit case
        // it fell to `default: maybeIgnore` = ZERO options, the only Error
        // code with no way to act. Offer a manual editor for the first
        // corrupted field group, plus delete for an unsalvageable shot.
        const nf = (v: number | null | undefined) => v != null && !Number.isFinite(v);
        const manualTarget =
          nf(shot.length) ? ('length' as const)
          : nf(shot.azimuth_in) || nf(shot.azimuth_out) ? ('azimuthInOut' as const)
          : nf(shot.pitch_in) || nf(shot.pitch_out) ? ('pitch' as const)
          : nf(shot.depth_in) || nf(shot.depth_out) ? ('depth' as const)
          : ('lrud' as const);
        return [
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manual'), manual: manualTarget, disabled: geometryDisabled },
          { id: 'delete-shot', label: t('detailPanel.warnings.resolveOptions.deleteShot'), disabled: geometryDisabled, danger: true },
          ...maybeIgnore,
        ];
      }
      case 'LONG_SHOT':
        return [
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manualLength'), manual: 'length', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      case 'ZERO_LENGTH':
      case 'NEGATIVE_LENGTH':
      case 'SHORT_SHOT':
        // Failing the minimum-length test: correct the length, delete the shot
        // (a spurious too-short segment is often best removed), or ignore.
        return [
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manualLength'), manual: 'length', disabled: geometryDisabled },
          { id: 'delete-shot', label: t('detailPanel.warnings.resolveOptions.deleteShot'), disabled: geometryDisabled, danger: true },
          ...maybeIgnore,
        ];
      case 'LARGE_DEPTH_CHANGE':
        return [
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manualDepth'), manual: 'depth', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      case 'NEGATIVE_LRUD':
        return [
          { id: 'lrud-abs', label: t('detailPanel.warnings.resolveOptions.useAbs'), disabled: editingBlocked },
          { id: 'lrud-clear', label: t('detailPanel.warnings.resolveOptions.clear'), disabled: editingBlocked },
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manual'), manual: 'lrud', disabled: editingBlocked },
          ...maybeIgnore,
        ];
      case 'LENGTH_BELOW_DEPTH_CHANGE': {
        const dz = shot.depth_out - shot.depth_in;
        const recorded = recordedPitch(shot);
        const lengthFromIncl = lengthFromInclination(dz, recorded);
        return [
          // Default: shortest valid length is the vertical change (vertical shot).
          { id: 'length-from-depth', label: t('detailPanel.warnings.resolveOptions.lengthFromDepth'), detail: len(Math.abs(dz)), disabled: geometryDisabled },
          // Only when the recorded inclination points the same way as the depth
          // change (a positive length can satisfy it); withheld for a reversed
          // pitch sign, which no length can fix (issue #57 / Codex).
          ...(lengthFromIncl !== null && recorded !== null && lengthFixableFromInclination(dz, recorded)
            ? [{ id: 'length-from-incl', label: t('detailPanel.warnings.resolveOptions.lengthFromInclination'), detail: len(lengthFromIncl), selectedNote: t('detailPanel.warnings.fromInclinationNote', { value: formatFixed(recorded, undefined, precision.inclination) }), disabled: geometryDisabled }]
            : []),
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manualLength'), manual: 'length', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      }
      case 'INCLINATION_DEPTH_MISMATCH': {
        // Depth gauges are trusted, so the depth change is never altered here. The
        // implied inclination is signed in the +down convention (DetailPanel's
        // computeInclination), matching the recorded pitch and the backend check.
        // Offer to adjust the inclination (keep the recorded length - always works,
        // also fixes a reversed sign), adjust the length (keep the recorded
        // inclination - only when its sign points the same way as the depth change,
        // else withheld), or split between them.
        const dz = shot.depth_out - shot.depth_in;
        const implied = impliedInclination(shot);
        const recorded = recordedPitch(shot);
        const lengthFromIncl = lengthFromInclination(dz, recorded);
        const midPitch = recorded === null ? null : (recorded + implied) / 2;
        const splitLength = lengthFromInclination(dz, midPitch);
        return [
          // Default (first): trust the recorded length, adjust the inclination.
          { id: 'incl-from-depth', label: t('detailPanel.warnings.resolveOptions.adjustInclination'), detail: `${formatFixed(implied, undefined, precision.inclination)}°`, disabled: geometryDisabled },
          ...(lengthFromIncl !== null && recorded !== null && lengthFixableFromInclination(dz, recorded)
            ? [{ id: 'length-from-incl', label: t('detailPanel.warnings.resolveOptions.adjustLength'), detail: len(lengthFromIncl), selectedNote: t('detailPanel.warnings.fromInclinationNote', { value: formatFixed(recorded, undefined, precision.inclination) }), disabled: geometryDisabled }]
            : []),
          ...(splitLength !== null && midPitch !== null && lengthFixableFromInclination(dz, midPitch)
            ? [{ id: 'split-incl-length', label: t('detailPanel.warnings.resolveOptions.splitInclLength'), detail: `${formatFixed(midPitch, undefined, precision.inclination)}° · ${len(splitLength)}`, disabled: geometryDisabled }]
            : []),
          { id: 'manual', label: t('detailPanel.warnings.resolveOptions.manualLength'), manual: 'length', disabled: geometryDisabled },
          ...maybeIgnore,
        ];
      }
      case 'multiple_datums_per_merge_component': {
        // Over-anchoring is a structural error, not a suppressible nuisance
        // (issue #57): no Ignore. The component is anchored by N zero datums; the
        // fix is to keep exactly one and remove the other N-1. Offer one "keep"
        // option per datum, plus a jump to the representative station.
        const refs = warning.datumRefs ?? [];
        const keepOptions: ResolutionOption[] = refs.map((r, i) => ({
          id: `keep-datum-${i}`,
          label: t('detailPanel.warnings.resolveOptions.keepDatum', { station: `${r.section_id}:${r.station_index}` }),
          disabled: editingBlocked || refs.length < 2,
        }));
        return [
          ...keepOptions,
          { id: 'go-to-station', label: t('detailPanel.warnings.resolveOptions.goToStation'), navigate: true },
        ];
      }
      case 'CLOSURE_ACROSS_ONE_SHOT':
        // A loop closure that ties the two ends of this shot together (#737):
        // the Closures Computed View skips it, so it closes nothing. An error -
        // no Ignore. Remove it, or go to its station to see it in Details.
        return [
          {
            id: 'remove-closure',
            label: t('detailPanel.warnings.resolveOptions.removeClosure'),
            disabled: editingBlocked || warning.closureId === undefined,
            danger: true,
          },
          ...(closureStation
            ? [{
                id: 'go-to-station',
                label: t('detailPanel.warnings.resolveOptions.goToClosureStation', {
                  station: `${closureStation.section_id}:${closureStation.station_index}`,
                }),
                navigate: true,
              }]
            : []),
        ];
      case 'CLOSURE_ADJUSTMENT_FAILED':
        // A part of the survey the closure solve could not adjust (#737). The
        // fix is in that part's closures or datums, so offer the way there. It
        // is advisory (the measured data is fine; only the derived view is
        // degraded), so it can be ignored like any other warning.
        return [
          { id: 'go-to-station', label: t('detailPanel.warnings.resolveOptions.goToStation'), navigate: true },
          ...maybeIgnore,
        ];
      default:
        return maybeIgnore;
    }
  }, [warning.code, warning.severity, warning.datumStationIndex, warning.datumRefs, warning.closureId, closureStation, shot, geometryDisabled, editingBlocked, projectSaved, t, precision]);

  // Default selection: prefer "Average" (the safe azimuth fix) when offered, and
  // for an inclination in/out warning `pitchDeviationDefault` (average, unless
  // one leg is a stored 0°), else the first enabled option (falls back to Ignore
  // when every fix is locked, e.g. a frozen shot in the closure-adjusted view).
  useEffect(() => {
    const enabled = options.filter((o) => !o.disabled);
    const wanted = shot && warning.code === 'PITCH_DEVIATION' ? pitchDeviationDefault(shot) : 'average';
    const preferred = enabled.find((o) => o.id === wanted) ?? enabled[0] ?? options[options.length - 1];
    setSelected(preferred?.id ?? '');
    if (shot) {
      const rp = recordedPitch(shot);
      setManual({
        azimuth: '',
        azimuthIn: shot.azimuth_in.toString(),
        azimuthOut: shot.azimuth_out.toString(),
        length: shot.length > 0 ? shot.length.toString() : '',
        depthIn: shot.depth_in.toString(),
        depthOut: shot.depth_out.toString(),
        pitch: rp !== null ? rp.toString() : '',
        left: shot.left?.toString() ?? '',
        right: shot.right?.toString() ?? '',
        up: shot.up?.toString() ?? '',
        down: shot.down?.toString() ?? '',
      });
    }
    // Re-seed only when the warning being resolved changes (resolve-all
    // stepping). Deliberately NOT keyed on `options`/`shot` object identity: a
    // background project refresh rebuilds those references and would otherwise
    // wipe the user's in-progress manual edit and reset their selection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [warning.sectionId, warning.shotId, warning.code, warning.closureId]);

  // Strict parse of a manual field (NaN when empty/non-numeric/garbage). Used
  // only by the proposedCreatesError advisory; the proposal math lives in
  // computeProposedRaw (which also uses strictNum), so the advisory and the apply
  // agree on what counts as a valid number (issue #57, finding #11).
  const num = (key: string): number => strictNum(manual[key]);

  // Live on-map preview of the proposed geometry as the user tries options
  // (issue #57). Anchored to the real solved endpoint so it shows just the
  // change; returns null (no ghost) for options that don't move stations, or
  // when our planar model can't reproduce the current endpoint (e.g. a
  // rotated/offset section) - better to show nothing than a misleading ghost.
  const preview = useMemo<GeometryPreview | null>(() => {
    if (!shot) return null;
    const opt = options.find((o) => o.id === selected);
    if (!opt || opt.ignore || opt.navigate || opt.id.startsWith('keep-datum') || opt.id === 'remove-closure') return null;
    if (opt.id === 'lrud-abs' || opt.id === 'lrud-clear' || opt.manual === 'lrud') return null;

    const stationAt = (stationIndex: number) =>
      activeStations.find(
        (s) => s.station_ref.section_id === warning.sectionId && s.station_ref.station_index === stationIndex,
      )?.coordinates;
    const start = stationAt(warning.shotIndex);
    const end = stationAt(warning.shotIndex + 1);
    if (!start || !end) return null;

    const depthChange = shot.depth_out - shot.depth_in;
    const currentHoriz = horizontalLength(shot.length, depthChange);
    const currentAz = averageAzimuth(shot.azimuth_in, shot.azimuth_out);

    // Single source of truth: the on-map ghost and the applied patch both derive
    // from computeProposedRaw, so they cannot diverge.
    const raw = computeProposedRaw(opt.id, opt.manual, shot, manual, negativeLrud);
    if (!raw || raw.invalid) return null;

    // Pitch-only proposals (incl-from-depth) change neither azimuth, length, nor
    // depth - no ghost (preserves the old switch's no-case-for-incl-from-depth).
    if (
      raw.azimuthIn === undefined &&
      raw.azimuthOut === undefined &&
      raw.length === undefined &&
      raw.depthIn === undefined &&
      raw.depthOut === undefined
    ) {
      return null;
    }

    // Suppress the clamped, misleading ghost when the proposed length/depth makes
    // |dz| exceed the slope length (the geometry the backend will reject). The
    // advisory notice handles user feedback; here we simply draw no ghost.
    if (raw.length !== undefined && exceedsSlopeLength(raw.length, shot.depth_in, shot.depth_out)) {
      return null;
    }
    if (
      raw.depthIn !== undefined &&
      raw.depthOut !== undefined &&
      exceedsSlopeLength(shot.length, raw.depthIn, raw.depthOut)
    ) {
      return null;
    }

    let propAz = currentAz;
    if (raw.azimuthIn !== undefined || raw.azimuthOut !== undefined) {
      // Both legs are always carried for azimuth fixes; for use-in/use-out they
      // are set equal so averageAzimuth(x, x) === x matches the old preview az.
      propAz = averageAzimuth(raw.azimuthIn ?? shot.azimuth_in, raw.azimuthOut ?? shot.azimuth_out);
    }
    let propHoriz = currentHoriz;
    if (raw.length !== undefined) {
      propHoriz = horizontalLength(raw.length, depthChange);
    } else if (raw.depthIn !== undefined && raw.depthOut !== undefined) {
      // Depth fix: recompute horizontal from the CURRENT slope length and the
      // new depth delta (matches the old 'manual depth' preview branch).
      propHoriz = horizontalLength(shot.length, raw.depthOut - raw.depthIn);
    }

    const ghostEnd = computeGhostEnd(start, end, currentAz, currentHoriz, propAz, propHoriz);
    if (!ghostEnd) return null;

    return {
      shots: [{ from: { x: start.x, y: start.y }, to: ghostEnd }],
      stations: [ghostEnd],
      color: PREVIEW_COLOR,
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, manual, shot, options, activeStations, negativeLrud, warning.sectionId, warning.shotIndex]);

  // Push the preview to the map; clear it when leaving the resolution view.
  useEffect(() => {
    setGeometryPreview(preview);
  }, [preview, setGeometryPreview]);
  useEffect(() => () => setGeometryPreview(null), [setGeometryPreview]);

  // True when the selected manual length/depth values make the shot shorter than
  // its depth change (|dz| > length): applying is allowed but will raise a
  // LENGTH_BELOW_DEPTH_CHANGE error on revalidation (issue #57). Drives an
  // advisory notice only; the Apply button stays enabled.
  const proposedCreatesError = useMemo<boolean>(() => {
    if (!shot) return false;
    const opt = options.find((o) => o.id === selected);
    if (!opt || opt.id !== 'manual') return false;
    if (opt.manual === 'depth') {
      return exceedsSlopeLength(shot.length, num('depthIn'), num('depthOut'));
    }
    if (opt.manual === 'length') {
      return exceedsSlopeLength(num('length'), shot.depth_in, shot.depth_out);
    }
    return false;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, manual, shot, options]);

  /**
   * Apply the selected option.
   * - 'resolved': a suppression/fix was applied (advance + count).
   * - 'navigated': the user jumped elsewhere; do not count as resolved.
   * - 'failed': nothing applied (validation/command error); stay put.
   */
  const apply = async (): Promise<'resolved' | 'navigated' | 'failed'> => {
    if (!shot || !section) return 'failed';
    const opt = options.find((o) => o.id === selected);
    if (!opt) return 'failed';

    if (opt.ignore) {
      // The backend can refuse (e.g. the shot no longer exists, or the code is
      // an error): only count it resolved if it actually persisted (#57 #1).
      const ok = await resolveWarning(warning.sectionId, warning.shotId, warning.code);
      if (!ok) {
        showToast(t('detailPanel.warnings.resolveFailed'), TOAST_KEYS.WARNING_RESOLVE, 'error');
        return 'failed';
      }
      return 'resolved';
    }

    if (opt.navigate) {
      // The over-anchoring datum, the closure's station, or (a part the solve
      // could not adjust) the start of the shot the warning is on.
      const ref =
        warning.datumStationIndex !== undefined
          ? { section_id: warning.sectionId, station_index: warning.datumStationIndex }
          : warning.code === 'CLOSURE_ACROSS_ONE_SHOT'
            ? closureStation
            : warning.code === 'CLOSURE_ADJUSTMENT_FAILED'
              ? { section_id: warning.sectionId, station_index: warning.shotIndex }
              : undefined;
      if (ref) {
        setSource('detail-panel');
        setSelection({ type: 'station', stationRef: ref });
        triggerCenterOnStation(ref);
        setDetailPanelTab('details');
      }
      return 'navigated'; // jumped to the station; nothing was suppressed or fixed
    }

    if (opt.id === 'remove-closure') {
      if (warning.closureId === undefined) return 'failed';
      // Undoable like any closure removal; failure surfacing is owned by
      // executeCommand's catch-all toast.
      const result = await removeLoopClosure(BigInt(warning.closureId));
      return result.success ? 'resolved' : 'failed';
    }

    if (opt.id.startsWith('keep-datum-')) {
      // Keep the chosen datum, remove every other anchor in the component so the
      // group is anchored by exactly one (issue #57, finding #4). Datums are keyed
      // by station_ref, so removing one never renumbers the others.
      const refs = warning.datumRefs ?? [];
      const keepIdx = Number(opt.id.slice('keep-datum-'.length));
      const toRemove = refs.filter((_, i) => i !== keepIdx);
      if (toRemove.length === 0) return 'failed';
      for (const ref of toRemove) {
        // Failure surfacing is owned by executeCommand's catch-all toast
        // (docs/notifications.md: one event, one surface).
        const result = await removeZeroDatum({ section_id: ref.section_id, station_index: ref.station_index });
        if (!result.success) return 'failed';
      }
      return 'resolved';
    }

    if (opt.id === 'delete-shot') {
      const result = await deleteShot(warning.sectionId, warning.shotId);
      if (result.success) {
        // The shot no longer exists; the resolve flow had it selected, so move
        // the selection to its parent section - otherwise Details renders
        // "Selection not found" for the now-dangling shot selection.
        setSource('detail-panel');
        setSelection({ type: 'section', sectionId: warning.sectionId });
      }
      // (failure surfacing owned by executeCommand's catch-all toast)
      return result.success ? 'resolved' : 'failed';
    }

    // Data fixes via the undoable update_shot command. The proposed RAW fields
    // come from the single source shared with the preview (computeProposedRaw),
    // so the ghost and the committed change cannot diverge.
    const raw = computeProposedRaw(opt.id, opt.manual, shot, manual, negativeLrud);
    if (!raw) return 'failed';
    if (raw.invalid) {
      const key =
        raw.invalid === 'azimuth'
          ? 'detailPanel.warnings.resolveInvalidAzimuth'
          : raw.invalid === 'length'
            ? 'detailPanel.warnings.resolveInvalidLength'
            : raw.invalid === 'pitch'
              ? 'detailPanel.warnings.resolveInvalidPitch'
              : 'detailPanel.warnings.resolveInvalid';
      showToast(t(key), TOAST_KEYS.WARNING_RESOLVE, 'warning');
      return 'failed';
    }

    const patch: Partial<ShotUpdates> = {};
    // use-in/use-out only ever changed the OPPOSITE leg; emit just that one leg
    // (computeProposedRaw carries both equal legs for the preview's average).
    if (opt.id === 'use-in') {
      patch.azimuth_out = shot.azimuth_in;
    } else if (opt.id === 'use-out') {
      patch.azimuth_in = shot.azimuth_out;
    } else {
      if (raw.azimuthIn !== undefined) patch.azimuth_in = raw.azimuthIn;
      if (raw.azimuthOut !== undefined) patch.azimuth_out = raw.azimuthOut;
    }
    if (raw.pitchIn !== undefined) patch.pitch_in = raw.pitchIn;
    if (raw.pitchOut !== undefined) patch.pitch_out = raw.pitchOut;
    if (raw.length !== undefined) patch.length = raw.length;
    if (raw.depthIn !== undefined) patch.depth_in = raw.depthIn;
    if (raw.depthOut !== undefined) patch.depth_out = raw.depthOut;
    if (raw.lrud) {
      for (const f of LRUD_FIELDS) {
        const v = raw.lrud[f];
        if (v !== undefined) patch[f] = v;
      }
    }

    // Guard against a no-op (e.g. lrud-abs/clear when nothing is negative after a
    // refresh): an empty patch would still push an undo entry and report success.
    if (Object.keys(patch).length === 0) {
      showToast(t('detailPanel.warnings.resolveInvalid'), TOAST_KEYS.WARNING_RESOLVE, 'warning');
      return 'failed';
    }

    // Failure surfacing is owned by executeCommand's catch-all toast.
    const result = await updateShot(warning.sectionId, warning.shotId, patch);
    return result.success ? 'resolved' : 'failed';
  };

  const handleApply = async () => {
    setBusy(true);
    try {
      const result = await apply();
      if (result === 'resolved') onResolved();
      else if (result === 'navigated') onExit();
      // 'failed' → stay on this warning (a toast explained why).
    } finally {
      setBusy(false);
    }
  };

  const isResolveAll = position !== undefined;
  const selectedOption = options.find((o) => o.id === selected);

  return (
    <div className="p-2" data-testid="warning-resolution">
      {/* Header: back · zoom button + (n of N) */}
      <div className="flex items-center justify-between mb-2 px-1">
        <button
          onClick={onExit}
          className="flex items-center gap-1 text-sm text-accent hover:underline"
          data-testid="warning-resolution-back"
        >
          <ChevronLeftIcon size={16} />
          {isResolveAll ? t('detailPanel.warnings.resolveExit') : t('detailPanel.warnings.resolveBack')}
        </button>
        <div className="flex items-center gap-2">
          {isResolveAll && position && (
            <span className="text-xs text-text-secondary" data-testid="warning-resolution-progress">
              {t('detailPanel.warnings.resolvePosition', { index: position.index + 1, total: position.total })}
            </span>
          )}
          {/* Each click zooms one step closer onto the problematic shot. */}
          <button
            type="button"
            onClick={() => triggerZoomToShot(warning.sectionId, warning.shotId)}
            className="p-1 text-text-secondary hover:text-text-primary rounded hover:bg-background-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset"
            title={t('detailPanel.warnings.zoomToShot')}
            aria-label={t('detailPanel.warnings.zoomToShot')}
            data-testid="warning-resolution-zoom"
          >
            <CenterMapIcon size={16} />
          </button>
        </div>
      </div>

      {/* The warning + current values */}
      <div className="px-1 mb-3">
        <div className="text-sm text-text-primary">{warning.message}</div>
        {shot && (warning.code === 'AZIMUTH_DEVIATION' || warning.code === 'INVALID_AZIMUTH') && (
          <div className="text-xs text-text-secondary mt-1">
            {t('detailPanel.labels.azimuthIn')} {az(shot.azimuth_in)} · {t('detailPanel.labels.azimuthOut')} {az(shot.azimuth_out)}
          </div>
        )}
        {shot && warning.code === 'PITCH_DEVIATION' && (
          <div className="text-xs text-text-secondary mt-1">
            {t('detailPanel.labels.pitchIn')} {formatFixed(shot.pitch_in ?? 0, undefined, precision.inclination)}° · {t('detailPanel.labels.pitchOut')} {formatFixed(shot.pitch_out ?? 0, undefined, precision.inclination)}°
          </div>
        )}
        {shot && warning.code === 'LARGE_DEPTH_CHANGE' && (
          <div className="text-xs text-text-secondary mt-1">
            {t('detailPanel.labels.depthIn')} {dep(shot.depth_in)} · {t('detailPanel.labels.depthOut')} {dep(shot.depth_out)}
          </div>
        )}
        {shot && (warning.code === 'ZERO_LENGTH' || warning.code === 'NEGATIVE_LENGTH' || warning.code === 'SHORT_SHOT' || warning.code === 'LONG_SHOT') && (
          <div className="text-xs text-text-secondary mt-1">{t('detailPanel.labels.length')} {len(shot.length)}</div>
        )}
      </div>

      {!shot ? (
        <div className="px-1 text-sm text-text-secondary">{t('detailPanel.selectionNotFound')}</div>
      ) : (
        <div className="border-t border-border pt-2 space-y-1" role="radiogroup" aria-label={t('detailPanel.warnings.resolveTitle')}>
          {options.map((opt) => (
            <div key={opt.id}>
              <label
                className={`flex items-center gap-2 px-1 py-1 rounded text-sm cursor-pointer hover:bg-background-50 ${opt.disabled ? 'opacity-50 cursor-not-allowed' : ''}`}
              >
                <input
                  type="radio"
                  name="warning-resolution-option"
                  value={opt.id}
                  checked={selected === opt.id}
                  disabled={opt.disabled}
                  onChange={() => setSelected(opt.id)}
                  data-testid={`warning-resolution-option-${opt.id}`}
                />
                <span className={opt.danger ? 'text-error font-medium' : undefined}>
                  {opt.label}
                  {opt.detail && <span className="text-text-secondary"> ({opt.detail})</span>}
                </span>
              </label>
              {/* Inline manual editors, shown when their option is selected. */}
              {selected === opt.id && opt.manual && (
                <div className="pl-6 pr-1 pb-1">
                  {opt.manual === 'azimuth' && (
                    <ManualField value={manual.azimuth ?? ''} onChange={(v) => setManual((m) => ({ ...m, azimuth: v }))} unit="°" placeholder={t('detailPanel.warnings.azimuthLabel')} testId={`manual-${opt.id}-azimuth`} />
                  )}
                  {opt.manual === 'azimuthInOut' && (
                    <div className="grid grid-cols-2 gap-2">
                      <ManualField value={manual.azimuthIn ?? ''} onChange={(v) => setManual((m) => ({ ...m, azimuthIn: v }))} unit="°" placeholder={t('detailPanel.labels.azimuthIn')} testId={`manual-${opt.id}-azimuth-in`} />
                      <ManualField value={manual.azimuthOut ?? ''} onChange={(v) => setManual((m) => ({ ...m, azimuthOut: v }))} unit="°" placeholder={t('detailPanel.labels.azimuthOut')} testId={`manual-${opt.id}-azimuth-out`} />
                    </div>
                  )}
                  {opt.manual === 'length' && (
                    <ManualField value={manual.length ?? ''} onChange={(v) => setManual((m) => ({ ...m, length: v }))} unit={t('common.units.meters')} placeholder={t('detailPanel.labels.length')} testId={`manual-${opt.id}-length`} />
                  )}
                  {opt.manual === 'pitch' && (
                    <ManualField
                      value={manual.pitch ?? ''}
                      onChange={(v) => setManual((m) => ({ ...m, pitch: v }))}
                      unit="°"
                      placeholder={t('detailPanel.labels.pitch')}
                      testId={`manual-${opt.id}-pitch`}
                      invalid={(() => {
                        const v = strictNum(manual.pitch);
                        return !Number.isNaN(v) && (v < -90 || v > 90);
                      })()}
                    />
                  )}
                  {opt.manual === 'depth' && (
                    <div className="grid grid-cols-2 gap-2">
                      <ManualField value={manual.depthIn ?? ''} onChange={(v) => setManual((m) => ({ ...m, depthIn: v }))} unit={t('common.units.meters')} placeholder={t('detailPanel.labels.depthIn')} testId={`manual-${opt.id}-depth-in`} />
                      <ManualField value={manual.depthOut ?? ''} onChange={(v) => setManual((m) => ({ ...m, depthOut: v }))} unit={t('common.units.meters')} placeholder={t('detailPanel.labels.depthOut')} testId={`manual-${opt.id}-depth-out`} />
                    </div>
                  )}
                  {opt.manual === 'lrud' && (
                    <div className="grid grid-cols-4 gap-2">
                      {LRUD_FIELDS.map((f) => (
                        <ManualField key={f} value={manual[f] ?? ''} onChange={(v) => setManual((m) => ({ ...m, [f]: v }))} placeholder={f[0]!.toUpperCase()} testId={`manual-${f}`} />
                      ))}
                    </div>
                  )}
                </div>
              )}
              {/* Advisory: the proposed length/depth makes the shot shorter than
                  its depth change, which revalidation will flag. Applying is
                  still allowed (the Apply button stays enabled) - we just warn. */}
              {selected === opt.id && (opt.manual === 'depth' || opt.manual === 'length') && proposedCreatesError && (
                <div className="pl-6 pr-1 pb-1 text-xs text-warning" data-testid={`warning-resolution-impossible-${opt.id}`}>
                  {t('detailPanel.warnings.proposedImpossibleGeometry')}
                </div>
              )}
              {/* Extra context (e.g. the inclination used), shown only while
                  this option is selected - mirrors the inline manual editors. */}
              {selected === opt.id && opt.selectedNote && (
                <div className="pl-6 pr-1 pb-1 text-xs text-text-secondary" data-testid={`warning-resolution-note-${opt.id}`}>
                  {opt.selectedNote}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Footer actions */}
      <div className="flex items-center justify-between gap-2 pt-3 mt-2 border-t border-border">
        {isResolveAll ? (
          <Button size="sm" onClick={onSkip} disabled={busy} data-testid="warning-resolution-skip">
            {t('detailPanel.warnings.resolveSkip')}
          </Button>
        ) : (
          <span />
        )}
        <Button
          size="sm"
          variant="primary"
          onClick={() => void handleApply()}
          disabled={busy || !shot || !selectedOption || (selectedOption.disabled ?? false)}
          data-testid="warning-resolution-apply"
        >
          {isResolveAll ? t('detailPanel.warnings.resolveApplyNext') : t('detailPanel.warnings.resolveApply')}
        </Button>
      </div>
    </div>
  );
}

function ManualField({
  value,
  onChange,
  unit,
  placeholder,
  testId,
  invalid = false,
}: {
  value: string;
  onChange: (v: string) => void;
  unit?: string;
  placeholder?: string;
  testId?: string;
  /** Highlight the field red (e.g. an out-of-range pitch). */
  invalid?: boolean;
}) {
  return (
    <div className="flex items-center gap-1">
      <input
        type="number"
        inputMode="decimal"
        step="0.1"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        className={`w-full ${invalid ? 'ring-2 ring-error' : ''}`}
        data-testid={testId}
      />
      {unit && <span className="text-xs text-text-secondary">{unit}</span>}
    </div>
  );
}
