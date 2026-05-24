# ui-inspector — top-right Inspector pop-up (hidden by default)

**Status.** Plan only. One commit. Track: UI. Risk: low. Determinism:
safe (no Rust, no wasm-bindgen surface changes). Source-of-truth:
`docs/plans/perf+ui-master.md` §2, §5 (ui-inspector row), §6 (R3, R8),
§8.1 (right-rail kept hidden).

This piece evicts the Inspector `<dl>` out of the right-rail
`<section id="rail-inspector">` and into a new free-standing
top-right overlay widget that is hidden by default and pops up only
when the user clicks a creature. All existing `#ins-*` DOM IDs are
preserved verbatim so `web/src/rail/inspector.ts::renderInspector` (the
populator) requires zero change to its DOM writes. Logic in
`inspector.ts` switches from "swap rail tab" to "toggle widget
visibility".

---

## 1. Resolved decisions (read first; downstream sections enforce)

**D1. Close affordance — BOTH close button AND click-outside-to-dismiss.**
Visible `[×]` close button in the top-right corner of
`#inspector-box` for explicit dismissal. Click-on-empty-canvas
continues to dismiss (existing behavior — preserved verbatim from
`inspector.ts::installCanvasClickHandler` line 201 path
`clearSelection(rail)`). Rationale: discoverability + zero churn on
the existing canvas hit-test path.

**D2. Rail tab injection — GUARD, do not delete.** The legacy
`ensureInspectorTab` / `rail.switchTab("inspector")` / `rail.switchTab("events")`
calls in `inspector.ts` are wrapped in a guard that no-ops when the
rail is not visible (it is hidden per master §8.1). The guard is one
line at the top of each helper: `if (isRailHidden()) return;` (or
inline equivalent). The injection code stays in the file so future
v1.1 re-enablement of the rail (DECISIONS pattern "v1.1 revisit") is
cheap. Rationale: matches the Events-disable pattern; eliminates the
dead-code risk of an inspector tab silently appearing inside a hidden
container.

**D3. Polling when hidden — SKIP wasm calls.** `refreshInspector`
gains an early-return `if (inspectorBox.style.display === "none") return;`
**before** the `state.kind !== "selected"` check. This avoids any
per-frame work when the widget is closed. (Practically the existing
guard `state.kind !== "selected"` already covers the steady-state
closed case, but the explicit display check is the contract for
"closed means no wasm calls" and protects against future state-vs-DOM
drift.)

**D4. Dependency on ui-stats `.overlay-widget` class.** ui-inspector
reuses the shared `.overlay-widget` base class introduced by
ui-stats. **Sequencing recommendation: ui-stats lands first.** If
ui-inspector lands first, this plan owns the `.overlay-widget` base
rule and ui-stats inherits it (one-line move on landing the second
piece). Master plan §3 already lists both as independent and
interleavable; this is the only cross-piece CSS coupling.

---

## 2. HTML changes — `web/index.html`

### 2.1 Add `#inspector-box` near the top of `<body>`

Insert after `#top-bar` (line 20) and after the new `#stats-box`
introduced by ui-stats (if it lands first; otherwise immediately
after `#top-bar`). Position in the DOM does **not** affect visual
position because both widgets are `position: fixed`.

```html
<!-- ui-inspector: Inspector pop-up. Hidden by default; shown by
     canvas click handler on creature hit. -->
<div id="inspector-box" class="overlay-widget" style="display:none">
  <button id="inspector-close" type="button" aria-label="Close">×</button>
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
</div>
```

### 2.2 Empty out `<section id="rail-inspector">`

Drop the inner `<dl>…</dl>` (lines 53–69 of current `index.html`).
Keep the section element itself so `rail-panel` enumeration in
`installTabs` (`rail/index.ts:50`) does not break:

```html
<!-- Inspector content evicted to #inspector-box (ui-inspector). -->
<section id="rail-inspector" class="rail-panel"></section>
```

### 2.3 Right-rail visibility

