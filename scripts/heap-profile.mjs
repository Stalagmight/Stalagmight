#!/usr/bin/env node
/**
 * WebKit Inspector → JSC heap class profile.
 *
 * Connects to a running WebKitGTK app exposing the Inspector
 * (WEBKIT_INSPECTOR_SERVER=127.0.0.1:PORT), repeatedly takes a heap
 * snapshot via `Heap.snapshot`, parses it, and prints a sorted list
 * of per-class retained byte counts + counts. Also prints a delta vs
 * the previous snapshot, so allocation pressure between checkpoints
 * is visible.
 *
 * The Heap.snapshot result is a string in V8 HeapSnapshot JSON format
 * (JSC adopted it). Format: `{ snapshot: { meta, node_count, ... },
 * nodes: [...], edges: [...], strings: [...] }`. Each node consumes
 * `nodeFields.length` entries in `nodes` (7 fields by default:
 *   type, name, id, self_size, edge_count, trace_node_id, detachedness)
 * where `type` is an index into meta.node_types[0] and `name` is an
 * index into `strings`.
 *
 * Usage:
 *   node scripts/heap-profile.mjs [--port 9229] [--interval 5] [--top 25]
 *
 * Press Enter in the terminal to take an ad-hoc snapshot (labelled
 * `manual`) on top of the interval snapshots.
 */
// Node 22+ has a built-in WebSocket global, so no need for the 'ws' package.
import readline from 'node:readline';
import fs from 'node:fs';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, idx, arr) => {
    if (cur.startsWith('--')) acc.push([cur.slice(2), arr[idx + 1] ?? '']);
    return acc;
  }, []),
);
const PORT = Number(args.port ?? 9229);
const INTERVAL_S = Number(args.interval ?? 0); // 0 = manual only
const TOP = Number(args.top ?? 25);

/**
 * An error as this tool prints it (#576 W51): its message - the inspector's
 * protocol error, or this tool's own - but a system error by its code (an fs
 * error's message names the --file path) and a parse error by its name (its
 * message quotes the snapshot, which holds the app's strings). Never the stack:
 * its frames name the checkout's absolute path.
 */
const errText = (e) => (e?.code ? e.code : e instanceof SyntaxError ? e.name : String(e?.message ?? e));

// ---------- Inspector connection ---------------------------------------------

async function findTarget() {
  // WebKitGTK exposes a JSON list at http://host:port/json describing
  // every inspectable target. We pick the first frontend page.
  const res = await fetch(`http://127.0.0.1:${PORT}/json`);
  if (!res.ok) throw new Error(`/json returned ${res.status}`);
  const targets = await res.json();
  if (!Array.isArray(targets) || targets.length === 0) {
    throw new Error('no inspectable targets');
  }
  // Prefer the main app page over service workers / iframes.
  const main = targets.find((t) => t.type === 'page') ?? targets[0];
  return main;
}

let nextId = 1;
const pending = new Map(); // id -> { resolve, reject }

