/**
 * Another process's output passed on a line at a time, each line with the
 * checkout folded to `<repo>` and the home directory to `~` (`foldLocalPaths`,
 * #576 RV19b-P8).
 *
 * `agent:mcp:smoke` passes the MCP server's stderr through this: an `app_start`
 * build writes `pnpm tauri build`'s output there, and cargo's `Compiling <crate>
 * (<path>)` and Tauri's `Built application at: <path>` name the checkout under
 * the user's home, beside the smoke's shareable check lines.
 */
import type { Stream } from 'stream';
import { StringDecoder } from 'string_decoder';

import { foldLocalPaths } from './shareableFailure';

/**
 * Writes `input` to `write` as it arrives, a line folded once it is whole, so a
 * path split across two chunks still folds and nothing waits for the stream to
 * end but a last line without a newline. Resolves once `input` has ended,
 * failed or closed, after that last line.
 */
export function passLinesFolded(input: Stream, write: (text: string) => void): Promise<void> {
  const decoder = new StringDecoder('utf8');
  let partial = '';
  return new Promise((resolve) => {
    let finished = false;
    const finish = (): void => {
      if (finished) return;
      finished = true;
      const last = partial + decoder.end();
      if (last !== '') write(foldLocalPaths(last));
      resolve();
    };
    input.on('data', (chunk: Buffer | string) => {
      const lines = (partial + (typeof chunk === 'string' ? chunk : decoder.write(chunk))).split('\n');
      partial = lines.pop() ?? '';
      if (lines.length > 0) write(lines.map((line) => `${foldLocalPaths(line)}\n`).join(''));
    });
    input.once('end', finish);
    input.once('error', finish);
    input.once('close', finish);
  });
}
