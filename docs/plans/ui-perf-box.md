# Plan — ui-perf: extract Perf-timing widget as a free-standing box

**Status:** plan only. No code lands until this is signed off.
**Scope:** Move the profiler panel (checkbox + stabilizing banner + 7-column
table) out of `#rail-stats` into a free-standing, always-visible widget at
the bottom-left of the viewport. Widen the container so columns no longer
truncate. Extract the renderer/poller from `web/src/rail/stats.ts` into a
new module. Zero Rust changes, zero new deps, no change to the profiler
JSON contract, no change to the seven (six visible) column definitions.

**Master plan row:** `ui-perf` — see
`docs/plans/perf+ui-master.md` §2, §5 ("ui-perf — bottom-left Perf
widget"), §6 R2 (ui-stats/ui-perf inheritance split of `rail/stats.ts`),
§6 R8 (z-order), §8.1 (right-rail stays as hidden empty shell).

**Spec / handoff anchors (read these first):**

- `docs/plans/perf+ui-master.md` — master plan; UI track. Confirms ui-perf
  owns the migration of `installProfilerPanel` out of `web/src/rail/stats.ts`
  (R2 mandates ui-perf takes everything from line 101 onward of that file).
- `docs/plans/perf-timing.md` — full plan for the shipped profiler. The
  JSON shape (§D5) and the polling pattern are stable contracts; this
  plan does NOT change them. The code-review block at the end of that
  doc (lines 1240–1263, "Code review — FIXES REQUIRED") flags the
  table-too-thin bug; ui-perf is where that fix lands.
- `web/index.html` lines 35–50 — current `#profiler-panel` block to
  extract (lives inside `#rail-stats`).
- `web/src/rail/stats.ts` lines 101–270 — `installProfilerPanel`,
  `startPolling`, `stopPolling`, `clearTable`, `pollAndRender`,
  `renderProfilerTable`, `escHtml`, and the `ProfNode`/`ProfReport`
  interfaces. This entire block moves to the new module.
- `web/src/perf.ts` — the TS-side profiler instrumentation. Exports
  `attachProfiler`, `setProfilerEnabled`, `isProfilerEnabled`,
  `reportJson`, `span`, `timed`. **Unchanged in this plan.** Do NOT
  collide names with this file.
- `web/src/styles.css` lines 294–337 — current `.profiler-table`,
  `.profiler-toggle`, `.profiler-stabilizing`, `#profiler-panel`
  rules. The `table-layout: fixed` + `overflow: hidden; text-overflow:
  ellipsis; white-space: nowrap` combo at lines 318–326 is the root
  cause of the truncation. Replaced here.
- `web/src/main.ts` lines 247–252 — current call site for
  `installProfilerPanel(world)`. Import path is updated.
- `web/src/rail/index.ts` lines 12, 75–115 — `installRail` does NOT
  call `installProfilerPanel`; that wiring lives in `main.ts` (verified
  via `grep`). ui-perf updates only `main.ts`'s import path.

---

## High-level decisions (pinned)

These are the load-bearing choices. Record under `DECISIONS.md` (under
the existing `v1.1 UI` section the master plan establishes).

**D1. Position: bottom-left.** The widget pins to the bottom-left corner
of the viewport (`bottom: 8px; left: 8px`). Justification:

- The master plan §6 R8 / §3 inventory carves the viewport corners three
  ways: top-left = Stats, top-right = Inspector, bottom-* = Perf. That
  leaves bottom-left and bottom-right both free; pick one.
- Bottom-left wins because (a) the top-left Stats box and bottom-left
  Perf box stack vertically along the same edge, giving the user one
  continuous "instrumentation gutter" on the left and reserving the
  entire right edge for the (hidden by default) Inspector that pops in
  on creature click; (b) the existing `#toast-stack` is anchored to the
  upper right (`styles.css:101–110`) and on a sub-1440px viewport
  bottom-right would risk overlap with toast leaving-animations or any
  future bottom-right HUD; (c) the saving-toast / autosave-failure
  status toast lives top-center (`styles.css:238–251`) so neither
  corner conflicts there, but bottom-left also keeps the widget out of
  the visual path of the seed-pill copy interaction in the top bar,
  which sometimes triggers a clipboard permission prompt UI on the
  right.
- No data dependency on position — only the CSS file changes if a
  future revision flips left↔right.

**D2. Always visible.** The widget is rendered on page load and never
hidden. The profiler checkbox inside it remains UNCHECKED by default
(per `perf-timing.md` §D9: state never persists). When the checkbox is
off, the tbody is empty with a single-row hint "enable to record"
(see D5). Rationale: the user explicitly asked for "always visible for
now so the user can see timing data without flipping a tab". Hiding
the entire widget would require a second affordance (a button) to
re-show it; not in scope.

**D3. New TS module path: `web/src/widgets/perf-panel.ts`.** New file.
Path choice rationale:

- The existing `web/src/perf.ts` is the TS-side profiler
  instrumentation (span/timed/reportJson/setProfilerEnabled). A new
  file at `web/src/perf-panel.ts` would sit beside it and read fine,
  but the master plan §5 ("ui-perf — Bottom-left Perf widget", file
  list) suggests `web/src/widgets/perf.ts`. Both options collide
  spatially or namewise with the existing instrumentation file. The
  pinned compromise: a new `web/src/widgets/` directory (new for this
  pass; ui-stats / ui-inspector may create siblings) with the file
  named `perf-panel.ts` to disambiguate from `web/src/perf.ts`. The
  imported symbol is still `installProfilerPanel` for source
  continuity.
- `web/src/widgets/` is a fresh directory. ui-perf creates it; ui-stats
  and ui-inspector (per their plans) may move `widgets/stats.ts` and
  `widgets/inspector.ts` into it. No coordination problem — they touch
  disjoint filenames. The implementer creates the directory in this
  commit if it doesn't yet exist.

**D4. Widen and let columns size to content.** Replace
`table-layout: fixed` + `overflow: hidden; text-overflow: ellipsis`
with `table-layout: auto` and `white-space: nowrap`. The container
gets `min-width: 560px; max-width: 760px; overflow-x: auto`. At
560px the table sits comfortably (6 columns × ~12 chars of monospace
including padding ≈ 540px); at narrow viewports the container
horizontally scrolls inside its bounded width without wrecking the
canvas underneath. Rationale: matches the master plan §5 ui-perf
guidance ("widen the box and let it scroll horizontally if needed");
matches §"Current bug" in this plan's brief — columns must not
truncate.

**D5. Empty-state hint: render via TS in `renderProfilerTable`.** When
the merged tree is empty (`tree.length === 0`) OR every root has
`call_count === 0`, the renderer emits a single row
`<tr><td colspan="6" class="pf-empty-hint">enable to record</td></tr>`.
Rationale: keeps the HTML static (no extra placeholder DOM the
renderer would need to remove), and naturally covers both
"profiler-off" and "profiler-on but no samples yet" states with the
same hint. (The stabilizing banner handles the "warming up" case
separately.) Replaces the current `clearTable()` behavior of leaving
the tbody literally empty when the profiler toggles off.

**D6. DOM ID stability contract — preserved.** `profiler-enable`,
`profiler-stabilizing`, `profiler-table`, `profiler-tbody` all keep
their IDs and tag names. The new module reads them by ID exactly as
today. New stable ID: `perf-box` (the outer container). The old
parent ID `profiler-panel` is dropped (it served only as an
in-rail group container and has no readers in TS; verified via
`grep -rn "profiler-panel" web/`).

**D7. Default state: profiler OFF, widget visible, table shows hint.**
Per `perf-timing.md` §D9 the checkbox state never persists. Page load:
the widget is in the DOM and visible; the checkbox is unchecked; the
tbody shows the single "enable to record" row from D5; the
`#profiler-stabilizing` banner is `display:none`. Clicking the
checkbox triggers `setProfilerEnabled(true)`, starts the 1Hz polling
loop, and the table populates on the first poll (≤1s).

**D8. Polling cadence — unchanged.** `setInterval(1000)` while the
checkbox is checked; cleared via `clearInterval` when unchecked. The
existing `startPolling` / `stopPolling` / `pollAndRender` functions
move verbatim with their state (`profilerPollHandle`,
`POLL_INTERVAL_MS = 1000`). Per `perf-timing.md` §"reviewer
checklist": polling handle is 0 when disabled.

**D9. JSON contract — unchanged.** `world.profile_report_json()`
(Rust) and `reportJson()` (TS, from `perf.ts`) emit the shape pinned
in `perf-timing.md` §D5. The renderer parses both, replaces the
Rust-side "frame" placeholder with the live TS subtree (existing
logic at `stats.ts:184–194` moves verbatim), and walks the merged
tree. Six visible columns: Stage, total ms, calls, /call us,
calls/par, share %. No column added, removed, or reordered. Header
labels copied verbatim from `index.html:43–46`.

**D10. Pointer-events on the outer container.** The widget is `position:
fixed` at the bottom-left. To avoid swallowing canvas pointer events
on the transparent regions around the table (e.g. between the bottom
edge of the table and the `max-width: 760px` slot), set
`pointer-events: none` on `#perf-box`, then `pointer-events: auto`
on the interactive children — concretely `.profiler-toggle` (the
checkbox label) and `.profiler-table` (the data table).

This pattern is confirmed by the cross-piece review (M4/M5): the
`.overlay-widget` base class should NOT hard-code `pointer-events`
either way; each widget overrides on its own container. ui-stats sets
`pointer-events: none` on `#stats-box` (canvas-pan-through). ui-perf
sets `pointer-events: none` on `#perf-box` for the same reason. The
widget text/table area does not need to capture clicks except via the
toggle checkbox; all other regions pass events through to the canvas.

Concrete CSS:

```css
#perf-box {
  pointer-events: none;
}
.profiler-toggle,
.profiler-table {
  pointer-events: auto;
}
```

---

## HTML diff — `web/index.html`

Two edits. (1) Remove the `<div id="profiler-panel">…</div>` block
currently at lines 35–50 (everything between the two existing
`<canvas>` elements and the closing `</section>` of `#rail-stats`).
(2) Add a new sibling element after the other top-level overlays
(after the closing `</aside>` of `#right-rail`, near `#toast-stack`).

**Diff sketch (concrete):**

```diff
       <section id="rail-stats" class="rail-panel is-active">
         <canvas id="chart-pop" width="260" height="80"></canvas>
         <canvas id="chart-species" width="260" height="80"></canvas>
-        <div id="profiler-panel">
-          <label class="profiler-toggle">
-            <input type="checkbox" id="profiler-enable" />
-            profiler
-          </label>
-          <div id="profiler-stabilizing" class="profiler-stabilizing" style="display:none"></div>
-          <table id="profiler-table" class="profiler-table">
-            <thead>
-              <tr>
-                <th>Stage</th><th>total ms</th><th>calls</th>
-                <th>/call us</th><th>calls/par</th><th>share %</th>
-              </tr>
-            </thead>
-            <tbody id="profiler-tbody"></tbody>
-          </table>
-        </div>
       </section>
       <!-- inspector section unchanged -->
     </aside>

+    <!-- ui-perf: free-standing perf widget (bottom-left, always visible) -->
+    <div id="perf-box" class="overlay-widget perf-widget">
+      <label class="profiler-toggle">
+        <input type="checkbox" id="profiler-enable" />
+        profiler
+      </label>
+      <div id="profiler-stabilizing" class="profiler-stabilizing" style="display:none"></div>
+      <table id="profiler-table" class="profiler-table">
+        <thead>
+          <tr>
+            <th>Stage</th><th>total ms</th><th>calls</th>
+            <th>/call us</th><th>calls/par</th><th>share %</th>
+          </tr>
+        </thead>
+        <tbody id="profiler-tbody"></tbody>
+      </table>
+    </div>
+
     <div id="toast-stack" aria-live="polite"></div>
```

Notes:

- All four inner IDs (`profiler-enable`, `profiler-stabilizing`,
  `profiler-table`, `profiler-tbody`) are preserved letter-for-letter.
- The `.overlay-widget` class is owned by ui-stats (per cross-piece
  coordination flag, §below). ui-perf consumes the base class for
  shared backdrop / border / padding styling. ui-perf adds the
  `.perf-widget` modifier for position / sizing rules.
- The widget sits adjacent to (not inside) `#right-rail`, so it
  remains visible even though the rail itself is hidden (master plan
  §8.1: rail becomes `display: none`).
- The `<aside id="right-rail">` itself is **not** touched by ui-perf.
  The existing `#rail-stats` `<section>` loses its three former
  children when all three UI pieces land: charts (to ui-stats),
  profiler block (to ui-perf, this plan). The rail section can then be
  hidden by ui-stats per master plan §8.1. ui-perf does NOT take
  responsibility for hiding the rail; it just removes its own block.

---

## CSS — `web/src/styles.css`

**Removals.** Delete the four current profiler blocks
(`#profiler-panel`, `.profiler-toggle`, `.profiler-stabilizing`,
`.profiler-table` + its child selectors) at lines 294–337. The
`.profiler-toggle`, `.profiler-stabilizing`, and `.profiler-table`
rules are re-added below with revisions; the `#profiler-panel`
selector is dropped (parent element no longer exists in HTML; see D6).

**Additions.** Place these rules in the same section of `styles.css`
where the current profiler rules live (after the seed-pill block,
before the eulogy card block — roughly lines 294+).

```css
/* ui-perf: free-standing perf widget (bottom-left, always visible).
   Builds on .overlay-widget (defined by ui-stats). */
#perf-box {
  position: fixed;
  bottom: 8px;
  left: 8px;
  min-width: 560px;
  max-width: 760px;
  overflow-x: auto;
  pointer-events: none;
  z-index: 5;
}
/* Interactive children opt back in; the text/layout regions
   (padding, stabilizing banner) remain pointer-events: none so
   canvas pan-through works on all non-interactive areas (M4/M5). */
.profiler-toggle,
.profiler-table {
  pointer-events: auto;
}

.profiler-toggle {
  display: inline-flex;
  gap: 4px;
  align-items: center;
  font-size: 11px;
  cursor: pointer;
  user-select: none;
}
.profiler-stabilizing {
  font-size: 10px;
  font-family: ui-monospace, monospace;
  color: rgba(255, 200, 100, 0.85);
  margin: 3px 0;
}

/* Widened: auto column sizing, nowrap to prevent mid-cell wrap,
   generous horizontal padding so columns don't visually collide. */
.profiler-table {
  table-layout: auto;
  width: 100%;
  border-collapse: collapse;
  font: 11px ui-monospace, monospace;
  color: var(--fg);
  margin-top: 4px;
  white-space: nowrap;
}
.profiler-table th,
.profiler-table td {
  padding: 2px 8px;
  text-align: right;
}
.profiler-table th:first-child,
.profiler-table td.pf-name {
  text-align: left;
}
.profiler-table th { opacity: 0.55; font-weight: normal; }
.profiler-table tr.pf-depth-0 td.pf-name { font-weight: bold; }
.profiler-table tr.pf-depth-1 td.pf-name { padding-left: 12px; }
.profiler-table tr.pf-depth-2 td.pf-name { padding-left: 24px; }
.profiler-table tr.pf-depth-3 td.pf-name { padding-left: 36px; }
.profiler-table tr.pf-depth-4 td.pf-name { padding-left: 48px; }

/* D5: empty-state hint inside tbody when no samples are recorded. */
.profiler-table tr td.pf-empty-hint {
  text-align: center;
  opacity: 0.45;
  font-style: italic;
  padding: 8px 0;
}
```

**Concrete value choices.**

- `min-width: 560px`. Sizing: 6 columns × (text-content + 16px
  padding from `padding: 2px 8px`). Header widths under 11px monospace
  ≈ `Stage`=42, `total ms`=64, `calls`=40, `/call us`=68, `calls/par`=76,
  `share %`=58, plus 16px padding per cell × 6 = 96; total ≈ 444. Add
  ~80px for the deepest indent level on the Stage column (`tick →
  vision_pass → raycast` style) and a little safety = ~560. (At 480px
  the deepest-row Stage column began wrapping in the current rail
  build; 560px gives clear headroom.)
- `max-width: 760px`. Caps the widget on ultra-wide displays so it
  doesn't run halfway across the viewport. The Stats box on the same
  left edge is also constrained (per ui-stats); 760px is wide enough
  for any v5 tick stage name plus reasonable values without forcing a
  scrollbar on a typical 1280px-and-up viewport.
- `overflow-x: auto`. If the user's viewport is narrower than 560px
  (sub-tablet), the container fits in its bounded width and scrolls.
  Per master plan §5 ui-perf: "widen the box and let it scroll
  horizontally if needed".
- `padding: 2px 8px` (was `1px 3px`). The 8px horizontal padding is
  the load-bearing fix for column-collision; combined with `nowrap`,
  columns now have visible breathing room.
- `table-layout: auto`. The current `fixed` layout was distributing the
  rail's 280px width across 6 columns ≈ 47px each, then ellipsis-clipping
  the names. `auto` sizes columns to content; with `nowrap`, content
  determines width.
- `width: 100%`. Harmless under `table-layout: auto + nowrap`: if
  content sums to less than container width, the table fills; if
  larger, columns size naturally and the container scrolls. Keep it so
  the table fills the widened container rather than collapsing to
  content width on the left.

**Dependency note.** `.overlay-widget` base styles (backdrop color,
border, border-radius, padding, font color) are defined by **ui-stats**
in this same pass. ui-perf consumes the class. If ui-stats lands first,
ui-perf inherits the styling; if ui-perf lands first, the widget will
render with default browser styling for a few hours/days until ui-stats
lands — acceptable for a planning-phase risk. (See §"Sequencing"
below.) If implementing ui-perf before ui-stats, copy the following
minimal fallback into ui-perf's CSS section with a TODO comment, and
remove it once ui-stats lands:

```css
/* TODO: remove once ui-stats lands and defines .overlay-widget */
.overlay-widget {
  background: var(--rail-bg);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 4px;
  padding: 6px 8px;
}
```

---

## TS module migration

**New file: `web/src/widgets/perf-panel.ts`** (per D3). The implementer
creates `web/src/widgets/` (new directory).

**Content origin.** Lines 101–270 of `web/src/rail/stats.ts` move
verbatim into the new file, with these adjustments:

1. The import string `../perf` is unchanged, but now resolves to
   `web/src/perf.ts` from the new file location
   `web/src/widgets/perf-panel.ts`. Concretely:
   ```ts
   import { setProfilerEnabled, isProfilerEnabled, reportJson as tsPerfReport } from "../perf";
   ```
2. The import string `../../wasm/evosim` is unchanged, but now resolves
   to `web/wasm/evosim` from the new file location
   `web/src/widgets/perf-panel.ts`. Concretely:
   ```ts
   import type { WorldHandle } from "../../wasm/evosim";
   ```
3. `clearTable()` is replaced. Old behavior: empties the tbody and
   hides the stabilizing banner. New behavior (per D5): emit the
   "enable to record" empty-state row instead of leaving the tbody
   literally empty. Concretely:
   ```ts
   function clearTable(): void {
     const tbody = document.getElementById("profiler-tbody");
     if (tbody) {
       tbody.innerHTML =
         `<tr><td colspan="6" class="pf-empty-hint">enable to record</td></tr>`;
     }
     const banner = document.getElementById("profiler-stabilizing");
     if (banner) banner.style.display = "none";
   }
   ```
4. `renderProfilerTable(tree)` (lines 224–266) gains an empty-state
   guard at the top (per D5). If the merged tree is empty OR every
   root has `call_count === 0`, render the hint and return:
   ```ts
   function renderProfilerTable(tree: ProfNode[]): void {
     const tbody = document.getElementById("profiler-tbody");
     if (!tbody) return;
     const allEmpty =
       tree.length === 0 || tree.every((n) => n.call_count === 0);
     if (allEmpty) {
       tbody.innerHTML =
         `<tr><td colspan="6" class="pf-empty-hint">enable to record</td></tr>`;
       return;
     }
     // ... existing tree-walk renderer unchanged ...
   }
   ```
5. `installProfilerPanel(world)` (lines 125–142) gains a one-time
   initial paint of the empty-state hint so the table isn't visually
   blank on first page load:
   ```ts
   export function installProfilerPanel(world: WorldHandle): void {
     const checkbox = document.getElementById("profiler-enable") as HTMLInputElement | null;
     if (!checkbox) return;
     checkbox.checked = false;     // D9: default off, no persistence
     clearTable();                  // initial paint of "enable to record" hint
     checkbox.addEventListener("change", () => {
       const on = checkbox.checked;
       setProfilerEnabled(on);
       if (on) startPolling(world);
       else { stopPolling(); clearTable(); }
     });
   }
   ```

The rest (`startPolling`, `stopPolling`, `pollAndRender`,
`renderProfilerTable` body, `escHtml`, the `ProfNode`/`ProfReport`
interfaces, `POLL_INTERVAL_MS`, the `profilerPollHandle`
module-level state) move verbatim. The merge of Rust + TS subtrees
(lines 184–194) is unchanged.

**Removals in `web/src/rail/stats.ts`.** Delete lines 101–270
(everything from `// ─── Profiler panel ────────────────` to the end
of `escHtml`). Also remove line 6:
```ts
import { setProfilerEnabled, isProfilerEnabled, reportJson as tsPerfReport } from "../perf";
```
After ui-perf's edit, `rail/stats.ts` contains only the charts code
(lines 1–99). Per master plan §6 R2, this is exactly the split: ui-perf
owns the profiler block; ui-stats owns the charts block. **Both
pieces edit `stats.ts`; the implementer who lands second resolves the
merge.** ui-perf's edit deletes a clean contiguous range, so even an
ordering accident is easy to rebase.

---

## Caller updates — `web/src/main.ts`

One line changes. At line 5:

```diff
-import { installProfilerPanel } from "./rail/stats";
+import { installProfilerPanel } from "./widgets/perf-panel";
```

The call site at line 252 (`installProfilerPanel(world);`) is
unchanged. No other file in the repo imports `installProfilerPanel`
(verified via `grep -rn "installProfilerPanel" web/`); only `main.ts`
references it.

`web/src/rail/index.ts` is **not touched**. Its `installRail`
function does not call `installProfilerPanel` today (verified via
`grep`); the wiring lives in `main.ts`. The `maybeSampleStats(world)`
call inside `pollRail` (line 115) stays exactly as today — it belongs
to ui-stats, not ui-perf.

---

## Stable contracts

**Wasm API.** Unchanged. `WorldHandle::profile_enable(bool)` and
`WorldHandle::profile_report_json() -> String` (src/wasm_api.rs
lines ~336–351) stay exactly as today.

**TS-side perf API.** Unchanged. `web/src/perf.ts` exports
`attachProfiler(world)`, `setProfilerEnabled(on)`, `isProfilerEnabled()`,
`reportJson()`, `span(name)`, `timed(name, fn)`. ui-perf does not
modify this file. The new widget module imports
`setProfilerEnabled`, `isProfilerEnabled`, `reportJson` (re-aliased to
`tsPerfReport` for symmetry with the Rust-side `profile_report_json`).

**JSON shape.** Unchanged. `ProfNode` and `ProfReport` interfaces
move verbatim with the renderer. Per `perf-timing.md` §D5:
`{ now_ms, window_ms, enabled, tree: [{ name, total_us, call_count,
parent_call_count, effective_window_ms, children }] }`.

**Stabilizing banner.** Unchanged behavior. The
`pollAndRender` function (lines 167–222 of current `stats.ts`)
computes `minEffective` across all nodes with `call_count > 0`, shows
the banner when `minEffective < 0.9 * window_ms`, hides it otherwise.
Text format: `stabilizing... (Xs of data)`. Per `perf-timing.md` R3.

**Polling cadence.** Unchanged: 1Hz `setInterval` while enabled;
cleared on disable; one immediate render at start so the table
isn't blank for 1s.

**DOM IDs (preserved).** `profiler-enable`, `profiler-stabilizing`,
`profiler-table`, `profiler-tbody`. **New ID:** `perf-box`. **Removed
ID:** `profiler-panel` (no readers in TS).

---

## Integration points (implementer checklist)

- [ ] Create directory `web/src/widgets/` (if not already created by
      ui-stats or ui-inspector landing first).
- [ ] Create file `web/src/widgets/perf-panel.ts`; move
      `installProfilerPanel`, `startPolling`, `stopPolling`,
      `clearTable`, `pollAndRender`, `renderProfilerTable`, `escHtml`,
      `ProfNode`, `ProfReport`, `POLL_INTERVAL_MS`, `profilerPollHandle`
      from `web/src/rail/stats.ts` into it.
- [ ] Apply the D5 empty-state changes to `clearTable` and
      `renderProfilerTable`, and the initial-paint call from
      `installProfilerPanel`.
- [ ] Update import paths in the new file: `../perf` for the TS-side
      profiler API, `../../wasm/evosim` for the WorldHandle type.
- [ ] In `web/src/rail/stats.ts`, delete lines 101–270 (the entire
      `// ─── Profiler panel ───` block) and line 6 (the
      `setProfilerEnabled / isProfilerEnabled / reportJson` import).
      File shrinks to ~99 lines (charts only).
- [ ] In `web/index.html`, remove `<div id="profiler-panel">…</div>`
      from inside `#rail-stats`; add `<div id="perf-box" class="overlay-widget perf-widget">…</div>`
      as a top-level sibling of `<aside id="right-rail">` and
      `#toast-stack`.
- [ ] In `web/src/styles.css`, delete the four existing profiler
      blocks (`#profiler-panel`, `.profiler-toggle`,
      `.profiler-stabilizing`, `.profiler-table` + descendants) at
      lines ~294–337; add the new blocks per §"CSS" above.
- [ ] In `web/src/main.ts` line 5, change import from
      `"./rail/stats"` to `"./widgets/perf-panel"`. The call at line
      252 stays.
- [ ] Verify `pnpm --filter ./web build` is clean.
- [ ] Verify no orphan reference to `#profiler-panel` exists:
      `grep -rn "profiler-panel" web/` should return zero hits after
      the edits.
- [ ] Verify no orphan import of profiler symbols from
      `./rail/stats` exists in any TS file:
      `grep -rn 'from "./rail/stats"' web/src/` and
      `grep -rn 'from "../rail/stats"' web/src/` should each show
      only `main.ts`'s `installSpeedControls` / nothing related to
      profiler.
- [ ] Verify `escHtml` has exactly one definition and no unescaped
      string interpolation in table rows: `grep -rn "escHtml" web/`
      should show only the function definition and its call sites
      inside `renderProfilerTable`. Confirm every `<td>` cell that
      renders a node name passes the value through `escHtml(...)` —
      no raw template-literal string interpolation of user-visible
      names.

---

## Tests

No new Rust tests (zero Rust changes). No new TS tests (Vitest still
not in the repo, per `perf-timing.md` §"TS-side tests"). Manual
verification gates:

1. **`pnpm --filter ./web build` is clean.** No type errors from the
   moved code (the imports re-resolve cleanly because both files now
   live two directories deep relative to `wasm/evosim` and one
   directory deep relative to `perf.ts`).
2. **Page load:** widget is visible at the bottom-left corner. The
   checkbox is unchecked. The table shows the header row and a single
   "enable to record" row in the tbody. The stabilizing banner is
   hidden.
3. **No truncation:** every column header is fully visible (no
   ellipsis), and the "/call us" and "calls/par" headers render
   without clipping. At 1280px viewport the widget's right edge is
   well inside the viewport.
4. **Toggle on:** click the checkbox. Within ≤1s the table populates
   with the Rust `tick` subtree and TS `frame` subtree. The tbody now
   has multiple rows; the empty-state hint is gone. Stage names like
   `eat_and_scavenge`, `movement_and_repulsion`, `bookkeeping_tail`
   render in full with no clipping.
5. **Toggle off:** click the checkbox again. Polling stops (verify in
   devtools: no more `profile_report_json` calls). The empty-state
   "enable to record" hint reappears in the tbody. The banner is
   hidden.
6. **Stabilizing banner:** toggle on, immediately observe the
   `stabilizing... (Xs of data)` banner for the first ~60s of recording.
   It disappears once every node has ≥54s of data
   (per the 0.9 × 60000 ms threshold).
7. **Canvas underneath:** click-and-drag on the aquarium canvas at a
   point covered by the transparent regions of `#perf-box` (the gap
   between widget edge and table). Panning still works — confirms
   pointer-events: none on outer container per D10.
8. **No console errors** during any of the above.

---

## Risks and mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Widget overlaps the bottom-left of the aquarium and hides part of the scene at small viewports | Medium | `bottom: 8px; left: 8px` keeps an 8px gutter; widget is ~360px tall when fully populated. On <900px viewport heights, user can toggle profiler off and the widget collapses to header + hint (~80px). Future v1.1 can add a collapse button. |
| Implementer leaves `width: 100%` AND `table-layout: fixed` together by mistake (the latter was removed but old habits) | Low | Tests step 3 above catches it. Reviewer checks the CSS diff explicitly. |
| `.overlay-widget` base class not yet defined (ui-stats lands second) | Medium | See "Sequencing" — implementer adds a minimal local fallback rule with a TODO until ui-stats lands. |
| Both ui-stats and ui-perf edit `web/src/rail/stats.ts` → merge conflict | High (by design) | The two pieces touch disjoint contiguous ranges (charts: 1–99; profiler: 101–270). The whoever-lands-second commit just deletes their range cleanly. Sequencing §below pins recommended order. |
| `web/src/main.ts:5` import path is stale on first compile after the file move | Low | Single-line diff in same commit; build catches if forgotten. |
| New `web/src/widgets/` directory collides with a TS path alias | Very low | No `paths` aliases in `web/tsconfig.json` (current). New directory is just another folder under `src/`. |
| Removing `#profiler-panel` from HTML accidentally drops `#profiler-stabilizing` because it was a child of the removed div | Low (mechanical) | The HTML diff explicitly re-emits all four child IDs inside the new `#perf-box`. Reviewer greps for each ID after the edit. |
| Bottom-left position conflicts with future "speed controls" dock or other HUD | Low | Speed controls live in the top bar (`installSpeedControls`); no other bottom-left widget exists. Future relocations are a single CSS edit. |

---

## Cross-piece coordination flags

These bubble up to the cross-reviewer (master plan §6):

- **C1. Both ui-stats and ui-perf edit `web/src/rail/stats.ts`.**
  Disjoint contiguous ranges. ui-stats owns lines 1–99 (charts +
  `maybeSampleStats` + `drawChart` + `drawCharts` + `getCanvases` +
  the `samples` ring + `SAMPLE_CAP` + `SAMPLE_INTERVAL_TICKS`);
  ui-perf owns lines 101–270 (profiler block) and removes the
  import of `setProfilerEnabled/isProfilerEnabled/reportJson` at
  line 6. After both land, `stats.ts` may itself be moved to
  `widgets/stats.ts` by ui-stats; ui-perf does NOT do that move.
- **C2. `.overlay-widget` base class is owned by ui-stats.** ui-perf
  and ui-inspector consume it. If ui-perf lands first, implementer
  adds a temporary minimal fallback (see §CSS dependency note).
- **C3. Right-rail teardown is owned by ui-stats / ui-inspector.**
  Per master plan §8.1, `<aside id="right-rail">` becomes
  `display: none`. ui-perf does NOT hide the rail; it just removes
  its own block from `#rail-stats`. The rail being hidden or visible
  has zero effect on `#perf-box` (which is a top-level sibling).
- **C4. Web/src/widgets/ directory creation.** First UI piece to
  land in this pass creates the directory. ui-perf's plan assumes
  the directory exists; the implementer creates it if missing.

---

## Sequencing for the implementer

Build in this order. Stop at each checkpoint and verify locally before
proceeding.

1. **Create `web/src/widgets/perf-panel.ts`** with the verbatim move
   + the D5 empty-state changes. Stub-test by leaving the old code
   in `stats.ts` and the old HTML in `index.html`; verify the new
   file compiles in isolation. Checkpoint: `pnpm build` clean.
2. **Update `web/src/main.ts` import** to point at the new module.
   Checkpoint: `pnpm build` still clean; page loads; profiler still
   works exactly as before (the old DOM is still in `#rail-stats`,
   the renderer is now in the new file but reads the same IDs).
3. **Edit `web/index.html`**: remove old `#profiler-panel`, add new
   `#perf-box`. Checkpoint: page loads; widget appears at
   bottom-left; profiler still works.
4. **Edit `web/src/styles.css`**: remove old rules, add new rules.
   Decide whether ui-stats has already landed `.overlay-widget`; if
   not, add the temporary fallback. Checkpoint: tests 1–8 above all
   pass.
5. **Edit `web/src/rail/stats.ts`**: delete lines 101–270 and the
   line-6 import. Checkpoint: `pnpm build` clean; no orphan
   references.
6. **Final pass.** Re-run all manual verification steps (§"Tests"
   1–8). Commit.

**Recommended overall ordering with the other UI pieces:** ui-stats
lands first (so `.overlay-widget` exists), then ui-perf, then
ui-inspector. ui-perf can land before ui-stats with the temporary
fallback rule per C2; not blocked. Per master plan §4, all three UI
pieces are golden-safe for the default build and may interleave
anywhere among the perf pieces.

---

## Constraints (re-confirmed)

- `pnpm build` clean.
- Zero new npm deps.
- Existing profiler JSON shape unchanged.
- Existing column count (6 visible) and labels unchanged.
- Zero Rust changes (no `src/` edits, no `Cargo.toml` edits, no
  `wasm-pack` flag changes, no test-file edits).
- The `web/src/perf.ts` file is unchanged.
- DOM ID stability contracts preserved (`profiler-enable`,
  `profiler-stabilizing`, `profiler-table`, `profiler-tbody`).
- §16 acceptance hash unaffected (no Rust → no behavior change
  → no determinism risk).

---

## Out-of-scope (explicit cuts)

- **No new column** for self-time / p95 / min / max. Per
  `perf-timing.md` §"Out-of-scope" — same cuts apply.
- **No CSV export / copy-report button** next to the toggle.
  `perf-timing.md` §"Out-of-scope" defers this to v1.1.
- **No collapse / minimize control** on the widget. If table height
  becomes annoying at small viewports, add in v1.1.
- **No persistence** of widget position or checkbox state. Per
  `perf-timing.md` §D9 the toggle is intentionally non-persistent.
- **No drag-to-reposition** of the widget. Fixed bottom-left.
- **No theming hooks** beyond reuse of `var(--fg)` and the
  `.overlay-widget` base.
- **No animation** on widget appear / table populate.
- **No rail teardown** — ui-stats / ui-inspector handle that.
- **No move of `web/src/perf.ts`** (the instrumentation file).

---

## Decisions to record (target file: `DECISIONS.md`)

Each of D1–D10 above gets a one-paragraph entry under a
`## v1.1 UI — ui-perf widget` heading (or under the existing
`## v1.1 UI` section if one of the other UI pieces has already
created it). Most load-bearing entries:

- **D1** — bottom-left position; "instrumentation gutter" rationale.
- **D3** — new module at `web/src/widgets/perf-panel.ts` (not
  `web/src/perf-panel.ts`) to avoid namespace ambiguity with the
  existing `web/src/perf.ts` instrumentation file.
- **D4** — `table-layout: auto` + `min-width: 560px` + `overflow-x:
  auto` is the truncation fix; cite the perf-timing.md
  "Code review — FIXES REQUIRED" block §1 as the bug origin.
- **D5** — empty-state hint rendered by TS (not in static HTML) so
  it covers both "profiler-off" and "no samples yet" states with one
  code path.
- **D6** — `perf-box` is the new stable ID; `profiler-panel` is
  dropped (no readers).

---

## Citations

- `docs/plans/perf+ui-master.md` §2 row "ui-perf"; §5 "ui-perf —
  Bottom-left Perf widget"; §6 R2 (UI inheritance split); §6 R8
  (z-order); §8.1 (right-rail kept as hidden empty shell).
- `docs/plans/perf-timing.md` §D5 (JSON contract — unchanged); §D9
  (toggle default OFF, no persistence); §R3 (stabilizing banner
  threshold); §"Code review — FIXES REQUIRED" item 1 (table-too-thin
  bug origin).
- `web/index.html` lines 32–51 — `#rail-stats` section containing
  the current `#profiler-panel` block to extract; lines 22–71 for
  the surrounding `<aside id="right-rail">` shell (untouched).
- `web/src/rail/stats.ts` lines 1–99 (charts — ui-stats territory),
  lines 101–270 (profiler — ui-perf territory), line 6 (import to
  remove).
- `web/src/perf.ts` — TS-side profiler API surface (unchanged here):
  `attachProfiler`, `setProfilerEnabled`, `isProfilerEnabled`,
  `reportJson`, `span`, `timed`.
- `web/src/styles.css` lines 294–337 — current profiler CSS to
  replace; lines 33–48 (`#top-bar` pointer-events pattern referenced
  by D10); lines 101–110 (`#toast-stack` — visual neighbor not
  conflicting).
- `web/src/main.ts` line 5 (import to update), line 252 (call site —
  unchanged).
- `web/src/rail/index.ts` lines 12, 75–115 — confirms
  `installRail` / `pollRail` do NOT call `installProfilerPanel`; the
  wiring is purely in `main.ts`.

---

*End of ui-perf plan.*

---

## Revision history

**r1 (initial).** Plan authored and approved with three minor should-fix
items (S1–S3) and no must-fix items. All DOM ID contracts preserved, file
path non-colliding, `stats.ts` carve-out scope verified clean.

**r2 (this revision).** Applied S1–S3 and cross-review M4/M5:

- **S1 (D3 wording).** Reflowed items 1 and 2 of the TS module migration
  import-path narrative to read "The import string `../perf` is unchanged,
  but now resolves to `web/src/perf.ts` from the new location
  `web/src/widgets/perf-panel.ts`." Same fix for `../../wasm/evosim`.
- **S2 (`.overlay-widget` fallback).** Promoted the prose-described
  minimal fallback into a concrete CSS code block with a TODO comment so
  the implementer can paste it directly if ui-stats hasn't landed yet.
- **S3 (`escHtml` grep).** Added a `grep -rn "escHtml"` step to the
  implementer checklist to confirm no XSS vector creeps in via table row
  rendering. (The existing `renderProfilerTable` already calls `escHtml`
  on every name; the checklist step makes this an explicit gate.)
- **M4/M5 (pointer-events policy).** Replaced the `#perf-box > *`
  catch-all with explicit named selectors (`.profiler-toggle`,
  `.profiler-table`) per the cross-review recommendation: the stabilizing
  banner and padding regions stay `pointer-events: none`; only the
  interactive controls opt back in to `auto`. Updated D10 to explain the
  M4/M5 rationale and inlined the concrete CSS block there.

---

## Code review

**Verdict: APPROVED — ships as-is.**

Commit `c9e446f` implements the ui-perf plan cleanly. Verified items:

1. **New module** `web/src/widgets/perf-panel.ts` (192 LOC) contains
   `installProfilerPanel`, `startPolling`, `stopPolling`, `clearTable`,
   `pollAndRender`, `renderProfilerTable`, `escHtml`, and the
   `ProfNode`/`ProfReport` interfaces. The D5 empty-state hint is
   rendered on init via `clearTable()`, on toggle-off, and as a guard
   inside `renderProfilerTable` when `tree.length === 0 || every n.call_count === 0`.
2. **`rail/stats.ts` reduced to chart-only** — 99 LOC; no profiler
   imports or code remain.
3. **`main.ts` import updated** to `./widgets/perf-panel` (line 5);
   call site unchanged (line 267).
4. **DOM IDs preserved.** `profiler-enable`, `profiler-stabilizing`,
   `profiler-table`, `profiler-tbody` all present inside the new
   `#perf-box` in `web/index.html` lines 72–87. Old `#profiler-panel`
   parent removed cleanly; no orphan refs (`grep` clean).
5. **`#perf-box` pointer-events policy correct.** `pointer-events: none`
   on the container; `.profiler-toggle, .profiler-table { pointer-events:
   auto }` opt-back per M4/M5.
6. **Truncation fix applied.** `table-layout: auto`, `white-space:
   nowrap`, `min-width: 560px`, `max-width: 760px`, `overflow-x: auto`,
   `padding: 2px 8px` — all per plan. The old `overflow: hidden;
   text-overflow: ellipsis` cell rules are gone.
7. **6-column empty-state hint** (`colspan="6"`, class `pf-empty-hint`)
   rendered in both `clearTable` and `renderProfilerTable`. Matching CSS
   rule at styles.css:422 styles it muted/italic/centered.
8. **`.overlay-widget` NOT redefined.** ui-perf consumes the class only
   (used on `#perf-box`); the base rule lives at styles.css:57 from
   ui-stats. Diff inspection confirms no redefinition.
9. **`escHtml` correctly wraps node names.** `renderProfilerTable` calls
   `escHtml(node.name)` on every `<td class="pf-name">` cell; numeric
   fields use `fmt(...)` which only emits `toFixed` output. No
   unescaped interpolation. Risk is low (Rust `&'static str` names) but
   defense-in-depth is intact.
10. **Zero Rust changes.** `git show c9e446f -- src/ tests/ Cargo.toml`
    returns empty.
11. **`pnpm build` clean.** Vite emits the standard wasm-bindgen
    dynamic-import advisory (pre-existing, unrelated); no TS errors.
12. **Acceptance unaffected.** `cargo test --release --test acceptance`:
    3/3 passed (`acceptance_t10000`, `save_load_step_preserves_determinism`,
    `profile_does_not_change_hash`).
13. **`styles.css` end-to-end intact.** Linter-applied formatting did
    not break the perf-box block (lines 361–427) — all D4/D10/D5 rules
    present and well-formed. No stray braces or duplicated selectors.

No must-fix or should-fix items. Plan can be marked complete.

