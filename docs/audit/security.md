# Security audit — evosim

Scope: Rust sim (`src/`), wasm boundary (`src/wasm_api.rs`), TS web shell (`web/src/`),
JS/Rust deps. Threat model: malicious save file (`fromJson`) and malicious
URL params (`?seed=`). The app has no server, no auth, no PII; the practical
risk surface is (a) DoS via wasm panic and (b) reflected/persisted XSS if
attacker-controlled content reaches a DOM sink.

---

## High

### H1. Save-load: structural fields used as array indices without validation → wasm panic (DoS)
`World::from_save_v1` accepts `SaveV1` deserialized from arbitrary JSON
(`wasm_api.rs:313`) and only validates SoA column lengths and per-brain
weight count. Several other fields flow straight into indexing operations
that panic on out-of-range input. The wasm build is `panic = "abort"`
(`Cargo.toml:55`), so any reachable panic kills the runtime — a crafted
save = guaranteed crash.

Concrete unchecked paths:

- `src/world.rs:1108` — `SunMap { capacity: save.sun.capacity, current: save.sun.current, demand: save.sun.demand, … }`. These are taken verbatim. The hot loop `for k in 0..self.sun.current.len() { … self.sun.demand[k] … self.sun.capacity[k] … }` (`src/world.rs:627-635`) panics if the three vectors differ in length. `SunMap::recompute_capacity` (`src/sun.rs:65-77`) indexes `[0 .. SUN_DIM*SUN_DIM)` and panics if `capacity.len() < 400`. Trigger: `set_slider("sun_gradient_strength", …)` (`wasm_api.rs:175-178`) after loading a save with a short `capacity`.
- `src/world.rs:1131` — `carrion: save.carrion`. Each `Carrion.sun_cell: usize` (`src/carrion.rs:14`) is later indexed into `self.sun.demand` / `self.sun.current` (e.g. via the carrion decay/return-to-sun path around `src/world.rs:915-920`). A `sun_cell` ≥ `sun.current.len()` panics on the next tick.
- `src/world.rs:1119` — `SpeciesRegistry::from_snapshot(save.species.list, max_id + 1)`. `Species::get(id)` (`src/species.rs:89-91`) does `&self.list[id as usize]`. Creature `species_id` columns are deserialized blindly (`creatures.species_id`); any `species_id ≥ list.len()` panics on the very next tick via `self.species.get(self.creatures.species_id[i])` at `src/world.rs:480`, `:811`, `:854`, `:960`, and `:249` (`wasm_api.rs::creature_inspect_json`).
- `src/world.rs:1118` — `max_id = save.species.list.iter().map(|s| s.id).max()`. `next_id = max_id + 1` will overflow if a save sets `id = u32::MAX` (debug-mode panic; release wraps). Speciation later writes `next_id` as a new id, possibly colliding with an existing entry.
- `src/world.rs:1098` — `SpatialGrid::rebuild(&creatures.x, &creatures.y)`. Positions are deserialized as raw `f32` and not clamped. NaN/Inf coordinates likely propagate into `cell_index_for` and `as usize` casts, with downstream OOB indexing of the grid bucket vec.
- `src/world.rs:1064` — `CreatureSoA::with_capacity(n.max(16))`. `n` is bounded only by the JSON parser (`serde_json` reads to memory first); a single multi-GB JSON would OOM-abort the wasm instance before reaching this line. Add a hard cap (e.g. 100k creatures, file size cap on JS side).

Fix sketch: `from_save_v1` should verify
`sun.capacity.len() == SUN_DIM*SUN_DIM == sun.current.len() == sun.demand.len()`,
that every `species.list[i].id == i as u32` (or rebuild a sparse map), that
all `species_id` and `parent_species_id` columns reference an in-range species,
that `x`/`y` are finite and within `[0, WORLD_SIZE)`, and that
`carrion[k].sun_cell < SUN_DIM*SUN_DIM`. All failures should return
`LoadError::StructuralError`, which JS already handles as "schema mismatch
modal → start fresh" (`web/src/main.ts:197-204`).

