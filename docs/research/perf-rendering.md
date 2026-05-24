# JS/TS Rendering Performance — Research Notes

_Repo: `/home/adamg/evosim`. Research only; no code was modified._

---

## Summary Table

| # | Opportunity | Est. frame-time saved | Effort | Risk |
|---|---|---|---|---|
| 1 | **Inspector throttle** — `refreshInspector` fires every frame; JSON.parse per frame | 0.3–1 ms/frame when active | Low | Low |
| 2 | **Canvas2D drawCreatures path-per-creature** — ~3000–9000 path ops/frame at 1500 creatures | 2–6 ms/frame | Medium | Low |
| 3 | **drawSunMap 400 fillRect + string-per-cell** — one `fillStyle` string allocation + `fillRect` per cell | 0.5–1.5 ms/frame | Medium | Low |
| 4 | **status DOM write every frame** — unconditional `textContent` set | 0.05–0.15 ms/frame | Trivial | None |
| 5 | **Stats chart redraws on every sample** — `maybeSampleStats` gates sampling on tick % 10 but then always redraws | 0.2–0.5 ms/sample | Low | None |
| 6 | **wasm buffer repacking cost** — `creatures_buffer()` repacks all N creatures into a JS Float32Array every frame | 0.5–2 ms/frame at 1500 creatures | Rust-side Medium | Low |
| 7 | **Off-screen creature culling** — full draw path runs even for creatures outside the viewport at high zoom | 1–4 ms/frame at zoom > 4 | Low | Low |
| 8 | **Render cadence at high sim speed** — 60 fps rendering while sim runs at 100× is human-imperceptible | Frees full frames entirely at N/fps | Low | Medium |
| 9 | **worldToScreen tuple allocation** — returns `[number, number]` per call, called 1×creature+per-cell | GC pressure; hard to quantify | Medium | Low |
| 10 | **`creature_ids_buffer` view safety** — unsafe wasm view can go stale if wasm memory grows mid-frame | Correctness risk, not a perf win | Medium | High |

---

## 1. Per-Frame Allocations — Wasm Buffer Views

### `creature_ids_buffer` and `creatures_buffer`

**File:** `src/wasm_api.rs:208–213` (`creature_ids_buffer`), `src/wasm_api.rs:103–133` (`creatures_buffer`)

Both functions reuse a `Vec<f32>` / `Vec<f64>` stored on `WorldHandle` (`creature_buf`, `id_buf`). They call `.clear()` + `.resize()` every frame — O(N) write — then return an `unsafe` JS view backed by wasm linear memory:

```rust
// wasm_api.rs:132
unsafe { js_sys::Float32Array::view(&self.creature_buf) }

// wasm_api.rs:213
unsafe { js_sys::Float64Array::view(&self.id_buf) }
```

**No new allocation on the Rust side** — the `Vec` is reused and grows monotonically to the high-water mark of creature count, which is good. However, *the JS-visible typed array is a zero-copy view into wasm linear memory*. The wasm spec (and wasm-bindgen documentation) warns that any wasm operation that grows wasm memory (`step_n` allocating more creature slots, for example) can move the underlying buffer, invalidating all outstanding views.

**Current usage in `main.ts:269`:**

```ts
const ids = world!.creature_ids_buffer() as unknown as Float64Array;
// ...
world!.step_n(ticksThisFrame);    // ← happens BEFORE buffer is fetched this frame
// ...
renderWorld(..., world!, stride, ids, ...);
```

Wait — the actual order in `main.ts:253–274` is:
1. `step_n(ticksThisFrame)` — line 254 — runs ticks that can grow the creature array
2. `creature_ids_buffer()` — line 269 — fetches the view *after* step_n has completed

So in the current code the view is fetched after `step_n` finishes for this frame. That means wasm memory growth during `step_n` has already happened before the view is obtained. **The view is safe for this frame** as long as nothing in `renderWorld`, `pollRail`, or any other called function triggers further wasm memory growth. `renderWorld` calls `world.sun_buffer()`, `world.sun_capacity_buffer()`, `world.creatures_buffer()`, and `world.carrion_buffer()` — the first three of these repopulate and return views from the same `WorldHandle` internal vecs. **Crucially**, every call to a `_buffer()` method may internally call `.resize()` on its backing `Vec`, which *could* trigger a wasm memory grow* if the vec has to expand.

