# ui-stats — top-left Stats widget

**Status:** plan only. One commit: `feat(ui): top-left stats widget`.
Reads from master plan `docs/plans/perf+ui-master.md` §2 (ui-stats row),
§3 (UI-pieces vs each-other dep notes), §5 (ui-stats piece briefing),
§6 R2 / R3 / R8 (cross-piece risks), §8 Resolved decisions
(A1: right-rail stays in DOM, hidden).

This piece is independent of all perf pieces and of `ui-inspector` /
`ui-perf`. Coordination contract per §6 R2: this plan owns the chart
drawing + `maybeSampleStats` + `#chart-pop` / `#chart-species`; the
`installProfilerPanel` block and all `#profiler-*` IDs are owned by
`ui-perf`. Coordination contract per §8.1: this plan does **not** delete
the rail; it hides it via CSS and evicts the two chart canvases.

---

## 1. Decisions (pinned up-front)

These are the choices a Sonnet implementer should treat as non-negotiable
unless they discover the codebase contradicts them.

1. **Container ID + position.** `<div id="stats-box" class="overlay-widget">`,
   `position: fixed; top: 8px; left: 8px;`. Sits flush to the top-left
   corner. Above the `#aquarium` canvas (`z-index: 5` to match
   `#right-rail`); below the modal backdrops (`z-index: 30`) and
   `#autosave-toast` (`z-index: 20`). No overlap with `#top-bar`
   because `#top-bar` is `pointer-events: none` over a transparent
   background gradient — `#stats-box` sits visually under the top-bar
   gradient on the far left, which is acceptable (the status text grows
   to the right, the seed pill is further right still).

2. **Move the canvases (do NOT keep them in `#rail-stats`).** Cut
   `<canvas id="chart-pop">` and `<canvas id="chart-species">` out of
   `<section id="rail-stats">` and paste them into `#stats-box`. IDs
   stay; the existing `getCanvases()` in `web/src/rail/stats.ts` finds
   them by ID regardless of DOM ancestry. `<section id="rail-stats">`
   stays in HTML as an empty stub (per §8.1).

3. **Hide the rail via CSS, do not delete it** (per §8.1).
   `#right-rail { display: none; }`. Preserve every rail-related
   element and module so a v1.1 events re-enable is cheap.

4. **Keep `stats.ts` where it is** (`web/src/rail/stats.ts`). Do NOT
   move it to `web/src/widgets/stats.ts`. Rationale: minimum-churn,
   no import-path churn in `main.ts`, and the file will be carved up
   by `ui-perf` anyway (which removes the profiler block). Moving the
   file in this commit and again in `ui-perf` would create needless
   rename noise across the two PRs. The cross-reviewer should confirm
   ui-perf does not also rename this file.

5. **Polling stays in `pollRail()`** (option (a) from the brief). The
   `maybeSampleStats(world)` call at `rail/index.ts:115` is unchanged —
   it draws into the canvases by ID, not by DOM tree position, so it
   continues to work after the canvases move out of the (hidden) rail.
   No new RAF loop, no new interval, no new `installStatsBox(world)`
   function.

6. **No new chart library, no framework, no styling lib.** Pure DOM +
   CSS + the existing Canvas2D draw code.

7. **No Rust changes.** This piece is HTML/CSS/TS only.

---

## 2. HTML diff (`web/index.html`)

Concrete diff, line-anchored against the file at HEAD
(`web/index.html:1–78` snapshot in §5 of the master plan).

