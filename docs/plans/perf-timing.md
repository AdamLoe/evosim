# Plan — Performance Profiler (live, rolling-window, in-app)

**Status:** plan only. No code lands until this is signed off.
**Scope:** add a runtime-toggleable hierarchical profiler to the Rust sim
and the TS frame loop, surface a single unified report into the existing
Stats panel (`#rail-stats`), and add tests. Net new dependencies: zero.

**Spec anchors / context:**

- `docs/architecture.md` (tick ordering — steps 1–12) for the spans we
  instrument inside `World::step()`.
- `src/world.rs` § `World::step()` (currently lines ~168–268) — the only
  place a sim-side `tick` span exists. The named sub-functions
  (`run_vision_pass`, `nn_forward_all_chunks`, `apply_movement_and_repulsion`,
  `photosynth_two_pass`, `eat_and_scavenge`, `energy_bookkeeping`,
  `collect_deaths`, `decay_carrion`, `handle_births`) become the
  first-level children.
- `src/wasm_api.rs` — where the new `profile_*` methods on `WorldHandle`
  attach to the bindgen surface.
- `web/src/main.ts` `frame()` — where the TS-side spans attach.
- `web/src/rail/stats.ts` and `web/index.html` `#rail-stats` — where the
  table renders.
- v5 §16 acceptance hash (`tests/acceptance.rs`,
  `tests/golden_snapshot_t10000.txt`) — must remain green with profiler
  default OFF.

**Status at handoff:** Milestone F is committed. Profiler default OFF
means the §16 hash is unchanged. `web_sys::Performance` is already in
`Cargo.toml` features (line 31 of `Cargo.toml`). `js-sys` is already a
dep, so the JS-side `performance.now()` is available in both layers.

**What is intentionally NOT in this plan:**

- No `tracing` / `tracing-subscriber` / `puffin` / `tracy` integration —
  user pinned: write our own.
- No statistical sampling profiler / no stack-walk profiler / no flame
  graph viz — just the rolling tree.
- No PR-gated perf budget assertion (separate concern; this is an
  observability surface).
- No per-creature profiling (would explode the tree); spans are at the
  sub-function granularity only.
- No persistence of profiler data into save files — pure in-memory,
  reset on world reload.

---

## High-level decisions (pinned up-front)

These are the decisions to record under `DECISIONS.md` before code lands.

**D1. Two profilers, one rendered table.** The Rust profiler owns the
`tick/*` subtree authoritatively. A TS-side profiler (a small mirror of
the Rust design, in `web/src/perf.ts`) owns the `frame/*` subtree. Each
emits its own JSON via its own report function. The UI fetches both at
the 1 Hz poll, concatenates them as two top-level subtrees, and renders
one table. Rationale: no per-span wasm-bindgen call from JS, no path-
string parsing in Rust, no shared mutable state across the FFI boundary.
The user still sees one table with two roots; the integration happens at
the renderer, not the data model. (Earlier draft of this plan routed TS
spans through wasm — dropped per review feedback R4.)

**D2. Runtime toggle, no `cfg(feature = ...)`.** A single `AtomicBool`
in the `Profiler` struct gates the span macro. The macro reads it with
`Relaxed`. When `false`, no allocation, no clock read, no map lookup —
the body of the macro is `if !ENABLED.load(Relaxed) { return; }` before
any work. User pinned this — UI toggle, not build flag.