Specifically:
- `sun_capacity_buffer()` (`wasm_api.rs:149–151`) returns a direct view of `self.inner.sun.capacity` with no repack — very low risk of grow.
- `sun_buffer()` (`wasm_api.rs:136–146`) clears and repopulates `self.sun_buf` — if `sun_buf` needs to grow (after a world restart or SUN_DIM change), that could trigger memory growth.
- `carrion_buffer()` (`wasm_api.rs:156–167`) similarly clears and repopulates `self.carrion_buf`.
- `creatures_buffer()` (`wasm_api.rs:103–133`) clears and repopulates `self.creature_buf`.

If `sun_buf` or `carrion_buf` trigger a wasm allocator grow, the already-returned `ids` view (obtained at line 269) would be silently invalidated. The window is small (these vecs are also high-watermarked and rarely grow), but the pattern is unsafe under strict wasm memory growth semantics.

**Practical fix:** re-fetch `ids` inside `renderWorld` right before use, or ensure `creature_ids_buffer()` is the *last* buffer call. Alternatively, use `Float64Array.from()` / `.slice()` to copy the data into a JS-owned buffer — adds a copy but makes the view-lifetime hazard disappear entirely.

### `sun_buffer`, `sun_capacity_buffer`, `carrion_buffer`

`sun_capacity_buffer` is the cheapest: it returns `Float32Array::view(&self.inner.sun.capacity)` with no repack at all (`wasm_api.rs:151`). This view points directly into the world's sun capacity vec — a 400-element f32 vec that never changes after init. Safe and zero-copy.

`sun_buffer` and `carrion_buffer` do a repack on every frame. `sun_buffer` repacks 400 floats (20×20 = 400 cells). `carrion_buffer` repacks 3 floats per carrion entity; typical count is small (tens of carrion at most).

### `JSON.parse(world.recent_events_json())` — Is It Called Per Frame?

**File:** `web/src/rail/index.ts:100–110`

`recent_events_json()` is **not** called per frame in the current code. The event diff path was removed for the v1.1 revisit (see comment at `index.ts:112`). The only JSON round-trip that *is* called each frame is in the inspector (`inspector.ts:143`, `JSON.parse(world.creature_inspect_json(foundIdx))`).

`species_list_json()` is polled at ~1 Hz (`index.ts:100`):

```ts
if (now - lastSpeciesPoll > 1000) {
    const json: SpeciesRow[] = JSON.parse(world.species_list_json());
    ...
    lastSpeciesPoll = now;
}
```

This is already well-gated. `species_list_json_inner` on the Rust side builds a full `HashMap<u32, u32>` from the creature species_id array on every call (`wasm_api.rs:377–399`) — O(N) — but at 1 Hz that's negligible.

---

## 2. Canvas2D Draw-Call Count

### drawSunMap — 400 fillRect Calls + 400 String Allocations

**File:** `web/src/render.ts:54–84`

Every frame, `drawSunMap` runs a 20×20 nested loop. Inside the loop:
- It calls `worldToScreen` (returning a `[number, number]` tuple — see section 9).
- It builds a color string: `ctx.fillStyle = \`rgb(${r},${g},${b})\`` — a template-literal string allocation per cell.
- It calls `ctx.fillRect(sx, sy, w, w)`.

That is **400 fillStyle string allocations** and **400 fillRect state-change-plus-draw calls** per frame. This is the dominant overhead for the background layer.

**Optimization path — `putImageData`:** Pre-allocate a `ImageData` object of 20×20 pixels, write RGBA bytes directly, then stretch it to screen with `drawImage(offscreenCanvas, ...)`. This collapses the 400 fillRect calls and 400 string allocations into a single `putImageData` + one `drawImage` call. The grid is redrawn every frame already (no caching), so switching to a pixel-write path is a pure win. Estimated saving: 0.5–1.5 ms/frame.