Impact: any attacker who can get a user to load a crafted save (file share,
URL-pasted JSON, future cloud-sync) crashes their tab. Persisted via IndexedDB
autosave envelope — a tampered IndexedDB row will crash the tab on every
reload until the user clears storage.

---

## Medium

### M1. `js_sys::*Array::view` returned across the wasm boundary
`src/wasm_api.rs:132`, `:145`, `:151`, `:166`, `:213` — five places return
`Float32Array::view(&self.creature_buf)` (etc.) directly to JS. These views
alias wasm linear memory. Per `wasm-bindgen` docs the view becomes
**undefined behavior** if wasm memory is grown or the backing `Vec` is
reallocated while JS still holds the view.

In practice the current call sites are safe because render.ts consumes the
view synchronously within one frame and the only wasm calls in between are
read-only getters (`web/src/render.ts:221-224`, `web/src/main.ts:295`). But
the contract is unenforced — a future maintainer who interleaves
`world.step()` between `world.creatures_buffer()` and the draw call will
silently corrupt rendering or read freed memory.

Mitigation: either (a) copy the view into a JS-owned `Float32Array` at the
call site (`new Float32Array(world.creatures_buffer())`) for the small
buffers, or (b) document the same-frame contract at each export with a
`// SAFETY` comment and assert it in dev builds.

### M2. esbuild ≤ 0.24.x dev-server CORS bypass (GHSA-67mh-4wv8-2f99)
`web/pnpm-lock.yaml:299` pins `esbuild@0.21.5` (transitive via `vite@5.4.21`).
The dev server accepts cross-origin requests, allowing any website the
developer visits to read source/sourcemaps and the in-progress build.
Fixed in esbuild 0.25.0; requires Vite 6.x. **Dev-only**, not shipped to
users — still worth bumping during a Vite major upgrade.

### M3. No Content-Security-Policy
`web/index.html` has no CSP `meta http-equiv` and `web/public/_headers` only
sets COOP/COEP (`web/public/_headers:1-4`). With H1 unresolved, an XSS that
manages to get into the species name path (see M5) would have full
script-execution rights. Add at minimum:

```
Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'
```

(`wasm-unsafe-eval` is required for the wasm-bindgen glue.)

### M4. `unsafe { &*self.profiler }` in `SpanGuard::drop` (`src/profiler.rs:301`)
Documented invariants (single-threaded wasm; lifetime bounded to the brace
block where `span!` was called) hold for the current call sites. However
the safety argument depends entirely on the macro never being used in a way
that lets the guard outlive the `Profiler` borrow — and there is no
compile-time enforcement (the field is `*const Profiler`, not `&'a Profiler`).
With the `threads` feature (`Cargo.toml:23`), `Profiler::new()` callers and
the `RefCell` it owns become reachable from rayon worker threads;
`SpanGuard` is `Send` by default because raw pointers used to be `Send` —
wait, raw pointers are `!Send`, so threading is fine, but the
`!Send/!Sync` claim in the comment relies on that, not on any explicit
`PhantomData<*const ()>` marker. Worth tightening: store a `&'a Profiler`
with a real lifetime, or `PhantomData<*const ()>` and `unsafe impl !Send`
guards, so the soundness argument is local.

### M5. `appendEventRow` builds innerHTML from un-escaped event data (`web/src/rail/events.ts:90`)
```ts
row.innerHTML = `<span class="tick">t=${ev.tick}</span>${formatEvent(ev, speciesNameCache)}`;
```
`formatEvent` interpolates `k.new_species_name`, `k.species_name`,
`parentName` directly into the HTML (`web/src/rail/events.ts:64-83`). These
strings are normally generated by `lineage_suffix` and are safe (ASCII
letters/digits truncated to 16 chars, `src/species.rs:62-78`), BUT under H1
they are deserialized from the user's save file and may contain arbitrary
UTF-8 including `<script>`. Today this function is **dead code** — the
events panel is `display:none` (`web/index.html:31`) and no caller invokes
`appendEventRow` (`web/src/rail/index.ts` only calls `renderSpeciesList`,
which uses `textContent` and is safe). Either delete `appendEventRow` /
`formatEvent` or switch to `textContent` + DOM `createElement` before the
events panel ships in v1.1.

