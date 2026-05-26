# WebGL2 instanced-rings renderer — design doc

**Date:** 2026-05-24
**Status:** design-only; no code lands this pass; implementation scheduled by a future orchestration pass.
**Author scope:** plan-only. The deliverable is this document.
**Original motivation:** `docs/audit/big-wins.md` #1 ("Move the renderer to WebGL/WebGPU instanced-quads with a stable SoA view"). v5 §14 deferred this item to v1.1+; the user authorized this design doc only.

---

## 1. Motivation

The current renderer (`web/src/render.ts`) is CPU-bound and Canvas2D-bound. Concretely, per frame at population N with the 5-ring stack:

- `drawSunMap` does `SUN_DIM × SUN_DIM = 20 × 20 = 400` `fillRect` calls plus 400 `fillStyle` string allocations (`"rgb(r,g,b)"`) per frame. Roughly 800 micro-allocs/frame.
- `drawCarrion` does `K_carrion` × (`beginPath`, `arc`, `fill`) — typically <100, fine.
- `drawCreatures` does, per creature:
  - 1 `fillStyle` string alloc, 1 `beginPath`, 1 `arc`, 1 `fill` for the body.
  - Up to 5 stroked rings — each is its own `beginPath` + `arc` + `stroke` + `lineWidth` set + `strokeStyle` set. Worst case ~25 Canvas2D state mutations + 5 path-builds + 5 strokes per creature.
  - 1 highlight ring with `toFixed(3)` string alloc when highlighted.
- The per-creature SoA view is already in WASM memory (`creatures_buffer()` in `src/wasm_api.rs:103`); currently 13 floats/creature, shrinking to 11 after S21 lands. The JS loop iterates it scalarly and issues one draw call set per creature. There is no off-screen culling; creatures beyond the viewport still cost a full draw-call stack.

**Observed and estimated frame budgets** (no measured profile attached to this doc; numbers are estimates anchored to the per-piece costs above):

| Population | Rings/creature | Est. CPU render cost | Notes |
|---|---|---|---|
| 800 (v1 baseline) | 1–3 | 4–7 ms | Comfortable headroom at 60fps. |
| 2 000 | 3 | 12–18 ms | Begins to compete with the 16.6 ms RAF budget. |
| 5 000 | 3–5 | 35–60 ms | Misses 60fps; per-creature draw stack and sun grid dominate. |
| 5 000 @ zoom 8× | 3–5 | 50–80 ms | Overdraw amplified by larger on-screen radii. |

(Once `audit-master.md` PR-4 lands its per-tick perf followups, the simulation can sustain 5 000+ creatures comfortably; the renderer becomes the next bottleneck.)

**Why WebGL2 fixes this:**

- One instanced draw call replaces the per-creature loop. The driver iterates N instances on the GPU with zero JS overhead.
- The SoA buffer goes straight from WASM memory to a GPU vertex buffer via `gl.bufferSubData(gl.ARRAY_BUFFER, 0, creaturesView)` — no per-instance JavaScript.
- Ring arithmetic moves from CPU (`ringR -= 1.5` per ring per creature) to a vertex/fragment shader (parallel, free).
- Overdraw at high zoom is fragment-bound and the GPU handles it natively; CPU stops being the limit.

WebGL2 is universally available on every browser the project supports (Chrome, Firefox, Safari 15+, Edge). The renderer can be migrated incrementally because the simulation, click handling, and SoA layout are unchanged.

---

## 2. Goals and non-goals

### Goals

- Sustain 60fps at 5 000 creatures with the full 5-ring stack and the sun grid, including sustained pan/zoom.
- Use GPU instancing (`drawArraysInstanced` with `vertexAttribDivisor`) so the per-frame JS cost is O(1) in creature count (one `bufferSubData` upload plus one draw call).
- Visual parity with the existing Canvas2D output within a perceptual ε: identical colors (`RING_COLORS`), identical ring order (eye → move → scav → mouth → armor), identical radii, identical highlight semantics (E.22).
- Progressive migration: ship the WebGL2 path alongside Canvas2D behind a flag, validate parity, then default-flip.
- Click handling remains identical: pointer → `screenToWorld` → `world.creature_at(wx, wy, tolerance)` (Rust-side). No GPU readback.
- Fall back to Canvas2D when `getContext('webgl2')` returns null.

### Non-goals

- **WebGPU.** Deferred per v5 §14; this design picks WebGL2 deliberately for browser breadth.
- **Shader-side culling.** v1.0 of this renderer uploads every creature every frame. A future v1.2 pass can add a CPU-side viewport-AABB cull or move to a compute-shader cull (WebGL2 has no compute; would require WebGPU).
- **Novel visual effects.** Parity only. Energy halos, motion trails, soft shadows, eye-cone overlays, and per-pixel sun fields are all things this architecture makes feasible — but not in scope of the initial migration. List them as v1.1 follow-ups.
- **Click-detection rewrite.** `world.creature_at(...)` stays Rust-side. No GPU picking.
- **Camera module changes.** `web/src/camera.ts` (pointer/wheel/pinch) and `screenToWorld` / `worldToScreen` (in `render.ts`) are reused unchanged. Camera state stays a JS object.
- **Touching the simulation Rust code.** This renderer reads `creatures_buffer()`, `sun_buffer()`, `sun_capacity_buffer()`, `carrion_buffer()`, `creature_ids_buffer()`. All five exist today.

---

## 3. Architecture sketch

### 3.1 Module shape

Add `web/src/render_gl.ts` alongside `web/src/render.ts`. It exports a sibling `renderWorldGL(...)` with the same signature as `renderWorld`, plus a one-time `initGL(canvas)` that compiles shaders and creates buffers/VAOs.

```
web/src/
  render.ts        // existing Canvas2D path. Untouched.
  render_gl.ts     // new: WebGL2 path. Same public API surface (renderWorldGL).
  gl/
    shaders.ts     // GLSL source strings + helpers for compile/link/uniform lookup.
    instancing.ts  // VAO + instance-attribute layout, bufferSubData wrapper.
```

`main.ts` chooses which renderer at boot based on the `?renderer=` query param and the WebGL2 capability check; the per-frame call site (`renderWorld(...)` at `web/src/main.ts:300`) becomes a function pointer assigned once at boot.

### 3.2 Data flow per frame

```
WASM linear memory
  ├── creatures SoA       --[creatures_buffer() → Float32Array view]--> GPU VBO (instance attrs)
  ├── sun fracs           --[sun_buffer() → Float32Array]--> GPU texture (20×20 R32F)
  ├── sun capacity        --[sun_capacity_buffer()]--> GPU texture (20×20 R32F, uploaded once unless changed)
  └── carrion             --[carrion_buffer()]--> GPU VBO (per-carrion instance attrs)
                                                       ↓
                                              Draw passes:
                                              1. clear
                                              2. sun grid (1 fullscreen quad, samples both sun textures)
                                              3. world frame (1 line-rect draw)
                                              4. carrion (instanced quads, N_carrion instances)
                                              5a. creature bodies (instanced quads, N_creatures instances, body shader)
                                              5b. creature rings (instanced quads, N_creatures × N_RINGS instances, ring shader)
                                              6. highlight ring (CPU-side culled to highlighted set,
                                                 N_highlight × 1 instances of the same ring shader with a flag)
```

