# Phase 7 — Table View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only, virtualized data-table view of the loaded file (all columns/rows) with column sort, global search, and per-column filters, reachable by a toolbar toggle that swaps the main area between plot and table.

**Architecture:** The pure sort/search/filter "view-index" logic lives in `oxideplot-core` (native-unit-testable), consumed by a thin table API on the wasm `OxidePlot` wrapper, which serves windowed row slices to a virtualized `TableView.svelte`. App toggles `viewMode: 'plot' | 'table'`.

**Tech Stack:** Rust (oxideplot-core + oxideplot-wasm/wasm-bindgen), Svelte 5 + TypeScript.

## Global Constraints

- Branch: `tauri-migration`.
- Data sort/filter happens in Rust (core), NOT in JS; the frontend only pulls the visible row window.
- Pure table logic goes in `oxideplot-core` (so it's covered by native `cargo test`); the `#[cfg(target_arch="wasm32")]` wasm module only wraps it.
- `oxideplot-core` must keep building for native AND `wasm32-unknown-unknown`. No egui in core. No polars.
- Table is READ-ONLY (no cell editing — deferred to backlog B5).
- `src/lib/wasm/` is generated/gitignored — run `npm run build:wasm` before `npm run build` / `tauri dev`.
- Theme via existing CSS variables (dark/light).
- Commit cadence: one commit per task; never commit a non-compiling tree.

---

## File structure

- `crates/oxideplot-core/src/data/table.rs` — NEW: `ColFilter`, `TableQuery`, `compute_view_index`, `window_rows` + unit tests.
- `crates/oxideplot-core/src/data/mod.rs` — MODIFY: `pub mod table;`.
- `crates/oxideplot-wasm/src/lib.rs` — MODIFY: table state on `OxidePlot` + `table_*` wasm methods wrapping core.
- `src/lib/renderer.ts` — MODIFY: `table*` wrappers + types.
- `src/lib/components/TableView.svelte` — NEW: virtualized table UI.
- `src/App.svelte` — MODIFY: `viewMode` toggle, "Table" toolbar button, conditional render.

---

## Task 1: Core table logic (`oxideplot-core`) + tests

**Files:**
- Create: `crates/oxideplot-core/src/data/table.rs`
- Modify: `crates/oxideplot-core/src/data/mod.rs`

**Interfaces — Produces (used by Task 2):**
- `pub enum ColFilter { Text(String), Range { min: Option<f64>, max: Option<f64> } }`
- `pub struct TableQuery { pub sort: Option<(usize, bool)>, pub search: String, pub col_filters: std::collections::HashMap<usize, ColFilter>, pub numeric_cols: Vec<bool> }` (derives `Default`, `Clone`)
- `pub fn compute_view_index(data: &crate::data::loader::LoadedData, q: &TableQuery) -> Vec<usize>`
- `pub fn window_rows(data: &crate::data::loader::LoadedData, view_index: &[usize], start: usize, count: usize) -> Vec<Vec<String>>`

- [ ] **Step 1: Write the failing tests**

Create `crates/oxideplot-core/src/data/table.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::loader::LoadedData;
    use std::collections::HashMap;

    fn fixture() -> LoadedData {
        LoadedData {
            columns: vec!["n".into(), "name".into()],
            column_data: vec![
                vec!["2".into(), "10".into(), "1".into()],          // col 0: numeric
                vec!["bob".into(), "alice".into(), "carol".into()], // col 1: text
            ],
            row_count: 3,
        }
    }
    fn q() -> TableQuery { TableQuery { numeric_cols: vec![true, false], ..Default::default() } }

    #[test]
    fn numeric_sort_orders_by_value_not_lexically() {
        let d = fixture();
        let query = TableQuery { sort: Some((0, true)), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![2, 0, 1]); // 1,2,10
    }
    #[test]
    fn numeric_sort_descending_reverses() {
        let d = fixture();
        let query = TableQuery { sort: Some((0, false)), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![1, 0, 2]); // 10,2,1
    }
    #[test]
    fn text_sort_is_lexical() {
        let d = fixture();
        let query = TableQuery { sort: Some((1, true)), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![1, 0, 2]); // alice,bob,carol
    }
    #[test]
    fn search_filters_case_insensitive_across_columns() {
        let d = fixture();
        let query = TableQuery { search: "AL".into(), ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![1]); // "alice"
    }
    #[test]
    fn numeric_range_filter_retains_in_range() {
        let d = fixture();
        let mut f = HashMap::new();
        f.insert(0, ColFilter::Range { min: Some(2.0), max: None });
        let query = TableQuery { col_filters: f, ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![0, 1]); // 2,10
    }
    #[test]
    fn text_column_filter_substring() {
        let d = fixture();
        let mut f = HashMap::new();
        f.insert(1, ColFilter::Text("o".into()));
        let query = TableQuery { col_filters: f, ..q() };
        assert_eq!(compute_view_index(&d, &query), vec![0, 2]); // bob, carol
    }
    #[test]
    fn window_rows_slices_in_view_order() {
        let d = fixture();
        let rows = window_rows(&d, &[2, 0, 1], 0, 2);
        assert_eq!(rows, vec![
            vec!["1".to_string(), "carol".to_string()],
            vec!["2".to_string(), "bob".to_string()],
        ]);
    }
    #[test]
    fn window_rows_clamps_out_of_bounds() {
        let d = fixture();
        let rows = window_rows(&d, &[0, 1, 2], 2, 10);
        assert_eq!(rows.len(), 1); // only row at index 2 remains
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p oxideplot-core table::`
Expected: FAIL — `ColFilter` / `TableQuery` / `compute_view_index` / `window_rows` not defined.

- [ ] **Step 3: Implement the module**

Prepend to `table.rs` (above the test module):

```rust
//! Read-only table view logic over `LoadedData`: filtering (global search +
//! per-column) and sorting (numeric-aware) into a view-order row index, plus
//! windowed row extraction. Pure + native-testable; the wasm layer wraps this.

use std::collections::HashMap;
use crate::data::loader::LoadedData;

#[derive(Clone, Debug)]
pub enum ColFilter {
    /// Case-insensitive substring match.
    Text(String),
    /// Numeric range; `None` bound = unbounded. Unparseable cells fail the filter.
    Range { min: Option<f64>, max: Option<f64> },
}

#[derive(Clone, Default)]
pub struct TableQuery {
    /// (column index, ascending). None = original row order.
    pub sort: Option<(usize, bool)>,
    /// Global case-insensitive substring filter across all columns. Empty = off.
    pub search: String,
    /// Per-column filters (column index -> filter).
    pub col_filters: HashMap<usize, ColFilter>,
    /// Per-column: true if the column should sort/filter numerically.
    pub numeric_cols: Vec<bool>,
}

#[inline]
fn cell<'a>(data: &'a LoadedData, col: usize, row: usize) -> &'a str {
    data.column_data
        .get(col)
        .and_then(|c| c.get(row))
        .map(|s| s.as_str())
        .unwrap_or("")
}

fn is_numeric(q: &TableQuery, col: usize) -> bool {
    q.numeric_cols.get(col).copied().unwrap_or(false)
}

fn row_passes(data: &LoadedData, q: &TableQuery, row: usize) -> bool {
    // Global search: any column contains the term (case-insensitive).
    if !q.search.is_empty() {
        let needle = q.search.to_lowercase();
        let hit = (0..data.column_data.len())
            .any(|c| cell(data, c, row).to_lowercase().contains(&needle));
        if !hit {
            return false;
        }
    }
    // Per-column filters (all must pass).
    for (&col, filt) in &q.col_filters {
        let val = cell(data, col, row);
        let ok = match filt {
            ColFilter::Text(s) => {
                s.is_empty() || val.to_lowercase().contains(&s.to_lowercase())
            }
            ColFilter::Range { min, max } => match val.trim().parse::<f64>() {
                Ok(v) => min.map_or(true, |lo| v >= lo) && max.map_or(true, |hi| v <= hi),
                Err(_) => false,
            },
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Filtered (search + per-column) then sorted row indices, in display order.
pub fn compute_view_index(data: &LoadedData, q: &TableQuery) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..data.row_count).filter(|&r| row_passes(data, q, r)).collect();

    if let Some((col, ascending)) = q.sort {
        if is_numeric(q, col) {
            // Numeric: parse to f64; unparseable/non-finite sort to the end.
            let key = |r: &usize| cell(data, col, *r).trim().parse::<f64>().ok()
                .filter(|v| v.is_finite());
            idx.sort_by(|a, b| {
                let (ka, kb) = (key(a), key(b));
                let ord = match (ka, kb) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    (Some(_), None) => std::cmp::Ordering::Less,   // values before blanks
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                if ascending { ord } else { ord.reverse() }
            });
        } else {
            idx.sort_by(|a, b| {
                let ord = cell(data, col, *a).cmp(cell(data, col, *b));
                if ascending { ord } else { ord.reverse() }
            });
        }
    }
    idx
}

/// Extract `count` rows starting at `start` (clamped) from `view_index`, each row
/// = all columns' cells as owned strings.
pub fn window_rows(data: &LoadedData, view_index: &[usize], start: usize, count: usize) -> Vec<Vec<String>> {
    let end = start.saturating_add(count).min(view_index.len());
    let cols = data.column_data.len();
    view_index[start.min(view_index.len())..end]
        .iter()
        .map(|&r| (0..cols).map(|c| cell(data, c, r).to_string()).collect())
        .collect()
}
```

Add to `crates/oxideplot-core/src/data/mod.rs`: `pub mod table;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p oxideplot-core table::`
Expected: PASS (8 tests). Also run `cargo build -p oxideplot-core --target wasm32-unknown-unknown` → succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/oxideplot-core/src/data/table.rs crates/oxideplot-core/src/data/mod.rs
git commit -m "feat(core): table view-index logic (sort/search/filter) + tests"
```

---

## Task 2: WASM table API + renderer wrappers

**Files:**
- Modify: `crates/oxideplot-wasm/src/lib.rs`
- Modify: `src/lib/renderer.ts`

**Interfaces — Consumes (Task 1):** `oxideplot_core::data::table::{ColFilter, TableQuery, compute_view_index, window_rows}` and `OxidePlot.loaded: Option<LoadedData>`, the existing per-column kind inference (used to fill `numeric_cols`).
**Produces (Task 3):** the renderer.ts methods listed below.

- [ ] **Step 1: Add table state + methods to `OxidePlot`** (`crates/oxideplot-wasm/src/lib.rs`, inside `mod wasm_impl`)

Add fields to `OxidePlot`: `table_query: oxideplot_core::data::table::TableQuery` and `table_index: Vec<usize>` (init `TableQuery::default()` / `vec![]` in `create`). Import `use oxideplot_core::data::table::{ColFilter, TableQuery, compute_view_index, window_rows};`.

In `load_file_bytes`, after storing `self.loaded`, initialize `numeric_cols` from the parsed data (a column is numeric if `column_to_f64(col).1 >= 0.5` OR `column_to_timestamps(col).is_some()` — same rule used for `ColumnMeta.kind`), reset `table_query` to default-with-numeric_cols, and recompute `self.table_index`.

Add a private `fn rebuild_table_index(&mut self)` = `if let Some(d) = &self.loaded { self.table_index = compute_view_index(d, &self.table_query) }`.

Add `#[wasm_bindgen]` methods:

```rust
pub fn table_columns(&self) -> Result<JsValue, JsValue> {
    // reuse FileMeta-style {name, kind}; or {name} — Task 3 needs name + numeric flag.
    // Return [{ name: String, numeric: bool }] from self.loaded.columns + table_query.numeric_cols.
}
pub fn table_set_sort(&mut self, col: usize, ascending: bool) { self.table_query.sort = Some((col, ascending)); self.rebuild_table_index(); }
pub fn table_clear_sort(&mut self) { self.table_query.sort = None; self.rebuild_table_index(); }
pub fn table_set_search(&mut self, term: String) { self.table_query.search = term; self.rebuild_table_index(); }
pub fn table_set_column_filter(&mut self, col: usize, spec: JsValue) -> Result<(), JsValue> {
    // spec: null clears; { text: string } -> ColFilter::Text; { min?: number, max?: number } -> ColFilter::Range
    // deserialize via serde_wasm_bindgen into a small enum/option and update self.table_query.col_filters
    self.rebuild_table_index(); Ok(())
}
pub fn table_row_count(&self) -> usize { self.table_index.len() }
pub fn table_window(&self, start: usize, count: usize) -> Result<JsValue, JsValue> {
    let rows = self.loaded.as_ref().map(|d| window_rows(d, &self.table_index, start, count)).unwrap_or_default();
    serde_wasm_bindgen::to_value(&rows).map_err(|e| JsValue::from_str(&e.to_string()))
}
```

Use small `#[derive(Serialize)]`/`#[derive(Deserialize)]` structs for the column-info return and the filter spec. (For `table_set_column_filter`, a clean shape: `#[derive(Deserialize)] struct FilterSpec { text: Option<String>, min: Option<f64>, max: Option<f64> }` — `text` present → `ColFilter::Text`; else if min/max present → `ColFilter::Range`; all-None → remove the filter.)

- [ ] **Step 2: Add `renderer.ts` wrappers** (`src/lib/renderer.ts`)

```ts
export interface TableColumn { name: string; numeric: boolean; }
// in the Renderer class:
tableColumns(): TableColumn[] { return this.plot!.table_columns(); }
tableSetSort(col: number, ascending: boolean): void { this.plot!.table_set_sort(col, ascending); }
tableClearSort(): void { this.plot!.table_clear_sort(); }
tableSetSearch(term: string): void { this.plot!.table_set_search(term); }
tableSetColumnFilter(col: number, spec: { text?: string; min?: number; max?: number } | null): void { this.plot!.table_set_column_filter(col, spec); }
tableRowCount(): number { return this.plot!.table_row_count(); }
tableWindow(start: number, count: number): string[][] { return this.plot!.table_window(start, count); }
```

- [ ] **Step 3: Verify**

Run: `npm run build:wasm` (succeeds) → `npm run build` (succeeds) → `cargo build -p oxideplot-core --target wasm32-unknown-unknown` (succeeds).
(No new behavioral unit test here — the logic is tested in Task 1; this task is wasm/JS wiring.)

- [ ] **Step 4: Commit**

```bash
git add crates/oxideplot-wasm/src/lib.rs src/lib/renderer.ts
git commit -m "feat: wasm table API wrapping core table logic + renderer wrappers"
```

---

## Task 3: TableView.svelte (virtualized) + App toggle

**Files:**
- Create: `src/lib/components/TableView.svelte`
- Modify: `src/App.svelte`

**Interfaces — Consumes (Task 2):** the `renderer.table*` methods and `TableColumn`.

- [ ] **Step 1: Build `TableView.svelte`**

Props: `renderer` (the `Renderer` instance). On mount (and when told to refresh), pull `columns = renderer.tableColumns()` and `rowCount = renderer.tableRowCount()`.

Virtualization (fixed `ROW_H = 24` px):
- Outer scroll `<div>` (the only scroller); inner content height = `rowCount * ROW_H` via a spacer div.
- On `scroll`, compute `first = Math.floor(scrollTop / ROW_H)`, `visible = Math.ceil(clientHeight / ROW_H) + OVERSCAN(8)`; fetch `rows = renderer.tableWindow(first, visible)`; render those rows absolutely positioned at `top = (first + i) * ROW_H` (or via a translateY on a rows container).
- **Sticky header:** one cell per column showing `name` + a sort caret. Click cycles: none → asc → desc → none (`tableSetSort(col, true)` → `tableSetSort(col, false)` → `tableClearSort()`); after change, reset scroll to 0, re-pull `rowCount`, re-render window.
- **Per-column filter row** (under the header): numeric columns (`col.numeric`) get two small number inputs (min/max) → `tableSetColumnFilter(col, { min, max })` (omit empty bounds; both empty → `null`); text columns get a text input → `tableSetColumnFilter(col, { text })` (empty → `null`). Debounce ~200ms; after change reset scroll + re-pull count + window.
- **Global search box** above the table → `tableSetSearch(term)` debounced; same reset.
- Show `rowCount` (e.g. "1,234 rows" / "… of N" when filtered). Themed with existing CSS vars; monospace cells; horizontal scroll if many columns.

- [ ] **Step 2: Wire the toggle into `App.svelte`**

- Add `let viewMode: 'plot' | 'table' = 'plot';`.
- Toolbar: a **"Table"** toggle button (`class:active={viewMode==='table'}`, `disabled={!hasData}`) that flips `viewMode` between `'plot'` and `'table'`.
- In the markup: keep `.canvas-wrap` mounted but hidden when `viewMode==='table'` (e.g. `class:hidden` with `display:none`) so the GPU surface/state is preserved; render `<TableView {renderer} />` (guarded by `hasData`) when `viewMode==='table'`. (Do NOT unmount the canvas — re-creating the wgpu surface is expensive.)
- When switching to `'table'`, tell the TableView to refresh (e.g. bind a method or use a `{#key viewMode}` / reactive trigger so it re-pulls columns + count on show, since data may have changed since last shown).

- [ ] **Step 3: Verify**

Run: `npm run build:wasm` → `npm run build` (no errors) → `cargo build -p oxideplot-egui-legacy`… *(legacy is removed)* → instead `cargo build` (workspace) succeeds.
Visual check (human): toggle Table → see all columns/rows; scroll is smooth on a large file; click header sorts (numeric columns sort by value); search + per-column filters narrow rows; toggle back → plot intact.

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/TableView.svelte src/App.svelte
git commit -m "feat: virtualized table view + plot/table toggle"
```

---

## Self-Review

**Spec coverage:**
- Toggle plot/table swap → Task 3 (viewMode). ✓
- Full loaded file, all columns/rows → Tasks 1–3 (LoadedData). ✓
- Virtualized rendering → Task 3 (windowed `tableWindow`). ✓
- Column sort (numeric-aware vs lexical) → Task 1 (`compute_view_index`) + Task 3 (header). ✓
- Global search → Task 1 + Task 3. ✓
- Per-column filter (text substring / numeric range) → Task 1 (`ColFilter`) + Task 3. ✓
- Sort/filter in Rust, windowed access → Tasks 1–2. ✓
- Read-only (no cell editing) → enforced (no edit path). ✓
- Native-testable logic in core → Task 1. ✓

**Placeholder scan:** Task 1 has complete code + real assertion tests. Tasks 2–3 give exact method signatures + the virtualization formula + exact UI behaviors; the wasm method bodies for `table_columns`/`table_set_column_filter` describe the exact serde shapes to (de)serialize rather than full boilerplate, which is appropriate (mechanical glue around the Task-1 API). No "TBD"/"handle edge cases".

**Type consistency:** `ColFilter`/`TableQuery`/`compute_view_index`/`window_rows` (Task 1) are consumed verbatim in Task 2; `renderer.table*` names + `TableColumn`/filter-spec shapes (Task 2) match Task 3's usage. Sort cycle (none→asc→desc→none) consistent between spec and Task 3.

**Fix applied during review:** Task 3 Step 3 originally referenced building the now-deleted `oxideplot-egui-legacy` crate; corrected to `cargo build` (workspace).
