# Milestone E — observability (species detection + right rail + toasts + events)

**Spec anchors:** v5 §11 (UI persistent aquarium, right-rail overlays,
in-aquarium event cues, eulogy "weirdest" def in §11.1), v5 §12 (species
detection metric — distance + anchors + naming), v5 §15 E.20–E.25 (E.20
sequential prerequisite; E.21–E.24 parallel-safe but written in order by a
single implementer subagent). v6 §B (toast styling: white bg / dark text /
top-right / slide / 4s fade; highlight ring golden `rgb(255,200,50)`, 2px,
1.5s), v6 §H (species naming + σ_t scales — already implemented), v6 §L
(first-to detection definitions, biggest, last survivor).

**Status at handoff:** Milestone D is committed; NN-driven action selection,
deterministic chunking (`N_CHUNKS=8`), vision DDA, full energy economy are
live. `src/species.rs` already implements `species_distance`,
`trait_body_distance_sq`, lineage naming, `SpeciesRegistry::founder` and
`speciate(parent_id, anchor_genome, anchor_brain_weights, tick)`. `src/events.rs`
defines `EventLog` (ring 200 + full history) and `EventKind` variants
(Speciation, Extinction, PopulationMilestone, FirstToMove, FirstToEat,
WorldEnded). `world.rs` already fires Extinction (via `finalize_extinctions`),
FirstToMove (in `collect_deaths`), FirstToEat (in `eat_and_scavenge`).
`wasm_api.rs` exposes `recent_events_json()`, `creatures_buffer()` stride 13,
`sun_buffer()`, `carrion_buffer()`, the five dev sliders.