```diff
   <body>
     <canvas id="aquarium"></canvas>
 
     <div id="top-bar">
       <span id="status">booting…</span>
       <span id="seed-display" class="seed-pill" title="Click to copy seed">
         <span id="seed-value">…</span>
         <button id="seed-copy-btn" aria-label="Copy seed">copy</button>
       </span>
       <span id="save-indicator" aria-live="polite" aria-label="Saving…">saving…</span>
     </div>
 
+    <!-- ui-stats: free-standing Stats widget, top-left, always visible.
+         Owns #chart-pop and #chart-species (evicted from #rail-stats). -->
+    <div id="stats-box" class="overlay-widget">
+      <canvas id="chart-pop" width="260" height="80"></canvas>
+      <canvas id="chart-species" width="260" height="80"></canvas>
+    </div>
+
     <!-- E.21: right rail shell -->
+    <!-- v1.1: rail hidden via CSS (#right-rail { display: none; }) per
+         master plan §8.1; DOM kept for future re-enable. Stats charts
+         moved out into #stats-box (see ui-stats plan). -->
     <aside id="right-rail">
       <nav id="rail-tabs" role="tablist">
         <button data-tab="stats" class="rail-tab is-active">Stats</button>
         <!-- Inspector tab injected dynamically when a creature is selected -->
         <!-- Events tab hidden for v1.1 revisit -->
       </nav>
 
       <!-- Events panel hidden for v1.1 revisit -->
       <section id="rail-events" class="rail-panel" style="display:none"></section>
       <section id="rail-stats" class="rail-panel is-active">
-        <canvas id="chart-pop" width="260" height="80"></canvas>
-        <canvas id="chart-species" width="260" height="80"></canvas>
         <div id="profiler-panel">
           <label class="profiler-toggle">
             <input type="checkbox" id="profiler-enable" />
             profiler
           </label>
           <div id="profiler-stabilizing" class="profiler-stabilizing" style="display:none"></div>
           <table id="profiler-table" class="profiler-table">
             <thead>
               <tr>
                 <th>Stage</th><th>total ms</th><th>calls</th>
                 <th>/call us</th><th>calls/par</th><th>share %</th>
               </tr>
             </thead>
             <tbody id="profiler-tbody"></tbody>
           </table>
         </div>
       </section>
       <section id="rail-inspector" class="rail-panel">
         …  (untouched — ui-inspector evicts this in its own commit)
       </section>
     </aside>
```

**What changes:**
- Two new lines for the `<div id="stats-box">` block (insert after
  `</div>` of `#top-bar`, before `<aside id="right-rail">`).
- Two lines removed: the `<canvas>` elements inside `#rail-stats`.
- Two comment-only additions for future readers.

**What does NOT change:**
- The `#profiler-panel` block inside `#rail-stats` stays in HTML in
  this commit; `ui-perf` is responsible for evicting it into a
  free-standing `#perf-box`. Until `ui-perf` lands, that profiler
  block is dead HTML (the rail is `display: none`, so it is
  invisible; `installProfilerPanel` still binds to `#profiler-enable`
  and would still work if the rail were unhidden, but the user has no
  way to interact with it).
- `<section id="rail-inspector">` block stays in HTML. ui-inspector
  evicts it in its own commit.
- `#right-rail`, `#rail-tabs`, `#rail-events`, `#rail-stats`,
  `#rail-inspector` IDs all remain (§9 DOM-ID contract below).
- `#toast-stack`, `#top-bar`, `#status`, `#seed-display`,
  `#seed-value`, `#seed-copy-btn`, `#save-indicator`, `#aquarium`
  unchanged.

---

## 3. CSS additions (`web/src/styles.css`)

Three changes, all additive plus one one-line rule to hide the rail.

### 3a. New `.overlay-widget` base class + `#stats-box` rules

Append (or insert near the existing `#top-bar` block at lines 33–48 for
visual proximity in the file):

```css
/* ui-stats: free-standing overlay widget convention used by the three
   evicted-from-rail boxes. Default pointer-events: auto so opaque
   widgets (ui-inspector, ui-perf) capture clicks correctly without any
   override. ui-stats and ui-perf set pointer-events: none on their own
   outer IDs because they are non-interactive display surfaces that must
   not block camera-drag events on their padded margins; ui-inspector
   inherits auto (no override needed). */
.overlay-widget {
  position: fixed;
  padding: 8px 10px;
  background: var(--rail-bg);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 4px;
  font: 12px ui-monospace, monospace;
  color: var(--fg);
  z-index: 5;
  pointer-events: auto;
}

/* ui-stats: top-left, under the top-bar gradient.
   pointer-events: none so the padded box area doesn't block
   camera-drag events; the chart canvases are pure render targets that
   don't need click handling, so no child opt-back is required. */
#stats-box {
  top: 8px;
  left: 8px;
  pointer-events: none;
}
#stats-box canvas {
  display: block;
  margin-bottom: 6px;
}
#stats-box canvas:last-child {
  margin-bottom: 0;
}
```

Values picked to mirror existing rail styling:
- `background: var(--rail-bg)` — the same `rgba(15, 22, 32, 0.85)` used
  by `#right-rail` (styles.css:57) so the visual weight matches.
