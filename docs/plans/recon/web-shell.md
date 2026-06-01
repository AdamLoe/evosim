# Recon: Web shell, worker bridge, SAB protocol (TS side), build/test

Current-state map for v2.0.0 implementation. Read-only recon — every claim
is anchored to `path:line`. Distilled, not pasted.

**Source-of-truth precedence (followed here):**
`src/` Rust (`constants.rs`, `wasm_api.rs` SAB layout) **>** generated
`web/wasm/*.d.ts` **>** `web/` TS **>** tests **>** docs. The Rust owns the
wire format; TS consumes it. Every spot where a TS constant mirrors a Rust
constant is flagged as a **cross-language coupling** and collected in the
final section.

> Sibling doc `docs/plans/recon/sim-core.md` covers the Rust sim internals;
> this doc deliberately stops at the wasm-bindgen surface.

---

## 0. Rust source-of-truth anchors (the wire format TS mirrors)

- `src/constants.rs:4` `WORLD_SIZE = 1200.0` (f32). The single world-size knob.
- `src/constants.rs:127-131` grass: `GRASS_CELL_SIZE = 1.25`,
  `GRASS_GRID_DIM = (WORLD_SIZE / GRASS_CELL_SIZE) = 960`,
  `GRASS_CELL_COUNT = 960² = 921_600`.
- `src/constants.rs:112` `MAX_POP_FOR_SIM = 32_000` — drives the snapshot SAB
  creature-region size.
- `src/constants.rs:118` `MAX_POPULATION_DEFAULT = 8_000` (soft slider cap);
  `:99` `STARTING_POP_DEFAULT = 32`; `:88-89` `REPULSION_K=2.0`,
  `REPULSION_MAX=0.1`.
- **Snapshot SAB layout (authoritative):** `src/wasm_api.rs:99-111`
  `SNAPSHOT_HEADER_BYTES = 32`, `SNAPSHOT_CREATURE_STRIDE = 32` (bytes; = 8
  f32 lanes), `SNAPSHOT_CREATURE_BYTES = MAX_POP_FOR_SIM × 32`,
  `SNAPSHOT_GRASS_BYTES`, `SNAPSHOT_SLOT_BYTES = header + creatures + grass`.
- **Creature SoA lane writer (authoritative byte offsets):**
  `src/wasm_api.rs:881-912` `fill_creature_bytes`. Per 32-byte record:
  `+0` x, `+4` y, `+8` body_r (`= CREATURE_SIZE × BODY_RADIUS_PER_SIZE = 1.0`),
  `+12` color_r, `+16` color_g, `+20` color_b (each `.max(0.15)` floored),
  `+24` id_lo (u32 LE), `+28` id_hi (u32 LE). Color is action-EMA with a 0.15
  display floor applied **Rust-side**.
- Header writer `src/wasm_api.rs:413-423`: `+0` tick u32, `+4` pop u32, `+8`
  world_ended u32, `+12` tps_bits (`f32::to_bits`), `+16` jank u32; `+20..32`
  padding (32B-align the SoA).
- Grass write `src/wasm_api.rs:434-438`: **single contiguous memcpy** of the
  sim's `Vec<f32>` density into the grass region (no quantize, raw f32/cell).
- Accessors: `world_size()` → `WORLD_SIZE` (`wasm_api.rs:307`), `grass_dim()`
  → `GRASS_GRID_DIM` (`:316`), `grass_cell_size()` → `GRASS_CELL_SIZE` (`:323`).
- Bindings generator + drift guard: `src/bin/gen_bindings.rs` writes
  `web/src/generated/{slider-ids,control-sab}.ts`; its `bindings_in_sync`
  unit test (`gen_bindings.rs:133`) fails CI on drift. **The drift guard is
  Rust-side, not TS-side.**

⚠ **Stale doc-comment, value is correct:** both `web/wasm/evosim.d.ts:197`
and `src/wasm_api.rs:313` say "Grass grid dimension (480)" — the actual
returned value is `GRASS_GRID_DIM = 960`. Comment lies; code is right.

---