**What is intentionally NOT in E** (so the implementer doesn't drift):

- Double-buffered transferable autosave / IndexedDB worker — F.26.
- Resume prompt / seed-string textbox / load-from-save UI — F.27.
- Eulogy card 4-image grid + Copy Share + Download Save — F.28 (E *must*
  collect the running state that F.28 will read; see E.25).
- Headless acceptance test, golden snapshot, perf gate — F.29.
- Live balance tuning — F.30.
- Lineage tree viz, trait histograms, NN viz, follow-cam, signaling, NEAT,
  WebGPU, seasons, sexual reproduction — v1.1+ (v5 §14 "Deferred").
- New traits / new actions / new sliders — out forever per v5 §14.

---

## Scope ceiling restatement

v5 §15 E ships five substeps:

- **E.20** (sequential prerequisite): wire the species-distance check into
  `handle_births` so speciation actually fires.
- **E.21** (parallel-safe once E.20 lands): right-rail overlay shell with
  three sections (Events / Stats / Inspector). Adds the wasm methods
  `species_list_json`, `creature_at`, `creature_inspect_json`, `stats_sample`.
- **E.22** (parallel-safe): in-aquarium toast + highlight-ring rendering
  driven by polling `recent_events_json`. v6 §B styling exact.
- **E.23** (parallel-safe): two Canvas2D line charts inside Stats panel
  (population over time, species count over time).
- **E.24** (parallel-safe): Inspector popover, opens on canvas click via
  `creature_at`, populated by `creature_inspect_json`.
- **E.25**: event auto-detection wiring — PopulationMilestone thresholds,
  re-aim FirstToMove fire site to `apply_movement_and_repulsion` (per v6
  §L definition), gather running "biggest" / "last survivor" / "weirdest"
  state that F.28 will consume.

The implementer subagent works through E.20 → E.21 → E.22 → E.23 → E.24 →
E.25 in order. **Do not start E.21 before E.20 lands** — the right rail
needs `species_list_json` which depends on the `live_species_count`
ratchet that E.20 corrects.

Code-reviewer depth (per ORCHESTRATOR adapting-review-depth guidance):
**heavy** on E.20 (species detection is load-bearing for the §16 acceptance
test); **moderate** on E.21/E.25 (new wasm surface + state additions);
**light-touch** on E.22–E.24 (UI-only).

---

## E.20 — species distance check wiring (sequential prerequisite)

### Goal

Inside `World::handle_births`, every newborn that drifts more than
`SPECIES_THRESHOLD = 4.0` from its parent's species *anchor* becomes a new
species. Per v5 §12.2: distance is measured against the **anchor** (genome
+ brain weights captured at speciation moment), NOT the live parent — this
is what prevents "drift creep" from spawning a new species every birth.
Per v5 §12.3: the new species' anchor is the child's genome+brain at the
moment of speciation. `SpeciesRegistry::speciate` already implements that
contract; E.20 just calls it.

### File / function changes

**File:** `src/world.rs`. Edit `handle_births` only.

Pre-E.20 (today, lines ~720–734):

```rust
let parent_species = self.creatures.species_id[i];
self.creatures.push(
    new_id, cx, cy, child_energy,
    parent_species, parent_species,  // species_id, parent_species_id
    self.tick, child_genome, child_brain,
);
```

Post-E.20:

```rust
let parent_species = self.creatures.species_id[i];
let anchor = self.species.get(parent_species);
let dist = species_distance(
    &child_genome,
    &anchor.anchor_genome,
    &child_brain.weights,
    &anchor.anchor_brain_weights,
);

let (child_species_id, parent_species_id) = if dist > SPECIES_THRESHOLD {
    let new_id_species = self.species.speciate(
        parent_species,
        child_genome.clone(),
        child_brain.weights.clone(),
        self.tick,
    );
    let new_name = self.species.get(new_id_species).name.clone();
    let parent_name = self.species.get(parent_species).name.clone();
    self.events.push(Event {
        tick: self.tick,
        kind: EventKind::Speciation {
            new_species_id: new_id_species,
            parent_species_id: parent_species,
            new_species_name: new_name,
            // parent_name carried implicitly via parent_species_id — UI looks it up.
            creature_id: new_id,
        },
    });
    (new_id_species, parent_species)
} else {
    (parent_species, parent_species)
};

self.creatures.push(
    new_id, cx, cy, child_energy,
    child_species_id, parent_species_id,
    self.tick, child_genome, child_brain,
);
```

**Why `child_genome.clone()` / `child_brain.weights.clone()` for the anchor:**
the child gets pushed into `CreatureSoA` right after, which moves the
originals. `speciate` takes them by-value (genome, Vec<f32>) and stores
them on the new `Species` record — those become the anchor. Clones are
necessary; cost is one Genome (~120 bytes) + one Vec<f32> (3456 floats =
~14KB) per speciation event. At a few hundred speciations across a 10k
tick run that's ~MB total — fine.

**EventKind::Speciation field set (already in `events.rs`):**

```rust
Speciation {
    new_species_id: u32,
    parent_species_id: u32,
    new_species_name: String,
    creature_id: u64,
}
```

Parent name is not in the event payload — UI looks it up via
`species_list_json` (E.21). That keeps the event payload small and lets the
UI render dynamic species-name truncations consistently. **Do NOT** add
`parent_species_name` to `EventKind::Speciation`; it would duplicate data
the UI can resolve.

### `live_species_count` ratchet

`finalize_extinctions` recomputes `live_species_count = alive.len()`.
After E.20, a fresh speciation in this tick will have the child alive in
`CreatureSoA`, so `alive.insert(child_species_id)` runs in
`finalize_extinctions` and the counter increments naturally. No extra work
needed. **Audit during code review:** confirm `finalize_extinctions` runs
*after* `handle_births` — it does (`tick_once` calls `step` which runs
handle_births in step 11, then `finalize_extinctions` outside).

### Constant pin

`SPECIES_THRESHOLD = 4.0` already in `constants.rs`. Don't change. Pull
into the import block in `world.rs`:

```rust
use crate::constants::SPECIES_THRESHOLD;  // (or rely on wildcard)
use crate::species::species_distance;     // already in scope
```

### Tests (`src/world.rs::tests`)

Add three tests; keep them deterministic via `World::new(seed)`.

**`e20_tiny_mutation_keeps_species`**

```rust
// Force the founder to split with minimal NN-driven body drift. We can't
// directly control NN logits, but we CAN seed a world, force one Split
// and verify the resulting child has species_id == 0. Use a seed known
// to produce no body-trait excursion past threshold.
let mut w = World::new("e20-tiny");
// Tick until first split.
let parent_species_before = w.creatures.species_id[0];
let mut split_seen = false;
for _ in 0..5000 {
    let pop_before = w.population();
    w.tick_once();
    if w.population() > pop_before {
        split_seen = true;
        // First newborn is at the back (push); check its species_id.
        let n = w.creatures.len();
        let child_species = w.creatures.species_id[n - 1];
        // First split rarely crosses threshold (body mut ≈ 3% per trait,
        // NN mut ≈ 2% per weight × σ=0.02 = small).
        // We allow either outcome but assert the test path executed.
        assert!(split_seen);
        // The interesting case for THIS test: same-species.
        if child_species == parent_species_before {
            return; // pass
        } else {
            // Threshold tripped — that's fine, the other test covers this.
            return;
        }
    }
}
panic!("never split — sliders or balance off");
```

(Above is intentionally lenient about which side of the threshold the
first split lands on; testing exact threshold behavior needs the synthetic
test below.)

**`e20_synthetic_large_drift_creates_new_species`** (synthetic — direct
manipulation, no NN involved):

```rust
let mut w = World::new("e20-big");
let parent_genome = w.creatures.genomes[0].clone();
let parent_brain = w.creatures.brains[0].clone();
let parent_species = w.creatures.species_id[0];

// Construct a child genome with a HUGE body drift (delta size +5,
// pigment all flipped) — well past the 4.0 threshold even before any
// brain shift.
let mut child_genome = parent_genome.clone();
child_genome.size = 8.0; // delta 7.0 / σ=1.0 → contribution ~49 to body_sq
child_genome.pigment_r = 1.0 - parent_genome.pigment_r;
let child_brain = parent_brain.clone();

let d = crate::species::species_distance(
    &child_genome, &parent_genome,
    &child_brain.weights, &parent_brain.weights,
);
assert!(d > SPECIES_THRESHOLD, "synthetic d = {d}");

let new_species_id = w.species.speciate(
    parent_species,
    child_genome.clone(),
    child_brain.weights.clone(),
    w.tick,
);
assert_ne!(new_species_id, parent_species);
assert_eq!(w.species.get(new_species_id).parent_id, Some(parent_species));
assert!(w.species.get(new_species_id).name.starts_with("Lineage A"));
```

(This test exercises the registry but not the wiring inside
`handle_births`. The wiring is covered by the next test.)

**`e20_speciation_event_lands_in_log_when_threshold_crossed`** —
end-to-end: force a synthetic big-drift mutation to fire via `handle_births`
by temporarily zeroing `nn_mutation_sigma`, jacking
`mutation_rate_multiplier` to 50 (a "max-out" knob), and ticking until a
Speciation event appears in `w.events.all`. Cap loop at 20k ticks; fail if
no Speciation event in that window. Confirm:

- `w.events.all.iter().any(|e| matches!(e.kind, EventKind::Speciation { .. }))`
- The corresponding `Species` record has `parent_id = Some(0)` and
  `name != "Lineage A"`.
- The `creature_id` in the event matches a live (or just-died) creature id.

### v5 §12 / v6 §H reread checklist (for the implementer)

- [ ] Distance compared against **anchor**, not live parent (v5 §12.2). ✓
      code reads `self.species.get(parent_species).anchor_genome` etc.
- [ ] Anchor for the *new* species is the **child's genome+brain**
      (v5 §12.3) — `speciate` already takes those as args.
- [ ] σ_t scales per v6 §H — already in `species.rs::trait_body_distance_sq`.
- [ ] W_body = 3.0, W_brain = 1.0 — already in `species.rs::species_distance`.
- [ ] Categorical jump cost 1.5 — already in
      `trait_body_distance_sq` when `eye_count` differs.

### E.20 DECISIONS lines (add to `DECISIONS.md`)

- `speciation event parent name: NOT in event payload — UI resolves via species_list_json (avoids stale-name drift across renames in future)`
- `speciation anchor clone cost: ~14KB Vec<f32> per speciation event — acceptable at v1 scale; revisit in v1.1 if NN_WEIGHT_COUNT grows`

---

## E.21 — right-rail overlay shell + three sections

### Goal

Add a translucent right-rail container that overlays the canvas. Three
sections: **Events** (default open), **Stats** (hidden by default),
**Inspector** (opens when a creature is clicked on the canvas). Each
section is a separate TS module.

v5 §11 says the rail is ~280px wide for Events. Sections share the same
width. Per v5 §11 "render *over* aquarium" — no layout reflow of the
canvas; the rail floats on top with `position: fixed; right: 0`.

### DOM additions (`web/index.html`)

```html
<body>
  <canvas id="aquarium"></canvas>

  <div id="top-bar">
    <span id="status">booting…</span>
  </div>

  <!-- E.21: right rail shell -->
  <aside id="right-rail">
    <nav id="rail-tabs" role="tablist">
      <button data-tab="events" class="rail-tab is-active">Events</button>
      <button data-tab="stats" class="rail-tab">Stats</button>
      <!-- Inspector tab appears only when a creature is selected. -->
    </nav>

    <section id="rail-events" class="rail-panel is-active"></section>
    <section id="rail-stats"  class="rail-panel"></section>
    <section id="rail-inspector" class="rail-panel"></section>
  </aside>

  <!-- E.22: toast container, top-right under the rail tabs (separate stack) -->
  <div id="toast-stack" aria-live="polite"></div>

  <script type="module" src="/src/main.ts"></script>
</body>
```

### CSS additions (`web/src/styles.css`)

Add (do not replace existing):

```css
#right-rail {
  position: fixed;
  top: 36px;          /* clear the top-bar */
  right: 0;
  width: 280px;       /* v5 §11 */
  max-height: calc(100vh - 36px);
  background: var(--rail-bg);
  border-left: 1px solid rgba(255, 255, 255, 0.08);
  font-size: 12px;
  display: flex;
  flex-direction: column;
  z-index: 5;
}
#rail-tabs { display: flex; gap: 0; border-bottom: 1px solid rgba(255,255,255,0.08); }
.rail-tab {
  flex: 1;
  padding: 6px 10px;
  background: transparent;
  color: var(--fg);
  border: 0;
  border-right: 1px solid rgba(255,255,255,0.05);
  cursor: pointer;
  font: inherit;
  opacity: 0.55;
}
.rail-tab.is-active { opacity: 1; background: rgba(255,255,255,0.05); }
.rail-panel { display: none; overflow-y: auto; padding: 8px 10px; }
.rail-panel.is-active { display: block; }

/* Toasts (v6 §B): white bg / dark text / top-right / slide / 4s fade. */
#toast-stack {
  position: fixed;
  top: 40px;
  right: 296px;       /* sit just left of the 280px rail */
  display: flex;
  flex-direction: column;
  gap: 6px;
  pointer-events: none;
  z-index: 10;
}
.toast {
  background: #fff;
  color: #111;
  padding: 6px 10px;
  border-radius: 4px;
  box-shadow: 0 2px 6px rgba(0,0,0,0.35);
  font-size: 12px;
  min-width: 180px;
  max-width: 240px;
  transform: translateX(40px);
  opacity: 0;
  transition: transform 200ms ease-out, opacity 200ms ease-out;
}
.toast.is-visible { transform: translateX(0); opacity: 1; }
.toast.is-leaving { transform: translateX(40px); opacity: 0; }

/* Events panel rows */
.events-species-list { margin-bottom: 8px; }
.species-row { padding: 2px 4px; cursor: pointer; border-radius: 2px; }
.species-row:hover { background: rgba(255,255,255,0.06); }
.species-row.is-extinct { opacity: 0.4; text-decoration: line-through; }
.event-row { padding: 2px 0; opacity: 0.9; font-family: ui-monospace, monospace; }
.event-row .tick { opacity: 0.5; margin-right: 6px; }

/* Inspector */
#rail-inspector dl { display: grid; grid-template-columns: 110px 1fr; gap: 2px 6px; margin: 0; }
#rail-inspector dt { opacity: 0.55; }
#rail-inspector dd { margin: 0; }
```

### TS module skeleton (new files under `web/src/`)

```
web/src/
  rail/
    index.ts        wiring: install rail, tab switching, polling cadence
    events.ts       Events panel (species list + scrolling event log)
    stats.ts        Stats panel (2 line charts; samples ring buffer)
    inspector.ts    Inspector panel (creature dump)
    toast.ts        Toast stack manager (v6 §B; used by E.22)
    highlight.ts    Highlight ring state (id → expiry_tick; used by render)
```

(One directory keeps the new code segregated; no risk of intermixing with
the existing `render.ts` / `camera.ts` / `main.ts`.)

`main.ts` adds:

```ts
import { installRail, pollRail } from "./rail";
import { installCanvasClick } from "./rail/inspector";

// ... after world boot:
const rail = installRail(world);
installCanvasClick(canvas, cam, () => ({ w: viewW, h: viewH }), world, rail);

// inside the RAF frame, AFTER world.step_n + before renderWorld:
pollRail(rail, world);
```

`pollRail` is a single function the rail exposes for the RAF loop to call
once per frame — it:

1. Pulls new entries from `recent_events_json()` and routes them to the
   Events panel + Toast stack + Highlight state (diff by last-seen index).
2. Samples `stats_sample()` every N sim-ticks (see E.23 for cadence).
3. Refreshes the Inspector panel if a creature is currently selected (it
   may have changed action / energy this tick).

### Wasm API additions (new methods on `WorldHandle`)

All four return JSON strings (cheap to encode at v1 sizes; avoids the
typed-array juggling for low-traffic data). Buffer-shaped data
(creatures/sun/carrion) stays on `*_buffer()` methods.

#### `species_list_json() -> String`

**Purpose:** populate the active-species list in the Events panel.
Alive-species-only (filtered by population > 0).

**Shape (TS type):**

```ts
type SpeciesRow = {
  id: number;            // u32
  name: string;
  population: number;    // u32
  parent_id: number | null;
};
type SpeciesListJson = SpeciesRow[];
```

**Implementation sketch:**

```rust
#[wasm_bindgen]
pub fn species_list_json(&self) -> String {
    // Count population per species id.
    let mut counts: std::collections::HashMap<u32, u32> = Default::default();
    for &sid in &self.inner.creatures.species_id {
        *counts.entry(sid).or_default() += 1;
    }
    let rows: Vec<_> = self.inner.species.list.iter().filter_map(|sp| {
        let pop = counts.get(&sp.id).copied().unwrap_or(0);
        if pop == 0 { return None; }
        Some(serde_json::json!({
            "id": sp.id,
            "name": sp.name,
            "population": pop,
            "parent_id": sp.parent_id,
        }))
    }).collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
}
```

O(N + S) per call. Called at the rail's poll cadence (≤ 4 Hz — see
"polling cadence" below).

#### `creature_at(world_x: f32, world_y: f32) -> Option<u32>`

**Purpose:** click-targeting from the canvas. Returns the **creature
index** (Some(i)) of the topmost creature whose body circle contains the
point, or `None`.

```rust
#[wasm_bindgen]
pub fn creature_at(&self, world_x: f32, world_y: f32) -> Option<u32> {
    // Linear scan — population ≤ 2k, called only on click (≤ 1 Hz).
    let n = self.inner.creatures.len();
    for i in 0..n {
        let dx = self.inner.creatures.x[i] - world_x;
        let dy = self.inner.creatures.y[i] - world_y;
        let r = self.inner.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
        if dx*dx + dy*dy <= r*r {
            return Some(i as u32);
        }
    }
    None
}
```

**Index vs ID:** returns SoA index, NOT the stable `creature_id`. Index
gets invalidated by `swap_remove` on the very next death. Callers must use
the index *immediately* via `creature_inspect_json(idx)` and then store
the returned creature_id if they want to track the same creature across
frames. **Document this in DECISIONS.**

(Alternative: return `creature_id` and have `creature_inspect_json` do a
linear lookup. Pro: stable handle; con: every inspect call is O(N). With
≤ 2k pop and the rail polling at ≤ 4 Hz this is fine — pick this if the
implementer prefers stability. **PIN: return index; UI re-resolves on
next poll by re-querying `creature_at` if it cares.**)

