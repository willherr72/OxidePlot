<script lang="ts">
  import { onMount } from 'svelte';
  import type { Renderer, TableColumn } from '../renderer.js';

  export let renderer: Renderer;

  const ROW_H = 24;
  const OVERSCAN = 8;

  // ── State ─────────────────────────────────────────────────────────────────
  let columns: TableColumn[] = [];
  let rowCount = 0;
  let rows: string[][] = [];
  let first = 0;

  // Sort state: null = none, col index = which, asc/desc
  type SortDir = 'asc' | 'desc';
  let sortCol: number | null = null;
  let sortDir: SortDir | null = null;

  // Global search
  let searchTerm = '';
  let searchTimer: ReturnType<typeof setTimeout> | null = null;

  // Per-column filter state
  interface TextFilter { kind: 'text'; value: string }
  interface NumFilter { kind: 'num'; min: string; max: string }
  type ColFilterState = TextFilter | NumFilter;
  let colFilters: ColFilterState[] = [];
  let filterTimers: (ReturnType<typeof setTimeout> | null)[] = [];

  // Scroll container
  let scrollEl: HTMLDivElement;

  // ── Lifecycle ────────────────────────────────────────────────────────────
  onMount(() => {
    loadMeta();
  });

  /** Pull columns + rowCount from the renderer; re-render window. Called on mount and by App. */
  export function refresh() {
    loadMeta();
  }

  function loadMeta() {
    // Cancel any pending debounced timers before resetting state (Finding B)
    if (searchTimer) { clearTimeout(searchTimer); searchTimer = null; }
    for (let i = 0; i < filterTimers.length; i++) {
      if (filterTimers[i] !== null) { clearTimeout(filterTimers[i]!); filterTimers[i] = null; }
    }
    try {
      columns = renderer.tableColumns();
      rowCount = renderer.tableRowCount();
      // Initialise per-column filter state to match columns
      colFilters = columns.map(col =>
        col.numeric
          ? { kind: 'num' as const, min: '', max: '' }
          : { kind: 'text' as const, value: '' }
      );
      filterTimers = columns.map(() => null);
    } catch (e) {
      console.warn('TableView: failed to load meta', e);
      columns = [];
      rowCount = 0;
      colFilters = [];
      filterTimers = [];
    }
    fetchWindow();
  }

  function fetchWindow() {
    if (!scrollEl) return;
    const scrollTop = scrollEl.scrollTop;
    const clientHeight = scrollEl.clientHeight;
    first = Math.floor(scrollTop / ROW_H);
    const visible = Math.ceil(clientHeight / ROW_H) + OVERSCAN;
    try {
      rows = renderer.tableWindow(first, visible);
    } catch (e) {
      rows = [];
    }
  }

  function onScroll() {
    fetchWindow();
  }

  // ── Sort ──────────────────────────────────────────────────────────────────
  function handleHeaderClick(colIdx: number) {
    // Cycle: none → asc → desc → none
    if (sortCol !== colIdx) {
      // New column: start with asc
      sortCol = colIdx;
      sortDir = 'asc';
      renderer.tableSetSort(colIdx, true);
    } else if (sortDir === 'asc') {
      sortDir = 'desc';
      renderer.tableSetSort(colIdx, false);
    } else {
      // Was desc → clear
      sortCol = null;
      sortDir = null;
      renderer.tableClearSort();
    }
    resetScrollAndRefresh();
  }

  function sortCaret(colIdx: number): string {
    if (sortCol !== colIdx) return '⇅';
    if (sortDir === 'asc') return '↑';
    return '↓';
  }

  // ── Global search ─────────────────────────────────────────────────────────
  function onSearchInput() {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      try { renderer.tableSetSearch(searchTerm); } catch (_) {}
      resetScrollAndRefresh();
    }, 200);
  }

  // ── Per-column filters ────────────────────────────────────────────────────
  function onColFilterInput(colIdx: number) {
    if (filterTimers[colIdx]) clearTimeout(filterTimers[colIdx]!);
    filterTimers[colIdx] = setTimeout(() => {
      applyColFilter(colIdx);
    }, 200);
  }

  function applyColFilter(colIdx: number) {
    const state = colFilters[colIdx];
    if (!state) return;
    try {
      if (state.kind === 'text') {
        if (state.value.trim() === '') {
          renderer.tableSetColumnFilter(colIdx, null);
        } else {
          renderer.tableSetColumnFilter(colIdx, { text: state.value });
        }
      } else {
        const minVal = state.min.trim() === '' ? undefined : parseFloat(state.min);
        const maxVal = state.max.trim() === '' ? undefined : parseFloat(state.max);
        if (minVal === undefined && maxVal === undefined) {
          renderer.tableSetColumnFilter(colIdx, null);
        } else {
          const spec: { min?: number; max?: number } = {};
          if (minVal !== undefined && !isNaN(minVal)) spec.min = minVal;
          if (maxVal !== undefined && !isNaN(maxVal)) spec.max = maxVal;
          renderer.tableSetColumnFilter(colIdx, spec);
        }
      }
    } catch (e) {
      console.warn('TableView: filter error', e);
    }
    resetScrollAndRefresh();
  }

  // ── Helpers ───────────────────────────────────────────────────────────────
  function resetScrollAndRefresh() {
    if (scrollEl) scrollEl.scrollTop = 0;
    first = 0;
    try {
      rowCount = renderer.tableRowCount();
    } catch (_) {}
    fetchWindow();
  }

  $: spacerHeight = rowCount * ROW_H;