## 1. Renderer — `web/src/render-gl.ts`

WebGL2, one module-level `state` (`render-gl.ts:355`), lazily built by
`initRenderer` (`:389`). Five programs: **disc** (bodies + trait/highlight
rings, shared annulus shader), **halo** (rim glow), **trail**, **grass**
(fullscreen quad), **frame** (world-bounds LINE_LOOP). Entry
`renderWorld(...)` (`:721`) → `renderWorldImpl` (`:758`).

### Per-instance stride-8 decode (the lane map)
- The renderer reads the live SoA `Float32Array` view directly; stride is
  `CREATURE_STRIDE = 8` from `sim-bridge.ts` (`render-gl.ts:15,735`). It
  **hard-throws** if stride ≠ 8 (`:736-741`) as a Rust/TS drift tripwire.
- Body decode loop `render-gl.ts:890-932`, `base = i*stride`:
  `creatures[base]` = x, `[base+1]` = y, `[base+2]` = radiusWorld,
  `[base+3..+6]` = color r/g/b. **ID is read as u32**, not f32: a
  `Uint32Array` view is built over the same buffer (`:864-868`, `:1022`) and
  `idLo=idView[base+6]`, `idHi=idView[base+7]`, `cid = idHi*2^32 + idLo`
  (`:897-899`). This is the f64 ladder (lossless < 2^53, DECISIONS E.21).
- The **GPU instance** layout is a *different* 8-float packing (NOT the SoA):
  `FLOATS_PER_INSTANCE = 8` (`:289`) = `[cx, cy, outer_px, inner_px, r,g,b,a]`.
  The JS cull loop repacks SoA → instance scratch. `radiusPx = max(1,
  radiusWorld × PX_PER_SIZE × zoom)` (`:906`).

### Disc/body shading floor + ring
- **Brightness floor** (the "disc-shading floor"): `COLOR_FLOOR = 125/255`
  (`render-gl.ts:914`). If a creature's max channel < floor, it's lifted
  preserving hue; true-zero history → mid-gray. (Distinct from the 0.15
  Rust-side EMA floor.)
- Min on-screen radius floored to 1px (`:906`). Ring stops are constants set
  once at link: `ringInner=0.96`, `ringOuter=1.0` (`:438-439`). Annulus
  branch vs AA filled-disc branch chosen by `inner_px>0` in `DISC_FS`
  (`:75-91`).

### Grass texture upload path
- `render-gl.ts:786-815`, gated on `getSettings().showGrass`. Texture is
  **R32F** (raw f32, no quantize): first frame / dim-change does full
  `texImage2D(...,R32F, dim, dim, ...,RED,FLOAT, grass)` (`:796`), subsequent
  frames `texSubImage2D(...,RED,FLOAT, grass)` (`:799`). **Full upload every
  frame** (~3.7 MB at 960²) — **no dirty-rect logic exists.** `dim` comes
  from `grass_dim` (runtime), compared against cached `grassTexDim`.
- ⚠ The shader/source comments still describe **R8** (`render-gl.ts:1,169-172`,
  `GRASS_FS:193`) — stale; the actual code path is R32F (`:554-569,796-799`).
  Float-linear filtering needs `OES_texture_float_linear`, else NEAREST
  fallback (`:559-569`).

### Frustum cull, camera math, trails, highlights
- **CPU frustum cull**: `render-gl.ts:843-856,903-905`. `halfW/​halfH =
  (view/zoom)*0.5`; per-creature reject if outside `[min,max]±margin`
  (`margin = radiusWorld+2`).
- **Trails** (`:884-955,980-997`): id-keyed `prevById`/`currById` maps swapped
  per paint; segment prev→curr drawn only if moved (`d²>0.04`) and `pop≤12000`
  (`:887`). No position interpolation (removed v1.13 Wave 2; main seq-gates
  paints).
- **Halo** pass skipped if `pop>8000` (`:968`); strength from `--creature-halo`
  alpha.
