//! In-app hierarchical profiler. Runtime-toggleable, zero-cost when off.
//! Sampled spans accumulate into a tree keyed by ancestor path; each node
//! holds a ring buffer of (timestamp_ms_u32, dur_us_u32) samples over the
//! last WINDOW_MS. Reports are produced on demand and prune stale samples.
//!
//! Determinism: this module performs no RNG calls and only reads
//! `web_sys::Performance::now()` (or a native Instant clock in tests).
//! Mutation is confined to its own state. Profiler state is excluded from
//! the v6 §M snapshot hash.
//!
//! Design decisions (see docs/plans/perf-timing.md §D1–D10):
//! - D2: AtomicBool runtime toggle, not cfg(feature).
//! - D3: Per-World profiler instance.
//! - D4: Per-node ring buffer of (timestamp_ms, dur_us); cap = SAMPLES_PER_NODE.
//! - D5: JSON shape stable contract; total_us and call_count RECOMPUTED per report.
//! - D8: RAII SpanGuard (RAII), not closure wrap.
//! - D9: Default OFF; toggle state is never persisted.
//! - D10: Zero RNG, excluded from hash.

// profiler: rolling window duration in milliseconds
pub const WINDOW_MS: u32 = 60_000;
// profiler: maximum samples per node ring buffer
pub const SAMPLES_PER_NODE: usize = 4096;
// profiler: hard cap on node count to bound memory + bugs
pub const MAX_NODES: usize = 256;
// profiler: stabilizing threshold: show banner if effective_window_ms < 90% of WINDOW_MS
#[allow(dead_code)]
pub(crate) const STABILIZING_THRESHOLD_PCT: u32 = 90;

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

/// Index into Profiler::nodes. Stable for the life of the Profiler.
pub type NodeId = u16;

/// Synthetic root for Rust tick spans (index 0).
const ROOT_TICK: NodeId = 0;
/// Synthetic root for TS frame spans (index 1).
const ROOT_FRAME: NodeId = 1;

#[derive(Default)]
struct Node {
    name: String,
    #[allow(dead_code)] // retained for potential v1.1 self-time computation
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// Ring buffer of (timestamp_ms_relative_to_epoch: u32, dur_us: u32).
    /// total_us and call_count are RECOMPUTED from this ring on every report —
    /// there is NO running counter field. (Design decision D4/R1.)
    samples: VecDeque<(u32, u32)>,
}

struct ProfilerInner {
    nodes: Vec<Node>,
    /// Stack of (node_id, start_us_u64) for the current span scope.
    stack: Vec<(NodeId, u64)>,
    /// Resolved child-by-name cache: (parent_id, name) → child_id.
    /// One allocation per new span name; thereafter O(1) lookup.
    child_index: HashMap<(NodeId, String), NodeId>,
    /// Epoch in ms; subtracted from now_ms to fit timestamps in u32.
    /// Gives ~49 days of headroom.
    epoch_ms: f64,
}

pub struct Profiler {
    enabled: AtomicBool,
    inner: RefCell<ProfilerInner>,
}

impl Profiler {
    pub fn new() -> Self {
        let mut nodes = Vec::with_capacity(16);
        // Node 0: synthetic "tick" root (Rust spans).
        nodes.push(Node {
            name: "tick".to_string(),
            parent: None,
            children: Vec::new(),
            samples: VecDeque::with_capacity(SAMPLES_PER_NODE),
        });
        // Node 1: synthetic "frame" root (TS spans recorded via record_external).
        nodes.push(Node {
            name: "frame".to_string(),
            parent: None,
            children: Vec::new(),
            samples: VecDeque::with_capacity(SAMPLES_PER_NODE),
        });

        // Push "tick" as the initial stack sentinel so sub-spans attach to it.
        // The tick root itself is pushed/popped by the macro in World::step().
        // We pre-seed it empty — no stack entry needed at init time.

        let inner = ProfilerInner {
            nodes,
            stack: Vec::with_capacity(16),
            child_index: HashMap::new(),
            epoch_ms: clock_now_ms(),
        };

        Self {
            enabled: AtomicBool::new(false),
            inner: RefCell::new(inner),
        }
    }

