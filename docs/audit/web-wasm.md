# Web / Wasm Audit — `web/` + `src/wasm_api.rs`

Scope: TypeScript frontend, the wasm-bindgen bridge, and queued UI plan docs
(`docs/plans/ui-*.md`). Findings are filed by area with file:line refs and
concrete fixes.

---

## 1. Performance — wasm boundary, per-frame work

### 1.1 `creature_ids_buffer` rebuilt every frame (hot)
**Files.** `src/wasm_api.rs:208-214`, `web/src/main.ts:295`.

The id buffer is `clear() + push_in_loop` every RAF tick, doing N float
conversions even when the population is identical to last frame. Fix:
mirror the pattern used for `creature_buf` — `resize(n)` then write by index.
Better, expose the raw `creatures.id: Vec<u64>` as a `Float64Array::view` over
a permanently-allocated `Vec<f64>` that is kept in lock-step with the SoA
inside `World` (write on push/swap_remove), so `creature_ids_buffer()` is
O(0). Same fix unlocks zero-copy id lookups for inspector + highlights.

### 1.2 `creatures_buffer` re-walks `genomes[i]` (AoS read)
**File.** `src/wasm_api.rs:108-131`.

After perf-5-genome-soa (already landed per recent commits) the renderer hot
fields are in a SoA mirror, but `creatures_buffer` still reads `&self.inner.creatures.genomes[i]` —
the unmirrored AoS — for size/pigment/flags. This re-introduces the cache
miss the SoA split was supposed to remove. Switch the per-creature pack
loop to the mirrored scalars (`creatures.size[i]`, `creatures.pigment_r[i]`,
flag arrays).

### 1.3 `creature_at` is O(N) linear scan from JS
**File.** `src/wasm_api.rs:226-238`.

Click only, so cost is small, but the spatial grid is already built. Use
`SpatialGrid::query_radius` to bound this to local cells; pays off when
population is high. Trivial change; protects 100x speed users from
multi-ms freezes on click.

### 1.4 `species_list_json` allocates a `HashMap` every poll
**File.** `src/wasm_api.rs:397-421`.

Called 1Hz (`web/src/rail/index.ts:100`). Replace with a `Vec<u32>`
counts buffer sized to `species.list.len()` (linear init + linear count).
For 100s of species the HashMap allocation isn't free.

### 1.5 Two JSON.parse calls per frame in profiler poll
**File.** `web/src/rail/stats.ts:173,179`.

When profiler is on, every second we round-trip both Rust *and* TS reports
through `JSON.parse`. The TS profiler builds the report as a string just to
re-parse it locally. Refactor `web/src/perf.ts` to expose a `report():
ProfNode` returning the JS object directly; keep `reportJson()` only for the
Rust-shape compat path. Removes ~5KB string churn/sec and a needless parse.

### 1.6 `recordSample` uses `Array.shift()` (O(N) ring buffer)
**File.** `web/src/perf.ts:118-124,126-133`.

`shift()` on a 4096-cap array is O(n). The Rust profiler uses a real ring
buffer; the TS mirror should too. Use a head-index ring (`samples` as
fixed-size two-array `(ts[], dur[])` or `{head, len, cap, ts:Float64Array, dur:Float64Array}`).
Same fix needed for `samples.shift()` in `web/src/rail/stats.ts:38`.

### 1.7 Inspector ID lookup is linear scan per frame
**File.** `web/src/rail/inspector.ts:122-128`.

`refreshInspector` walks the entire ids buffer every frame to find the
selected creature. With N=~1000 creatures that's 60k cmps/sec. Cache the
last known SoA index and check it first; fall back to linear only on miss.
Or build a `Map<id, idx>` once per frame in `pollRail` and pass it in.

### 1.8 `creature_inspect_json` per frame (selected case)
**File.** `web/src/rail/inspector.ts:143`, `src/wasm_api.rs:243-280`.

Each frame allocates a fresh `serde_json::Value`, stringifies it, ships
across boundary, and JS `JSON.parse`s. The plan doc
`docs/plans/ui-inspector-box.md` §6 acknowledges this. Two cheaper paths:

- Drop the cadence to ~5 Hz (set a `lastInspectorPoll` like the species
  list does). The inspector values don't visibly change per frame.
