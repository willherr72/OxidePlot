# Phase 7 — Table View Design

**Date:** 2026-06-18
**Status:** Approved design → implementation plan next
**Roadmap:** Phase 7 of `2026-06-18-oxideplot-feature-roadmap.md`

## Goal

Add a data-table view of the loaded file (all columns/rows), reachable by a
toolbar toggle that swaps the main area between the plot and the table. The
table is a read-only inspector with virtualized rendering, column sort, a global
search filter, and per-column filters.

## Layout

- `App.svelte` gains `viewMode: 'plot' | 'table'` (default `'plot'`).
- A toolbar **"Table"** toggle button flips `viewMode`. When `'table'`, the
  `.canvas-wrap` (canvas + SVG overlays) is hidden and `<TableView>` fills the
  main area; the toolbar stays. The button is disabled when no file is loaded.
- Switching back to `'plot'` shows the canvas unchanged (the plot/view state is
  untouched while in table mode).

## Architecture & data flow

The parsed data lives in the WASM module (`OxidePlot.loaded: LoadedData`,
column-major `Vec<Vec<String>>` + column names). **All sort/filter work happens
in WASM**; the virtualized frontend pulls only the visible row slice. (Copying
the full dataset to JS to sort/filter there is rejected — it doesn't scale.)

WASM keeps a **view-order index**: `Vec<usize>` of original row indices in the
current sort + filter order. Search/filter/sort changes recompute this vector;
`table_window` slices it.

```
load file → table_columns() → headers
user sorts/searches/filters → WASM recomputes view-index → frontend resets scroll
frontend scroll → table_window(start, count) → visible rows (view order)
```

## WASM table API (`crates/oxideplot-wasm/src/lib.rs`, on `OxidePlot`)

Add a `TableState { sort: Option<(usize, bool)>, search: String, col_filters: HashMap<usize, ColFilter>, view_index: Vec<usize> }` (or equivalent) recomputed by a private `rebuild_table_index()`.

- `table_columns() -> JsValue` → `[{ name: String, kind: String }]` (reuse the
  existing kind inference; `kind` drives sort/filter behavior).
- `table_set_sort(col: usize, ascending: bool)` — set sort key; recompute index.
  **Numeric columns** (kind `numeric`/`datetime`) sort by parsed `f64`
  (non-finite/unparseable sort last); **text** columns sort lexically.
- `table_clear_sort()` — restore original row order.
- `table_set_search(term: String)` — case-insensitive substring match across all
  columns; empty term clears. Recompute index.
- `table_set_column_filter(col: usize, spec: JsValue)` — per-column filter:
  text columns → substring; numeric columns → `{ min?: f64, max?: f64 }` range.
  Passing null clears that column's filter. Recompute index.
- `table_row_count() -> usize` — number of rows after filtering.
- `table_window(start: usize, count: usize) -> JsValue` — `[[String]]` rows
  (each row = cells for all columns), in view order, clamped to bounds.

`rebuild_table_index()`: start from `0..row_count`, apply search + all column
filters (retain), then stable-sort by the sort key. Cheap to recompute on each
change (linear filter + sort).

`renderer.ts` wraps each method (`tableColumns`, `tableSetSort`, `tableClearSort`,
`tableSetSearch`, `tableSetColumnFilter`, `tableRowCount`, `tableWindow`).

## Frontend — `src/lib/components/TableView.svelte`

- **Virtualized scroller:** a scroll container with total height = `rowCount *
  rowHeight`; render only the rows in the visible window (computed from
  `scrollTop` + container height, plus a small overscan), positioned via a top
  spacer (or transform). On scroll, recompute the window and call
  `renderer.tableWindow(start, count)`. Fixed `rowHeight` for simple math.
- **Sticky header row:** column name + sort indicator (▲/▼/none); clicking
  cycles asc → desc → none (`tableSetSort`/`tableClearSort`). Under each header,
  a filter input: a text box for text columns, two small min/max number inputs
  for numeric columns (`tableSetColumnFilter`, debounced).
- **Global search box** above the table (`tableSetSearch`, debounced).
- After any sort/search/filter change: re-pull `tableRowCount()`, reset scroll to
  top, re-request the first window.
- Themed via the existing CSS variables (consistent with dark/light).

## Testing

WASM-side unit tests on `rebuild_table_index` (or the building blocks) over a
small fixture dataset:
- numeric sort orders by value, not lexically (`"2" < "10"`); descending reverses.
- text sort is lexical.
- search filters rows to those containing the term (case-insensitive).
- numeric column filter `{min,max}` retains only in-range rows; text column
  filter retains substring matches.
- combined search + column filter + sort produces the expected view-index.

## Non-goals (deferred to backlog B5)

Inline cell editing, column reorder/resize, conditional formatting, freeze
columns, copy-cell. This phase is a read-only inspector with sort/search/filter.