### 3.3 Per-instance attribute layout (creatures)

**This design targets the post-S21 layout.** `docs/plans/audit-s21-drop-unused-flag-floats.md` is the authoritative source of truth. S21 ships stride **11** with **five separate `float` flag fields** — it does NOT pack a `ring_mask`. Any future flag-packing into a `ring_mask` uint is deferred to a follow-up piece ("S21b") and is explicitly out of scope here. Consuming the raw flag floats is simpler and avoids bit-unpacking in GLSL.

Post-S21 per-creature record (stride = 11 floats):

| Offset | Field | Type | Used by |
|---|---|---|---|
| 0 | `x` | float | vertex shader (world→clip) |
| 1 | `y` | float | vertex shader |
| 2 | `radius_world` | float | vertex shader (`= size × BODY_RADIUS_PER_SIZE`, pre-computed in Rust) |
| 3 | `pigment_r` | float | fragment shader (body color, 0..1) |
| 4 | `pigment_g` | float | fragment shader |
| 5 | `pigment_b` | float | fragment shader |
| 6 | `flag_eye` | float | vertex+fragment (1.0 if eye_count > 0, else 0.0) |
| 7 | `flag_move` | float | vertex+fragment (1.0 if move_speed > 0, else 0.0) |
| 8 | `flag_scav` | float | vertex+fragment (1.0 if scavenge_efficiency > 0, else 0.0) |
| 9 | `flag_mouth` | float | vertex+fragment (1.0 if eat_efficiency > 0, else 0.0) |
| 10 | `flag_armor` | float | vertex+fragment (1.0 if armor > 0, else 0.0) |

`radius_world` is the body radius in world units already multiplied by `BODY_RADIUS_PER_SIZE` — the vertex shader does not need to replicate that multiply.

Ring flags are tested in GLSL as `a_flag_eye > 0.5` (matching the Canvas2D `data[i+6] > 0.5` convention). Future micro-perf passes may introduce a packed `ring_mask` uint field if GPU-side bit-testing proves worthwhile; today's design reads the raw flag floats directly.

To draw N_RINGS trait rings per creature with instancing, the implementation uses the `gl_InstanceID % N_RINGS` trick: draw `N_creatures × N_RINGS` instances of a single quad and compute `ring_index = gl_InstanceID % N_RINGS` and `creature_index = gl_InstanceID / N_RINGS` directly in the vertex shader. No extra VBO for ring indices is needed.

**Active-ring CPU pack (per frame, O(N)).** Canvas2D packs only active rings inward — rings are drawn densely from `radiusPx - 1` inward, skipping inactive traits entirely. The WebGL2 vertex shader must reproduce this behavior. The approach chosen is **(a): pre-compute the active-ring slot on the CPU before the `bufferSubData` upload**.

Before uploading Buffer A each frame, the `render_gl.ts` upload function reads the five flag floats for each creature and builds a 5-element `packed_ring_offsets[i]` auxiliary array (or packs the slot index into a 5-float `a_ring_slot[0..4]` attribute). Concretely, for each of the five rings in flag order (eye, move, scav, mouth, armor), if the flag is active it receives the next sequential slot index (0, 1, 2, …); inactive rings receive `-1` (sentinel for "don't draw"). This O(N) CPU pass runs once per frame in JS, after `world.creatures_buffer()` is fetched and before `gl.bufferSubData`.

This adds a per-frame O(N) JS loop (roughly 5 comparisons × N creature entries), which is a measurable but small cost — roughly equivalent to the existing Canvas2D ring-dispatch loop that it replaces. The benefit is that the vertex shader uses the pre-computed slot index directly via a simple `float` attribute rather than shader-side popcount arithmetic.

Concrete instance-attr buffers bound to the VAO:

- **Buffer A** (per-creature SoA, uploaded once per frame via `bufferSubData`; stride 11 floats from `creatures_buffer()` plus 5 additional floats appended per creature for ring slots):
  - `a_pos` (vec2, `divisor = N_RINGS`) — world-space center.
  - `a_radius_world` (float, `divisor = N_RINGS`) — pre-computed body radius.
  - `a_pigment` (vec3, `divisor = N_RINGS`) — RGB, 0..1.
  - `a_ring_slot[5]` (5× float, `divisor = N_RINGS`) — pre-computed active-ring slot per trait flag (-1 = inactive, 0..4 = draw at that inward slot). This is the CPU-packed offset described above.
- **Body draw:** body fill is a **separate draw call** after the ring draw (see §3.5 body pass). It reads `a_pos`, `a_radius_world`, and `a_pigment` from Buffer A (same divisor=1, one instance per creature). This is cleaner than routing body through the ring shader as a special `ring_index = N_RINGS - 1` case, which breaks the slot math.

Draw call for rings:
```
gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, N_creatures * N_RINGS);
```

Draw call for body fill (separate pass, same VAO with different shader or uniform):
```
gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, N_creatures);
```

`divisor = N_RINGS` is supported on every WebGL2 implementation per the spec (instanced attribute divisors are unrestricted positive integers).

**Important:** Do not store the `Float32Array` returned by `world.creatures_buffer()` across frames — WASM memory growth invalidates the view. Call it fresh each frame (see Risk R-WGL-6).

### 3.4 Vertex shader (illustrative GLSL, not production)

Two shaders are used: one for the **ring pass** and one for the **body pass** (separate draw calls, see §3.3 and §3.5).

#### Ring-pass vertex shader