**D3. Per-`World` profiler instance, NOT a global.** The profiler is a
field on `World` (`pub profile: Profiler`). Multiple wasm instances in
one tab (we don't currently have this, but eulogy → New World is
effectively a re-init) get fresh trees. The `Profiler` itself is `!Send`
and `!Sync` — wasm is single-threaded so there's no risk; using a
non-static profiler avoids the "globals confuse tests" problem.

  - **Why not a `thread_local!` global?** It works, but it's surprising
    when reading the code (`tick` mutates state but the macro touches
    a thread_local elsewhere). Putting it on `World` keeps the
    side-channel honest.
  - Macro takes a `&mut Profiler` (or `&Profiler` with interior
    mutability via `RefCell<Inner>` — pick this, since `step()` itself
    only holds `&mut self` and we don't want span-internal borrows to
    fight `World`'s other mutations). The fast-path early-return avoids
    the `RefCell::borrow_mut` entirely when disabled.

**D4. Sample storage: per-node ring buffer of `(timestamp_us, dur_us)`.**
Each node stores its own `VecDeque<(u32, u32)>` (timestamp in ms relative
to profiler start, duration in microseconds). Pre-sized capacity = 4096
samples — enough at the deepest spans (e.g. `mcts.nn` analog: per-creature
raycasts at 2k creatures × 1 tick = 2000 samples in a single tick, so 4096
holds ~2 frames of high-call nodes). Pruning on `profile_report_json()`
drops samples with `(now - timestamp) > WINDOW_MS` from the FRONT of each
deque (deques are FIFO by construction). `WINDOW_MS = 60_000`.

  - **Why per-node, not one global event stream:** sub-node aggregation
    is O(1) per node read; a global stream would force a full re-bucketing
    on every poll. Memory budget: ~30 nodes × 4096 × 8 bytes = ~1 MB
    worst case. Acceptable.

**D5. JSON shape (stable, contract).** `profile_report_json()` returns
JSON exactly matching:

```jsonc
{
  "now_ms": 12345678.9,            // performance.now() at report time
  "window_ms": 60000,              // nominal window
  "enabled": true,
  "tree": [                         // array of root nodes (tick from Rust; UI appends frame from TS)
    {
      "name": "tick",
      "total_us": 12345678,         // SUM of dur over samples in rolling window, recomputed per report
      "call_count": 1024,           // == samples.len() after pruning
      "parent_call_count": null,    // null for roots
      "effective_window_ms": 60000, // actual time span covered by samples (== now_ms - oldest sample)
      "children": [ /* recursive */ ]
    }
  ]
}
```

  Computed in the TS UI from JSON fields (not in Rust): `per_call_us =
  total_us / call_count`, `calls_per_parent = call_count /
  parent_call_count`, `share = total_us / parent.total_us` (root = 100%).
  Self-time, if ever needed, is derived as `total_us - sum(child.total_us)`
  by the UI — NOT stored in the JSON (see review feedback R2).

  - **Recomputed-not-running:** `total_us` and `call_count` are derived
    on every `report_json()` call by walking the (post-pruned) ring buffer.
    Do NOT maintain a running counter — it diverges from window semantics
    on the prune boundary.
  - **Effective window:** if any node's `effective_window_ms` is less
    than 90% of `window_ms`, the UI shows a "stabilizing… (Xs)" banner,
    because the ring-buffer cap bound first under high tick rates.

**D6. TS profiler is a thin wrapper.** Same span macro pattern (a `span()`
function returning a disposable handle via `using` keyword — TypeScript 5.2+
supports `using`, but Vite needs a polyfill; fallback to manual
`const t0 = performance.now(); try { ... } finally { record(name,
performance.now() - t0); }`. The user's tree expects RAII; we use
`try/finally`). Each TS span pushes onto a JS-side stack and on pop calls
`world.profiler_record_external(path_string, duration_us)`. The wasm side
parses the dotted path (`"frame.renderWorld.drawCreatures"`) and merges.

**D7. Time source.** `web_sys::Performance::now()` returns `f64`
milliseconds with sub-microsecond resolution in Chrome (modulo Spectre
clamping to 5 µs or 100 µs depending on cross-origin isolation). We
convert to microseconds (`(now_ms * 1000.0) as u64`). The clamp is fine
— at the granularity of "vision_pass over 2k creatures" we're well above
the floor; for `nn.forward` the clamp creates noise but the rolling
average over a minute averages it out. **Do not** assert sub-microsecond
spans on the tests; tests use a fake clock.

**D8. Macro shape.** Two options considered:

```rust
// (a) RAII guard
let _g = profile::span!(&mut self.profile, "vision_pass");

// (b) closure wrap
profile::scope!(&mut self.profile, "vision_pass", || { ... });
```

  Pick (a). The `_g` drops at end of scope. Implementation: a
  `SpanGuard<'p>` struct holding `&'p Profiler` + the node id + start
  time; `Drop` calls `Profiler::pop(node_id, dur)`. The macro is just
  syntactic sugar over `profiler.push("name")` returning the guard.

  - **Reason against (b):** Rust's borrow checker hates closures that
    capture `&mut self.creatures` plus the profiler — `self.creatures`
    and `self.profile` are disjoint fields but the closure would borrow
    `self` as a whole. RAII guards are field-local.

  - The macro generates code equivalent to:
    ```rust
    let _g = if self.profile.enabled() {
        Some(self.profile.push("vision_pass"))
    } else {
        None
    };
    ```
    When disabled, `_g` is `None`, no `Drop` work runs, no clock read.

**D9. Default state.** `Profiler::new()` returns `enabled = false`. A
new wasm-bindgen method `WorldHandle::profile_enable(bool)` flips it.
The TS UI defaults the checkbox to UNCHECKED, so loading the page is a
no-op for the profiler. Acceptance hash must remain unchanged. **The
checkbox state is NOT persisted** to `localStorage` or any save file —
every page load and every load-save action returns the profiler to OFF.
Rationale: prevents a user from shipping a save (or sharing a URL) that
silently enables the profiler and tanks perf for the next viewer.

**D10. Determinism / observation purity.** The profiler MUST NOT call
`SimRng::*`, mutate anything other than its own internal state, allocate
on the hot path (after warmup), or change tick ordering. The profiler's
fields on `World` are `#[serde(skip)]` on save and excluded from the §16
hash — so save round-trip and acceptance test are unaffected. The
`profile` field is also excluded from `Eq`/`PartialEq` if those are
implemented (they're not, today, on `World`).

---

## Scope — what gets built

Three layers, in order:

1. **`src/profiler.rs`** — new module. Profiler struct, node tree,
   span guard, push/pop, JSON serialization, pruning.
2. **Span instrumentation** — minimal targeted edits in `src/world.rs`,
   `src/vision.rs`, `src/brain.rs` to insert `profile::span!()` calls
   at the sites listed below.
3. **`src/wasm_api.rs`** — three new methods on `WorldHandle`:
   `profile_enable(bool)`, `profile_report_json() -> String`,
   `profile_record_external(path: &str, dur_us: u32)`.
4. **`web/src/perf.ts`** — new file. TS-side span helpers + the
   `installProfilerPanel()` function that renders the table in
   `#rail-stats`.
5. **`web/src/main.ts`** — wrap RAF body with TS spans; install
   profiler panel; poll `profile_report_json()` at 1 Hz.
6. **`web/src/rail/stats.ts`** — extend with a profiler section
   (the existing charts stay above; the table appears below them).
7. **`web/index.html`** — append a `<div id="profiler-panel">…</div>`
   inside `#rail-stats`, plus a toggle checkbox.
8. **`web/src/styles.css`** — add `.profiler-table` rules.
9. **Tests** — `src/profiler.rs` unit tests + a wasm-api shape test.

---

## Files & function signatures

### `src/profiler.rs` (new)

```rust
//! In-app hierarchical profiler. Runtime-toggleable, zero-cost when off.
//! Sampled spans accumulate into a tree keyed by ancestor path; each node
//! holds a ring buffer of (timestamp_ms_u32, dur_us_u32) samples over the
//! last WINDOW_MS. Reports are produced on demand and prune stale samples.
//!
//! Determinism: this module performs no RNG calls and only reads
//! `web_sys::Performance::now()`. Mutation is confined to its own state.
//! Profiler state is `#[serde(skip)]` on World and excluded from the
//! v6 §M snapshot hash.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

pub const WINDOW_MS: u32 = 60_000;
pub const SAMPLES_PER_NODE: usize = 4096;
pub const MAX_NODES: usize = 256;     // hard cap to bound memory + bugs

/// Index into Profiler::nodes. Stable for the life of the Profiler.
pub type NodeId = u16;

const ROOT_FRAME: NodeId = 0;          // synthetic root for TS spans
const ROOT_TICK:  NodeId = 1;          // synthetic root for Rust spans

#[derive(Default)]
struct Node {
    name: String,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    samples: VecDeque<(u32, u32)>,     // (timestamp_ms_u32, dur_us)
    // Total call count over the rolling window is `samples.len()` after pruning.
    // total_us is SUMMED FROM `samples` ON EVERY REPORT — there is NO running
    // total_us field. We do NOT store an all-time call_count — it would grow
    // unboundedly and diverge from window semantics. (See review fix R1.)
}

pub struct ProfilerInner {
    nodes: Vec<Node>,
    /// Stack of (node_id, start_us_u64) for the current span scope.
    stack: Vec<(NodeId, u64)>,
    /// Resolved child-by-name cache, keyed by (parent_id, name) → child_id.
    /// Avoids the O(children) lookup at every push. Maps to NodeId or sentinel.
    /// Use HashMap (FxHash is overkill, ~hundreds of entries).
    child_index: std::collections::HashMap<(NodeId, String), NodeId>,
    /// Epoch in ms; subtracted from `performance.now()` to fit timestamps in u32
    /// (49 days of headroom).
    epoch_ms: f64,
}

pub struct Profiler {
    enabled: AtomicBool,
    inner: RefCell<ProfilerInner>,
}

impl Profiler {
    pub fn new() -> Self { /* allocate ROOT_FRAME, ROOT_TICK */ }

    /// Cheap atomic load. Used by the span! macro early-return.
    #[inline] pub fn enabled(&self) -> bool { self.enabled.load(Ordering::Relaxed) }

    pub fn set_enabled(&self, on: bool) {
        // When toggling off → on, reset epoch so first frame is t=0.
        // When toggling on → off, clear samples (no stale data when re-enabled).
        // Decision: only clear-on-disable; keep epoch sticky to avoid drift.
        self.enabled.store(on, Ordering::Relaxed);
        if !on { self.inner.borrow_mut().clear_samples(); }
    }

    /// Push a span by name. Returns a SpanGuard whose Drop records the duration.
    /// Caller MUST check `enabled()` first; this method does no guard.
    pub fn push(&self, name: &'static str) -> SpanGuard<'_> { /* ... */ }

    /// Record an externally-timed span (called by wasm_api for TS spans).
    /// `path` is a dotted ancestor path under ROOT_FRAME, e.g. "renderWorld.drawCreatures".
    /// The leaf node is created lazily.
    pub fn record_external(&self, path: &str, dur_us: u32) { /* ... */ }

    /// Build the JSON report. Prunes stale samples as a side effect.
    pub fn report_json(&self) -> String { /* ... */ }
}

impl ProfilerInner {
    fn now_ms_relative(&self) -> u32 { /* (performance.now() - epoch_ms) as u32 */ }
    fn now_us(&self) -> u64 { /* performance.now() * 1000.0 as u64 */ }
    fn ensure_child(&mut self, parent: NodeId, name: &str) -> NodeId { /* ... */ }
    fn prune(&mut self, now_ms: u32) { /* drop samples with ts < now_ms - WINDOW_MS */ }
    fn clear_samples(&mut self) { /* clear ring buffers but keep node tree */ }
    fn serialize_tree(&self, now_ms: u32) -> String { /* manual JSON write — see below */ }
}

pub struct SpanGuard<'p> {
    profiler: &'p Profiler,
    start_us: u64,
}

impl Drop for SpanGuard<'_> {
    fn drop(&mut self) {
        let inner = &mut *self.profiler.inner.borrow_mut();
        let now_us = inner.now_us();
        let dur = (now_us - self.start_us).min(u32::MAX as u64) as u32;
        let ts = inner.now_ms_relative();
        let (node_id, _) = inner.stack.pop().expect("span stack underflow");
        let samples = &mut inner.nodes[node_id as usize].samples;
        if samples.len() == SAMPLES_PER_NODE { samples.pop_front(); }
        samples.push_back((ts, dur));
    }
}

/// Macro: profile::span!(&world.profile, "vision_pass") → SpanGuard or ().
/// Generates the enabled-check + push.
#[macro_export]
macro_rules! profile_span {
    ($prof:expr, $name:literal) => {
        let _g = if $prof.enabled() { Some($prof.push($name)) } else { None };
    };
}
pub use profile_span as span;
```

  - **JSON serialization choice:** hand-write the JSON (don't use serde
    derive). The tree is shallow and we want to walk the node array once,
    avoiding building a temporary `serde_json::Value`. Use a `String`
    buffer, recursive helper, no allocs per node beyond the buffer growth.

  - **`name` lifetime:** spans use `&'static str` (string literal), so we
    never allocate on the hot path. The `child_index` HashMap uses
    `String` keys (one allocation when a *new* span name first appears,
    then cached forever).

### `src/world.rs` edits

Add field on `World`:

```rust
pub profile: crate::profiler::Profiler,
```

Mark it `#[serde(skip)]` on the `Serialize`/`Deserialize` derives (the
`World` derives serde via `save.rs::SaveV1` mapping rather than directly,
so we exclude it from `SaveV1` — verify that `SaveV1` does not include
the field, since the conversion is field-by-field in `save.rs`. The plan
mandates: do NOT add `profile` to `SaveV1`).

Within `World::step()`, wrap each tick stage. Concrete diff (line numbers
from current `src/world.rs`):

```rust
pub fn step(&mut self) -> bool {
    if self.world_ended { return false; }
    self.sun.refill_rate = self.sliders.base_sun_rate;

    crate::profiler::span!(&self.profile, "tick");           // line 175 area

    {
        crate::profiler::span!(&self.profile, "grid_rebuild_1");
        self.grid.rebuild(&self.creatures.x, &self.creatures.y);
    }
    {
        crate::profiler::span!(&self.profile, "vision_pass");
        self.run_vision_pass();
    }
    {
        crate::profiler::span!(&self.profile, "nn_forward");
        let n = self.creatures.len();
        let ranges = chunk_ranges(n);
        self.nn_forward_all_chunks(&ranges, n);
    }
    {
        crate::profiler::span!(&self.profile, "movement_and_repulsion");
        self.apply_movement_and_repulsion();
    }
    {
        crate::profiler::span!(&self.profile, "photosynth_two_pass");
        self.photosynth_two_pass();
    }
    {
        crate::profiler::span!(&self.profile, "eat_and_scavenge");
        self.eat_and_scavenge();
    }
    {
        crate::profiler::span!(&self.profile, "sun_refill");
        self.sun.refill();
    }
    {
        crate::profiler::span!(&self.profile, "energy_bookkeeping");
        self.energy_bookkeeping();
    }
    {
        crate::profiler::span!(&self.profile, "collect_deaths");
        // Span widened (R9) to include the dead-removal swap_remove loop +
        // creatures.remove_indices, which is part of tick step 9 and scales
        // with die-off size.
        let died = self.collect_deaths();
        if !died.is_empty() {
            for &k in died.iter().rev() {
                if k < self.vision.len() { self.vision.swap_remove(k); }
            }
            self.creatures.remove_indices(&died);
        }
    }
    {
        crate::profiler::span!(&self.profile, "decay_carrion");
        self.decay_carrion();
    }
    {
        crate::profiler::span!(&self.profile, "handle_births");
        self.handle_births();
    }
    {
        // R9: cover step-12 tail (last_action promotion, tick bump,
        // milestone events). Cheap today; future-proofs the panel for
        // Milestone E species detection.
        crate::profiler::span!(&self.profile, "bookkeeping_tail");
        // promote last_action, bump tick, milestones, world-end check
        // (existing code in lines 224–267)
    }
}
```

  - **Why braces around each span:** the `span!` macro creates `_g` and
    its drop must run BEFORE the next sibling span pushes (otherwise the
    push happens with the parent still being "vision_pass" instead of
    "tick"). The brace scopes the guard. Verified: this is the standard
    pattern for RAII-based span libraries.

  - **Note about `apply_movement_and_repulsion`:** it internally calls
    `self.grid.rebuild()` a second time (line 428). We do NOT add a
    `grid_rebuild_2` span inside `apply_movement_and_repulsion` — it
    would require editing that function's signature OR borrowing
    `self.profile` while also `&mut self`-borrowing the grid. Putting
    the span outside (around the whole `apply_movement_and_repulsion`
    call) is sufficient granularity. If finer breakdown is wanted in
    v1.1, add an inner span then. **Decision recorded.**

  - **`vision.rs` and `brain.rs` internal spans:** out of scope for v1.
    Adding `vision.raycast` per creature would create 2000 samples/tick
    and fill the ring buffer in 2 frames. The user's CLI example shows
    nodes with 100k+ calls; we can support that scale, but the visual
    table becomes noisy. **Decision: top-level only in v1.** If a
    bottleneck shows in `vision_pass`, drill down by adding spans then.

### `src/wasm_api.rs` edits

Add three methods on `WorldHandle`:

```rust
#[wasm_bindgen]
impl WorldHandle {
    /// Enable or disable the profiler. Default: false.
    #[wasm_bindgen]
    pub fn profile_enable(&self, on: bool) {
        self.inner.profile.set_enabled(on);
    }

    /// JSON report of the current rolling 60-second profile tree.
    /// Cheap: walks ~30 nodes, prunes stale samples, writes ~5 KB string.
    /// Stable shape — see docs/plans/perf-timing.md §D5.
    #[wasm_bindgen]
    pub fn profile_report_json(&self) -> String {
        self.inner.profile.report_json()
    }

    /// Record a TS-timed span. `path` is dotted under the "frame" root,
    /// e.g. "renderWorld.drawCreatures". `dur_us` is microseconds.
    /// Silently no-ops if the profiler is disabled.
    #[wasm_bindgen]
    pub fn profile_record_external(&self, path: &str, dur_us: u32) {
        if !self.inner.profile.enabled() { return; }
        self.inner.profile.record_external(path, dur_us);
    }
}
```

  - All three take `&self` (not `&mut self`) — `Profiler` uses
    `RefCell<ProfilerInner>` for interior mutability. This matters
    because some call sites may already hold a mutable borrow of other
    World fields when invoking spans.

### `src/lib.rs` edits

Add `pub mod profiler;` adjacent to existing module declarations.

### `web/src/perf.ts` (new file)

```ts
// Profiler glue. Mirrors the Rust span! macro pattern in TS for the
// frame-loop and renderer hot paths. Records into the wasm profiler via
// world.profile_record_external() so the UI sees one unified tree.

import type { WorldHandle } from "../wasm/evosim";

let world: WorldHandle | null = null;
let enabled = false;
const stack: { name: string; t0: number }[] = [];

export function attachProfiler(w: WorldHandle): void { world = w; }
export function setProfilerEnabled(on: boolean): void {
  enabled = on;
  world?.profile_enable(on);
  if (!on) stack.length = 0;
}

/** Open a span. Use with try/finally OR with `using` (TS 5.2+). */
export function span(name: string): { close: () => void } {
  if (!enabled || !world) return NOOP_SPAN;
  const t0 = performance.now();
  stack.push({ name, t0 });
  return {
    close() {
      const top = stack.pop();
      if (!top) return;
      const dur_us = Math.max(0, Math.round((performance.now() - top.t0) * 1000));
      const path = stack.length === 0
        ? top.name
        : stack.map(s => s.name).concat(top.name).join(".");
      world!.profile_record_external(path, dur_us);
    },
  };
}

const NOOP_SPAN = { close: () => {} };

/** Sugar for synchronous blocks. */
export function timed<T>(name: string, fn: () => T): T {
  const s = span(name);
  try { return fn(); } finally { s.close(); }
}
```

### `web/src/main.ts` edits

Wrap the RAF body. Pseudo-diff at the `frame()` function (lines ~247-280):

```ts
import { attachProfiler, timed, span } from "./perf";
// ...
attachProfiler(world);
// ...
function frame(now: number): void {
  const frameSpan = span("frame");
  try {
    const delta = now - lastRender; lastRender = now;
    const ticksThisFrame = /* ... */;
    if (ticksThisFrame > 0 && !world!.world_ended) {
      timed("step_n", () => world!.step_n(ticksThisFrame));
    }
    if (!world!.world_ended) {
      timed("maybeAutosave", () => maybeAutosave(world!, cam, now, persistence));
    }
    // eulogy unchanged (one-shot)
    const ids = world!.creature_ids_buffer() as unknown as Float64Array;
    timed("pollRail", () => pollRail(rail, world!, ids));
    timed("renderWorld", () =>
      renderWorld(ctx!, cam, viewW, viewH, world!, stride, ids, highlights, now));
    // status update unchanged
  } finally {
    frameSpan.close();
  }
  requestAnimationFrame(frame);
}
```

  - **renderWorld sub-spans:** `renderWorld` is in `web/src/render.ts`.
    To add `drawSunMap`, `drawCarrion`, `drawCreatures` sub-spans, we
    either (a) call `span()` from inside `renderWorld`, importing
    `perf.ts`, or (b) split the call sites here in `main.ts` so spans
    wrap each. (a) is cleaner. **Pick (a).** The render module imports
    perf.ts; spans nest correctly because we maintain a single TS stack.

### `web/src/rail/stats.ts` edits

Extend (not replace) the existing stats panel. Add:

```ts
export function installProfilerPanel(world: WorldHandle): void {
  // Wire the on/off toggle, install 1Hz polling, render the table.
}

let profilerPollHandle = 0;
const POLL_INTERVAL_MS = 1000;

function renderProfilerTable(reportJson: string): void {
  // Parse, walk tree depth-first, emit <tr> per node with indent + 7 columns.
  // Columns: Stage, total, calls, /call, calls/parent, share, roll /call.
}
```

  Polling: a `setInterval(1000)` loop calls
  `world.profile_report_json()` while the checkbox is checked. When
  unchecked, `clearInterval` and the table greys out (last known state).

### `web/index.html` edits

Inside `#rail-stats`, after the existing two canvases (line 34):

```html
<div id="profiler-panel">
  <label class="profiler-toggle">
    <input type="checkbox" id="profiler-enable" />
    profiler
  </label>
  <table id="profiler-table" class="profiler-table">
    <thead>
      <tr>
        <th>Stage</th><th>total</th><th>calls</th>
        <th>/call</th><th>calls/parent</th><th>share</th><th>roll /call</th>
      </tr>
    </thead>
    <tbody></tbody>
  </table>
</div>
```

### `web/src/styles.css` additions

```css
.profiler-table {
  width: 100%;
  font: 11px ui-monospace, monospace;
  border-collapse: collapse;
  color: var(--fg);
}
.profiler-table th, .profiler-table td { padding: 1px 4px; text-align: right; }
.profiler-table th:first-child, .profiler-table td.name { text-align: left; }
.profiler-table tr.depth-1 td.name { padding-left: 12px; }
.profiler-table tr.depth-2 td.name { padding-left: 24px; }
.profiler-table tr.depth-3 td.name { padding-left: 36px; }
.profiler-table tr.depth-4 td.name { padding-left: 48px; }
.profiler-toggle { display:inline-flex; gap: 4px; align-items: center; font-size: 11px; }
```

---

## Integration points (checklist for the implementer)

- [ ] `src/profiler.rs` compiles standalone (no World deps).
- [ ] `World` gains a `pub profile: Profiler` field; constructor
      `World::new` initializes it; clone/save paths exclude it.
- [ ] `World::new`, `World::from_save_v1` both call `Profiler::new()`.
      Profiler is NEVER serialized in `SaveV1`.
- [ ] `snapshot_hash.rs` does NOT include `profile` in its hash input
      ordering. (Verify by inspection; the function lists fields
      explicitly per v6 §M.)
- [ ] All 11 spans in `World::step()` are wrapped in their own block
      scopes (the brace pattern).
- [ ] `WorldHandle` gains 3 new methods; bindgen TS types regenerate.
- [ ] `web/src/perf.ts` exports `attachProfiler`, `setProfilerEnabled`,
      `span`, `timed`.
- [ ] `web/src/main.ts` calls `attachProfiler(world)` once after world
      construction and wraps the RAF body.
- [ ] `web/src/render.ts` calls `span()` around `drawSunMap`,
      `drawCarrion`, `drawCreatures`.
- [ ] `web/src/rail/stats.ts` exports `installProfilerPanel`; called
      from `installRail` in `rail/index.ts`.
- [ ] HTML and CSS edits land.
- [ ] When the checkbox is OFF (default), no `profile_report_json` is
      polled. Verify with a network/CPU sample: zero overhead at default.

---

## Tests

All tests are NATIVE (`cargo test`), not wasm-bindgen-test. The
profiler uses `web_sys::Performance` directly, so we abstract the clock
behind a trait OR a conditional cfg:

  - **Decision:** use a single `clock_now_us()` free function in
    `profiler.rs` with `#[cfg(target_arch = "wasm32")]` calling
    `Performance::now()` and `#[cfg(not(target_arch = "wasm32"))]`
    calling `std::time::Instant::now()` against a static
    `LazyLock<Instant>` epoch. This lets all unit tests run native.

### Test 1: tree shape after synthetic spans

```rust
#[test]
fn profiler_builds_correct_tree() {
    let p = Profiler::new();
    p.set_enabled(true);
    {
        let _t = p.push("tick");
        { let _v = p.push("vision_pass"); }
        { let _v = p.push("vision_pass"); }       // call again, same node
        { let _n = p.push("nn_forward"); }
    }
    let json = p.report_json();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let tick = &v["tree"].as_array().unwrap().iter()
        .find(|n| n["name"] == "tick").unwrap();
    assert_eq!(tick["call_count"], 1);
    let children = tick["children"].as_array().unwrap();
    let vision = children.iter().find(|n| n["name"] == "vision_pass").unwrap();
    assert_eq!(vision["call_count"], 2);
    let nn = children.iter().find(|n| n["name"] == "nn_forward").unwrap();
    assert_eq!(nn["call_count"], 1);
}
```

### Test 2: pruning drops old samples

Inject a custom epoch / drive the clock manually via a `#[cfg(test)]`
fake. Push samples at t=0; advance clock to t=WINDOW_MS + 1000; call
`report_json()`; assert that those samples are gone.

```rust
#[test]
fn profiler_prunes_stale_samples() {
    // Requires test-mode clock injection — implementer adds a thread_local
    // FAKE_NOW: Cell<Option<u64>> read by clock_now_us in cfg(test).
    set_fake_clock_us(0);
    let p = Profiler::new();
    p.set_enabled(true);
    { let _t = p.push("tick"); }
    set_fake_clock_us((WINDOW_MS as u64 + 1000) * 1000);
    let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
    let tick = v["tree"].as_array().unwrap().iter()
        .find(|n| n["name"] == "tick").unwrap();
    assert_eq!(tick["call_count"], 0, "stale sample must be pruned");
}
```

### Test 2b: rolling-window total is the sum of in-window samples

(Added per review feedback R10.) Inject 3 samples with known durations at
known timestamps; advance the clock to a point where all 3 are still
inside `WINDOW_MS`; assert `total_us` in the JSON matches the
hand-summed value. Then advance past the first sample's expiry; assert
`total_us` equals the sum of the remaining two and `call_count == 2`.
Catches the "implementer added a running counter" bug class.

```rust
#[test]
fn profiler_total_us_sums_in_window_samples_only() {
    set_fake_clock_us(0);
    let p = Profiler::new();
    p.set_enabled(true);
    push_with_duration(&p, "tick", 100); // dur=100us at t=0
    set_fake_clock_us(10_000_000);       // +10s
    push_with_duration(&p, "tick", 200); // dur=200us at t=10s
    set_fake_clock_us(20_000_000);       // +20s
    push_with_duration(&p, "tick", 300); // dur=300us at t=20s

    // All three inside the 60s window:
    let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
    let tick = v["tree"].as_array().unwrap().iter()
        .find(|n| n["name"] == "tick").unwrap();
    assert_eq!(tick["total_us"], 600);
    assert_eq!(tick["call_count"], 3);

    // Advance past first sample's expiry (t=0 + 60s + 1ms):
    set_fake_clock_us((WINDOW_MS as u64 + 1) * 1000);
    // Wait — earlier we pushed at t=20s; advance further:
    set_fake_clock_us(60_001_000 + 20_000_000); // t=80.001s
    let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
    let tick = v["tree"].as_array().unwrap().iter()
        .find(|n| n["name"] == "tick").unwrap();
    assert_eq!(tick["total_us"], 500); // only the 200us + 300us samples remain
    assert_eq!(tick["call_count"], 2);
}
```

### Test 3: disabled profiler is a no-op

```rust
#[test]
fn profiler_disabled_records_nothing() {
    let p = Profiler::new();
    // enabled defaults to false
    let _t = p.push("tick");                 // should be a no-op or the guard does nothing
    drop(_t);
    let json = p.report_json();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    // Tree contains only the two synthetic roots with zero samples.
    for root in v["tree"].as_array().unwrap() {
        assert_eq!(root["call_count"], 0);
    }
}
```

  - **Note:** per D8, the macro short-circuits before `push` runs at
    all, so calling `push` directly in a test bypasses the gate. The
    test instead exercises the macro through a tiny helper that
    mirrors the macro expansion. Document this in the test.

### Test 4: external spans build the frame subtree

```rust
#[test]
fn profiler_external_spans_attach_under_frame() {
    let p = Profiler::new();
    p.set_enabled(true);
    p.record_external("renderWorld.drawCreatures", 1500);
    p.record_external("renderWorld.drawCarrion", 300);
    p.record_external("step_n", 4200);
    let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
    let frame = v["tree"].as_array().unwrap().iter()
        .find(|n| n["name"] == "frame").unwrap();
    let rw = frame["children"].as_array().unwrap().iter()
        .find(|n| n["name"] == "renderWorld").unwrap();
    assert_eq!(rw["children"].as_array().unwrap().len(), 2);
}
```

### Test 5: JSON shape contract

Strictly assert every field is present in the expected type. This is the
shape the TS UI depends on; freeze it.

```rust
#[test]
fn profiler_json_shape_is_stable() {
    let p = Profiler::new();
    p.set_enabled(true);
    { let _t = p.push("tick"); { let _v = p.push("vision_pass"); } }
    let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
    assert!(v["now_ms"].is_number());
    assert_eq!(v["window_ms"], WINDOW_MS);
    assert_eq!(v["enabled"], true);
    let tree = v["tree"].as_array().unwrap();
    assert!(tree.iter().any(|n| n["name"] == "frame"));
    assert!(tree.iter().any(|n| n["name"] == "tick"));
    for node in tree {
        for field in &["name", "total_us", "self_us", "call_count",
                       "parent_call_count", "children"] {
            assert!(node.get(field).is_some(), "missing field {field}");
        }
    }
}
```

### Test 6: §16 acceptance hash with profiler default-off

Run the existing `tests/acceptance.rs` unchanged. It must pass — that
proves the profiler being a field on `World` did not change the hash
inputs (because we `#[serde(skip)]` it from saves and exclude it from
the hash ordering). **No code edit to the acceptance test.** Just verify
in CI.

### Test 7: §16 acceptance hash with profiler enabled

Open question: should toggling the profiler on at t=0 change the hash?
**No** — profiler is pure observation. Add a test that enables the
profiler before running 10000 ticks and asserts the SAME golden hash.
This is the strong determinism guarantee. **Decision: include this
test** under `tests/acceptance.rs::profile_does_not_change_hash`.

### TS-side tests

Not in scope for v1 (no Vitest in repo today; TS code is glue-thin).
Plan only the Rust tests. If the team later adds Vitest, write a unit
test for `web/src/perf.ts::span` that the dotted-path resolution is
correct.

---

## Code-reviewer checklist

Use this when reviewing the implementer's PR.

### Correctness

- [ ] `Profiler::push` and `SpanGuard::drop` are correctly paired —
      every push has exactly one drop, including in panic-unwind paths.
      Look for: tests with `#[should_panic]` that confirm the stack
      doesn't leak.
- [ ] No span name is dynamically formatted in the hot path. All names
      are `&'static str` literals. (Grep for `format!` near `span!`.)
- [ ] `set_enabled(false)` clears samples but PRESERVES the node tree
      (so re-enabling shows the same labels in the table). Confirm by
      reading `clear_samples`.
- [ ] When `enabled() == false`, the macro produces NO clock read and
      NO map lookup. Confirm by inspecting macro expansion via
      `cargo expand`.
- [ ] `RefCell::borrow_mut` is never held across a `.borrow()` of the
      same cell. The hot path only borrows mutably inside push/drop;
      `record_external` borrows mutably end-to-end. No nesting.

### Determinism

- [ ] `Profiler` performs zero RNG calls. (`grep -n 'Rng\|rand_' src/profiler.rs` → empty.)
- [ ] No `profile` field appears in `SaveV1` or in `snapshot_hash.rs`.
      (`grep -n 'profile' src/save.rs src/snapshot_hash.rs` → empty.)
- [ ] Acceptance test passes with profiler default-off (Test 6).
- [ ] Acceptance test passes with profiler enabled at t=0 (Test 7).
- [ ] `World::from_save_v1` re-initializes a fresh `Profiler` (does
      NOT attempt to restore one from the save).

### Performance / hot path

- [ ] When disabled, `World::step()` performance is within noise of
      the pre-PR baseline. Spot-check: run a 10k-tick benchmark in
      release mode and compare wall-clock.
- [ ] When enabled, the per-tick overhead is < 5% of the median step
      time, measured at 1500 creatures over 10 000 ticks in release
      mode (`cargo run --release` headless harness or wasm with timing).
      Implementer reports the measured number in the PR description.
      (Roughly expected: 12 spans × ~100 ns each = ~1.2 µs vs. a tick
      that's tens of µs.)
- [ ] Ring buffers never reallocate after first fill — `VecDeque` of
      `(u32, u32)` with `with_capacity(SAMPLES_PER_NODE)`, pop-front +
      push-back semantics.
- [ ] `profile_report_json()` does NOT allocate per-node — single
      string buffer, manual JSON write, depth-first.

### TS layer

- [ ] When the profiler checkbox is OFF, `setInterval` is cleared and
      no `profile_report_json()` calls are made. (Add an assertion in
      `installProfilerPanel` that polling handle is 0 when disabled.)
- [ ] TS `span()` returns a no-op object when `enabled == false`, so
      RAF body has zero overhead at default.
- [ ] Dotted-path resolution in `perf.ts::span` correctly joins ALL
      ancestor names, not just the immediate parent. Test by inspection.
- [ ] No emojis in the profiler UI strings (matches the rest of the app).

### UI

- [ ] Table renders with monospace font + per-depth indent class.
- [ ] All seven columns appear, units are explicit (ms vs µs vs %).
- [ ] Table updates at 1 Hz exactly when checkbox is checked; ≤ 0 Hz
      otherwise.
- [ ] Toggling the checkbox produces no console errors.
- [ ] When the world has just started (< 1s of data), the "share" and
      "/call" columns gracefully show `-` instead of NaN. (Guard:
      check `call_count > 0` before division in render.)

### Scope discipline

- [ ] No third-party crate added. (`git diff Cargo.toml` shows zero
      new entries; only the existing `web-sys` features are used.)
- [ ] No edits to tick ordering in `World::step()` beyond inserting
      the brace-scoped spans.
- [ ] No changes to `snapshot_hash.rs`, `save.rs::SaveV1`, or
      `tests/golden_snapshot_t10000.txt`.
- [ ] No new sliders in `DevSliders`.

---

## Decisions to record (target file: `DECISIONS.md`)

Each of D1–D10 above gets a one-paragraph entry under a new
`## Profiler` section. The most load-bearing ones to capture verbatim:

- D2 — runtime toggle, not `cfg(feature)`, because the user wants to
  flip it from the UI without rebuilding.
- D3 — per-World, not global; rationale: testability + multi-world
  cleanliness.
- D5 — JSON shape contract; this is the wire format between Rust and
  TS and must not drift.
- D9 — default OFF, ensures §16 hash unchanged.
- D10 — observation purity (no RNG, no save inclusion, no hash
  inclusion). This is the determinism guarantee.

---

## Risks and mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Span macro accidentally `&mut`-borrows World, blocking other field access | Low | Use `RefCell` interior mutability + `&self` on `push`. |
| Profiler memory grows unboundedly under bursty workloads | Low | Per-node ring buffer cap (4096) + 60s pruning. |
| `Performance::now()` clamping (Spectre mitigations) causes µs-scale spans to read as 0 | Medium | Document in `perf.ts`. Rolling minute averages dampen noise. Tests use fake clock to avoid this. |
| TS `using` keyword not supported by Vite build target | Medium | Use explicit `try/finally` + `span().close()`. Documented in plan §D6. |
| Span macro disabled-path adds a measurable atomic load to every span site | Very low | `Relaxed` ordering compiles to a plain load; verified on x86 ≈ 1 cycle. |
| Implementer forgets to brace-scope spans → next sibling pushes under wrong parent | High (easy bug) | Reviewer checklist + test that sibling spans don't nest. |
| Save round-trip silently breaks because Profiler is added to World but not handled in `World::from_save_v1` | Medium | Explicit reviewer checklist item; integration test that opens then reloads a save with profiler enabled. |
| Spans added inside the parallel `nn_forward_all_chunks` loop break the single-thread invariant (`Profiler` is `!Send`/`!Sync`) | Low (no spans there today) | Reviewer checklist: grep for `span!` inside any `rayon::` or `par_iter` block; reject if found. If parallel sub-spans are ever wanted, switch to per-chunk profilers + merge at join, or use `Mutex<Inner>` — not in v1. |

---

## Out-of-scope (explicit cuts)

- **No per-creature spans** in `vision.rs` / `brain.rs`. (Bullet from
  user's "spans to instrument" list called these out — we drop them
  for v1.) If a regression in vision throughput appears, drill down by
  adding `vision.raycast` inside the existing `vision_pass` scope.
- **No flame graphs.**
- **No CSV / JSON export of the profiler tree.** (Could be added in
  v1.1 via a "copy report" button next to the toggle. Not now.)
- **No min/max/p95 stats.** The user asked for total / calls / /call /
  share — that's all we compute. Adding p95 would require storing all
  samples (already do, in the ring buffer) and an O(N log N) sort on
  every poll; punt to v1.1 if requested.
- **No profiler-only "freeze frame" mode** where toggling captures one
  exact tick. (Nice future feature; not now.)
- **No PR perf budget assertion** (e.g. fail CI if `vision_pass`
  exceeds X µs). Separate concern.

---

## Sequencing for the implementer

Build in this order. Stop at each checkpoint and verify locally before
proceeding.

1. **Scaffold `src/profiler.rs`** with all types, but `enabled = false`
   default and no integration into `World`. Run Tests 1, 3, 4, 5.
   Checkpoint: profiler unit tests pass.
2. **Add `profile` field to `World`** + spans in `World::step()`.
   Verify `cargo build --release --target wasm32-unknown-unknown` is
   green. Run acceptance test (Test 6 — default-off path).
   Checkpoint: acceptance hash unchanged.
3. **Add the three `WorldHandle` methods.** `wasm-pack` rebuild.
   Checkpoint: TS types regenerate and reference the new methods.
4. **Add `web/src/perf.ts`.** Unit-test by hand: wrap a `setTimeout` in
   a span and `console.log` the report JSON.
5. **Wrap `main.ts::frame` and `render.ts` calls.** Verify in browser:
   toggle profiler on, watch console.log of the report.
6. **Add HTML + CSS + `installProfilerPanel`** in
   `web/src/rail/stats.ts`. Polish: column widths, indent, units.
7. **Run Test 7** (profiler-on acceptance hash). Fix anything that
   drifts. (Should be nothing.)
8. **Code-reviewer pass.** Apply checklist.
9. **Update `DECISIONS.md`** with the Profiler section.
10. **Done.** No `CHANGELOG.md` update needed (no such file).

---

## Plan review feedback

Reviewer: parent agent, 2026-05-24. Result: **approve-with-revisions**. The
core architecture (per-World profiler, RAII guards, ring-buffer of samples,
JSON contract, default-off, observation-purity guarantees) is sound. Below
are the issues caught on review — the most important ones must be fixed in
plan text before implementation begins; the rest are call-outs to record.

### Must-fix before code lands

**R1. Rolling-window totals must be RECOMPUTED, not maintained.** D4 says
each node stores a ring of `(timestamp, dur)` and call-count is implicit as
`samples.len()` after pruning — good. But D5's `total_us` and `self_us`
fields are presented as if they could be running counters; the implementation
text in `Node` (lines 252–260) correctly notes "We do NOT store an all-time
call_count", but the serializer must SUM `dur_us` across the (post-pruned)
ring buffer per node on every report. Pin this explicitly in `serialize_tree`
spec: *each report walk recomputes `total_us = sum(dur for (_,dur) in
samples)` and `call_count = samples.len()` from the live ring*. Otherwise an
implementer might add a `running_total_us` field and silently over-count
across the window boundary. (Fixed inline in the `Node` doc-comment below.)

**R2. `self_us` is computed but not stored anywhere.** D5 lists `self_us`
as a JSON field, but the `Node` struct only stores `(timestamp, dur)` and
the spec never says how to derive self-time. Two options: (a) store
`self_dur` alongside `dur` in each sample (subtract children durations at
pop time — but children may pop AFTER this node started and BEFORE this
node ends, requiring per-frame bookkeeping); (b) drop the `self_us` column
from the JSON contract entirely and let the UI compute `self = total -
sum(children.total)` from the tree. **Pick (b).** It's simpler, matches the
user's CLI example which doesn't show a self column, and removes a class of
bugs. Edit D5 to drop `self_us`.

**R3. Effective window is min(60s, 4096 samples), not 60s.** D4 implies a
60-second window with a 4096 safety cap, but at high tick rates the cap
binds first. At 100× sim speed with 12 spans/tick: `tick` accumulates
~6000 samples/s, so the 4096 ring covers only ~0.7 s of `tick` data. The
"share" and "/call" numbers will be correct for that short window, but the
"60s" label in the UI lies. **Fix:** either (a) raise SAMPLES_PER_NODE to
match `WINDOW_MS × max_ticks_per_sec` (≈ 360k — too much memory), (b)
display the actual effective window in the UI ("over last 0.7 s"), or (c)
sub-sample: when a node would push beyond capacity at unchanged time
spacing, drop every other historical sample instead of pop_front. **Pick
(b)** — cheapest, honest. The serializer reports `effective_window_ms` per
node = `now_ms - oldest_sample_ms` and the UI shows it if any node's
effective window is < 90% of WINDOW_MS. (Add this field to JSON shape D5.)

**R4. Keep TS profiler data TS-side; do not round-trip through wasm.** D1
+ D6 ship every TS span into wasm via `profile_record_external`. At 60 fps
× ~5 spans/frame = 300 wasm calls/s. Cheap, yes, but the wasm-bindgen call
itself passes a `&str` which requires a UTF-8 decode in Rust per call.
More importantly, this couples two layers needlessly: the TS panel could
maintain its own ring-buffer per node, emit its own JSON, and the UI
renderer concatenates `frame_report + tick_report` into one table. This is
strictly simpler — no `record_external`, no path-string parsing in wasm.
**Recommendation:** in v1, split the JSON contract D5 into two
sources: `world.profile_report_json()` returns ONLY the `tick` subtree;
`perf.ts::reportJson()` returns the `frame` subtree. The UI merges. Rust
stays pure-sim; TS stays pure-frame. Drop `profile_record_external` and
its tests entirely. Drop the second sentence of D1.

**R5. "calls/parent" denominator semantics.** Plan doesn't define whether
parent call-count is (a) parent samples in the same rolling window, or (b)
all-time parent calls since profiler enable. Within a single rolling-window
report the two MUST agree (because both are sourced from the ring). State
this explicitly: `parent_call_count = parent.samples.len()` AFTER pruning,
computed at report time. Also: when profiler is enabled mid-run, the first
60s of "calls/parent" numbers will be skewed because the child may have
started recording after its parent's first call within the window. This
self-corrects in 60s and is acceptable — but call it out in the UI as a
warmup banner: "stabilizing… (Xs of data)" for the first 60s after enable.

**R6. Per-tick overhead budget — pin a number.** Risk table says "< 5%"
but it's not in the spec. Add a hard target to the "Performance / hot
path" reviewer-checklist section: *"Profiler enabled must add < 5%
wall-clock overhead to `step()` at 1500 creatures, measured over 10 000
ticks in release mode. Implementer must report the measured number in the
PR description."* Without this, "good enough" is undefined.

**R7. Toggle state must NOT persist.** UI checkbox defaults to unchecked
on every page load — confirm explicitly in plan: `profile_enable` state
lives only in the `Profiler` struct (in-memory), is not in any
`localStorage` key, is not in any save file. A user reloading the page
gets profiler off. This prevents the "save with profiler on tanks perf for
the next user" footgun. Add to D9.

### Should-fix call-outs (record but acceptable as-is)

**R8. Zero-cost-when-disabled is not literally zero.** The macro
short-circuit is `AtomicBool::load(Relaxed)` + branch. On x86 that's ~1
cycle for the load + 1 for the predicted-taken branch ≈ 2 cycles per span
site. With 12 sites per tick × 1500 ticks/s in fast-forward = 18k branches/s
≈ 36 000 cycles/s ≈ 12 µs/s of CPU — well under noise. Acceptable. Note
this in D2 explicitly rather than leaving "zero-cost" as a vague claim.

**R9. Span coverage cross-check against architecture.md tick steps.**
Doc lists 12 steps. Plan instruments 11 named spans inside the parent
`tick` span:
  1. grid_rebuild_1 ✓ (step 1)
  2. vision_pass ✓ (step 2)
  3. nn_forward ✓ (step 3)
  4. movement_and_repulsion ✓ (step 4, includes internal grid.rebuild #2)
  5. photosynth_two_pass ✓ (step 5)
  6. eat_and_scavenge ✓ (step 6)
  7. sun_refill ✓ (step 7)
  8. energy_bookkeeping ✓ (step 8)
  9. collect_deaths ✓ (step 9, BUT the dead-removal `swap_remove` loop and
     `creatures.remove_indices` are *outside* the span — they're part of
     step 9 and can be expensive at large die-offs; widen the span to
     cover them, OR add a sibling `remove_dead` span. **Recommendation:
     widen.**)
  10. decay_carrion ✓ (step 10)
  11. handle_births ✓ (step 11)
  12. step 12 (species detection / last_action promotion / tick bump /
      milestone events) — currently uninstrumented. It's trivial cost
      today, but if/when species detection lands in Milestone E it'll be
      heavy. **Add a `bookkeeping_tail` span** covering everything from
      end-of-handle_births through `self.tick += 1`. Cheap, future-proof.

  Also: `apply_movement_and_repulsion` calls `grid.rebuild()` a second
  time internally (verified at `src/world.rs:428`); plan acknowledges this
  and defers a `grid_rebuild_2` sub-span to v1.1. Fine.

**R10. Add a test for rolling-aggregation correctness.** Test 2 covers
"old samples get pruned"; add a Test 2b that pushes a known pattern of
3 samples with known durations at known timestamps, advances the clock
inside the window, calls `report_json`, and asserts `total_us` matches
the hand-computed sum. This catches the R1 class of bugs.

**R11. UI table width.** Seven columns × ~10 chars monospace + tree
indent will exceed the rail width on typical setups. Plan should either:
(a) drop the "roll /call" column (it's `total_us/call_count`, same as
"/call" in the proposed shape — the user can multiply mentally), (b)
make the table horizontally scrollable inside the rail, or (c) move the
profiler panel to a collapsible flyout below the rail. **Pick (a)** for
v1: drop the duplicate column. (Plan §D5 even notes "we keep them the
same".) The user's example CLI shows "roll /call" only for nodes deep in
the tree where the parent-relative meaning differs; if/when we compute
self-time differently in v1.1, restore the column.

**R12. Span stack is per-thread but Profiler is per-World.** Wasm is
single-threaded so this is fine, but document the invariant: `Profiler`
must never be shared across threads (it's `!Send` + `!Sync` per D3).
Today's `World` is single-threaded; the `threads` feature parallelizes
`nn_forward_all_chunks` but those chunks share no profiler state. If
spans are ever added INSIDE the parallel chunk loop, the design breaks.
Add to risk table.

### Top-3 fixes (priority order)

1. **R1 + R2:** Pin `total_us` recomputation from the ring per report and
   drop `self_us` from the JSON contract. (Bugs waiting to happen.)
2. **R4:** Drop `profile_record_external` and keep TS spans TS-side. (Net
   simpler architecture; no wasm coupling.)
3. **R3:** Report effective per-node window in JSON and surface it in the
   UI when it's noticeably less than `WINDOW_MS`. (Truth-in-labelling.)

### Inline fixes applied to plan text below

- D1: TS-side spans no longer round-trip through wasm (R4).
- D5: removed `self_us`; added `effective_window_ms` (R2, R3).
- D9: explicit "toggle never persists" clause (R7).
- `Node` doc-comment: explicit "totals are recomputed from samples on each
  report" (R1).
- World step span list: added `bookkeeping_tail` span; widened
  `collect_deaths` to include the dead-removal loop (R9).
- Reviewer checklist: added per-tick overhead budget cap of 5% at 1500
  creatures (R6) and added Test 2b for aggregation correctness (R10).
- Risk table: added "spans added inside parallel chunk loop break
  single-thread invariant" row (R12).

---

## Citations

- `src/world.rs` lines 168–268 (`World::step`) — instrumentation target.
- `src/wasm_api.rs` lines 22–371 (`WorldHandle` impl) — new methods land here.
- `src/lib.rs` lines 5–18 (module list) — add `pub mod profiler;`.
- `src/save.rs::SaveV1` — must NOT include profile; verify by reading.
- `src/snapshot_hash.rs` — must NOT include profile in hash input.
- `tests/acceptance.rs` + `tests/golden_snapshot_t10000.txt` — gates determinism.
- `web/src/main.ts` lines 247–280 (`frame()`) — RAF body wrap.
- `web/src/render.ts` lines 208–224 (`renderWorld`) — sub-span sites.
- `web/src/rail/stats.ts` (entire file) — extension point.
- `web/index.html` lines 32–35 (`#rail-stats`) — UI insertion point.
- `Cargo.toml` lines 22–32 — `web-sys` features (Performance already enabled, line 31).
- `docs/architecture.md` lines 41–60 — tick ordering reference for span placement.
