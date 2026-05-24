# Wasm Boundary Audit (`src/wasm_api.rs` ↔ `web/src/`)

Scope: every JS↔wasm crossing on the per-frame path plus the 1Hz/click/save paths. File:line refs are absolute; signatures are quoted as they appear today.

## Summary of per-frame crossings (RAF tick)

Today, each RAF frame issues these boundary calls in this order:

| # | Call | File:line | Notes |
|---|---|---|---|
| 1 | `world.step_n(n)` | main.ts:280 | Unavoidable; the actual work. |
| 2 | `world.world_ended` (getter) | main.ts:284, 289, 303 | 3 separate gets per frame. |
| 3 | `world.creature_ids_buffer()` | main.ts:295 | Allocates+copies u64→f64 buffer every frame. |
| 4 | `world.tick`, `world.seed`, `world.population`, `world.species_count`, `world.world_size`, `world.sun_dim` (getters) | main.ts:304 / render.ts:221-224 | 6+ getters every frame for status bar + render. |
| 5 | `world.sun_buffer()` | render.ts:221 | Allocates+copies normalized fracs every frame. |
| 6 | `world.sun_capacity_buffer()` | render.ts:221 | Zero-copy (direct view) — already good. |
| 7 | `world.carrion_buffer()` | render.ts:223 | Allocates+copies SoA→AoS every frame. |
| 8 | `world.creatures_buffer()` | render.ts:224 | Same; the big one. |
| 9 | `pollRail` (refreshInspector, stats) | main.ts:298 | Within: `world.creature_inspect_json(idx)` (JSON parse every frame while inspector open); `world.stats_sample()` every 10 ticks; `world.species_list_json()` every ~1 s. |

Roughly **~10 separate boundary crossings per frame** before the inspector/rail polls. wasm-bindgen getters cost ~50–200 ns each + GC overhead from the returned `String` allocations on `tick`/`seed`. The string getters dominate (`seed` is hot — re-cloned on `main.ts:304` every frame).

---

## Hot-path issues

### H1. `creatures_buffer` repacks SoA→AoS every frame
`src/wasm_api.rs:103-133` clears+resizes `creature_buf` and copies 13 floats per creature on every call. At 1000 creatures × 60 fps that's 60×13×1000 = ~0.8 MB/s of memcpy + per-element multiplies (energy/age normalization, ring-flag branches). The returned `Float32Array::view` is correctly zero-copy *after* the fill.

**Proposed**: split into two layers.
- Keep a persistent SoA layout in wasm linear memory matching what the renderer reads (separate `Float32Array` views per column: `x[]`, `y[]`, `radius[]`, `r[]`, `g[]`, `b[]`, `flags_u8[]`). Expose **pointers + len** via getters that return `usize`/`u32`; build the `Float32Array` views once in JS using `new Float32Array(memory.buffer, ptr, len)` and refresh only when the buffer moves (i.e. on push/remove).
- Renderer iterates columns directly. No per-frame repack.

Current sig:
```rust
pub fn creatures_buffer(&mut self) -> js_sys::Float32Array
```
Proposed sigs:
```rust
pub fn creatures_xy_ptr(&self) -> *const f32  // x then y interleaved, or two ptrs
pub fn creatures_rgb_ptr(&self) -> *const f32
pub fn creatures_radius_ptr(&self) -> *const f32   // derived; see below
pub fn creatures_flags_ptr(&self) -> *const u8     // packed bits: eye|move|scav|mouth|armor
pub fn creatures_len(&self) -> u32
pub fn creatures_buffer_version(&self) -> u32      // bump on push/remove → JS re-views
```

Expected speedup: eliminates one SoA→AoS pass per frame (the loop at `wasm_api.rs:108-131`). Order of magnitude: **0.2–0.5 ms saved per frame at 1k creatures**, plus removes the only big per-frame allocation churn (`creature_buf` resize on grow). Most importantly: lets the renderer become tight column scans.

Notes:
- `radius_world = g.size * BODY_RADIUS_PER_SIZE` could be computed once at push and stored alongside `g_size` (or just read from `g_size` and multiplied in the renderer).
- Flag bits should be a packed `u8` (one byte per creature) instead of 5 floats. Saves 20 B/creature.
- `energy_frac` and `age_frac` are currently computed in Rust each frame and not actually read by the renderer (`render.ts:134-135` comments them out). **Drop fields 6–7 entirely** until something consumes them.

### H2. `creature_ids_buffer` rebuilds an f64 mirror every frame
`src/wasm_api.rs:208-214`: clears `id_buf`, pushes `id as f64` for every creature. This is pure boundary tax — the renderer only uses ids to map highlights/inspector idx. `creature.rs:45` stores `id: Vec<u64>` already.

