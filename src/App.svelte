<script lang="ts">
  import { onMount } from 'svelte';
  import { pickFile, readFile, saveFile, loadPrefs, savePrefs } from './lib/api.js';
  import type { SeriesSpec, ViewState, SeriesInfoEntry } from './lib/renderer.js';
  import type { FileMeta } from './lib/renderer.js';
  import Graph from './lib/components/Graph.svelte';
  import ColumnDialog from './lib/components/ColumnDialog.svelte';
  import SeriesList from './lib/components/SeriesList.svelte';
  import Settings from './lib/components/Settings.svelte';

  // ── Workspace: a vertical stack of graphs ────────────────────────────────────
  // The workspace renders a `<Graph>` per entry in `graphs`, stacked vertically
  // (equal heights). Exactly one graph is "focused" at a time; the toolbar/panels
  // all target the focused graph. App holds a ref per graph (keyed by id), calls
  // the focused graph's methods for actions, then re-pulls its exposed state to
  // feed the panels (one-directional data-flow).
  //
  // IMPORTANT: graph identity is by `id` (a monotonic counter), NOT array index —
  // graphs get removed, so indices are unstable.
  let graphs: { id: number }[] = [{ id: 0 }];
  let focusedId = 0;
  let nextId = 1;

  /** Component instances keyed by graph id, populated via `bind:this`. */
  let graphRefs: Record<number, Graph> = {};

  /** The focused graph's component instance (may be undefined momentarily right
   *  after add/remove or before mount — callers/markup must guard). */
  $: focusedGraph = graphRefs[focusedId] as Graph | undefined;

  // ── Panel-facing state (mirrored FROM the focused graph after each action) ─────
  let seriesInfo: SeriesInfoEntry[] = [];
  let viewState: ViewState | null = null;
  let hasData = false;
  let drawMode: 'lines' | 'step' | 'points' = 'lines';
  let viewMode: 'plot' | 'table' = 'plot';
  let cursorMode = false;
  // Appearance (mirrors of the focused graph's settings; seeded with the
  // graph's defaults so the Settings panel shows correct initial values).
  let showGrid = true;
  let lineWidth = 2.0;
  let pointRadius = 3.0;
  let normalized = false;

  /** Pull all panel-facing state from the focused graph.
   *  Reads `graphRefs[focusedId]` directly (not the reactive `focusedGraph`
   *  alias) so it always sees the just-assigned `focusedId` even when called
   *  synchronously inside setFocus/removeGraph, before reactive statements run. */
  function syncFromGraph() {
    const g = graphRefs[focusedId];
    if (!g) return;
    g.refresh();
    seriesInfo = g.getSeriesInfo();
    viewState = g.getViewState();
    hasData = g.getHasData();
    drawMode = g.getDrawMode();
    viewMode = g.getViewMode();
    cursorMode = g.getCursorMode();
    showGrid = g.getShowGrid();
    lineWidth = g.getLineWidth();
    pointRadius = g.getPointRadius();
    normalized = g.getNormalized();
    const err = g.getError();
    if (err) error = err;
  }

  // ── Workspace: add / remove / focus ──────────────────────────────────────────

  /** Focus the graph with the given id and re-pull panel state from it so the
   *  toolbar/panels reflect the newly-focused graph. */
  function setFocus(id: number) {
    if (focusedId === id) return;
    focusedId = id;
    // The newly-focused graph is already mounted; re-pull its exposed state.
    syncFromGraph();
  }

  /** Append a new (empty) graph and focus it. */
  function addGraph() {
    const id = nextId++;
    graphs = [...graphs, { id }];
    focusedId = id;
    // graphRefs[id] mounts on the next tick; syncFromGraph guards on undefined,
    // and the new graph fires `ready` → handleGraphReady which syncs + themes it.
  }

  /** Remove the graph with the given id. Disabled when only one graph remains.
   *  If the removed graph was focused, focus moves to a neighbor. */
  function removeGraph(id: number) {
    if (graphs.length <= 1) return;
    const idx = graphs.findIndex(g => g.id === id);
    if (idx === -1) return;

    const wasFocused = id === focusedId;
    graphs = graphs.filter(g => g.id !== id);
    delete graphRefs[id]; // clean up the dangling ref
    graphRefs = graphRefs; // nudge reactivity

    if (wasFocused) {
      // Move focus to a neighbor (prefer the previous one, else the new first).
      const neighbor = graphs[Math.min(idx, graphs.length - 1)];
      focusedId = neighbor.id;
      syncFromGraph();
    }
  }

  let fileMeta: FileMeta | null = null;
  let filePath: string | null = null;
  let error: string | null = null;
  let loading = false;

  // ── Workspace-level byte cache (shared dataset) ───────────────────────────────
  // Bytes are read from disk once on open and cached here. A newly-added (empty)
  // graph can reuse the cached bytes to parse the same dataset and pick its own
  // columns, without re-reading from disk.
  let loadedBytes: Uint8Array | null = null;
  let loadedName = '';

  /** True when the cache has bytes AND the focused graph has no series (empty). */
  $: canUseLoadedData = loadedBytes !== null && seriesInfo.length === 0;

  // ── Prefs ──────────────────────────────────────────────────────────────────
  interface Prefs {
    recentFiles: string[];
    theme: string;
  }
  const DEFAULT_PREFS: Prefs = { recentFiles: [], theme: 'dark' };
  let prefs: Prefs = { ...DEFAULT_PREFS };
  let showRecent = false;

  /** Per-theme WebGPU background color [r, g, b, a]. */
  const THEME_BG: Record<string, [number, number, number, number]> = {
    dark:  [0.10, 0.10, 0.12, 1.0],
    light: [0.97, 0.97, 0.98, 1.0],
  };

  /** Apply the given theme to the document root and (if a graph is ready)
   *  update the WebGPU clear color and re-render. Theme is workspace-global:
   *  the background is pushed to EVERY graph in the stack. */
  function applyTheme(theme: string, renderNow = false) {
    document.documentElement.setAttribute('data-theme', theme);
    const bg = THEME_BG[theme] ?? THEME_BG['dark'];
    for (const g of graphs) {
      graphRefs[g.id]?.setBackground(bg[0], bg[1], bg[2], bg[3], renderNow);
    }
  }

  /** Persist current prefs to disk. */
  async function persistPrefs() {
    try {
      await savePrefs(JSON.stringify(prefs));
    } catch (e) {
      console.warn('Failed to persist prefs:', e);
    }
  }

  /** Toggle between dark and light themes, persist, and re-render. */
  async function toggleTheme() {
    prefs = { ...prefs, theme: prefs.theme === 'dark' ? 'light' : 'dark' };
    applyTheme(prefs.theme, true);
    await persistPrefs();
  }

  /** Add path to recent files (dedupe, cap at 8), then persist. */
  async function recordRecentFile(path: string) {
    const filtered = prefs.recentFiles.filter(p => p !== path);
    prefs = { ...prefs, recentFiles: [path, ...filtered].slice(0, 8) };
    await persistPrefs();
  }

  // ── Settings panel ─────────────────────────────────────────────────────────
  let showSettings = false;

  function toggleSettings() {
    showSettings = !showSettings;
  }

  function handleLineWidth(event: CustomEvent<{ value: number }>) {
    focusedGraph?.setLineWidth(event.detail.value);
    syncFromGraph();
  }

  function handlePointRadius(event: CustomEvent<{ value: number }>) {
    focusedGraph?.setPointRadius(event.detail.value);
    syncFromGraph();
  }

  function handleShowGrid(event: CustomEvent<{ value: boolean }>) {
    focusedGraph?.setShowGrid(event.detail.value);
    syncFromGraph();
  }

  function handleNormalized(event: CustomEvent<{ value: boolean }>) {
    focusedGraph?.setNormalized(event.detail.value);
    syncFromGraph();
  }

  // ── View mode (plot / table) ───────────────────────────────────────────────
  async function toggleViewMode() {
    await focusedGraph?.toggleViewMode();
    syncFromGraph();
  }

  // ── Draw mode ──────────────────────────────────────────────────────────────
  const DRAW_MODE_LABELS: Record<'lines' | 'step' | 'points', string> = {
    lines: 'Lines', step: 'Step', points: 'Points',
  };

  function cycleDrawMode() {
    focusedGraph?.cycleDrawMode();
    syncFromGraph();
  }

  function handleFit() {
    focusedGraph?.fit();
    syncFromGraph();
  }

  // ── Cursor mode ────────────────────────────────────────────────────────────
  function toggleCursorMode() {
    focusedGraph?.toggleCursorMode();
    syncFromGraph();
  }

  // ── Sync X (Task 4) ──────────────────────────────────────────────────────────
  /** When true, panning/zooming any graph also sets the same X-range on all others. */
  let syncX = false;

  function toggleSyncX() {
    syncX = !syncX;
    // When turning ON, immediately align all other graphs to the focused graph's
    // current X-range so they snap into sync without requiring the user to pan first.
    if (syncX) {
      const focused = graphRefs[focusedId];
      if (!focused) return;
      const vs = focused.getViewState();
      if (!vs) return;
      for (const g of graphs) {
        if (g.id !== focusedId) {
          graphRefs[g.id]?.applyXRange(vs.x_min, vs.x_max);
        }
      }
    }
  }

  // ── Graph events ─────────────────────────────────────────────────────────────
  function handleXRange(emittingId: number, detail: { x_min: number; x_max: number }) {
    // Only propagate when Sync X is on; never call back to the emitting graph.
    if (!syncX) return;
    for (const g of graphs) {
      if (g.id === emittingId) continue;
      graphRefs[g.id]?.applyXRange(detail.x_min, detail.x_max);
    }
  }

  function handleDataChanged(id: number) {
    // A graph's data changed; only the focused graph drives the panels.
    if (id === focusedId) syncFromGraph();
  }

  /** A graph's renderer is live — push the persisted-theme background to it.
   *  If it is the focused graph, also sync the panels from it. */
  function handleGraphReady(id: number) {
    const bg = THEME_BG[prefs.theme] ?? THEME_BG['dark'];
    graphRefs[id]?.setBackground(bg[0], bg[1], bg[2], bg[3], true);
    if (id === focusedId) syncFromGraph();
  }

  /** A file dropped on a graph → focus that graph, then run the open flow
   *  targeting it. The drop targets THE GRAPH THAT EMITTED, not the focused one
   *  (the Graph also emits `focusrequest` before `droppath`, but set focus here
   *  too so openPath loads into the dropped-on graph). */
  function handleDropPath(id: number, event: CustomEvent<{ path: string }>) {
    setFocus(id);
    void openPath(event.detail.path);
  }

  /** SeriesList mutated the focused graph's renderer (visibility/remove/move/fx). */
  function handleSeriesChange() {
    syncFromGraph();
  }

  onMount(async () => {
    // Load prefs first so recent files are available immediately.
    try {
      const txt = await loadPrefs();
      let parsed: Partial<Prefs> = {};
      try { parsed = JSON.parse(txt); } catch (_) {}
      prefs = { ...DEFAULT_PREFS, ...parsed };
    } catch (_) {
      // non-fatal: use defaults
    }

    // Apply persisted theme to chrome immediately (graph may not be ready yet).
    document.documentElement.setAttribute('data-theme', prefs.theme);
    // Apply the persisted-theme WebGPU background. Child `onMount` (the graph's
    // renderer init) and this parent `onMount` race; applyTheme is idempotent and
    // setBackground no-ops if the renderer isn't ready, and the graph also fires
    // `ready` → handleGraphReady, so whichever finishes last sets it correctly.
    applyTheme(prefs.theme, true);
  });

  /** Load a file at a known path (shared by dialog-pick, recent-click, drag-drop). */
  async function openPath(path: string) {
    const g = focusedGraph;
    if (!g) return;
    loading = true;
    error = null;
    try {
      filePath = path;
      const numArr = await readFile(path);
      const bytes = new Uint8Array(numArr);
      const filename = path.split(/[\\/]/).pop() ?? path;
      fileMeta = g.loadBytes(bytes, filename);
      // Cache the bytes at workspace level (only after a successful parse) so
      // other (empty) graphs can reuse them without re-reading from disk.
      loadedBytes = bytes;
      loadedName = filename;
      await recordRecentFile(path);
    } catch (e) {
      error = `Failed to open file: ${e}`;
      // If a recent file is now inaccessible, drop it from the list.
      prefs = { ...prefs, recentFiles: prefs.recentFiles.filter(p => p !== path) };
      await persistPrefs();
    } finally {
      loading = false;
    }
  }

  async function handleOpen() {
    error = null;
    loading = true;
    try {
      const path = await pickFile();
      if (!path) { loading = false; return; }
      await openPath(path);
    } catch (e) {
      error = `Failed to open file: ${e}`;
      loading = false;
    }
  }

  async function handleOpenRecent(path: string) {
    showRecent = false;
    await openPath(path);
  }

  function handleConfirm(event: CustomEvent<SeriesSpec[]>) {
    const specs = event.detail;
    fileMeta = null; // close dialog
    error = null;
    const g = focusedGraph;
    if (!g) return;
    try {
      g.setSeries(specs);
      syncFromGraph();
    } catch (e) {
      error = `Failed to render series: ${e}`;
    }
  }

  function handleCancel() {
    fileMeta = null;
  }

  /** Load the cached bytes into the focused (empty) graph so the user can pick
   *  their own columns from the same dataset — identical flow to a fresh open,
   *  no disk read. Only callable when `canUseLoadedData` is true. */
  function handleUseLoadedData() {
    const g = focusedGraph;
    if (!g || !loadedBytes) return;
    error = null;
    try {
      fileMeta = g.loadBytes(loadedBytes, loadedName);
    } catch (e) {
      error = `Failed to load cached data: ${e}`;
    }
  }

  // ── Export ─────────────────────────────────────────────────────────────────

  async function handleExportCsv() {
    if (!hasData || !focusedGraph) return;
    error = null;
    try {
      const csv = focusedGraph.exportCsv();
      const bytes = new TextEncoder().encode(csv);
      await saveFile('oxideplot.csv', bytes);
    } catch (e) {
      error = `Export CSV failed: ${e}`;
    }
  }

  async function handleExportPng() {
    if (!hasData || !focusedGraph) return;
    error = null;
    try {
      const blob = await focusedGraph.capturePng();
      if (!blob) {
        error = 'PNG capture returned null — the WebGPU canvas may not support toBlob.';
        return;
      }
      const arrayBuf = await blob.arrayBuffer();
      const bytes = new Uint8Array(arrayBuf);
      await saveFile('oxideplot.png', bytes);
    } catch (e) {
      error = `Export PNG failed: ${e}`;
    }
  }

  async function handleCopy() {
    if (!hasData || !focusedGraph) return;
    error = null;
    try {
      const blob = await focusedGraph.capturePng();
      if (!blob) {
        error = 'PNG capture returned null — clipboard copy unavailable.';
        return;
      }
      if (typeof navigator.clipboard === 'undefined' || !navigator.clipboard.write) {
        error = 'Clipboard API unavailable in this context.';
        return;
      }
      await navigator.clipboard.write([
        new ClipboardItem({ 'image/png': blob }),
      ]);
    } catch (e) {
      error = `Copy to clipboard failed: ${e}`;
    }
  }