function send(ws, method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

// ---------- Snapshot capture + parse -----------------------------------------

let snapshotChunks = [];
let snapshotInProgress = null; // { resolve, reject }

function handleNotification(msg) {
  if (msg.method === 'Heap.addHeapSnapshotChunk') {
    snapshotChunks.push(msg.params.chunk);
  } else if (msg.method === 'Heap.trackingHeapSnapshotComplete' ||
             msg.method === 'Heap.heapSnapshotComplete') {
    if (snapshotInProgress) {
      const full = snapshotChunks.join('');
      snapshotChunks = [];
      const inProg = snapshotInProgress;
      snapshotInProgress = null;
      try {
        const parsed = JSON.parse(full);
        inProg.resolve(parsed);
      } catch (e) {
        inProg.reject(e);
      }
    }
  }
}

async function takeSnapshot(ws) {
  if (snapshotInProgress) throw new Error('snapshot already in progress');
  snapshotChunks = [];
  const snapPromise = new Promise((resolve, reject) => {
    snapshotInProgress = { resolve, reject };
    // safety timeout
    setTimeout(() => {
      if (snapshotInProgress) {
        snapshotInProgress.reject(new Error('snapshot timeout 120s'));
        snapshotInProgress = null;
      }
    }, 120_000);
  });
  // Different webkit versions name this differently. Try both.
  try {
    await send(ws, 'Heap.snapshot');
  } catch (e) {
    try {
      await send(ws, 'HeapProfiler.takeHeapSnapshot');
    } catch (e2) {
      throw new Error(`Heap.snapshot failed: ${e.message}; HeapProfiler.takeHeapSnapshot failed: ${e2.message}`);
    }
  }
  return snapPromise;
}

/**
 * Aggregate a parsed snapshot into { className -> { count, selfBytes } }.
 * Uses self_size (not retained), which is what JSC reports per node.
 */
function aggregate(snap) {
  const meta = snap.snapshot.meta;
  const fields = meta.node_fields; // e.g. ['type','name','id','self_size','edge_count','trace_node_id','detachedness']
  const nodeTypes = meta.node_types[0]; // index 0 is the 'type' enum
  const fieldCount = fields.length;
  const iType = fields.indexOf('type');
  const iName = fields.indexOf('name');
  const iSelf = fields.indexOf('self_size');
  const nodes = snap.nodes;
  const strings = snap.strings;

  const byClass = new Map(); // key -> { count, bytes }
  const totalNodes = nodes.length / fieldCount;

  let totalBytes = 0;
  for (let i = 0; i < nodes.length; i += fieldCount) {
    const typeIdx = nodes[i + iType];
    const nameIdx = nodes[i + iName];
    const self = nodes[i + iSelf];
    const typeName = nodeTypes[typeIdx];
    let key;
    if (typeName === 'object' || typeName === 'closure' || typeName === 'native') {
      // object: name is the constructor (e.g. "Object", "Array", "Float32Array")
      key = `${typeName}:${strings[nameIdx] ?? '<anon>'}`;
    } else if (typeName === 'string' || typeName === 'concatenated string' || typeName === 'sliced string') {
      key = 'string';
    } else if (typeName === 'array') {
      key = 'array';
    } else if (typeName === 'hidden') {
      key = `hidden:${strings[nameIdx] ?? ''}`;
    } else {
      key = typeName;
    }
    const cur = byClass.get(key);
    if (cur) {
      cur.count++;
      cur.bytes += self;
    } else {
      byClass.set(key, { count: 1, bytes: self });
    }
    totalBytes += self;
  }
  return { byClass, totalNodes, totalBytes };
}

function fmtBytes(n) {
  if (n < 1024) return `${n}B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}K`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)}M`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)}G`;
}

function reportSnapshot(label, agg, prev) {
  console.log(`\n=== ${label} ===`);
  console.log(`  total nodes: ${agg.totalNodes.toLocaleString()}   total self bytes: ${fmtBytes(agg.totalBytes)}`);
  if (prev) {
    const dN = agg.totalNodes - prev.totalNodes;
    const dB = agg.totalBytes - prev.totalBytes;
    const sign = (x) => (x >= 0 ? '+' : '');
    console.log(`  Δ nodes: ${sign(dN)}${dN.toLocaleString()}   Δ bytes: ${sign(dB)}${fmtBytes(dB)}`);
  }
  // top by self bytes
  const rows = [...agg.byClass.entries()]
    .map(([k, v]) => {
      const p = prev?.byClass.get(k);
      return {
        k,
        count: v.count,
        bytes: v.bytes,
        dCount: p ? v.count - p.count : null,
        dBytes: p ? v.bytes - p.bytes : null,
      };
    })
    .sort((a, b) => b.bytes - a.bytes)
    .slice(0, TOP);
  console.log(`  top ${TOP} classes by self bytes:`);
  console.log(`    ${'class'.padEnd(38)} ${'count'.padStart(12)} ${'bytes'.padStart(10)} ${'Δcount'.padStart(12)} ${'Δbytes'.padStart(10)}`);
  for (const r of rows) {
    const dC = r.dCount === null ? '' : (r.dCount >= 0 ? `+${r.dCount}` : `${r.dCount}`);
    const dB = r.dBytes === null ? '' : (r.dBytes >= 0 ? `+${fmtBytes(r.dBytes)}` : `-${fmtBytes(-r.dBytes)}`);
    console.log(`    ${r.k.padEnd(38).slice(0, 38)} ${r.count.toLocaleString().padStart(12)} ${fmtBytes(r.bytes).padStart(10)} ${dC.padStart(12)} ${dB.padStart(10)}`);
  }
}