    /// Cheap atomic load. Used by the span! macro early-return path.
    #[inline]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            self.inner.borrow_mut().clear_samples();
        } else {
            // Reset epoch so first frame is t≈0.
            self.inner.borrow_mut().epoch_ms = clock_now_ms();
        }
    }

    /// Push a span by name. Returns a SpanGuard whose Drop records the duration.
    /// Caller MUST check `enabled()` first — this method does no guard.
    /// The guard uses a raw pointer internally so the caller's `&mut self`
    /// borrows on other World fields are not blocked.
    pub fn push(&self, name: &'static str) -> SpanGuard {
        let start_us = clock_now_us();
        let mut inner = self.inner.borrow_mut();
        // Determine parent: the current top of stack, or ROOT_TICK if stack is empty.
        let parent = inner.stack.last().map(|&(id, _)| id).unwrap_or(ROOT_TICK);
        let node_id = inner.ensure_child(parent, name);
        inner.stack.push((node_id, start_us));
        drop(inner);
        SpanGuard {
            profiler: self as *const Profiler,
            start_us,
        }
    }

    /// Push the ROOT_TICK span itself (called from World::step tick root macro).
    /// This is the only span that has no parent — it IS the root.
    pub fn push_root(&self) -> SpanGuard {
        let start_us = clock_now_us();
        let mut inner = self.inner.borrow_mut();
        inner.stack.push((ROOT_TICK, start_us));
        drop(inner);
        SpanGuard {
            profiler: self as *const Profiler,
            start_us,
        }
    }

    /// Record an externally-timed span (called from TS via wasm_api).
    /// Record an externally-timed sample under the `tick` root, addressed by a
    /// dotted path. Used by aggregating callers (the parallel NN pass) that
    /// compute their own duration outside the RAII span machinery (because the
    /// Profiler isn't Send/Sync and can't be touched from worker threads).
    ///
    /// `path` is a dotted ancestor chain under ROOT_TICK, e.g.
    /// `"nn.proximity.creatures"`. The leaf node is created lazily.
    /// Silently no-ops when the profiler is disabled.
    pub fn record_under_tick(&self, path: &str, dur_us: u32) {
        if !self.enabled() {
            return;
        }
        let mut inner = self.inner.borrow_mut();
        let now_ms = inner.now_ms_relative();
        let mut current = ROOT_TICK;
        for part in path.split('.') {
            if part.is_empty() {
                continue;
            }
            current = inner.ensure_child(current, part);
        }
        let samples = &mut inner.nodes[current as usize].samples;
        if samples.len() == SAMPLES_PER_NODE {
            samples.pop_front();
        }
        samples.push_back((now_ms, dur_us));
    }

    /// `path` is a dotted ancestor chain under ROOT_FRAME, e.g.
    /// "renderWorld.drawCreatures". The leaf node is created lazily.
    /// Silently no-ops if the profiler is disabled (fast path in wasm_api).
    #[cfg(test)]
    pub fn record_external(&self, path: &str, dur_us: u32) {
        let mut inner = self.inner.borrow_mut();
        let now_ms = inner.now_ms_relative();
        let mut current = ROOT_FRAME;
        for part in path.split('.') {
            if part.is_empty() {
                continue;
            }
            current = inner.ensure_child(current, part);
        }
        let samples = &mut inner.nodes[current as usize].samples;
        if samples.len() == SAMPLES_PER_NODE {
            samples.pop_front();
        }
        samples.push_back((now_ms, dur_us));
    }

    /// Build the JSON report. Prunes stale samples as a side effect.
    /// Serializes the full tree in one pass with a single String buffer.
    pub fn report_json(&self) -> String {
        let mut inner = self.inner.borrow_mut();
        let now_ms_rel = inner.now_ms_relative();
        let now_ms_abs = clock_now_ms();
        inner.prune(now_ms_rel);

        let window_ms = WINDOW_MS;
        let enabled = self.enabled.load(Ordering::Relaxed);

        // Collect root nodes (tick=0, frame=1).
        let mut out = String::with_capacity(2048);
        out.push_str("{\"now_ms\":");
        push_f64(&mut out, now_ms_abs);
        out.push_str(",\"window_ms\":");
        out.push_str(&window_ms.to_string());
        out.push_str(",\"enabled\":");
        out.push_str(if enabled { "true" } else { "false" });
        out.push_str(",\"tree\":[");

        // Serialize tick root (node 0) and frame root (node 1).
        serialize_node(
            &inner.nodes,
            ROOT_TICK,
            now_ms_rel,
            window_ms,
            None,
            &mut out,
        );
        out.push(',');
        serialize_node(
            &inner.nodes,
            ROOT_FRAME,
            now_ms_rel,
            window_ms,
            None,
            &mut out,
        );

        out.push_str("]}");
        out
    }
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfilerInner {
    /// Current time relative to epoch, in milliseconds (u32).
    fn now_ms_relative(&self) -> u32 {
        let ms = clock_now_ms() - self.epoch_ms;
        ms.max(0.0) as u32
    }

    /// Ensure a child node with the given name exists under `parent`.
    /// Returns its NodeId. Creates lazily on first call.
    fn ensure_child(&mut self, parent: NodeId, name: &str) -> NodeId {
        let key = (parent, name.to_string());
        if let Some(&id) = self.child_index.get(&key) {
            return id;
        }
        // Guard against unbounded growth.
        if self.nodes.len() >= MAX_NODES {
            // Return parent as a no-op fallback — recording into the parent
            // is less bad than panicking or silently dropping data.
            return parent;
        }
        let new_id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            name: name.to_string(),
            parent: Some(parent),
            children: Vec::new(),
            samples: VecDeque::with_capacity(SAMPLES_PER_NODE),
        });
        self.nodes[parent as usize].children.push(new_id);
        self.child_index.insert(key, new_id);
        new_id
    }

    /// Drop samples whose timestamp is outside the rolling window.
    /// VecDeque is FIFO, so we pop from front until the front is in-window.
    fn prune(&mut self, now_ms: u32) {
        let cutoff = now_ms.saturating_sub(WINDOW_MS);
        for node in &mut self.nodes {
            while let Some(&(ts, _)) = node.samples.front() {
                if ts < cutoff {
                    node.samples.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    /// Clear all ring buffers but preserve the node tree topology.
    fn clear_samples(&mut self) {
        for node in &mut self.nodes {
            node.samples.clear();
        }
        self.stack.clear();
    }
}

/// RAII span guard. Holds a raw pointer to the Profiler to avoid tying
/// up the borrow of `self` (allowing `&mut self` method calls while the
/// guard is alive). This is safe because:
/// 1. `World` is `!Send`/`!Sync` — single-threaded wasm only.
/// 2. `Profiler` is a field of `World`; the guard lifetime is bounded to
///    the scope where `span!` was called, which is within a `World::step`
///    brace-block that cannot outlive `World`.
/// 3. The guard's Drop only borrows `RefCell<ProfilerInner>` mutably
///    (interior mutability), which is distinct from the `&mut self` borrows
///    on other World fields. No two borrows of the same RefCell overlap.
pub struct SpanGuard {
    /// Raw pointer to the Profiler. See safety note above.
    profiler: *const Profiler,
    start_us: u64,
}

// SpanGuard is intentionally not Send/Sync — same as Profiler.
impl Drop for SpanGuard {
    fn drop(&mut self) {
        // Safety: pointer is valid for the duration of the brace scope (see above).
        let profiler = unsafe { &*self.profiler };
        let now_us = clock_now_us();
        let dur = (now_us - self.start_us).min(u32::MAX as u64) as u32;
        let mut inner = profiler.inner.borrow_mut();
        let ts = inner.now_ms_relative();
        if let Some((node_id, _start)) = inner.stack.pop() {
            let samples = &mut inner.nodes[node_id as usize].samples;
            if samples.len() == SAMPLES_PER_NODE {
                samples.pop_front();
            }
            samples.push_back((ts, dur));
        }
    }
}

// ─── JSON serialization ───────────────────────────────────────────────────────

/// Recursively serialize a node (and its children) into `out`.
fn serialize_node(
    nodes: &[Node],
    id: NodeId,
    now_ms: u32,
    window_ms: u32,
    parent_call_count: Option<u64>,
    out: &mut String,
) {
    let node = &nodes[id as usize];

    // Recompute totals from the (post-prune) ring buffer — no running counter.
    let call_count = node.samples.len() as u64;
    let total_us: u64 = node.samples.iter().map(|&(_, d)| d as u64).sum();

    // Effective window: time span actually covered by current samples.
    let effective_window_ms: u32 = if let Some(&(oldest_ts, _)) = node.samples.front() {
        // effective = now_ms - oldest_ts, capped at window_ms.
        // (newest_ts is implicit in now_ms for report purposes.)
        now_ms.saturating_sub(oldest_ts).min(window_ms)
    } else {
        0
    };

    out.push_str("{\"name\":");
    push_json_str(out, &node.name);
    out.push_str(",\"total_us\":");
    out.push_str(&total_us.to_string());
    out.push_str(",\"call_count\":");
    out.push_str(&call_count.to_string());
    out.push_str(",\"parent_call_count\":");
    match parent_call_count {
        Some(n) => out.push_str(&n.to_string()),
        None => out.push_str("null"),
    }
    out.push_str(",\"effective_window_ms\":");
    out.push_str(&effective_window_ms.to_string());
    out.push_str(",\"children\":[");

    let children = &node.children;
    for (i, &child_id) in children.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        serialize_node(nodes, child_id, now_ms, window_ms, Some(call_count), out);
    }

    out.push_str("]}");
}

fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn push_f64(out: &mut String, v: f64) {
    // Simple fixed-point representation for millisecond timestamps.
    out.push_str(&format!("{v:.3}"));
}

// ─── Clock abstraction ───────────────────────────────────────────────────────

/// Current time in milliseconds. On wasm32, uses `web_sys::Performance::now()`
/// (monotonic, sub-microsecond resolution). On native (tests), uses a
/// monotonic Instant against a static epoch.
#[cfg(target_arch = "wasm32")]
fn clock_now_ms() -> f64 {
    web_sys::window()
        .expect("no window")
        .performance()
        .expect("no performance")
        .now()
}

#[cfg(not(target_arch = "wasm32"))]
fn clock_now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);

    // In tests, fake clock overrides the real clock.
    #[cfg(test)]
    {
        let fake = FAKE_NOW_US.with(|c| c.get());
        if let Some(us) = fake {
            return us as f64 / 1000.0;
        }
    }

    epoch.elapsed().as_secs_f64() * 1000.0
}