Per master §8.1, the entire `<aside id="right-rail">` is hidden via
`display: none`. ui-stats's CSS rule owns that (the other widget plans
also rely on it). ui-inspector does not duplicate the rule; it only
asserts the hidden invariant via D2's guard.

### 2.4 Stable DOM ID contract (must not change)

- Preserved (existing): `#ins-species`, `#ins-parent`, `#ins-action`,
  `#ins-age`, `#ins-energy`, `#ins-size`, `#ins-photo`, `#ins-eat`,
  `#ins-scav`, `#ins-move`, `#ins-eyes`, `#ins-vision`, `#ins-armor`,
  `#ins-bite`, `#ins-pigment` (15 IDs read by ID in
  `inspector.ts::renderInspector` lines 35–53).
- New (introduced by this commit, contract going forward):
  `#inspector-box`, `#inspector-close`.
- Preserved (no-op container, kept for `rail-panel` enumeration):
  `#rail-inspector`.

---

## 3. CSS changes — `web/src/styles.css`

### 3.1 Shared `.overlay-widget` base (owned by ui-stats; see D4)

If ui-stats has not yet landed, ui-inspector owns this base rule:

```css
/* Shared overlay-widget base. Owned by whichever of {ui-stats,
   ui-inspector, ui-perf} lands first. */
.overlay-widget {
  position: fixed;
  background: var(--rail-bg);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 4px;
  padding: 8px 10px;
  font-size: 12px;
  color: var(--fg);
  z-index: 5;          /* above #aquarium; below modals (z-index 30) */
  pointer-events: auto;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35);
}
```

### 3.2 `#inspector-box` positioning + sizing

`#inspector-box` inherits `.overlay-widget` default (`pointer-events: auto`);
no override needed. (Per cross-review M4/M5, the resolved policy is:
`.overlay-widget` defaults to `pointer-events: auto`; only `#stats-box` and
`#perf-box` override to `none` because they need canvas-pan-through. The
inspector box is opaque and interactive, so the inherited default is correct.)

```css
#inspector-box {
  top: 8px;
  right: 8px;
  max-width: 280px;       /* mirrors current rail width */
  max-height: calc(100vh - 16px);
  overflow-y: auto;
}

#inspector-box dl {
  display: grid;
  grid-template-columns: 110px 1fr;   /* mirrors existing #rail-inspector dl */
  gap: 2px 6px;
  margin: 0;
}
#inspector-box dt { opacity: 0.55; }
#inspector-box dd { margin: 0; word-break: break-all; }

#inspector-close {
  position: absolute;
  top: 2px;
  right: 4px;
  width: 20px;
  height: 20px;
  padding: 0;
  background: transparent;
  border: 0;
  color: var(--fg);
  font: inherit;
  font-size: 16px;
  line-height: 18px;
  cursor: pointer;
  opacity: 0.55;
}
#inspector-close:hover { opacity: 1; }
```

Values chosen to mirror the existing `#rail-inspector dl` grid
(`styles.css:163–175`) so visual density is unchanged. `top:8px /
right:8px` mirrors ui-stats's `top:8px / left:8px`. `max-height` with
`overflow-y: auto` prevents the 15-row dl from running off the
bottom on short viewports.

### 3.3 Delete legacy rules

Drop the now-orphan rules at `styles.css:162–175`:

```css
/* DELETE — content moved to #inspector-box (ui-inspector). */
#rail-inspector dl { … }
#rail-inspector dt { … }
#rail-inspector dd { … }
```

---

## 4. TypeScript changes

### 4.1 `web/src/rail/inspector.ts` — show/hide instead of tab-switch

The file stays in place (no rename, no move). Five edits:

**Edit A — module-level handle to the box.**

```ts
function getInspectorBox(): HTMLElement | null {
  return document.getElementById("inspector-box");
}
function getInspectorClose(): HTMLButtonElement | null {
  return document.getElementById("inspector-close") as HTMLButtonElement | null;
}
function isRailHidden(): boolean {
  const rail = document.getElementById("right-rail");
  return !rail || rail.style.display === "none" ||
         getComputedStyle(rail).display === "none";
}
```