- `border: 1px solid rgba(255, 255, 255, 0.08)` — same alpha as the
  rail's left border (styles.css:58).
- `font: 12px ui-monospace, monospace` — matches the rail base font
  size (12px from styles.css:60) and the chart-internal monospace
  labels drawn by `drawChart`.
- `padding: 8px 10px` — matches `.rail-panel` padding (styles.css:92).
- `z-index: 5` — same layer as `#right-rail` (styles.css:62). Modals
  at 30, autosave toast at 20, toast-stack at 10 all sit above.
- **pointer-events policy (M4/M5 resolution):** `.overlay-widget` base
  is `pointer-events: auto` so opaque widgets like ui-inspector inherit
  click-capture without an override. `#stats-box` opts back to `none`
  because its chart canvases are pure render targets; no child opt-back
  is needed. ui-perf likewise sets `pointer-events: none` on `#perf-box`
  for the same reason. ui-inspector inherits `auto` from the base class
  and adds no pointer-events override.

### 3b. Hide the rail (per §8.1)

Replace the existing `#right-rail { … }` block (styles.css:51–64) with
a single rule, OR add a one-line override after it. Recommend the
override pattern so the original rule survives intact for the future
re-enable:

```css
/* v1.1: rail hidden by default (master plan §8.1).
   DOM stays so installRail/pollRail and the inspector tab-injection
   code path don't break; re-enable by removing this rule. */
#right-rail {
  display: none;
}
```

Add this directly **after** the existing `#right-rail` block
(currently at styles.css:51–64) so it wins on cascade order. Do not
delete the original block — it stays as the future-enabled style.

### 3c. (Optional) `#toast-stack` adjustment

Toast stack currently positions `right: 296px` so it sits just left of
the 280px rail (styles.css:104). With the rail hidden, the toasts
will float in empty viewport space at `right: 296px`, which is fine —
they appear about a rail-width from the right edge. **Do not change
this in the ui-stats commit.** It is `ui-inspector`'s call whether to
adjust toast placement (since the inspector box will sit top-right and
may want the toasts elsewhere). Document in this plan that ui-stats
leaves `#toast-stack` alone.

---

## 4. TypeScript changes (file-by-file)

### 4a. `web/src/rail/stats.ts` — **no changes in this commit**

The two chart canvases moved in the DOM, but `getCanvases()` looks
them up by ID (`stats.ts:23–24`), so the lookup still works. The
`drawCharts` / `drawChart` / `maybeSampleStats` exports are unchanged.
The profiler-panel half of this file (lines 101–270) is owned by
`ui-perf` and gets carved out in that commit, not this one.

### 4b. `web/src/rail/index.ts` — **no changes in this commit**

`pollRail` continues to call `maybeSampleStats(world)` at line 115.
The other four pollRail steps (species cache, inspector refresh,
toast tick, highlight prune) are unaffected by ui-stats and continue
to run every frame even though their visible outputs (the rail panels)
are now hidden. They become dead-but-cheap; ui-inspector and ui-perf
will decide their fate.

`installTabs()` (lines 41–66) and the `rail-tab` click handlers
continue to attach to DOM that is now invisible. Per §6 R3 and §8.1,
do NOT delete this code — `ensureInspectorTab` in
`web/src/rail/inspector.ts` calls `rail.switchTab("inspector")` and
would break if `RailState.switchTab` disappeared. ui-inspector will
rewire that path; ui-stats must not.

### 4c. `web/src/main.ts` — **no changes in this commit**

`installRail(world)` continues to be called (line 242) and continues
to return a `RailState` used by the inspector click handler (line 245)
and `pollRail` (line 283). `installProfilerPanel(world)` continues to
be imported from `./rail/stats` and called (lines 5, 252) — that's
`ui-perf`'s migration, not this one.

### 4d. Net TS LoC delta for this commit: **0**

This commit is HTML-and-CSS only on the frontend side. That is the
correct shape — the chart drawing code already finds its canvases by
ID, so re-parenting the canvases is a pure markup change.

---

## 5. Integration points (what calls what after the commit)