- **Highlight/selection ring** (rare path `:1020-1067`): reads `highlightMap:
  Map<id,expiryMs>`, draws annulus at `radiusPx+4/+2`, color (255,200,50).
  `HIGHLIGHT_FADE_MS=1500` (`:297`), permanent sentinel
  `Number.MAX_SAFE_INTEGER` (`:296`).
- World-bounds frame uses `world_size` uniform; reads `0..W` square
  (`:817-832`). All vertex shaders hardcode the world→screen transform
  `(world - u_cam_pos)*u_zoom + viewport*0.5`.

---

## 2. Camera — `web/src/render.ts`, `web/src/camera.ts`

- `PX_PER_SIZE = 2.5` (`render.ts:10`) — render radius = size × 2.5 px at base
  zoom.
- `makeCamera(worldSize)` (`render.ts:12`): initial `zoom = 4/3 ≈ 1.33`,
  centered at `worldSize/2`. World size is **passed in**, not hardcoded.
- **`clampCamera`** (`render.ts:18-27`): **`zoom = clamp(zoom, 0.25, 16)`** —
  the min-zoom clamp floor is **0.25** (the mission's "0.5" anchor is wrong;
  see notes). Pan clamp: `cx,cy ∈ [0, worldSize]` (world edge can't leave the
  viewport center). `viewW/viewH` are accepted but currently unused (`:25-26`).
- `worldToScreen`/`screenToWorld` (`render.ts:29-39`) — pure, world-size-free.
- Controls `camera.ts`: pointer drag pan (`pan = -d/zoom`), wheel zoom factor
  `1.063` per tick cursor-anchored (`:44-51`), two-finger pinch (`:61-90`).
  All re-clamp via `clampCamera(cam, getWorldSize(), w, h)`; `getWorldSize` is
  a **callback** (`main.ts:160` → `latestSnapshotWorldSize()`), so camera math
  already tracks a runtime world size.
- **World-size assumption baked into camera math:** none beyond the
  `[0,worldSize]` pan clamp and `worldSize/2` initial center — both take
  `worldSize` as a parameter. The only constant is `PX_PER_SIZE`.

---

## 3. Settings — `web/src/settings.ts`

- One localStorage key **`"evosim.settings.v1"`** (`settings.ts:10`),
  `SCHEMA_VERSION = 1` (`:11`). The version stamp is stored as field `v`
  (`Settings.v`, default `:109`) and **force-overwritten to `SCHEMA_VERSION`
  on every load and persist** (`:230,109`) — so a stale `v` never survives;
  there is **no migration ladder**, just key-whitelist + sanitize.
- **Load/merge path:** `readFromStorage()` (`:207`) → `pickKnown()` (`:148`)
  drops any key not in `KNOWN_KEYS` (`:146`); `mutationBuckets` and
  `nnTopology` get structural validators (`sanitiseBuckets:168`,
  `sanitiseTopology:185`). Merged as `{...DEFAULTS, ...stored, v:VERSION}`
  (`:227-231`). `hasStoredSetting(key)` (`:222`) lets boot apply
  env-derived defaults without clobbering explicit user choices (used for
  low-core `maxPopulation` lowering, `main.ts:128-133`).
- **Full settings list** (`Settings` interface `:55-106`, `DEFAULTS :108-144`):
  - Pacing/UI (live, page-side): `targetTPS=180`, `autoRun=false`,
    `showProfiler=false`, `showGrass=true`, `grassOpacity=1.0`,
    `profilerWindowMs=10_000`, `railOpen=false`, `theme="midnight"`,
    `railW=420`, `profilerH=240`.
  - **Live-tunable sim sliders** (apply to running world): `upkeepMultiplier`,
    `moveCostMultiplier`, `eatBiteFrac`, `grassPropagK`, `grassGrowthR`,
    `grassEnergyPerBite`, `grassBitesPerBlock`, `digestionCooldown`,
    `repulsionMax`, `maxPopulation`, `mutRate`, `mutationBuckets[8]`, `maxAge`,
    `splitThreshold`, `splitGift`, `splitJitter`.
  - **Construction-only** (shape next world only): `energyMax=100`,
    `initialGrassSeedCount=8000`, **`fullGrassOnInit=false`**, `founderCount=32`,
    `nnTopology` (`{layerSizes:[48,24], activations:["lrelu","lrelu"]}` `:46-49`).
    Construction-only set is enumerated in devpanel (`devpanel.ts:31-36`).