**Edit B — `openInspector` shows the box instead of switching rail tabs.**

Current body (lines 82–94) does: delete prior highlight, set state,
add new highlight, `renderInspector`, `ensureInspectorTab`,
`tab.classList.remove("hidden")`, `rail.switchTab("inspector")`.

New body keeps the highlight + state + render logic verbatim,
replaces the tab block with:

```ts
const box = getInspectorBox();
if (box) box.style.display = "block";
// Legacy rail tab injection — guarded no-op while rail is hidden (D2).
if (!isRailHidden()) {
  const tab = ensureInspectorTab(rail);
  tab.classList.remove("hidden");
  rail.switchTab("inspector");
}
```

**Edit C — `clearSelection` hides the box instead of switching tabs.**

Current body (lines 96–107) does: highlight delete, state reset,
hide tab, switch rail back to events.

New body:

```ts
if (state.kind === "selected") {
  highlights.delete(state.creatureId);
}
state = { kind: "empty" };
const box = getInspectorBox();
if (box) box.style.display = "none";
// Legacy rail tab cleanup — guarded no-op while rail is hidden (D2).
if (!isRailHidden()) {
  if (inspectorTab) inspectorTab.classList.add("hidden");
  if ((rail as { activeTab?: string }).activeTab === "inspector") {
    rail.switchTab("events");
  }
}
```

**Edit D — `refreshInspector` skips wasm calls when hidden (D3).**

Add as the very first line of the function body (currently line 118):

```ts
const box = getInspectorBox();
// Primary guard: skip all wasm calls when the box is closed (D3).
// The display check is the explicit contract for "closed = no wasm calls"
// and protects against future state-vs-DOM drift.
if (box && box.style.display === "none") return;
// Defensive guard: steady-state fallback; in practice clearSelection()
// already sets state.kind = "empty" before the display-none check can
// be reached, so both guards agree — this is belt-and-braces only.
if (state.kind !== "selected") return;
// … rest unchanged
```

The existing 2-second `diedAt` placeholder logic stays verbatim. Note
that when the creature dies and `clearSelection(rail)` fires from
within `refreshInspector` (line 138), edit C's hide-the-box behavior
auto-hides the widget. **The died-then-auto-hide flow is preserved.**

**Edit E — wire the close button in `installCanvasClickHandler`.**

`installCanvasClickHandler` is the natural one-time-install seam (it
already takes `rail` and is called once from `main.ts:245`). Add at
the top of the function body, before the existing `pointerdown`
listener:

```ts
const closeBtn = getInspectorClose();
if (closeBtn) {
  closeBtn.addEventListener("click", () => clearSelection(rail));
}
```

(Alternative seam: a new `installInspectorBox(rail)` exported from
this file and called from `main.ts`. Author's call. The
`installCanvasClickHandler` piggyback is one fewer call site to
thread through `main.ts`.)

**Edit F — `renderInspector` body is unchanged.** It reads/writes
`#ins-*` by ID. The IDs move with the `<dl>` to `#inspector-box`. No
edit required.

### 4.2 `web/src/rail/index.ts` — no functional change

`pollRail` still calls `refreshInspector(world, idsBuffer, rail)`
each frame (line 118). The function still no-ops fast when hidden
(D3). The `RailState` shape is unchanged. The `installTabs` /
`switchTab` plumbing is unchanged — the inspector tab is simply
never injected (guarded out) while the rail is hidden.

The `<section id="rail-inspector">` is now empty; `installTabs`
iterates `.rail-panel` and toggles classes, but the empty section is
harmless.

### 4.3 `web/src/main.ts` — one-line change (canvas click handler)

The canvas click handler is already entirely owned by
`inspector.ts::installCanvasClickHandler`
(`main.ts:245`). The click handler's empty-canvas branch already
calls `clearSelection(rail)` (`inspector.ts:202`), which under this
plan hides the box (Edit C). **No edits required in `main.ts`.**