</script>

<div class="table-view">
  <!-- Search bar + row count -->
  <div class="table-topbar">
    <input
      class="search-input"
      type="text"
      placeholder="Search all columns…"
      bind:value={searchTerm}
      on:input={onSearchInput}
    />
    <span class="row-count">{rowCount.toLocaleString()} rows</span>
  </div>

  <!-- Scroll container -->
  <div
    class="scroll-container"
    bind:this={scrollEl}
    on:scroll={onScroll}
  >
    <!-- Sticky header -->
    <div class="thead">
      <!-- Column name row -->
      <div class="tr header-row">
        {#each columns as col, i}
          <div
            class="th"
            class:numeric={col.numeric}
            on:click={() => handleHeaderClick(i)}
            role="columnheader"
            tabindex="0"
            on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleHeaderClick(i); }}
            aria-sort={sortCol === i ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none'}
            title="Click to sort"
          >
            <span class="col-name">{col.name}</span>
            <span class="sort-caret" class:active={sortCol === i}>{sortCaret(i)}</span>
          </div>
        {/each}
      </div>
      <!-- Filter row -->
      <div class="tr filter-row">
        {#each columns as col, i}
          <div class="th filter-cell">
            {#if colFilters[i]?.kind === 'num'}
              <input
                class="filter-input num-filter"
                type="number"
                placeholder="min"
                bind:value={(colFilters[i] as { kind: 'num'; min: string; max: string }).min}
                on:input={() => onColFilterInput(i)}
              />
              <input
                class="filter-input num-filter"
                type="number"
                placeholder="max"
                bind:value={(colFilters[i] as { kind: 'num'; min: string; max: string }).max}
                on:input={() => onColFilterInput(i)}
              />
            {:else if colFilters[i]?.kind === 'text'}
              <input
                class="filter-input text-filter"
                type="text"
                placeholder="filter…"
                bind:value={(colFilters[i] as { kind: 'text'; value: string }).value}
                on:input={() => onColFilterInput(i)}
              />
            {/if}
          </div>
        {/each}
      </div>
    </div>

    <!-- Virtual scroller body -->
    <div class="tbody-spacer" style="height:{spacerHeight}px; position:relative;">
      {#each rows as row, ri}
        <div
          class="tr data-row"
          class:even={(first + ri) % 2 === 0}
          style="position:absolute; top:{(first + ri) * ROW_H}px; left:0; right:0; height:{ROW_H}px;"
        >
          {#each row as cell, ci}
            <div class="td" class:numeric={columns[ci]?.numeric}>
              {cell}
            </div>
          {/each}
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .table-view {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    background: var(--bg);
    color: var(--text);
    font-size: 0.82rem;
    overflow: hidden;
  }

  /* Top bar: search + count */
  .table-topbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 10px;
    background: var(--toolbar-bg);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .search-input {
    flex: 1;
    max-width: 360px;
    padding: 4px 8px;
    background: var(--btn-bg);
    border: 1px solid var(--btn-border);
    border-radius: 4px;
    color: var(--text);
    font-size: 0.82rem;
    outline: none;
  }

  .search-input::placeholder {
    color: var(--text-muted);
  }

  .search-input:focus {
    border-color: var(--btn-active-border);
  }

  .row-count {
    color: var(--text-muted);
    font-size: 0.78rem;
    white-space: nowrap;
    margin-left: auto;
  }

  /* Scroll container fills remaining height */
  .scroll-container {
    flex: 1;
    overflow: auto;
    position: relative;
  }

  /* Sticky header */
  .thead {
    position: sticky;
    top: 0;
    z-index: 10;
    background: var(--panel-bg);
    border-bottom: 2px solid var(--border-mid);
  }

  /* Row layout */
  .tr {
    display: flex;
    align-items: stretch;
    min-width: max-content;
  }

  /* Header cells — fixed width so header / filter / data columns all align. */
  .th {
    flex: 0 0 150px;
    width: 150px;
    min-width: 150px;
    max-width: 150px;
    padding: 4px 8px;
    border-right: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 4px;
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
    color: var(--text-dim);
    font-weight: 600;
    font-size: 0.80rem;
    background: var(--panel-bg);
    transition: background 0.1s;
  }

  .th:hover {
    background: var(--btn-hover-bg);
    color: var(--text);
  }

  .col-name {
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
  }

  .sort-caret {
    color: var(--text-muted);
    font-size: 0.72rem;
    flex-shrink: 0;
  }

  .sort-caret.active {
    color: var(--btn-active-text);
  }

  /* Filter row cells */
  .filter-cell {
    cursor: default;
    gap: 2px;
    padding: 3px 4px;
    background: var(--toolbar-bg);
  }

  .filter-cell:hover {
    background: var(--toolbar-bg);
    color: var(--text);
  }

  .filter-input {
    flex: 1;
    min-width: 0;
    width: 100%;
    padding: 2px 4px;
    background: var(--btn-bg);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text);
    font-size: 0.75rem;
    outline: none;
  }

  .filter-input::placeholder {
    color: var(--text-muted);
  }

  .filter-input:focus {
    border-color: var(--btn-active-border);
  }

  .num-filter {
    width: calc(50% - 2px);
    flex: none;
  }

  /* Body rows */
  .data-row {
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    min-width: max-content;
  }

  .data-row.even {
    background: color-mix(in srgb, var(--panel-bg) 50%, var(--bg) 50%);
  }

  .data-row:hover {
    background: var(--col-row-hover);
  }

  /* Data cells — same fixed width as headers so columns line up exactly. */
  .td {
    flex: 0 0 150px;
    width: 150px;
    min-width: 150px;
    max-width: 150px;
    height: 100%;
    padding: 0 8px;
    border-right: 1px solid var(--border);
    display: flex;
    align-items: center;
    font-family: var(--font-data);
    font-size: 0.78rem;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    box-sizing: border-box;
  }

  /* Freeze the first column (timestamp / index) as a horizontal-scroll anchor
     so you never lose which row is which across many columns. */
  .th:first-child,
  .td:first-child {
    position: sticky;
    left: 0;
    z-index: 5;
    background: var(--panel-bg);
    border-right: 2px solid var(--border-mid);
    flex: 0 0 200px;
    width: 200px;
    min-width: 200px;
    max-width: 200px;
  }
  .data-row.even .td:first-child {
    background: color-mix(in srgb, var(--panel-bg) 50%, var(--bg) 50%);
  }
  .thead .th:first-child {
    z-index: 20;
  }

  .td.numeric {
    justify-content: flex-end;
    font-variant-numeric: tabular-nums;
  }

  .th.numeric .col-name {
    text-align: right;
  }

  /* Spacer that holds the virtual scroll height */
  .tbody-spacer {
    width: 100%;
  }
</style>