```
main.ts
  └── installRail(world)              [unchanged: returns RailState,
                                       wires hidden-rail tab clicks]
  └── installProfilerPanel(world)     [unchanged: binds #profiler-enable,
                                       still in hidden #rail-stats —
                                       ui-perf will relocate]
  └── installCanvasClickHandler(...)  [unchanged: still calls
                                       rail.switchTab("inspector") —
                                       ui-inspector will rewire]
  └── frame loop
       └── pollRail(rail, world, ids)
            ├── speciesNameCache poll       [updates hidden DOM, dead-but-cheap]
            ├── maybeSampleStats(world)     [draws into now-relocated canvases]
            ├── refreshInspector(...)       [updates hidden DOM until ui-inspector]
            ├── tickToasts(now)             [visible — toast-stack still on-screen]
            └── pruneHighlights(now)        [visible — overlay on aquarium]
```

The visible UI surface after this commit:
- `#aquarium` (canvas, full viewport)
- `#top-bar` (status + seed pill + save indicator + speed buttons)
- `#stats-box` (top-left, two charts) ← **new**
- `#toast-stack` (top-right area, ~rail-width from edge)
- Modals (eulogy, resume prompt, schema-mismatch) when fired

The hidden UI surface (DOM lives, `display: none`):
- `#right-rail` and all its children (tabs, panels, inspector dl,
  profiler panel until ui-perf moves it)

---

## 6. Tests / manual verification

There are no automated UI tests in this repo.

**Required green check.** `pnpm --filter ./web build` (= tsc + vite
build) MUST be clean. No new TS files means no new tsc work, but the
HTML/CSS edits go through vite's HTML pipeline; a typo there will
fail the build.

**Manual verification recipe** (the implementer must run this before
declaring the commit done):

1. `pnpm --filter ./web build` is clean.
2. `pnpm --filter ./web dev` (or whatever the dev script is —
   confirm by reading `web/package.json`) and load
   `http://localhost:47821/` (port per `BUILD-REPORT.md` /
   `web/vite.config.ts`).
3. **Top-left widget present.** A semi-transparent dark box appears
   in the top-left corner with two empty chart placeholders showing
   `pop: waiting…` and `species: waiting…` within ~1 second.
4. **Charts populate.** Within ~10 sim ticks (the sample interval),
   the population line appears in the top canvas; the species line
   appears in the bottom canvas. Both update as the sim runs.
5. **Rail is gone.** No right-rail visible. Right side of the
   viewport is empty aquarium canvas (modulo toasts).
6. **Inspector click still injects a tab into the (hidden) rail
   without crashing.** Click any creature on the canvas. The
   inspector tab-injection code runs (no console error); the
   inspector data is updated in the hidden DOM but not visible. This
   is the expected pre-ui-inspector state. After ui-inspector lands,
   clicking opens the top-right `#inspector-box` instead.
7. **No console errors.** Open devtools console; no errors during
   boot, no errors when ticking, no errors when clicking a creature.
8. **Top bar still works.** Seed pill copies on click; speed buttons
   switch sim speed; save indicator pulses every 5 minutes; pop / tick
   text updates.
9. **Toasts still appear** (e.g. trigger one by causing an autosave
   failure — easiest is to disable IndexedDB in devtools). Toast
   stack is in the top-right region of the viewport, ~280px from the
   right edge (no change from today).
10. **Eulogy still fires** when the population dies out. (Not easy to
    trigger manually in 10 seconds — implementer can use a tiny seed
    that crashes early, or just confirm the `showEulogyCard` import
    in `main.ts` is unchanged.)

---

## 7. Risks

**R-S1. Hidden rail breaks an unnoticed dependency.** Something
non-obvious might call `getBoundingClientRect()` on a rail child and
get a zero-size rect, then divide by it. Mitigation: the existing
codebase doesn't appear to do this for the rail (the inspector and
events readers all set `textContent` / `innerHTML`, never measure).
The implementer should grep `getBoundingClientRect`, `offsetWidth`,
`offsetHeight`, `clientWidth`, `clientHeight` against the rail IDs
to confirm.

**R-S2. `#stats-box` overlaps the status text on narrow viewports.**
On a viewport narrower than ~280px, the 260px-wide chart canvases
will visually overlap or push past the status text. Acceptable for
this commit — the target deployment is desktop browsers; the existing
rail had the same issue at 280px wide. If mobile is needed later,
the implementer can revisit with a `@media (max-width: 600px)` rule
that hides the stats box or shrinks the canvases. Not in scope.