**Alternative — cache unchanged cells:** Between frames, the sun grid only changes for cells where creatures photosynthesized or where the refill ticked over. Cells that did not change can skip the fillRect. This requires a JS-side dirty-bit array. Simpler than putImageData but still involves 400 comparisons.

**Note on zoom antialiasing:** The fillRect approach lets the browser scale the world-space cells to screen coordinates with DPR-aware transform. `putImageData` bypasses the canvas transform and requires manual upscaling via `drawImage`. At zoom < 1 there could be a visible quality difference, but at the typical zoom 4× that evosim starts at this is not a concern.

### drawCreatures — Up to 6+ Path Operations Per Creature

**File:** `web/src/render.ts:106–191`

The creature draw loop does the following per creature:
1. One `fillStyle = \`rgb(${cr},${cg},${cb})\`` string allocation
2. `beginPath()` + `arc()` + `fill()` — 3 API calls for the body
3. Up to 5 rings × (`strokeStyle = ...` + `beginPath()` + `arc()` + `stroke()`) = up to 20 more calls
4. Optional highlight ring — 4 more calls if the creature is highlighted

At 1500 creatures, this is approximately **1500–7500 canvas API calls per frame** for the body+rings alone, plus a string allocation per creature for the fill color.

The key inefficiency is that every creature with the same color combination calls `fillStyle` separately. Canvas2D state changes (`fillStyle`, `strokeStyle`, `lineWidth`) are not free — the browser validates and potentially recompiles pipeline state on each change. **Grouping creatures by color** (sort by `(cr, cg, cb)`, batch all creatures with the same color into a single `beginPath`/multiple `arc` calls / `fill`) can reduce fill calls from 1500 to the number of distinct colors. In practice, with RGB evolution, there may be many distinct colors, so the benefit depends on how much color clustering evolves.

A cleaner middle path: **batch the body layer separately from the ring layer**. Draw all bodies first (1 beginPath, N arcs, 1 fill — if all are the same color) or group by quantized color. Then draw rings in a second pass. This reduces state transitions significantly even without perfect color grouping.

**The highlight-map scan** at `render.ts:119–125` iterates `ids.length` (= N creatures) every frame to build `highlightIdx: Set<number>`, even when `highlightMap.size === 0`. The guard (`if (highlightMap.size > 0)`) is present but only skips the inner loop entirely. This is O(N) every frame; at 1500 creatures with highlights inactive it is cheap but still creates a new `Set` on every frame allocation.

**The `new Set<number>()` allocation at render.ts:119** happens on every frame unconditionally. Even if `highlightMap.size === 0`, a fresh `Set` is allocated and discarded. Reuse a module-level `Set` that is cleared each frame instead.

### drawCarrion

**File:** `web/src/render.ts:86–104`

One `fillStyle` write, then a `beginPath`/`arc`/`fill` per carrion entity. Carrion count is typically small (< 50 entities at steady state based on `CARRION_MAX_AGE = 100` ticks). Not a meaningful hotspot.

---

## 3. Render Cadence vs. Sim Speed

**File:** `web/src/main.ts:247–280`

The RAF loop currently renders **every frame at 60 fps regardless of sim speed**. At `speed = 100`, the loop batches up to 200 sim ticks per frame (capped at `Math.min(200, ...)`) but still calls `renderWorld` after every batch.

At 100× sim speed, 200 ticks of evolution happen per frame but the visual output is a single render. The human eye cannot track individual creature movements that fast. Dropping render to every 2nd or 4th RAF at 100× would halve or quarter the Canvas2D cost with no perceptible visual loss.

A simple formula: `const renderEveryN = speed >= 100 ? 4 : speed >= 10 ? 2 : 1;` with a frame counter.

**Risk:** the stats panel chart (`drawCharts` in `stats.ts:38`) already gates on tick modulus — it runs only when sampling fires. Reducing render frequency does not affect it. The `status` DOM write and `pollRail` can continue every frame (they are cheap). Only the canvas `renderWorld` call needs to be skipped.