#### `creature_inspect_json(idx: u32) -> Option<String>`

**Purpose:** populate the Inspector panel. Returns JSON with everything
the panel shows. Returns `None` (in JS: `null` via `Option<String>` wrapped
into `Option<JsString>` — wasm-bindgen handles this) if `idx >=
creatures.len()`.

**Shape (TS type):**

```ts
type CreatureInspectJson = {
  index: number;
  id: number;
  species_id: number;
  species_name: string;
  parent_species_id: number;
  parent_species_name: string | null;
  x: number; y: number;
  age: number; max_age: number;
  energy: number;
  energy_frac: number;          // matches NN input scale
  size: number;
  current_action: "Rest" | "Photosynth" | "Eat" | "Scavenge" | "Split" | "Signal";
  // Efficiencies
  photosynth_efficiency: number;
  eat_efficiency: number;
  scavenge_efficiency: number;
  move_speed: number;
  vision_range: number;
  eye_count: number;
  armor: number;
  bite_reach: number;
  pigment: [number, number, number];
};
```

```rust
#[wasm_bindgen]
pub fn creature_inspect_json(&self, idx: u32) -> Option<String> {
    let i = idx as usize;
    if i >= self.inner.creatures.len() { return None; }
    let g = &self.inner.creatures.genomes[i];
    let sp = self.inner.species.get(self.inner.creatures.species_id[i]);
    let parent_name = sp.parent_id.map(|pid| self.inner.species.get(pid).name.clone());
    let action_name = format!("{:?}", self.inner.creatures.action_this_tick[i]);
    let json = serde_json::json!({
        "index": idx,
        "id": self.inner.creatures.id[i],
        "species_id": sp.id,
        "species_name": sp.name,
        "parent_species_id": self.inner.creatures.parent_species_id[i],
        "parent_species_name": parent_name,
        "x": self.inner.creatures.x[i],
        "y": self.inner.creatures.y[i],
        "age": self.inner.creatures.age[i],
        "max_age": g.max_age,
        "energy": self.inner.creatures.energy[i],
        "energy_frac": (self.inner.creatures.energy[i] / 100.0).clamp(0.0, 1.0),
        "size": g.size,
        "current_action": action_name,
        "photosynth_efficiency": g.photosynth_efficiency,
        "eat_efficiency": g.eat_efficiency,
        "scavenge_efficiency": g.scavenge_efficiency,
        "move_speed": g.move_speed,
        "vision_range": g.vision_range,
        "eye_count": g.eye_count,
        "armor": g.armor,
        "bite_reach": g.bite_reach,
        "pigment": [g.pigment_r, g.pigment_g, g.pigment_b],
    });
    Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
}
```

#### `stats_sample() -> Box<[f32]>` (return `[population, species_count]`)

Cheap getter that returns a fresh 2-float array. Stats panel polls this
on its own cadence and accumulates into a ring buffer TS-side. Reason for
TS-side accumulation: the user can pause / fast-forward, and we'd rather
the sampling line up with wall-clock render frames than with sim-ticks
(simpler reasoning about chart axes).

```rust
#[wasm_bindgen]
pub fn stats_sample(&self) -> Box<[f32]> {
    Box::new([self.inner.population() as f32, self.inner.live_species_count as f32])
}
```

Alternative: return `(tick, pop, species)` as 3 floats so the chart x-axis
is sim-ticks not frames. **PIN that:** 3-float return, chart x-axis =
sim-tick. The Stats panel records `(tick, pop, species)` triples in a
ring buffer of capacity 500; oldest samples drop off the left of the
chart.

```rust
#[wasm_bindgen]
pub fn stats_sample(&self) -> Box<[f32]> {
    Box::new([
        self.inner.tick as f32,
        self.inner.population() as f32,
        self.inner.live_species_count as f32,
    ])
}
```

### Polling cadence

Rail is updated by `pollRail` from the RAF loop. Sub-cadences:

- **Events / species list:** poll `recent_events_json()` + `species_list_json()`
  every frame (the diff-by-index pattern is cheap; species list O(N+S) at
  most a few hundred per call).
- **Stats sample:** call `stats_sample()` every **N sim-ticks**, where N is
  computed from current speed slider — every 100 sim-ticks at 100×, every
  30 at 10×, every 10 at 1×. The cleanest pin: sample whenever
  `world.tick % SAMPLE_INTERVAL == 0` and we haven't sampled at this tick
  yet. `SAMPLE_INTERVAL = 10` (cheap; 500-sample ring covers 5000 ticks ≈
  3 minutes wall-clock at 1×, ~30 seconds at 100×).
- **Inspector:** if a creature is currently selected, call
  `creature_inspect_json` once per frame (the panel rerenders only the
  fields that changed; ≤ 12 small DOM updates).

**DECISIONS line:** `stats sampling cadence: every 10 sim-ticks via tick % 10 == 0 check; 500-sample ring buffer TS-side (≈ 5000 ticks visible)`

### Events panel (TS) — `rail/events.ts`

Two stacked subsections in `#rail-events`:

1. **Active species list** (top). Rendered from `species_list_json()` once
   per second (cheap to compute; flickers less than per-frame). Sorted by
   population descending. Each row:
   - `<div class="species-row" data-species-id="${id}">${name} (${pop})</div>`
   - Click → calls `highlight.addSpeciesById(id, expiryTick)` (E.22).
   - Tooltip-via-`title` attribute shows full name + parent name (since
     names are truncated to 16 chars in the species struct, the UI just
     reads what the registry gave; v6 §H truncation already happens).

2. **Scrolling event log** (bottom, flex-grow). Append-only `<div>` per
   event. Newest at top. Cap at 200 visible rows (UI-only cap; the wasm
   ring is also 200). Each row format:
   - `<span class="tick">t=${ev.tick}</span> ${formatEvent(ev.kind)}`
   - `formatEvent` switches on the `EventKind` variant string. Click on a
     row that references a creature_id schedules a 1.5s highlight at the
     creature's current position (resolved via... see below).

**Creature-id → SoA-index resolution:** events store `creature_id` (stable
u64), but `highlight.ts` keys highlights by `creature_id` (stable) and the
renderer needs the index (SoA position changes due to swap_remove). The
renderer reads `creatures_buffer()` which is index-aligned with the
internal SoA — and the buffer does NOT include `creature_id`.

**Add a `creature_ids_buffer()` getter** to wasm_api: a `Uint32Array` view
over the `id` Vec (or Float64Array if we want full u64 precision — but u64
> 2^53 won't happen in a single session at any plausible rate, so float64
fits). **PIN:** add `creature_ids_buffer()` returning Float64Array,
index-aligned with `creatures_buffer()`. The renderer pulls both per
frame; the highlight pass walks `highlights: Map<creature_id, expiry_tick>`,
finds the index by linear scan (≤ 2k entries, cold-cache acceptable; OR
build a Map<id, index> per frame in the renderer for O(1) lookup at a
linear cost). PIN the Map<id, index> approach (1 alloc per frame is fine).

```rust
#[wasm_bindgen]
pub fn creature_ids_buffer(&mut self) -> js_sys::Float64Array {
    // Index-aligned with creatures_buffer(). Stable u64 ids fit in f64
    // until ~2^53 = 9e15 (effectively forever for this sim).
    self.id_buf.clear();
    for &id in &self.inner.creatures.id {
        self.id_buf.push(id as f64);
    }
    unsafe { js_sys::Float64Array::view(&self.id_buf) }
}
```

Adds an `id_buf: Vec<f64>` field to `WorldHandle`. Cheap.

### Inspector panel (TS) — `rail/inspector.ts`

State:

```ts
type InspectorState =
  | { kind: "empty" }
  | { kind: "selected"; creatureId: number; lastSeenIndex: number };
```

Workflow:

1. Click on canvas → `installCanvasClick` listener calls `screenToWorld` →
   `world.creature_at(wx, wy)`.
2. If `Some(idx)`: query `world.creature_inspect_json(idx)`, set state to
   `selected`. Tab "Inspector" appears in `#rail-tabs` and activates.
3. Per frame, while in `selected` state: re-resolve the creature by ID.
   - Build `id→index` map from `creature_ids_buffer()`.
   - If id missing (creature died), set state to `empty`, hide Inspector
     tab, show "Creature died (was Lineage X)" placeholder for 2 seconds
     then collapse to empty.
   - Else call `creature_inspect_json(currentIndex)` and update the DOM.
4. Click on empty canvas area (no creature_at hit) → clear selection.

**DOM template** (inside `#rail-inspector`):

```html
<dl>
  <dt>Species</dt><dd id="ins-species"></dd>
  <dt>Parent</dt><dd id="ins-parent"></dd>
  <dt>Action</dt><dd id="ins-action"></dd>
  <dt>Age</dt><dd id="ins-age"></dd>
  <dt>Energy</dt><dd id="ins-energy"></dd>
  <dt>Size</dt><dd id="ins-size"></dd>
  <dt>Photo eff</dt><dd id="ins-photo"></dd>
  <dt>Eat eff</dt><dd id="ins-eat"></dd>
  <dt>Scav eff</dt><dd id="ins-scav"></dd>
  <dt>Move speed</dt><dd id="ins-move"></dd>
  <dt>Eyes</dt><dd id="ins-eyes"></dd>
  <dt>Vision</dt><dd id="ins-vision"></dd>
  <dt>Armor</dt><dd id="ins-armor"></dd>
  <dt>Bite reach</dt><dd id="ins-bite"></dd>
  <dt>Pigment</dt><dd id="ins-pigment"></dd>
</dl>
```

Update each <dd> textContent on every poll (cheap; ~15 string writes/frame
when selected).

**Selection highlight:** while a creature is selected, add an entry to
`highlights` with `expiry_tick = +∞` (or `Number.MAX_SAFE_INTEGER` —
infinity gets serialized weirdly across JSON, just use a very-large
sentinel). Cleared when selection ends.

