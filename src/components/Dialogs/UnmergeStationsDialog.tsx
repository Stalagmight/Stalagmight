import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage, invokeIpc } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import type { DetachLinePreview, DetachLineResult } from '../../types/ipc.generated';
import type { ArchivedStation } from '../../types/ipc/ArchivedStation';
import type { NameCollision } from '../../types/ipc/NameCollision';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { listNames } from '../../utils/listNames';
import { EntityBlob } from '../common/EntityBlob';
import { Prose } from '../common/Prose';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { displayStationName } from '../../utils/formatters';
import {
  answeredName,
  CollisionRows,
  HiddenComments,
  HomeIdentityPill,
  renamesFromAnswers,
  renamesProblem,
  recordIdentity,
  StickyDialogFooter,
  useCollisionAnswers,
} from './keptStationParts';
import { useKeptProse, type KeptProse } from './keptStationProse';

/** A station's name as a detach shows it: after the chosen answers. */
type NameOf = (id: number, fallback: string) => string;

/** The record's own name before any letter the dialog's answers give: a
 *  collision names it as it is, else the preview's name. */
function ownNameOf(id: number, fallback: string, collisions: readonly NameCollision[] | undefined): string {
  for (const collision of collisions ?? []) {
    for (const suggestion of [collision.letter, collision.alternative]) {
      if (suggestion.station.id === id) return suggestion.station.name;
    }
  }
  return fallback;
}

/**
 * Whether a station on `lines` already carries a letter after `name`
 * (`0a`, `0b`): a name that took a letter earlier because of the station
 * that comes back, which the dialog's note is about (RS-FX6, P3-13). Without
 * one, the note says nothing true and is left out.
 */
export function letterTakenEarlier(
  name: string,
  lines: readonly number[],
  lineStationNames: (lineId: number) => readonly string[],
): boolean {
  if (name === '') return false;
  return lines.some((line) =>
    lineStationNames(line).some(
      (other) => other.length > name.length && other.startsWith(name) && /^[a-z]+$/.test(other.slice(name.length)),
    ),
  );
}

/** A station that moves inside the one that comes back: shown directly
 *  inside it, with the stations still archived inside `record` (any depth). */
export interface InsideRow {
  record: ArchivedStation;
  inner: ArchivedStation[];
}

/** Every station the station that comes back (`id`) keeps inside it
 *  (RS-FX4), each of which a second Detach gives back, as the backend moves
 *  them: `id`'s whole subtree, and, when the detached line keeps the
 *  junction (RS 4.2 case 2), every record but the junction's own history,
 *  since they go with the lines that come off (RS-FX8, N7). One row per
 *  station directly inside the one that comes back (a record directly under
 *  `id` comes up to it), naming the stations nested inside it (RS-FX9: the
 *  nested ones move too, and picking North / 9 left Side / 7, archived in
 *  Side / 0, unsaid). History records are never named: nothing brings them
 *  back. */
export function recordsInside(
  records: readonly ArchivedStation[] | undefined,
  id: number,
  kept = false,
): InsideRow[] {
  const list = records ?? [];
  const byId = new Map(list.map((r) => [r.id, r] as const));
  // The record each one is under once `id` comes back: `id`'s own records
  // come up to it (undefined), the others keep their owners.
  const ownerOf = (r: ArchivedStation): number | undefined => (r.into === id ? undefined : r.into);
  // Whether `r` is inside `id`'s subtree (bounded: a cycle is no member).
  const underId = (r: ArchivedStation): boolean => {
    let at: ArchivedStation | undefined = r;
    for (let steps = 0; at && steps <= list.length; steps++) {
      if (at.into === id) return true;
      at = at.into === undefined ? undefined : byId.get(at.into);
    }
    return false;
  };
  const moves = (r: ArchivedStation): boolean =>
    r.id !== id && (kept ? !(r.into === undefined && r.lines.length === 0) : underId(r));
  const moved = list.filter(moves);
  // The top of each moved record's chain among the moved, bounded.
  const topOf = (r: ArchivedStation): ArchivedStation | undefined => {
    let at = r;
    for (let steps = 0; steps <= list.length; steps++) {
      const owner = ownerOf(at);
      if (owner === undefined) return at;
      const next = byId.get(owner);
      if (!next || !moves(next)) return at;
      at = next;
    }
    return undefined;
  };
  const rows: InsideRow[] = moved
    .filter((r) => ownerOf(r) === undefined && r.lines.length > 0)
    .map((record) => ({ record, inner: [] }));
  const rowOf = new Map(rows.map((row) => [row.record.id, row] as const));
  for (const r of moved) {
    if (ownerOf(r) === undefined || r.lines.length === 0) continue;
    const top = topOf(r);
    if (top) rowOf.get(top.id)?.inner.push(r);
  }
  return rows;
}

