import { readFileSync } from 'node:fs';
import { dirname, resolve as pathResolve } from 'node:path';
import { createRequire } from 'node:module';

// Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Node-side: `setupTauriMocks` injects
// what it reads into the page before the mock attaches.

// ----------------------------------------------------------------------------
// msgpack UMD bundle loader.
//
// The production frontend hits `load_project_full_binary` and expects a
// msgpack-encoded `BinaryEnvelope` (see src/utils/loadProjectBinary.ts).
// The mock runs inside the browser via `addInitScript` and can't import
// from npm, so before the mock attaches we inject the @msgpack/msgpack
// UMD bundle, which exposes `window.MessagePack.encode`.  The mock
// handler for `load_project_full_binary` builds the same fixture as
// `load_project_full`, repacks it into the binary wire format, and
// encodes it - exercising the real decoder end-to-end with no
// production-code fallbacks.
// ----------------------------------------------------------------------------
let cachedMsgpackUmd: string | null = null;
function loadMsgpackUmd(): string {
  if (cachedMsgpackUmd !== null) return cachedMsgpackUmd;
  const req = createRequire(import.meta.url);
  const cjsPath = req.resolve('@msgpack/msgpack');
  // dist.cjs/index.cjs → ../dist.umd/msgpack.min.js
  const pkgRoot = dirname(dirname(cjsPath));
  const umdPath = pathResolve(pkgRoot, 'dist.umd/msgpack.min.js');
  cachedMsgpackUmd = readFileSync(umdPath, 'utf8');
  return cachedMsgpackUmd;
}

export { loadMsgpackUmd };
