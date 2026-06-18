# Phase 10 — Multi-Graph Workspace + Synced X-Axis Design

**Date:** 2026-06-18
**Status:** Approved (user delegated design) → plan next
**Roadmap:** Phase 10 of `2026-06-18-oxideplot-feature-roadmap.md` (the largest phase)

## Goal

Turn the single-plot app into a **workspace of stacked plot panels**, each with
its own series and Y-axis, with an optional **synced X-axis** (pan/zoom one →
the others follow). Add / remove / focus graphs.

## Key architectural decision: N-canvas (not N-viewport)

Each graph is its **own `<canvas>` + its own `OxidePlot`** instance (wgpu
surface + renderer + data). This **reuses the entire proven single-graph engine
per graph** — plotting, pan/zoom, viewport LTTB, axes, cursors, transforms,
table, settings all already work for one `OxidePlot`; a graph is just one of
them. The alternative (one renderer drawing N sub-viewports) would require
rewriting the renderer, overlays, and data model — far more work and risk.

**Cost:** N independent wgpu devices for N graphs. For realistic counts (2–8)
this is acceptable. (A shared-device optimization is backlog, not MVP.)

## Workspace model

`App.svelte` becomes a **workspace**:
- A **vertical stack** of graph panels (equal height for the MVP; resizable
  dividers are backlog).
- A workspace **toolbar** (the existing one, elevated to workspace level).
- The existing **panels** (series list, settings, table) and the plot-targeting
  toolbar controls (Fit, draw-mode, cursors, fx, export) act on the **focused
  graph**.
- **Focus:** clicking a graph focuses it (highlighted border). Exactly one graph
  is focused. Workspace-global controls: Open File, theme (applies to all
  graphs' backgrounds), Add Graph, Sync X.

## Components

### `src/lib/components/Graph.svelte` (new — extracted from `App.svelte`)
Encapsulates **one** plot: its `<canvas>`, its `OxidePlot` (create/render/resize),
the pointer pan/zoom/cursor handlers, and the `Axes` + `Cursors` SVG overlays.
Exposes (via `bind:this` / props / events): a `Renderer` accessor (so the
focused-graph's controls can drive it), a `focused` prop (for the border), an
`onFocus` event, and an `onXRangeChange` event (emits `{x_min, x_max}` after any
view change — used by sync). This is the **big refactor**: nearly all of the
current `App.svelte` canvas/interaction/overlay logic moves here, and `App`
renders one or more `<Graph>`.

### `App.svelte` (workspace)
- Holds `graphs: GraphHandle[]` (ids) and `focusedId`. Renders a `<Graph>` per
  entry in a vertical flex stack.
- Toolbar: **Add Graph** (append), per-graph **remove ×** (min 1 graph),
  **Sync X** toggle, plus the existing controls now routed to the focused graph.
- Series list / settings / table read & act on the **focused graph's** Renderer;
  switching focus refreshes them.

## Data flow (shared file, per-graph series)

- **Open File** reads the bytes once (Tauri `read_file`) and **caches them at the
  workspace**. It loads them into the **focused** graph (that graph's `OxidePlot`
  parses + shows its column dialog → series for that graph).
- **Add Graph** creates an empty graph. If a file is cached, an **"Use loaded
  data"** affordance (or the same Open flow reusing cached bytes) lets that graph
  parse the cached bytes and pick its own series.
- Net effect: one dataset, multiple panels each plotting a chosen subset of
  series. (Each graph parses the cached bytes independently — simple reuse;
  parsing is cheap. A shared parsed-data pool is a backlog optimization.)

## Synced X-axis

- A workspace **"Sync X"** toggle (default off).
- Each `<Graph>` emits `onXRangeChange({x_min, x_max})` after any X-changing op
  (pan/zoom/fit) — it already re-reads `viewState()` there.
- When Sync is **on**, the workspace forwards the emitting graph's X-range to all
  **other** graphs via a new `OxidePlot.set_x_range(x_min, x_max)` (sets the view
  X, keeps Y, `rebuild_visible()` + `render()`).
- **Feedback-loop guard:** programmatic `set_x_range` must NOT re-trigger an
  `onXRangeChange` that re-propagates. Guard with a "suppress emit" flag on the
  Graph while applying a synced range (or compare against the last-applied range
  and skip if unchanged).
- Most meaningful when graphs share an X domain (e.g., the same time file).

## WASM additions

- `OxidePlot.set_x_range(x_min: f64, x_max: f64)`: set `self.view.x_min/x_max`,
  leave Y, `rebuild_visible()` + `render()`. (Everything else is reused.)

## Testing

- Native: `set_x_range` adjusts only X (a small `PlotViewState`/OxidePlot-level
  check if feasible; otherwise covered by existing view tests + visual). Most of
  this phase is UI structure (Graph extraction, focus, stacking, sync wiring)
  verified by build + the human visual check.
- The Graph-extraction refactor must be **behavior-preserving** for a single
  graph: the app with one graph behaves exactly as Phase 9 does today (the
  regression bar for Task 1).

## Non-goals (backlog)

Resizable graph dividers, grid/tabbed/detachable layouts, a shared parsed-data
pool (vs per-graph parse), drag-series-between-graphs, per-graph independent
files (the model is one shared dataset), shared wgpu device across canvases.

## Decomposition (tasks — see the plan)

1. Extract `Graph.svelte` (canvas + OxidePlot + interaction + overlays); App
   renders ONE Graph, behavior-identical. *(Big refactor; regression-gated.)*
2. Workspace: stack of graphs, Add/remove, focus model, route toolbar/panels to
   the focused graph.
3. Shared file bytes (read once, load into focused graph; new graphs reuse the
   cached bytes).
4. `set_x_range` (wasm) + Sync X toggle + X-range propagation with feedback guard.