- Or expose a typed wasm getter — return a `Float32Array` view of the
  inspect fields plus separate string getters for `species_name`,
  `current_action` only when they change.

### 1.9 `sun_buffer`/`sun_capacity_buffer` per frame even when sun is static
**Files.** `src/wasm_api.rs:135-152`, `web/src/render.ts:221`.

`sun_buffer` recomputes `current/capacity` for every cell every frame.
`sun_capacity_buffer` is a `view()` (cheap) but `drawSunMap` then walks the
full SUN_DIM² grid (`web/src/render.ts:64-83`) regardless of viewport. At
zoom 8× most cells are off-screen. Two fixes:
- Frustum-cull cells in `drawSunMap` to the visible world rect, big win.
- Optionally only repack `sun_buf` when `sun.current` changed since last
  call (track a tick stamp inside `Sun`).

### 1.10 `drawCreatures` highlight scan rebuilt every frame
**File.** `web/src/render.ts:117-125`.

`highlightIdx` Set is rebuilt every frame from the full ids buffer even when
no highlights exist. The early-return `highlightMap.size > 0` is good, but
when one creature is selected (permanent highlight) it still walks all
ids/frame. Lift the loop: build the set inside `pollRail` once when
highlights mutate, not in the render path.

### 1.11 `drawCreatures` per-creature alloc of `fillStyle` string
**File.** `web/src/render.ts:148-149`.

`ctx.fillStyle = `rgb(${cr},${cg},${cb})`` allocates a string per creature.
At N=1000 that's 60k string allocs/sec. Two options:
- Batch creatures by pigment-quantized bucket (e.g. 5-bit RGB) and draw in
  one fill per bucket with `Path2D`.
- Or precompute a `creatureColorStrings: string[]` mirror on the wasm side
  once per palette change (rare) — but quantizing on the JS side is simpler.

### 1.12 `ctx.beginPath/arc/fill` per creature
**File.** `web/src/render.ts:150-152,164-168,186-188`.

Each creature does up to 7 separate `beginPath/arc/stroke` ops (body + 5
rings + highlight). Coalesce into a single `Path2D` per color and one
`fill`/`stroke` per color. With v1 ring colors (5) total ops drop from
~7N to ~6 fixed. This is probably the biggest non-wasm render win
available.

### 1.13 `populateCell` clobbers existing caption
**File.** `web/src/eulogy.ts:122-134`.

`captionEl.textContent = `${captionEl.textContent} · ${...}`` chains the
existing literal (e.g. "First to move") with metadata — fine when shown
once, but if `showEulogyCard` ever gets re-entered the captions double up.
Use a data attribute or rebuild from a constant.

### 1.14 Speed multiplier accounts ticks against wall delta but no cap
**File.** `web/src/main.ts:276-281`.

`ticksThisFrame = min(200, max(1, round(speed*delta/16.66)))`. On a tab
that backgrounded for 5s at 100x speed the user comes back to 100×300/16.66
≈ 1800 → clamped to 200. Fine for explosion-control but means a foregrounded
tab can never "catch up" the 5s of lost sim. That's intentional but worth
documenting. Bigger concern: `delta` is unbounded the first frame after a
tab return — clamp it to e.g. 100ms to avoid the catch-up freeze.

---

## 2. Memory — leaks, retention

### 2.1 `populateCell` portrait canvases never released
**Files.** `web/src/eulogy.ts:130-131`, `web/src/render.ts:230-275`.

Each call creates a new `<canvas>` via `document.createElement`. The
backdrop is added to `document.body` but never removed once the user clicks
"New World" (the code does `window.location.href = ...` which navigates
away, so it's not a real leak — but if a future change ever shows the card
without a navigation, the four 96×96 canvases stay forever). Add a
`backdrop.remove()` to the New World handler before the navigate.

### 2.2 Inspector tab button retained after `clearSelection`
**File.** `web/src/rail/inspector.ts:101-103`.