</script>

<main>
  <!-- Toolbar -->
  <div class="toolbar">
    <button class="open-btn" on:click={handleOpen} disabled={loading}>
      {loading ? 'Loading…' : 'Open File'}
    </button>
    {#if prefs.recentFiles.length > 0}
      <div class="recent-wrap">
        <button
          class="cursor-btn"
          on:click={() => (showRecent = !showRecent)}
          title="Recent files"
        >
          Recent ▾
        </button>
        {#if showRecent}
          <!-- svelte-ignore a11y-no-static-element-interactions -->
          <div class="recent-dropdown" on:mouseleave={() => (showRecent = false)}>
            {#each prefs.recentFiles as rpath}
              <button
                class="recent-item"
                title={rpath}
                on:click={() => handleOpenRecent(rpath)}
              >
                {rpath.split(/[\\/]/).pop() ?? rpath}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
    <button
      class="cursor-btn"
      on:click={addGraph}
      title="Add a new graph below"
    >
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
      Add Graph
    </button>
    {#if canUseLoadedData}
      <button
        class="cursor-btn"
        on:click={handleUseLoadedData}
        title="Load the cached dataset into this graph so you can pick its own series"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
        Use loaded data
      </button>
    {/if}
    <button
      class="cursor-btn"
      disabled={!hasData}
      on:click={handleFit}
      title="Re-fit view to all data (same as double-click)"
    >
      Fit
    </button>
    <button
      class="cursor-btn"
      class:active={syncX}
      on:click={toggleSyncX}
      title={syncX ? 'Sync X ON — all graphs share the same X-range (click to disable)' : 'Sync X OFF — pan/zoom one graph to sync all others'}
    >
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
      Sync X
    </button>
    <button
      class="cursor-btn"
      class:active={cursorMode}
      on:click={toggleCursorMode}
      title={cursorMode ? 'Cursor mode ON — click to place cursors (toggle off to clear)' : 'Cursor mode OFF'}
    >
      Cursors
    </button>
    <button
      class="draw-mode-btn"
      disabled={!hasData}
      on:click={cycleDrawMode}
      title="Cycle draw mode: Lines → Step → Points"
    >
      {DRAW_MODE_LABELS[drawMode]}
    </button>
    <button
      class="cursor-btn"
      class:active={viewMode === 'table'}
      disabled={!hasData}
      on:click={toggleViewMode}
      title={viewMode === 'table' ? 'Switch to plot view' : 'Switch to table view'}
    >
      Table
    </button>
    <button
      class="cursor-btn"
      class:active={showSettings}
      on:click={toggleSettings}
      title="Toggle settings panel"
    >
      Settings
    </button>
    <button
      class="cursor-btn"
      disabled={!hasData}
      on:click={handleExportCsv}
      title="Export all series to CSV"
    >
      Export CSV
    </button>
    <button
      class="cursor-btn"
      disabled={!hasData}
      on:click={handleExportPng}
      title="Save plot as PNG (note: WebGPU canvas — verify image is not blank)"
    >
      Export PNG
    </button>
    <button
      class="cursor-btn"
      disabled={!hasData}
      on:click={handleCopy}
      title="Copy plot PNG to clipboard"
    >
      Copy
    </button>
    <!-- Theme toggle -->
    <button
      class="theme-btn"
      on:click={toggleTheme}
      title={prefs.theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
      aria-label={prefs.theme === 'dark' ? 'Light mode' : 'Dark mode'}
    >
      {#if prefs.theme === 'dark'}
        <!-- sun icon: click to switch to light -->
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
      {:else}
        <!-- moon icon: click to switch to dark -->
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
      {/if}
    </button>
    {#if filePath && !fileMeta}
      <span class="file-label" title={filePath}>
        {filePath.split(/[\\/]/).pop()}
      </span>
    {/if}
    {#if error}
      <span class="error-msg">{error}</span>
    {/if}
  </div>

  <!-- Workspace: a vertical stack of graphs plus the focused graph's overlay
       panels. The relative wrapper gives the absolutely-positioned
       SeriesList/Settings panels (which target the focused graph) a positioning
       context over the plot area. Each graph is keyed by id (stable across
       removals) and flexes to equal height. -->
  <div class="workspace">
    <div class="graph-stack">
      {#each graphs as g (g.id)}
        <!-- svelte-ignore a11y-no-static-element-interactions -->
        <div class="graph-slot">
          <Graph
            bind:this={graphRefs[g.id]}
            focused={g.id === focusedId}
            on:ready={() => handleGraphReady(g.id)}
            on:focusrequest={() => setFocus(g.id)}
            on:xrange={(e) => handleXRange(g.id, e.detail)}
            on:datachanged={() => handleDataChanged(g.id)}
            on:droppath={(e) => handleDropPath(g.id, e)}
          />
          {#if graphs.length > 1}
            <button
              class="remove-graph-btn"
              on:click|stopPropagation={() => removeGraph(g.id)}
              title="Remove this graph"
              aria-label="Remove graph"
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
            </button>
          {/if}
        </div>
      {/each}
    </div>

    <!-- Focused-graph panels (hidden in table mode, matching prior behavior). -->
    {#if viewMode === 'plot' && focusedGraph}
      {#if seriesInfo.length > 0}
        <SeriesList
          series={seriesInfo}
          renderer={focusedGraph.renderer}
          on:change={handleSeriesChange}
        />
      {/if}
      {#if showSettings}
        <Settings
          {lineWidth}
          {pointRadius}
          {showGrid}
          {normalized}
          on:linewidth={handleLineWidth}
          on:pointradius={handlePointRadius}
          on:showgrid={handleShowGrid}
          on:normalized={handleNormalized}
        />
      {/if}
    {/if}
  </div>

  <!-- Column-selection dialog -->
  {#if fileMeta}
    <ColumnDialog
      meta={fileMeta}
      on:confirm={handleConfirm}
      on:cancel={handleCancel}
    />
  {/if}
</main>

<style>
  /* ── CSS custom properties — dark theme (default) ── */
  :global(:root),
  :global(:root[data-theme="dark"]) {
    --bg: #1a1a1f;
    --panel-bg: #16161e;
    --panel-bg-alpha: rgba(20, 20, 30, 0.88);
    --toolbar-bg: #111118;
    --text: #e0e0ee;
    --text-muted: #8888aa;
    --text-dim: #b0b0cc;
    --border: #2a2a3a;
    --border-mid: #3a3a50;
    --btn-bg: #2a2a3a;
    --btn-border: #44445a;
    --btn-hover-bg: #3a3a52;
    --btn-hover-text: #e0e0ff;
    --btn-active-bg: #1a4a60;
    --btn-active-text: #00e5ff;
    --btn-active-border: #00b8d9;
    --accent: #3060c0;
    --recent-bg: #1a1a28;
    --recent-item-hover: #2a2a42;
    --axis-line-major: rgba(220, 220, 240, 0.85);
    --axis-line-minor: rgba(180, 180, 200, 0.5);
    --axis-text: rgba(200, 200, 220, 0.9);
    --axis-text-stroke: rgba(10, 10, 18, 0.7);
    --grid-line: rgba(255, 255, 255, 0.06);
    --cursor-dot-stroke: rgba(10, 10, 18, 0.7);
    --cursor-readout-bg: rgba(10, 10, 20, 0.82);
    --cursor-readout-border: rgba(180, 180, 220, 0.25);
    --cursor-readout-text: #d0d0ee;
    --cursor-readout-vals: #c0c0dd;
    --cursor-divider: rgba(180, 180, 220, 0.2);
    --cursor-delta-label: #aaaacc;
    --cursor-delta-vals: #e0e0ff;
    --series-row-hover: rgba(60, 60, 90, 0.4);
    --series-ctrl-btn: #8888aa;
    --series-ctrl-hover-bg: rgba(80, 80, 120, 0.5);
    --series-name-text: #d0d0ee;
    --swatch-border: rgba(255, 255, 255, 0.15);
    --settings-header: #a0a0cc;
    --settings-label: #a8a8c4;
    --settings-val: #7878a0;
    --dialog-bg: #1e1e28;
    --dialog-overlay: rgba(0, 0, 0, 0.7);
    --dialog-text: #e0e0ee;
    --dialog-h2: #ffffff;
    --dialog-subtitle: #7a7a9a;
    --dialog-section-title: #8888aa;
    --col-row-hover: #2a2a3a;
    --col-row-selected: #252540;
    --col-kind-numeric-bg: #1a3a1a;
    --col-kind-numeric-text: #60dd60;
    --col-kind-datetime-bg: #1a2a3a;
    --col-kind-datetime-text: #60aadd;
    --col-kind-text-bg: #3a2a1a;
    --col-kind-text-text: #ddaa60;
    --btn-cancel-bg: #2e2e44;
    --btn-cancel-text: #aaaacc;
  }

  /* ── CSS custom properties — light theme ── */
  :global(:root[data-theme="light"]) {
    --bg: #f5f5f7;
    --panel-bg: #ffffff;
    --panel-bg-alpha: rgba(255, 255, 255, 0.92);
    --toolbar-bg: #e8e8ed;
    --text: #1a1a1f;
    --text-muted: #666688;
    --text-dim: #444455;
    --border: #d0d0d8;
    --border-mid: #b0b0bc;
    --btn-bg: #e0e0e8;
    --btn-border: #b8b8c8;
    --btn-hover-bg: #d0d0dc;
    --btn-hover-text: #111118;
    --btn-active-bg: #cce4f0;
    --btn-active-text: #006688;
    --btn-active-border: #0088bb;
    --accent: #3060c0;
    --recent-bg: #f0f0f5;
    --recent-item-hover: #e0e0ea;
    --axis-line-major: rgba(40, 40, 60, 0.8);
    --axis-line-minor: rgba(60, 60, 80, 0.4);
    --axis-text: rgba(20, 20, 40, 0.9);
    --axis-text-stroke: rgba(245, 245, 248, 0.85);
    --grid-line: rgba(0, 0, 0, 0.07);
    --cursor-dot-stroke: rgba(245, 245, 248, 0.85);
    --cursor-readout-bg: rgba(255, 255, 255, 0.90);
    --cursor-readout-border: rgba(80, 80, 100, 0.25);
    --cursor-readout-text: #222230;
    --cursor-readout-vals: #333344;
    --cursor-divider: rgba(80, 80, 100, 0.2);
    --cursor-delta-label: #555566;
    --cursor-delta-vals: #111120;
    --series-row-hover: rgba(160, 160, 200, 0.2);
    --series-ctrl-btn: #555566;
    --series-ctrl-hover-bg: rgba(100, 100, 160, 0.15);
    --series-name-text: #222230;
    --swatch-border: rgba(0, 0, 0, 0.15);
    --settings-header: #444460;
    --settings-label: #333348;
    --settings-val: #666680;
    --dialog-bg: #ffffff;
    --dialog-overlay: rgba(0, 0, 0, 0.45);
    --dialog-text: #1a1a2f;
    --dialog-h2: #000010;
    --dialog-subtitle: #555568;
    --dialog-section-title: #666680;
    --col-row-hover: #ebebf0;
    --col-row-selected: #dde8f5;
    --col-kind-numeric-bg: #d4f0d4;
    --col-kind-numeric-text: #1a6a1a;
    --col-kind-datetime-bg: #d4e4f0;
    --col-kind-datetime-text: #1a4a7a;
    --col-kind-text-bg: #f0e8d4;
    --col-kind-text-text: #7a4a1a;
    --btn-cancel-bg: #e0e0ea;
    --btn-cancel-text: #444460;
  }

  :global(body) {
    margin: 0;
    background: var(--bg);
    overflow: hidden;
    color: var(--text);
    font-family: sans-serif;
  }

  main {
    width: 100vw;
    height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    background: var(--toolbar-bg);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    height: 42px;
    box-sizing: border-box;
  }

  /* Workspace holds the graph stack + its overlay panels; relative so the
     panels' absolute positioning anchors to the plot area. */
  .workspace {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  /* The vertical stack of graphs — fills the workspace; each slot flexes equally. */
  .graph-stack {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  /* One graph + its remove control. flex:1 → equal heights across the stack.
     position:relative anchors the remove button to this slot. The inner Graph
     renders a flex column, so make this slot a flex column too. */
  .graph-slot {
    position: relative;
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .remove-graph-btn {
    position: absolute;
    top: 6px;
    right: 6px;
    z-index: 150;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    background: var(--panel-bg-alpha);
    color: var(--text-muted);
    border: 1px solid var(--btn-border);
    border-radius: 5px;
    cursor: pointer;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
  }

  .remove-graph-btn:hover {
    background: var(--btn-hover-bg);
    color: #ff6666;
    border-color: var(--border-mid);
  }

  .open-btn {
    padding: 5px 16px;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 5px;
    cursor: pointer;
    font-size: 0.85rem;
    font-weight: 600;
    transition: opacity 0.15s;
  }

  .open-btn:hover:not(:disabled) {
    opacity: 0.85;
  }

  .open-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .cursor-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 14px;
    background: var(--btn-bg);
    color: var(--text-dim);
    border: 1px solid var(--btn-border);
    border-radius: 5px;
    cursor: pointer;
    font-size: 0.85rem;
    font-weight: 600;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
  }

  .cursor-btn:hover {
    background: var(--btn-hover-bg);
    color: var(--btn-hover-text);
  }

  .cursor-btn.active {
    background: var(--btn-active-bg);
    color: var(--btn-active-text);
    border-color: var(--btn-active-border);
  }

  .draw-mode-btn {
    padding: 5px 14px;
    background: var(--btn-bg);
    color: var(--text-dim);
    border: 1px solid var(--btn-border);
    border-radius: 5px;
    cursor: pointer;
    font-size: 0.85rem;
    font-weight: 600;
    min-width: 60px;
    transition: background 0.15s, color 0.15s;
  }

  .draw-mode-btn:hover:not(:disabled) {
    background: var(--btn-hover-bg);
    color: var(--btn-hover-text);
  }

  .draw-mode-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .theme-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 5px 10px;
    background: var(--btn-bg);
    border: 1px solid var(--btn-border);
    border-radius: 5px;
    cursor: pointer;
    color: var(--text);
    transition: background 0.15s;
    margin-left: auto;
  }

  .theme-btn:hover {
    background: var(--btn-hover-bg);
  }

  .file-label {
    font-size: 0.8rem;
    color: var(--text-muted);
    max-width: 400px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .error-msg {
    font-size: 0.8rem;
    color: #ff6666;
    max-width: 600px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .recent-wrap {
    position: relative;
  }

  .recent-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    background: var(--recent-bg);
    border: 1px solid var(--btn-border);
    border-radius: 5px;
    min-width: 220px;
    max-width: 400px;
    z-index: 100;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
    overflow: hidden;
  }

  .recent-item {
    display: block;
    width: 100%;
    padding: 6px 12px;
    background: transparent;
    color: var(--text-dim);
    border: none;
    border-bottom: 1px solid var(--border);
    text-align: left;
    cursor: pointer;
    font-size: 0.82rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    transition: background 0.1s, color 0.1s;
  }

  .recent-item:last-child {
    border-bottom: none;
  }

  .recent-item:hover {
    background: var(--recent-item-hover);
    color: var(--btn-hover-text);
  }
</style>