**Tabs hidden:** When `document.hidden` is true, the browser typically throttles RAF to 1 Hz. The sim tick batch still runs (since `world.step_n` is called before the RAF check). No explicit hidden-tab bypass exists in the current code (`main.ts:247–280`). The render still fires at 1 Hz when hidden. This is acceptable behavior; no bug here.

---

## 4. DOM Updates — `#status` Text

**File:** `web/src/main.ts:277`

```ts
status.textContent = `seed: ${world!.seed}  ·  tick ${world!.tick}  ·  pop ${world!.population}  ·  species ${world!.species_count}${ended}`;
```

This runs unconditionally every RAF frame. It creates a template-literal string each frame, accesses four wasm getter properties (each potentially crossing the wasm-JS boundary), and sets `textContent` which triggers browser text relayout. The browser may short-circuit relayout if the string is identical to the previous value (browser-dependent), but string construction and wasm getter calls still happen.

**Fix:** cache the last tick/pop/species values; only rebuild and set `textContent` when they change. At 60 fps with speed=1 (30 ticks/s), about half the frames produce a tick change. At speed=100, every frame changes tick. The wasm getters (`world.tick`, `world.population`, `world.species_count`) are simple struct-field reads on the Rust side — O(1) and cheap, but the JS-wasm boundary crossing adds a small overhead per call. Saving is minor but trivial to implement.

---

## 5. Stats Panel — Chart Redraws

**File:** `web/src/rail/stats.ts:27–39`

```ts
export function maybeSampleStats(world: WorldHandle): void {
  const tick = world.tick;
  if (tick === lastSampledTick) return;
  if (tick % SAMPLE_INTERVAL_TICKS !== 0) return;
  ...
  drawCharts();
}
```

The sampling guard (`tick % 10 !== 0`) works correctly at speed=1: roughly every 10 sim-ticks = every ~0.33s a sample is taken. At speed=100, tick advances by ~200 per frame, so roughly 20 samples fire per frame — every frame at steady state (since tick % 10 == 0 will hit many times per 200-tick batch, but `lastSampledTick` deduplicates to one sample per frame). Actually `lastSampledTick` stores the *last sampled tick*, so the check `tick === lastSampledTick` prevents double-sampling for the same tick. But the `tick % 10` check fires on whatever the current tick value is, not on every intermediate tick — so the sample rate at speed=100 is still effectively one per frame (one per RAF, not 200 per RAF). This is correct.

However, `drawCharts()` is called on every sample — meaning **every time sampling fires, two canvas line charts are redrawn**. At speed=1 this is ~3×/s; at speed=100 it is ~60×/s (once per RAF). Redrawing 500-point line charts 60 times per second on two canvases is not free.

**Fix:** add a wall-clock gate so `drawCharts()` fires at most once per second (or once per 250 ms), regardless of how many samples accumulated. The chart data changes slowly enough that 1–4 Hz redraws are indistinguishable from 60 Hz.

```ts
// Pseudocode — not modifying code here
let lastChartRedraw = 0;
if (now - lastChartRedraw > 1000) { drawCharts(); lastChartRedraw = now; }
```

---

## 6. Inspector — Per-Frame JSON Round-Trip

**File:** `web/src/rail/inspector.ts:113–152`

`refreshInspector` is called every RAF frame when a creature is selected (`index.ts:118`):

```ts
const jsonStr = world.creature_inspect_json(foundIdx);  // inspector.ts:143
if (!jsonStr) { ... }
const data: CreatureInspectJson = JSON.parse(jsonStr);  // inspector.ts:148
renderInspector(data);                                  // inspector.ts:149
```

`creature_inspect_json` on the Rust side (`wasm_api.rs:243–280`) builds a `serde_json::json!({...})` with ~20 fields and serializes it to a `String`. This is returned across the wasm boundary as a JS string copy. Then `JSON.parse` deserializes it back into a JS object. Then 15 DOM `getElementById` + `textContent` writes happen.

At 60 fps with an inspector open, this is **60 JSON serialize → cross-boundary copy → JSON.parse → 15 DOM writes per second**. The JSON payload is small (~300–500 bytes), so `JSON.parse` itself is fast, but the serde serialization on the Rust side and the wasm-to-JS string copy are not negligible.

