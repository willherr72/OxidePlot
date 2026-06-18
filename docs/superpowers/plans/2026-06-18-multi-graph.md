# Phase 10 — Multi-Graph Workspace + Synced X Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** A workspace of stacked plot panels (each its own canvas+`OxidePlot`), with add/remove/focus and an optional synced X-axis.

**Architecture:** N-canvas — each graph is a `<Graph>` component owning a canvas + `OxidePlot` (reusing the whole single-graph engine). `App.svelte` becomes a workspace: a vertical stack of `<Graph>`s + the toolbar/panels routed to the **focused** graph. Sync forwards X-ranges between graphs.

**Tech Stack:** Svelte 5 + TS (most of the work); Rust (one small `set_x_range`).

## Global Constraints
- Branch `tauri-migration`. No egui/polars in core. `src/lib/wasm/` generated/gitignored — `build:wasm` before `build`/`tauri dev`. Theme via CSS vars. One commit per task; never commit a non-compiling tree.
- **Task 1 is regression-gated:** with exactly one graph, the app must behave EXACTLY as Phase 9 does today.

## File structure
- `src/lib/components/Graph.svelte` — NEW: one plot (canvas + OxidePlot + interaction + Axes/Cursors overlays).
- `src/App.svelte` — MODIFY: workspace (stack of Graphs, toolbar/panels routed to focused graph, add/remove/focus, Sync X).
- `crates/oxideplot-wasm/src/lib.rs` — MODIFY: `set_x_range`.
- `src/lib/renderer.ts` — MODIFY: `setXRange` wrapper.

---

## Task 1: Extract `Graph.svelte` (single graph, behavior-preserving)

**Files:** Create `src/lib/components/Graph.svelte`; modify `src/App.svelte`.

**Goal:** Move ALL per-plot logic out of `App.svelte` into a reusable `<Graph>`, then have `App` render exactly one `<Graph>` wired to the existing toolbar/panels. **No user-visible change** — this is a pure structural refactor.

**What moves into `Graph.svelte`** (currently inline in App):
- the `<canvas bind:this>` + `.canvas-wrap`, and the `<Axes>` + `<Cursors>` overlays;
- the `Renderer` instance for this graph;
- the pointer pan/zoom/cursor handlers, `pixelScale`, drag state, click-vs-drag, `cursors`, `cursorMode`;
- `viewState`, `ticks`, `refreshView`, the per-graph `onMount` (renderer `init`/`create`, initial render, `ResizeObserver`), and the canvas-local drag-drop listener (keep drag-drop here so a file dropped on a graph loads into it);
- the `viewMode` plot/table swap + `<TableView>` for THIS graph, and `showGrid`.

**What `Graph` exposes** (so the workspace toolbar/panels can drive the focused graph):
- `export const renderer: Renderer` (or a getter) — accessible via `bind:this={graphRef}` so App calls `graphRef.renderer.*`.
- Reactive, readable state for the focused-graph panels: `seriesInfo`, `viewState`, `drawMode`, `viewMode`, `showGrid`, etc. — expose via exported functions/props or a small `state` store the parent reads. (Simplest in Svelte 5: export accessor methods like `getSeriesInfo()` + a `refresh()` and let App pull on focus/after actions; OR export bindable state. Pick one and use it consistently.)
- Events: `on:focusrequest` (pointerdown anywhere on the graph → ask App to focus it), `on:xrange` (detail `{x_min, x_max}` after a view change — for Task 4), `on:datachanged` (after load/setSeries/transform — so App refreshes the focused-graph panels).
- Props: `focused: boolean` (draw a focus border when true).