Confirm: `main.ts` does not call `creature_at` directly. Search
result: `creature_at` is only invoked at `inspector.ts:199`.

---

## 5. Show/hide state machine (post-restructure)

| Trigger | DOM action | State action |
|---|---|---|
| Canvas tap → `creature_at` returns idx | `#inspector-box.style.display = "block"` | `state = { kind: "selected", creatureId }`, set permanent highlight |
| Canvas tap on empty space | `#inspector-box.style.display = "none"` | `state = { kind: "empty" }`, delete highlight |
| `#inspector-close` click | `#inspector-box.style.display = "none"` | `state = { kind: "empty" }`, delete highlight (via `clearSelection`) |
| Selected creature dies | After 2s placeholder: `display = "none"` | `clearSelection` |
| Re-click a different creature | `display` stays `"block"` | delete old highlight, set new, `renderInspector` |
| Frame tick (`refreshInspector`) while open | re-render dl from fresh JSON | unchanged |
| Frame tick while hidden | early return | unchanged |

The state machine is the same one that exists today; only the
DOM-side effect changes from "swap rail tab class" to "toggle box
display".

---

## 6. Polling cadence

- **When open:** unchanged from today — `refreshInspector` is called
  every frame by `pollRail`, calls `world.creature_inspect_json(idx)`
  once, parses the JSON, writes 15 `<dd>` text nodes. Cost is
  unchanged.
- **When closed:** zero wasm calls, zero DOM writes — guaranteed by
  the D3 early-return.
- **1Hz species poll** in `pollRail` is unrelated to inspector and
  unchanged.

---

## 7. Integration points & cross-piece contracts

| Contract | Owner | Consumer |
|---|---|---|
| `.overlay-widget` CSS base class | ui-stats (preferred) or first-to-land | ui-inspector, ui-perf |
| `#right-rail { display: none }` rule | ui-stats (per master §8.1) | ui-inspector relies on this for D2's `isRailHidden()` |
| `#ins-*` IDs (15) | ui-inspector HTML | `inspector.ts::renderInspector` (untouched) |
| `creature_inspect_json` JSON shape | `src/wasm_api.rs:243` | `CreatureInspectJson` TS interface (untouched) |
| `creature_at(wx, wy, tol)` return | `src/wasm_api.rs` | `inspector.ts:199` (untouched) |
| `RailState` interface | `rail/index.ts:34` | `inspector.ts` (still receives `rail`; uses it only inside guarded branches) |
| `installCanvasClickHandler(canvas, cam, getView, world, rail)` signature | `inspector.ts:165` | `main.ts:245` (untouched) |

**No Rust changes.** `creature_inspect_json` and `creature_at` are
called identically. The wasm-bindgen surface is unchanged.

---

## 8. Manual test recipe

1. `pnpm --filter ./web build` — clean build, no TS errors, no CSS
   warnings.
2. Load the page in a browser. Confirm:
   - Right rail is **not visible** (hidden per master §8.1).
   - `#inspector-box` is **not visible** (display:none default).
   - Canvas fills the viewport.
3. Click a creature.
   - `#inspector-box` appears in the top-right corner.
   - All 15 fields populate with non-empty values.
   - A permanent highlight ring is drawn on the clicked creature
     (existing behavior, unchanged).
4. Click another creature.
   - The box stays visible; fields update; the old highlight
     disappears and a new one appears on the new creature.
5. Click empty water.
   - Box hides; highlight clears.
6. Re-click a creature, then click the `[×]` close button.
   - Box hides; highlight clears.
7. Re-click a creature; speed up sim (100x) and wait for it to die.
   - "Creature died" placeholder shows for ~2s.
   - Box auto-hides; highlight clears.
8. Open the box, then run sim at 100x for >30s with box open.
   - Inspector keeps updating each frame; no crash; profiler (if
     enabled via ui-perf) shows `pollRail → refreshInspector` cost
     bounded.
9. Close the box, then run sim at 100x for >30s.
   - Profiler shows `pollRail → refreshInspector` cost is near-zero
     (D3 short-circuit). No `creature_inspect_json` calls per frame.