function stationRefKey(ref: StationRef): string {
  return `${ref.line_id}:${ref.station_index}`;
}

function sameStationRef(a: StationRef, b: StationRef): boolean {
  return a.line_id === b.line_id && a.station_index === b.station_index;
}

/** Whether detaching `ref`'s line gives it back the station it had before a
 *  merge, while the other lines keep the junction: a line that is NOT the
 *  junction's owner (the owner shows it by its own name, so detaching it
 *  either mints a new Station ID or, `kept`, keeps the station on it). */
function comesOffAlone(ref: StationRef, previews: ReadonlyMap<number, DetachLinePreview>): boolean {
  const preview = previews.get(ref.line_id);
  return preview?.restored === true && !preview.kept;
}

/**
 * The slot to preselect (demo findings I-7, V-1): the likely intent is a line
 * that is NOT the junction's owner, which comes off alone with its own station
 * back ({@link comesOffAlone}). So the clicked one (`slots[0]`) when it is such
 * a line, else the first other line that is, else the clicked one. A clicked
 * owner that keeps the junction (`kept`: every other line comes off with it,
 * on a hub together) is no longer preselected when another line can come off
 * alone. While the previews are still out, or when none says, the clicked one.
 * A two-line junction whose result does not depend on the choice offers none
 * ({@link sameDetachResult}).
 */
export function defaultDetachSlot(
  slots: StationRef[],
  previews: ReadonlyMap<number, DetachLinePreview>,
): StationRef | undefined {
  const [clicked, ...others] = slots;
  if (!clicked || !previews.has(clicked.line_id) || comesOffAlone(clicked, previews)) return clicked;
  return others.find((ref) => comesOffAlone(ref, previews)) ?? clicked;
}

/** The preview's per-line results in the order the dialog lists them: the
 *  lines that come off first, then those that stay, each in slot order. */
export function orderedResults(preview: DetachLinePreview): DetachLineResult[] {
  return [...preview.lines.filter((r) => r.comes_off), ...preview.lines.filter((r) => !r.comes_off)];
}

/** What a detach does when it does not depend on the line chosen. */
export interface SameDetachResult {
  /** The slot of the line that comes off: it gets back `preview`'s station. */
  off: StationRef;
  /** The slot of the line that keeps the junction. */
  keeper: StationRef;
  preview: DetachLinePreview;
}

/**
 * Demo finding N-2: on a junction of two lines where one line keeps the
 * station when detached (`kept`: an archived station names every other line) and
 * the other gets back exactly the station the keeper's preview names, both
 * choices give the same result: the keeper stays on the junction and the
 * other line comes off with its own station back. Then there is nothing to
 * choose, and two radio rows that read as different actions mislead ("Main
 * Line (the line you clicked)" preselected, while North Passage is the line
 * that comes off). `null` while a preview is out, or when the choice changes
 * the result.
 */
export function sameDetachResult(
  slots: StationRef[],
  previews: ReadonlyMap<number, DetachLinePreview>,
): SameDetachResult | null {
  const [a, b] = slots;
  if (slots.length !== 2 || !a || !b || a.line_id === b.line_id) return null;
  const either = (keeper: StationRef, off: StationRef): SameDetachResult | null => {
    const kept = previews.get(keeper.line_id);
    const back = previews.get(off.line_id);
    if (!kept?.kept || !back || back.kept || !back.restored) return null;
    if (back.name !== kept.name || back.station_id !== kept.station_id) return null;
    return { off, keeper, preview: back };
  };
  return either(a, b) ?? either(b, a);
}