**R-S3. Top-bar gradient overlaps the box visually.** `#top-bar` is
a transparent gradient from `rgba(0,0,0,0.55)` at top to transparent
at bottom (styles.css:43). It sits at `top: 0; left: 0; right: 0`
with `pointer-events: none`. The `#stats-box` at `top: 8px; left: 8px`
will be drawn over by the top-bar gradient on its top edge. Visual
result is a subtly darker tint on the top ~30px of the stats box.
Acceptable; if cosmetically jarring, the implementer can drop the
top-bar's bottom-fade by adjusting the linear-gradient, but **do not
do this in this commit** — it's a separate aesthetic call.

**R-S4. Inspector tab-injection collides with hidden rail.**
`ensureInspectorTab` in `web/src/rail/inspector.ts` injects a new
`.rail-tab` button into `#rail-tabs`. Since the rail is
`display: none`, the new tab is invisible too — the user can't click
it, the inspector data never becomes visible. Expected and correct
until ui-inspector lands; confirm no console errors and that the
inspector code path doesn't crash. Test step §6.6 covers this.

**R-S5. Z-order conflict with future ui-inspector / ui-perf boxes.**
Per §6 R8, all three UI boxes use `z-index: 5`. They are positioned
in disjoint viewport corners (top-left, top-right, bottom-left) so
they never overlap. No fix needed in this commit; the cross-reviewer
must check that ui-inspector picks top-right and ui-perf picks
bottom-left so the no-overlap invariant holds.

---

## 8. Sequencing inside this commit

The commit is small enough to land as one atomic change. Suggested
authoring order for the implementer:

1. Edit `web/index.html` (move canvases, insert `#stats-box` div,
   add comments). Save.
2. Edit `web/src/styles.css` (add `.overlay-widget` + `#stats-box`
   block; add the `#right-rail { display: none; }` override). Save.
3. Run `pnpm --filter ./web build`. Should be green.
4. Run `pnpm --filter ./web dev` and walk through §6 manual recipe.
5. Commit with message:
   `feat(ui): top-left stats widget`
   Body:
   - moves #chart-pop / #chart-species out of #rail-stats into a
     free-standing #stats-box (top-left, always visible)
   - hides #right-rail via CSS per master plan §8.1; DOM stays for
     v1.1 re-enable
   - no Rust changes; no new deps; pnpm build green

---

## 9. DOM ID stability contract

After this commit, the following IDs MUST exist in the DOM (some
visible, some hidden via `display: none`) so other modules don't
crash:

**Owned-and-relocated by ui-stats** (now inside `#stats-box`):
- `#chart-pop` — read by `getCanvases()` in `rail/stats.ts:23`.
- `#chart-species` — read by `getCanvases()` in `rail/stats.ts:24`.

**New by ui-stats:**
- `#stats-box` — the new container.

**Owned by other UI pieces, must not be touched in this commit:**
- `#profiler-panel`, `#profiler-enable`, `#profiler-stabilizing`,
  `#profiler-table`, `#profiler-tbody` — owned by ui-perf.
- `#rail-inspector`, `#ins-species`, `#ins-parent`, `#ins-action`,
  `#ins-age`, `#ins-energy`, `#ins-size`, `#ins-photo`, `#ins-eat`,
  `#ins-scav`, `#ins-move`, `#ins-eyes`, `#ins-vision`, `#ins-armor`,
  `#ins-bite`, `#ins-pigment` — owned by ui-inspector.

**Kept hidden by ui-stats (per §8.1), must not be deleted:**
- `#right-rail` — the rail shell.
- `#rail-tabs` — tab nav, used by `installTabs` in `rail/index.ts:41`.
- `#rail-events` — empty stub, kept for v1.1 events re-enable.
- `#rail-stats` — empty stub after canvases move out (still contains
  the profiler block until ui-perf moves it).
- `.rail-tab` class (on the Stats tab button + the dynamically
  injected Inspector tab) — used by `switchTab` and inspector click
  handler.
- `.rail-panel` class on the three `<section>` elements — used by
  `switchTab`.

**Global UI IDs unchanged:**
- `#aquarium`, `#top-bar`, `#status`, `#seed-display`, `#seed-value`,
  `#seed-copy-btn`, `#save-indicator`, `#toast-stack`.

---

## 10. Constraints recap

- `pnpm --filter ./web build` MUST be clean (tsc strict + vite).
- No new npm deps.
- No new chart library, no framework, no styling library.
- No Rust changes; determinism unaffected (pure frontend).
- Toast container, save indicator, top-bar, eulogy must all continue
  to function (covered by §6 manual recipe).