/// Current time in microseconds (u64). On wasm32, uses `Performance::now()`
/// which returns fractional milliseconds — multiply by 1000 for µs resolution.
/// Non-monotonic risk eliminated: `Performance::now()` is monotonic by spec.
#[cfg(target_arch = "wasm32")]
fn clock_now_us() -> u64 {
    let ms = web_sys::window()
        .expect("no window")
        .performance()
        .expect("no performance")
        .now();
    (ms * 1000.0) as u64
}

/// Thread-safe clock readable from rayon workers. On wasm32, binds directly to
/// the global `performance.now()` which exists in both Window and Worker scopes
/// (avoids `web_sys::window()` which is main-thread-only). On native, falls
/// through to `clock_now_us()`.
#[cfg(target_arch = "wasm32")]
mod ts_perf {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = performance, js_name = now)]
        pub fn now() -> f64;
    }
}

#[cfg(target_arch = "wasm32")]
pub fn clock_now_us_threadsafe() -> u64 {
    (ts_perf::now() * 1000.0) as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub fn clock_now_us_threadsafe() -> u64 {
    clock_now_us()
}

#[cfg(not(target_arch = "wasm32"))]
fn clock_now_us() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);

    #[cfg(test)]
    {
        let fake = FAKE_NOW_US.with(|c| c.get());
        if let Some(us) = fake {
            return us;
        }
    }

    epoch.elapsed().as_micros() as u64
}

