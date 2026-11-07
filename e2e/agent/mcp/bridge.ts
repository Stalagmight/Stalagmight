/**
 * File-based bridge to the stalagmight-app MCP server, for driving the harness
 * from plain shell commands. This is how an agent (or a human in a terminal)
 * uses the harness when no live MCP client connection is available, e.g. a
 * subagent whose session-level MCP server is down.
 *
 * Usage (on the repo-pinned Node, `nvm use`):
 *   pnpm agent:bridge <bridgeDir>
 *
 * Protocol:
 *   - Append ONE json object per line to <bridgeDir>/requests.jsonl:
 *       {"id":"r1","tool":"app_status","args":{},"timeoutMs":120000}
 *     Requests are processed strictly in order. Default timeoutMs 180000.
 *   - Responses append to <bridgeDir>/responses.jsonl:
 *       {"id":"r1","ok":true,"ms":12,"content":[...]}
 *     Text content that parses as JSON becomes {type:"json",value}; screenshot
 *     image data is stripped (the PNG is on disk in the session dir, its path
 *     rides in the text part).
 *   - {"id":"x","tool":"__exit"} stops the app session and shuts the bridge down.
 *   - <bridgeDir>/ready (containing the bridge PID) appears once the server is
 *     connected and requests will be honored.
 */
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

import { passLinesFolded } from '../lib/foldedLines';
import { foldLocalPaths, shareableErrorText } from '../lib/shareableFailure';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const serverPath = path.join(repoRoot, 'e2e', 'agent', 'mcp', 'server.ts');

interface BridgeRequest {
  id: unknown;
  tool: string;
  args?: Record<string, unknown>;
  timeoutMs?: number;
}

interface ContentPart {
  type: string;
  text?: string;
  [key: string]: unknown;
}

function sanitize(content: unknown): unknown[] {
  if (!Array.isArray(content)) return [];
  return (content as ContentPart[]).map((c) => {
    if (c.type === 'image') {
      return { type: 'image', note: 'png data stripped; file path is in the text part / session dir' };
    }
    if (c.type === 'text' && typeof c.text === 'string') {
      try {
        return { type: 'json', value: JSON.parse(c.text) };
      } catch {
        return { type: 'text', text: c.text };
      }
    }
    return c;
  });
}

async function main(): Promise<void> {
  const bridgeDir = process.argv[2];
  if (!bridgeDir) {
    process.stderr.write('usage: pnpm agent:bridge <bridgeDir>\n');
    process.exit(2);
  }
  fs.mkdirSync(bridgeDir, { recursive: true });
  const reqPath = path.join(bridgeDir, 'requests.jsonl');
  const resPath = path.join(bridgeDir, 'responses.jsonl');
  fs.appendFileSync(reqPath, '');

  const transport = new StdioClientTransport({
    command: process.execPath,
    args: ['--import', 'tsx', serverPath],
    cwd: repoRoot,
    env: { ...process.env } as Record<string, string>,
    // Piped, not inherited (#576 W51), as `agent:mcp:smoke` pipes it (RV19b-P8):
    // a build's output on the server's stderr names the checkout under the home
    // directory. It prints here a line at a time, folded.
    stderr: 'pipe',
  });
  const serverOutput = transport.stderr;
  if (serverOutput) void passLinesFolded(serverOutput, (text) => process.stderr.write(text));
  const client = new Client({ name: 'stalagmight-bridge', version: '0.1.0' });
  await client.connect(transport);
  // Folded (#576 W51): the bridge directory is the user's path.
  process.stderr.write(`[bridge] connected; node ${process.versions.node}; watching ${foldLocalPaths(reqPath)}\n`);
  fs.writeFileSync(path.join(bridgeDir, 'ready'), String(process.pid));

  const respond = (obj: object): void => {
    fs.appendFileSync(resPath, `${JSON.stringify(obj)}\n`);
  };

  const pending: BridgeRequest[] = [];
  // Start reading at EOF: a bridge relaunched against a non-empty bridgeDir
  // must not re-execute the entire request history (every mutation would run
  // twice). The protocol's `ready` file tells writers when to start appending,
  // so anything already in the file predates this bridge instance.
  let offset = 0;
  try {
    offset = fs.statSync(reqPath).size;
    if (offset > 0) {
      process.stderr.write(`[bridge] skipping ${offset} bytes of pre-existing requests (history)\n`);
    }
  } catch {
    offset = 0;
  }
  let busy = false;

  async function pump(): Promise<void> {
    if (busy) return;
    busy = true;
    while (pending.length) {
      const req = pending.shift() as BridgeRequest;
      if (req.tool === '__exit') {
        respond({ id: req.id, ok: true, exiting: true });
        try {
          await client.callTool({ name: 'app_stop', arguments: {} }, undefined, { timeout: 30000 });
        } catch {
          // Session may not be running.
        }
        await client.close().catch(() => undefined);
        process.exit(0);
      }
      const t0 = Date.now();
      try {
        const res = (await client.callTool(
          { name: req.tool, arguments: req.args ?? {} },
          undefined,
          { timeout: req.timeoutMs ?? 180000 },
        )) as { isError?: boolean; content?: unknown };
        respond({
          id: req.id,
          ok: !res.isError,
          isError: Boolean(res.isError),
          ms: Date.now() - t0,
          content: sanitize(res.content),
        });
      } catch (err) {
        respond({
          id: req.id,
          ok: false,
          ms: Date.now() - t0,
          error: err instanceof Error ? err.message : String(err),
        });
      }
    }
    busy = false;
  }

  setInterval(() => {
    let text: string;
    try {
      text = fs.readFileSync(reqPath, 'utf8');
    } catch {
      return;
    }
    if (text.length <= offset) return;
    const chunk = text.slice(offset);
    const upto = chunk.lastIndexOf('\n');
    if (upto < 0) return;
    offset += upto + 1;
    for (const line of chunk.slice(0, upto).split('\n')) {
      if (!line.trim()) continue;
      try {
        pending.push(JSON.parse(line) as BridgeRequest);
      } catch {
        respond({ id: null, ok: false, error: `unparseable request line: ${line.slice(0, 120)}` });
      }
    }
    void pump();
  }, 150);
}

main().catch((err) => {
  // The shareable crash form (#576 W33-NF6): a crash in session setup can quote
  // the app (an IPC rejection, a page script's error).
  process.stderr.write(`[bridge] fatal: ${shareableErrorText(err)}\n`);
  process.exit(1);
});
