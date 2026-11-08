/**
 * `agent:smoke`'s `started:` line (#576 W58 Q14): the session's StartResult as
 * one JSON object, its paths folded, and its X display only in the form the
 * harness's own Xvfb pool gives it.
 *
 * The display is `:N` for a pool display and for a plain local `DISPLAY`, and
 * the sentinel `aqua` on darwin. Any other `DISPLAY` the session inherited -
 * `localhost:10.0` under `ssh -X`, `host.example:0` - names a machine, so it
 * prints as `<display>`.
 */
import type { StartResult } from '../sessionTypes';
import { foldLocalPaths } from './shareableFailure';

/** The display as the started line prints it: `:N` or `aqua` as itself, anything else `<display>`. */
export function shareableDisplay(display: string): string {
  return /^:\d{1,6}$/.test(display) || display === 'aqua' ? display : '<display>';
}

/** The StartResult as the started line prints it: the display shaped, every path folded. */
export function smokeStartedLine(started: StartResult): string {
  return foldLocalPaths(JSON.stringify({ ...started, display: shareableDisplay(started.display) }));
}