```glsl
#version 300 es
precision highp float;
precision highp int;

uniform mat3 u_view;        // world → clip-space (camera + viewport)
uniform float u_zoom;       // camera zoom, for converting world radius to screen px
const int N_RINGS = 5;      // five trait rings (eye, move, scav, mouth, armor)

in vec2  a_quad_corner;     // [-1,-1]..[+1,+1], per-vertex
in vec2  a_pos;             // world-space center, per-creature, divisor=N_RINGS
in float a_radius_world;    // body radius in world units (pre-multiplied by BODY_RADIUS_PER_SIZE), divisor=N_RINGS
in vec3  a_pigment;         // RGB 0..1, divisor=N_RINGS (passed through for fragment)
// Per-creature active-ring slot for each of the 5 traits (CPU-packed, divisor=N_RINGS):
//   >= 0: draw this ring at the given inward slot (0 = innermost active ring)
//   <  0: this ring is inactive for this creature — discard
in float a_slot_eye;
in float a_slot_move;
in float a_slot_scav;
in float a_slot_mouth;
in float a_slot_armor;

out vec3  v_pigment;
out float v_ring_r_outer_px;   // outer radius in screen pixels
out float v_ring_r_inner_px;   // inner radius in screen pixels
flat out int v_ring_trait;      // 0=eye,1=move,2=scav,3=mouth,4=armor (-1 = inactive → discard)
out vec2  v_local_px;          // pixel offset from instance center

void main() {
    // Recover per-instance ring index from gl_InstanceID.
    int ring_index = gl_InstanceID % N_RINGS; // 0=eye,1=move,2=scav,3=mouth,4=armor

    // Read the CPU-pre-computed active slot for this trait.
    float slot_f;
    if      (ring_index == 0) slot_f = a_slot_eye;
    else if (ring_index == 1) slot_f = a_slot_move;
    else if (ring_index == 2) slot_f = a_slot_scav;
    else if (ring_index == 3) slot_f = a_slot_mouth;
    else                      slot_f = a_slot_armor;

    // slot_f < 0 means inactive: emit a degenerate quad (all vertices at origin).
    // The fragment shader will discard it via v_ring_trait = -1.
    if (slot_f < 0.0) {
        gl_Position = vec4(0.0);
        v_ring_trait = -1;
        v_pigment = a_pigment;
        v_ring_r_outer_px = 0.0;
        v_ring_r_inner_px = 0.0;
        v_local_px = vec2(0.0);
        return;
    }
    int active_slot = int(slot_f + 0.5); // round to nearest int

    // Canvas2D ring-positioning parity:
    //   ringR = radiusPx - 1       (start just inside body edge)
    //   ringR -= 1.5 per drawn ring (step inward by RING_STEP for each active ring)
    // active_slot 0 → outermost active ring (innermost in Canvas2D's inward-first draw order
    // means the FIRST ring drawn: radiusPx - 1). Each subsequent slot steps inward by 1.5.
    float RING_STEP = 1.5;
    float body_radius_px = a_radius_world * u_zoom;
    float r_outer_px = body_radius_px - 1.0 - float(active_slot) * RING_STEP;
    float r_inner_px = r_outer_px - 1.0;

    // Bounding quad: pad by 1.5px for AA headroom.
    float quad_half_px = r_outer_px + 1.5;
    // Convert quad half-size from screen-px back to world units for the view transform.
    float quad_half_world = quad_half_px / u_zoom;

    vec2 world_corner = a_pos + a_quad_corner * quad_half_world;
    vec3 clip = u_view * vec3(world_corner, 1.0);
    gl_Position = vec4(clip.xy, 0.0, 1.0);

    v_pigment = a_pigment;
    v_ring_r_outer_px = r_outer_px;
    v_ring_r_inner_px = r_inner_px;
    v_ring_trait = ring_index; // 0..4
    v_local_px = a_quad_corner * quad_half_px;
}
```

#### Body-pass vertex shader

```glsl
#version 300 es
precision highp float;

uniform mat3  u_view;
uniform float u_zoom;

in vec2  a_quad_corner;
in vec2  a_pos;           // divisor=1 (one instance per creature)
in float a_radius_world;  // divisor=1
in vec3  a_pigment;       // divisor=1

out vec3  v_pigment;
out float v_body_radius_px;
out vec2  v_local_px;

void main() {
    float radius_px      = a_radius_world * u_zoom;
    float quad_half_px   = radius_px + 1.5; // AA headroom
    float quad_half_world = quad_half_px / u_zoom;

    vec2 world_corner = a_pos + a_quad_corner * quad_half_world;
    vec3 clip = u_view * vec3(world_corner, 1.0);
    gl_Position = vec4(clip.xy, 0.0, 1.0);

    v_pigment        = a_pigment;
    v_body_radius_px = radius_px;
    v_local_px       = a_quad_corner * quad_half_px;
}
```

### 3.5 Fragment shaders (illustrative)

#### Ring-pass fragment shader

```glsl
#version 300 es
precision highp float;
precision highp int;

in vec3  v_pigment;
in float v_ring_r_outer_px;
in float v_ring_r_inner_px;
flat in int v_ring_trait;  // -1 = inactive (degenerate quad)
in vec2  v_local_px;

out vec4 frag_color;

// RING_COLORS: eye(0), move(1), scav(2), mouth(3), armor(4).
// Matches Canvas2D RING_COLORS in web/src/render.ts.
const vec4 RING_COLORS[5] = vec4[5](
    vec4(1.0,  1.0,  1.0,  0.85), // 0: eye   (white)
    vec4(0.94, 0.86, 0.31, 0.85), // 1: move  (yellow)
    vec4(0.55, 0.35, 0.16, 0.85), // 2: scav  (brown)
    vec4(1.0,  0.31, 0.31, 0.85), // 3: mouth (red)
    vec4(0.78, 0.78, 0.82, 0.85)  // 4: armor (silver)
);

void main() {
    if (v_ring_trait < 0) discard; // inactive ring — degenerate quad

    float d = length(v_local_px);
    // SDF annulus between inner and outer radii.
    float aa_outer = clamp(v_ring_r_outer_px - d, 0.0, 1.0);
    float aa_inner = clamp(d - v_ring_r_inner_px, 0.0, 1.0);
    float alpha = min(aa_outer, aa_inner);
    if (alpha <= 0.0) discard;

    vec4 col = RING_COLORS[v_ring_trait];
    frag_color = vec4(col.rgb, col.a * alpha);
}
```

#### Body-pass fragment shader

```glsl
#version 300 es
precision highp float;

in vec3  v_pigment;
in float v_body_radius_px;
in vec2  v_local_px;

out vec4 frag_color;

void main() {
    float d = length(v_local_px);
    if (d > v_body_radius_px) discard;
    // 1-pixel SDF AA at the body edge.
    float aa = clamp(v_body_radius_px - d, 0.0, 1.0);
    frag_color = vec4(v_pigment, aa);
}
```