/**
 * Detach a line from a station several lines share (plan 5.2; the dialog kept
 * its `unmerge-stations` name for telemetry). Demo finding V-1: the choice is
 * worded as what comes off (the row of a line that keeps the junction reads
 * "Far Room and Upstream, together (Main Line keeps this station)", so the
 * history, which names the lines that come off, names what the user picked),
 * and under it the dialog lists what every line ends up with, name and
 * Station ID, from the backend's per-line preview: lines that come off
 * together stay joined on ONE station, so a line among them whose own station
 * is another archived station keeps that one archived inside it, and the
 * dialog says so.
 *
 * On the station graph a merge is ONE station that two or more lines pass. The
 * user picks which line leaves it: that line gets its own station at the same
 * place (no comment), and the datums and closures attached through it go
 * along; the other lines stay joined. A line an archived station names gets
 * that station back whole (RS 4.2: Station ID, name, comment, with the lines
 * and the stations archived inside it); a native line beside another native
 * line gets a new station with the junction's name. When archived stations
 * name every other line, the chosen line keeps the junction and the others
 * get back one archived station, the one the picker chooses (I-1). The dialog says which, from the
 * backend's `preview_detach_line` (review finding UX-4), asked for every line
 * when the dialog opens. The clicked slot's line is listed first. It is
 * preselected unless it is the junction's owner (detaching it would give it a
 * NEW Station ID, or keep the junction on it while the others come off) and
 * another line would come off alone with its own station back: that line is
 * then the likely intent and preselected (demo findings I-7, V-1). When the
 * chosen line gets its station back, the dialog says which comment comes
 * back with it (an archived station keeps its own). Its own line detaches with
 * `unmerge_stations(slot)`; another line with `unmerge_station_pair(slot,
 * other)`, which detaches the PARTNER's line. When the choice does not change
 * the result ({@link sameDetachResult}: two lines, one keeps the junction and
 * the other gets its own station back) the dialog offers no choice: one
 * sentence says which line comes off and what it gets back, and Detach
 * detaches that line, so the history names it too (demo finding N-2). For a
 * two-line junction the body waits for both previews, so the list never
 * flashes before the sentence replaces it.
 */