- **`full_grass_on_init`**: present (`Settings.fullGrassOnInit`, default false,
  `:92,132`); slider name `"full_grass_on_init"` is SAB index 21
  (`slider-ids.ts:30`).
- **`_reserved_curriculum_*` slots:** these live **only in the generated
  slider table** (`slider-ids.ts:26-30`): `_reserved_curriculum_min_pop`,
  `_reserved_curriculum_max_pop`, `_reserved_curriculum_min_factor`,
  `_reserved_auto_curriculum`, plus `_reserved_legacy_nn_mutation_sigma`
  (idx 1). They have **no `Settings` field and no devpanel widget** — they're
  reserved SAB lanes, no-ops on the Rust apply side.

---

## 4. Dev-panel UI — `web/src/widgets/devpanel.ts`

This is the **Settings rail tab** installer (`installDevPanel`, `:468`).

- **Two interaction tiers** (`:1-26`): **live-apply** (Display group:
  showGrass, grassOpacity, theme — push to bridge + persist immediately,
  `makeLiveSlider:322`, `makeLiveToggle:415`) vs **stage-then-apply** (Energy /
  Grass / Eat / Lifecycle groups — `makeStagedSlider:205`, `makeStagedToggle:282`;
  edits only touch widget state until the footer **Apply**).
- **Footer**: Apply (`applyAll:130`) pushes every dirty staged slider via
  `debouncedSetSlider` + persists; Cancel (`cancelAll:150`) restores snapshot;
  Reset (`resetAll:158`) → `resetSettings()` + push/persist defaults. Dirty
  rows get `.is-dirty` class; Apply/Cancel disabled when nothing dirty
  (`refreshDirtyState:119`).
- **Construction-only grouping + "restart required" toast:**
  `CONSTRUCTION_ONLY_SLIDERS` set (`:31-36`); rows flagged `nextWorld:true`
  get the `.next-world` label class. Apply/Reset fire
  `showToast(TOAST_CONSTRUCTION = "Saved. Click Restart to see these changes
  in a new world.")` (`:38-39,146,174`) — this IS the restart-required toast
  (`web/src/toast.ts`).
- **Show/hide gating:** `full_grass_on_init` toggle disables the
  "Initial seeded cells" row via `.is-disabled` (`:578-585`). `splitThreshold`
  max is dynamically capped to `energyMax-1` (`updateSplitThresholdCap:667`).
- `currentSliderState()` (`:67`) snapshots all live widget readers (+ fans
  persisted mutation buckets into `bucket_k_*`) for the boot `initial_sliders`
  payload — so a mid-drag restart carries the dragged value.
- ⚠ **Max population slider is NOT in devpanel** — moved to the perf panel
  (`devpanel.ts:661-663`; lives in `perf-panel.ts:480-524`). Likewise
  autoRun → top-bar button, profiler-visibility → top-bar/floating button.

---

## 5. Main thread + worker bridge

### Boot handshake — `web/src/main.ts`, `web/src/sim-worker.ts`
- Worker spawned ES-module (`main.ts:348`); only **one** postMessage survives:
  `boot` → `boot_ready` (`sim-bridge.ts:187-235`). Worker boot:
  `handleBoot` (`sim-worker.ts:137`) inits wasm, inits rayon pool
  (`TARGET_RAYON_WORKERS=12`, `:73,146-149`), constructs
  `WorldHandle.newWithFounderCount(...)` (`:172`), applies `initial_sliders` by
  name (`:185-191`), allocates the **control SAB**
  (`new SharedArrayBuffer(CONTROL_SAB_BYTES)`, `:197`), runs **one tick +
  one snapshot** before replying (`:223-224`), then posts `boot_ready`.
