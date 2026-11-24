/**
 * The import wizards' connection descriptions as prose (RS-PROSE): the
 * existing project station a suggestion connects to is that slot's pill
 * (`prose.station(ref, true)`, coloured by its line); a station of an incoming
 * line, which is not in the project yet and so has no line id, is a neutral
 * station pill with that line's wizard name as its context. Each reads, word
 * for word, what `dmpFileStationLabel` / `suggestionTargetStationLabel` write
 * for plain text (select options, the alternatives' buttons).
 */
import type { CrossLineSuggestion } from '../types/commands';
import { dmpFileStationLabel, dmpFileStationName } from './importStationNames';
import { proseToken, type ProsePart, type ProseTokens } from './prose';
import { lineDisplayName } from './stationIdentity';

/** `Line A / 5`, station `index` (file numbering) of the incoming line `lineName`, as a neutral station part. */
export function dmpFileStationPart(index: number, lineName: string | null | undefined): ProsePart {
  return {
    kind: 'station',
    text: dmpFileStationLabel(index, lineName),
    name: dmpFileStationName(index),
    context: lineDisplayName(lineName),
  };
}

/**
 * Station `index` at the far end of a DMP suggestion as a prose token: an
 * existing line's slot by its pill, another incoming line's station by
 * {@link dmpFileStationPart} under the name the user gave that line in the
 * wizard (`wizardLineName`, by import index), the file's until renamed.
 */
export function suggestionTargetStationProse(
  prose: Pick<ProseTokens, 'station'>,
  suggestion: Pick<CrossLineSuggestion, 'targetType' | 'existingLineId' | 'existingLineName'> &
    Partial<Pick<CrossLineSuggestion, 'targetImportIndex'>>,
  index: number,
  wizardLineName?: (importIndex: number) => string | undefined,
): string {
  if (suggestion.targetType === 'existing' && suggestion.existingLineId != null) {
    return prose.station({ line_id: suggestion.existingLineId, station_index: index }, true);
  }
  const lineName =
    suggestion.targetType === 'importing' && suggestion.targetImportIndex != null
      ? (wizardLineName?.(suggestion.targetImportIndex) ?? suggestion.existingLineName)
      : suggestion.existingLineName;
  return proseToken(dmpFileStationPart(index, lineName));
}
