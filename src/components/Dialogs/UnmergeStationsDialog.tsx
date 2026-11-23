import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
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
  recordIdentity,
  StickyDialogFooter,
  useCollisionAnswers,
  useIdentityLabel,
} from './keptStationParts';

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

/** The records a restored station keeps inside it (RS-FX4): those the
 *  junction archived under it, which a second Detach gives back. */
export function recordsInside(records: readonly ArchivedStation[] | undefined, id: number): ArchivedStation[] {
  return (records ?? []).filter((r) => r.into === id && r.lines.length > 0);
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
 * station when detached (`kept`: every other line knew it by an alias) and
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
 * together stay joined on ONE station, so a line among them that had its own
 * Station ID keeps it only as an alias, and the list says so.
 *
 * On the station graph a merge is ONE station that two or more lines pass. The
 * user picks which line leaves it: that line gets its own station at the same
 * place (no comment), and the datums and closures attached through it go
 * along; the other lines stay joined. A line that was merged there gets back
 * the Station ID and name it had (the junction's alias naming it); any other
 * gets the name it showed the junction by, under a new Station ID. When every
 * other line knew the junction by an alias, the chosen line keeps it and the
 * others get back the station they had (I-1). The dialog says which, from the
 * backend's `preview_detach_line` (review finding UX-4), asked for every line
 * when the dialog opens. The clicked slot's line is listed first. It is
 * preselected unless it is the junction's owner (detaching it would give it a
 * NEW Station ID, or keep the junction on it while the others come off) and
 * another line would come off alone with its own station back: that line is
 * then the likely intent and preselected (demo findings I-7, V-1). When the
 * chosen line gets its station back and the
 * others keep the junction, the comment stays with them; the dialog says so,
 * since a merge (or the 0.6.0 upgrade) may have joined that line's own
 * comment into it and the alias does not keep it. Its own line detaches with
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
  const otherLines = selected
    ? slots.filter((ref) => ref.line_id !== selected.line_id).map((ref) => labels.lineName(ref.line_id))
    : [];
  /** A row's label: the line (a line pill), or for a line that keeps the
   *  junction the lines that then come off together, as a sentence (V-1). */
  const rowLabel = (ref: StationRef, clicked: boolean): ReactNode => {
    const line = labels.lineName(ref.line_id);
    if (previews.get(ref.line_id)?.kept) {
      const others = listNames(
        slots.filter((o) => o.line_id !== ref.line_id).map((o) => labels.lineName(o.line_id)),
        t,
      );
      return t(clicked ? 'dialogs.detachLine.togetherClicked' : 'dialogs.detachLine.together', { line, others });
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

  const handleConfirm = async () => {
    if (!stationRef || !target) return;
    if (isEditingBlockedNow()) {
      setError(t('dialogs.keptStations.editingDisabled'));
      return;
    }

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
              {t(waiting ? 'dialogs.detachLine.leadWaiting' : 'dialogs.detachLine.lead', {
                station: labels.station(stationRef),
                count: slots.length,
              })}
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
                {naming.kept
                  ? t('dialogs.detachLine.keeps', {
                      line: labels.lineName(selected.line_id),
                      others: listNames(otherLines, t),
                      count: otherLines.length,
                    })
                  : t(naming.restored ? 'dialogs.detachLine.restores' : 'dialogs.detachLine.copies', {
                      line: labels.lineName(selected.line_id),
                    })}
              </p>
            )}

            {naming && selected && (
              <ComesBack
                preview={naming}
                line={selected.line_id}
                lines={naming.kept ? otherLines.length : 1}
                nameOf={nameOf}
                inside={recordsInside(junctionRecords, naming.station_id)}
              />
            )}

            {naming && naming.lines.length > 0 && <DetachResults preview={naming} nameOf={nameOf} />}

            {naming && <ShownElsewhere preview={naming} />}

            {naming && <HiddenComments hidden={naming.comments_hidden} testId="unmerge-stations-comments-hidden" />}

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

            {!waiting && (
              <p className="text-xs text-text-secondary" data-testid="unmerge-stations-explain">
                {naming?.kept && selected
                  ? t('dialogs.detachLine.explainKeeps', {
                      line: labels.lineName(selected.line_id),
                      others: listNames(otherLines, t),
                    })
                  : t('dialogs.detachLine.explain')}
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
            disabled={isLoading || !target || waiting || pickPending || editingBlocked}
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
 *  Under RS a line shows its node's one identity, so no line keeps an alias
 *  of its own; what comes back and the lines that come off together are
 *  `ComesBack`'s, the collision rows `CollisionRows`' (RS 7). */
function DetachResults({ preview, nameOf }: { preview: DetachLinePreview; nameOf: NameOf }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  return (
    <div className="text-sm text-text-primary" data-testid="unmerge-stations-results">
      <p className="text-text-secondary">{t('dialogs.detachLine.results')}</p>
      <ul className="mt-1 space-y-1 list-disc pl-5">
        {orderedResults(preview).map((r) => {
          const params = {
            line: labels.lineName(r.line_id),
            name: displayStationName(nameOf(r.station_id, r.name)),
            id: r.station_id,
            joined: listNames(r.joined_with.map(labels.lineName), t),
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
              {t(key, params)}
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
  const labels = useIdentityLabels();
  const label = useIdentityLabel();
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
            {t('dialogs.detachLine.shownElsewhere', {
              line: labels.lineName(e.station.line_id),
              before: label(e.before),
              after: label(e.after),
            })}
          </li>
        ))}
      </ul>
    </div>
  );
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
  inside: readonly ArchivedStation[];
}) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const label = useIdentityLabel();
  const comment = (preview.comment ?? '').trim();
  const lineResult = preview.lines.find((r) => r.line_id === line);
  const shownAfter = lineResult ? nameOf(lineResult.station_id, lineResult.name) : nameOf(preview.station_id, preview.name);
  return (
    <>
      {preview.restored && (
        <div className="text-sm text-text-primary" data-testid="unmerge-stations-comes-back">
          <p>
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
          {inside.map((record) => (
            <p key={record.id} className="text-text-secondary" data-testid={`unmerge-stations-inside-${record.id}`}>
              {t('dialogs.detachLine.staysInside', {
                station: label(recordIdentity(record)),
                id: record.id,
                line: labels.lineName(record.lines[0]!),
              })}
            </p>
          ))}
        </div>
      )}
      {(preview.together ?? []).length > 0 && (
        <p className="text-sm text-text-primary" data-testid="unmerge-stations-together">
          {t('dialogs.detachLine.offTogether', {
            count: (preview.together ?? []).length,
            lines: listNames((preview.together ?? []).map(labels.lineName), t),
            line: labels.lineName(line),
          })}
        </p>
      )}
      {preview.shown_before && (
        <p className="text-sm text-text-secondary" data-testid="unmerge-stations-shown-before">
          {t('dialogs.detachLine.shownBefore', {
            line: labels.lineName(line),
            station: label(preview.shown_before),
            name: displayStationName(shownAfter),
          })}
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
  const labels = useIdentityLabels();
  const line = labels.lineName(off.line_id);
  const keeperLine = labels.lineName(keeper.line_id);
  const comment = (preview.comment ?? '').trim();
  return (
    <>
      <p className="text-sm text-text-primary" data-testid="unmerge-stations-naming">
        {t('dialogs.detachLine.comesOff', {
          line,
          name: displayStationName(nameOf(preview.station_id, preview.name)),
          id: preview.station_id,
          keeper: keeperLine,
          station: labels.station(keeper),
        })}
      </p>
      {comment !== '' && (
        <p className="text-sm text-text-secondary" data-testid="unmerge-stations-comes-back-comment">
          {t('dialogs.detachLine.comesBackComment', { comment })}
        </p>
      )}
      <p className="text-xs text-text-secondary" data-testid="unmerge-stations-explain">
        {t('dialogs.detachLine.explainComesOff', { line })}
      </p>
    </>
  );
}