- `boot_ready` carries (`sim-worker.ts:230-242`): `world_size`, `grass_dim`,
  `threads`, `rayon_ok`, `max_pop_for_sim()`, **`wasm_memory` (the
  `WebAssembly.Memory` handle)**, `snapshot_buf_byte_offset/len`,
  `control_sab`, `sliders_defaults_json`.
- Main attach (`main.ts:377-417`): asserts
  `ready.max_pop_for_sim === MAX_POP_FOR_SIM` (rebuild-wasm guard, `:378`);
  builds **views over `wasm_memory.buffer`** at `snapshot_buf_byte_offset`
  (`snapshotView = new DataView(buffer, baseOffset, len)`, `:407-409`).
  Saves `latestWorldSize`/`latestGrassDim` from the reply (`:410-411`) —
  **these are the runtime dims everything downstream uses.** Snapshot region
  lives in **wasm linear memory**, not a separate SAB (v1.11).

### Snapshot views & sizing constants
- View sizing uses TS mirror constants from `sim-bridge.ts`:
  `CREATURE_STRIDE=8` (`:88`), `SNAPSHOT_HEADER_BYTES=32` (`:104`),
  `CREATURE_SOA_BYTES = MAX_POP_FOR_SIM×8×4` (`:107`),
  `GRASS_BYTES = 921_600×4` (`:116`), `GRASS_CELL_COUNT = GRASS_BYTES/4` (`:123`),
  `SLOT_BYTES`/`SNAPSHOT_SAB_BYTES` (`:126,136`). Slot offset helpers
  `slotOffset`/`creatureSoAOffset`/`grassOffset` (`:139-151`).
- Per-frame the creature view is `new Float32Array(buffer, base+
  creatureSoAOffset(slot), pop*CREATURE_STRIDE)` and grass is
  `new Float32Array(buffer, base+grassOffset(slot), GRASS_CELL_COUNT)`
  (`main.ts:283-294`). ⚠ **Grass view length is the hardcoded
  `GRASS_CELL_COUNT=921_600`, NOT derived from `latestGrassDim`** — a key
  coupling for runtime world_size (see notes).

### RAF loop + pacing
- RAF `frame()` (`main.ts:260-336`): reads `CTRL_SEQ`; **seq-gate** skips paint
  if `seq===lastPaintedSeq` (`:265-271`) so FPS ≤ TPS. Reads `CTRL_CURRENT_SLOT`
  for the live slot, `readSnapshotHeader` (`:278`), clamps `pop≤MAX_POP_FOR_SIM`
  (`:279`), builds views, calls `pollRail` + `renderWorld` + `setPanelStatus`.
- **Worker pacing is SYNCHRONOUS `Atomics.wait`, NOT `Atomics.waitAsync`**
  (`sim-worker.ts:467`, also pause-park `:417`). The loop `simLoop()`
  (`:388-471`) is a tight `while` that never yields to the event loop; control
  is SAB-only (read at top via `readControlSab():250`). Pacing: `remainingMs =
  1000/targetTPS - elapsed`; park on `CTRL_FUTEX` for `remainingMs` if
  `> 0.25ms` (`:459-468`). Main wakes the futex via `Atomics.notify` on
  slider/pause/TPS/inspect writes (`sim-bridge.ts:370-380` etc).
  **The mission's "`Atomics.waitAsync` futex pacing" anchor is outdated** — that
  was the v1.6→v1.9 design; v1.10 replaced it (see `sim-bridge.ts:11-15`).
- **Restart** (`main.ts:188-199`): spawn a **new** worker (`spawnSimWorker("")`),
  then `oldBridge.terminate()` → `worker.terminate()` (`sim-bridge.ts:479-496`).
  Each restart gets fresh wasm memory, so views are re-attached on every boot.
  Bound to `r`/`R` hotkey (`:222-229`) and top-bar/overlay buttons.

### Control messages (all SAB writes, `web/src/sim-bridge.ts`)
- Sliders: `debouncedSetSlider` (16ms trailing-edge, `:331`) →
  `writeSliderImmediate` writes `ctrlF32[CTRL_SLIDERS_BASE+idx]=value` then
  `Atomics.add(CTRL_CONTROL_EPOCH,1)` (`:352-364`). Worker re-applies all
  lanes on epoch advance via `set_slider_by_index` (`sim-worker.ts:262-269`).
