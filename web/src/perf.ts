// TS-side profiler mirror. Maintains a frame subtree independently of the
// Rust profiler (R4: no round-trip through wasm). Records into a local ring
// buffer per node, emits JSON via reportJson() for the UI to concatenate
// with the Rust tick subtree.
//
// Design: mirrors the Rust Profiler (D1, D6): same ring-buffer-per-node
// semantics, same JSON shape, same pruning on report. No wasm calls needed.

// v1.6 Wave B: this module is purely TS-side. The Rust profiler is enabled
// via a `profile_enable` postMessage to the sim worker (wired in
// `web/src/widgets/perf-panel.ts`); main holds no wasm handle.

// ─── Constants (mirror of src/profiler.rs) ────────────────────────────────

const WINDOW_MS = 60_000;
const SAMPLES_PER_NODE = 4096;
const MAX_NODES = 256;

// ─── Types ────────────────────────────────────────────────────────────────

type NodeId = number;

interface PerfNode {
  name: string;
  parent: NodeId | null;
  children: NodeId[];
  samples: Array<[number, number]>; // [timestamp_ms, dur_us]
}

// ─── State ────────────────────────────────────────────────────────────────

// Node 0: "frame" root.
const nodes: PerfNode[] = [
  { name: "frame", parent: null, children: [], samples: [] },
];
const childIndex = new Map<string, NodeId>(); // "parentId:name" → child id
const stack: Array<{ nodeId: NodeId; t0: number }> = [];

let enabled = false;
let epochMs = performance.now();

// ─── Public API ───────────────────────────────────────────────────────────

export function setProfilerEnabled(on: boolean): void {
  enabled = on;
  // Rust-side profile_enable lands via a SimBridge postMessage from
  // `web/src/widgets/perf-panel.ts`; we only toggle the TS-side state here.
  if (on) {
    // Reset epoch on enable so first sample is at t≈0.
    epochMs = performance.now();
    clearSamples();
  } else {
    clearSamples();
    stack.length = 0;
  }
}

export function isProfilerEnabled(): boolean {
  return enabled;
}

/** Open a named span. Returns a handle with a close() method.
 *
 * v1.7: when called at the top of the stack with a name that exactly matches
 * a top-level root ("frame"), the sample is attached to the root itself
 * instead of opening a duplicate child under it. This makes
 * `span("frame")` at the top of the RAF callback the canonical outer
 * bracket — its total_us shows up as the root's own measured time, not as
 * a `frame.frame` child row in the panel. */
export function span(name: string): { close: () => void } {
  if (!enabled) return NOOP_SPAN;
  const t0 = performance.now();
  let nodeId: NodeId;
  if (stack.length === 0 && name === "frame") {
    // Attach the sample to the pre-seeded root rather than a same-named child.
    nodeId = 0;
  } else {
    const parent = stack.length > 0 ? stack[stack.length - 1].nodeId : 0; // 0 = frame root
    nodeId = ensureChild(parent, name);
  }
  stack.push({ nodeId, t0 });
  return {
    close() {
      const top = stack.pop();
      if (!top) return;
      const durUs = Math.max(0, Math.round((performance.now() - top.t0) * 1000));
      const tsMs = Math.max(0, performance.now() - epochMs);
      recordSample(top.nodeId, tsMs, durUs);
    },
  };
}

const NOOP_SPAN = { close: () => {} };

/** Sugar for synchronous blocks. */
export function timed<T>(name: string, fn: () => T): T {
  const s = span(name);
  try {
    return fn();
  } finally {
    s.close();
  }
}

/** Build a JSON report for the TS-side frame subtree. Same shape as Rust. */
export function reportJson(): string {
  const nowMs = performance.now();
  const nowMsRel = nowMs - epochMs;
  pruneAll(nowMsRel);
  const parts: string[] = [];
  serializeNode(0, nowMsRel, null, parts);
  return parts[0] ?? '{"name":"frame","total_us":0,"call_count":0,"parent_call_count":null,"effective_window_ms":0,"children":[]}';
}

// ─── Internals ────────────────────────────────────────────────────────────

function ensureChild(parent: NodeId, name: string): NodeId {
  const key = `${parent}:${name}`;
  const cached = childIndex.get(key);
  if (cached !== undefined) return cached;
  if (nodes.length >= MAX_NODES) return parent;
  const id = nodes.length;
  nodes.push({ name, parent, children: [], samples: [] });
  nodes[parent].children.push(id);
  childIndex.set(key, id);
  return id;
}

function recordSample(nodeId: NodeId, tsMs: number, durUs: number): void {
  const node = nodes[nodeId];
  if (node.samples.length >= SAMPLES_PER_NODE) {
    node.samples.shift();
  }
  node.samples.push([tsMs, durUs]);
}

function pruneAll(nowMsRel: number): void {
  const cutoff = nowMsRel - WINDOW_MS;
  for (const node of nodes) {
    while (node.samples.length > 0 && node.samples[0][0] < cutoff) {
      node.samples.shift();
    }
  }
}

function clearSamples(): void {
  for (const node of nodes) {
    node.samples.length = 0;
  }
}

function serializeNode(
  id: NodeId,
  nowMsRel: number,
  parentCallCount: number | null,
  output: string[],
): string {
  const node = nodes[id];
  const callCount = node.samples.length;
  const totalUs = node.samples.reduce((s, [, d]) => s + d, 0);
  const effectiveWindowMs =
    callCount > 0 ? Math.min(WINDOW_MS, nowMsRel - node.samples[0][0]) : 0;

  const childParts: string[] = [];
  for (const childId of node.children) {
    childParts.push(serializeNode(childId, nowMsRel, callCount, []));
  }

  const json =
    `{"name":${JSON.stringify(node.name)}` +
    `,"total_us":${totalUs}` +
    `,"call_count":${callCount}` +
    `,"parent_call_count":${parentCallCount === null ? "null" : parentCallCount}` +
    `,"effective_window_ms":${Math.max(0, Math.round(effectiveWindowMs))}` +
    `,"children":[${childParts.join(",")}]}`;

  output.push(json);
  return json;
}