// ─── Test clock injection ─────────────────────────────────────────────────────

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static FAKE_NOW_US: Cell<Option<u64>> = const { Cell::new(None) };
}

#[cfg(test)]
pub fn set_fake_clock_us(us: u64) {
    FAKE_NOW_US.with(|c| c.set(Some(us)));
}

#[cfg(test)]
pub fn clear_fake_clock() {
    FAKE_NOW_US.with(|c| c.set(None));
}

/// Test helper: push the ROOT_TICK span and immediately drop it with a fixed duration.
/// Bypasses the macro to allow direct duration injection.
/// Uses `name` to decide whether to call push_root() or push().
#[cfg(test)]
pub fn push_with_duration(p: &Profiler, name: &'static str, dur_us: u32) {
    // Temporarily manipulate the clock to get the right duration:
    // push starts at current fake clock, then advance to current + dur_us.
    let current = FAKE_NOW_US.with(|c| c.get()).unwrap_or(0);
    {
        let guard = if name == "tick" {
            p.push_root()
        } else {
            p.push(name)
        };
        // Advance fake clock by dur_us so drop() measures the right duration.
        FAKE_NOW_US.with(|c| c.set(Some(current + dur_us as u64)));
        drop(guard);
    }
    // Restore clock to current + dur_us (already set above).
}