export default function UnmergeStationsDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore();
  const { stations, unmergeStations, unmergeStationPair } = useProjectStore();
  const labels = useIdentityLabels();
  const prose = useKeptProse();
  const editingBlocked = useEditingBlocked();

  const stationRef = dialogData.stationRef as StationRef | undefined;
  const mergedStationsFromDialog = dialogData.mergedStations as StationRef[] | undefined;
  // The line the user already chose (the "Also on" detach button, the
  // rename prompt's "Detach Side"): preselected, the default never moves it.
  const chosenLine = typeof dialogData.chosenLine === 'number' ? dialogData.chosenLine : undefined;

  /** Every slot of the station: the clicked one first, then the other lines through it. */
  const slots = useMemo(() => {
    if (!stationRef) return [];
    const fromDialog = Array.isArray(mergedStationsFromDialog) ? mergedStationsFromDialog : [];
    const others =
      fromDialog.length > 0
        ? fromDialog
        : stations.find((station) => sameStationRef(station.station_ref, stationRef))?.merged_with ?? [];
    const deduped = new Map<string, StationRef>([[stationRefKey(stationRef), stationRef]]);
    for (const ref of others) deduped.set(stationRefKey(ref), ref);
    return Array.from(deduped.values());
  }, [mergedStationsFromDialog, stationRef, stations]);

  const [selectedKey, setSelectedKey] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // What each line would get, by line id. A failed preview only hides that
  // line's sentence; the detach reports its own refusal.
  const [previews, setPreviews] = useState<ReadonlyMap<number, DetachLinePreview>>(new Map());
  // Every preview has answered (or failed).
  const [previewsSettled, setPreviewsSettled] = useState(false);
  // Once the user picks a line, the default stops following the previews.
  const pickedRef = useRef(false);
  // The record the remaining junction shows (RS 4.2 case 2), when the user
  // picked one; the selected line's preview is asked again with it. The
  // pick's preview is kept apart from the line's default one (`previews`),
  // so clearing the pick shows the default again (review finding ui-1).
  const [restKeep, setRestKeep] = useState<number | undefined>(undefined);
  const [pickPreview, setPickPreview] = useState<{
    line: number;
    restKeep: number;
    preview: DetachLinePreview;
  } | null>(null);
  const [pickFailed, setPickFailed] = useState(false);

  useEffect(() => {
    const chosen = chosenLine === undefined ? undefined : slots.find((ref) => ref.line_id === chosenLine);
    pickedRef.current = chosen !== undefined;
    setPreviews(new Map());
    setPreviewsSettled(false);
    setRestKeep(undefined);
    setPickPreview(null);
    setPickFailed(false);
    setSelectedKey(chosen ? stationRefKey(chosen) : slots.length > 0 ? stationRefKey(slots[0]!) : '');
    if (!stationRef || slots.length === 0) return;
    let live = true;
    void Promise.all(
      slots.map((ref) =>
        invokeIpc<DetachLinePreview>(
          'preview_detach_line',
          { stationRef, lineId: ref.line_id },
          { purpose: 'preview detach line' },
        )
          .then((preview): [number, DetachLinePreview] | null => [ref.line_id, preview])
          .catch(() => null),
      ),
    ).then((answers) => {
      if (!live) return;
      const byLine = new Map(answers.filter((a): a is [number, DetachLinePreview] => a !== null));
      setPreviews(byLine);
      setPreviewsSettled(true);
      const preferred = defaultDetachSlot(slots, byLine);
      if (!pickedRef.current && preferred) setSelectedKey(stationRefKey(preferred));
    });
    return () => {
      live = false;
    };
  }, [slots, stationRef, chosenLine]);

  const selected = useMemo(
    () => slots.find((ref) => stationRefKey(ref) === selectedKey) ?? null,
    [slots, selectedKey],
  );
  const same = useMemo(() => sameDetachResult(slots, previews), [slots, previews]);

  // A remaining-junction pick asks the chosen line's preview again with it.
  const restLine = same ? same.off.line_id : selected?.line_id;
  useEffect(() => {
    if (restKeep === undefined || !stationRef || restLine === undefined) return;
    let live = true;
    Promise.resolve()
      .then(() =>
        invokeIpc<DetachLinePreview>(
          'preview_detach_line',
          { stationRef, lineId: restLine, restKeep },
          { purpose: 'preview detach line' },
        ),
      )
      .then((preview) => {
        if (!live) return;
        if (!preview) throw new Error('no preview');
        setPickPreview({ line: restLine, restKeep, preview });
      })
      .catch(() => {
        if (!live) return;
        // Back to the station chosen for them: the preview on screen.
        setRestKeep(undefined);
        setPickFailed(true);
      });
    return () => {
      live = false;
    };
  }, [restKeep, restLine, stationRef]);
  // The pick's preview has not answered yet: what is on screen is not what
  // Detach would send.
  const pickPending =
    restKeep !== undefined &&
    !(pickPreview !== null && pickPreview.line === restLine && pickPreview.restKeep === restKeep);
  /** The preview on screen for `line`: the pick's once it answered, else the
   *  line's default. */
  const previewOf = (line: number): DetachLinePreview | null =>
    restKeep !== undefined && line === restLine && !pickPending && pickPreview
      ? pickPreview.preview
      : previews.get(line) ?? null;
  const naming = selected ? previewOf(selected.line_id) : null;
  // A two-line junction waits for its previews: they decide whether there is
  // a choice to show at all.
  const waiting = slots.length === 2 && !previewsSettled;
  const target = same ? same.off : selected;
  // The other lines as prose tokens, for the sentences that name them.
  const otherLines = selected
    ? slots.filter((ref) => ref.line_id !== selected.line_id).map((ref) => prose.line(ref.line_id))
    : [];
  /** A row's label: the line (a line pill), or for a line that keeps the
   *  junction the lines that then come off together, as a sentence (V-1)
   *  with its lines as pills (the radio's name stays the plain sentence). */
  const rowLabel = (ref: StationRef, clicked: boolean): ReactNode => {
    const line = labels.lineName(ref.line_id);
    if (previews.get(ref.line_id)?.kept) {
      const others = listNames(
        slots.filter((o) => o.line_id !== ref.line_id).map((o) => prose.line(o.line_id)),
        t,
      );
      const text = t(clicked ? 'dialogs.detachLine.togetherClicked' : 'dialogs.detachLine.together', {
        line: prose.line(ref.line_id),
        others,
      });
      return (
        <span>
          <Prose text={text} />
        </span>
      );
    }
    return <EntityBlob entity="line" lineId={ref.line_id}>{clicked ? t('dialogs.detachLine.thisLine', { line }) : line}</EntityBlob>;
  };

  // The preview of the line that comes off: its collisions are what the
  // dialog asks about.
  const targetPreview = target ? previewOf(target.line_id) : null;
  const [answers, setAnswer] = useCollisionAnswers(targetPreview?.collisions);
  // The names shown follow the chosen answers (RS-FX6, P2-2): the station
  // that comes back keeps its own name when the other one takes the letter.
  const nameOf: NameOf = (id, fallback) => answeredName(id, fallback, targetPreview?.collisions, answers);
  // The junction's records: the ones under the station that comes back stay
  // inside it (RS-FX6, P3-12).
  const junctionRecords = stations.find((station) => stationRef && sameStationRef(station.station_ref, stationRef))?.archived;
  // The note on earlier letters only when one exists (RS-FX6, P3-13).
  const lettersStay =
    targetPreview?.restored === true &&
    letterTakenEarlier(
      ownNameOf(targetPreview.station_id, targetPreview.name, targetPreview.collisions),
      targetPreview.lines.filter((r) => r.station_id === targetPreview.station_id).map((r) => r.line_id),
      labels.lineStationNames,
    );

  // A name the command would refuse is never sent (RS-FX9).
  const renameProblem = targetPreview ? renamesProblem(renamesFromAnswers(targetPreview.collisions, answers)) : null;

  const handleConfirm = async () => {
    if (!stationRef || !target) return;
    if (isEditingBlockedNow()) {
      setError(t('dialogs.keptStations.editingDisabled'));
      return;
    }
    if (renameProblem !== null) return;

    setIsLoading(true);
    setError(null);
    try {
      // The dialog's answers (RS 4.2): every collision answered, and the
      // record the remaining junction shows when the user picked one.
      const answered = targetPreview
        ? {
            renames: renamesFromAnswers(targetPreview.collisions, answers),
            ...(restKeep !== undefined ? { restKeep } : {}),
          }
        : undefined;
      const own = sameStationRef(target, stationRef);
      const result = answered
        ? own
          ? await unmergeStations(stationRef, answered)
          : await unmergeStationPair(stationRef, target, answered)
        : own
          ? await unmergeStations(stationRef)
          : await unmergeStationPair(stationRef, target);
      if (!result.success) {
        setError(result.error ?? t('dialogs.detachLine.failed'));
        return;
      }
      trackDialogComplete('unmerge-stations');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.detachLine.failed')));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationRef) {
    return (
      <DialogWrapper title={t('dialogs.detachLine.title')} onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
        <div className="space-y-4">
          <p className="text-error text-sm">{t('dialogs.detachLine.notFound')}</p>
          <div className="flex justify-end">
            <Button onClick={closeDialog} data-testid="unmerge-stations-close">{t('dialogs.detachLine.close')}</Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title={t('dialogs.detachLine.title')} onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        {same ? (
          <SameResultBody same={same} nameOf={nameOf} />
        ) : (
          <>
            <p className="text-sm text-text-secondary" data-testid="unmerge-stations-lead">
              <Prose
                text={t(waiting ? 'dialogs.detachLine.leadWaiting' : 'dialogs.detachLine.lead', {
                  station: prose.station(stationRef),
                  count: slots.length,
                })}
              />
            </p>

            {!waiting && (
              <div className="space-y-2 max-h-56 overflow-y-auto border border-border rounded p-2">
                {slots.map((ref, i) => {
                  const key = stationRefKey(ref);
                  return (
                    <label key={key} className="flex items-center gap-2 text-sm p-1 rounded hover:bg-background-50">
                      <input
                        type="radio"
                        name="unmerge-station"
                        value={key}
                        checked={selectedKey === key}
                        onChange={() => {
                          pickedRef.current = true;
                          setSelectedKey(key);
                          // A remaining-junction pick belongs to the line it
                          // was made for.
                          setRestKeep(undefined);
                          setPickFailed(false);
                        }}
                        data-testid={`unmerge-station-option-${ref.line_id}-${ref.station_index}`}
                      />
                      {rowLabel(ref, i === 0)}
                    </label>
                  );
                })}
              </div>
            )}

            {naming && selected && (
              <p className="text-sm text-text-primary" data-testid="unmerge-stations-naming">
                <Prose
                  text={
                    naming.kept
                      ? t('dialogs.detachLine.keeps', {
                          line: prose.line(selected.line_id),
                          others: listNames(otherLines, t),
                          count: otherLines.length,
                        })
                      : t(naming.restored ? 'dialogs.detachLine.restores' : 'dialogs.detachLine.copies', {
                          line: prose.line(selected.line_id),
                        })
                  }
                />
              </p>
            )}

            {/* The remaining junction's station comes before what it decides:
                what the lines that come off get back, the stations inside
                it and the After Detach list (RS-FX10). */}
            {naming && (naming.rest_choices ?? []).length > 1 && (
              <RestPicker
                preview={naming}
                restKeep={restKeep}
                onRestKeep={(id) => {
                  setPickFailed(false);
                  setRestKeep(id === naming.rest_default ? undefined : id);
                }}
                disabled={isLoading}
              />
            )}
            {pickFailed && (
              <p className="text-xs text-error" data-testid="unmerge-stations-rest-failed">
                {t('dialogs.detachLine.restFailed')}
              </p>
            )}

            {naming && selected && (
              <ComesBack
                preview={naming}
                line={selected.line_id}
                lines={naming.kept ? otherLines.length : 1}
                nameOf={nameOf}
                inside={recordsInside(junctionRecords, naming.station_id, naming.kept)}
              />
            )}

            {naming && naming.lines.length > 0 && <DetachResults preview={naming} nameOf={nameOf} />}

            {naming && <ShownElsewhere preview={naming} />}

            {naming && <HiddenComments hidden={naming.comments_hidden} testId="unmerge-stations-comments-hidden" />}

            {!waiting && (
              <p className="text-xs text-text-secondary" data-testid="unmerge-stations-explain">
                <Prose
                  text={
                    naming?.kept && selected
                      ? t('dialogs.detachLine.explainKeeps', {
                          line: prose.line(selected.line_id),
                          others: listNames(otherLines, t),
                        })
                      : t('dialogs.detachLine.explain')
                  }
                />
              </p>
            )}
          </>
        )}

        {targetPreview && (
          <CollisionRows
            collisions={targetPreview.collisions}
            answers={answers}
            onAnswer={setAnswer}
            testIdPrefix="unmerge-stations"
            subject={targetPreview.restored ? { id: targetPreview.station_id, role: 'comesBack' } : undefined}
            disabled={isLoading}
          />
        )}

        {lettersStay && (
          <p className="text-xs text-text-secondary" data-testid="unmerge-stations-letters-stay">
            {t('dialogs.detachLine.lettersStay')}
          </p>
        )}

        <StickyDialogFooter testId="unmerge-stations-footer">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="unmerge-stations-cancel">
            {t('dialogs.detachLine.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !target || waiting || pickPending || editingBlocked || renameProblem !== null}
            data-testid="unmerge-stations-confirm"
          >
            {isLoading ? t('dialogs.detachLine.working') : t('dialogs.detachLine.confirm')}
          </Button>
        </StickyDialogFooter>
      </div>
    </DialogWrapper>
  );
}

/** What every line ends up with after the chosen detach (V-1): its name and
 *  Station ID there, and whether it comes off (with which lines) or stays.
 *  Under RS a line shows its node's one identity, so no line keeps a name of
 *  its own for it; what comes back and the lines that come off together are
 *  `ComesBack`'s, the collision rows `CollisionRows`' (RS 7). */
function DetachResults({ preview, nameOf }: { preview: DetachLinePreview; nameOf: NameOf }) {
  const { t } = useTranslation();
  const prose = useKeptProse();
  return (
    <div className="text-sm text-text-primary" data-testid="unmerge-stations-results">
      <p className="text-text-secondary">{t('dialogs.detachLine.results')}</p>
      <ul className="mt-1 space-y-1 list-disc pl-5">
        {orderedResults(preview).map((r) => {
          const params = {
            line: prose.line(r.line_id),
            // The name the line shows afterwards, said with its Station ID:
            // text (no slot shows it yet).
            name: displayStationName(nameOf(r.station_id, r.name)),
            id: r.station_id,
            joined: listNames(r.joined_with.map((id) => prose.line(id)), t),
          };
          const key = !r.comes_off
            ? 'dialogs.detachLine.resultStays'
            : r.joined_with.length > 0
              ? 'dialogs.detachLine.resultOffWith'
              : preview.restored
                ? 'dialogs.detachLine.resultOff'
                : 'dialogs.detachLine.resultOffNew';
          return (
            <li key={r.line_id} data-testid={`unmerge-stations-result-${r.line_id}`}>
              <Prose text={t(key, params)} />
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * The stations away from the junction that show another station after the
 * detach (RS 4.2, M-P2-8): a loop end whose closure's kept end comes off with
 * the lines, or a station a promotion the detach makes merges. Nothing when
 * there are none.
 */
function ShownElsewhere({ preview }: { preview: DetachLinePreview }) {
  const { t } = useTranslation();
  const prose = useKeptProse();
  const entries = preview.shown_elsewhere ?? [];
  if (entries.length === 0) return null;
  return (
    <div className="text-sm text-text-primary" data-testid="unmerge-stations-shown-elsewhere">
      <p className="text-text-secondary">{t('dialogs.detachLine.shownElsewhereLead')}</p>
      <ul className="mt-1 space-y-1 list-disc pl-5">
        {entries.map((e) => (
          <li
            key={`${e.station.line_id}:${e.station.station_index}`}
            data-testid={`unmerge-stations-shown-elsewhere-${e.station.line_id}-${e.station.station_index}`}
          >
            <Prose
              text={t('dialogs.detachLine.shownElsewhere', {
                line: prose.line(e.station.line_id),
                before: prose.identity(e.before),
                after: prose.identity(e.after),
              })}
            />
          </li>
        ))}
      </ul>
    </div>
  );
}

/** A station that stays inside the one that comes back, with the stations
 *  nested in it, each by its home line (its Station ID said in words), and
 *  the line to detach again to get it back. */
function insideText(t: TFunction, prose: KeptProse, record: ArchivedStation, inner: readonly ArchivedStation[]): string {
  const line = prose.line(record.lines[0]!);
  const station = prose.identityNoId(recordIdentity(record));
  if (inner.length === 0) return t('dialogs.detachLine.staysInside', { station, id: record.id, line });
  return t('dialogs.detachLine.staysInsideWith', {
    station,
    id: record.id,
    inner: listNames(
      inner.map((r) => t('dialogs.detachLine.insideStation', { station: prose.identityNoId(recordIdentity(r)), id: r.id })),
      t,
    ),
    line,
  });
}

/**
 * What comes back on the detached line (RS 4.2, 7): the station's name (as
 * the chosen answers leave it, RS-FX6 P2-2), its comment when it has one, and
 * its Station ID, said of the lines that get it ("They get back" when
 * several do, P3-15); the stations an earlier merge archived inside it,
 * which stay inside it (P3-12); the lines that come off with it because they
 * were one station; and, when a loop closure made the line show another
 * station, the name it shows instead afterwards (its own, the one that comes
 * back, or the junction's when it keeps the junction).
 */
function ComesBack({
  preview,
  line,
  lines,
  nameOf,
  inside,
}: {
  preview: DetachLinePreview;
  line: number;
  /** How many lines get the station back. */
  lines: number;
  nameOf: NameOf;
  inside: readonly InsideRow[];
}) {
  const { t } = useTranslation();
  const prose = useKeptProse();
  const comment = (preview.comment ?? '').trim();
  const lineResult = preview.lines.find((r) => r.line_id === line);
  const shownAfter = lineResult ? nameOf(lineResult.station_id, lineResult.name) : nameOf(preview.station_id, preview.name);
  return (
    <>
      {preview.restored && (
        <div className="text-sm text-text-primary" data-testid="unmerge-stations-comes-back">
          <p>
            {/* The name a station archived now comes back by: text. */}
            {t('dialogs.detachLine.comesBack', {
              name: displayStationName(nameOf(preview.station_id, preview.name)),
              id: preview.station_id,
              count: lines,
            })}
          </p>
          {comment !== '' && (
            <p className="text-text-secondary" data-testid="unmerge-stations-comes-back-comment">
              {t('dialogs.detachLine.comesBackComment', { comment })}
            </p>
          )}
          {inside.map(({ record, inner }) => (
            <p key={record.id} className="text-text-secondary" data-testid={`unmerge-stations-inside-${record.id}`}>
              <Prose text={insideText(t, prose, record, inner)} />
            </p>
          ))}
        </div>
      )}
      {(preview.together ?? []).length > 0 && (
        <p className="text-sm text-text-primary" data-testid="unmerge-stations-together">
          <Prose
            text={t('dialogs.detachLine.offTogether', {
              count: (preview.together ?? []).length,
              lines: listNames((preview.together ?? []).map((id) => prose.line(id)), t),
              line: prose.line(line),
            })}
          />
        </p>
      )}
      {preview.shown_before && (
        <p className="text-sm text-text-secondary" data-testid="unmerge-stations-shown-before">
          <Prose
            text={t('dialogs.detachLine.shownBefore', {
              line: prose.line(line),
              station: prose.identity(preview.shown_before),
              name: displayStationName(shownAfter),
            })}
          />
        </p>
      )}
    </>
  );
}

/** The remaining junction's station (RS 4.2 case 2), when several archived
 *  stations could show it: one radio each, rule K's preselected. */
function RestPicker({
  preview,
  restKeep,
  onRestKeep,
  disabled,
}: {
  preview: DetachLinePreview;
  restKeep: number | undefined;
  onRestKeep: (id: number) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const chosen = restKeep ?? preview.rest_default ?? preview.station_id;
  return (
    <fieldset className="space-y-1 text-sm" data-testid="unmerge-stations-rest">
      <legend className="text-text-primary">{t('dialogs.detachLine.restLead')}</legend>
      {preview.rest_choices.map((identity) => (
        <label key={identity.id} className="flex items-center gap-2 p-1 rounded hover:bg-background-50">
          <input
            type="radio"
            name="unmerge-rest"
            value={identity.id}
            checked={chosen === identity.id}
            disabled={disabled}
            onChange={() => onRestKeep(identity.id)}
            data-testid={`unmerge-stations-rest-${identity.id}`}
          />
          <HomeIdentityPill identity={identity} testId={`unmerge-stations-rest-${identity.id}-pill`} />
        </label>
      ))}
    </fieldset>
  );
}

/** The whole body when the choice does not change the result (N-2): which
 *  line comes off and what it gets back (its comment too), and which line
 *  keeps the station. */
function SameResultBody({ same: { off, keeper, preview }, nameOf }: { same: SameDetachResult; nameOf: NameOf }) {
  const { t } = useTranslation();
  const prose = useKeptProse();
  const line = prose.line(off.line_id);
  const comment = (preview.comment ?? '').trim();
  return (
    <>
      <p className="text-sm text-text-primary" data-testid="unmerge-stations-naming">
        <Prose
          text={t('dialogs.detachLine.comesOff', {
            line,
            name: displayStationName(nameOf(preview.station_id, preview.name)),
            id: preview.station_id,
            keeper: prose.line(keeper.line_id),
            station: prose.station(keeper),
          })}
        />
      </p>
      {comment !== '' && (
        <p className="text-sm text-text-secondary" data-testid="unmerge-stations-comes-back-comment">
          {t('dialogs.detachLine.comesBackComment', { comment })}
        </p>
      )}
      <p className="text-xs text-text-secondary" data-testid="unmerge-stations-explain">
        <Prose text={t('dialogs.detachLine.explainComesOff', { line })} />
      </p>
    </>
  );
}