All shaders are illustrative. Real production code adds: `precision highp int` where needed for the ring-pass integer arithmetic, correct sRGB handling (linear in, sRGB out — either `gl.SRGB8_ALPHA8` framebuffer or manual gamma encoding in the shader), and premultiplied alpha for correct blending of overlapping creatures (resolve Open question #8 before implementation). The body-pass draw executes **before** the ring-pass draw so the opaque body fill is composited correctly under the semi-transparent rings.

---

## 4. Tessellation strategy — recommendation: single quad per (creature × ring) with SDF in fragment

Two candidates were considered:

### Strategy A — single quad per (creature × ring) with SDF in fragment shader (chosen)

- Vertex count: `4 × N_creatures × N_RINGS`. For N = 5 000 and N_RINGS = 6: 120 000 vertices. Trivial.
- Fragment cost: O(quad area). At zoom 1×, a creature is ~25 px²; quad is ~36 px²; 5 000 creatures × 6 rings × 36 px² ≈ 1.08 M fragments/frame. At zoom 8×: ~16× more = 17 M fragments. A laptop iGPU pushes 1 G fragments/s easily; 17 M/frame is ~17 ms at the lowest end and far less on a real GPU. Comfortable.
- Pros: tiny vertex count, exact AA via SDF, ring step is one subtraction in the fragment, naturally handles zoom because radii scale with the view matrix.
- Cons: at very high zoom (>10×) overdraw climbs; rings beyond the body radius cost discarded fragments. Mitigation: clip the quad to a tighter AABB (`outer_radius + 1.5`) — already in the vertex shader above.

### Strategy B — N-quad-per-ring (N ≈ 16) fan tessellation

- Vertex count: `16 × N_creatures × N_RINGS` ≈ 480 000 verts for N = 5 000. Still fine.
- Fragment cost: lower than A at high zoom because the rasterizer covers only the ring band, not the full bounding quad. At low zoom, each segment is sub-pixel and you waste vertex work.
- Pros: lower fragment cost at extreme zoom.
- Cons: 16× more vertex shader invocations, harder anti-aliasing (needs `derivatives()` or per-segment normals), no good way to draw a clean 1-pixel outline at varying zoom, more complex code, more attribute traffic.

**Decision: A (SDF in fragment).** The simulation's per-frame fragment budget is well within the GPU envelope even at zoom 16× (the camera clamp at `web/src/render.ts:31`). The implementation simplicity and AA quality of SDF outweigh the marginal fragment-cost win of N-quad. If profiling later shows we're fragment-bound at zoom 16× with the maximum population, the v1.2 follow-up is to add CPU-side culling (cheap) or move to per-creature LOD (cheaper rings off at low pixel-radii).

---

## 5. Camera, DPR, and viewport

### 5.1 Camera as uniform

Camera state stays in JS (`Camera` in `web/src/render.ts:17`). Once per frame, build a 3×3 world-to-clip matrix:

```
u_view = T_clip · S_pixel_to_clip · T_screen_center · S_zoom · T_minus_camera_center
```

Conceptually: translate by `-cam.cx, -cam.cy`, scale by `cam.zoom`, translate to viewport center, scale by `2/viewW, -2/viewH` (flip Y), translate by `(-1, +1)`. This is one `mat3` uniform upload per frame (~36 bytes).

`screenToWorld` (used by the click handler in `web/src/main.ts:260` and by camera zoom in `web/src/camera.ts:42`) is unchanged. It already produces world coordinates from screen pixels using the same camera object.

### 5.2 DPR

S22 caps `devicePixelRatio` at 2 for the aquarium canvas in the Canvas2D path. The WebGL2 path applies the same cap:

```ts
const dpr = Math.min(window.devicePixelRatio || 1, 2);
canvas.width = Math.floor(viewW * dpr);
canvas.height = Math.floor(viewH * dpr);
gl.viewport(0, 0, canvas.width, canvas.height);
```

Pass `dpr` as a uniform for any pixel-space ring widths (the 1-pixel outline scales with DPR to stay 1 CSS pixel on screen).

### 5.3 Resize

A `ResizeObserver` on the canvas (or the existing resize handler in `main.ts`) updates `viewW`, `viewH`, the canvas size, and `gl.viewport(...)`. The view matrix is rebuilt every frame, so no extra step is needed.

---

## 6. Sun grid and carrion

### 6.1 Sun grid as a texture

Today `drawSunMap` (`web/src/render.ts:54`) does 400 `fillRect` calls. In WebGL2:

- Allocate a 20×20 `R32F` texture once at init (`gl.texImage2D(..., gl.R32F, 20, 20, ...)`).
- Each frame, upload the fraction array (`world.sun_buffer()`, already a Float32Array view) via `gl.texSubImage2D(..., gl.RED, gl.FLOAT, sun_buffer)`. 1.6 KB upload.
- Allocate a second 20×20 R32F texture for capacity. Upload once at boot (or whenever the user adjusts the `set_base_sun_rate` slider — capacity can change with dev sliders; check `world.sun_capacity_buffer()` each frame and skip the upload if unchanged via a JS-side dirty bit).
- Draw a single fullscreen quad in a "sun pass" with a shader that samples both textures, applies the same brightness formula as Canvas2D (`brightness = 0.04 + 0.18 * capN * frac`), and writes the color. No additive blending; this is the background pass — write into the cleared framebuffer.

This collapses 400 draw calls + 400 string allocs into 1 draw call + 1 texture upload.

### 6.2 Carrion

Carrion is `[x, y, pool]` per entry in `carrion_buffer()`. Same instanced-quad pattern as creatures, with a fixed gray color (`rgb(102, 102, 102)`) and radius `(2 + 4 * pool) × cam.zoom × 0.5 + 1` — port the existing formula in `drawCarrion` (`web/src/render.ts:99`) to the vertex shader. One draw call.

### 6.3 World frame

The "subtle border around the world's extent" (`drawAquariumFrame`) is one `gl.LINES` draw, 4 segments. Trivial.

---

## 7. Click handling and inspector highlight

### 7.1 Click → `creature_at`

Unchanged. The pointer-down handler (`installCanvasClickHandler` called from `web/src/main.ts:260`) converts screen coordinates to world coordinates via `screenToWorld(cam, viewW, viewH, e.clientX, e.clientY)` and calls `world.creature_at(wx, wy, tolerance_world)`. This returns an SoA index today; post-S18 it will return a stable `u64` id. Either way, the WebGL2 renderer is not involved — there is no GPU-side picking, no readback, no race.

**Important:** The click-tolerance argument is in **world units**. The current code (`web/src/main.ts` near the click handler) computes a tolerance from pixel slop / zoom — that calculation is unchanged.

### 7.2 Highlight ring (E.22)

Today `drawCreatures` builds a `highlightIdx: Set<number>` of SoA indices and stroke-draws a 2-pixel ring with a fading alpha for highlighted creatures (`web/src/render.ts:117–190`).

In WebGL2, two options:

- **Option A (recommended):** issue a separate small draw call after the main creature draw, using a per-highlight instance buffer (uploaded each frame) of `(x, y, size, alpha)`. Typically <10 highlights; cost is negligible.
- **Option B:** add a per-creature `highlight_alpha` attribute and let the main shader draw the ring conditionally. Wastes one attribute slot for the common case (alpha=0); rejected.

The highlight pass uses the same SDF shader as the trait rings, parameterized for the golden color and 2px width.

The highlight set itself stays a JS `Map<creatureId, expirationMs>` — same data structure as today (`highlightMap` parameter at `web/src/render.ts:113`). Building the per-frame highlight instance buffer is O(highlights), bounded.

---

## 8. Fallback path

At boot, in `main.ts` after the canvas is created:

```ts
let renderFn: typeof renderWorld;
const gl = canvas.getContext("webgl2", { antialias: false, premultipliedAlpha: true });
const want = new URLSearchParams(location.search).get("renderer");
if (gl && want !== "canvas2d") {
  initGL(gl); // compile shaders, create buffers
  renderFn = (ctx, cam, w, h, world, stride, ids, hl, now) =>
    renderWorldGL(gl, cam, w, h, world, stride, ids, hl, now);
  console.info("renderer: webgl2");
} else {
  const ctx = canvas.getContext("2d")!;
  renderFn = (_ctx, cam, w, h, world, stride, ids, hl, now) =>
    renderWorld(ctx, cam, w, h, world, stride, ids, hl, now);
  console.info("renderer: canvas2d", gl === null ? "(webgl2 unavailable)" : "(forced)");
}
```

Detection at boot only. No mid-run swap. The fallback is logged once; no UI change. The Canvas2D path is byte-for-byte the existing `renderWorld`, so the fallback is just "no migration occurred." `web/src/main.ts:300` becomes `renderFn(...)`.

WebGL2 is currently unavailable on:
- Safari < 15 (released Sep 2021; tiny tail).
- Some Android browsers on devices without GLES 3.0 (rare in 2026).
- Headless environments without GPU (some CI). Not relevant — CI does not exercise this path.

The COOP/COEP headers required for SharedArrayBuffer (already in place per `docs/dev-server-prompt.md`) are not affected. WebGL2 has no SAB requirement.

---

## 9. Progressive migration plan

### Phase 1 — side-by-side, opt-in

- Implement `render_gl.ts` to feature-parity with `render.ts`.
- Wire the boot-time choice as in §8.
- Default is Canvas2D. Set `?renderer=webgl2` in the URL to opt in.
- Visual-parity manual QA: open the app at the same seed in two tabs (one Canvas2D, one WebGL2) and compare side-by-side. Look for: ring widths, anti-aliasing, color match, highlight pulse, sun-cell brightness, carrion size scaling, behavior under pan/zoom.
- Document any parity deltas in `DECISIONS.md` (e.g., "WebGL2 ring AA is 1px softer than C2D stroke; accepted").
- Land Phase 1 in a single PR. The Canvas2D path is unchanged.

### Phase 2 — default-flip

- After ≥1 release of Phase 1 with no user-reported regressions, swap the default: `webgl2` becomes the default; `?renderer=canvas2d` becomes the escape hatch.
- Keep the Canvas2D path code intact; do not delete.

### Phase 3 — Canvas2D removal

- After ≥1 release of Phase 2 with no fallback-triggered reports, delete `render.ts` (except `renderCreaturePortrait`, which is used by the eulogy card on a separate canvas — that stays Canvas2D forever; it's a one-shot render of a single creature into a 96×96 offscreen canvas, no perf concern).
- `RING_COLORS` constant moves to a shared module (or stays exported from `render.ts` if the portrait function stays there).

Each phase is independently shippable; if any phase reveals issues, it can be rolled back without affecting the simulation.

---

## 10. Risks

### R-WGL-1 — Visual parity (SDF AA vs Canvas2D stroke)

Canvas2D's `stroke()` uses a sub-pixel coverage algorithm that's been tuned for decades. The SDF approach in §3.5 gives 1-pixel-band AA which is similar but not identical. Mitigation: side-by-side Phase 1 QA; document any accepted delta. Worst-case: ship a `u_aa_width` uniform tunable in dev console.

### R-WGL-2 — Shader compilation latency on first frame

Some drivers (especially older Intel iGPUs on Windows) take 50–200 ms to compile a shader the first time. This shows up as a single dropped first frame. Mitigation: compile shaders during `initGL(gl)` at boot, **before** the simulation loop starts. The boot path already has a perceptible WASM-init pause; the shader compile fits in there invisibly.

### R-WGL-3 — Shader debugging is harder

Canvas2D errors throw with stack traces; WebGL errors are silent unless polled. Mitigation: ship a debug overlay (off by default; toggled by a query param `?gldebug=1`) that:
- Calls `gl.getError()` after each draw call and logs non-`NO_ERROR` to console.
- Checks `getShaderInfoLog` and `getProgramInfoLog` at link time and throws on any non-empty log.
- Reports detected extension support and renderer info via `WEBGL_debug_renderer_info`.

This adds zero hot-path cost in non-debug mode.

### R-WGL-4 — Browsers without WebGL2

~3–5% of browsers in 2026 lack WebGL2 (mostly Safari 14 holdouts and locked-down enterprise Chrome). The Canvas2D fallback covers all of them; only the perf benefit is lost.

### R-WGL-5 — Stride coupling with S21 and future changes

The instance-attribute layout (§3.3) is tightly coupled to `creature_stride()` and the field order in `src/wasm_api.rs:103`. Post-S21, the expected stride is **11**. Any further change must update the instance-buffer schema in `render_gl.ts` in lockstep. Mitigation: include a runtime assertion in `initGL` that `creature_stride() === 11` (matching `EXPECTED_CREATURE_STRIDE` from S21's Canvas2D assert); the assertion catches the mismatch at boot instead of silently rendering garbage.

### R-WGL-6 — Float32Array view staleness across WASM memory growth

`big-wins.md #2` and `#4` flag this hazard. If WASM memory grows (e.g., population spike triggers reallocation of the creatures SoA), every cached `Float32Array` view goes stale. The Canvas2D renderer re-fetches `creatures_buffer()` every frame, which returns a fresh view. The WebGL2 path **must do the same** — call `world.creatures_buffer()` each frame and `gl.bufferSubData` from that fresh view. Do not cache the view across frames.

### R-WGL-7 — Premultiplied alpha and ring overlap

Ring colors today are stroked with `0.85` alpha and no explicit blend mode (Canvas2D defaults to source-over with straight alpha). In WebGL, the equivalent is `gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);` (straight) or premultiplied throughout. Pick one and stick to it — mixing produces subtly wrong overlaps where rings cross. The illustrative shaders in §3.5 emit straight alpha; the implementation should commit to one mode at the start and document it.

### R-WGL-8 — Pixel-snapped strokes

Canvas2D `stroke()` with `lineWidth: 1` produces a crisp 1-pixel band when coordinates fall on half-pixel boundaries; otherwise it's blurry. SDF rendering is always sub-pixel-accurate, so the WebGL2 path may look subtly different at certain zooms. Generally an improvement, but worth a visual-parity check.

---

## 11. Effort estimate

**L (multi-day implementation pass).** Breakdown for a future planner:

- Shader plumbing (initGL, compile/link, error overlay, VAO setup): 1 day.
- Creature passes (body shader + ring shader, CPU active-ring pack loop, Buffer A layout, SDF fragment shader, ring-positioning parity check vs Canvas2D): 2–3 days. (The CPU pack loop and two-shader body/ring split add a half-day vs the earlier estimate.)
- Sun pass (texture upload, fullscreen quad, brightness formula): 0.5 day.
- Carrion + highlight passes: 0.5 day.
- DPR, resize, fallback wiring, query-param flag: 0.5 day.
- Phase-1 manual QA against Canvas2D (including pixel-diff at fixed seed + `?paused=1`): 1–1.5 days.

Total: 6–8 engineering days, plus QA cycles in Phases 2 and 3. Suggest scheduling alongside the next perf-and-rendering pass (after `audit-master.md` PR-4 lands).

---

## 12. Out-of-scope of this doc

- WebGPU implementation (defer until WebGPU lands on iOS Safari stable; tracker: `big-wins.md` honorable mentions).
- Shader-side culling (frustum culling is cheap and adequate on CPU at expected population scales; defer until profiling demonstrates a frag-shader bind).
- Compute-shader vision raycasts (would require WebGPU).
- Replacing `web/src/camera.ts` (pan/zoom logic is fine; the camera object is reused).
- Touching the Rust simulation code (read-only consumer of existing `creatures_buffer`, `sun_buffer`, `carrion_buffer`, `creature_ids_buffer`, `world.creature_at`).
- Adding WebGPU as a third codepath (one new renderer is enough churn).
- Replacing `renderCreaturePortrait` (one-shot 96×96 offscreen render; Canvas2D is the right tool).
- An `OffscreenCanvas` + render worker. The `big-wins #1` writeup mentions this as a natural pairing; defer to a follow-up. The bottleneck this design addresses is per-frame CPU draw cost, not main-thread contention.

---

## 13. Open questions for the next orchestration

These are unresolved design decisions that a future planner must answer before the implementation pass begins.

1. **Pre- or post-S21 layout? — RESOLVED (2026-05-24).** This doc targets the post-S21 layout (stride 11, five separate float flags). If implementation begins before S21 lands, expand Buffer A to read the raw 13-float pre-S21 layout and strip `energy_frac`/`age_frac` at JS upload time, remapping flag offsets from 8–12 to 6–10. There is no `ring_mask` — S21 does not introduce one; see §3.3 for the definitive post-S21 field table.
2. **Shader-side LOD?** At zoom < 1×, creatures are sub-pixel and the SDF AA wastes fragments. Worth a `if (radius_px < 1.5) { simple_dot_shader }` branch? Defer to v1.1 unless profiling shows it.
3. **Dedicated render worker (`OffscreenCanvas`)?** Pairs naturally with this design (transfer the canvas to a worker, run the GL pass off the main thread). Adds complexity to the WASM-memory access path (the worker needs its own view of the SoA buffer; SAB makes this doable). Probably v1.1 follow-up; not v1.
4. **Sun grid: per-cell capacity drift detection.** Sun capacity changes only when dev sliders change. Should the renderer poll a `capacity_dirty` flag from the wasm API, or just re-upload every frame (1.6 KB)? Trivial either way; re-upload is simpler.
5. **Does the inspector's "draw a highlight ring around the selected creature" path break under WebGL2?** §7.2 says no — the highlight is drawn in a separate pass after the main creature pass, reading the same `highlightMap`. Confirm during Phase 1 QA.
6. **sRGB framebuffer or manual encoding?** Picking sRGB at framebuffer-config time gives correct blending automatically; picking manual lets us match Canvas2D's (subtly wrong-by-spec) behavior more closely. Probably go sRGB and accept the tiny color shift; document in DECISIONS.
7. **Antialiasing mode on the WebGL context.** `antialias: false` (and rely on SDF AA in the shader) is the right choice for this design — MSAA would double fragment work for no benefit. Confirm during implementation.
8. **Premultiplied vs straight alpha.** Pick one at boot and document. §3.5 assumes straight; premultiplied is more conventional for WebGL and slightly faster.
9. **What does "visual parity" tolerate?** Define ε before Phase 1 QA starts (e.g., "no perceptible difference at 1× zoom in a static screenshot; minor AA differences accepted at 8×+ zoom"). Without an explicit threshold, Phase 1 will drag on.
10. **Eye-cone overlays.** `web/src/render.ts` does not currently draw vision cones, but `big-wins #1` mentions them as a natural future visual. Should the v1 WebGL2 renderer reserve attribute slots for `eye_angle` and `eye_fov` to enable this without a second migration? Tiny cost; recommend yes. (Base layout in §3.3 is now fixed; these would be extra per-creature attributes appended to Buffer A alongside `a_ring_slot[]`.)
11. **Cross-piece coordination with `audit-master.md` PR-2's S20 (grid-backed `creature_at`).** S20 changes the implementation of `creature_at` but not its signature. No effect on this design. Re-confirm at planning time.
12. **Does Phase 3 (Canvas2D removal) need to preserve `web/src/render.ts` for `renderCreaturePortrait`?** Yes — the portrait function is unrelated to the aquarium renderer and is used by `web/src/eulogy.ts`. Phase 3 deletes only the aquarium-specific exports (`drawSunMap`, `drawCarrion`, `drawCreatures`, `drawAquariumFrame`, `renderWorld`). `RING_COLORS`, `Camera`, `makeCamera`, `clampCamera`, `worldToScreen`, `screenToWorld`, `PX_PER_SIZE`, `renderCreaturePortrait` all stay.

---

## 14. Related files

- `web/src/render.ts` — current Canvas2D renderer; the migration target.
- `web/src/camera.ts` — pan/zoom/pinch input handling; unchanged.
- `web/src/main.ts` — boot path, RAF loop, click handler; one call site swaps to `renderFn`.
- `web/src/eulogy.ts` — uses `renderCreaturePortrait`; unaffected.
- `src/wasm_api.rs` — `creatures_buffer`, `creature_stride`, `creature_ids_buffer`, `sun_buffer`, `sun_capacity_buffer`, `carrion_buffer`, `creature_at`. Read-only consumers.
- `docs/audit/big-wins.md` #1 — original motivation.
- `docs/plans/audit-triage.md` REJECT R10 — current scope decision (v5 §14 deferred; design doc authorized).
- `docs/plans/audit-master.md` §4 WebGL2 entry — points at this doc.
- `docs/plans/audit-s21-drop-unused-flag-floats.md` — stride change that affects §3.3.
- `docs/dev-server-prompt.md` — COOP/COEP already in place; not required for WebGL2 but does not block.

---

## Review feedback

**Reviewer:** opus, audit cross-review pass (2026-05-24).
**Verdict:** REVISE. Three blocking issues must be resolved before this doc is implementation-ready. Overall structure, scope discipline, and motivation are strong; the bugs are concentrated in §3.3-§3.5 (instance schema + shader math) and would mislead a future implementer.

### Blocking issues

**B1 — Post-S21 stride schema is wrong (severity: blocker).**
§3.3 ("Assuming the post-S21 layout") describes a 7-field record `[x, y, size, pigment_r, pigment_g, pigment_b, ring_mask (uint, packed from flags)]` and §3.3 footnote claims "S21 packs them into one mask. … if implementation begins before S21 lands, the planner should expand the layout to read the raw 13 floats and pack the mask in JS."

This contradicts `docs/plans/audit-s21-drop-unused-flag-floats.md` §2 ("After (stride = 11)") and §8 ("Acceptance criteria") explicitly: S21 ships an **11-float** stride with **five separate flag floats** (offsets 6..10), one per ring. S21 does NOT pack a `ring_mask`. The S21 doc §2 table is unambiguous on this point.

Consequence: a future implementer following §3.3 will allocate a `uint` instance attribute that the Rust packer never writes, read garbage, and `discard` every ring. The doc must either (a) describe the real 11-float layout with five `float` flag attrs (with the JS-side mask-pack happening in `bufferSubData` prep — adds a per-frame O(N) pack loop, defeating part of the "one upload" claim), or (b) explicitly add a new post-S21 follow-up piece "S21b: pack flags into a single u8 ring_mask field" and depend on it. Pick one; right now §3.3 references a layout that does not and will not exist.

Open question #1 ("Pre- or post-S21 layout?") is not a substitute for fixing this — both branches as written reference a `ring_mask` that S21 doesn't produce. Open question #10 ("reserve attribute slots for `eye_angle` / `eye_fov`") is also moot until the base layout is fixed.

**B2 — Ring-positioning parity break vs Canvas2D (severity: blocker).**
Canvas2D's `drawCreatures` (web/src/render.ts:158-173) packs only *active* rings: `ringR = radiusPx - 1`, then `ringR -= 1.5` per drawn ring. A creature with `flagEye=false, flagMove=true, flagScav=true` draws the move ring at `radiusPx - 1` (innermost slot), scav at `radiusPx - 2.5`. The active-ring set is densely packed inward.

The WebGL2 design §3.4 vertex shader computes ring radius from a fixed slot: `r_outer_world = body_radius_px - float(a_ring_index) * ring_step`. Disabled rings discard their fragments but leave a *gap*. Same creature: move ring drawn at slot 1 (`body_radius - 1.5`), scav at slot 2 (`body_radius - 3.0`). Visual mismatch on every creature that doesn't have all five rings active — which is most creatures most of the time.

This breaks goal 1 ("identical ring order … identical radii"). The fix is non-trivial: either compute per-creature "active ring count up to this slot" in the shader (requires popcount on a sub-mask, easy with `bitCount(mask & ((1u << ring_index) - 1u))` in GLSL 300 es — note: `bitCount` is in GLSL 4.00 desktop, in WebGL2 it requires checking; `findLSB`/`bitCount` are core in GLSL 300 es), or pre-compute packed offsets CPU-side and upload them. Either way, §3.4 needs a rewrite, and Risk R-WGL-1 ("AA softness") underplays the parity problem — this is a topological mismatch, not a sub-pixel one.

**B3 — Vertex/fragment shader inconsistency on body fill (severity: blocker).**
§3.4 comment: "body fill instance uses ring_index = N_RINGS-1 (largest quad)" — but the same shader computes `r_outer_world = body_radius_px - float(a_ring_index) * ring_step`. At `ring_index = 5` (N_RINGS-1, with N_RINGS=6), `r_outer = body_radius - 7.5`. That's the **smallest** quad, not the largest, and likely negative for small creatures. The body would be smaller than every trait ring, and for `size < 3` would have a negative radius.

Also the §3.5 fragment shader gates on `v_ring_index < 5u` for the ring-mask discard and uses `v_ring_index == 5u` for body, hardcoding the magic number `5` while the constant is `N_RINGS = 6u`. The mask only has 5 bits but `1u << v_ring_index` for `ring_index = 5` reads bit 5 (undefined for the 5-flag world). Body needs its own draw call or its own well-defined path through the shader; the current "draw body as an extra ring instance with special index" trick is half-implemented and incorrect.

### Non-blocking issues

**N1 — `divisor` semantics confusion (severity: major).** §3.3 says Buffer A uses `divisor = N_RINGS` and Buffer B uses `divisor = 1`. With `drawArraysInstanced(..., N_creatures * N_RINGS)`, that yields one creature per N_RINGS instances and one ring_index per instance — correct. But §3.3 also says "Buffer B (static, length = N_RINGS, uploaded once at init)". A divisor-1 attribute advances every instance; with `N_creatures * N_RINGS` instances total, you need `N_creatures * N_RINGS` ring indices in the buffer, OR you need to use `gl_InstanceID % N_RINGS` directly in the shader (no attribute needed) — either works, but as written Buffer B underflows after N_RINGS instances. The cleanest fix: drop Buffer B and use `int ring_index = gl_InstanceID % int(N_RINGS);` in the vertex shader. Mention this; the current text wastes a VBO and is wrong on size.

**N2 — `mat3` view-matrix uniform alignment (severity: minor).** §5.1 claims "one mat3 uniform upload per frame (~36 bytes)". WebGL2 `mat3` is column-major with each column padded to vec4 — so it's actually 48 bytes on the wire, and `uniformMatrix3fv` requires a 9-element Float32Array. Trivial, but the "~36 bytes" misleads about UBO sizing if anyone tries to pack it.

**N3 — `R32F` texture filterability (severity: minor).** §6.1 uses `R32F` for the sun grid. In WebGL2 base spec, `R32F` is renderable but *not* filterable without the `OES_texture_float_linear` extension. The sun grid uses `texelFetch` (effectively nearest), so this is fine — but the doc should state "we sample with `texelFetch` / `NEAREST`" so an implementer doesn't try `LINEAR` filtering and silently fall back to NEAREST or fail on conformant drivers. `R16F` is filterable and 1.6 KB is small; switching is also fine.

**N4 — `precision highp float` portability (severity: minor).** §3.4 declares `precision highp float;` but says nothing about `highp int` (which `uint a_ring_mask` requires). WebGL2 requires highp support in fragment shaders for `#version 300 es`, but it's worth a one-line statement.

**N5 — Premultiplied alpha decision still open (severity: minor).** Open question #8 and Risk R-WGL-7 both flag the straight-vs-premultiplied choice but don't resolve it. The doc should pick one and bake it into §3.5 — leaving this to "the implementation should commit to one mode at the start" pushes a design decision into the implementation pass, which is what this doc is supposed to prevent.

**N6 — `creatures_buffer()` view freshness (severity: minor, but well-flagged).** Risk R-WGL-6 correctly identifies the WASM-memory-growth staleness hazard. Good. One reinforcement: the doc should explicitly say "do not store the Float32Array returned by `creatures_buffer()` as a `render_gl.ts` module-level binding"; the temptation to cache for the `bufferSubData` upload path is real. A one-liner in §3.2 would be sufficient.

**N7 — Click-tolerance world units assertion (severity: trivial).** §7.1 says "click-tolerance argument is in world units" and "that calculation is unchanged". Good — but cite the actual file/line (`web/src/main.ts` around the click handler) so the implementer doesn't hunt for it. The doc is otherwise excellent at file:line citations; this one slipped.

**N8 — Highlight-pass shader reuse (severity: minor).** §7.2 says the highlight pass "uses the same SDF shader as the trait rings, parameterized for the golden color and 2px width". The trait-ring shader as written in §3.5 reads `a_ring_mask` and `a_ring_index` and indexes a `RING_COLORS` array — none of which apply to highlights. Either spell out the uniform/define overrides for the highlight variant, or accept that highlights need a sibling shader (small; ~30 lines). Currently underspecified.

**N9 — Tessellation strategy: AA at extreme zoom (severity: minor).** §4 dismisses N-quad-per-ring as inferior. Agreed for the body and outer ring, but at zoom 16× with `RING_STEP = 1.5` world units, the 1-pixel inner-vs-outer-radius difference becomes 24 device pixels and SDF AA is fine. The §4 fragment cost analysis ignores **early-z / `discard` cost on tiled GPUs** (mobile Safari): `discard` defeats early-z on PowerVR/Mali, so the cost per quad at high zoom is the full bounding-quad area times all overlapping creatures, not just the touched fragments. Doesn't change the recommendation, but the fragment budget table in §4 is optimistic by a factor of 2-3× on mobile. Worth a sentence.

**N10 — Phase 1 manual QA is the only parity gate (severity: minor).** §9 / §11 lean entirely on "open two tabs and eyeball them". For visual parity that's fine, but for the ring-positioning issue (B2 above) eyeballing misses a half-creature-off ring radius at zoom 1×. Recommend adding: "Capture a screenshot at fixed seed + fixed tick (use the existing acceptance-test seed and `?paused=1`), diff via a pixel-comparison tool with documented epsilon." This is one-time test infra; small. Without it, B2-class bugs ship.

**N11 — Effort estimate optimistic (severity: minor).** §11 estimates 4-6 engineering days. With B1, B2, and B3 fixed (ring-pack offsets, mask packing in JS or new Rust piece, body draw path, highlight shader variant, mobile-Safari AA validation, pixel-diff QA harness), realistic is 6-9 days. Not a doc-quality issue; just a planning input.

### Strengths (for the record)

- §1 quantifies the per-frame cost with file:line citations to the exact Canvas2D operations being eliminated. The "why WebGL2 fixes this" bullets are accurate.
- §2's goals/non-goals are crisp; WebGPU deferral and click-handling preservation are correctly scoped.
- §8 fallback path is concrete and complete; the boot-time-only detection is the right call.
- §9 progressive migration (opt-in → default → delete with `renderCreaturePortrait` carve-out) is exactly the right shape for a renderer swap.
- Risks §10 are real and well-categorized (R-WGL-5 stride coupling, R-WGL-6 view staleness are particularly important).
- §13 open questions are mostly meaningful (1, 3, 4, 6, 9, 10 are real design choices for next pass). Q5, Q7, Q11 are confirmations rather than open questions and could be deleted or marked "answered: confirm during QA".
- Scope discipline holds throughout: nowhere does the doc say "implement now". The "design-only" framing in §1 and §12 is honored.
- §14 related-files list is thorough.

### Severity summary

| Issue | Severity | Blocks impl? |
|---|---|:---:|
| B1 post-S21 stride mismatch | blocker | yes |
| B2 ring-positioning parity break | blocker | yes |
| B3 body-fill shader inconsistency | blocker | yes |
| N1 divisor / ring-index buffer | major | no (impl will discover) |
| N2 mat3 alignment | minor | no |
| N3 R32F filterability | minor | no |
| N4 highp int | minor | no |
| N5 premultiplied alpha unresolved | minor | no |
| N6 view caching warning | minor | no |
| N7 cite click-handler line | trivial | no |
| N8 highlight shader spec | minor | no |
| N9 mobile early-z / discard | minor | no |
| N10 pixel-diff QA harness | minor | no |
| N11 effort estimate | minor | no |

### Recommended next step

Author revises §3.3, §3.4, §3.5 to either (a) match the real S21 11-float / five-flag-float layout and pack the mask CPU-side, or (b) propose a follow-up S21b Rust piece to introduce the packed mask, and update the dependency graph. Resolve B2 with a ring-pack offset scheme (either popcount-in-shader or CPU-side packed offsets) and §3.5's body path. Then this doc is implementation-ready.

---

## Plan-update changelog (2026-05-24)

**Conflict resolution — B1, B2, B3 from WebGL2 review.**

**B1 resolved — §3.3 instance-attribute schema rewritten.**
- Replaced the invented 7-float / `ring_mask` layout with the actual S21 stride-11 layout.
- Full per-field table now lists all 11 floats in order: `x, y, radius_world, pigment_r, pigment_g, pigment_b, flag_eye, flag_move, flag_scav, flag_mouth, flag_armor`.
- `ring_mask` uint is gone. Ring flags are consumed as raw float attributes (`a_flag_eye > 0.5`), matching Canvas2D convention exactly.
- Noted that future micro-perf passes may introduce a packed `ring_mask`; today's design does not.
- Dropped the two-VBO (Buffer A + Buffer B ring-index buffer) scheme. Ring index is now computed via `gl_InstanceID % N_RINGS` in the vertex shader — no extra VBO, no underflow hazard (resolves review N1 as well).
- Added explicit warning: do not cache the `Float32Array` from `world.creatures_buffer()` across frames (R-WGL-6 reinforcement).

**B2 resolved — Ring-positioning parity, CPU-pack approach (option a) chosen.**
- §3.3 now documents the CPU active-ring pack pass: before `bufferSubData` each frame, an O(N) JS loop assigns sequential active-slot indices (0, 1, 2, …) to active trait flags and -1 to inactive ones. These are uploaded as five `a_ring_slot[]` float attributes (divisor = N_RINGS) in Buffer A.
- §3.4 ring-pass vertex shader rewritten to read the pre-computed `a_slot_*` attribute for the current `ring_index = gl_InstanceID % N_RINGS`, compute `r_outer_px = body_radius_px - 1.0 - float(active_slot) * RING_STEP`, and emit a degenerate quad (all vertices at origin) for inactive rings. Fragment shader discards via `v_ring_trait < 0`.
- This exactly reproduces Canvas2D's `ringR = radiusPx - 1; ringR -= 1.5 per drawn ring` active-ring-dense-pack behavior.
- Per-frame O(N) CPU cost noted; accepted as equivalent to the Canvas2D ring-dispatch loop it replaces.

**B3 resolved — Body fill routed through separate draw call (not ring shader).**
- Removed the broken `ring_index = N_RINGS - 1` body-as-ring trick from §3.4.
- Added a dedicated body-pass vertex shader and body-pass fragment shader in §3.5.
- §3.2 data-flow draw order updated: pass 5a = body draw (N_creatures instances), pass 5b = ring draw (N_creatures × N_RINGS instances). Body executes first so the opaque fill composites correctly under semi-transparent rings.
- `RING_COLORS` array in ring-pass fragment shader is now 5 elements (traits only); no body sentinel entry.
- Magic number `5u` in fragment shader replaced by correct 5-trait count with `v_ring_trait < 0` as the inactive sentinel.

**Other minor fixes applied.**
- §1 motivation: updated "13-float" to note current vs post-S21 stride.
- §3.2 draw-pass list: split pass 5 into 5a (bodies) and 5b (rings).
- Open question #1: marked RESOLVED with the post-S21 layout decision and pre-S21 fallback instructions.
- Open question #10: updated note that eye-cone attribute slots would append to Buffer A alongside `a_ring_slot[]`.
- R-WGL-5: updated expected stride to 11 (post-S21).
- §11 effort estimate: revised upward to 6–8 days to account for CPU pack loop, two-shader creature pass, and pixel-diff QA harness.