// ─── Macro ───────────────────────────────────────────────────────────────────

/// profile_span!(&profiler, "name") — RAII span guard.
///
/// Generates the enabled-check + push. When disabled, produces None with no
/// clock read and no allocation. When enabled, returns Some(SpanGuard) whose
/// Drop records the duration into the profiler's ring buffer.
///
/// Usage:
/// ```ignore
/// let _g = crate::profiler::span!(&self.profile, "vision_pass");
/// ```
#[macro_export]
macro_rules! profile_span {
    ($prof:expr, "tick") => {
        // The tick root span uses push_root() (no parent lookup).
        let _g = if $prof.enabled() {
            Some($prof.push_root())
        } else {
            None
        };
    };
    ($prof:expr, $name:literal) => {
        let _g = if $prof.enabled() {
            Some($prof.push($name))
        } else {
            None
        };
    };
}

#[allow(unused_imports)]
pub(crate) use profile_span as span;

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 1: tree shape after synthetic spans.
    #[test]
    fn profiler_builds_correct_tree() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);
        {
            let _t = p.push_root(); // tick root
            {
                let _v = p.push("vision_pass");
            }
            {
                let _v = p.push("vision_pass"); // call again, same node
            }
            {
                let _n = p.push("nn_forward");
            }
        }
        let json = p.report_json();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        assert_eq!(tick["call_count"], 1, "tick must have call_count=1");
        let children = tick["children"].as_array().unwrap();
        let vision = children
            .iter()
            .find(|n| n["name"] == "vision_pass")
            .unwrap();
        assert_eq!(
            vision["call_count"], 2,
            "vision_pass must have call_count=2"
        );
        let nn = children.iter().find(|n| n["name"] == "nn_forward").unwrap();
        assert_eq!(nn["call_count"], 1, "nn_forward must have call_count=1");
        clear_fake_clock();
    }

    /// Test 2: pruning drops stale samples.
    #[test]
    fn profiler_prunes_stale_samples() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);
        {
            let _t = p.push_root();
        }
        // Advance past the window: WINDOW_MS + 1 s.
        set_fake_clock_us((WINDOW_MS as u64 + 1000) * 1000);
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        assert_eq!(
            tick["call_count"], 0,
            "stale sample must be pruned after window expires"
        );
        clear_fake_clock();
    }

    /// Test 2b: rolling-window total is the sum of in-window samples only.
    /// Verifies the R1 requirement: total_us is recomputed from samples, not a counter.
    #[test]
    fn profiler_total_us_sums_in_window_samples_only() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);

        push_with_duration(&p, "tick", 100); // dur=100us at t=0
        set_fake_clock_us(10_000_000); // +10s
        push_with_duration(&p, "tick", 200); // dur=200us at t=10s
        set_fake_clock_us(20_000_000); // +20s
        push_with_duration(&p, "tick", 300); // dur=300us at t=20s

        // All three inside the 60s window.
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        assert_eq!(tick["total_us"], 600, "total_us must be 100+200+300=600");
        assert_eq!(tick["call_count"], 3, "call_count must be 3");

        // Advance past the first sample's expiry (t=0 sample expires at t=60s).
        // Set clock to t=65s: cutoff = 65000 - 60000 = 5000ms.
        // t=0ms sample (ts=0) has 0 < 5000 → pruned.
        // t=10s sample (ts=10000ms) has 10000 >= 5000 → survives.
        // t=20s sample (ts=20000ms) has 20000 >= 5000 → survives.
        set_fake_clock_us(65_000_000); // t=65s
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        assert_eq!(
            tick["total_us"], 500,
            "only 200+300=500 remain after t=0 sample pruned"
        );
        assert_eq!(tick["call_count"], 2, "call_count must be 2 after pruning");
        clear_fake_clock();
    }

    /// Test 3: disabled profiler is a no-op (macro version).
    #[test]
    fn profiler_disabled_records_nothing() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        // enabled defaults to false — do NOT call set_enabled(true).
        // The macro short-circuits before push() is called.
        // We simulate the macro behavior by checking enabled() first.
        if p.enabled() {
            let _t = p.push_root();
        }
        let json = p.report_json();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        for root in v["tree"].as_array().unwrap() {
            assert_eq!(
                root["call_count"], 0,
                "disabled profiler must record nothing (node: {})",
                root["name"]
            );
        }
        clear_fake_clock();
    }

    /// Test 4: external spans attach under the frame root.
    #[test]
    fn profiler_external_spans_attach_under_frame() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);
        p.record_external("renderWorld.drawCreatures", 1500);
        p.record_external("step_n", 4200);
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let frame = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "frame")
            .unwrap()
            .clone();
        let rw = frame["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "renderWorld")
            .unwrap()
            .clone();
        assert_eq!(
            rw["children"].as_array().unwrap().len(),
            1,
            "renderWorld must have 1 child"
        );
        // step_n is a direct child of frame.
        let step = frame["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "step_n")
            .unwrap()
            .clone();
        assert_eq!(step["call_count"], 1);
        clear_fake_clock();
    }

    /// Test 5: JSON shape contract — every field is present and typed correctly.
    #[test]
    fn profiler_json_shape_is_stable() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);
        {
            let _t = p.push_root();
            {
                let _v = p.push("vision_pass");
            }
        }
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        assert!(v["now_ms"].is_number(), "now_ms must be a number");
        assert_eq!(v["window_ms"], WINDOW_MS, "window_ms must match WINDOW_MS");
        assert_eq!(v["enabled"], true, "enabled must be true");
        let tree = v["tree"].as_array().unwrap();
        assert!(
            tree.iter().any(|n| n["name"] == "frame"),
            "tree must contain frame root"
        );
        assert!(
            tree.iter().any(|n| n["name"] == "tick"),
            "tree must contain tick root"
        );
        // Check every required field exists on each root node.
        for node in tree {
            for field in &[
                "name",
                "total_us",
                "call_count",
                "parent_call_count",
                "effective_window_ms",
                "children",
            ] {
                assert!(
                    node.get(field).is_some(),
                    "missing field {field} in node {}",
                    node["name"]
                );
            }
        }
        clear_fake_clock();
    }

    /// Test 6b: profiler sibling spans don't nest into each other.
    #[test]
    fn profiler_siblings_do_not_nest() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);
        {
            let _t = p.push_root();
            {
                let _a = p.push("span_a");
            } // drops here
            {
                let _b = p.push("span_b");
            } // drops here — should be sibling of span_a, not child
        }
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        let children = tick["children"].as_array().unwrap();
        // span_a and span_b must both be direct children of tick.
        assert!(
            children.iter().any(|n| n["name"] == "span_a"),
            "span_a must be direct child of tick"
        );
        assert!(
            children.iter().any(|n| n["name"] == "span_b"),
            "span_b must be direct child of tick"
        );
        // span_b must NOT appear under span_a.
        let span_a = children
            .iter()
            .find(|n| n["name"] == "span_a")
            .unwrap()
            .clone();
        assert!(
            span_a["children"].as_array().unwrap().is_empty(),
            "span_a must have no children"
        );
        clear_fake_clock();
    }

    /// Profiler overhead test: measures absolute cost of 11 spans per iteration
    /// and asserts each span pair (push+drop) costs < 5µs.
    ///
    /// Rationale: comparing off vs on is impractical in unit-test mode because
    /// the off-path is nearly eliminated by the branch predictor (0µs baseline),
    /// making the ratio meaningless. Instead, we measure the absolute on-path
    /// cost, which is the real concern: at 11 spans × 1500-creature tick, the
    /// overhead must stay < 5% of tick time.
    ///
    /// Expected: 11 push+drop pairs at ~50-200ns each = ~550ns-2200ns per tick.
    /// A tick with 1500 creatures takes ~1-5ms → overhead ≈ 0.05-0.22% (well < 5%).
    ///
    /// This test asserts < 5µs per span pair (very lenient: ~25× the expected cost)
    /// to catch only catastrophic regressions (e.g., accidentally allocating on push).
    #[test]
    fn profiler_overhead_within_5us_per_span() {
        use std::time::Instant;
        clear_fake_clock(); // Use real clock for this test.

        const ITERS: usize = 10_000;
        // 10 named sub-spans per tick.
        const SPANS_PER_ITER: usize = 11;

        let p = Profiler::new();
        p.set_enabled(true);

        let t0 = Instant::now();
        for _ in 0..ITERS {
            let _t = p.push_root();
            let _a = p.push("vision_pass");
            drop(_a);
            let _b = p.push("nn_forward");
            drop(_b);
            let _c = p.push("movement_and_repulsion");
            drop(_c);
            let _graze = p.push("graze");
            drop(_graze);
            let _e = p.push("eat");
            drop(_e);
            let _grass = p.push("grass_step");
            drop(_grass);
            let _g = p.push("energy_bookkeeping");
            drop(_g);
            let _h = p.push("collect_deaths");
            drop(_h);
            let _j = p.push("handle_births");
            drop(_j);
            let _k = p.push("bookkeeping_tail");
            drop(_k);
        }
        let total_us = t0.elapsed().as_micros() as f64;
        let per_span_us = total_us / (ITERS * SPANS_PER_ITER) as f64;

        // Report for PR description.
        eprintln!(
            "profiler overhead: {ITERS} ticks × {SPANS_PER_ITER} spans; \
             total={total_us:.1}µs; per_span={per_span_us:.3}µs"
        );

        // Assert < 5µs per span pair — a 25× safety margin over expected ~200ns.
        // Failures here indicate algorithmic regression (e.g., per-call allocation).
        assert!(
            per_span_us < 5.0,
            "per-span overhead {per_span_us:.3}µs exceeds 5µs threshold; \
             total={total_us:.1}µs over {ITERS}×{SPANS_PER_ITER} spans"
        );
    }

    /// Test: effective_window_ms is 0 when no samples, and pinned to the actual
    /// time span when samples exist.
    #[test]
    fn profiler_effective_window_ms_is_accurate() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);

        // No samples yet — effective_window_ms must be 0.
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        assert_eq!(
            tick["effective_window_ms"], 0,
            "no samples → effective_window_ms=0"
        );

        // Push one sample at t=0.
        push_with_duration(&p, "tick", 100);
        // Advance to t=5s.
        set_fake_clock_us(5_000_000);
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        // effective_window_ms = now_ms - oldest_ts ≈ 5000ms.
        let ew = tick["effective_window_ms"].as_u64().unwrap();
        assert!(
            (4990..=5010).contains(&ew),
            "effective_window_ms={ew} expected ~5000"
        );
        clear_fake_clock();
    }

    /// Test: native (non-wasm) clock path produces monotone, non-zero µs values.
    /// Guards against regressions where the std::time path returns constant 0
    /// or regresses to a non-monotonic source.
    #[test]
    fn profiler_native_clock_is_monotone_and_nonzero() {
        // Use the real clock (no fake clock override).
        clear_fake_clock();

        let t0 = clock_now_us();
        // Sleep to guarantee ≥1 µs elapsed on the real clock even under heavy
        // CPU contention (e.g. rayon threads active during parallel test builds).
        // Previously a busy-spin of 10_000 iterations, which was unreliable when
        // the timer resolution is coarse or the spin finishes in < 1 µs.
        std::thread::sleep(std::time::Duration::from_millis(1));
        let t1 = clock_now_us();

        assert!(
            t1 >= t0,
            "clock_now_us must be non-decreasing (monotone): t0={t0} t1={t1}"
        );
        // The OnceLock epoch is set on first call; both t0 and t1 are elapsed-from-epoch.
        // t0 could theoretically be 0 if called in the same µs as EPOCH init,
        // but t1 must be > 0 after the busy-spin.
        assert!(
            t1 > 0,
            "clock_now_us must return non-zero after warm-up: t1={t1}"
        );
    }

    /// Test: sample-cap behavior — when SAMPLES_PER_NODE is exceeded, oldest is dropped.
    #[test]
    fn profiler_ring_buffer_cap_drops_oldest() {
        set_fake_clock_us(0);
        let p = Profiler::new();
        p.set_enabled(true);
        // Fill the ring buffer past capacity.
        for i in 0..(SAMPLES_PER_NODE + 10) {
            set_fake_clock_us(i as u64 * 1000);
            push_with_duration(&p, "tick", 1);
        }
        // call_count must not exceed SAMPLES_PER_NODE.
        let v: serde_json::Value = serde_json::from_str(&p.report_json()).unwrap();
        let tick = v["tree"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "tick")
            .unwrap()
            .clone();
        let cc = tick["call_count"].as_u64().unwrap();
        assert!(
            cc <= SAMPLES_PER_NODE as u64,
            "call_count={cc} must not exceed SAMPLES_PER_NODE={SAMPLES_PER_NODE}"
        );
        clear_fake_clock();
    }
}