**Proposed**: expose pointer + len directly:
```rust
pub fn creature_ids_ptr(&self) -> *const u64
pub fn creature_ids_len(&self) -> u32
```
JS builds a `BigUint64Array` view once and re-views on version bump. Comparisons against the selected creature id become `BigInt` compares (cheap for the inspector — one creature) **or** keep f64 by storing a parallel `Vec<f64>` mirror that's only written on push/remove (no per-frame work).

Even cheaper: since the renderer only needs ids to look up highlights, and the highlight map is tiny, the renderer could index into `world.creatures.id` only when the highlight map is non-empty. That's already what `render.ts:117-125` does for the highlight set, but main.ts:295 fetches `ids` unconditionally and `inspector.ts:122-128` does a linear scan of `idsBuffer` every frame. Both go away with `creature_at_id(id) -> Option<u32>` (see M2).

Expected speedup: eliminates per-frame f64 push loop (~0.05–0.2 ms at 1k creatures) and the JS-side linear id→idx scan in `inspector.ts:122-128`.

### H3. `sun_buffer` recomputes `current/capacity` every frame
`src/wasm_api.rs:136-146`: divides each cell every frame. `sun.current` and `sun.capacity` are already separate `Vec<f32>` in linear memory (`sun.rs:9-10`).

**Proposed**: expose the raw buffers; divide on the GPU (canvas2d won't help here, but trivially in the JS draw loop, which already takes the result per-cell anyway):
```rust
pub fn sun_current_ptr(&self) -> *const f32
pub fn sun_capacity_ptr(&self) -> *const f32
pub fn sun_len(&self) -> u32
```
The renderer at `render.ts:67-73` already does `(cap-1)/6` and other math per cell, so adding one more divide is free.

Expected speedup: skips full `sun_buf` rewrite per frame (SUN_DIM² floats). At SUN_DIM=64 that's 4096 ops/frame; negligible compute but eliminates an allocation/copy. Also collapses two boundary calls into ptr getters.

Note: `sun_capacity_buffer` (`wasm_api.rs:149-152`) is **already** a `view(&self.inner.sun.capacity)` with no copy. Good. But it's called every frame for a buffer that only changes when `sun_gradient_strength` slider moves. Cache the view in JS keyed on a version counter.

### H4. `carrion_buffer` SoA→AoS repack
`src/wasm_api.rs:156-167`. `Carrion` is `Vec<Carrion>` (AoS) in Rust (`world.rs:54`) — so the repack into `[x, y, pool_frac]` is unavoidable until carrion goes SoA. Smaller volume than creatures.

**Proposed (cheap)**: keep current shape but expose pointer:
```rust
pub fn carrion_buffer(&mut self) -> *const f32   // still owns &mut self for repack
pub fn carrion_len(&self) -> u32  // # carrion (triples = len*3)
```
JS does `new Float32Array(memory.buffer, ptr, len*3)`.

**Proposed (better)**: refactor `Carrion` to SoA (`x: Vec<f32>`, `y: Vec<f32>`, `pool: Vec<f32>`) — sim hot paths would benefit too — then expose three ptrs zero-copy.

Speedup minor (~0.05 ms at typical carrion counts) but consistent with the other buffers.

### H5. Status-bar string template fires getters every frame
`web/src/main.ts:304` calls `world.seed`, `world.tick`, `world.population`, `world.species_count`, `world.world_ended` for the status string. Each getter is a wasm trampoline; `seed` returns a freshly cloned `String` (allocation on every frame).

**Proposed**:
- Cache `seed` once at boot (it's invariant — already passed to `installSeedDisplay` at main.ts:219). Drop `seed` from status updates.
- Batch tick/pop/species into a single typed-array call:
  ```rust
  pub fn status_sample(&self) -> Box<[u32]>  // [tick, pop, species, world_ended as u32]
  ```
  Or reuse `stats_sample` (which already returns `[tick, pop, species]` as f32 — see S1).
- Throttle the status DOM update to ~5 Hz; rebuilding `textContent` 60×/s wastes layout.

Expected: 4 fewer boundary calls per frame + no `String` clone of `seed` per frame.

### H6. Multiple `world_ended` getter calls
`main.ts:284, 289, 303` — 3 getter trampolines per frame for the same boolean. Trivial fix in JS: hoist to a local at the top of `frame()`.

### H7. `creature_inspect_json` per frame while inspector open
`web/src/rail/inspector.ts:143` calls `world.creature_inspect_json(foundIdx)` every RAF frame while a creature is selected — that's JSON serialization (`wasm_api.rs:254-279`, a `serde_json::json!` macro build + `to_string`) plus a JS `JSON.parse` of ~20 fields, **every frame**. Most fields are invariant per creature (genome traits, max_age, species_name) and only `age`, `energy`, `current_action`, `x`, `y` change tick-to-tick.

**Proposed**: split into two calls.
```rust
pub fn creature_inspect_static(&self, idx: u32) -> Option<String>   // genome + species + parent (called on selection only)
pub fn creature_inspect_dynamic(&self, idx: u32) -> Box<[f32]>      // [age, energy, x, y, action_id] every frame
```
Action becomes a small enum id (u8) lookuped in JS to a string. No JSON. Renderer/inspector reads from a 5-float Float32Array each frame.

Expected speedup: removes a `serde_json::to_string` + `JSON.parse` from every frame while inspector is open. Probably **0.1–0.5 ms per frame** and much less GC pressure.

---

## 1Hz/click/save paths

### S1. `stats_sample` returns `Box<[f32]>` (copied)
`src/wasm_api.rs:285-291`: returns `Box<[f32; 3]>`. wasm-bindgen marshals this as a fresh `Float32Array` (copy from linear memory to JS heap). Fine at 1/10-tick frequency. **Keep**, but: since this is the same data as the status bar (H5), consolidate into one API used by both paths.

### S2. `species_list_json` builds a JSON every poll
`src/wasm_api.rs:201, 397-421`: O(N+S) HashMap aggregate + JSON serialize at 1 Hz. With v1 species counts (small) this is fine. **No change needed.**

### S3. `profile_report_json` — JSON at 1 Hz
`src/wasm_api.rs:349-351`. ~5 KB JSON, 1 Hz, only when profiler is on. Fine.

### S4. `recent_events_json` — dead code
`src/wasm_api.rs:187-189`. No callers in `web/src/`. `rail/index.ts:112` comment confirms event diff was removed. **Delete.**

### S5. `events_total_count` — dead code
`src/wasm_api.rs:194-196`. No callers in `web/src/` (only in tests). **Delete.**

### S6. `set_slider` — dead code
`src/wasm_api.rs:171-183`. No callers in `web/src/`. Dev-panel sliders apparently never shipped. **Delete or wire the UI.**

### S7. `snapshot_json` is the right shape
`src/wasm_api.rs:299-302`: returns a `String`, passed straight to the persistence worker via `postMessage` (`main.ts:93`). Already avoids main-thread parse. Good. (Comment at main.ts:90-92 confirms intentional.)

### S8. `creature_at` is O(N) linear scan
`src/wasm_api.rs:226-238`: linear over all creatures on click. The world already has a `SpatialGrid` (perf-3 cursor work) — `creature_at` should use it.

**Proposed**:
```rust
pub fn creature_at(&self, wx: f32, wy: f32, tol: f32) -> Option<u32>
// internally: grid.query(wx, wy, max_radius+tol) → distance test only against candidates
```
At 1 Hz click rate this is cosmetic, but linear scans at 1k+ creatures will start to show. Easy win for free with existing infrastructure.

### S9. `creature_at` returns SoA index, not stable id
`wasm_api.rs:224` and `inspector.ts:199-204` then immediately calls `creature_inspect_json(idx)`. But every subsequent frame `refreshInspector` (`inspector.ts:122-128`) scans the entire `idsBuffer` to map `creatureId → idx` because indices shift on removal.

**Proposed**: add `creature_idx_by_id(id: u64) -> Option<u32>` so JS doesn't need `creature_ids_buffer` at all for the inspector path. Combined with H2/H7, this eliminates the per-frame `creature_ids_buffer` boundary call entirely (it would only be needed if the highlight map were big, which it's not).

```rust
pub fn creature_idx_by_id(&self, id: u64) -> Option<u32>
// O(N) today; O(1) with an id→idx HashMap on World (worth it).
```

Expected speedup: eliminates one boundary call + JS linear scan per frame while inspector is active. Maybe **0.05–0.1 ms/frame**, mostly tidiness.

### S10. `hof_json` — fine
`src/wasm_api.rs:372-391`: one-shot at world end. JSON is fine.

---

## Missing APIs the UI clearly wants

### M1. Batched per-frame snapshot
Today the renderer needs `creatures_buffer + sun_buffer + sun_capacity_buffer + carrion_buffer + creature_ids_buffer + world_ended + tick + population + species_count` per frame. That's **9 boundary crossings**.

**Proposed**: one `frame_snapshot()` that returns nothing but bumps a single "all buffers consistent now" version counter. JS holds permanent `Float32Array`/`BigUint64Array` views into linear memory (refreshed only when `version_buffers` changes — i.e. after a creature push/remove or memory.grow). Scalars (tick/pop/species/world_ended) read from a tiny shared metadata Float32Array.

```rust
pub fn metadata_ptr(&self) -> *const u32   // [tick, pop, species, world_ended, buffers_version, mem_version]
pub fn metadata_len() -> u32  // const 6
```

JS:
```ts
const meta = new Uint32Array(memory.buffer, world.metadata_ptr(), 6);
// per frame: read meta[0..4] from a Uint32Array view; no calls.
// if (meta[5] !== lastMemVersion) → re-view all buffer Float32Arrays
```

This is the single biggest structural improvement: **collapses ~10 trampolines per frame to ~0 plus a typed-array read.** Expected: **0.1–0.3 ms/frame** removed from boundary overhead alone (wasm-bindgen trampoline + JS object creation for each `Float32Array` return), plus the indirect wins from H1/H2/H3/H4 above.

### M2. `creature_idx_by_id`
See S9.

### M3. Inspector split (static/dynamic)
See H7.

### M4. SharedArrayBuffer is already on
`main.ts:131-139` calls `initThreadPool`. SAB is required for that anyway, so all the `*_ptr` proposals above are safe to ship (linear memory is already SAB-backed). The current `Float32Array::view` calls (`wasm_api.rs:132, 145, 151, 166, 213`) already rely on direct linear-memory views, so this is just doing more of the same.

### M5. Transferable types
Saves go to the persistence worker via `postMessage(snapshot_json)` (`main.ts:93`). The string is structured-cloned. For multi-MB saves, consider:
- Have `snapshot_json` write into a fresh `Uint8Array` backed by an `ArrayBuffer` we can **transfer** to the worker (zero-copy). API: `snapshot_bytes() -> Vec<u8>` returns a `Uint8Array`; in JS, slice into a fresh `ArrayBuffer` and pass `[ab]` in the transfer list.
- Or: keep JSON but use `TextEncoder` to a transferable buffer.

Speedup depends on save size; at v1 sizes (hundreds of KB) the gain is small. Not urgent. Note structured clone of a `String` is reasonably fast in V8 because strings are immutable & ref-counted.

---

## Awkward call patterns

### A1. `render.ts:221` long argument lists with getters inline
```ts
drawSunMap(ctx, cam, viewW, viewH, world.world_size, world.sun_dim, world.sun_buffer(), world.sun_capacity_buffer());
```
`world.world_size` and `world.sun_dim` are constants for the run. Hoist to const at boot. **Trivial.**

### A2. `creatures_buffer()` takes `&mut self`
`wasm_api.rs:103` requires `&mut` only because of the scratch `creature_buf`. If we move to the ptr API (H1), this becomes `&self` and unlocks parallel reads (theoretical — wasm is single-thread for JS callers anyway). More importantly, `&mut self` here means JS can only hold one borrow at a time, which precludes overlapping the renderer with a side panel reading creature data. Not a real bottleneck today, but a smell.

### A3. `creature_stride` is a free function
`wasm_api.rs:429-431`. Imported at `main.ts:1`. Fine; it's a constant. With M1's flat-column proposal, stride goes away entirely.

### A4. `world.seed` returns a fresh `String` every call
`wasm_api.rs:67-70`. Used at status bar every frame. Cache in JS.

---

## Stale APIs to remove

| API | File:line | Reason |
|---|---|---|
| `recent_events_json` | wasm_api.rs:187 | Unused in web/src (event diff removed; rail/index.ts:112). |
| `events_total_count` (getter) | wasm_api.rs:194 | Unused in web/src. |
| `set_slider` | wasm_api.rs:171 | No UI wiring. |
| `step` (single-tick) | wasm_api.rs:47 | Web only uses `step_n`; native tests use it. Keep but mark `#[cfg(any(test, feature = "native"))]` or accept as small surface. |

Removing the three above shrinks the wasm-bindgen glue and removes `serde_json::to_string` of events from the binary.

---

## Recommended priority order

1. **M1 (batched metadata + permanent buffer views)** — collapses ~10 per-frame trampolines; foundational.
2. **H1 (creatures_buffer → column ptrs)** — kills the per-frame SoA→AoS copy.
3. **H7 + S9 + M2 (inspector dynamic split + idx_by_id)** — kills per-frame `creature_inspect_json` JSON cycle.
4. **H2/H3/H4 (ids/sun/carrion via ptrs)** — same pattern as H1, smaller payoff individually.
5. **H5/H6/A1/A4 (status bar getter hygiene)** — trivial wins.
6. **S4/S5/S6 (delete stale APIs)** — code hygiene.
7. **S8 (creature_at via SpatialGrid)** — cheap, future-proofs against larger N.
8. **M5 (transferable save bytes)** — only if save sizes grow.

Combined expected per-frame savings at 1k creatures: **~0.5–1.2 ms** of boundary/serialization tax removed, with the bulk coming from H1 + M1 + H7. At the current sim sizes the wins are modest but the structural simplification (one snapshot point, permanent views, no per-frame JSON) is the bigger benefit and unblocks scaling to 10k+ creatures.