- Pause `setPaused` (`:366`), TPS `setTargetTps` (`:374` — no epoch bump,
  read every tick), profile-window/clear/jank epochs, inspector req/resp
  (`requestInspectAt:410`, `requestInspectId:421`, polled at 60Hz
  `pollResponses:500`). Inspector id split into lo/hi u32 (`:425-432`).

---

## 6. Widgets — `web/src/widgets/*`, `web/src/rail/*`

> ⚠ **`docs/repository-layout.md` is stale here.** It lists a right-rail
> **Monitor** tab and `web/src/rail/{monitor,stats}.ts` + `worker-stats.ts`
> in the Monitor tab. Reality (v1.13 Wave 2): the rail tabs are
> **Inspector / NN / Settings** (`rail/index.ts:8`), and the population graph
> + per-worker stats + profiler **moved into the bottom perf panel**. Files
> `rail/monitor.ts` and `rail/stats.ts` **do not exist** anymore.

- **Population graph** (the "pop pop-graph widget"): now in
  `web/src/widgets/perf-panel.ts`. Sampler `setPanelStatus()` (`:141-182`)
  pushes `{tick, population}` into `popSamples` ring (`POP_SAMPLE_CAP=500`),
  gated to one sample per `POP_SAMPLE_INTERVAL_TICKS=10` ticks (`:38,173-179`).
  Painted by `redrawPopChart()` (`:640`) to a 2D `<canvas id="chart-pop">`
  (`:300`). **Single population line today** (`ymax = max(pop samples)`,
  `:663-701`) — no per-species / multi-line. A separate FPS/TPS chart
  (`redrawFpsTpsChart:553`) sits beside it.
- **Inspector panel** — `web/src/rail/inspector.ts`. Click handler
  `installCanvasClickHandler` (`:239`): tap (dist<10, <500ms) while the
  Inspector tab is active + rail open. **SoA fast-path** first
  (`findNearestInSoA:64`, tolerance `6.0/zoom` `:277`), then async
  `requestInspectId`/`requestInspectAt` for rich fields. Per-frame
  `refreshInspector` (`:194`) repaints body fields from SoA and throttles
  `inspect_id` to 250ms (`:27,209`). Renders `creature_inspect_json` via
  `renderInspector` (`:134`); typed shape `CreatureInspectJson` (`:155-171`).
  Reads ids from a `Uint32Array` SoA view (`idAt:50`, lanes +6/+7).
- **Per-worker NN stats** — `web/src/widgets/worker-stats.ts`,
  `installWorkerStatsPanel` (`:45`) builds a table inside the perf panel's
  "CPU Process Monitor", polls `requestNnStats()` at 750ms (`:12,98`).
- **Rail orchestrator** — `web/src/rail/index.ts`. `installRail()` →
  `installTabs()` (`:15`); tabs keyed by `data-tab`; `pollRail()` (`:48`)
  called per RAF from main, updates SoA + inspector + prunes highlights.
  `nn-tab.ts` hosts the mutation-bucket + topology editor (Apply respawns the
  worker via `restart()`).
- **Right-rail layout / resize**: width var `--rail-w` (clamp [280,720]),
  bottom panel `--profiler-h` (clamp [160, 60% innerHeight]), drag handles in
  `main.ts:567-628`; persisted as `railW`/`profilerH`.

---

## 7. WASM bindings — `web/wasm/*.d.ts`

- `wasm-pack --target web` output (gitignored, regenerated each Rust build):
  `evosim.js` (glue), `evosim_bg.wasm`, **`evosim.d.ts`** (typed surface),
  `evosim_bg.wasm.d.ts`.
- TS imports: worker does `import init, { WorldHandle, max_pop_for_sim } from
  "../wasm/evosim"` (`sim-worker.ts:19-22`) and feature-probes
  `initThreadPool` / `rayon_current_num_threads` off the module namespace
  (`:65-70`) since they only exist on threaded builds.
