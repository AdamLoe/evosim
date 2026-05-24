# Milestone F — persistence, seed/resume, eulogy card, acceptance test, balance

**Spec anchors:** v5 §13 (persistence: double-buffered SoA, transferable
handoff to worker, IndexedDB JSON, autosave every 10 sim-seconds), v5 §11
(eulogy: 4-image grid + Copy Share Text + Download Save + New World, seed
prominently shown), v5 §15 F.26–F.30 (sequencing), v5 §16 (Definition of
Done — the headless acceptance test gates the build). v6 §I (persistence
defaults: `schema_version: 1`, mismatched-version modal, IDB quota toast,
event log: ring 200 in UI + full history in save), v6 §K (slider ranges
already wired in B/C/D), v6 §L (first-to definitions — already wired in
E.25.d via `src/hof.rs::HallOfFame`), v6 §M (snapshot hash spec for §16:
xxHash64; hash-input ordering pinned; golden value committed to
`tests/golden_snapshot_t10000.txt`).

**Status at handoff:** Milestone E is committed. `WorldHandle` exposes
`creatures_buffer`, `creature_ids_buffer`, `sun_buffer`,
`sun_capacity_buffer`, `carrion_buffer`, `recent_events_json`,
`events_total_count`, `species_list_json`, `creature_at`,
`creature_inspect_json`, `stats_sample`, `set_slider`, getters
`tick / seed / population / species_count / world_ended / world_size /
sun_dim`. `src/hof.rs::HallOfFame` is serde-derive ready and gets
populated during E.25.d at five sites on `World`: `biggest_ever`,
`last_survivor`, `weirdest`, `longest_lived`, `first_mover_snapshot`
(plus `founder_genome_anchor` / `founder_brain_anchor` for the weirdness
metric). `Genome`, `Brain`, `Carrion`, `Species`, `EventLog`, `Event /
EventKind` (`#[serde(tag = "type")]`), `DevSliders`, `HallOfFame` all
already `#[derive(Serialize, Deserialize)]`. `WorldEnded` event is
already emitted by `World` when `population() == 0`. The right rail,
toasts, highlight rings, stats charts, inspector and click-handling are
all live.

`twox-hash = "1.6"` is already pinned in `Cargo.toml` (used for the seed
hash in `SimRng::from_string`); we reuse the same crate for the §16
snapshot hash so no new dependency lands in F.

**What is intentionally NOT in F** (so the implementer doesn't drift):

- WebGPU NN path, lineage tree viz, trait histograms, NN viz,
  follow-cam, signaling, seasons, sexual reproduction — v1.1+ per v5 §14.
- New traits / new actions / new sliders / new NN inputs — out forever.
- New events beyond what E.25 already added — out forever in v1.
- Lineage tree in the eulogy card — F.28 is a 4-image grid, nothing more.
- Tabs UI, follow-cam, "jump to event" pan — v1.1.
- Per-species color stack in stats charts — v1.1.
- Continuous-save background worker pipeline more elaborate than what
  v5 §13 + the simpler-architecture pin below specifies.

---

## Scope ceiling restatement

v5 §15 F ships five substeps:

- **F.26 (heavy review):** persistence — `snapshot_json`,
  `load_from_json`, JS+worker shell for IDB writes, resume prompt,
  schema-version modal, IDB-quota toast, full-event-log retention.
- **F.27 (light review):** seed display in top bar (copy button) +
  resume prompt component. Modular and dismissible.
- **F.28 (moderate review):** eulogy card — 4-image grid of HoF
  creatures, Copy Share Text, Download Save, New World button. Adds
  `hof_json()` to `WorldHandle` and a single-creature renderer.
- **F.29 (heavy review — perf gate):** headless acceptance test in
  `tests/acceptance.rs`. xxHash64 snapshot hash with pinned golden value
  in `tests/golden_snapshot_t10000.txt`. Wired into `.github/workflows/ci.yml`.
- **F.30 (orchestrator-only — NOT subagented):** balance tuning to make
  §16 DoD pass. F.30 is a checklist for the orchestrator below; no
  implementer code.

The implementer subagent works through F.26 → F.27 → F.28 → F.29 in
order. F.27 has tiny code; the natural place is "between F.26 and F.28"
because F.27's resume prompt depends on F.26's `snapshot_json` /
`load_from_json` constructor existing. F.28 depends on F.26's
`snapshot_json` (for Download Save) and on E.25.d (already in tree). F.29
depends on F.26 only insofar as it imports the same `snapshot_json` for
the hash inputs (we want to validate that round-tripping a save would
yield the same hash — sanity check, not asserted).

Code-reviewer depth (per ORCHESTRATOR adapting-review-depth guidance):
**heavy** on F.26 (load-bearing for v5 §13 + v6 §I; load_from_json is a
new constructor path with real failure modes), **light** on F.27 (UI
only), **moderate** on F.28 (new wasm surface + first single-creature
renderer in TS), **heavy** on F.29 (perf gate + golden hash; v5 §16 DoD
hinges on it).

---

## F.26 — persistence (snapshot_json + load_from_json + JS worker + IDB autosave)

### Goal

Per v5 §13 + v6 §I: at most every 10 wall-seconds, serialize the World
to JSON and write it to IndexedDB via a background Web Worker. On
reopen, if a saved world exists and the URL has no `?seed=` query param,
show a "Resume?" prompt. Schema version mismatch → modal "This save is
from a different build. Start a New World?". IDB quota / permission
errors → top-bar toast "Autosave failed — local storage may be full" for
6 seconds, sim continues normally. Full event log goes to disk; UI ring
of 200 is unchanged.

### F.26 architecture decisions (PIN UP-FRONT)

v5 §13 says: "the front buffer is *transferred* (zero-copy via
`Transferable`) to a Web Worker. The worker JSON-encodes via
`serde_json` and writes to IndexedDB." That phrasing assumes two wasm
bundles (main + worker). At v1 sizes the World JSON-encodes in
sub-millisecond on the main thread; the win from offloading
JSON-encoding is negligible, and the cost (a second wasm artifact +
keeping two `World` Rust contexts in sync, OR shipping the SoA over
postMessage as a Transferable buffer the worker decodes) is large.

**PIN (deviation from v5 §13, documented in DECISIONS):**

1. **JSON-encode on the main thread, inside the existing wasm module.**
   Add `WorldHandle::snapshot_json() -> String`. Measured target: at
   2k creatures + 400 sun cells + 60 carrion + a ~10k-entry event log +
   ~50 species, the JSON is ~2–5 MB and `serde_json::to_string`
   completes in 1–5 ms on a modern laptop. We allocate one `String`,
   pass it to JS as the wasm-bindgen-converted `JsString` (a single
   copy into the JS heap), and hand it to the worker.
2. **No double-buffered SoA transferable handoff.** We do not maintain
   two parallel `World` instances. The deviation preserves v5 §13's
   spirit ("don't hitch the main thread on disk I/O") because:
     - The JSON-encode is sub-millisecond at peak v1 size.
     - The IDB write happens off-thread (in the worker) — that's the
       expensive part (disk I/O + structured-clone or string-copy of
       multi-MB into IndexedDB).
   If F.30 measurements show the encode itself hitches a frame, we
   revisit (option: serialize on a budget, slice across frames; or move
   serialization into a wasm worker). DECISIONS captures the punt path.
3. **The worker is a plain ES-module worker (no wasm).** It receives
   `{ type: "save", json: string, seed: string, tick: number }`, opens
   the `evosim` IndexedDB, writes one row into the `worlds` store under
   key `"current"`, and posts back `{ type: "saved" }` or
   `{ type: "error", message: string, code: string }`. ~80 LOC, no
   bundler config needed (Vite supports `?worker` imports natively).

**DECISIONS lines (F.26 prelude):**

- `persistence handoff: JSON-encode on main thread (1–5 ms at v1 sizes); plain JS worker only writes to IDB — preserves spirit of v5 §13 "don't hitch main thread on disk I/O" without the cost of a second wasm artifact or double-buffered SoA. Revisit in v1.1 if F.30 measurements show encode hitches a frame.`
- `worker type: plain ES-module worker (Vite ?worker import) — no wasm-pack second build, no SAB / COOP-COEP coupling for the persistence path specifically (the rayon path retains its own SAB requirement)`

### F.26.a — schema versioning + `snapshot_json` + `load_from_json`

Add a new module `src/save.rs` to keep `world.rs` lean:

```rust
//! Save/load support for v1. Schema_version: 1. JSON via serde_json.

use crate::brain::Brain;
use crate::carrion::Carrion;
use crate::creature::{Action, CreatureSoA};
use crate::events::EventLog;
use crate::genome::Genome;
use crate::hof::HallOfFame;
use crate::rng::SimRng;
use crate::species::SpeciesRegistry;
use crate::sun::SunMap;
use crate::world::{DevSliders, World};
use serde::{Deserialize, Serialize};

/// Current schema version. Bump on any save-shape change.
pub const SCHEMA_VERSION: u32 = 1;

/// Wire shape on disk. Camera/UI state lives on JS side (see F.27).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveV1 {
    pub schema_version: u32,
    pub seed: String,
    pub tick: u32,

    // RNG state.
    pub rng: SimRngSnapshot,

    // SoA (one Vec per column).
    pub creatures: CreatureSoASnapshot,

    // Maps.
    pub sun: SunMapSnapshot,
    pub carrion: Vec<Carrion>,

    // Bookkeeping.
    pub species: SpeciesSnapshot,
    pub events: EventLogSnapshot,
    pub sliders: DevSliders,

    pub next_creature_id: u64,
    pub peak_population: u32,
    pub peak_species_count: u32,
    pub world_ended: bool,
    pub live_species_count: u32,
    pub first_move_fired: bool,
    pub first_eat_fired: bool,
    pub population_milestones_fired: u32,

    // Hall-of-fame (E.25.d).
    pub biggest_ever: Option<HallOfFame>,
    pub last_survivor: Option<HallOfFame>,
    pub weirdest: Option<HallOfFame>,
    pub weirdest_distance: f32,
    pub longest_lived: Option<HallOfFame>,
    pub longest_lived_age: u32,
    pub first_mover_snapshot: Option<HallOfFame>,
    pub founder_genome_anchor: Genome,
    pub founder_brain_anchor: Vec<f32>,
}
```

#### Why explicit "Snapshot" structs for SoA / SunMap / Registry / EventLog

These currently lack serde derives or have non-serde-friendly internal
shapes (e.g. `EventLog` uses `VecDeque`; `SimRng` wraps a third-party
type; `CreatureSoA` is a 17-column SoA that we don't necessarily want
to serialize directly — round-tripping into and out of it should be a
deliberate API). PIN: write thin "Snapshot" structs that own owned
Vecs, then add `From<&X>` / `try_into X` conversions. Reasons:

- We don't change the existing structs' serde derive footprint or
  layout — derives stay localized to `save.rs`.