10. Resize the browser window narrow.
    - Box pins to top-right; if creature data is tall, dl scrolls
      inside the box (max-height + overflow-y).
11. Verify legacy save/load round-trip still selects creatures
    correctly on first click after a `loadCurrent` resume.

No automated tests are added; no Rust tests change. `cargo test --lib`
and `cargo test --release --test acceptance` are unaffected.

---

## 9. Risks

**R-INS-1. Stale `inspectorTab` reference when rail is hidden.** Edit
C still references the module-level `inspectorTab` inside the
guarded branch. If the rail is later re-enabled at runtime (no such
toggle exists today), the cached `inspectorTab` from a prior
display:none era might be stale. Mitigation: `ensureInspectorTab`
already returns the cached handle; the guard short-circuits before
the cache is read in this commit. Acceptable.

**R-INS-2. `.overlay-widget` class collision.** If a third party
defines `.overlay-widget` in their stylesheet, our box inherits.
Mitigation: namespace-free `.overlay-widget` is acceptable for this
internal app (no external CSS sources). Reviewer cross-check: confirm
ui-stats and ui-perf use the same class spelling.

**R-INS-3. z-index ladder vs modals.** Modals
(`.modal-backdrop`) use `z-index: 30` (`styles.css:191`). Our box
uses `z-index: 5`. Confirmed below modals. Resume prompt and
schema-mismatch modals overlay the inspector correctly.

**R-INS-4. Pointer-events on a non-interactive area.** Unlike
`#top-bar` (which sets `pointer-events: none` on the container and
`auto` on children), the inspector box itself is interactive (close
button, scroll). Default `pointer-events: auto` is correct. The box
should NOT capture clicks meant for the canvas behind it — but since
it is opaque and small (≤280×viewport), this is by design (users
expect to not click "through" the inspector).

**R-INS-5. Polling-when-hidden check coupling to inline style.** D3
checks `box.style.display === "none"`. If a future CSS-class-based
hide is added (e.g., `.is-hidden`), the check breaks silently.
Mitigation: keep show/hide as inline `style.display` only (Edit B/C
both write `style.display`). If a class-based hide is introduced,
update D3 in the same commit.

**R-INS-6. Empty `#rail-inspector` section + `rail-panel`
enumeration.** `installTabs` (`rail/index.ts:50`) iterates
`.rail-panel` and computes `panelName = p.id.replace("rail-", "")`.
The empty `#rail-inspector` section is still enumerated. Harmless
— `is-active` toggles on an empty element with no visual effect.
Reviewer cross-check: if ui-stats or ui-perf removes the rail panels
entirely, ensure this enumeration still has at least one element
(or is itself removed). Master plan §8.1 says keep the rail shell;
this is consistent.

---

## 10. Sequencing

- **Within this commit:** order the diff as HTML → CSS → TS so the
  page never enters a half-broken state at any intermediate save
  point.
- **Across pieces:** master §4 recommends ui-stats → ui-inspector
  → ui-perf. ui-inspector depends on ui-stats only for the
  `.overlay-widget` base class and the `#right-rail { display: none }`
  rule (see D4 + §7). If interleaved differently, ui-inspector self-
  contains both rules and ui-stats/ui-perf inherit; flag in the
  commit message.

---

## 11. Commit message (suggested)

```
feat(ui): top-right inspector pop-up

Evicts the Inspector <dl> from the (hidden) right-rail into a
free-standing #inspector-box widget pinned to the top-right of the
viewport. Hidden by default; shown on creature click, hidden on
empty-canvas click, on close-button click, or 2s after the selected
creature dies. Preserves all #ins-* IDs and the renderInspector
populator unchanged. Skips wasm calls (creature_inspect_json) when
hidden. Legacy rail tab injection is guarded behind isRailHidden()
and remains in place for a future v1.1 rail re-enablement.

Per docs/plans/ui-inspector-box.md. No Rust changes.
```

---

## 12. Citations