- **Exported constants the TS reads** are *getters on `WorldHandle`*, not free
  constants: `world_size`, `grass_dim`, `grass_cell_size`,
  `snapshot_buf_byte_offset/len`, `snapshot_{header,creature,grass,slot}_bytes`,
  `tps`, `jank_count`, `population`, `tick`, `seed`, `world_ended`
  (`evosim.d.ts:193-241`). Free fn `max_pop_for_sim()` (`:253`) used in the
  boot assert. Report shapes documented in `.d.ts` doc-comments:
  `nn_worker_stats_json` (`:69-97`), `profile_report_json` (`:113-120`),
  `creature_inspect_json` (`:38-45`), `sliders_defaults_json` (`:155-161`).
- Construction: `newWithFounderCount(seed, grass_seed, energy_max,
  founder_count, full_grass_on_init, nn_topology_json)` (`evosim.d.ts:68`) —
  the sole construction path; `nn_topology_json` `""` ⇒ legacy 48→24 default.

---

## 8. Build / dev loop

- **Wasm build (the threaded incantation — required):**
  ```
  rustup run nightly wasm-pack build --target web --out-dir web/wasm --dev --features threads
  ```
  (`docs/agent-context/dev-loop.md:19`). **`--features threads` is mandatory**
  — without it the sim silently single-threads. Verify:
  `grep -c initThreadPool web/wasm/evosim.js` → 2 (threaded) | 0 (plain);
  `grep -F 'shared:true' web/wasm/evosim.js` → 1 (`dev-loop.md:24-27`).
  TS-only changes need no rebuild (Vite HMR); a hard-reload is needed after a
  wasm rebuild (`dev-loop.md:34-36,55-57`).
- **Dev server:** `cd web && pnpm dev` (Vite, **port 47821**,
  `vite.config.ts`), serves COOP/COEP headers (mirrored in
  `web/public/_headers`). One-liner in `dev-loop.md:47-53`.
- **TS gates:** `cd web && pnpm typecheck` (`tsc --noEmit`); `pnpm build`
  (`tsc --noEmit && vite build`) (`package.json:8,10`).
- **Playwright e2e:** `cd web && pnpm test:e2e` (= `playwright test`,
  `package.json:11`). Runner boots Vite via `webServer` and reuses an existing
  `:47821` server (`playwright.config.ts`, `reuseExistingServer:true`).
  Chromium-only, serial (`workers:1`). Variants:
  `pnpm test:e2e --headed|--debug`, `--project chromium --grep "pause"`.
- **Specs + coverage** (`web/tests/e2e/`):
  - `sim-bridge.spec.ts` — UI smoke for pause / TPS / slider / profile-toggle /
    restart. **Forces targetTPS=1000** to catch the yield-less-loop regression
    class.
  - `sab-control.spec.ts` — SAB-transport regression: slider write observed,
    inspector click round-trip, `sim_worker`/`snapshot`/`nn.build_input`
    profile trees populate.
  - `defaults-drift.spec.ts` — asserts Rust `sliders_defaults_json()` ==
    `settings.ts` `DEFAULTS` for every shared slider + bucket (reads
    `window.__rustSlidersDefaults` stashed at `main.ts:414`).
- **`bindings_in_sync` is Rust-side** (`src/bin/gen_bindings.rs:133`), run by
  `cargo test`, not a TS test.
- ⚠ **Broken-test risk (verified):** `sim-bridge.spec.ts:26` and
  `sab-control.spec.ts:25` read `document.getElementById("status")`, but there
  is **no `#status` element in `index.html`** anymore (removed v1.13 Wave 2;
  the real status line is `#perf-status-line`, `perf-panel.ts:256`). So
  `readStatus` falls through its `?? ""` and `waitForBoot` (keyed on
  `/tick \d+/`) will **time out at 15s** — these two specs are effectively
  non-functional as boot gates today. The test-owning agent must re-point them
  at `#perf-status-line`. `defaults-drift.spec.ts` is unaffected (it polls
  `window.__rustSlidersDefaults`).