**What stays in `App.svelte`** (workspace level, targets the focused graph's `renderer`):
- the toolbar (Open, Fit, draw-mode, Cursors, Settings, Table toggle, Export, Theme) — its plot actions call the focused graph's `renderer`/state;
- the panels: `ColumnDialog`, `SeriesList`, `Settings` — bound to the focused graph;
- the file open flow + recent files + prefs + theme (theme calls every graph's `setBackground`).

- [ ] **Step 1:** Create `Graph.svelte`, moving the per-plot markup + script listed above out of `App.svelte`. Give it the exposed renderer accessor, the events, and the `focused` prop. Keep its internal behavior identical to today.
- [ ] **Step 2:** Rewrite `App.svelte` to render a single `<Graph bind:this={focusedGraph} focused={true} on:datachanged={…} on:xrange={…} />` and wire the existing toolbar/panels to `focusedGraph.renderer` + the graph's exposed state. The file-open flow calls into the focused graph (e.g. `focusedGraph.loadBytes(bytes, name)` exposed by Graph, or App passes bytes to the graph). Theme applies to the graph's background.
- [ ] **Step 3: Verify** — `npm run build:wasm` → `npm run build` (no errors) → `cargo build` (workspace). **Regression visual (human, at phase end):** the one-graph app must do everything Phase 9 did (open, plot, pan/zoom/fit, cursors, draw modes, series list, fx transforms/resample, settings, table, export, theme, recent files, drag-drop) identically.
- [ ] **Step 4: Commit** — `feat: extract Graph component (single-graph behavior preserved)`.

> This is the riskiest task. If exposing the focused-graph state to App gets tangled, prefer: Graph owns all its state + a `refresh()`; App keeps a `focusedGraph` ref and calls `focusedGraph.renderer.*` then `focusedGraph.refresh()`; the panels receive their data via props the App reads from the focused graph after each action. Keep the data-flow one-directional and simple.

---

## Task 2: Workspace — stack, add/remove, focus, routing

**Files:** Modify `src/App.svelte`.

- [ ] **Step 1:** Replace the single graph with `graphs: { id: number }[]` (start with one) and `focusedId`. Render a `<Graph>` per entry in a **vertical flex stack** (equal heights), each with `focused={g.id === focusedId}`, `bind:this` stored in a map/array, and `on:focusrequest={() => focusedId = g.id}`.
- [ ] **Step 2:** Toolbar: **"Add Graph"** (push a new id; new graph starts empty), and a per-graph **remove ×** (disable when only one graph; removing the focused one moves focus to a neighbor). A monotonic id counter (do NOT reuse indices as ids — graphs get removed).
- [ ] **Step 3:** Route the toolbar plot-controls + panels (SeriesList/Settings/Table/ColumnDialog/Fit/draw-mode/cursors/export) to the **focused** graph's renderer/state. On focus change, refresh those panels from the newly-focused graph. Open File + theme remain workspace-global (theme calls `setBackground` on every graph).
- [ ] **Step 4: Verify** — builds green. Visual (human, phase end): add/remove graphs; focusing a graph highlights it and points the toolbar/panels at it; each graph plots independently.
- [ ] **Step 5: Commit** — `feat: multi-graph workspace (stack, add/remove, focus routing)`.

---

## Task 3: Shared file bytes

**Files:** Modify `src/App.svelte` (+ `Graph.svelte` if it needs a `loadBytes` entry point).

- [ ] **Step 1:** On Open File, read bytes once (Tauri `read_file`) and cache at the workspace (`loadedBytes: Uint8Array | null`, `loadedName`). Load them into the **focused** graph (its `OxidePlot.load_file_bytes` → its column dialog → its series).
- [ ] **Step 2:** When a file is cached, show a **"Use loaded data"** action on an empty graph (or have the focused-graph Open reuse the cache) so a newly-added graph can parse the cached bytes and pick its OWN series — one dataset, multiple panels. (Recent-files / drag-drop also feed the focused graph + cache.)
- [ ] **Step 3: Verify** — builds green. Visual (phase end): open a file once; add a graph; populate it from the loaded data with different series than graph 1.
- [ ] **Step 4: Commit** — `feat: shared workspace file bytes across graphs`.

---

## Task 4: Synced X-axis

**Files:** `crates/oxideplot-wasm/src/lib.rs`, `src/lib/renderer.ts`, `src/lib/components/Graph.svelte`, `src/App.svelte`.

- [ ] **Step 1 (wasm):** add `#[wasm_bindgen] pub fn set_x_range(&mut self, x_min: f64, x_max: f64)` to `OxidePlot`: set `self.view.x_min = x_min; self.view.x_max = x_max;` (leave Y), `self.rebuild_visible(); self.render();`. `renderer.ts`: `setXRange(min, max)`.
- [ ] **Step 2 (Graph):** emit `on:xrange` with `{x_min, x_max}` whenever the view's X changes (in the pan/zoom/fit paths, after `refreshView`). Add a `suppressXEmit` guard so an externally-applied range (`setXRange`) does NOT re-emit. Expose a method `applyXRange(min, max)` that sets `suppressXEmit`, calls `renderer.setXRange`, refreshes, then clears the flag.
- [ ] **Step 3 (App):** add a **"Sync X"** toolbar toggle (`syncX: boolean`, default false). On a graph's `xrange` event, if `syncX`, call `applyXRange(x_min, x_max)` on every OTHER graph. (Guard prevents feedback loops; also skip if the target's range already equals the incoming one.)
- [ ] **Step 4: Verify** — `npm run build:wasm` → `npm run build` → `cargo build`. Visual (phase end): with 2+ graphs sharing an X domain and Sync X on, pan/zoom one → the others' X follows; with Sync off, they're independent.
- [ ] **Step 5: Commit** — `feat: synced X-axis across graphs (set_x_range + workspace propagation)`.

---

## Self-Review
- Spec coverage: N-canvas Graph (T1), workspace stack/add/remove/focus/routing (T2), shared bytes (T3), synced X + set_x_range (T4). ✓
- Placeholders: T1 specifies exactly what moves/exposes/stays (a relocation refactor — the code exists in App; reproducing it verbatim here is not useful, so the plan enumerates the pieces + the Graph interface precisely). T4 has the exact wasm method + the feedback-guard mechanism. Tasks 2–3 give exact state/flow. No "TBD".
- Type consistency: `Graph` interface (renderer accessor, `focused` prop, `focusrequest`/`xrange`/`datachanged` events, `applyXRange`) used consistently across T1–T4; `set_x_range`/`setXRange` names match (wasm ↔ renderer ↔ Graph). Monotonic graph ids (not indices) called out in T2 to avoid stale-id bugs.
- Risk: T1 is the large refactor — regression-gated to single-graph parity; the fallback data-flow guidance is in the T1 note.