`inspectorTab.classList.add("hidden")` keeps the node + listener; fine. But
`ensureInspectorTab` re-creates the button on rail re-entry if the cache
miss path is triggered (it isn't currently). Add a single-shot guard or
move the tab creation into `installRail`.

### 2.3 Worker message handler swap pattern leaks pending handlers
**File.** `web/src/persistence/index.ts:60-71,78-89`.

`loadCurrent`/`deleteCurrent` swap `worker.onmessage` for the duration of
the in-flight request, then restore. If two `loadCurrent` calls overlap (or
a `save()` "saved" message arrives during a load), the saved-handler is
silently discarded for that ack: the `saved` event never fires `onSavedCb`.
Replace with a request-id model (postMessage carries `reqId`; client routes
on `reqId` from a `Map<reqId, resolve>`). Or simply queue load/delete
operations so they cannot overlap.

### 2.4 IDB worker holds connection forever
**File.** `web/src/persistence/worker.ts:5-21`.

`dbPromise` is a module-scoped singleton; the IDBDatabase handle is never
closed. Fine for the lifetime of the page (worker dies with the tab) but
note that during a `versionchange` (a different tab upgrades the schema) we
won't release. Add `db.onversionchange = () => { db.close(); dbPromise = null; }`.

### 2.5 `eulogyShown` is module-scoped — survives reload? No, but…
**File.** `web/src/main.ts:103`.

Fine in practice (page navigates on "New World"), but if the user ever
restarts a world in-place this guard stays true. If ui-inspector-box.md or
a future plan adds in-place restart, reset to false there.

### 2.6 `speciesNameCache` cleared+repopulated 1Hz
**File.** `web/src/rail/index.ts:73,103-104`.

Currently cache is unused (events tab hidden) but still rebuilt every 1s.
Either gate behind an `eventsEnabled` flag or drop the clear and only
write-through on new species.

---

## 3. Type-safety holes

### 3.1 `as unknown as Float64Array` on wasm getters
**Files.** `web/src/main.ts:295`, `web/src/rail/stats.ts:33`.

The `.d.ts` (`web/wasm/evosim.d.ts`) declares these returns as
`Float64Array` already — these double-casts mask a real fix. Either:
- Fix `wasm_api.rs` to return `js_sys::Float64Array` (already done) and let
  the generated `.d.ts` types flow, or
- Update the import to use the typed signature directly. Test what
  `creature_ids_buffer()` returns in the `.d.ts` and drop the cast.

### 3.2 `as unknown as Worker` in the worker file
**File.** `web/src/persistence/worker.ts:53,60,63,70,...`.

Inside a worker module, `self` is `DedicatedWorkerGlobalScope`, which has
`postMessage` directly. Declare `const ctx = self as unknown as DedicatedWorkerGlobalScope;`
once at the top (or add `/// <reference lib="webworker" />` so TS picks the
right lib) — currently every call is a double-cast.

### 3.3 Event messages typed as untyped objects
**File.** `web/src/persistence/index.ts:33-37,62,79`.

`m = ev.data as { type: string; ... }` — define a `WorkerResponse` discriminated
union and use a switch on `type` for type-narrowing. Same on the worker side.

### 3.4 `escapeHtml` duplicated across files
**Files.** `web/src/eulogy.ts:161-167`, `web/src/persistence/ui.ts:63-69`,
`web/src/rail/stats.ts:268-270` (named `escHtml`, slightly different).

Three copies, two with `&quot;` and one without. Centralize in `web/src/util/escape.ts`.

### 3.5 `(rail as { activeTab?: string }).activeTab`
**File.** `web/src/rail/inspector.ts:104`.

`RailState` already declares `activeTab: string` (web/src/rail/index.ts:35),
so this cast is unnecessary and obscures intent.

### 3.6 `populateCell(... entry: HoFEntry | null)` with `entry === null` branch
**File.** `web/src/eulogy.ts:122-129`.

Sets `captionEl.textContent` by concatenating with `captionEl.textContent` —
returns void after writing, even though the function looks like it might
short-circuit. Clearer with an early return + dedicated empty styling.

### 3.7 `as HTMLButtonElement` casts without runtime check
**File.** `web/src/main.ts:109,145`, throughout the rail code.

If the IDs ever drift these silently produce nulls and the next access
throws. The `installSeedDisplay` path null-checks correctly; others (line
145 for save-indicator) do — but `flashButton` (eulogy:151) doesn't. Minor;
make sure ui-stats / ui-perf / ui-inspector plans keep this.

---

## 4. Rendering bottlenecks

(Many overlap §1; quick recap of the visual-only items.)

### 4.1 DPR-aware canvas but no DPR cap
**File.** `web/src/main.ts:21`.

`window.devicePixelRatio || 1` is unbounded — on a 4K phone (DPR=3) the
canvas is 3× the pixel count of CSS, tripling fill cost. Cap at 2 for the
aquarium; portrait canvases (eulogy) are fine at native DPR.

### 4.2 Sun map drawn at the *world* tile size, no LOD
**File.** `web/src/render.ts:64-83`.

At zoom < 1 each cell is sub-pixel; at zoom > 4 each cell is huge. No LOD
or merging. Could be a `createImageData(SUN_DIM, SUN_DIM)` filled once and
drawn scaled — single draw call instead of `SUN_DIM²` fillRect ops. For
SUN_DIM=32 (1024 cells) that's a small win; for larger SUN_DIM it's
massive.

### 4.3 No render frustum cull on creatures
**File.** `web/src/render.ts:127-191`.

Even if a creature is off-screen the full ring stack is drawn. Skip when
`(sx,sy)` ± `radiusPx` is outside `(0,0,viewW,viewH)`. ~free.

---

## 5. Accessibility

### 5.1 Canvas has no `role="img"` or label
**File.** `web/index.html:11`.

Screen readers see "canvas". Add `role="application"` (or `role="img"`
with a dynamic `aria-label` summarizing pop/species) and keyboard focus
support so the click handler has a non-pointer equivalent.

### 5.2 No keyboard equivalents for pan/zoom/select
**File.** `web/src/camera.ts:*`, `web/src/rail/inspector.ts:175-210`.

Pointer-only camera + tap-to-inspect. Add arrow-key pan, `+`/`-` zoom, and
Tab-cycle through visible creatures with Enter to select. Critical for
keyboard-only users and useful for power users.

### 5.3 Speed buttons created without aria
**File.** `web/src/main.ts:313-334`.

The speed buttons are programmatically built; they have no
`aria-pressed` state. The currently-active speed should set
`aria-pressed="true"` and the others `"false"`.

### 5.4 Modals are not focus-trapped, not `role="dialog"`
**Files.** `web/src/persistence/ui.ts:8-36,39-61`, `web/src/eulogy.ts:43-120`.

Resume / schema-mismatch / eulogy backdrops:
- No `role="dialog"`, no `aria-modal="true"`, no `aria-labelledby`.
- No focus management — focus stays where it was, Esc doesn't close, Tab
  can exit to the canvas underneath.
- No focus restoration on close.

These are showstoppers for any a11y review. Fix when ui-stats/etc land.

### 5.5 Close-on-overlay-click missing
**Files.** `web/src/persistence/ui.ts`, `web/src/eulogy.ts`.

Clicking the dimmed backdrop does nothing — only data-action buttons close
the modal. Common a11y pattern is "click outside = cancel".

### 5.6 No prefers-reduced-motion respect
**File.** `web/src/styles.css:111-133,249-257`.

Toast slide-in / status-toast fade animations don't honor
`@media (prefers-reduced-motion: reduce)`. Wrap them.

### 5.7 `tabindex` and tab order
The tab list in `#rail-tabs` is `role="tablist"` but the buttons aren't
`role="tab"`, the panels aren't `role="tabpanel"`, and arrow-key navigation
between tabs is missing. Hidden tab (when Inspector pop-up plan lands)
should be `aria-hidden`.

---

## 6. Bundle bloat / build

### 6.1 No minification verification, sourcemaps shipped
**File.** `web/vite.config.ts:25-30`.

`sourcemap: true` → 147KB sourcemap for the 36KB JS chunk gets shipped in
the production `dist/`. For Cloudflare Pages this is bandwidth + cache
churn. Toggle off in prod, or move to `sourcemap: 'hidden'` so the file is
emitted but not referenced.

### 6.2 `serde_json` for event/species/inspector/HoF/profile
**Files.** `src/wasm_api.rs:187,201,243,299,349,372`.

`serde_json` is hot in wasm code paths and contributes nontrivially to the
431KB wasm (~30% historically). Three of these (`recent_events_json`,
`species_list_json`, `creature_inspect_json`) are tiny payloads that could
be hand-formatted as strings or returned as typed JS objects via
`#[wasm_bindgen(getter)]` on dedicated structs (`#[wasm_bindgen]` over
`SpeciesRow` exposes fields without serde). Worth a measurement to see if
removing serde_json from wasm shrinks the bundle.

### 6.3 `wee_alloc` / panic strategy
No `Cargo.toml` shown; verify `lto = true`, `codegen-units = 1`,
`opt-level = "z"` or `"s"`, and `panic = "abort"` in the wasm profile. The
431KB suggests defaults — should be < 250KB with these.

### 6.4 No HTTP compression check
Vite's default is no compression; on Cloudflare Pages brotli is automatic
but local `preview` doesn't compress. Add a `vite-plugin-compression`
preview step to size-check.

### 6.5 No code-splitting beyond worker
The persistence worker is split (good). The eulogy card and modal UI are
in the main bundle even though they're shown 0–1 times per session.
Dynamic-import them: `import("./eulogy").then(m => m.showEulogyCard(...))`.

---

## 7. Error handling gaps

### 7.1 `console.warn` swallows save failures silently
**File.** `web/src/main.ts:117,135,210`, `web/src/rail/index.ts:85,106`.

User has no UI signal when species_list fails or initThreadPool fails.
The autosave-failure toast pattern (`showAutosaveFailureToast`) is the
right model. Add a generic non-blocking error toast for these.

### 7.2 `WorldHandle.fromJson` errors swallowed
**File.** `web/src/main.ts:195-204`.

Catches any non-`schema-mismatch:` error and treats it as "schema mismatch"
which mis-labels `invalid-json:` and `structural:` failures. Add distinct
copy or at least log the original tag.

### 7.3 `pollRail` swallows `JSON.parse` on every frame
**File.** `web/src/rail/index.ts:104-106`.

If the species JSON ever malformeds, we warn 60×/sec. Add a backoff or
silence after first warn per minute.

### 7.4 `frame` callback exceptions stop the loop
**File.** `web/src/main.ts:271-309`.

If `step_n`, `pollRail`, or `renderWorld` throws, `requestAnimationFrame(frame)`
at line 308 never runs (it's after the try/finally but a re-throw stops it).
Wrap the whole body in try/catch and always reschedule with an error toast.

### 7.5 `creature_at` returns `Option<u32>` but TS checks both `undefined` and `null`
**File.** `web/src/rail/inspector.ts:201`.

wasm-bindgen Option<u32> becomes `number | undefined`. The `|| null` check
is dead code; could be `if (idx == null)` if both null and undefined are
expected, but as written it's clearer to drop the null branch.

### 7.6 `beforeunload` save uses `world.world_ended` check but world may already be null
**File.** `web/src/main.ts:227-235`.

If a load failed and re-entered the boot path, `world` is rebound before
this handler attaches; but if the user navigates *during* the prompt the
listener attaches with the wrong world (the inner closure captures
the let). Move attachment to *after* successful world bind (which it
mostly is — but the `if (world)` guard is still defensive in the wrong
direction; should be `world && !world.world_ended`).

### 7.7 `from_json` panics? Mostly fine, but `World::from_save_v1` allocations may fail
**File.** `src/wasm_api.rs:298-330`.

`snapshot_json` uses `.expect("save serialization is infallible")`. Under
OOM in wasm32 that aborts the world. Document it; or return `Result`.

### 7.8 `triggerDownload` race
**File.** `web/src/eulogy.ts:136-148`.

`URL.revokeObjectURL(url)` is on `setTimeout(..., 0)` — Firefox and Safari
have been known to cancel the download if the URL is revoked before the
download starts. Bump to a small delay (e.g. 1000ms) or revoke on `a.click`
plus visibility-change.

---

## 8. Cross-cutting / wasm-bindgen surface

### 8.1 Every JSON return string allocates a Rust String
**Files.** `src/wasm_api.rs:187,201,243,299,349,372`.

Each call shifts a UTF-8 string across the boundary. wasm-bindgen does a
copy. For the high-cadence ones (inspector at frame rate, profile at 1Hz)
prefer typed structs marshaled with `#[wasm_bindgen]` getters. Master plan
already calls this out via SoA work for the creature buffer; same pattern
applies here.

### 8.2 `set_slider` unknown-name silently ignored
**File.** `src/wasm_api.rs:181`.

Return `Result<(), JsValue>` so UI can warn on typo.

### 8.3 No `Drop` for `WorldHandle`
The handle is JS-owned and freed via wasm-bindgen, but if a dev forgets
`world.free()` the rayon thread pool keeps running (post perf-4). Document
explicitly in `evosim.d.ts` header that long-lived handles must be freed
before constructing a new one.

### 8.4 `stats_sample` returns `Box<[f32]>` (heap alloc per call)
**File.** `src/wasm_api.rs:285-291`.

Called ~6 Hz. Each call allocates a new Vec on the wasm heap and copies
out. Convert to a reusable `Vec<f32>` field on the handle (same pattern
as creature_buf) and return a `Float32Array::view`.

---

## 9. UI plan gaps (`docs/plans/ui-*.md`)

The three plans (ui-stats, ui-perf, ui-inspector) collectively move
the right-rail content into free-standing overlays. After auditing them
plus `perf+ui-cross-review.md`, the *known* gaps:

### 9.1 No plan for §1.5 / §1.10 perf-during-render fixes
The plans address `installProfilerPanel` location and table truncation,
but not the per-frame JSON parses and highlight rebuilds. Worth a
follow-up `ui-render-batch.md` covering §1.10–§1.12 (path-2D batching).

### 9.2 No accessibility audit anywhere
None of the three plans mention ARIA, focus management, keyboard nav, or
prefers-reduced-motion. Given the canvas-only world this is the biggest
*queued* gap — the new overlay widgets will inherit the same a11y holes
as the current rail (§5).

### 9.3 ui-inspector retains per-frame inspector poll
`ui-inspector-box.md` §6 documents the per-frame `creature_inspect_json`
poll as a known cost. Plan should at minimum dial it to 5Hz (matches
inspector text-update perception), saving 80% of the wasm-string traffic
for selected creatures. Trivially compatible with all other plans.

### 9.4 ui-perf — no chart-render backpressure
The profiler table re-builds via `innerHTML = rows.join("")` (rail/stats.ts:265).
ui-perf's redesign retains this. For a 30-row table it's fine; but if
nested nodes ever grow, switch to a single DOM diff. Mention in the plan.

### 9.5 ui-stats — no perf-timing integration mentioned for chart sampling
`maybeSampleStats` (rail/stats.ts:29) does a wasm call every frame to check
the tick. After the rail is hidden it still runs. The plan should call out
that ui-stats inherits this; better: drive sampling off the existing
`world.tick` check that main already does (pass tick into pollRail rather
than re-reading via getter).

### 9.6 No plan for canvas DPR cap or frustum cull (§4.1, §4.3)
Not covered by any current plan. Cheap to add to ui-stats or a small perf
follow-up.

### 9.7 No `ui-` plan covers seed-pill / share / save-indicator a11y
After ui-stats hides the rail, the only non-overlay UI is the top-bar. It
gets no a11y attention in the current plans.

### 9.8 Master plan §6 R8 z-order vs. modal z-index
Modal backdrops are z-index:30 (`styles.css:191`), the rail is 5
(`styles.css:62`). New overlay widgets must be > 5 and < 20 (autosave
toast). All three plans agree on this but it isn't centralized — single
CSS-custom-property `--z-overlay: 10` would prevent drift.

---

## Top 10 quick wins (rough impact-effort)

1. §1.2 `creatures_buffer` — read from SoA mirror, not `genomes[i]`. Code change ~10 lines.
2. §1.12 Coalesce creature draws into Path2Ds per color. Biggest render win.
3. §1.10 Lift highlight-set build out of render. ~free, removes O(N) per frame.
4. §1.1 Eliminate `creature_ids_buffer` clear/push (keep f64 mirror).
5. §4.1 Cap DPR at 2 for `#aquarium`. One line.
6. §4.3 Frustum cull creatures (and §1.9 sun cells).
7. §1.6 Replace `Array.shift()` ring with index ring in `perf.ts` + `stats.ts`.
8. §3.1 / §3.4 / §3.5 — small TS cleanups, prevents future drift.
9. §5.4 + §5.5 Modal a11y (role, focus trap, click-outside).
10. §6.1 Drop sourcemaps in prod build (`sourcemap: 'hidden'`).