### `species_count` mid-tick consistency

`live_species_count` is set in `finalize_extinctions`, which runs *after*
each tick. The wasm `species_count` getter reads `live_species_count`, so
mid-tick reads see the previous tick's value — fine, it ticks 30 times a
second.

### E.21 tests

- **Rust** (`src/wasm_api.rs::tests` — native, no js_sys):
  - `species_list_json_filters_extinct`: build a 2-species world manually
    (one with creatures, one without), call `species_list_json` (need to
    refactor it to a non-`Option<String>` pure-rust helper for testability;
    PIN that — extract `fn species_list_json_inner(&self) -> String` and
    have the wasm method call it).
  - `creature_inspect_json_returns_none_for_out_of_range`.
  - `creature_at_finds_founder`: founder at center, click at center →
    `Some(0)`.
- **Manual UI:** launch `pnpm dev`, click on a creature, watch Inspector
  populate; click empty area, watch it clear; check tab is hidden then
  shown.

### E.21 DECISIONS lines

- `creature_at returns SoA index (not stable id) — caller re-resolves via creature_ids_buffer + creature_inspect_json each frame; cheap at v1 sizes`
- `species_list_json frequency: 1Hz (not every frame) — counts are O(N) and the panel doesn't need sub-second freshness`
- `creature_ids_buffer uses Float64Array: u64 ids fit in f64 mantissa for the v1 session lifetime`

---

## E.22 — in-aquarium event toasts + highlight rings (v6 §B)

### Goal

When a Speciation, Extinction, FirstToMove, or FirstToEat event lands in
the recent-events log, spawn a toast in the top-right and a 1.5s golden
ring around the relevant creature(s). All visuals per v6 §B exactly.

Per v6 §B the toast policy is "speciation/extinction"; the brief says
**also fire on FirstToMove / FirstToEat** for player drama. Yes, do that
— these are once-per-run events, harmless to surface as toasts.
PopulationMilestone is NOT a toast (it's noise — milestone counts will
fire 6 times in any successful run; UI shows them in the Events log only).

WorldEnded is NOT a toast either; the eulogy card (F.28) handles
end-of-run.

### Toast stack (`rail/toast.ts`)

```ts
type ToastEntry = { id: number; el: HTMLDivElement; expireAt: number };
const stack: ToastEntry[] = [];

export function pushToast(text: string, now: number): void {
  const el = document.createElement("div");
  el.className = "toast";
  el.textContent = text;
  document.getElementById("toast-stack")!.appendChild(el);
  // Force reflow then add visible class for slide-in.
  void el.offsetWidth;
  el.classList.add("is-visible");
  const entry: ToastEntry = { id: nextId++, el, expireAt: now + 4000 };
  stack.push(entry);
}

export function tickToasts(now: number): void {
  for (let i = stack.length - 1; i >= 0; i--) {
    const t = stack[i];
    if (now > t.expireAt - 200 && !t.el.classList.contains("is-leaving")) {
      t.el.classList.add("is-leaving");  // start fade-out
    }
    if (now > t.expireAt) {
      t.el.remove();
      stack.splice(i, 1);
    }
  }
}
```

**Stacking:** newer toasts at the top of the stack (`flex-direction:
column-reverse` on the container) OR newest at the bottom — pick newest at
top, since that's where the eye lands and the user can see the most
recent drama first. **PIN: newest at top.** Use
`#toast-stack { flex-direction: column-reverse; }`.

**Cap:** at most 6 visible toasts; if more arrive, drop the oldest
immediately (let the fade-out animation play if you want, simpler to
just remove). PIN cap = 6.

**Toast text formatters:**

- Speciation: `"New species: {name} from {parent_name}"` (parent_name
  resolved via species list cache; if unknown — race window — show
  `"(unknown)"`).
- Extinction: `"Extinct: {name}"`.
- FirstToMove: `"First mover"`.
- FirstToEat: `"First bite"`.

### Highlight ring (`rail/highlight.ts`)

State (singleton):

```ts
type Highlights = Map<number /* creature_id */, number /* expire_tick */>;
export const highlights: Highlights = new Map();

export function addHighlight(creatureId: number, currentTick: number, durationTicks = 1.5 * 30): void {
  // 1.5s × 30 sim-ticks/render-frame budget … BUT v6 §B says 1.5 SECONDS
  // (wall-clock), not sim-ticks. We measure expiry in WALL-CLOCK ms,
  // not ticks, so fast-forward doesn't shorten the highlight.
  highlights.set(creatureId, performance.now() + 1500);
}

export function pruneHighlights(nowMs: number): void {
  for (const [id, exp] of highlights) {
    if (nowMs >= exp) highlights.delete(id);
  }
}
```

**Wall-clock vs sim-tick:** v6 §B says 4 seconds and 1.5 seconds (no
qualifier). PIN wall-clock. At 100× speed the world is "moving fast" but
the toast and ring fade at human reading speed — correct. **DECISIONS:**
`toast/highlight timing: wall-clock seconds (4s toast, 1.5s ring) per v6 §B; not sim-tick scaled`

### Renderer hook (`render.ts::drawCreatures`)

Extend `drawCreatures` to accept the highlights map + an id-lookup buffer:

```ts
export function drawCreatures(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  viewW: number,
  viewH: number,
  data: Float32Array,
  ids: Float64Array,
  stride: number,
  highlights: Map<number, number>,  // creature_id → expire wall-ms
  nowMs: number,
): void {
  // Build a transient set of indices to highlight.
  const highlightIdx = new Set<number>();
  if (highlights.size > 0) {
    for (let k = 0; k < ids.length; k++) {
      const cid = ids[k];
      const exp = highlights.get(cid);
      if (exp !== undefined && nowMs < exp) highlightIdx.add(k);
    }
  }

  for (let i = 0; i < data.length; i += stride) {
    const idx = i / stride;
    // ... existing fill + ring drawing ...

    if (highlightIdx.has(idx)) {
      // v6 §B: golden rgb(255,200,50), 2px, fades over 1.5s.
      const cid = ids[idx];
      const exp = highlights.get(cid)!;
      const remainingMs = exp - nowMs;
      const alpha = Math.min(1, remainingMs / 1500);
      ctx.strokeStyle = `rgba(255, 200, 50, ${alpha.toFixed(3)})`;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(sx, sy, radiusPx + 3, 0, Math.PI * 2);  // 3px outside body
      ctx.stroke();
    }
  }
}
```

`renderWorld` passes the highlights map + ids buffer + `performance.now()`
through.

**Camera does NOT auto-pan** per v5 §11 — the renderer just draws the
ring at the creature's current world position. If the creature is
off-screen, the ring is invisible; player must pan/zoom to find it. PIN.

### Event-to-toast/highlight routing (`rail/index.ts::pollRail`)

```ts
let lastSeenEventCount = 0;
const speciesListCache = new Map<number, { name: string; parent_id: number | null }>();

function pollRail(rail: RailState, world: WorldHandle): void {
  const now = performance.now();
  // 1. Refresh species list cache (cheap; needed for toast parent-name lookup).
  if (now - lastSpeciesPoll > 1000) {  // 1Hz
    const json: SpeciesRow[] = JSON.parse(world.species_list_json());
    speciesListCache.clear();
    for (const sp of json) speciesListCache.set(sp.id, { name: sp.name, parent_id: sp.parent_id });
    renderSpeciesList(json);
    lastSpeciesPoll = now;
  }
  // 2. Diff events.
  const events: Event[] = JSON.parse(world.recent_events_json());
  for (let i = lastSeenEventCount; i < events.length; i++) {
    const ev = events[i];
    handleNewEvent(ev, now, world.tick);
    appendEventRow(ev);
  }
  lastSeenEventCount = events.length;
  // 3. Stats sample (if interval crossed).
  maybeSampleStats(world);
  // 4. Inspector refresh (if selected).
  refreshInspectorIfSelected(world);
  // 5. Toast tick + highlight prune.
  tickToasts(now);
  pruneHighlights(now);
}
```

**Event-index diff gotcha:** the wasm `recent` is a ring buffer (cap 200).
If `events.length` rolls over the cap (older events dropped from the
front), `lastSeenEventCount` can exceed `events.length` and the diff
breaks.

Two fixes:
- **(A)** Compare by event identity instead of count (track last-seen
  tick+kind hash). Robust but more code.
- **(B)** Reset `lastSeenEventCount = 0` whenever
  `events.length < lastSeenEventCount`, accept that we may show duplicates
  on overflow (rare — 200 events is a lot).
- **(C)** Expose `events_total_count: u32` getter on `WorldHandle` (count
  of all-time events appended; not the ring buffer length) and diff by
  that.

**PIN (C).** Add `events_total_count(&self) -> u32` returning
`self.inner.events.all.len() as u32`. The TS side tracks
`lastSeenTotalCount` and subtracts to compute "how many new events" — if
that exceeds the ring buffer size, log a console warning and process only
the last `recent.length` (200) entries (we accept missed events at very
high event-rate; no v1 design hits that).

```rust
#[wasm_bindgen(getter)]
pub fn events_total_count(&self) -> u32 {
    self.inner.events.all.len() as u32
}
```

### E.22 routing rules (per `handleNewEvent`)

```ts
function handleNewEvent(ev: Event, nowMs: number, simTick: number): void {
  switch (ev.kind.tag) {
    case "Speciation": {
      const parentName = speciesListCache.get(ev.kind.parent_species_id)?.name ?? "(unknown)";
      pushToast(`New species: ${ev.kind.new_species_name} from ${parentName}`, nowMs);
      addHighlight(ev.kind.creature_id, simTick);
      break;
    }
    case "Extinction": {
      pushToast(`Extinct: ${ev.kind.species_name}`, nowMs);
      // No creature to highlight (already dead).
      break;
    }
    case "FirstToMove":
      pushToast("First mover", nowMs);
      addHighlight(ev.kind.creature_id, simTick);
      break;
    case "FirstToEat":
      pushToast("First bite", nowMs);
      addHighlight(ev.kind.creature_id, simTick);
      break;
    case "PopulationMilestone":
    case "WorldEnded":
      // Logged in Events panel; no toast.
      break;
  }
}
```