**Fix:** throttle `refreshInspector` to ~5 Hz (every 200 ms). The inspector is a debugging panel, not a real-time HUD. A 200 ms update lag is imperceptible for field values like energy, age, and action. Estimated saving: 0.3–1 ms/frame when inspector is open.

Also, the id-scan at `inspector.ts:123–128`:

```ts
let foundIdx = -1;
for (let k = 0; k < idsBuffer.length; k++) {
    if (idsBuffer[k] === creatureId) { foundIdx = k; break; }
}
```

This is O(N) on the `ids` buffer every frame when an inspector is open. At 1500 creatures, this is 1500 comparisons per frame in JS. Throttling the inspector to 5 Hz eliminates 55 of those 60 scans per second. The scan itself is cache-warm (typed array access) so the cost is low but nonzero.

---

## 7. Camera-to-Canvas Math — `worldToScreen` Tuple Allocation

**File:** `web/src/render.ts:42–46`

```ts
export function worldToScreen(...): [number, number] {
    const sx = ...;
    const sy = ...;
    return [sx, sy];
}
```

`worldToScreen` returns a new two-element array on every call. It is called:
- Once per creature in `drawCreatures` (line 144) — 1500 calls/frame
- Once per sun cell in `drawSunMap` (line 75) — 400 calls/frame
- Once per carrion entity in `drawCarrion` (line 98)
- Twice in `drawAquariumFrame` (lines 201, 202)

Total: ~1900+ array allocations per frame, all becoming garbage. V8/SpiderMonkey will optimize many of these away via escape analysis (arrays that don't escape a function's scope can be stack-allocated), but across function call boundaries the optimization is less reliable.

**Fix:** convert callers to pass output parameters by reference (an `out: { sx: number; sy: number }` struct reused across calls), or inline the math at each call site. Inlining is the highest-confidence fix — the math is two multiplications and two additions, trivially inlinable. Estimated saving: modest GC pressure reduction; hard to quantify without profiling.

---

## 8. Off-Screen Creature Culling

**File:** `web/src/render.ts:127–191` (drawCreatures loop)

The `drawCreatures` loop processes all `data.length / stride` creatures regardless of whether they are visible on screen. At zoom level 4 (the startup default), the world is 600 units wide and the viewport at typical 1440px is ~360 world-units wide — roughly 60% of creatures are in view. At zoom 16×, only ~(1/4)^2 = 6.25% of the world is visible.

No visibility check exists in the current code. Adding a fast bounding-box reject before the `beginPath`/`arc`/`fill` sequence:

```ts
if (sx + radiusPx < 0 || sx - radiusPx > viewW ||
    sy + radiusPx < 0 || sy - radiusPx > viewH) continue;
```

This check is 4 comparisons and runs before the 3+ canvas API calls. At zoom 16×, this would cull ~94% of all creature draw calls, saving roughly 94% of the per-creature path operation cost. Even at zoom 4×, ~40% of creatures could be culled. Estimated saving: 1–4 ms/frame at high zoom.

The `worldToScreen` call still needs to happen before the check (to compute `sx`/`sy`), but that is cheaper than `beginPath`/`arc`/`fill`/`stroke`.

---

## 9. Typed-Array View Lifetime Hazard (Safety Finding)

**File:** `src/wasm_api.rs:132, 145, 151, 164, 213`

All five buffer functions use `unsafe { js_sys::Float32Array::view(...) }` or `js_sys::Float64Array::view(...)`. As documented in the wasm-bindgen docs, these views are *directly backed by wasm linear memory*. They become silently invalid if any wasm allocation causes the underlying `WebAssembly.Memory` to grow.

The critical call sequence in `renderWorld` (`render.ts:208–225`):

```ts
// render.ts:221
drawSunMap(..., world.sun_buffer(), world.sun_capacity_buffer());
// render.ts:223
drawCarrion(..., world.carrion_buffer());
// render.ts:224
drawCreatures(..., world.creatures_buffer(), ids, ...);
```