---

## Low

### L1. `expect("save serialization is infallible")` (`src/wasm_api.rs:301`)
Genuinely infallible for the current `SaveV1` shape — all fields are
`Serialize`-clean — but a future `#[serde(serialize_with = …)]` could
introduce a fallible serializer. The other JSON paths (`recent_events_json`,
`hof_json`, `species_list_json`, `creature_inspect_json`) all use
`unwrap_or_else(|_| "…".into())`. Cheap to fix for consistency.

### L2. `seed` query param flows into `WorldHandle::new` without length cap (`web/src/main.ts:215`)
`new URLSearchParams(window.location.search).get("seed")` is passed verbatim
to wasm as the seed string. The seed is hashed (`SimRng::from_string`), and
`World::seed: String` is stored and shown in the HoF eulogy. It is rendered
through `escapeHtml` in `web/src/eulogy.ts:60` and `web/src/persistence/ui.ts:17`
(both safe) and via `textContent` in `web/src/main.ts:108`. So no XSS path.
But a 10 MB URL would still get stored in IndexedDB. Cap to ~128 chars on
the JS side before constructing the world.

### L3. `species_name` rendered through `textContent` in the inspector (`web/src/rail/inspector.ts:35-36`)
Safe today (`set()` uses `textContent` at `:31`). Flagged only because the
DOM `<dt>/<dd>` selectors are filled from a JSON blob whose `species_name`
field is attacker-controlled under H1. Today: no XSS. If anyone refactors
`set()` to `innerHTML`, this becomes a live sink.

### L4. No file-path traversal sink
There is no fs / `path.join` / dynamic `import()` on user-supplied strings
anywhere; saves go through IndexedDB only (`web/src/persistence/worker.ts`).
Filename in `triggerDownload` is built from `hof.seed` and `hof.day_count`
(`web/src/eulogy.ts:102`) — user-controlled but only used as the
`Blob`/`a.download` attribute, which the browser sanitises before writing
to the user's own Downloads folder. No server-side path lookup.

### L5. `cargo-audit` not available in this environment
`cargo audit` errored ("no such command"). Manually scanning `Cargo.lock`
for known-bad pins: `getrandom 0.2` (`Cargo.toml:14`), `rand 0.8`,
`rand_xoshiro 0.6`, `serde_json 1.x`, `wasm-bindgen 0.2`, `js-sys 0.3`,
`twox-hash 1.6` — none currently carry open advisories matching the major
versions used. `wide 0.7` has had advisories in older 0.6 lines only.
Re-run `cargo install cargo-audit && cargo audit` in CI to keep this fresh.

---

## Summary

| Sev | Issue | File |
|-----|-------|------|
| H1  | Unvalidated save-load fields → panic / abort | `src/world.rs:1047-1167`, `src/species.rs:89-91`, `src/sun.rs:65-77` |
| M1  | `Float32Array::view` lifetime contract unenforced | `src/wasm_api.rs:132,145,151,166,213` |
| M2  | esbuild 0.21.5 dev-server CORS (dev-only) | `web/pnpm-lock.yaml:299` |
| M3  | No CSP | `web/index.html`, `web/public/_headers` |
| M4  | `unsafe` profiler ptr — local soundness argument | `src/profiler.rs:291-314` |
| M5  | Dead `innerHTML` sink that will become live in v1.1 | `web/src/rail/events.ts:90` |
| L1  | `expect()` on JSON serialize | `src/wasm_api.rs:301` |
| L2  | URL `seed` length uncapped | `web/src/main.ts:162-215` |
| L3  | Latent inspector sink | `web/src/rail/inspector.ts:35-36` |
| L5  | `cargo audit` not run | CI |

Top priority: **H1**. Add a `validate_save(&SaveV1) -> Result<(), LoadError>`
that walks every field used as an index or length and returns
`StructuralError` rather than panicking. The JS layer already does the
right thing on `structural:` errors (`web/src/main.ts:197-204`).
