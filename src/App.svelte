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
  let viewMode: 'plot' | 'table' | 'dist' = 'plot';
  let cursorMode = false;
  // Appearance (mirrors of the focused graph's settings; seeded with the
  // graph's defaults so the Settings panel shows correct initial values).
  let showGrid = true;
  let lineWidth = 2.0;
  let pointRadius = 3.0;
  let normalized = false;
  let autoscaleMode = 'minmax';
  let yScale = 'linear';
  let downsampleMode = 'minmax';
  /** Index of the currently-selected series row (drives the Distribution view). */
  let selectedSeriesIndex = 0;

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
    autoscaleMode = g.getAutoscaleMode();
    yScale = g.getYScale();
    downsampleMode = g.getDownsampleMode();
    selectedSeriesIndex = g.getSelectedSeriesIndex();
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

  /** Remove all series from the focused graph (Clear button). */
  function handleClear() {
    graphRefs[focusedId]?.clear();
    syncFromGraph();
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
    dark:  [0.055, 0.059, 0.075, 1.0], // matches --bg #0e0f13 (graphite)
    light: [0.957, 0.957, 0.945, 1.0], // matches --bg #f4f4f1 (warm paper)
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

  function handleAutoscaleMode(event: CustomEvent<{ value: string }>) {
    focusedGraph?.setAutoscaleMode(event.detail.value);
    syncFromGraph();
  }

  function handleYScale(event: CustomEvent<{ value: string }>) {
    focusedGraph?.setYScale(event.detail.value);
    syncFromGraph();
  }

  function handleDownsampleMode(event: CustomEvent<{ value: string }>) {
    focusedGraph?.setDownsampleMode(event.detail.value);
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
  <!-- Instrument header bar -->
  <header class="toolbar">
    <!-- Wordmark -->
    <div class="brand" title="OxidePlot">
      <svg class="brand-mark" width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <rect x="1.5" y="1.5" width="21" height="21" rx="5.5" stroke="var(--border-mid)" stroke-width="1.5"/>
        <path d="M4 16 L8.5 16 L11.5 7 L14.5 18.5 L20 11" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      <span class="brand-name">Oxide<span class="brand-accent">Plot</span></span>
    </div>

    <div class="tsep"></div>

    <!-- File -->
    <div class="tgroup">
      <button class="tbtn primary" on:click={handleOpen} disabled={loading}>
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
        {loading ? 'Loading…' : 'Open'}
      </button>
      {#if prefs.recentFiles.length > 0}
        <div class="recent-wrap">
          <button class="tbtn" on:click={() => (showRecent = !showRecent)} title="Recent files">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><polyline points="12 7 12 12 16 14"/></svg>
            Recent
            <svg class="caret" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="6 9 12 15 18 9"/></svg>
          </button>
          {#if showRecent}
            <!-- svelte-ignore a11y-no-static-element-interactions -->
            <div class="recent-dropdown" on:mouseleave={() => (showRecent = false)}>
              {#each prefs.recentFiles as rpath}
                <button class="recent-item" title={rpath} on:click={() => handleOpenRecent(rpath)}>
                  {rpath.split(/[\\/]/).pop() ?? rpath}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
      {#if canUseLoadedData}
        <button class="tbtn" on:click={handleUseLoadedData} title="Load the cached dataset into this graph so you can pick its own series">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
          Use Data
        </button>
      {/if}
    </div>

    <div class="tsep"></div>

    <!-- Graphs -->
    <div class="tgroup">
      <button class="tbtn" on:click={addGraph} title="Add a new graph below">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
        Add Graph
      </button>
      <button class="tbtn" disabled={!hasData} on:click={handleClear} title="Remove all series from the focused graph">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/></svg>
        Clear
      </button>
    </div>

    <div class="tsep"></div>

    <!-- View -->
    <div class="tgroup">
      <button class="tbtn" disabled={!hasData} on:click={handleFit} title="Re-fit view to all data (same as double-click)">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 3h6v6"/><path d="M9 21H3v-6"/><path d="M21 3l-7 7"/><path d="M3 21l7-7"/></svg>
        Fit
      </button>
      <button class="tbtn" class:active={syncX} on:click={toggleSyncX} title={syncX ? 'Sync X ON — all graphs share the same X-range (click to disable)' : 'Sync X OFF — pan/zoom one graph to sync all others'}>
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
        Sync X
      </button>
      <button class="tbtn" class:active={cursorMode} on:click={toggleCursorMode} title={cursorMode ? 'Cursor mode ON — click to place cursors (toggle off to clear)' : 'Cursor mode OFF'}>
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><line x1="12" y1="2" x2="12" y2="6"/><line x1="12" y1="18" x2="12" y2="22"/><line x1="2" y1="12" x2="6" y2="12"/><line x1="18" y1="12" x2="22" y2="12"/></svg>
        Cursors
      </button>
      <button class="tbtn drawmode" disabled={!hasData} on:click={cycleDrawMode} title="Cycle draw mode: Lines → Step → Points">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="3 12 7 12 10 5 14 19 17 12 21 12"/></svg>
        {DRAW_MODE_LABELS[drawMode]}
      </button>
    </div>

    <div class="tsep"></div>

    <!-- Output -->
    <div class="tgroup">
      <button class="tbtn" class:active={showSettings} on:click={toggleSettings} title="Toggle settings panel">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="4" y1="21" x2="4" y2="14"/><line x1="4" y1="10" x2="4" y2="3"/><line x1="12" y1="21" x2="12" y2="12"/><line x1="12" y1="8" x2="12" y2="3"/><line x1="20" y1="21" x2="20" y2="16"/><line x1="20" y1="12" x2="20" y2="3"/><line x1="2" y1="14" x2="6" y2="14"/><line x1="10" y1="8" x2="14" y2="8"/><line x1="18" y1="16" x2="22" y2="16"/></svg>
        Settings
      </button>
      <button class="tbtn" disabled={!hasData} on:click={handleExportCsv} title="Export all series to CSV">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="8" y1="13" x2="16" y2="13"/><line x1="8" y1="17" x2="13" y2="17"/></svg>
        CSV
      </button>
      <button class="tbtn" disabled={!hasData} on:click={handleExportPng} title="Save plot as PNG (note: WebGPU canvas — verify image is not blank)">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>
        PNG
      </button>
      <button class="tbtn" disabled={!hasData} on:click={handleCopy} title="Copy plot PNG to clipboard">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
        Copy
      </button>
    </div>

    {#if filePath && !fileMeta}
      <span class="file-label" title={filePath}>{filePath.split(/[\\/]/).pop()}</span>
    {/if}
    {#if error}
      <span class="error-msg" title={error}>{error}</span>
    {/if}

    <div class="tspacer"></div>

    <!-- Theme -->
    <button class="tbtn icon-only theme" on:click={toggleTheme} title={prefs.theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'} aria-label={prefs.theme === 'dark' ? 'Light mode' : 'Dark mode'}>
      {#if prefs.theme === 'dark'}
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
      {:else}
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
      {/if}
    </button>
  </header>

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
            on:viewmode={() => { setFocus(g.id); syncFromGraph(); }}
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
    {#if viewMode !== 'table' && focusedGraph}
      {#if seriesInfo.length > 0}
        <SeriesList
          series={seriesInfo}
          renderer={focusedGraph.renderer}
          selectedIndex={selectedSeriesIndex}
          on:change={handleSeriesChange}
          on:select={(e) => { focusedGraph?.setSelectedSeriesIndex(e.detail); syncFromGraph(); }}
        />
      {/if}
      {#if viewMode === 'plot' && showSettings}
        <Settings
          {lineWidth}
          {pointRadius}
          {showGrid}
          {normalized}
          {autoscaleMode}
          {yScale}
          {downsampleMode}
          on:linewidth={handleLineWidth}
          on:pointradius={handlePointRadius}
          on:showgrid={handleShowGrid}
          on:normalized={handleNormalized}
          on:autoscalemode={handleAutoscaleMode}
          on:yscale={handleYScale}
          on:downsamplemode={handleDownsampleMode}
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
    --bg: #0e0f13;
    --panel-bg: #15171c;
    --panel-bg-alpha: rgba(17, 19, 24, 0.86);
    --toolbar-bg: #0a0b0e;
    --toolbar-bg-2: #14161c;
    --text: #e6e8ec;
    --text-muted: #696f7a;
    --text-dim: #aab0b9;
    --border: #1f222a;
    --border-mid: #2c303a;
    --btn-bg: #181b21;
    --btn-border: #272b34;
    --btn-hover-bg: #21252e;
    --btn-hover-text: #f3f5f8;
    --btn-active-bg: rgba(255, 106, 43, 0.14);
    --btn-active-text: #ff8f54;
    --btn-active-border: rgba(255, 106, 43, 0.5);
    --accent: #ff6a2b;
    --accent-bright: #ff8f54;
    --accent-dim: rgba(255, 106, 43, 0.45);
    --accent-bg: rgba(255, 106, 43, 0.12);
    --radius: 7px;
    --radius-sm: 5px;
    --shadow-panel: 0 10px 30px rgba(0, 0, 0, 0.55), inset 0 1px 0 rgba(255, 255, 255, 0.04);
    --recent-bg: #14161c;
    --recent-item-hover: #21252e;
    --axis-line-major: rgba(225, 228, 234, 0.85);
    --axis-line-minor: rgba(170, 176, 188, 0.45);
    --axis-text: rgba(205, 210, 220, 0.92);
    --axis-text-stroke: rgba(10, 11, 14, 0.75);
    --grid-line: rgba(255, 255, 255, 0.05);
    --cursor-dot-stroke: rgba(10, 11, 14, 0.75);
    --cursor-readout-bg: rgba(12, 13, 17, 0.85);
    --cursor-readout-border: rgba(255, 106, 43, 0.3);
    --cursor-readout-text: #d4d8df;
    --cursor-readout-vals: #c2c7d0;
    --cursor-divider: rgba(170, 176, 188, 0.18);
    --cursor-delta-label: #9aa0ab;
    --cursor-delta-vals: #ffb083;
    --series-row-hover: rgba(255, 255, 255, 0.04);
    --series-ctrl-btn: #7a808b;
    --series-ctrl-hover-bg: rgba(255, 106, 43, 0.16);
    --series-name-text: #d4d8df;
    --swatch-border: rgba(255, 255, 255, 0.18);
    --settings-header: #ff8f54;
    --settings-label: #aab0b9;
    --settings-val: #7a808b;
    --dialog-bg: #15171c;
    --dialog-overlay: rgba(6, 7, 9, 0.72);
    --dialog-text: #e6e8ec;
    --dialog-h2: #ffffff;
    --dialog-subtitle: #767c87;
    --dialog-section-title: #ff8f54;
    --col-row-hover: #1f232b;
    --col-row-selected: rgba(255, 106, 43, 0.12);
    --col-kind-numeric-bg: rgba(96, 221, 96, 0.12);
    --col-kind-numeric-text: #74d674;
    --col-kind-datetime-bg: rgba(96, 170, 221, 0.12);
    --col-kind-datetime-text: #6cb6e6;
    --col-kind-text-bg: rgba(255, 106, 43, 0.14);
    --col-kind-text-text: #ff9a5e;
    --btn-cancel-bg: #21252e;
    --btn-cancel-text: #aab0b9;
  }

  /* ── CSS custom properties — light theme ── */
  :global(:root[data-theme="light"]) {
    --bg: #f4f4f1;
    --panel-bg: #ffffff;
    --panel-bg-alpha: rgba(255, 255, 255, 0.92);
    --toolbar-bg: #eceae4;
    --toolbar-bg-2: #f4f4f1;
    --text: #1b1d22;
    --text-muted: #8a8f98;
    --text-dim: #44484f;
    --border: #dddbd3;
    --border-mid: #c6c4bc;
    --btn-bg: #ffffff;
    --btn-border: #d3d1c9;
    --btn-hover-bg: #f0efe9;
    --btn-hover-text: #14161a;
    --btn-active-bg: rgba(214, 78, 22, 0.12);
    --btn-active-text: #c2470f;
    --btn-active-border: rgba(214, 78, 22, 0.5);
    --accent: #e25416;
    --accent-bright: #c2470f;
    --accent-dim: rgba(226, 84, 22, 0.4);
    --accent-bg: rgba(226, 84, 22, 0.1);
    --radius: 7px;
    --radius-sm: 5px;
    --shadow-panel: 0 10px 30px rgba(40, 30, 20, 0.16), inset 0 1px 0 rgba(255, 255, 255, 0.6);
    --recent-bg: #ffffff;
    --recent-item-hover: #f0efe9;
    --axis-line-major: rgba(40, 42, 50, 0.8);
    --axis-line-minor: rgba(60, 62, 70, 0.4);
    --axis-text: rgba(25, 27, 34, 0.9);
    --axis-text-stroke: rgba(244, 244, 241, 0.85);
    --grid-line: rgba(0, 0, 0, 0.06);
    --cursor-dot-stroke: rgba(244, 244, 241, 0.85);
    --cursor-readout-bg: rgba(255, 255, 255, 0.92);
    --cursor-readout-border: rgba(226, 84, 22, 0.35);
    --cursor-readout-text: #22242c;
    --cursor-readout-vals: #33363f;
    --cursor-divider: rgba(80, 82, 90, 0.18);
    --cursor-delta-label: #5a5e66;
    --cursor-delta-vals: #b8430d;
    --series-row-hover: rgba(0, 0, 0, 0.04);
    --series-ctrl-btn: #6a6e76;
    --series-ctrl-hover-bg: rgba(226, 84, 22, 0.12);
    --series-name-text: #22242c;
    --swatch-border: rgba(0, 0, 0, 0.15);
    --settings-header: #c2470f;
    --settings-label: #33363f;
    --settings-val: #6a6e76;
    --dialog-bg: #ffffff;
    --dialog-overlay: rgba(30, 25, 20, 0.42);
    --dialog-text: #1b1d22;
    --dialog-h2: #0a0b0d;
    --dialog-subtitle: #6a6e76;
    --dialog-section-title: #c2470f;
    --col-row-hover: #f0efe9;
    --col-row-selected: rgba(226, 84, 22, 0.1);
    --col-kind-numeric-bg: rgba(26, 106, 26, 0.12);
    --col-kind-numeric-text: #1a6a1a;
    --col-kind-datetime-bg: rgba(26, 74, 122, 0.12);
    --col-kind-datetime-text: #1a4a7a;
    --col-kind-text-bg: rgba(226, 84, 22, 0.14);
    --col-kind-text-text: #b8430d;
    --btn-cancel-bg: #eceae4;
    --btn-cancel-text: #44484f;
  }

  :global(body) {
    margin: 0;
    background: var(--bg);
    overflow: hidden;
    color: var(--text);
    font-family: var(--font-ui);
  }

  main {
    width: 100vw;
    height: 100vh;
    display: flex;
    flex-direction: column;
  }

  /* ── Instrument header bar ── */
  .toolbar {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 12px;
    height: 50px;
    flex-shrink: 0;
    box-sizing: border-box;
    background: linear-gradient(180deg, var(--toolbar-bg-2), var(--toolbar-bg));
    border-bottom: 1px solid var(--border);
    box-shadow: 0 1px 0 rgba(0, 0, 0, 0.25);
  }

  /* Wordmark */
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    padding-right: 2px;
    user-select: none;
  }
  .brand-mark {
    display: block;
    filter: drop-shadow(0 0 7px var(--accent-dim));
  }
  .brand-name {
    font-family: var(--font-display);
    font-weight: 800;
    font-size: 1.02rem;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--text);
    line-height: 1;
  }
  .brand-accent {
    color: var(--accent);
  }

  /* Button groups + separators */
  .tgroup {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .tsep {
    width: 1px;
    height: 22px;
    background: var(--border);
    margin: 0 5px;
    flex-shrink: 0;
  }
  .tspacer {
    flex: 1 1 auto;
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

  /* The vertical stack of graphs — fills the workspace; each slot flexes equally
     down to its min-height, then the stack scrolls once slots hit that floor. */
  .graph-stack {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  /* One graph + its remove control. flex:1 0 260px → equal heights across the
     stack while at least one graph fits, but never squishes below 260px —
     once more graphs are stacked than fit, the stack scrolls instead.
     position:relative anchors the remove button to this slot. The inner Graph
     renders a flex column, so make this slot a flex column too. */
  .graph-slot {
    position: relative;
    flex: 1 0 260px;
    min-height: 260px;
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

  /* ── Tool button ── */
  .tbtn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    height: 30px;
    background: transparent;
    color: var(--text-dim);
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-family: var(--font-ui);
    font-size: 0.71rem;
    font-weight: 500;
    letter-spacing: 0.045em;
    text-transform: uppercase;
    white-space: nowrap;
    transition: background 0.13s ease, color 0.13s ease, border-color 0.13s ease;
  }
  .tbtn svg {
    flex-shrink: 0;
    opacity: 0.9;
  }
  .tbtn .caret {
    opacity: 0.55;
    margin-left: -3px;
  }

  .tbtn:hover:not(:disabled) {
    background: var(--btn-hover-bg);
    color: var(--btn-hover-text);
    border-color: var(--btn-border);
  }
  .tbtn:active:not(:disabled) {
    transform: translateY(0.5px);
  }
  .tbtn:disabled {
    opacity: 0.32;
    cursor: not-allowed;
  }

  /* Active (toggled-on) — amber */
  .tbtn.active {
    background: var(--btn-active-bg);
    color: var(--btn-active-text);
    border-color: var(--btn-active-border);
  }
  .tbtn.active svg {
    opacity: 1;
  }

  /* Primary (Open) — amber-filled */
  .tbtn.primary {
    background: var(--accent);
    color: #160f08;
    border-color: transparent;
    font-weight: 700;
    box-shadow: 0 1px 8px var(--accent-dim);
  }
  .tbtn.primary svg {
    opacity: 1;
  }
  .tbtn.primary:hover:not(:disabled) {
    background: var(--accent-bright);
    color: #160f08;
    border-color: transparent;
  }

  .tbtn.drawmode {
    min-width: 92px;
  }

  .tbtn.icon-only {
    padding: 0;
    width: 30px;
    justify-content: center;
  }
  .tbtn.theme:hover {
    color: var(--accent);
  }

  /* Status text */
  .file-label {
    font-family: var(--font-data);
    font-size: 0.72rem;
    color: var(--text-muted);
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding-left: 4px;
  }

  .error-msg {
    font-family: var(--font-data);
    font-size: 0.72rem;
    color: var(--accent);
    max-width: 420px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 4px 9px;
    border: 1px solid var(--accent-dim);
    border-radius: var(--radius-sm);
    background: var(--accent-bg);
  }

  /* Recent dropdown */
  .recent-wrap {
    position: relative;
  }

  .recent-dropdown {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    background: var(--recent-bg);
    border: 1px solid var(--border-mid);
    border-radius: var(--radius);
    min-width: 240px;
    max-width: 420px;
    z-index: 100;
    box-shadow: var(--shadow-panel);
    overflow: hidden;
    padding: 4px;
  }

  .recent-item {
    display: block;
    width: 100%;
    padding: 7px 10px;
    background: transparent;
    color: var(--text-dim);
    border: none;
    border-radius: var(--radius-sm);
    text-align: left;
    cursor: pointer;
    font-family: var(--font-data);
    font-size: 0.76rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    transition: background 0.1s, color 0.1s;
  }

  .recent-item:hover {
    background: var(--recent-item-hover);
    color: var(--btn-hover-text);
  }
</style>