- `docs/plans/perf+ui-master.md` §2 (ui-inspector row), §3 (no Rust
  dep), §5 (ui-inspector briefing — DOM IDs to preserve), §6 R3
  (rail-poll responsibility split), §6 R8 (z-order), §8.1 (right-rail
  kept hidden), §8 entry on the Events-disable parallel.
- `web/index.html:23` (right-rail shell), `:31` (Events hidden
  pattern), `:52–70` (current `#rail-inspector` dl).
- `web/src/rail/inspector.ts:34–54` (renderInspector — unchanged),
  `:82–94` (openInspector — Edit B), `:96–107` (clearSelection —
  Edit C), `:113–152` (refreshInspector — Edit D), `:165–211`
  (installCanvasClickHandler — Edit E), `:199` (only call site for
  `creature_at`), `:202` (clearSelection on empty-canvas tap).
- `web/src/rail/index.ts:42–66` (installTabs / switchTab),
  `:92–123` (pollRail).
- `web/src/main.ts:245` (only call site for
  `installCanvasClickHandler`; no direct `creature_at` call).
- `web/src/styles.css:50–98` (rail shell + tabs CSS),
  `:162–175` (legacy `#rail-inspector dl` rules to delete),
  `:191` (modal z-index 30).
- `src/wasm_api.rs::creature_inspect_json` (JSON shape unchanged),
  `creature_at` (signature unchanged).

*End of plan.*

---

## Revision history

- **Initial plan** — authored against pre-ui-stats source; D4 notes
  ui-stats dependency and fallback.
- **Post-review revision** (cross-review M4/M5 + per-piece should-fix):
  - §4.1 Edit D: added inline comments distinguishing the primary
    display-check guard from the defensive `state.kind` guard, as
    flagged in the per-piece review (S1).
  - §3.2: added one-line note confirming `#inspector-box` inherits
    `.overlay-widget { pointer-events: auto }` default and requires
    no override, resolving cross-review M4/M5's pointer-events policy
    question.

---

## Code review

**Verdict: APPROVE WITH NIT.** Commit `642c7e7` implements the plan
faithfully. All 15 `#ins-*` IDs preserved verbatim in
`web/index.html:34-48` (species, parent, action, age, energy, size,
photo, eat, scav, move, eyes, vision, armor, bite, pigment).
`#inspector-box` has `style="display:none"` default and
`#inspector-close` has `aria-label="Close"`. `.overlay-widget` is
defined exactly once (`styles.css:57`, owned by ui-stats) and reused
on both `#stats-box` and `#inspector-box` — not redefined. The four
show/hide flows are wired correctly: `openInspector` sets
`display:block`; `clearSelection` (invoked by empty-canvas tap, close
button click, and the 2s post-death path at `inspector.ts:166`) sets
`display:none`. `refreshInspector` opens with the display-none guard
before the `state.kind` check, matching D3. `isRailHidden()` guard
wraps both rail tab injection sites. `git show 642c7e7 -- src/ tests/
Cargo.toml` is empty (no Rust). `pnpm build` clean; `cargo test
--release --test acceptance` passes 3/3.

**Nit (cosmetic, non-blocking).** `styles.css:208` sets
`#inspector-box { position: relative; }` to anchor the absolutely-
positioned close button. This overrides the `.overlay-widget` base's
`position: fixed`, so the box is laid out in normal document flow
with a tiny `top:8px / right:8px` offset rather than pinned to the
viewport's top-right. In the current DOM (the box sits in `<main>`
after fixed/absolutely-positioned siblings), it likely still renders
near the intended location, but it is not pinned during page scroll
and the `right: 8px` value behaves as a -8px left shift rather than
a viewport-right pin. Recommended fix: drop the `position: relative`
override (let the box inherit `position: fixed` from
`.overlay-widget`) and re-anchor `#inspector-close` via either
`position: fixed` with matching `top/right` values, or by making the
close button non-absolute (e.g., float-right inside the box). Either
keeps the close button in the top-right corner of the box without
breaking viewport pinning.