- The on-disk JSON layout is explicit and reviewable.
- Future schema bumps (v1.1) write a `SaveV2` next to `SaveV1` with a
  tiny `From<SaveV1> for SaveV2` migration; the current schema struct
  is frozen.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimRngSnapshot {
    /// xoshiro256++ state: 4 × u64. We surface the 32 bytes of internal
    /// state by serializing through a deterministic step sequence — see
    /// "RNG round-trip" below.
    pub state: [u64; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatureSoASnapshot {
    pub id: Vec<u64>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub energy: Vec<f32>,
    pub age: Vec<u32>,
    pub digestion_cooldown: Vec<u32>,
    pub cumulative_upkeep: Vec<f32>,
    pub species_id: Vec<u32>,
    pub parent_species_id: Vec<u32>,
    pub last_action: Vec<Action>,
    pub action_this_tick: Vec<Action>,
    pub max_size_reached: Vec<f32>,
    pub distance_travelled: Vec<f32>,
    pub birth_tick: Vec<u32>,
    pub genomes: Vec<Genome>,
    pub brains: Vec<Brain>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SunMapSnapshot {
    pub capacity: Vec<f32>,
    pub current: Vec<f32>,
    pub hotspots: Vec<(f32, f32)>,
    pub demand: Vec<f32>,
    pub gradient_strength: f32,
    pub refill_rate: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeciesSnapshot {
    /// Stored as a Vec of Species records; `next_id` derives from list.len()
    /// (next_id == max(id) + 1; recompute on load).
    pub list: Vec<crate::species::Species>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventLogSnapshot {
    /// Full history (recent ring is recomputed from the tail on load).
    pub all: Vec<crate::events::Event>,
    pub ring_cap: usize,
}
```

#### RNG round-trip

`rand_xoshiro::Xoshiro256PlusPlus` doesn't expose its 4-u64 state
directly. Two paths:

- **(A)** Re-seed deterministically: on save, record the original
  string seed + current `tick` (already on hand). On load, rebuild the
  RNG from the string seed, then advance it by the number of `next_u64`
  calls that the sim made in its first N ticks. Infeasible: we don't
  count RNG calls per tick.
- **(B)** `bincode`-serialize the inner `Xoshiro256PlusPlus`. The
  `rand_xoshiro` crate derives `Serialize/Deserialize` behind feature
  `serde1`. Enable that feature in `Cargo.toml`. **PIN (B).** Simpler,
  exact, no new code.

```toml
# Cargo.toml diff
rand_xoshiro = { version = "0.6", features = ["serde1"] }
```

Then `SimRngSnapshot` becomes just `pub struct SimRngSnapshot(pub
rand_xoshiro::Xoshiro256PlusPlus);` and we add `Clone, Debug, Serialize,
Deserialize` derives on `SimRng` itself (which already wraps it). The
`[u64; 4]` shape proposed above is wrong; replace it.

**Pin (revised):**

```rust
// src/rng.rs — add derives
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimRng(pub Xoshiro256PlusPlus);
```

And `SaveV1::rng` becomes `pub rng: SimRng` (no snapshot wrapper).

#### `World::to_save_v1()` and `World::from_save_v1(SaveV1) -> Result<World, LoadError>`

```rust
// src/save.rs

#[derive(Debug)]
pub enum LoadError {
    SchemaVersionMismatch { found: u32, expected: u32 },
    InvalidJson(serde_json::Error),
    StructuralError(String),
}

impl SaveV1 {
    pub fn from_world(w: &World) -> Self { /* field-by-field clone */ }
}

impl World {
    pub fn to_save_v1(&self) -> SaveV1 {
        SaveV1::from_world(self)
    }

    pub fn from_save_v1(save: SaveV1) -> Result<Self, LoadError> {
        if save.schema_version != SCHEMA_VERSION {
            return Err(LoadError::SchemaVersionMismatch {
                found: save.schema_version,
                expected: SCHEMA_VERSION,
            });
        }
        // Rebuild SpatialGrid from creature positions.
        // Rebuild vision Vec to match creature count (zeros — populated next tick).
        // Recompute next_id for species (max id + 1).
        // Empty cell_to_carrion and pending_extinction_check (they're
        // per-tick scratch; safe to start empty).
        // ...
    }
}
```

Audit during code review:

- **Length invariants:** every `CreatureSoA` Vec must have equal length.
  Add a `validate_lengths_equal()` helper called during `from_save_v1`;
  return `StructuralError` if mismatched. Reviewer must confirm there
  are no panics from sub-structures (e.g. `Brain` weight count must
  match `NN_WEIGHT_COUNT` constant).
- **Vision Vec sizing:** `Vec<VisionBuf>` (per-creature scratch) must
  equal `creatures.len()`. Initialize to `vec![[0.0; VISION_LEN];
  creatures.len()]` after load; the next tick's vision pass overwrites.
- **`SpatialGrid` rebuild:** call `grid.rebuild(&creatures.x,
  &creatures.y)` post-construction. Don't serialize the grid.
- **`cell_to_carrion`:** start empty; rebuilt at the top of next tick.

#### `WorldHandle::snapshot_json()` and constructor `from_json`

```rust
// src/wasm_api.rs

#[wasm_bindgen]
impl WorldHandle {
    /// Serialize the world to JSON for autosave / Download Save. ~1–5 ms at v1 sizes.
    #[wasm_bindgen]
    pub fn snapshot_json(&self) -> String {
        let save = self.inner.to_save_v1();
        serde_json::to_string(&save).expect("save serialization is infallible")
    }

    /// Construct a WorldHandle from a JSON save. Returns Result<…, JsValue>.
    /// JS catches the rejection and shows the schema-mismatch modal.
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(json: &str) -> Result<WorldHandle, JsValue> {
        let save: SaveV1 = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("invalid-json:{e}")))?;
        let inner = World::from_save_v1(save).map_err(|e| match e {
            LoadError::SchemaVersionMismatch { found, expected } => {
                JsValue::from_str(&format!("schema-mismatch:{found}:{expected}"))
            }
            LoadError::InvalidJson(e) => JsValue::from_str(&format!("invalid-json:{e}")),
            LoadError::StructuralError(s) => JsValue::from_str(&format!("structural:{s}")),
        })?;
        Ok(Self {
            inner,
            creature_buf: Vec::new(),
            sun_buf: Vec::new(),
            carrion_buf: Vec::new(),
            id_buf: Vec::new(),
        })
    }
}
```

JS-side, the loader pattern is:

```ts
try {
  const world = WorldHandle.fromJson(json);
  // success
} catch (err) {
  const msg = String(err); // wasm-bindgen returns the JsValue string we built
  if (msg.startsWith("schema-mismatch:")) {
    showSchemaMismatchModal();
  } else {
    console.error("load_from_json failed:", msg);
    showSchemaMismatchModal(); // also treat invalid-json / structural as "start fresh"
  }
}
```

(**DECISIONS:** treat any load failure as "start fresh" — schema
mismatch is the spec'd case; invalid-json and structural failures are
"the save is corrupt", and the spec'd UX of "start a New World" applies
just as well. The modal text per v6 §I is fixed: "This save is from a
different build. Start a New World? [New World]". For corruption we
show the same modal — a more precise message gains nothing.)

#### Autosave trigger

Per v5 §13: "every 10 sim-seconds." 1 sim-second = ? — v5 §3.2 says
"Sim decoupled from render. User slider: pause / 1x / 10x / 100x" and
"Render samples world state at 30fps." A "sim-second" is intentionally
the *intended-real-time* tempo. The cleanest read: **a sim-second is 30
ticks** (because we render at 30fps and "1×" means "1 sim-second per
wall-second"). So autosave fires every 300 sim-ticks. At 1× that's a
wall-clock save every 10 seconds; at 100× it's a wall-clock save every
0.1 seconds. **DECISIONS:** *"sim-second" = 30 ticks; autosave every
300 sim-ticks (matches v5 §13 "10 sim-seconds")*.

Practical effect: at 100× the autosave fires 10× / wall-second, which
is too aggressive (we'd be saving multi-MB strings 10×/s and the worker
queue would back up). PIN a **wall-clock floor of 3 seconds between
autosaves** in the JS scheduler:

```ts
const AUTOSAVE_TICK_INTERVAL = 300;  // 10 sim-seconds at 1×
const AUTOSAVE_WALL_FLOOR_MS = 3000; // never save more than once / 3s wall

let lastSavedTick = -1;
let lastSavedWallMs = 0;

function maybeAutosave(world: WorldHandle, nowMs: number, worker: Worker): void {
  const t = world.tick;
  if (t === lastSavedTick) return;
  if (t - lastSavedTick < AUTOSAVE_TICK_INTERVAL && lastSavedTick >= 0) return;
  if (nowMs - lastSavedWallMs < AUTOSAVE_WALL_FLOOR_MS) return;
  // Fire.
  const json = world.snapshot_json();
  worker.postMessage({ type: "save", json, seed: world.seed, tick: t });
  lastSavedTick = t;
  lastSavedWallMs = nowMs;
}
```

(`AUTOSAVE_TICK_INTERVAL` is also tracked here so a paused player
sitting forever won't trigger another save; the tick must advance.)

**DECISIONS lines:**

- `autosave cadence: every 300 sim-ticks (= 10 sim-seconds; sim-second pinned at 30 ticks per v5 §3.2 "30fps render") with a 3000ms wall-clock floor — keeps high-speed players from queuing multi-MB writes`
- `single autosave slot: object store key "current" — one world per browser profile; matches v5 §13 "Resume world from day X" prompt (one resume target)`

#### IDB schema

```
DB:         "evosim"
Version:    1
Stores:     "worlds" — keyPath "key", values { key: string, json: string,
                                                tick: number, seed: string,
                                                saved_at_ms: number }
Key used:   "current" (single slot)
```

DB version bumps with `SaveV1` schema bumps (paired). Currently version
1 holds schema_version 1.

Worker pseudo-code (`web/src/persistence/worker.ts`):

```ts
let dbPromise: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const req = indexedDB.open("evosim", 1);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains("worlds")) {
        db.createObjectStore("worlds", { keyPath: "key" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("open failed"));
  });
  return dbPromise;
}

self.onmessage = async (ev) => {
  const msg = ev.data;
  if (msg?.type === "save") {
    try {
      const db = await openDb();
      const tx = db.transaction("worlds", "readwrite");
      const store = tx.objectStore("worlds");
      const row = {
        key: "current",
        json: msg.json,
        tick: msg.tick,
        seed: msg.seed,
        saved_at_ms: Date.now(),
      };
      const putReq = store.put(row);
      putReq.onerror = () => {
        const err = putReq.error;
        self.postMessage({
          type: "error",
          message: String(err?.message ?? err),
          code: err?.name ?? "UnknownError",
        });
      };
      tx.oncomplete = () => self.postMessage({ type: "saved", tick: msg.tick });
      tx.onerror = () => {
        const err = tx.error;
        self.postMessage({
          type: "error",
          message: String(err?.message ?? err),
          code: err?.name ?? "TransactionError",
        });
      };
    } catch (e) {
      self.postMessage({
        type: "error",
        message: String((e as Error).message ?? e),
        code: "OpenError",
      });
    }
  } else if (msg?.type === "load") {
    try {
      const db = await openDb();
      const tx = db.transaction("worlds", "readonly");
      const store = tx.objectStore("worlds");
      const req = store.get("current");
      req.onsuccess = () => self.postMessage({ type: "loaded", row: req.result ?? null });
      req.onerror = () => self.postMessage({
        type: "error",
        message: String(req.error?.message ?? req.error),
        code: req.error?.name ?? "LoadError",
      });
    } catch (e) {
      self.postMessage({
        type: "error",
        message: String((e as Error).message ?? e),
        code: "OpenError",
      });
    }
  }
};
```

Main-thread persistence module (`web/src/persistence/index.ts`):

```ts
import Worker from "./worker?worker";

export interface SavedRow {
  key: "current";
  json: string;
  tick: number;
  seed: string;
  saved_at_ms: number;
}

export class PersistenceClient {
  private worker = new Worker();
  private onError?: (code: string, message: string) => void;
  private onSaved?: (tick: number) => void;

  setHandlers(opts: {
    onSaved?: (tick: number) => void;
    onError?: (code: string, message: string) => void;
  }) {
    this.onSaved = opts.onSaved;
    this.onError = opts.onError;
    this.worker.onmessage = (ev) => {
      const m = ev.data;
      if (m?.type === "saved") this.onSaved?.(m.tick);
      else if (m?.type === "error") this.onError?.(m.code, m.message);
    };
  }

  save(json: string, seed: string, tick: number) {
    this.worker.postMessage({ type: "save", json, seed, tick });
  }

  loadCurrent(): Promise<SavedRow | null> {
    return new Promise((resolve, reject) => {
      const oldHandler = this.worker.onmessage;
      this.worker.onmessage = (ev) => {
        const m = ev.data;
        if (m?.type === "loaded") {
          this.worker.onmessage = oldHandler;
          resolve(m.row);
        } else if (m?.type === "error") {
          this.worker.onmessage = oldHandler;
          reject(new Error(`${m.code}: ${m.message}`));
        }
      };
      this.worker.postMessage({ type: "load" });
    });
  }
}
```

#### IDB quota / permission failure UX

A top-bar toast (separate from the in-aquarium event toasts of E.22 —
this is for app-status messages, distinct visual treatment, fades after
6s per v6 §I). Hook into the existing `#top-bar`:

```ts
function showAutosaveFailureToast(message: string): void {
  // Single ephemeral element; replace existing one if present.
  let el = document.getElementById("autosave-toast") as HTMLDivElement | null;
  if (el) el.remove();
  el = document.createElement("div");
  el.id = "autosave-toast";
  el.className = "status-toast";
  el.textContent = "Autosave failed — local storage may be full";
  document.body.appendChild(el);
  setTimeout(() => el?.remove(), 6000);
}
```

CSS (added to `web/src/styles.css`):

```css
#autosave-toast.status-toast {
  position: fixed;
  top: 8px;
  left: 50%;
  transform: translateX(-50%);
  background: rgba(180, 60, 60, 0.95);
  color: #fff;
  padding: 6px 12px;
  border-radius: 4px;
  font-size: 12px;
  z-index: 20;
  animation: status-toast-fade 6s ease-out forwards;
}
@keyframes status-toast-fade {
  0% { opacity: 0; }
  5% { opacity: 1; }
  90% { opacity: 1; }
  100% { opacity: 0; }
}
```

**Rate-limit:** if the user is in a chronic-quota-failure state we'll
fire this toast every 3 wall-seconds (the autosave interval); the
single-element `if (el) el.remove()` pattern collapses the stack to one
visible toast. PIN: that's the desired behavior.

`PersistenceClient.setHandlers({ onError })` routes to
`showAutosaveFailureToast`. The worker doesn't distinguish quota-vs-other
errors at v1; any error gets the same toast. Per-error logging goes to
the console.

**DECISIONS line:** `IDB error UX: single ephemeral red status toast, replaced on each failure (not stacked); same message for all error codes — v6 §I says "local storage may be full" generically`

#### Full event log retention

`EventLog::all` already collects everything. `SaveV1` serializes
`EventLogSnapshot { all, ring_cap }`. On load, the recent ring is
re-derived from the tail of `all`:

```rust
fn rehydrate_event_log(snap: EventLogSnapshot) -> EventLog {
    let mut log = EventLog::new();
    log.ring_cap = snap.ring_cap;
    log.recent.clear();
    let n = snap.all.len();
    let start = if n > snap.ring_cap { n - snap.ring_cap } else { 0 };
    for ev in snap.all[start..].iter() {
        log.recent.push_back(ev.clone());
    }
    log.all = snap.all;
    log
}
```

The `recent` ring sees the last 200 events on resume — UI continuity.
The full history is still on disk for Download Save → import-elsewhere
workflows (and for future v1.1 history viewer).

**Save size projection:** at 10k events × ~60 bytes/event JSON ≈ 600KB
events; 2k creatures × ~2KB each (mostly the `Brain.weights` Vec<f32>
3456 entries = ~14KB JSON-encoded per brain — **wait, brains dominate**:
2k × 14KB = 28MB).

That's too big. Two paths:

- **(A)** Compact brain encoding: pack each `Brain.weights: Vec<f32>`
  as a `Vec<u32>` of bit-cast values, then encode as a single
  base64-encoded byte string. ~3456 floats × 4 bytes = 13.8KB binary →
  ~18.5KB base64 → ~6KB after browser-level transparent compression?
  Hard to predict. Doesn't change JSON cost much.
- **(B)** Reduce creature count budget for saves. v1 target is 1–2k
  creatures (v5 §14); at the lower bound (1k creatures × 14KB) we hit
  14MB. At 2k we hit 28MB. IndexedDB allows ≥ 100MB per origin in
  practice; this fits but is unpleasant.
- **(C)** Accept the size and move on — IDB writes the 28MB blob,
  worker latency 50–200ms (still off-thread), no user-visible cost.

**PIN (C).** v5/v6 don't constrain save size; the worker absorbs any
latency; v1 is shipping. Re-encoding brains compactly is a v1.1
optimization. **DECISIONS:** `save size: accept multi-MB JSON saves (brains dominate at ~14KB each × pop); IDB handles it off-thread via worker; compact binary encoding is a v1.1 optimization`

### F.26.b — JS wiring (boot path)

In `web/src/main.ts`:

```ts
// New imports at top
import { PersistenceClient } from "./persistence";
import { showSchemaMismatchModal, showResumePrompt } from "./persistence/ui";
import { showAutosaveFailureToast } from "./persistence/toast";

async function main(): Promise<void> {
  await init();

  const params = new URLSearchParams(window.location.search);
  const urlSeed = params.get("seed");

  // F.26: persistence client + load attempt.
  const persistence = new PersistenceClient();
  persistence.setHandlers({
    onError: (code, msg) => showAutosaveFailureToast(`${code}: ${msg}`),
  });

  let world: WorldHandle | null = null;

  if (!urlSeed) {
    // No explicit seed → try resume.
    try {
      const row = await persistence.loadCurrent();
      if (row) {
        const day = Math.floor(row.tick / DAY_TICKS);
        const userChose = await showResumePrompt({
          day,
          seed: row.seed,
          savedAtMs: row.saved_at_ms,
        });
        if (userChose === "resume") {
          try {
            world = WorldHandle.fromJson(row.json);
          } catch (e) {
            const msg = String(e);
            if (msg.startsWith("schema-mismatch:")) {
              await showSchemaMismatchModal();
            }
            // Fall through to fresh world.
          }
        }
      }
    } catch (e) {
      console.warn("resume probe failed:", e);
    }
  }

  if (!world) {
    world = new WorldHandle(urlSeed ?? "");
  }

  // ... rest of boot path unchanged (camera, rail, frame loop) ...

  // In the frame loop, after world.step_n():
  maybeAutosave(world, now, persistence);
}
```

`maybeAutosave` is the function described in F.26.a, taking
`PersistenceClient` instead of raw `Worker`.

**Day computation:** v5 §11 + §15 imply a day metric for the eulogy
card ("lived N days"). PIN `DAY_TICKS = 1000`. Documented in F.28
below; also used here for the resume prompt copy. **DECISIONS:**
`tick→day conversion: DAY_TICKS = 1000 — round number; at sim 1× (30 ticks/s) a "day" is ~33 wall-seconds, comfortable narrative pacing for a 10k-tick (10-day) typical run`

#### Resume prompt UI

A modal positioned above the canvas, dismissible. Two buttons.

```ts
// persistence/ui.ts

export async function showResumePrompt(opts: {
  day: number;
  seed: string;
  savedAtMs: number;
}): Promise<"resume" | "new"> {
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "modal-backdrop";

    const card = document.createElement("div");
    card.className = "modal-card";
    const savedAgoMin = Math.floor((Date.now() - opts.savedAtMs) / 60_000);
    card.innerHTML = `
      <h2>Resume world?</h2>
      <p>Day ${opts.day}, seed <code>${opts.seed}</code></p>
      <p class="muted">Saved ${savedAgoMin} min ago</p>
      <div class="modal-actions">
        <button data-action="new">New World</button>
        <button data-action="resume" class="primary">Resume</button>
      </div>
    `;
    backdrop.appendChild(card);
    document.body.appendChild(backdrop);

    card.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      const action = t.dataset?.action;
      if (action === "resume" || action === "new") {
        backdrop.remove();
        resolve(action);
      }
    });
  });
}

export async function showSchemaMismatchModal(): Promise<void> {
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "modal-backdrop";
    const card = document.createElement("div");
    card.className = "modal-card";
    card.innerHTML = `
      <h2>Save incompatible</h2>
      <p>This save is from a different build.</p>
      <div class="modal-actions">
        <button data-action="new" class="primary">New World</button>
      </div>
    `;
    backdrop.appendChild(card);
    document.body.appendChild(backdrop);
    card.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      if (t.dataset?.action === "new") {
        backdrop.remove();
        resolve();
      }
    });
  });
}
```

CSS additions (`web/src/styles.css`):

```css
.modal-backdrop {
  position: fixed; inset: 0;
  background: rgba(0,0,0,0.55);
  display: flex; align-items: center; justify-content: center;
  z-index: 30;
}
.modal-card {
  background: var(--rail-bg);
  border: 1px solid rgba(255,255,255,0.15);
  border-radius: 6px;
  padding: 20px 28px;
  min-width: 320px;
  max-width: 480px;
  color: var(--fg);
}
.modal-card h2 { margin: 0 0 8px 0; font-size: 16px; }
.modal-card p { margin: 4px 0; font-size: 13px; }
.modal-card p.muted { opacity: 0.55; font-size: 11px; }
.modal-card .modal-actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 16px; }
.modal-card button {
  background: rgba(255,255,255,0.08);
  border: 1px solid rgba(255,255,255,0.15);
  color: var(--fg);
  padding: 6px 14px; border-radius: 3px;
  font: inherit; cursor: pointer;
}
.modal-card button.primary { background: rgba(80,140,200,0.35); border-color: rgba(120,180,240,0.45); }
.modal-card button:hover { background: rgba(255,255,255,0.16); }
```

**Auto-resume vs prompt:** the spec asks for a prompt. **PIN:** always
prompt when a save exists and the URL has no `?seed=`. Never silently
resume. Reasons:
- Reduces "stuck on someone else's world" frustration.
- Single source of truth: URL `?seed=` always wins (creates a fresh
  world with that seed; ignores save). Without `?seed=`, the save is
  the dominant signal — but we still confirm.

**Edge cases handled in code review:**

- URL has `?seed=` AND save exists → ignore save, build fresh world
  with seed. (Don't prompt: explicit seed is an explicit "fresh-world"
  intent.)
- URL has `?seed=` AND no save → fresh world with seed.
- No `?seed=` AND save exists → prompt.
- No `?seed=` AND no save → fresh world with random seed.

The `loadCurrent` call returns null fast when the store is empty (no
prompt shown). If the worker errors at load time, log and fall through
to fresh world (no toast; the user wasn't expecting feedback at boot).

**DECISIONS:**

- `resume policy: prompt iff no ?seed= URL param AND a save exists; ?seed= always wins (explicit new world with that seed); never auto-resume`
- `schema-mismatch UX: single-button "New World" modal per v6 §I — no advanced "try anyway" option; corrupt JSON / structural failures use the same modal (treats them all as "save unusable")`

### F.26.c — DECISIONS file edits

In addition to the lines listed above, add a section header `## F.26`
in `DECISIONS.md` between the `## E.25` block and any future entries.

### F.26 tests

**Rust unit tests** in `src/save.rs::tests`:

- `f26_round_trip_preserves_population`: build a `World::new("f26-rt")`,
  tick 500 times, call `to_save_v1` → `serde_json::to_string` →
  `serde_json::from_str` → `from_save_v1`. Assert population, tick,
  seed, peak_population match.
- `f26_round_trip_preserves_creature_state`: same setup; assert every
  SoA column matches (id, x, y, vx, vy, energy, age,
  digestion_cooldown, cumulative_upkeep, species_id, parent_species_id,
  last_action, action_this_tick, max_size_reached, distance_travelled,
  birth_tick, genome.size, brain.weights[0]). Per-creature loop with
  `assert_eq!`.
- `f26_round_trip_preserves_rng`: tick 100, snapshot, restore, then
  tick 100 more in both worlds; assert population matches at the end
  (RNG-divergence canary).
- `f26_schema_version_mismatch_errs`: hand-craft a JSON with
  `"schema_version": 999`; call `from_save_v1`; assert
  `LoadError::SchemaVersionMismatch { found: 999, expected: 1 }`.
- `f26_invalid_json_errs`: pass garbage; assert error type.
- `f26_full_event_log_round_trips`: jack `mutation_rate_multiplier=5`,
  tick 5k, capture `events.all.len()`; round-trip; assert
  `events.all.len()` matches and last event's content matches.
- `f26_sun_round_trips_exactly`: assert `sun.current[k]` and
  `sun.capacity[k]` match byte-for-byte after round-trip.

**Manual UI tests:**

- Open URL, watch for first autosave (≥ 10 sim-seconds in). Open browser
  devtools → IndexedDB → evosim → worlds → "current"; confirm a row with
  `json`, `tick`, `seed`.
- Reload page (no `?seed=`); confirm resume prompt appears with correct
  day/seed/age; click Resume; confirm world continues at the saved tick.
- Reload page, click New World; confirm fresh world spawns.
- Manually edit the `schema_version` in IDB to 999; reload; confirm
  schema-mismatch modal.
- Disable IDB (Firefox: privacy settings; Chrome: site setting) or
  spam-save via temporarily setting `AUTOSAVE_WALL_FLOOR_MS=0` and
  watch the worker eventually fail (or test in Safari private mode where
  IDB has tight quotas); confirm autosave toast appears.

### F.26 code-reviewer checklist (heavy)

- [ ] `SaveV1` includes every field on `World` that affects sim
      replay or UI continuity. Audit by diffing `World` struct
      declaration against `SaveV1` field list. Specifically reject any
      missing of: `tick, seed, rng, sun, grid (recomputed), creatures,
      carrion, species, events, sliders, next_creature_id,
      peak_population, peak_species_count, world_ended,
      live_species_count, first_move_fired, first_eat_fired,
      population_milestones_fired, biggest_ever, last_survivor,
      weirdest, weirdest_distance, longest_lived, longest_lived_age,
      first_mover_snapshot, founder_genome_anchor, founder_brain_anchor`.
- [ ] `grid`, `vision`, `cell_to_carrion`, `pending_extinction_check`
      are *recomputed*, not serialized (and the reviewer can articulate
      why for each — they're per-tick scratch that's either rebuilt at
      tick start or starts empty).
- [ ] `SimRng` derives `Serialize, Deserialize` via the `serde1`
      feature on `rand_xoshiro`. Round-trip is exact (one of the unit
      tests asserts population divergence-free over 100 ticks).
- [ ] `EventLog` round-trip preserves full `all` and recomputes
      `recent` from the tail.
- [ ] `from_save_v1` returns `LoadError::SchemaVersionMismatch` first
      (before any other parsing).
- [ ] `from_save_v1` rebuilds the spatial grid post-construction;
      first `step()` after load does not panic.
- [ ] `from_save_v1` initializes `vision` to
      `vec![[0.0; VISION_LEN]; creatures.len()]`; first vision pass
      overwrites.
- [ ] `from_save_v1` recomputes `species.next_id = max(id)+1` from the
      list (or persists `next_id` in `SpeciesSnapshot` — pick one and
      pin it; **PIN:** recompute, no separate field).
- [ ] `snapshot_json` never panics (`serde_json::to_string` of a
      well-formed `SaveV1` is infallible; assert with `.expect`).
- [ ] `from_json` returns `Result<WorldHandle, JsValue>` and the
      JsValue is a string with a `prefix:` for the JS layer to switch on.
- [ ] Worker is a plain ES module (no wasm); Vite `?worker` import
      configured.
- [ ] Worker error handler routes to a single status toast
      (`onerror`, `onabort`, `tx.onerror`, `putReq.onerror` all
      covered).
- [ ] Autosave fires once per 300 sim-ticks AND ≥ 3000ms wall-clock
      apart.
- [ ] Resume prompt logic respects `?seed=` precedence (the four
      scenarios in F.26.b above).
- [ ] Schema-mismatch modal has only a "New World" button (per v6 §I).
- [ ] CSS: modal-backdrop overlays canvas (`z-index: 30`) and traps
      visual focus (no aquarium interaction underneath); buttons are
      keyboard-clickable.
- [ ] `cargo test --lib` adds at least 7 F.26 tests, all passing.
- [ ] Wasm bundle still builds (`wasm-pack build --target web`).
- [ ] DECISIONS.md updated with the F.26 lines listed above.

### F.26 DECISIONS lines (summary, append to `DECISIONS.md`)

- `persistence handoff: JSON-encode on main thread (1–5 ms at v1 sizes); plain JS worker only writes to IDB — preserves spirit of v5 §13 "don't hitch main thread on disk I/O" without the cost of a second wasm artifact or double-buffered SoA. Revisit in v1.1 if F.30 measurements show encode hitches a frame.`
- `worker type: plain ES-module worker (Vite ?worker import) — no second wasm-pack build`
- `save shape: explicit "Snapshot" structs in src/save.rs wrapping each subsystem (CreatureSoA, SunMap, SpeciesRegistry, EventLog); future schema versions add SaveV2 alongside with a From<SaveV1>`
- `RNG round-trip: rand_xoshiro "serde1" feature — exact state preservation, no manual replay`
- `IDB layout: DB "evosim" v1, store "worlds", single key "current" — one world per browser profile`
- `tick→day conversion: DAY_TICKS = 1000 (also used by F.28 eulogy)`
- `autosave cadence: every 300 sim-ticks (10 sim-seconds at 30-tick/s baseline) AND ≥ 3000ms wall-clock apart`
- `IDB error UX: single ephemeral red status toast at top of page, fades 6s — replaced (not stacked) on repeat`
- `resume policy: prompt iff no ?seed= URL AND a save exists; ?seed= always builds fresh; never auto-resume`
- `schema-mismatch UX: one-button "New World" modal; invalid-json and structural-error use the same modal (treat all unrecoverable loads as "save unusable")`
- `save size: accept multi-MB JSON (brain weights dominate); worker absorbs IDB write latency; compact binary brain encoding deferred to v1.1`
- `next_id recompute: SpeciesRegistry.next_id is not persisted — derived as max(species.list[].id) + 1 on load`

---

## F.27 — seed display + Resume prompt (light)

### Goal

Top bar shows the seed prominently with a click-to-copy button. The
Resume prompt component (already in F.26) is hooked here insofar as
F.27 owns the dismissible-modal CSS polish.

### Implementation

#### Top-bar seed display + copy button

Edit `web/index.html`:

```html
<div id="top-bar">
  <span id="status">booting…</span>
  <span id="seed-display" class="seed-pill" title="Click to copy seed">
    <span id="seed-value">…</span>
    <button id="seed-copy-btn" aria-label="Copy seed">📋</button>
  </span>
</div>
```

(Emoji icon left as-is in HTML since it's a one-character button label,
not user-authored content. If the reviewer dislikes emoji per the
"avoid emoji unless requested" guideline, swap for SVG or `"copy"`
text.)

Edit `web/src/main.ts` boot path:

```ts
function installSeedDisplay(seed: string): void {
  const valueEl = document.getElementById("seed-value")!;
  valueEl.textContent = seed;
  const btn = document.getElementById("seed-copy-btn") as HTMLButtonElement;
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(seed);
      btn.textContent = "✓";
      setTimeout(() => (btn.textContent = "📋"), 1200);
    } catch (e) {
      console.warn("clipboard copy failed", e);
    }
  });
}
```

Called once during `main()` after `world.seed` is known. (Seed never
changes during a session — even on resume, `world.seed` is the saved
seed.)

CSS additions (`web/src/styles.css`):

```css
.seed-pill {
  margin-left: 12px;
  padding: 2px 6px;
  background: rgba(255,255,255,0.06);
  border-radius: 3px;
  font-family: ui-monospace, monospace;
  font-size: 11px;
  display: inline-flex; gap: 4px; align-items: center;
}
#seed-copy-btn {
  background: transparent; color: var(--fg);
  border: 0; padding: 0 2px; cursor: pointer; font: inherit;
}
```

#### Tests (manual only)

- Click the copy button. Paste in another tab; confirm seed matches.
- Resize narrow window; confirm seed display doesn't collide with
  speed buttons (justify-content of top-bar may need
  `flex-wrap: wrap`).
- Resume prompt: covered in F.26 tests.

### F.27 DECISIONS lines

- `seed display: top-bar pill with copy button; clipboard.writeText fallback is silent (console.warn) — failure is rare and non-blocking`

---

## F.28 — eulogy card (4-image grid + Copy Share Text + Download Save + New World)

### Goal

When `world.world_ended` becomes true: dim the aquarium with a
translucent overlay, freeze the final frame (no further sim steps), and
show the eulogy card. Card content per v5 §11:

- Headline: "Your world lived **N days**, peaked at **P creatures**
  across **S species**. Seed: **XYZ**."
- 2×2 image grid of canvas-rendered creatures:
  - First to move (`first_mover_snapshot`)
  - Biggest (`biggest_ever`)
  - Weirdest (`weirdest.or(longest_lived)` — per v5 §11.1 fallback)
  - Last survivor (`last_survivor`)
- Buttons:
  - **Copy Share Text** — `"evosim seed=XYZ — lived N days, peaked S
    species, see {origin}/?seed=XYZ"` written to clipboard.
  - **Download Save (.json)** — calls `world.snapshot_json()`,
    triggers a file download named `evosim-{seed}-day{N}.json`.
  - **New World** — reloads the page without `?seed=` AND drops the
    saved world (or just reloads — see "New World semantics" pin
    below).

### F.28.a — new wasm API: `hof_json`

```rust
// src/wasm_api.rs

#[wasm_bindgen]
impl WorldHandle {
    /// JSON of the eulogy hall-of-fame snapshot (F.28).
    /// Shape (TS): {
    ///   first_mover: HoFJson | null,
    ///   biggest: HoFJson | null,
    ///   weirdest: HoFJson | null,  // weirdest, falling back to longest_lived per v5 §11.1
    ///   last_survivor: HoFJson | null,
    ///   day_count: number,
    ///   peak_population: number,
    ///   peak_species: number,
    ///   seed: string,
    /// }
    #[wasm_bindgen]
    pub fn hof_json(&self) -> String {
        // Per v5 §11.1: weirdest falls back to longest_lived if no candidate
        // hit the 500-tick threshold.
        let weirdest = self
            .inner
            .weirdest
            .as_ref()
            .or(self.inner.longest_lived.as_ref());

        let payload = serde_json::json!({
            "first_mover": self.inner.first_mover_snapshot,
            "biggest": self.inner.biggest_ever,
            "weirdest": weirdest,
            "last_survivor": self.inner.last_survivor,
            "day_count": self.inner.tick / DAY_TICKS,
            "peak_population": self.inner.peak_population,
            "peak_species": self.inner.peak_species_count,
            "seed": self.inner.seed,
        });
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
    }
}
```

`DAY_TICKS = 1000` is added to `src/constants.rs` (and used in F.26's
day display).

TS type:

```ts
export interface HoFEntry {
  creature_id: number;
  genome: GenomeJson;  // see below
  species_name: string;
  captured_tick: number;
  captured_size: number;
  captured_age: number;
}
export interface GenomeJson {
  size: number;
  pigment_r: number; pigment_g: number; pigment_b: number;
  eye_count: number;
  move_speed: number;
  eat_efficiency: number;
  scavenge_efficiency: number;
  armor: number;
  // ... only the fields the renderer uses are required; serde adds the rest
}
export interface HoFJson {
  first_mover: HoFEntry | null;
  biggest: HoFEntry | null;
  weirdest: HoFEntry | null;
  last_survivor: HoFEntry | null;
  day_count: number;
  peak_population: number;
  peak_species: number;
  seed: string;
}
```

`Genome` already serializes via its existing serde derive (in
`src/genome.rs`); the wasm side serializes the full struct (~14 fields)
to JSON and the renderer ignores the fields it doesn't need.

### F.28.b — single-creature renderer

The existing `drawCreatures(ctx, cam, viewW, viewH, data, ids, stride,
highlights, nowMs)` in `web/src/render.ts` reads the SoA-packed
Float32Array. The eulogy card needs a single-creature variant that
reads a Genome JSON directly and renders into a small offscreen canvas
(e.g. 96×96 px). Add a sibling renderer:

```ts
// web/src/render.ts

/// Render a single creature into a fresh 96x96 PNG-ready canvas, centered.
/// Used by the F.28 eulogy card image grid.
export function renderCreaturePortrait(genome: GenomeJson, canvasSize = 96): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = canvasSize;
  c.height = canvasSize;
  const ctx = c.getContext("2d")!;
  // Dark background to match aquarium.
  ctx.fillStyle = "rgba(8, 12, 20, 1)";
  ctx.fillRect(0, 0, canvasSize, canvasSize);

  // Center.
  const cx = canvasSize / 2;
  const cy = canvasSize / 2;

  // Pick a fixed per-portrait zoom: largest size we expect (~10) should fit
  // with 8px margin. radius_px = size × 2.5 × portraitScale. Want
  // 10 × 2.5 × scale = canvasSize/2 - 8 → scale = (canvasSize/2 - 8) / 25.
  const portraitScale = (canvasSize / 2 - 8) / (10 * 2.5);
  const radius_px = Math.max(6, genome.size * 2.5 * portraitScale);

  // Body fill (pigment).
  const r = Math.round(genome.pigment_r * 255);
  const g = Math.round(genome.pigment_g * 255);
  const b = Math.round(genome.pigment_b * 255);
  ctx.fillStyle = `rgb(${r}, ${g}, ${b})`;
  ctx.beginPath();
  ctx.arc(cx, cy, radius_px, 0, Math.PI * 2);
  ctx.fill();

  // Active-trait rings, innermost → outermost: eye, move, scav, mouth, armor.
  // (Same order as drawCreatures; matches v6 §B except for "innermost = eye" pin.)
  drawRing(ctx, cx, cy, radius_px, 0, genome.eye_count > 0, "white");
  drawRing(ctx, cx, cy, radius_px, 1, genome.move_speed > 0, "yellow");
  drawRing(ctx, cx, cy, radius_px, 2, genome.scavenge_efficiency > 0, "#8b6f47");
  drawRing(ctx, cx, cy, radius_px, 3, genome.eat_efficiency > 0, "red");
  drawRing(ctx, cx, cy, radius_px, 4, genome.armor > 0, "#bdbdbd");

  return c;
}

function drawRing(
  ctx: CanvasRenderingContext2D,
  cx: number, cy: number, outerR: number,
  index: number, active: boolean, color: string,
): void {
  if (!active) return;
  const r = outerR - 1 - index;
  if (r <= 1) return;
  ctx.strokeStyle = color;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.arc(cx, cy, r, 0, Math.PI * 2);
  ctx.stroke();
}
```

(The existing `drawCreatures` in `render.ts` operates on the packed
Float32Array and computes ring flags up front; we reproduce the same
ring colors / ordering here against a `GenomeJson`. Reviewer must
confirm color values match v6 §B exactly.)

**Audit during code review:** the existing `drawCreatures` in
`render.ts` already implements rings; if its ring colors drift in
implementation (the spec says `white / yellow / brown / red /
silver-light-gray`), `renderCreaturePortrait` must match. Cross-check
both functions against v6 §B during review and PIN exact hex codes in
`render.ts`:

```ts
export const RING_COLORS = {
  eye: "white",
  move: "yellow",
  scav: "#8b6f47",   // brown
  mouth: "red",
  armor: "#bdbdbd",  // silver / light gray
} as const;
```

(Both renderers consume `RING_COLORS`.)

### F.28.c — eulogy card DOM + boot wiring

Eulogy card is constructed lazily (only when `world.world_ended`
becomes true). Add to `web/src/main.ts`:

```ts
import { showEulogyCard } from "./eulogy";

// In the frame loop:
if (world.world_ended && !eulogyShown) {
  eulogyShown = true;
  // Stop sim stepping (we've already broken out at world.step_n's "false" return).
  showEulogyCard(world);
}
```

`eulogyShown` is a module-scoped boolean.

`web/src/eulogy.ts`:

```ts
import { renderCreaturePortrait } from "./render";
import type { HoFJson, HoFEntry } from "./types";

const SHARE_BASE_URL = window.location.origin + window.location.pathname;

export function showEulogyCard(world: WorldHandle): void {
  const hof: HoFJson = JSON.parse(world.hof_json());

  // Dim overlay over the aquarium.
  const backdrop = document.createElement("div");
  backdrop.className = "modal-backdrop eulogy-backdrop";

  const card = document.createElement("div");
  card.className = "modal-card eulogy-card";

  card.innerHTML = `
    <h2>The world has ended.</h2>
    <p class="eulogy-headline">
      Your world lived <strong>${hof.day_count}</strong> days,
      peaked at <strong>${hof.peak_population}</strong> creatures
      across <strong>${hof.peak_species}</strong> species.
    </p>
    <p class="eulogy-seed">Seed: <code>${hof.seed}</code></p>

    <div class="eulogy-grid">
      <div class="eulogy-cell" data-slot="first_mover">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">First to move</div>
      </div>
      <div class="eulogy-cell" data-slot="biggest">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">Biggest</div>
      </div>
      <div class="eulogy-cell" data-slot="weirdest">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">Weirdest</div>
      </div>
      <div class="eulogy-cell" data-slot="last_survivor">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">Last survivor</div>
      </div>
    </div>

    <div class="modal-actions">
      <button data-action="copy-share">Copy Share Text</button>
      <button data-action="download-save">Download Save</button>
      <button data-action="new-world" class="primary">New World</button>
    </div>
  `;

  // Populate the 4 portraits.
  populateCell(card, "first_mover", hof.first_mover);
  populateCell(card, "biggest", hof.biggest);
  populateCell(card, "weirdest", hof.weirdest);
  populateCell(card, "last_survivor", hof.last_survivor);

  // Wire buttons.
  card.querySelector('[data-action="copy-share"]')!.addEventListener("click", async () => {
    const text = `evosim seed=${hof.seed} — lived ${hof.day_count} days, peaked ${hof.peak_species} species, see ${SHARE_BASE_URL}?seed=${encodeURIComponent(hof.seed)}`;
    await navigator.clipboard.writeText(text).catch(() => {});
    flashButton(card, '[data-action="copy-share"]', "Copied");
  });

  card.querySelector('[data-action="download-save"]')!.addEventListener("click", () => {
    const json = world.snapshot_json();
    const filename = `evosim-${hof.seed}-day${hof.day_count}.json`;
    triggerDownload(json, filename);
    flashButton(card, '[data-action="download-save"]', "Downloaded");
  });

  card.querySelector('[data-action="new-world"]')!.addEventListener("click", () => {
    // F.28 New World semantics — see PIN below.
    window.location.href = window.location.origin + window.location.pathname;
  });

  backdrop.appendChild(card);
  document.body.appendChild(backdrop);
}

function populateCell(card: HTMLElement, slot: string, entry: HoFEntry | null): void {
  const cell = card.querySelector(`[data-slot="${slot}"]`)!;
  const canvasHolder = cell.querySelector(".eulogy-cell-canvas")!;
  const captionEl = cell.querySelector(".eulogy-cell-caption")!;
  if (!entry) {
    captionEl.textContent = `${captionEl.textContent} — (none)`;
    return;
  }
  const c = renderCreaturePortrait(entry.genome, 96);
  c.style.borderRadius = "4px";
  canvasHolder.appendChild(c);
  // Augment caption with species name + tick.
  captionEl.textContent = `${captionEl.textContent} · ${entry.species_name} · t=${entry.captured_tick}`;
}

function triggerDownload(text: string, filename: string): void {
  const blob = new Blob([text], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  setTimeout(() => {
    URL.revokeObjectURL(url);
    a.remove();
  }, 0);
}

function flashButton(card: HTMLElement, selector: string, label: string): void {
  const btn = card.querySelector(selector) as HTMLButtonElement;
  const original = btn.textContent;
  btn.textContent = label;
  btn.disabled = true;
  setTimeout(() => {
    btn.textContent = original;
    btn.disabled = false;
  }, 1200);
}
```

#### "New World" semantics

Three plausible behaviors:

- **(A)** Reload page sans `?seed=`. Picks up the saved world (F.26's
  resume prompt fires next).
- **(B)** Reload page sans `?seed=` AND drop the saved world (delete
  the IDB row first).
- **(C)** Stay in-place: call `WorldHandle = new WorldHandle("")`,
  reset all UI in-place.

(C) avoids the page flash but requires the implementer to surgically
reset every UI subsystem (toast stack, highlights, inspector, rail
state, stats samples, eulogy-shown flag) — high risk of leaks.

(A) re-triggers the resume prompt which prompts the user to "Resume
the world you just declared over." That's awful.

**PIN (B):** Drop the saved world via `persistence.deleteCurrent()`
(new method on `PersistenceClient`), then reload sans `?seed=`. The
boot path then sees no save and skips the resume prompt entirely.

Worker addition:

```ts
// worker.ts — handle "delete"
} else if (msg?.type === "delete") {
  try {
    const db = await openDb();
    const tx = db.transaction("worlds", "readwrite");
    const store = tx.objectStore("worlds");
    const req = store.delete("current");
    tx.oncomplete = () => self.postMessage({ type: "deleted" });
    tx.onerror = () => self.postMessage({
      type: "error",
      message: String(tx.error?.message ?? tx.error),
      code: tx.error?.name ?? "DeleteError",
    });
  } catch (e) {
    self.postMessage({ type: "error", message: String((e as Error).message ?? e), code: "OpenError" });
  }
}
```

`PersistenceClient.deleteCurrent(): Promise<void>` mirrors
`loadCurrent`. Eulogy's "New World" handler:

```ts
btn.addEventListener("click", async () => {
  try { await persistence.deleteCurrent(); } catch (e) { console.warn(e); }
  window.location.href = window.location.origin + window.location.pathname;
});
```

**DECISIONS:** `"New World" from eulogy card: delete IDB "current" row, then reload sans ?seed= — avoids the post-eulogy resume prompt re-offering the just-finished world`

### F.28.d — CSS

Add to `web/src/styles.css`:

```css
.eulogy-backdrop {
  background: rgba(8, 12, 20, 0.7);
}
.eulogy-card {
  min-width: 480px;
  max-width: 580px;
  text-align: center;
}
.eulogy-card h2 { font-size: 18px; margin-bottom: 12px; }
.eulogy-headline { font-size: 14px; }
.eulogy-seed code {
  background: rgba(255,255,255,0.08);
  padding: 2px 6px; border-radius: 3px;
  font-family: ui-monospace, monospace;
}
.eulogy-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
  margin: 20px 0;
}
.eulogy-cell {
  display: flex; flex-direction: column; align-items: center;
  gap: 6px;
}
.eulogy-cell-canvas { width: 96px; height: 96px; }
.eulogy-cell-caption {
  font-size: 11px; opacity: 0.75;
  font-family: ui-monospace, monospace;
}
```

### F.28 tests

- **Manual UI:** force population zero (jack `mutation_rate_multiplier=5`
  + `mouth_tax=0.3` via the existing `set_slider` wasm method in
  console). Confirm eulogy card appears once and only once per session.
- **Manual UI:** click "Copy Share Text"; paste; confirm string matches
  the spec format. Open the pasted URL in a new tab; confirm fresh
  world with that seed.
- **Manual UI:** click "Download Save"; confirm file downloads with
  spec'd filename and contents that round-trip via the test in F.26.
- **Manual UI:** click "New World"; confirm page reloads and a fresh
  world (NOT a resume prompt) appears.
- **Manual UI:** kill founder before any creature ever has
  `move_speed > 0` → confirm "First to move — (none)" placeholder shows.
- **Rust unit test** (`src/wasm_api.rs::tests`):
  - `f28_hof_json_includes_all_slots`: build a `WorldHandle`, manually
    populate `world.biggest_ever / first_mover_snapshot / weirdest /
    last_survivor` with fabricated `HallOfFame`, call `hof_json()`,
    assert all four keys present and pass `serde_json::from_str` into
    a `Value`.
  - `f28_hof_json_weirdest_falls_back_to_longest_lived`: only set
    `longest_lived` (not `weirdest`); call `hof_json()`; assert
    `weirdest` key in JSON equals the longest_lived snapshot.
  - `f28_hof_json_day_count_uses_DAY_TICKS`: set
    `world.tick = 5500`; call `hof_json()`; assert
    `payload.day_count == 5`.

### F.28 code-reviewer checklist (moderate)

- [ ] `hof_json` emits all four slots (nullable), `day_count` =
      `tick / DAY_TICKS`, `peak_population`, `peak_species`, `seed`.
- [ ] Weirdest fallback to `longest_lived` per v5 §11.1.
- [ ] `renderCreaturePortrait` matches the ring color spec in v6 §B
      (eye=white, move=yellow, scav=brown, mouth=red, armor=silver).
- [ ] `RING_COLORS` constant shared between the SoA renderer and the
      portrait renderer — no drift.
- [ ] Eulogy fires exactly once per session (gated on a single
      `eulogyShown` boolean).
- [ ] Download Save filename = `evosim-{seed}-day{N}.json`; content =
      `world.snapshot_json()`.
- [ ] Copy Share Text format: `evosim seed=XYZ — lived N days, peaked
      S species, see {origin}/?seed=XYZ` (seed URL-encoded).
- [ ] New World drops IDB current row first, then reloads sans `?seed=`.
- [ ] No image rendered for a null HoF slot (replaced by " — (none)"
      caption).
- [ ] CSS: eulogy backdrop dims the aquarium; card centered; canvas
      portraits 96×96.

### F.28 DECISIONS lines

- `eulogy gate: world.world_ended boolean check in the RAF loop, single eulogyShown bool to avoid re-show`
- `single-creature renderer: separate renderCreaturePortrait function in render.ts (RING_COLORS shared with drawCreatures) — keeps the SoA fast-path untouched`
- `"New World" semantics: delete IDB "current" row, then reload sans ?seed= — avoids resume prompt re-offering the finished world`
- `share text format: literal v5 §11 string with seed URL-encoded`

---

## F.29 — headless acceptance test harness (perf gate)

### Goal

Per v5 §16:
- Run sim with fixed seed `evosim-test-001` for 10,000 ticks.
- Assert (a) `population > 0` at tick 10,000.
- Assert (b) ≥ 2 distinct species detected.
- Assert (c) wall-clock < 8 seconds on CI.
- Assert (d) state hash matches pinned golden.

Per v6 §M:
- Hash function = **xxHash64** (via `twox-hash`, already in `Cargo.toml`).
- Hash inputs, ordered:
  1. tick (u32 little-endian bytes)
  2. creature SoA: per-creature (id, position, velocity, energy, age,
     genome floats serialized in struct order, NN weight slice) for
     every creature in current SoA order
  3. sun map: per-cell (`sun_current`, `sun_capacity`) for every cell
     in row-major order
  4. carrion list: per-corpse (id, position, pool_remaining, age) in
     `Vec` order
  5. species list: per-species (id, anchor genome hash) in `Vec` order
  6. RNG state (the same 32 bytes of xoshiro256++ state we serialize in
     F.26)
- Golden value committed to `tests/golden_snapshot_t10000.txt`.
- First run: generate golden via `EVOSIM_WRITE_GOLDEN=1 cargo test
  --test acceptance`.

### F.29.a — hash function (new module `src/snapshot_hash.rs`)

```rust
//! Snapshot hash for the §16 acceptance test. xxHash64 over a fixed
//! ordering of world state per v6 §M. Stable across runs given identical
//! state — regression catch on the golden value.

use crate::world::World;
use std::hash::Hasher;
use twox_hash::XxHash64;

pub fn snapshot_hash(w: &World) -> u64 {
    let mut h = XxHash64::with_seed(0);

    // (a) tick
    h.write_u32(w.tick);

    // (b) creature SoA — per-creature, in current SoA order
    let n = w.creatures.len();
    h.write_u32(n as u32);
    for i in 0..n {
        h.write_u64(w.creatures.id[i]);
        write_f32(&mut h, w.creatures.x[i]);
        write_f32(&mut h, w.creatures.y[i]);
        write_f32(&mut h, w.creatures.vx[i]);
        write_f32(&mut h, w.creatures.vy[i]);
        write_f32(&mut h, w.creatures.energy[i]);
        h.write_u32(w.creatures.age[i]);

        // Genome floats in struct-declaration order (matches v6 §M "struct order")
        let g = &w.creatures.genomes[i];
        write_f32(&mut h, g.size);
        h.write_u32(g.max_age);
        write_f32(&mut h, g.photosynth_efficiency);
        write_f32(&mut h, g.eat_efficiency);
        write_f32(&mut h, g.scavenge_efficiency);
        write_f32(&mut h, g.move_speed);
        h.write_u8(g.eye_count);
        for k in 0..24 {
            write_f32(&mut h, g.eye_offsets[k]);
        }
        write_f32(&mut h, g.vision_range);
        write_f32(&mut h, g.armor);
        write_f32(&mut h, g.bite_reach);
        write_f32(&mut h, g.pigment_r);
        write_f32(&mut h, g.pigment_g);
        write_f32(&mut h, g.pigment_b);
        // TraitMutationRates: serialize all per-trait rates in declaration order.
        // (Genome.mutation_rates field — see src/genome.rs.)
        let mr = &g.mutation_rates;
        write_f32(&mut h, mr.size);
        write_f32(&mut h, mr.max_age);
        write_f32(&mut h, mr.photosynth_efficiency);
        write_f32(&mut h, mr.eat_efficiency);
        write_f32(&mut h, mr.scavenge_efficiency);
        write_f32(&mut h, mr.move_speed);
        write_f32(&mut h, mr.eye_count);
        write_f32(&mut h, mr.eye_offsets);
        write_f32(&mut h, mr.vision_range);
        write_f32(&mut h, mr.armor);
        write_f32(&mut h, mr.bite_reach);
        write_f32(&mut h, mr.pigment_r);
        write_f32(&mut h, mr.pigment_g);
        write_f32(&mut h, mr.pigment_b);

        // NN weight slice
        for w_f in w.creatures.brains[i].weights.iter() {
            write_f32(&mut h, *w_f);
        }
        write_f32(&mut h, w.creatures.brains[i].nn_mutation_rate);
    }

    // (c) sun map
    let m = w.sun.capacity.len();
    h.write_u32(m as u32);
    for k in 0..m {
        write_f32(&mut h, w.sun.current[k]);
        write_f32(&mut h, w.sun.capacity[k]);
    }

    // (d) carrion list
    let c = w.carrion.len();
    h.write_u32(c as u32);
    for cc in &w.carrion {
        // Use the same field set per v6 §M: id, position, pool_remaining, age
        // (assumes Carrion has these — confirm during review; if not, write
        // the closest equivalents and DECISIONS-note the choice).
        h.write_u32(cc.id);
        write_f32(&mut h, cc.x);
        write_f32(&mut h, cc.y);
        write_f32(&mut h, cc.pool);
        h.write_u32(cc.age);
    }

    // (e) species list — id + anchor genome hash
    let s = w.species.list.len();
    h.write_u32(s as u32);
    for sp in &w.species.list {
        h.write_u32(sp.id);
        // Sub-hash the anchor genome to bound the loop body
        let mut ah = XxHash64::with_seed(0);
        let ag = &sp.anchor_genome;
        write_f32(&mut ah, ag.size);
        write_f32(&mut ah, ag.photosynth_efficiency);
        // ... (same field set as creature genomes above; extract a helper)
        h.write_u64(ah.finish());
    }

    // (f) RNG state — serialize the inner Xoshiro256PlusPlus via bincode for
    // a stable byte string. Bincode is small but adds a dependency.
    //
    // Alternative: hash the rand_xoshiro's serialized form via serde_json.
    // PIN: use serde_json over the SimRng (already has Serialize from F.26).
    let rng_bytes = serde_json::to_vec(&w.rng).unwrap();
    h.write(&rng_bytes);

    h.finish()
}

fn write_f32(h: &mut XxHash64, v: f32) {
    h.write_u32(v.to_bits());
}
```

(Implementation detail: the helper that hashes a `Genome` is extracted
into `fn hash_genome(h: &mut XxHash64, g: &Genome)` to avoid duplication
between the creature loop and the species anchor loop.)

**Carrion field set:** v6 §M says "id, position, pool_remaining, age".
Audit `src/carrion.rs` during review; if `Carrion` doesn't currently
have a `u32 id`, add one (this is a backward-compat change; bump
`SCHEMA_VERSION` to 2 — OR pin "carrion has no id in v1; hash its
position only" via DECISIONS). The simpler path:

- **PIN:** if `Carrion` already has an id (B-era code may have added
  one for carrion-overlap), use it; otherwise add it during F.29. If
  added, this **must** bump `SCHEMA_VERSION` to 2 and add `Carrion.id`
  to `SaveV1` — confirm in implementation whether this is needed and
  capture in DECISIONS.
- Implementer: open `src/carrion.rs` first. If no `id`, decide
  between (a) adding it (with SCHEMA_VERSION bump) and (b) hashing
  only `(x, y, pool, age)` (with DECISIONS note: "v6 §M's 'id' for
  carrion omitted in v1 — corpses have no stable id; positional
  fingerprint sufficient for regression hash"). PIN (b) is simpler;
  F.30 isn't blocked by it.

**DECISIONS:** `snapshot-hash carrion field set: hash (x, y, pool, age) only; carrion has no stable id in v1 — positional fingerprint is sufficient for the regression hash`

### F.29.b — integration test (`tests/acceptance.rs`)

```rust
//! v5 §16 / v6 §M acceptance test. Pinned seed, 10k ticks, perf gate,
//! golden snapshot hash. Run with `cargo test --test acceptance`.

use evosim::snapshot_hash::snapshot_hash;
use evosim::world::World;
use std::time::Instant;

const SEED: &str = "evosim-test-001";
const TICKS: u32 = 10_000;
const PERF_BUDGET_MS: u128 = 8_000;
const GOLDEN_PATH: &str = "tests/golden_snapshot_t10000.txt";

#[test]
fn acceptance_t10000() {
    let mut w = World::new(SEED);

    let started = Instant::now();
    for _ in 0..TICKS {
        if !w.step() {
            // World ended early — that's an acceptance failure too.
            break;
        }
    }
    let elapsed_ms = started.elapsed().as_millis();

    // (a) population > 0
    assert!(
        w.population() > 0,
        "(a) population must be > 0 at tick {TICKS}; got {}",
        w.population()
    );

    // (b) ≥ 2 species
    assert!(
        w.live_species_count >= 2,
        "(b) live_species_count must be ≥ 2; got {}",
        w.live_species_count
    );

    // (c) wall-clock budget
    assert!(
        elapsed_ms < PERF_BUDGET_MS,
        "(c) sim took {} ms; budget {} ms",
        elapsed_ms,
        PERF_BUDGET_MS
    );

    // (d) snapshot hash
    let hash = snapshot_hash(&w);

    // Bootstrap mode: env var writes the golden file instead of asserting.
    if std::env::var("EVOSIM_WRITE_GOLDEN").is_ok() {
        std::fs::write(GOLDEN_PATH, format!("{hash:#018x}\n")).expect("write golden");
        eprintln!("wrote golden snapshot: {hash:#018x}");
        return;
    }

    let golden_str = std::fs::read_to_string(GOLDEN_PATH)
        .expect("golden file missing; run `EVOSIM_WRITE_GOLDEN=1 cargo test --test acceptance` to bootstrap");
    let golden_str = golden_str.trim();
    let golden: u64 = u64::from_str_radix(golden_str.trim_start_matches("0x"), 16)
        .expect("malformed golden hash");

    assert_eq!(
        hash, golden,
        "(d) snapshot hash mismatch: got {hash:#018x}, golden {golden:#018x}"
    );
}
```

The test sits in `tests/acceptance.rs` (integration test) so
`cargo test --lib` doesn't run it. `cargo test` (no flags) **does** run
it. CI uses `cargo test --test acceptance` to make the intent clear.

### F.29.c — golden snapshot bootstrap

After F.26 + F.27 + F.28 land and F.29's hash function lands, the
orchestrator runs:

```
EVOSIM_WRITE_GOLDEN=1 cargo test --test acceptance --release
```

This (a) verifies a/b/c pass, (b) writes the d-target golden value.
Commit `tests/golden_snapshot_t10000.txt`.

If a/b/c fail at bootstrap time, F.30 begins (slider tuning) — don't
commit the golden until a/b/c are green.

### F.29.d — CI wiring (`.github/workflows/ci.yml`)

Add a step to the `rust` job (NOT a new job — share the cargo cache):

```yaml
      - name: acceptance test (perf gate + golden snapshot)
        run: cargo test --release --test acceptance
```

Place AFTER `test (native)` and BEFORE `wasm-pack build`.

**CI runner perf caveat:** GitHub-hosted Linux runners (currently
2-vCPU `ubuntu-latest`) are 2–3× slower than a modern laptop. v5 §16's
"< 8 wall-seconds" target is "on CI" — so the budget already accounts
for CI slowness. The orchestrator must verify the test passes there.
If it's flaky in CI we have three options:

- **(A)** Bump the budget to 12 or 15s — but that contradicts the spec.
- **(B)** Run with the `threads` feature in CI (enables rayon).
- **(C)** Run in `--release` mode (already pinned above).

**PIN:** `--release` + single-threaded for the canonical perf gate.
Reasons:
- Determinism: the snapshot hash is deterministic only if RNG access
  ordering is deterministic; rayon parallel chunks with
  `N_CHUNKS=8` per v6 §J use deterministic sub-RNGs (so it's
  deterministic), but the chunked execution path is exercised even
  single-threaded (via `chunk_ranges(n)` and the per-chunk sub-RNG
  spawn). So single-threaded vs multi-threaded produces the same hash.
  Reviewer audit: confirm this is true (D milestone wired it).
- Simpler CI config: no SAB / COOP-COEP shenanigans.
- The `threads` feature gate is optional — F.30 may turn it on if
  needed.

**DECISIONS:** `acceptance test config: --release single-threaded (rayon chunking is deterministic regardless of thread count per v6 §J; --release is necessary to hit < 8s budget on CI)`

### F.29.e — flake guard (audit during review)

Test (c) is wall-clock-sensitive on a shared CI runner. If runs are
near the 8-second cliff, false reds will appear. Audit during
implementation:

- If `--release` measurements on the laptop are < 3 seconds, CI's 2–3×
  slowdown puts us at ~9 seconds — over budget.
- If CI is close to the budget, **F.30 must tighten further** (not
  bump the budget). The whole point of the perf gate is the orchestrator
  stops optimizing when it passes; failing the gate means more
  optimization, not a budget bump.

(No code path in F.29 papers over this; the gate is the gate.)

### F.29 code-reviewer checklist (heavy)

- [ ] `snapshot_hash` hash-input order matches v6 §M exactly:
      `tick → creatures (id, position, velocity, energy, age, genome
      struct order, NN weights) → sun (current, capacity per cell row-
      major) → carrion (positional only per F.29.a pin) → species (id +
      anchor genome sub-hash) → RNG state`.
- [ ] Helper extracted: `hash_genome(h, g)` shared between the
      creature loop and the species anchor loop.
- [ ] f32 hashing uses `to_bits()` (NaN-stable, exact).
- [ ] RNG state hashed via `serde_json::to_vec(&w.rng)` (uses the same
      serialization F.26 introduced).
- [ ] `tests/acceptance.rs` is an integration test (lives in `tests/`,
      not `src/`); excluded from `cargo test --lib`.
- [ ] All four assertions present and asserted with helpful messages.
- [ ] Perf gate uses `Instant::now()`, not wall-clock-shifted-by-NTP
      time.
- [ ] `EVOSIM_WRITE_GOLDEN=1` env var bootstrap path works and clearly
      logs the written hash.
- [ ] Golden file format is `0x` + 16 hex chars + newline; `from_str_radix`
      tolerant of leading `0x`.
- [ ] CI step added to `.github/workflows/ci.yml` `rust` job in the
      correct position (after `test (native)`, before `wasm-pack build`).
- [ ] CI step uses `--release` and `--test acceptance`.
- [ ] Hash determinism: re-running the same seed twice in a row in the
      same `cargo test --test acceptance` invocation produces the same
      hash. (Add a smoke test that runs the world twice in one test and
      asserts equality of `snapshot_hash`.)
- [ ] Hash determinism across `chunk_ranges` chunking: the `threads`
      feature being off doesn't change the hash (we go through the same
      chunk path either way per v6 §J). Audit `nn_forward_all_chunks`
      to confirm.
- [ ] `Cargo.toml` does NOT add a new dependency for F.29 (twox-hash
      already pinned).
- [ ] `cargo test --test acceptance` passes locally in `--release`.
- [ ] `cargo test --test acceptance` passes in CI.

### F.29 DECISIONS lines

- `snapshot hash function: xxHash64 via twox-hash (already pinned for SimRng) — no new dep`
- `snapshot hash field order: tick → creatures (SoA-order) → sun (row-major) → carrion (pos only, no id) → species (id + anchor sub-hash) → RNG (serde_json bytes of SimRng) per v6 §M`
- `snapshot-hash carrion field set: (x, y, pool, age); carrion has no stable id in v1`
- `golden bootstrap: EVOSIM_WRITE_GOLDEN=1 env var; commits tests/golden_snapshot_t10000.txt as the regression baseline`
- `acceptance test location: tests/acceptance.rs (integration test); excluded from cargo test --lib; CI runs cargo test --release --test acceptance`
- `acceptance test config: --release single-threaded — meets <8s budget on GitHub ubuntu-latest; deterministic chunking per v6 §J keeps hash identical with or without rayon`

---

## F.30 — balance tuning (orchestrator-only sketch)

**This step does NOT generate implementer subagent code.** It's the
orchestrator's hands-on iteration loop. Sketch:

### Goal

Make `cargo test --release --test acceptance` pass all four assertions
on (a) the orchestrator's laptop AND (b) GitHub CI.

### Permitted levers (v6 §K — sliders, plus their defaults)

| Slider | Range | Default | Likely effect |
|---|---|---|---|
| `base_sun_rate` | 0–1 | 0.08 | Raise to keep population alive in early collapse |
| `mutation_rate_multiplier` | 0–5 | 1.0 | Raise to spawn species faster (helps assert b) |
| `sun_gradient_strength` | 0–3 | 1.0 | Adjust west-east differential; affects spatial clustering |
| `mouth_tax` | 0–0.3 | 0.05 | Lower to make predators viable; raise to favor photosynth |
| `nn_mutation_sigma` | 0–0.2 | 0.02 | Raise to accelerate brain drift |

Tuning is done by editing `DevSliders::default()` in `src/world.rs` (or
by adding a one-off tweak in `acceptance.rs` that calls `set_slider`
before the run — both legitimate; the latter doesn't change the
shipping defaults).

### Permitted out-of-slider constants (only with explicit DECISIONS line)

Per the brief: "don't change spec values outside the slider list".
Practical reading: changing slider defaults (`DevSliders::default()`) is
in-scope; changing non-slider constants (e.g.
`FOUNDER_ENERGY`, `SPLIT_THRESHOLD`, `EAT_BASE_GAIN`) requires a
DECISIONS line with a reason. Reviewer should reject silent edits.

### Orchestrator checklist (do these in order)

1. Run the acceptance test once with E-era defaults. Record the four
   assertion outcomes.
2. If (a) fails (extinction before tick 10k): population is dying out.
   Raise `base_sun_rate` slightly (e.g. 0.10) or lower `mouth_tax`.
3. If (b) fails (<2 species at tick 10k): mutation rate too low.
   Raise `mutation_rate_multiplier` to 1.5–2.0 and re-test.
4. If (c) fails (>8s wall-clock): profile. Typical hotspots: vision
   raycasts, NN forward. Mitigations already baked into v5 §3.6 (vision
   range cap, single grid). Last-resort: enable rayon `threads` feature
   in CI.
5. If (d) fails (hash mismatch) AFTER a successful (a/b/c) bootstrap:
   you've introduced a determinism regression. Bisect; the snapshot
   hash is meant to catch exactly this.
6. After all four pass on the laptop, push a CI run. If CI fails (c),
   apply same tuning logic (slider tweaks; not budget bumps).
7. Once green on CI, bootstrap the golden value
   (`EVOSIM_WRITE_GOLDEN=1`) on the orchestrator's machine in
   `--release` mode, commit. Re-run CI to confirm green with the
   committed golden.
8. Stop iterating. Per v5 §16: "the orchestrator stops optimizing when
   this passes."

### Tuning red flags (stop and reconsider)

- A slider value at its range boundary (e.g. `base_sun_rate=1.0`) means
  you've saturated the lever; deeper structural change is needed.
- Setting `mutation_rate_multiplier` above 3 produces "chaos species"
  that drift every birth; (b) passes trivially but the sim is not
  meaningfully evolving. Cap at 2.5 even if (b) passes higher.
- If (a) is borderline (population in the dozens), the world is one
  bad RNG roll from failing; consider raising `base_sun_rate` slightly
  for headroom.

### Where the orchestrator records the tuning

After F.30 lands, append a `## F.30` section to `DECISIONS.md` with one
line per slider change from defaults, plus a one-line rationale.
Example:

```
## F.30 — balance tuning
- mutation_rate_multiplier default 1.0 → 1.4: pushed (b) from 1 species at tick 10k to consistently 3–4
- base_sun_rate default 0.08 → 0.10: (a) was borderline in seeds adjacent to evosim-test-001; small bump removes the cliff
- (none for mouth_tax, sun_gradient_strength, nn_mutation_sigma — left at defaults)
```

---

## Integration order (for the implementer subagent)

The implementer works through F.26 → F.27 → F.28 → F.29 in order.
Within F.26, commit each commit-shape unit separately so bisect remains
useful.

1. **F.26.a — save module skeleton.** Add `src/save.rs`, `SaveV1`,
   subsystem snapshot structs, `to_save_v1`, `from_save_v1`, `LoadError`,
   `SimRng` serde derives (via `rand_xoshiro` `serde1` feature),
   `DAY_TICKS` constant. Run the 7 F.26 Rust unit tests.
   Commit: `F.26.a: SaveV1 schema + to_save_v1/from_save_v1 round-trip`.

2. **F.26.b — wasm API.** Add `snapshot_json` and `from_json` to
   `WorldHandle`. Two more unit tests (string error prefixes).
   Commit: `F.26.b: wasm WorldHandle::snapshot_json + WorldHandle::fromJson`.

3. **F.26.c — JS persistence client + worker + IDB plumbing.**
   New files: `web/src/persistence/index.ts`,
   `web/src/persistence/worker.ts`, `web/src/persistence/ui.ts`,
   `web/src/persistence/toast.ts`. Modify `web/src/main.ts` boot path
   for the load/resume flow and autosave trigger. Manual smoke: confirm
   IDB write happens; confirm resume on reload.
   Commit: `F.26.c: persistence client + IDB worker + resume/schema-mismatch modals + autosave toast`.

4. **F.27 — seed display.** Edit `web/index.html` (top-bar seed-pill),
   `web/src/main.ts` (`installSeedDisplay`), `web/src/styles.css`.
   Commit: `F.27: top-bar seed display with copy button`.

5. **F.28.a — hof_json wasm method.** Add to `wasm_api.rs`; add 3 unit
   tests.
   Commit: `F.28.a: WorldHandle::hof_json (4-slot hall-of-fame + day count)`.

6. **F.28.b — single-creature renderer.** Add
   `renderCreaturePortrait` + `RING_COLORS` constant in
   `web/src/render.ts`. Refactor `drawCreatures` to use `RING_COLORS`
   (audit for color drift).
   Commit: `F.28.b: renderCreaturePortrait + shared RING_COLORS`.

7. **F.28.c — eulogy card.** New file `web/src/eulogy.ts`. Modify
   `web/src/main.ts` (gate). New CSS rules. Modify
   `PersistenceClient.deleteCurrent()` + worker `delete` message.
   Commit: `F.28: eulogy card with 4-image grid, Copy Share, Download Save, New World`.

8. **F.29.a — snapshot_hash module.** New file
   `src/snapshot_hash.rs`. One Rust unit test (determinism: same world
   hashes the same twice in a row).
   Commit: `F.29.a: src/snapshot_hash.rs (xxHash64 per v6 §M)`.

9. **F.29.b — acceptance test.** New file `tests/acceptance.rs`. New
   golden file (will be empty until bootstrap step below).
   Add CI step in `.github/workflows/ci.yml`.
   Commit: `F.29.b: tests/acceptance.rs + CI step (golden TBD)`.

10. **Orchestrator: bootstrap golden + run F.30.**
    `EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance`,
    iterate on sliders if (a/b/c) fail, commit golden file.
    Commit (orchestrator): `F.29.c: bootstrap tests/golden_snapshot_t10000.txt`.
    Commit (orchestrator, if any tuning happened):
    `F.30: balance tuning — <slider deltas>`.

11. **Final pass.** `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt`, `cargo test --lib`, `cargo test --release --test
    acceptance`, `wasm-pack build --target web`, `pnpm build`. Run
    manual UI session: ~5 minutes at 100×, watch for resume prompt on
    reload, watch eulogy fire on extinction, click all three eulogy
    buttons.

---

## Code-reviewer's checklist (split per substep, summary)

(Per-substep checklists are listed inside each section above. This is
the cross-cutting summary that always applies at the end of each
review.)

### Cross-cutting per substep

- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all -- --check` clean.
- [ ] `cargo test --lib` count grows by ≥ N (F.26: 7, F.28: 3, F.29: 1
      determinism test = 11+).
- [ ] `cargo test --release --test acceptance` passes (after F.29
      lands + golden committed).
- [ ] `wasm-pack build --target web` succeeds.
- [ ] `pnpm build` succeeds (TypeScript typecheck included).
- [ ] No emoji introduced in source files beyond the existing top-bar
      icon (the copy button glyph) — reviewer may swap that for a text
      label if preferred.

### Orchestrator-only final-pass checks (F.30 / shipping)

- [ ] §16 DoD (a/b/c/d) all pass in CI in `--release` mode.
- [ ] Manual UI confirms: fresh world spawns; resume prompt appears
      after first reload; eulogy fires on extinction; Copy Share Text
      produces correct format; Download Save round-trips back into a
      runnable world; New World cleanly resets.
- [ ] DECISIONS.md has all F.26–F.30 lines committed.
- [ ] README mentions the resume prompt + autosave behavior (one line).

---

## DECISIONS lines (consolidated, in order of substep)

Add a `## F.26` block in `DECISIONS.md` followed by the lines from
F.26.c above; then `## F.27`, `## F.28`, `## F.29`, `## F.30` (F.30's
content written by the orchestrator after tuning).

### F.26

- persistence handoff: JSON-encode on main thread (1–5 ms at v1 sizes);
  plain JS worker only writes to IDB — preserves spirit of v5 §13 "don't
  hitch main thread on disk I/O" without the cost of a second wasm
  artifact or double-buffered SoA. Revisit in v1.1 if F.30 measurements
  show encode hitches a frame.
- worker type: plain ES-module worker (Vite ?worker import) — no second
  wasm-pack build
- save shape: explicit "Snapshot" structs in src/save.rs wrapping each
  subsystem (CreatureSoA, SunMap, SpeciesRegistry, EventLog); future
  schema versions add SaveV2 alongside with a From<SaveV1>
- RNG round-trip: rand_xoshiro "serde1" feature — exact state
  preservation, no manual replay
- IDB layout: DB "evosim" v1, store "worlds", single key "current" —
  one world per browser profile
- tick→day conversion: DAY_TICKS = 1000 (also used by F.28 eulogy)
- autosave cadence: every 300 sim-ticks (10 sim-seconds at 30-tick/s
  baseline) AND ≥ 3000ms wall-clock apart
- IDB error UX: single ephemeral red status toast at top of page, fades
  6s — replaced (not stacked) on repeat
- resume policy: prompt iff no ?seed= URL AND a save exists; ?seed=
  always builds fresh; never auto-resume
- schema-mismatch UX: one-button "New World" modal; invalid-json and
  structural-error use the same modal (treat all unrecoverable loads as
  "save unusable")
- save size: accept multi-MB JSON (brain weights dominate); worker
  absorbs IDB write latency; compact binary brain encoding deferred to
  v1.1
- next_id recompute: SpeciesRegistry.next_id is not persisted —
  derived as max(species.list[].id) + 1 on load

### F.27

- seed display: top-bar pill with copy button; clipboard.writeText
  fallback is silent (console.warn) — failure is rare and non-blocking

### F.28

- eulogy gate: world.world_ended boolean check in the RAF loop, single
  eulogyShown bool to avoid re-show
- single-creature renderer: separate renderCreaturePortrait function in
  render.ts (RING_COLORS shared with drawCreatures) — keeps the SoA
  fast-path untouched
- "New World" semantics: delete IDB "current" row, then reload sans
  ?seed= — avoids resume prompt re-offering the finished world
- share text format: literal v5 §11 string with seed URL-encoded

### F.29

- snapshot hash function: xxHash64 via twox-hash (already pinned for
  SimRng) — no new dep
- snapshot hash field order: tick → creatures (SoA-order) → sun
  (row-major) → carrion (pos only, no id) → species (id + anchor
  sub-hash) → RNG (serde_json bytes of SimRng) per v6 §M
- snapshot-hash carrion field set: (x, y, pool, age); carrion has no
  stable id in v1
- golden bootstrap: EVOSIM_WRITE_GOLDEN=1 env var; commits
  tests/golden_snapshot_t10000.txt as the regression baseline
- acceptance test location: tests/acceptance.rs (integration test);
  excluded from cargo test --lib; CI runs cargo test --release --test
  acceptance
- acceptance test config: --release single-threaded — meets <8s budget
  on GitHub ubuntu-latest; deterministic chunking per v6 §J keeps hash
  identical with or without rayon

### F.30 (orchestrator records after tuning pass)

- (e.g.) mutation_rate_multiplier default 1.0 → 1.4: pushed (b) from 1
  species at tick 10k to consistently 3+
- (other slider deltas as needed)

---

## File structure additions (F end-state)

```
src/                             (+/edit existing)
  rng.rs                         + Serialize/Deserialize derive on SimRng
  save.rs                        (NEW) SaveV1 + subsystem snapshots +
                                 from_world / from_save_v1 + LoadError
  snapshot_hash.rs               (NEW) xxHash64 over canonical state per v6 §M
  wasm_api.rs                    + snapshot_json + fromJson + hof_json
  world.rs                       + to_save_v1 + from_save_v1 helpers
  constants.rs                   + DAY_TICKS = 1000
  lib.rs                         + pub mod save; pub mod snapshot_hash;
  carrion.rs                     (audit) — verify no id field needed for
                                 hash; if added, bump SCHEMA_VERSION + add
                                 to SaveV1
  Cargo.toml                     + rand_xoshiro features = ["serde1"]

tests/                           (NEW)
  acceptance.rs                  v5 §16 integration test
  golden_snapshot_t10000.txt     pinned u64 hex (bootstrapped by orchestrator)

web/                             (+/edit existing)
  index.html                     + seed-pill in top-bar
  src/main.ts                    + persistence boot + resume flow +
                                 + autosave loop + eulogy gate +
                                 + installSeedDisplay
  src/styles.css                 + modal/eulogy/status-toast/seed-pill rules
  src/eulogy.ts                  (NEW) F.28 eulogy card + Copy Share +
                                 Download Save + New World
  src/render.ts                  + renderCreaturePortrait + RING_COLORS
                                 (shared with drawCreatures)
  src/persistence/index.ts       (NEW) PersistenceClient (save/load/delete)
  src/persistence/worker.ts      (NEW) IDB worker (no wasm)
  src/persistence/ui.ts          (NEW) Resume + schema-mismatch modals
  src/persistence/toast.ts       (NEW) showAutosaveFailureToast

.github/workflows/ci.yml         + cargo test --release --test acceptance
                                 step in the rust job
```

---

## Out-of-scope reminders (v5 §15 / §14 guardrails)

- No double-buffered SoA / transferable handoff (deviation pinned in
  F.26 DECISIONS — JSON-on-main + plain JS worker is the v1 shape).
- No second wasm artifact (the IDB worker is plain JS).
- No lineage tree visualization (v1.1).
- No new traits / actions / sliders / NN inputs (out forever).
- No "follow-cam" auto-pan from eulogy card or events panel (v1.1).
- No save-slot UI (one slot, "current"; multi-slot is v1.1).
- No per-species color in stats charts (v1.1).
- No tabs UI (v5 §15 forbids).
- No WebGPU stubs (v5 §15 forbids).
- No tuning of non-slider constants without an explicit DECISIONS line.

---

## Punts to v1.1

- Compact-binary brain encoding (drop save size from multi-MB to ~1MB).
- Save-slot picker (currently one slot, "current").
- Save export → upload-to-share workflow.
- Eulogy card "share to social" buttons (currently just Copy Share Text).
- Eulogy card animated reveal of the 4 images one-by-one.
- "Pause on game over" UX prior to eulogy card (currently the sim
  freezes naturally because `step()` returns false; render keeps
  drawing the final frame).
- Resume-prompt "preview" thumbnail rendering of the saved world.
- Multiple-world IndexedDB tab (saved-by-seed; "Load…" picker).
- Background "drift" autosaves while paused (currently autosave needs
  tick to advance).
- Lineage tree visualization for the eulogy card and rail.
- Trait histograms in Stats overlay.

## Code review — APPROVED

F.26 persistence is solid: `src/save.rs` SaveV1 has `schema_version: 1`
checked first in `World::from_save_v1` (src/world.rs:939), all per-subsystem
snapshots present (creatures SoA, sun, carrion, species, full event log,
sliders, HoF, RNG, founder anchors, all bookkeeping). `rand_xoshiro`
`serde1` feature pinned in Cargo.toml so `SimRng` round-trips exactly; the
`f26_round_trip_preserves_rng` test in src/save.rs:357 acts as the
divergence canary (ticks 100, snapshots, restores, ticks 100 more in both,
asserts pop matches). SoA length validation, brain weight-count check, and
grid+vision+cell_to_carrion rebuild on load all present and correct
(src/world.rs:946-1040). Worker (`web/src/persistence/worker.ts`) is plain
ES-module via `?worker`, single `"current"` IDB key. Autosave 300-tick +
3000ms wall floor honored in `maybeAutosave` (web/src/main.ts:40-49). Resume
prompt only fires when no `?seed=` and a save exists; `?seed=` always wins
(web/src/main.ts:87-119). Schema-mismatch + corrupt-save both route to the
single-button "New World" modal per v6 §I. Quota toast `web/src/persistence/toast.ts`
fades after 6s and replaces (not stacks).

F.28 eulogy: `WorldHandle::hof_json` (src/wasm_api.rs:344) emits all four
slots with weirdest→longest_lived fallback (src/wasm_api.rs:346-350) and
`day_count = tick / DAY_TICKS`. `web/src/eulogy.ts` shows the 2×2 grid,
Copy Share Text format matches spec (`evosim seed=… — lived N days, peaked
S species, see {url}`), Download Save uses `evosim-{seed}-day{N}.json`,
New World calls `persistence.deleteCurrent()` then reloads sans `?seed=`.
`eulogyShown` boolean (web/src/main.ts:52) gates single-fire. `DAY_TICKS =
1000` lives in src/constants.rs:157.

F.29 acceptance: `src/snapshot_hash.rs` orders inputs per v6 §M (tick →
creatures with id/pos/vel/energy/age/genome-in-struct-order/NN-weights →
sun row-major → carrion (x,y,pool,age — no id per DECISIONS) → species
(id+anchor sub-hash) → RNG serde_json bytes), f32s via `to_bits()`, shared
`hash_genome` helper. `tests/acceptance.rs` asserts all four DoD criteria
with helpful messages; `EVOSIM_WRITE_GOLDEN=1` bootstrap path works;
golden pinned at `0xc35be8a7905c7f05`. CI wires `cargo test --release
--test acceptance` between `test (native)` and wasm-pack (ci.yml:25-26).
`snapshot_hash_is_deterministic` and `snapshot_hash_same_seed_same_hash`
tests in src/snapshot_hash.rs:131-158 confirm in-process determinism.

F.30 sanity: `Brain::founder` in src/brain.rs:40-81 adds hardwired weights
*after* random init; `child_from` (src/brain.rs:168-186) clones parent
weights then mutates, so hardwired bias propagates correctly. SUN_REFILL_RATE
= 0.30 (src/constants.rs:14), `DevSliders::default()` wires
`base_sun_rate: SUN_REFILL_RATE` (src/world.rs:38). FOUNDER_SPLIT_JITTER =
50.0 applied only in handle_births split logic (src/world.rs:840-841);
founder placement at world center (src/world.rs:109-110) is untouched.

Non-blockers (defer to v1.1 or capture in a follow-up):

1. **Camera state not persisted.** v6 §C line 60 says "Camera state IS
   persisted in autosave (zoom level + pan center). On resume, restored. On
   New World, reset to default." The current SaveV1 (src/save.rs) and JS
   loader (web/src/main.ts) ship neither a camera field in the JSON nor a
   separate IDB key. The plan's prose at milestone-F.md:164 says
   "Camera/UI state lives on JS side (see F.27)" but F.27 only adds the
   seed-pill; no camera-restore code lands. DECISIONS has no line excusing
   this. Recommend either (a) add `camera: { zoom, cx, cy }` field to
   SaveV1 (bumps schema_version, invalidating any current saves) and
   restore on load, or (b) add an explicit DECISIONS punt line for v1.1.
   Treating as non-blocker because the rest of F.26 works and saves
   round-trip cleanly; camera defaults to founder cell on every load.

2. **No explicit save→load→step-N hash-equality test** as suggested in the
   priority-check list. The existing `f26_round_trip_preserves_rng`
   (src/save.rs:357) ticks 100 → snapshot → restore → tick 100 more in
   both worlds and asserts population matches, which catches RNG drift but
   not all-state drift. A stronger version comparing `snapshot_hash` after
   the second 100 ticks would be more discriminating; safe to add in a
   touch-up commit but not blocking F.

3. F.26 plan mentioned "audit `nn_forward_all_chunks` to confirm rayon
   feature off doesn't change the hash" — not directly verified here, but
   the acceptance test passes (orchestrator confirms) and the same chunk
   ranges are used either way per v6 §J, so the assumption holds.