// ---------- Main -------------------------------------------------------------

async function main() {
  // --file MODE: parse a saved heap snapshot from disk and print the class
  // breakdown. No inspector connection. Use this when a snapshot was saved
  // by hand from the WebKit DevTools Memory tab.
  if (args.file) {
    const paths = args.file.split(',').map((p) => p.trim()).filter(Boolean);
    let prev = null;
    for (const [i, p] of paths.entries()) {
      // By position, not by path: the path is the caller's own, and can name
      // their directories (#576 W51).
      const label = `--file ${i + 1} of ${paths.length}`;
      console.log(`\nparsing ${label} …`);
      const raw = fs.readFileSync(p, 'utf8');
      const snap = JSON.parse(raw);
      const agg = aggregate(snap);
      reportSnapshot(label, agg, prev);
      prev = agg;
    }
    return;
  }

  const target = await findTarget();
  // The page title is the app's: it names the open project's file (src/App.tsx).
  console.log(`connecting to ${target.webSocketDebuggerUrl ?? target.url ?? '<unknown>'} (title: ${String(target.title ?? '').length} chars)`);
  const ws = new WebSocket(target.webSocketDebuggerUrl ?? `ws://127.0.0.1:${PORT}/devtools/page/${target.id}`);
  await new Promise((res, rej) => {
    ws.addEventListener('open', res, { once: true });
    ws.addEventListener('error', rej, { once: true });
  });

  ws.addEventListener('message', (ev) => {
    let msg;
    const data = typeof ev.data === 'string' ? ev.data : (ev.data instanceof Blob ? null : String(ev.data));
    if (data === null) return;
    try { msg = JSON.parse(data); } catch { return; }
    if (msg.id !== undefined) {
      const p = pending.get(msg.id);
      if (p) {
        pending.delete(msg.id);
        if (msg.error) p.reject(new Error(msg.error.message ?? JSON.stringify(msg.error)));
        else p.resolve(msg.result);
      }
    } else if (msg.method) {
      handleNotification(msg);
    }
  });

  // Enable the Heap domain so notifications arrive
  try { await send(ws, 'Heap.enable'); } catch {}
  try { await send(ws, 'HeapProfiler.enable'); } catch {}
  try { await send(ws, 'Runtime.enable'); } catch {}

  let prev = null;

  async function snapAndReport(label) {
    process.stdout.write(`\n[${new Date().toISOString().slice(11, 19)}] taking snapshot (${label})…`);
    const t0 = Date.now();
    const snap = await takeSnapshot(ws);
    const t1 = Date.now();
    process.stdout.write(` ${(t1 - t0) / 1000}s\n`);
    const agg = aggregate(snap);
    reportSnapshot(label, agg, prev);
    prev = agg;
  }

  // Initial snapshot
  await snapAndReport('initial');

  if (INTERVAL_S > 0) {
    setInterval(() => {
      snapAndReport(`auto +${INTERVAL_S}s`).catch((e) => console.error('snapshot err', errText(e)));
    }, INTERVAL_S * 1000);
  }

  // Manual trigger on Enter
  const rl = readline.createInterface({ input: process.stdin });
  console.log('\nPress Enter to take a labelled snapshot. Type a label first to name it. Ctrl+C to exit.');
  rl.on('line', (line) => {
    const label = line.trim() || 'manual';
    snapAndReport(label).catch((e) => console.error('snapshot err', errText(e)));
  });
}

main().catch((e) => {
  console.error(`heap-profile: ${errText(e)}`);
  process.exit(1);
});
