/**
 * Picks the session implementation for the running platform. Every consumer
 * (MCP server, smokes) goes through here instead of naming a platform class, so
 * adding one is a change to this file only.
 *
 * An unsupported platform is refused HERE, immediately and by name. It used to
 * be discovered as a 15 s "tauri-driver did not start listening" timeout on
 * macOS, which reads as a broken driver rather than an unsupported OS (#422).
 */
// FIRST import on purpose, before anything can pull in webdriverio: it patches
// webdriver's undici. See the module comment for the Node >= 26 failure it fixes.
import './lib/undici-dispatcher-pin';
import { targetPlatform } from './lib/platform';
import { valueShape } from './lib/shareableFailure';
import { AppSession } from './session';
import { MacSession } from './sessionMac';
import type { AgentSession } from './sessionTypes';

const DESIGN_DOC = 'docs/plans/2026-08-05_macos-agent-harness.md';

export function createSession(): AgentSession {
  const plat = targetPlatform();
  if (plat === 'linux') return new AppSession();
  if (plat === 'darwin') return new MacSession();
  // Node's own platform name, or the override's shape (#576 W51): STALAGMIGHT_AGENT_PLATFORM is the user's text.
  const named = plat === process.platform ? `"${process.platform}"` : `${valueShape(plat)} (STALAGMIGHT_AGENT_PLATFORM)`;
  throw new Error(
    `the agent harness has no session backend for platform ${named}: it requires Linux ` +
      '(tauri-driver: `cargo install tauri-driver --locked`, plus webkit2gtk-driver) or macOS ' +
      '(tauri-wd: `cargo install --git https://github.com/FumblingBarbell/tauri-webdriver ' +
      `tauri-webdriver-automation\` - see issue #421 and ${DESIGN_DOC}).`,
  );
}