- Right-rail DOM preserved (per §8.1) so future v1.1 re-enable is
  cheap.
- The `installProfilerPanel` migration is NOT in this commit — it is
  owned by ui-perf (§6 R2).
- The inspector click handler / tab injection are NOT touched in this
  commit — they are owned by ui-inspector.

---

## 11. Citations

- Master plan: `docs/plans/perf+ui-master.md` §2, §3 (ui-pieces dep
  notes), §5 ui-stats briefing, §6 R2 (chart vs profiler split), R3
  (pollRail responsibilities), R8 (z-order), §8.1 (rail stays
  hidden).
- HTML source-of-truth: `web/index.html:23–71` (current rail) and
  `web/index.html:32–34` (current canvas locations).
- Stats TS: `web/src/rail/stats.ts:22–41` (getCanvases +
  maybeSampleStats — proves ID-based lookup) and `stats.ts:101–270`
  (profiler block owned by ui-perf, not this plan).
- Rail TS: `web/src/rail/index.ts:41–66` (installTabs — must not
  delete per R3) and `rail/index.ts:92–123` (pollRail — unchanged in
  this commit).
- Main entry: `web/src/main.ts:5` (installProfilerPanel import — stays
  pointing at `./rail/stats` for now), `main.ts:242–252` (rail +
  profiler install — unchanged), `main.ts:283` (pollRail call —
  unchanged).
- CSS source-of-truth: `web/src/styles.css:51–64` (rail block —
  hide-override goes immediately after this), `styles.css:33–48`
  (top-bar — pointer-events pattern to mirror), `styles.css:177–181`
  (existing `#rail-stats canvas` rule — superseded by the new
  `#stats-box canvas` rule).
- v5 §11 (Stats panel intent — population + species charts).
- v6 §A (DOM-only, no framework).

---

*End of ui-stats plan.*

---

## Code review

**Verdict: APPROVED.** Commit `0d9b726` implements the plan exactly as
written.

Verified:
- All required DOM IDs present in `web/index.html` (`chart-pop`,
  `chart-species`, `right-rail`, `rail-stats`, `rail-events`,
  `rail-inspector`, `aquarium`, `top-bar`, `status`, `seed-display`,
  `seed-value`, `seed-copy-btn`, `save-indicator`, `toast-stack`).
- `#stats-box` exists with `class="overlay-widget"` and both canvases
  inside (`web/index.html:24–27`).
- `.overlay-widget` base class defined with `pointer-events: auto`
  (`web/src/styles.css:50–66`) per cross-review M4/M5.
- `#stats-box { pointer-events: none }` override present
  (`web/src/styles.css:71–75`).
- `#right-rail { display: none }` override added after the original
  rail block (`web/src/styles.css:101–106`); original rail styling
  preserved.
- Z-order: stats-box=5, toast-stack=10, autosave-toast=20,
  modal-backdrop=30. Eulogy uses `modal-backdrop eulogy-backdrop` so
  it inherits z-index 30. Stats sits cleanly below all overlays.
- No Rust changes (`git show 0d9b726 -- src/ tests/ Cargo.toml` empty).
- No TS changes (`git show 0d9b726 -- web/src/*.ts web/src/**/*.ts`
  empty). Net TS LoC delta: 0, matches plan §4d.
- `pnpm build` clean (tsc strict + vite build, 431ms).
- `cargo test --release --test acceptance` — 3/3 passing.

Not assessed (deferred to user per plan §7 R-S2 / R-S3): visual layout,
narrow-viewport overlap, top-bar gradient interaction.

---

## Revision history

- **v1 (initial plan):** `.overlay-widget` set `pointer-events: none`
  globally with child opt-back via `> *`. Per-piece review approved this
  version as-is.
- **v2 (cross-review M4/M5):** Inverted pointer-events policy.
  `.overlay-widget` base now `pointer-events: auto` so opaque widgets
  (ui-inspector) capture clicks without an override. `#stats-box` opts
  back to `pointer-events: none` (chart canvases are pure render
  targets; no child opt-back needed). ui-perf mirrors the same
  `#perf-box { pointer-events: none }` pattern. ui-inspector inherits
  `auto` and adds no override. See `docs/plans/perf+ui-cross-review.md`
  §2 M4/M5 for rationale.