`world.creatures_buffer()` is called last. If that call causes wasm memory to grow (because `creature_buf` needs to expand), then `ids` (obtained earlier at `main.ts:269`) could be invalidated. More concretely: `sun_buffer()` calls `self.sun_buf.resize(n, 0.0)` — if `sun_buf` needs to grow, wasm memory grows, invalidating the `ids` Float64Array.

**Ordering in the current code (`main.ts:269–274`):**
1. `ids = world.creature_ids_buffer()` — fetches id view
2. `pollRail(rail, world, ids)` — passes ids to inspector (reads ids buffer)
3. `renderWorld(...)` — calls `sun_buffer()`, `creatures_buffer()`, etc.

If `sun_buffer()` inside `renderWorld` triggers a wasm memory grow, the `ids` view that was already passed to `renderWorld` and the `ids` view used in `pollRail` could both be stale. The inspector reads from `ids` in `refreshInspector` (called from `pollRail`, which happens *before* `renderWorld`), so that specific use is safe. But `renderWorld` passes `ids` to `drawCreatures` at line 224, and inside `drawCreatures`, calls to `sun_buffer()` etc. have already happened before `drawCreatures` runs — so the order is:

1. `sun_buffer()` call (potentially grows wasm mem) — render.ts:221
2. `sun_capacity_buffer()` call — render.ts:221
3. `carrion_buffer()` call — render.ts:223
4. `creatures_buffer()` call — render.ts:224 ← if this grows mem, `ids` is now stale
5. Inside `drawCreatures`, `ids` is read — render.ts:119+

Step 4 growing memory would invalidate the `ids` view before step 5 uses it. In practice, `creature_buf` grows monotonically to the population high-water mark and then stabilizes, so this hazard only triggers when population is growing and `creature_buf` needs expansion — which is exactly the most interesting phase of a sim run.

**Recommended fix:** Copy `ids` immediately after obtaining it with `Float64Array.prototype.slice()`, or re-fetch it as the last buffer call right before `drawCreatures`.

---

## 10. Render-Path Bypass on Hidden Tabs

**File:** `web/src/main.ts:247–280`

No `document.hidden` check exists. The RAF fires at browser-throttled rate (~1 Hz) when the tab is hidden. `step_n` still runs, which is correct (sim keeps running). `renderWorld` also runs at 1 Hz when hidden — this is wasteful but the cost at 1 Hz is negligible.

No action required. If sim-only mode (skip render when hidden) were desired, add `if (document.hidden) { requestAnimationFrame(frame); return; }` after `step_n`. The architecture doc states "sim runs only while tab is open," which the current behavior satisfies (RAF fires only while the tab is attached to a browsing context).

---

## Top 3 Render-Side Wins

**1. Off-screen creature culling (section 8) + batch-by-color body layer (section 2).**
These two changes together address the largest measured cost: ~1500–9000 canvas API calls per frame for creatures. A four-line viewport test before `beginPath`/`arc`/`fill` eliminates all draw work for off-screen creatures with no visual change. Grouping bodies by color class then cuts the remaining in-view draw calls further. Together these can plausibly save 2–6 ms/frame — the difference between a 60 fps and a 30 fps cap at high population.

**2. Inspector throttle to 5 Hz (section 6) + stats chart wall-clock gate (section 5).**
Both are trivially safe throttles with zero functional impact. The inspector change eliminates a serde serialization + wasm-JS string copy + JSON.parse + 15 DOM writes happening 55 out of 60 times per second. The chart gate avoids two 500-point canvas line chart redraws running at 60 Hz when the user has speed set to 100×. Together: ~0.5–1.5 ms/frame and a meaningful reduction in GC pressure.

**3. drawSunMap → putImageData (section 3).**
Replacing 400 `fillStyle = rgb(...)` string allocations + 400 `fillRect` calls with a single `putImageData` is a medium-effort refactor (write to an `Uint8ClampedArray` per frame, call `putImageData` once, then stretch via `drawImage` or `ctx.scale` + `ctx.imageSmoothingEnabled = false`). The saving — 0.5–1.5 ms/frame — is modest but consistent across all zoom levels and represents the dominant cost in the background layer. The only tradeoff is losing DPR-aware sub-pixel antialiasing on cell edges, which is not meaningful for a 20×20 ecological heatmap.