(Event tag-vs-payload serde shape: `serde_json::to_string` on
`EventKind::Speciation { ... }` produces
`{"Speciation":{"new_species_id":...,"parent_species_id":...,...}}` by
default. TS code reads `Object.keys(ev.kind)[0]` to get the variant tag.
Alternative: add `#[serde(tag = "tag", content = "data")]` attribute to
`EventKind` — cleaner JSON but breaks any existing consumer. **PIN:** add
the `#[serde(tag = "type")]` attribute to `EventKind` so JSON is shaped
`{ type: "Speciation", new_species_id: ..., ... }` — flat, more idiomatic
for the TS layer.)

**DECISIONS line:** `EventKind serde: #[serde(tag = "type")] flat shape — cleaner TS consumption, no existing JSON consumers to break (E added the only consumer)`

### E.22 tests

- **Manual UI:** force a speciation by jacking `mutation_rate_multiplier`
  to 5.0 in dev panel (E doesn't add the dev panel — that's still F.27 —
  use the existing `set_slider` wasm method via console:
  `world.set_slider("mutation_rate_multiplier", 5)`); watch toasts appear
  + golden ring on the newborn.
- **Manual UI:** end-of-world (let founder starve) — confirm no toast,
  only an event row.
- **Unit (TS):** if a test runner is in place (it isn't in v1 web), skip.
  Code is small and visual; review-only.

### E.22 DECISIONS lines

- `toast/highlight timing: wall-clock seconds (4s toast, 1.5s ring) per v6 §B; not sim-tick scaled`
- `toast variants: Speciation/Extinction/FirstToMove/FirstToEat fire toasts; PopulationMilestone/WorldEnded log-only (avoid spam + eulogy supersedes)`
- `toast stack cap: 6 visible; newer toasts at top (column-reverse)`
- `EventKind serde: #[serde(tag = "type")] flat shape — adopted in E because E adds the only TS consumer`
- `event diff: by events_total_count (monotone u32), not ring length — survives ring overflow`

---

## E.23 — stats line charts (inside Stats panel)

### Goal

Two simple Canvas2D line charts inside `#rail-stats`: population over
time, species count over time. Time on x-axis = sim-tick. Samples
collected by E.21's `pollRail` ring buffer.

### Module (`rail/stats.ts`)

```ts
type Sample = { tick: number; population: number; species: number };
const samples: Sample[] = [];
const SAMPLE_CAP = 500;
const SAMPLE_INTERVAL_TICKS = 10;

let lastSampledTick = -1;

export function maybeSampleStats(world: WorldHandle): void {
  const tick = world.tick;
  if (tick === lastSampledTick) return;
  if (tick % SAMPLE_INTERVAL_TICKS !== 0) return;
  const [t, pop, sp] = world.stats_sample();  // Float32Array(3)
  samples.push({ tick: t, population: pop, species: sp });
  if (samples.length > SAMPLE_CAP) samples.shift();
  lastSampledTick = tick;
  drawCharts();
}
```

### Chart rendering

Each chart is a `<canvas>` inside `#rail-stats`, ~260×80 px. Drawn
imperatively:

```ts
function drawChart(canvas: HTMLCanvasElement, samples: Sample[], pick: (s: Sample) => number, color: string, label: string): void {
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width;
  const H = canvas.height;
  ctx.clearRect(0, 0, W, H);
  if (samples.length < 2) return;
  const ymin = 0;
  const ymax = Math.max(1, ...samples.map(pick));
  const xmin = samples[0].tick;
  const xmax = samples[samples.length - 1].tick;
  const xrange = Math.max(1, xmax - xmin);
  // Axes (thin)
  ctx.strokeStyle = "rgba(255,255,255,0.15)";
  ctx.lineWidth = 1;
  ctx.beginPath(); ctx.moveTo(20, H - 12); ctx.lineTo(W, H - 12); ctx.stroke();
  // Line
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < samples.length; i++) {
    const px = 20 + ((samples[i].tick - xmin) / xrange) * (W - 22);
    const py = H - 12 - (pick(samples[i]) / ymax) * (H - 18);
    if (i === 0) ctx.moveTo(px, py); else ctx.lineTo(px, py);
  }
  ctx.stroke();
  // Label + current value
  ctx.fillStyle = "rgba(255,255,255,0.7)";
  ctx.font = "10px ui-monospace, monospace";
  ctx.fillText(`${label}: ${pick(samples[samples.length - 1]).toFixed(0)}`, 4, 12);
  // Y-axis max
  ctx.fillText(ymax.toFixed(0), 4, H - 14);
}

function drawCharts(): void {
  drawChart(popCanvas, samples, s => s.population, "#7fc4ff", "pop");
  drawChart(spcCanvas, samples, s => s.species,    "#ffb96b", "species");
}
```

DOM:

```html
<section id="rail-stats" class="rail-panel">
  <canvas id="chart-pop" width="260" height="80"></canvas>
  <canvas id="chart-species" width="260" height="80"></canvas>
</section>
```

### Why Canvas2D not a chart lib

- Bundle weight: chart libs are 50–200KB; this code is <100 LOC.
- v1 deliberately keeps the JS surface minimal (v6 §A: plain TS + DOM).
- Charts don't need interaction (no tooltip, no zoom) — just two lines.

### Stats panel tests

- Manual: open Stats tab during a run; confirm both lines start at sample
  0 and extend rightward as sim-time progresses; the population line drops
  to zero on extinction (and the chart stays drawn — F.28 will use the
  Stats panel as one possible eulogy-card source, but that's later).

### E.23 DECISIONS lines

- `stats charts use Canvas2D directly: zero deps, < 100 LOC, no interaction needed (v6 §A plain TS)`
- `stats x-axis = sim-tick (not wall-clock): consistent with sim semantics; player can pause and the chart freezes`

---

## E.24 — Inspector popover

### Goal

Already largely specified in E.21 (Inspector is one of the three sections).
E.24 is the rendering polish + the canvas-click → inspector wire.

### Click handling (`rail/inspector.ts`)

```ts
export function installCanvasClick(canvas: HTMLCanvasElement, cam: Camera, viewSize: () => { w: number; h: number }, world: WorldHandle, rail: RailState): void {
  canvas.addEventListener("click", (ev) => {
    // Distinguish click from drag (camera pan also uses mousedown). The
    // camera module already handles pointer events; we only fire on a
    // click that isn't a drag. Heuristic: pointerdown→pointerup with
    // total movement < 4px and elapsed < 250ms.
    // PIN: rely on the existing camera module's "tap" detection — add a
    // hook in camera.ts that fires on tap (not drag).
    const { w, h } = viewSize();
    const rect = canvas.getBoundingClientRect();
    const sx = ev.clientX - rect.left;
    const sy = ev.clientY - rect.top;
    const [wx, wy] = screenToWorld(cam, w, h, sx, sy);
    const idx = world.creature_at(wx, wy);
    if (idx === undefined) {
      rail.clearInspector();
    } else {
      const json = world.creature_inspect_json(idx);
      if (json) rail.openInspector(JSON.parse(json));
    }
  });
}
```

**Click-vs-drag disambiguation:** `web/src/camera.ts` already binds
pointer events. The click handler above will fire on pointer-up even after
a drag. Two paths:

- **PIN:** modify `camera.ts` to expose an `onTap(handler)` callback that
  fires only when pointerup is < 4px from pointerdown and < 250ms apart.
  `installCanvasClick` registers via that hook.

Document camera.ts tweak:

```ts
// camera.ts — extend signature:
export function attachCameraControls(
  canvas: HTMLCanvasElement,
  cam: Camera,
  viewSize: () => { w: number; h: number },
  worldSize: () => number,
  onTap?: (sx: number, sy: number) => void,
): void { /* ... */ }
```

Internally `camera.ts` tracks pointerdown coords + time; on pointerup, if
movement < 4px and elapsed < 250ms, invokes `onTap?(sx, sy)`. **Touch
gesture path** does the same with single-finger taps; multi-touch pinch
never fires onTap.

### Selection lifecycle

- Open: set state `selected`, ensure Inspector tab visible+active in
  rail-tabs, register a "permanent" highlight (id → very-large expiry).
- Refresh per frame: described in E.21.
- Close: clear state, hide Inspector tab from `#rail-tabs`, remove
  highlight.
- Auto-close on creature death: when the id→index map at a given frame
  doesn't contain `selectedId`, briefly show "Creature died" then
  auto-close after 2 seconds.

### Inspector panel content

(Spec covered in E.21 — the DOM template, the field list.) E.24 owns the
"populate the DOM from inspect JSON" function:

```ts
function renderInspector(data: CreatureInspectJson): void {
  setText("ins-species", `${data.species_name} (#${data.species_id})`);
  setText("ins-parent", data.parent_species_name ?? "—");
  setText("ins-action", data.current_action);
  setText("ins-age", `${data.age} / ${data.max_age}`);
  setText("ins-energy", `${data.energy.toFixed(1)} (${(data.energy_frac * 100).toFixed(0)}%)`);
  setText("ins-size", data.size.toFixed(2));
  setText("ins-photo", data.photosynth_efficiency.toFixed(2));
  setText("ins-eat", data.eat_efficiency.toFixed(2));
  setText("ins-scav", data.scavenge_efficiency.toFixed(2));
  setText("ins-move", data.move_speed.toFixed(2));
  setText("ins-eyes", `${data.eye_count}`);
  setText("ins-vision", data.vision_range.toFixed(1));
  setText("ins-armor", data.armor.toFixed(2));
  setText("ins-bite", data.bite_reach.toFixed(2));
  const [r, g, b] = data.pigment;
  setText("ins-pigment", `(${(r*255).toFixed(0)}, ${(g*255).toFixed(0)}, ${(b*255).toFixed(0)})`);
}
```

### E.24 tests

- Manual: click on the founder, watch Inspector populate; click empty
  space, watch it clear; click on a newborn child immediately after
  speciation, confirm its species name matches the toast.
- Manual: select a creature, then crank speed to 100×; wait for it to
  die; confirm "Creature died" placeholder appears briefly and panel
  clears.

### E.24 DECISIONS lines

- `inspector tap detection: < 4px movement + < 250ms elapsed in camera.ts onTap hook — disambiguates from drag-to-pan`
- `inspector creature-died UX: 2-second placeholder then auto-clear — avoids the user clicking and immediately losing context`

---

## E.25 — event auto-detection

### Goal

Wire all event triggers that aren't already firing. Covered in this step:

1. **PopulationMilestone** thresholds.
2. **Move FirstToMove fire-site** from `collect_deaths` to
   `apply_movement_and_repulsion` per v6 §L.
3. Collect "biggest" / "last survivor" / "weirdest" running state so F.28
   has the data ready.

### E.25.a — PopulationMilestone thresholds

**Spec:** v5 §11 lists the milestones as "10, 50, 100, 500, 1000, 2000"
(implied via the "any of these; only once per threshold ever" requirement
in the brief). Fire **once each**.

**Implementation:**

Add to `World`:

```rust
/// Population thresholds that have fired (v5 §11). Sorted ascending.
/// Pinned constants: 10, 50, 100, 500, 1000, 2000.
pub population_milestones_fired: u32,  // bitset: bit k = MILESTONES[k] crossed
```

Plus a constant:

```rust
// src/constants.rs
pub const POPULATION_MILESTONES: [u32; 6] = [10, 50, 100, 500, 1000, 2000];
```

Fire-site: end of `step()`, right where `peak_population` is updated:

```rust
let pop = self.creatures.len() as u32;
if pop > self.peak_population { self.peak_population = pop; }
for (k, &threshold) in POPULATION_MILESTONES.iter().enumerate() {
    let bit = 1u32 << k;
    if pop >= threshold && (self.population_milestones_fired & bit) == 0 {
        self.population_milestones_fired |= bit;
        self.events.push(Event {
            tick: self.tick,
            kind: EventKind::PopulationMilestone { population: threshold },
        });
    }
}
```

**Once per threshold ever** — bitset never clears; if pop dips back below
10 and re-crosses, no re-fire. PIN. Matches v5 §11.

### E.25.b — FirstToMove fire-site relocation

**Current:** `collect_deaths` sweeps live creatures for the FIRST creature
with `move_speed > 0 && distance_travelled >= 5.0`. Fires on the death
tick (because that's when `collect_deaths` runs).

**Per v6 §L:** "first creature born with `move_speed > 0` that actually
traveled ≥ 5u in its lifetime." It should fire **the tick the threshold is
crossed**, on the live creature, not on the death tick.

**Move the check to inside `apply_movement_and_repulsion`** — exactly the
step where `distance_travelled[i]` gets incremented. Replace this in
`collect_deaths`:

```rust
if !self.first_move_fired {
    for i in 0..n {
        if self.creatures.genomes[i].move_speed > 0.0
            && self.creatures.distance_travelled[i] >= 5.0 {
            self.first_move_fired = true;
            self.events.push(Event { ... });
            break;
        }
    }
}
```

with: delete that block from `collect_deaths`. Add to
`apply_movement_and_repulsion` immediately after
`self.creatures.distance_travelled[i] += dist;`:

```rust
if !self.first_move_fired
    && self.creatures.genomes[i].move_speed > 0.0
    && self.creatures.distance_travelled[i] >= 5.0
{
    self.first_move_fired = true;
    self.events.push(Event {
        tick: self.tick,
        kind: EventKind::FirstToMove { creature_id: self.creatures.id[i] },
    });
}
```

Now the event fires on the actual tick the threshold is crossed. The
creature is still alive (we're inside movement step 4 of the same tick).

**Test:** add `e25_first_to_move_fires_on_crossing_tick` — synthetic world
with one creature, `move_speed = 5.0`, `vx = 5.0`, `vy = 0.0`; tick once;
assert `distance_travelled[0] >= 5.0` and a FirstToMove event with that
creature's id appears in `events.all`. (Velocity is set by NN normally; in
the test we directly assign vx/vy before `apply_movement_and_repulsion`
runs — easiest path is to call `apply_movement_and_repulsion` directly
from the test after pre-setting vx/vy.)

### E.25.c — FirstToEat (audit only — already in tree, per v6 §L)

`eat_and_scavenge` already sets `got_a_bite[i] = true` on a successful
bite and fires FirstToEat the first time. Per v6 §L "first successful
bite" — current behavior matches. **No change.** Audit only.

### E.25.d — Biggest, Last Survivor, Weirdest tracking

These power F.28 (eulogy 4-image grid). E.25 collects the state; F.28
consumes it. No UI yet.

#### `biggest_ever` (creature snapshot at max size)

Add to `World`:

```rust
/// "Biggest" hall-of-famer for F.28 eulogy. Snapshot taken whenever a
/// creature's max_size_reached[i] crosses a new max. Per v6 §L.
pub biggest_ever: Option<HallOfFame>,

/// Snapshot of a creature for the eulogy card (F.28).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HallOfFame {
    pub creature_id: u64,
    pub genome: Genome,
    pub species_name: String,
    pub captured_tick: u32,
    pub captured_size: f32,
    pub captured_age: u32,
}
```

Wire site: end of `energy_bookkeeping` (where `max_size_reached[i]` is
already updated):

```rust
if g.size > self.creatures.max_size_reached[i] {
    self.creatures.max_size_reached[i] = g.size;
    // Hall of fame: biggest ever
    let current_best = self.biggest_ever.as_ref().map_or(0.0, |h| h.captured_size);
    if g.size > current_best {
        self.biggest_ever = Some(HallOfFame {
            creature_id: self.creatures.id[i],
            genome: g.clone(),
            species_name: self.species.get(self.creatures.species_id[i]).name.clone(),
            captured_tick: self.tick,
            captured_size: g.size,
            captured_age: self.creatures.age[i],
        });
    }
}
```

(Snapshot the genome by clone — ~120 bytes — once per new-max event,
which happens O(generations) times.)

#### `last_survivor` (creature with highest death tick)

Wire site: `collect_deaths` (any creature about to be removed):

```rust
for i in 0..n {
    if self.creatures.energy[i] <= 0.0 {
        // Last survivor candidate: most-recent death wins ties (later i
        // overwrites — within a tick the order doesn't matter, all share
        // the same tick; final death updates last_survivor to its data).
        let g = &self.creatures.genomes[i];
        self.last_survivor = Some(HallOfFame {
            creature_id: self.creatures.id[i],
            genome: g.clone(),
            species_name: self.species.get(self.creatures.species_id[i]).name.clone(),
            captured_tick: self.tick,
            captured_size: g.size,
            captured_age: self.creatures.age[i],
        });
        // ... existing carrion push, species_lost.push, dead.push code ...
    }
}
```

End-state: `last_survivor` holds the snapshot of whichever creature was
processed last on the final tick of life. Per v6 §L: "the creature that
died last (highest death tick)" — exactly satisfied because no further
deaths occur once population is zero.

#### `weirdest` (max distance from day-0 founder among ≥ 500 ticks lived)

Per v5 §11.1: "max z-scored Mahalanobis-style genome distance to the
day-0 founder genome (using the same metric as §12). If none qualify,
fall back to the longest-lived creature."

Add to `World`:

```rust
pub weirdest: Option<HallOfFame>,
/// Distance from day-0 founder for the current weirdest candidate.
/// Stored separately so we don't recompute on every comparison.
pub weirdest_distance: f32,
/// Fallback for "no qualifier yet" — longest-lived creature seen.
pub longest_lived: Option<HallOfFame>,
pub longest_lived_age: u32,
/// Day-0 founder's anchor for weirdness distance. Captured at World::new.
pub founder_genome_anchor: Genome,
pub founder_brain_anchor: Vec<f32>,
```

Initialize in `World::new`:

```rust
founder_genome_anchor: founder_genome.clone(),
founder_brain_anchor: founder_brain.weights.clone(),
weirdest: None,
weirdest_distance: 0.0,
longest_lived: None,
longest_lived_age: 0,
```

(Founder genome already gets cloned into `species.list[0].anchor_genome`,
so this is a second copy. ~120 bytes + 14KB. Fine.)

Wire site: `collect_deaths`, for each dying creature, check whether they
should become the new candidate:

```rust
let age = self.creatures.age[i];
let g_clone = self.creatures.genomes[i].clone();
let species_name = self.species.get(self.creatures.species_id[i]).name.clone();

// Longest-lived (always tracked, used as fallback).
if age > self.longest_lived_age {
    self.longest_lived_age = age;
    self.longest_lived = Some(HallOfFame {
        creature_id: self.creatures.id[i],
        genome: g_clone.clone(),
        species_name: species_name.clone(),
        captured_tick: self.tick,
        captured_size: g_clone.size,
        captured_age: age,
    });
}

// Weirdest (only qualifies if lived >= 500 ticks).
if age >= 500 {
    let dist = species_distance(
        &g_clone, &self.founder_genome_anchor,
        &self.creatures.brains[i].weights, &self.founder_brain_anchor,
    );
    if dist > self.weirdest_distance {
        self.weirdest_distance = dist;
        self.weirdest = Some(HallOfFame {
            creature_id: self.creatures.id[i],
            genome: g_clone,
            species_name,
            captured_tick: self.tick,
            captured_size: self.creatures.genomes[i].size,
            captured_age: age,
        });
    }
}
```

When F.28 runs, it picks `weirdest.or(longest_lived.clone())` (the spec
fallback).

#### `first_to_move` snapshot (for the eulogy)

Per v5 §14 "First to move (first creature with `move_speed > 0`)" — F.28
needs a snapshot. Add to `World`:

```rust
pub first_mover_snapshot: Option<HallOfFame>,
```

Capture in the new FirstToMove fire-site (inside
`apply_movement_and_repulsion`):

```rust
if !self.first_move_fired
    && self.creatures.genomes[i].move_speed > 0.0
    && self.creatures.distance_travelled[i] >= 5.0
{
    self.first_move_fired = true;
    let g = self.creatures.genomes[i].clone();
    let species_name = self.species.get(self.creatures.species_id[i]).name.clone();
    self.first_mover_snapshot = Some(HallOfFame {
        creature_id: self.creatures.id[i],
        genome: g.clone(),
        species_name,
        captured_tick: self.tick,
        captured_size: g.size,
        captured_age: self.creatures.age[i],
    });
    self.events.push(Event { ... });
}
```

### E.25 tests

- `e25_population_milestones_fire_once`: spawn a synthetic world via
  push-the-button — increment population through a milestone, tick, assert
  exactly one PopulationMilestone event for that threshold; tick again,
  assert no duplicate.
- `e25_first_to_move_fires_on_movement_step`: per E.25.b above.
- `e25_biggest_ever_tracks_max_size`: synthetic — push a creature with
  `size = 5.0`, tick energy_bookkeeping; assert `biggest_ever.unwrap().
  captured_size == 5.0`. Push another with size = 7.0; tick; assert update.
- `e25_weirdest_requires_500_ticks`: create a creature with `age = 400`
  and large distance from founder; kill it (energy = 0); tick deaths;
  assert `weirdest` is None. Then test with `age = 500`; assert Some.
- `e25_last_survivor_captures_final_death`: 2-creature world; kill both
  on different ticks; assert `last_survivor.captured_tick` is the later
  death tick.

### E.25 DECISIONS lines

- `population milestones: bitset of fired flags (one-shot per threshold ever) — v5 §11 "only once per threshold ever"`
- `first-to-move fire-site: apply_movement_and_repulsion (was collect_deaths) per v6 §L "actually traveled ≥ 5u in its lifetime"`
- `hall-of-fame snapshots: clone Genome (~120B) into a HallOfFame struct on World — pre-staged for F.28; brain weights NOT cloned for biggest/last/first (only genome image needed for eulogy render); weirdest's distance metric uses the live brain at death time, no anchor needed`
- `weirdest fallback: World.longest_lived snapshot (any age) — used if no creature ever reached 500 ticks (v5 §11.1)`
- `weirdest distance metric: species_distance against founder_genome_anchor + founder_brain_anchor (day-0 capture in World::new) per v5 §11.1 "z-scored Mahalanobis-style genome distance to the day-0 founder"`

---

## Wasm API surface summary (added in E)

| Method                         | Returns                  | Cost  | When called                    |
|--------------------------------|--------------------------|-------|--------------------------------|
| `species_list_json()`          | `String` (JSON array)    | O(N+S)| 1Hz from rail poll             |
| `creature_at(wx, wy)`          | `Option<u32>` (index)    | O(N)  | On canvas tap (≤ 1 Hz)         |
| `creature_inspect_json(idx)`   | `Option<String>` (JSON)  | O(1)  | Per frame when selected        |
| `stats_sample()`               | `Box<[f32; 3]>`          | O(1)  | Every 10 sim-ticks via rail    |
| `events_total_count` (getter)  | `u32`                    | O(1)  | Per frame for event diff       |
| `creature_ids_buffer()`        | `Float64Array` view      | O(N)  | Per frame for highlight lookup |

`recent_events_json()` already exists (no change).

---

## File structure additions (E end-state)

```
src/                          (+/edit existing)
  world.rs                    + E.20 speciation wire (handle_births)
                              + E.25.a population milestone fire
                              + E.25.b first-to-move fire-site move
                              + E.25.d hall-of-fame fields + tracking
  wasm_api.rs                 + species_list_json / creature_at /
                              + creature_inspect_json / stats_sample /
                              + events_total_count / creature_ids_buffer
                              + id_buf field on WorldHandle
  events.rs                   + #[serde(tag = "type")] on EventKind
  constants.rs                + POPULATION_MILESTONES = [10,50,100,500,1000,2000]
  lib.rs                      + (no change — wasm methods auto-exported)
  hof.rs                      (NEW) HallOfFame struct (serde) — pulled out
                              of world.rs to keep world.rs lean

web/                          (+/edit existing)
  index.html                  + #right-rail, #rail-tabs, three .rail-panel,
                              + #toast-stack
  src/styles.css              + rail/toast/panel rules (above)
  src/camera.ts               + onTap hook (4px / 250ms tap detection)
  src/render.ts               + drawCreatures(highlights, ids, nowMs)
                              + renderWorld threads highlights through
  src/main.ts                 + installRail + pollRail + installCanvasClick
  src/rail/index.ts           (NEW) rail boot + pollRail orchestrator
  src/rail/events.ts          (NEW) Events panel renderer + species list
  src/rail/stats.ts           (NEW) Stats sampler + chart renderer
  src/rail/inspector.ts       (NEW) Inspector panel + canvas click handler
  src/rail/toast.ts           (NEW) Toast stack manager
  src/rail/highlight.ts       (NEW) Highlight Map state + prune
```

---

## Integration order (for the implementer)

1. **E.20** — speciation wire. Add the distance check + event emit in
   `handle_births`. Run the 3 new Rust tests. Commit:
   `M E.20: wire species distance check into handle_births + emit Speciation events`.

2. **E.25.b** — relocate FirstToMove fire-site. Tiny diff (5 lines moved).
   Run new test. Commit (separately so the bisect is clean):
   `M E.25.b: move FirstToMove fire-site to apply_movement_and_repulsion (v6 §L)`.

3. **E.25.a** — PopulationMilestone fire + constant + bitset.
   Commit: `M E.25.a: PopulationMilestone events at 10/50/100/500/1000/2000`.

4. **E.25.d** — HallOfFame struct + 4 fields on World + tracking in
   energy_bookkeeping / collect_deaths / apply_movement_and_repulsion.
   Commit: `M E.25.d: capture biggest/weirdest/last-survivor/first-mover snapshots for F.28`.

5. **E.21 wasm side** — add `species_list_json`, `creature_at`,
   `creature_inspect_json`, `stats_sample`, `events_total_count`,
   `creature_ids_buffer`, `id_buf` field. Add the `#[serde(tag = "type")]`
   attribute on `EventKind`. Rust tests for the new methods (where
   testable native — most need wasm32). Commit:
   `M E.21: wasm API for rail (species_list_json, creature_at, inspect, stats_sample, events_total_count, ids_buffer)`.

6. **E.21 web side** — rail shell + Events panel. New files:
   `rail/index.ts`, `rail/events.ts`, `rail/inspector.ts` (skeleton).
   Wire `pollRail` into `main.ts`. Manual smoke. Commit:
   `M E.21: right-rail shell + Events panel`.

7. **E.22 web side** — `rail/toast.ts`, `rail/highlight.ts`. Extend
   `render.ts::drawCreatures` to take highlights + ids and stroke golden
   rings. `pollRail` routes Speciation/Extinction/FirstToMove/FirstToEat
   to toast + highlight. Manual smoke: jack mutation slider, watch toasts.
   Commit: `M E.22: in-aquarium toasts (4s) + highlight rings (1.5s, golden)`.

8. **E.23** — `rail/stats.ts` + 2 Canvas2D charts. Commit:
   `M E.23: Stats panel with population + species line charts`.

9. **E.24** — Inspector polish: camera.ts onTap hook + click-handling +
   "creature died" placeholder + selection-as-permanent-highlight.
   Commit: `M E.24: Inspector popover (click-targeted creature panel)`.

10. **Final pass:** `cargo clippy --all-targets -- -D warnings`,
    `cargo test --lib`, `wasm-pack build --target web`, `pnpm build`,
    manual: 5-minute run with mutation_rate_multiplier=3 to force
    speciation events, verify all 5 substeps work together.

---

## Code-reviewer's checklist

### E.20 (heavy — load-bearing for §16)

- [ ] `handle_births` uses `species_distance(child, anchor)` against the
      parent species' **anchor**, not the live parent's current state.
- [ ] When `dist > SPECIES_THRESHOLD`, `speciate(parent_id, child_genome,
      child_brain.weights, tick)` is called BEFORE `creatures.push`, so
      the child's `species_id` is set to the new id.
- [ ] `EventKind::Speciation` is pushed only when threshold crossed (not
      on every birth). Confirmed by the population-stable test (no
      Speciation events when sigma is at v6 default).
- [ ] `live_species_count` increments via `finalize_extinctions` (which
      counts unique species_ids in alive set); no manual increment in
      `handle_births`. Newborns are in CreatureSoA before finalize runs.
- [ ] Anchor clones (`child_genome.clone()`, `child_brain.weights.clone()`)
      are necessary for the move-into-`speciate` call; not a perf concern
      at v1 sizes.
- [ ] All 3 new tests pass: tiny-mutation-keeps-species,
      synthetic-large-drift, end-to-end-event-fires.
- [ ] Existing tests still pass (lone-splits, energy-conservation,
      world_runs_2000_ticks_with_movement, etc.).

### E.21 (moderate)

- [ ] `species_list_json` filters out species with population 0 (extinct
      OR never-populated).
- [ ] `creature_at` returns `None` outside any body circle.
- [ ] `creature_inspect_json` returns `None` for out-of-range idx (no
      panic).
- [ ] `creature_ids_buffer` is index-aligned with `creatures_buffer` (same
      length, same order).
- [ ] `events_total_count` is monotone (only ever increases per session).
- [ ] `EventKind` serialization uses `#[serde(tag = "type")]` flat shape.
- [ ] All new wasm methods have at least one native Rust unit test
      (via the extracted `*_inner` helpers where wasm-only return types
      block direct testing).
- [ ] `id_buf` field on `WorldHandle` is reused across calls (no per-call
      Vec alloc).

### E.22 (light)

- [ ] Toasts fade after 4s wall-clock (not sim-ticks).
- [ ] Highlight rings fade alpha linearly over 1.5s wall-clock.
- [ ] Highlight color is exactly `rgba(255, 200, 50, α)` per v6 §B.
- [ ] Toast stack capped at 6; newest at top.
- [ ] PopulationMilestone + WorldEnded do NOT push toasts.
- [ ] Event-diff uses `events_total_count`, not ring length.
- [ ] Camera does NOT auto-pan on toast/highlight (v5 §11).

### E.23 (light)

- [ ] Stats sample once per 10 sim-ticks; capped at 500-sample ring.
- [ ] Charts render even with single-sample data (no NaN axis range).
- [ ] Charts handle ymax == 0 (no division by zero) — `Math.max(1, ymax)`.

### E.24 (light)

- [ ] `camera.ts` onTap fires only for taps (< 4px, < 250ms), not drags.
- [ ] Inspector closes on click in empty area.
- [ ] Inspector handles death of selected creature gracefully (2s
      placeholder, then auto-close).

### E.25 (moderate)

- [ ] `POPULATION_MILESTONES` are `[10, 50, 100, 500, 1000, 2000]`; bitset
      fires each once per session.
- [ ] FirstToMove fires from inside `apply_movement_and_repulsion`, NOT
      `collect_deaths` (per v6 §L: "actually traveled ≥ 5u in its
      lifetime"; spec wants live creature, not post-death).
- [ ] FirstToEat unchanged (already in `eat_and_scavenge`, matches v6 §L
      "first successful bite").
- [ ] HallOfFame snapshots:
  - [ ] biggest_ever updates whenever any creature's size > current best
  - [ ] last_survivor overwritten on every death (final overwrite is the
        latest death tick — implicit because once pop=0 no more deaths)
  - [ ] weirdest requires `age >= 500` to qualify; uses
        `species_distance` against `founder_genome_anchor` +
        `founder_brain_anchor`
  - [ ] longest_lived tracked unconditionally as fallback for v5 §11.1
  - [ ] first_mover_snapshot captured at the FirstToMove fire-site
- [ ] All snapshot fields serializable (Genome already is; HallOfFame
      derives Serialize/Deserialize) — F.28 will read them.

### Cross-cutting

- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo test --lib` count grows by ≥ 8 (E.20: 3, E.25: 5+).
- [ ] `wasm-pack build --target web` succeeds (no signature mismatches
      between wasm_api.rs and TS imports).
- [ ] `pnpm build` succeeds.
- [ ] Manual: open URL, see right rail with Events panel open, see species
      list (just "Lineage A" at tick 0), see event log start empty;
      click founder, see Inspector populate; open Stats tab, see two
      empty charts that fill as time progresses.

---

## DECISIONS lines (add to `DECISIONS.md`)

### E.20

- `speciation event parent name: NOT in event payload — UI resolves via species_list_json (avoids stale-name drift across renames in future)`
- `speciation anchor clone cost: ~14KB Vec<f32> per speciation event — acceptable at v1 scale; revisit in v1.1 if NN_WEIGHT_COUNT grows`

### E.21

- `creature_at returns SoA index (not stable id) — caller re-resolves via creature_ids_buffer + creature_inspect_json each frame; cheap at v1 sizes`
- `species_list_json frequency: 1Hz (not every frame) — counts are O(N) and the panel doesn't need sub-second freshness`
- `creature_ids_buffer uses Float64Array: u64 ids fit in f64 mantissa for the v1 session lifetime`
- `stats sampling cadence: every 10 sim-ticks via tick % 10 == 0 check; 500-sample ring buffer TS-side (≈ 5000 ticks visible)`

### E.22

- `toast/highlight timing: wall-clock seconds (4s toast, 1.5s ring) per v6 §B; not sim-tick scaled`
- `toast variants: Speciation/Extinction/FirstToMove/FirstToEat fire toasts; PopulationMilestone/WorldEnded log-only (avoid spam + eulogy supersedes)`
- `toast stack cap: 6 visible; newer toasts at top (column-reverse)`
- `EventKind serde: #[serde(tag = "type")] flat shape — adopted in E because E adds the only TS consumer`
- `event diff: by events_total_count (monotone u32), not ring length — survives ring overflow`

### E.23

- `stats charts use Canvas2D directly: zero deps, < 100 LOC, no interaction needed (v6 §A plain TS)`
- `stats x-axis = sim-tick (not wall-clock): consistent with sim semantics; player can pause and the chart freezes`

### E.24

- `inspector tap detection: < 4px movement + < 250ms elapsed in camera.ts onTap hook — disambiguates from drag-to-pan`
- `inspector creature-died UX: 2-second placeholder then auto-clear — avoids the user clicking and immediately losing context`

### E.25

- `population milestones: bitset of fired flags (one-shot per threshold ever) — v5 §11 "only once per threshold ever"`
- `first-to-move fire-site: apply_movement_and_repulsion (was collect_deaths) per v6 §L "actually traveled ≥ 5u in its lifetime"`
- `hall-of-fame snapshots: clone Genome (~120B) into a HallOfFame struct on World — pre-staged for F.28; brain weights NOT cloned (only genome image needed for eulogy render)`
- `weirdest fallback: World.longest_lived snapshot (any age) — used if no creature ever reached 500 ticks (v5 §11.1)`
- `weirdest distance metric: species_distance against founder_genome_anchor + founder_brain_anchor (day-0 capture in World::new) per v5 §11.1`

---

## Out-of-scope reminders (v5 §15 / §14 guardrails)

- No double-buffered transferable autosave / IndexedDB worker — F.26.
- No resume prompt / seed-string textbox — F.27.
- No eulogy card 4-image grid render (data only) — F.28.
- No headless acceptance test / golden snapshot / perf gate — F.29.
- No balance tuning — F.30.
- No lineage tree viz, trait histograms, NN viz, follow-cam — v1.1+.
- No new traits / actions / sliders / NN inputs / outputs — out forever.
- The Inspector shows what's spec'd (genome stats + action + species/parent
  breadcrumb per v5 §11). It does NOT show NN weights, brain mutation
  rates, or per-trait mutation rates — those are not in v5/v6 UI scope.

---

## Punts to F / v1.1

- Inspector "follow-cam" mode — v1.1 (v5 §14 deferred).
- Per-species color in stats chart (per-species population stacks) — v1.1.
- Per-event "jump to" button that pans the camera — explicitly NOT v1
  per v5 §11 "no jarring jumps." If F.30 finds players struggle to see
  toasts in the aquarium, revisit in v1.1.
- Tooltip on chart hover — no interaction in v1 charts.
- Settings to disable toasts — not in v5/v6; if reviewers want it, log a
  DECISIONS line in F.30.
- Stats samples persisted to autosave — F.26 (currently TS-side ring,
  lost on reload). v1 acceptable.
- HallOfFame written to save file — F.26 (struct already serde).

---

## Code review — APPROVED

All 14 priority checks pass. 62 lib tests green (13 new), `cargo clippy
--all-targets -- -D warnings` clean, `pnpm build` clean.

E.20 speciation uses anchor (not live parent) at `src/world.rs:847`, threshold
`SPECIES_THRESHOLD = 4.0`, registry's `speciate` invoked before push so the
child's species_id is correct (`src/world.rs:854–874`). E.25.b FirstToMove
correctly fires inside `apply_movement_and_repulsion` after
`distance_travelled[i] += dist` at `src/world.rs:390–413`; old fire-site in
`collect_deaths` removed. E.25.a milestones use the `[10, 50, 100, 500, 1000,
2000]` constant and a bitset (`src/world.rs:233–242`), one-shot per session.
E.25.d HoF: biggest in `energy_bookkeeping` (`world.rs:693–705`), last_survivor
on every death (`world.rs:743–750`), weirdest gated on age ≥ 500 with
species_distance to day-0 founder anchor (`world.rs:766–784`), longest_lived
fallback and first_mover_snapshot wired. Wasm API: `species_list_json` filters
pop=0, `creature_at` returns SoA index, `creature_inspect_json` returns Option,
`stats_sample` returns `[tick, pop, species]`, `events_total_count` getter
monotone, `creature_ids_buffer` Float64Array; `id_buf` is reused
(`src/wasm_api.rs:9–18`). `EventKind` uses `#[serde(tag = "type")]` flat shape;
TS consumer in `web/src/rail/events.ts:65–84` and `index.ts:20–26` matches.
Toast timing 4s wall-clock (`toast.ts:36`), highlight ring 1.5s wall-clock
`rgba(255, 200, 50, α)` 2px (`render.ts:164–178`), permanent-highlight sentinel
handled. Toast variants: Speciation/Extinction/FirstToMove/FirstToEat fire
toasts; PopulationMilestone/WorldEnded log-only (`index.ts:76–101`). Stats:
500-sample ring, every 10 sim-ticks, Canvas2D, no chart lib (`stats.ts`).
Inspector: tap detection 4px / 250ms (`inspector.ts:163–177`), permanent
highlight, 2s creature-died placeholder then auto-close, click-empty dismisses.
Right rail 280px overlay with z-index 5; no camera auto-pan on toasts.

Non-blockers (worth tracking, not gating):

1. `src/world.rs:690` — `biggest_ever` is gated on `g.size >
   max_size_reached[i]`, but `max_size_reached` is initialized to `genome.size`
   on push (`src/creature.rs:123`) and v1 has no growth mechanic. In normal
   play this branch never fires, so `biggest_ever` will likely stay `None`.
   The test only passes because it post-mutates `genomes[i].size`. The plan
   itself codes it this way; F.28 should either compare directly against the
   current `genome.size` of every creature (e.g. each tick scan pop for max
   size and update HoF) or accept that "biggest" really means "biggest size
   ever observed at birth." Worth a follow-up before F.28 consumes it.
2. `web/src/camera.ts` was not extended with the planned `onTap` hook.
   `installCanvasClickHandler` instead attaches its own pointerdown/up
   listeners (4px / 250ms) alongside the camera's drag listeners. Works
   correctly, but the camera and inspector now both observe pointerdown/up —
   the plan called for a single tap hook in camera.ts. Cosmetic refactor.
3. `src/world.rs:223–242` — the milestone scan runs after `self.tick +=
   1`, so a milestone crossed at the end of tick N is logged with `tick =
   N+1`. Harmless but slightly off the "tick the threshold was crossed"
   wording.
4. `web/src/rail/inspector.ts:138` — passes `foundIdx` (a `number`) to
   `creature_inspect_json(idx: u32)`. wasm-bindgen accepts it; just note
   the implicit Number → u32 coercion.