---

## Key risks / notes for v2.0.0

### Cross-language coupling points (Rust constant ↔ TS mirror)
These are exactly the spots Wave 1's runtime `world_size` turns from
constant-equality into computed-dims-equality. Each must move from a baked
constant to a value derived from the boot-time `world_size`/`grass_dim`:

1. **`MAX_POP_FOR_SIM`** — `src/constants.rs:112` (32_000) mirrored at
   `sim-bridge.ts:79`; boot asserts equality (`main.ts:378`). Drives
   `CREATURE_SOA_BYTES` (`sim-bridge.ts:107`) and the trail/instance scratch
   ceilings (`render-gl.ts:603`).
2. **Grass dims / `GRASS_CELL_COUNT`** — `src/constants.rs:129-131`
   (960 / 921_600) mirrored as the hardcoded `GRASS_BYTES = 921_600*4` and
   `GRASS_CELL_COUNT` (`sim-bridge.ts:116,123`). **The per-frame grass
   `Float32Array` view length is this hardcoded constant** (`main.ts:290-294`),
   while the *texture upload dim* already uses the runtime `latestGrassDim`
   (`render-gl.ts:790-799`). If world_size becomes runtime, the grass-region
   byte size and the view length must derive from `grass_dim²`, or the view
   will over/under-run the SAB slot. **This is the single most likely
   silent-corruption site.**
3. **Snapshot header / creature stride** — `SNAPSHOT_HEADER_BYTES=32`,
   stride 32B/8-lane: `src/wasm_api.rs:99-104` ↔ `sim-bridge.ts:88,104`.
   Stride is also runtime-throw-guarded (`render-gl.ts:736`). Stride is
   world-size-independent; header/SoA sizing is not (depends on
   `MAX_POP_FOR_SIM`).
4. **`SLOT_BYTES` / `SNAPSHOT_SAB_BYTES`** (`sim-bridge.ts:126,136`) are
   computed from the above three — they'll recompute correctly **iff** the
   grass and pop terms become runtime-derived.
5. **Control SAB layout + slider table** — fully generated
   (`generated/control-sab.ts`, `generated/slider-ids.ts`) from Rust by
   `gen-bindings`, drift-guarded Rust-side. **Not** world-size-coupled; safe.

### Where the mission's stated anchors look wrong
- **Min-zoom clamp is `0.25`, not `0.5`** (`render.ts:19`). The `0.5` in the
  mission brief does not appear; the initial zoom is `4/3`.
- **Pacing is synchronous `Atomics.wait`, not `Atomics.waitAsync`**
  (`sim-worker.ts:417,467`). The brief's "`Atomics.waitAsync` futex pacing" is
  the pre-v1.10 design; SAB-only control replaced it (`sim-bridge.ts:11-15`).
- **Grass upload is R32F (raw f32), not R8** (`render-gl.ts:796-799`).
  Several code comments still say R8 (`:1,169-172`) — stale; ignore them.
- **No dirty-rect grass logic exists** — full ~3.7 MB texture upload every
  painted frame (full `texImage2D` on dim-change, `texSubImage2D` otherwise).
  A runtime-larger world makes this upload bigger; worth a perf note for v2.
- **Rail is Inspector/NN/Settings, not Inspector/Monitor/Settings**, and the
  pop graph / worker-stats / profiler all live in the bottom **perf panel**,
  not the rail. `docs/repository-layout.md:60-66` is stale on this.
- **Settings version stamp** is a single `v=1` field under key
  `evosim.settings.v1`, force-reset on load with **no migration path** — a v2
  schema bump should decide whether to rename the key or add migration, since
  unknown keys are silently dropped today (`settings.ts:148-165`).
- **`grass_dim` doc-comments say "480"** in both the generated `.d.ts:197` and
  `wasm_api.rs:313`, but the value is **960**. Don't trust the comment.
- **Two e2e specs read a removed `#status` element** — likely already failing
  or only passing by the `?? ""` fallback timing out; re-point before relying
  on them as v2 gates.
