<script lang="ts">
  /**
   * Graph.svelte — a single self-contained plot surface.
   *
   * Owns its own `Renderer`, canvas, axis/cursor overlays, pan/zoom/cursor
   * interaction, view state, per-graph series info, draw mode, view mode
   * (plot/table), grid + appearance settings, and the table view for this graph.
   *
   * The workspace toolbar/panels (in App.svelte) drive the focused graph by
   * holding a `bind:this` ref to it, calling its action methods, and reading its
   * exposed accessor methods after each action. Data-flow is one-directional:
   * the Graph owns all state, App pulls via the accessors / `refresh()`.
   *
   * Events:
   *   - focusrequest        — pointerdown anywhere on the graph (ask App to focus it)
   *   - xrange {x_min,x_max} — after a view change (for cross-graph X-sync, Task 4)
   *   - datachanged         — after load/setSeries/transform (App refreshes panels)
   */
  import { onMount, tick, createEventDispatcher } from 'svelte';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { Renderer } from '../renderer.js';
  import type { FileMeta, SeriesSpec, AxisTicksData, ViewState, SeriesInfoEntry } from '../renderer.js';
  import TableView from './TableView.svelte';
  import DistView from './DistView.svelte';
  import Axes from '../overlay/Axes.svelte';
  import Cursors from '../overlay/Cursors.svelte';
  import type { CursorPoint } from '../overlay/Cursors.svelte';

  // ── Public props ────────────────────────────────────────────────────────────
  /** Draw a focus border when true. */
  export let focused = false;

  // ── Public renderer accessor ─────────────────────────────────────────────────
  /** This graph's renderer — App reaches it via `bind:this={graphRef}` then `graphRef.renderer.*`. */
  export const renderer = new Renderer();

  const dispatch = createEventDispatcher<{
    focusrequest: void;
    xrange: { x_min: number; x_max: number };
    datachanged: void;
    droppath: { path: string };
    ready: void;
    viewmode: void;
  }>();

  let canvas: HTMLCanvasElement;

  // ── Per-graph view + data state ──────────────────────────────────────────────
  let viewState: ViewState | null = null;
  let ticks: AxisTicksData | null = null;
  let seriesInfo: SeriesInfoEntry[] = [];
  let hasData = false;
  let dragHover = false;
  /** Most recent renderer init/load error (App reads via getError()), or null. */
  let initError: string | null = null;

  // ── Appearance / settings (per graph) ────────────────────────────────────────
  let showGrid = true;
  let lineWidth = 2.0;
  let pointRadius = 3.0;
  let normalized = false;
  let autoscaleMode = 'minmax';
  let yScale = 'linear';
  let downsampleMode = 'minmax';

  // ── View mode (plot / table / dist) ─────────────────────────────────────────
  let viewMode: 'plot' | 'table' | 'dist' = 'plot';
  let tableView: TableView;
  let distView: DistView;

  // ── Selected series (SeriesList row selection; Dist now shows all series as
  //    small multiples and no longer depends on this — kept for a future
  //    single-series view, e.g. Spectrogram) ────────────────────────────────
  let selectedSeriesIndex = 0;

  // ── Draw mode ────────────────────────────────────────────────────────────────
  type DrawMode = 'lines' | 'step' | 'points';
  const DRAW_MODES: DrawMode[] = ['lines', 'step', 'points'];
  let drawMode: DrawMode = 'lines';

  // ── Cursor mode ──────────────────────────────────────────────────────────────
  let cursorMode = false;
  let cursors: CursorPoint[] = [];

  // ── Pan state ────────────────────────────────────────────────────────────────
  let dragging = false;
  let lastPx = 0;
  let lastPy = 0;
  // Track pointer-down CSS position for click-vs-drag discrimination
  let pointerDownCssX = 0;
  let pointerDownCssY = 0;
  const CLICK_THRESHOLD_PX = 4;

  // ── View refresh ─────────────────────────────────────────────────────────────
  /** Pull view state from the renderer without emitting any events.
   *  Called by the exported `refresh()` (App-driven) to avoid the
   *  App.syncFromGraph → refresh → dispatch('xrange') → handleXRange recursion. */
  function pullViewState() {
    try {
      viewState = renderer.viewState();
      ticks = renderer.axisTicks();
    } catch (_) {
      // renderer not ready yet
    }
  }

  /** Pull view state then emit xrange — used only by genuine user-interaction
   *  handlers (pan, wheel-zoom, fit, dblclick, resize) so Task-4 sync still works. */
  function refreshView() {
    pullViewState();
    if (viewState) {
      dispatch('xrange', { x_min: viewState.x_min, x_max: viewState.x_max });
    }
  }

  function refreshSeriesInfo() {
    try {
      seriesInfo = renderer.seriesInfo();
    } catch (_) {
      seriesInfo = [];
    }
    // Keep the selection in range as series are added/removed/reordered.
    if (seriesInfo.length === 0) {
      selectedSeriesIndex = 0;
    } else if (selectedSeriesIndex > seriesInfo.length - 1) {
      selectedSeriesIndex = seriesInfo.length - 1;
    }
  }

  // ── CSS-pixel → canvas-backing-pixel scale factors ───────────────────────────
  function pixelScale(): { sx: number; sy: number } {
    const rect = canvas.getBoundingClientRect();
    return {
      sx: canvas.width / rect.width,
      sy: canvas.height / rect.height,
    };
  }

  function onPointerDown(e: PointerEvent) {
    // Ask App to focus this graph regardless of button.
    dispatch('focusrequest');
    if (e.button !== 0) return; // left button only — don't pan on right/middle click
    dragging = true;
    const { sx, sy } = pixelScale();
    const rect = canvas.getBoundingClientRect();
    // CSS px position (for click-vs-drag threshold)
    pointerDownCssX = e.clientX - rect.left;
    pointerDownCssY = e.clientY - rect.top;
    // Backing-pixel position for pan
    lastPx = pointerDownCssX * sx;
    lastPy = pointerDownCssY * sy;
    canvas.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const { sx, sy } = pixelScale();
    const rect = canvas.getBoundingClientRect();
    const curX = (e.clientX - rect.left) * sx;
    const curY = (e.clientY - rect.top) * sy;
    const dx = curX - lastPx;
    const dy = curY - lastPy;
    lastPx = curX;
    lastPy = curY;
    renderer.pan(dx, dy);
    refreshView();
  }

  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;

    // Click-vs-drag: if movement in CSS px is below threshold AND cursorMode is on,
    // treat as a cursor placement click.
    const rect = canvas.getBoundingClientRect();
    const upCssX = e.clientX - rect.left;
    const upCssY = e.clientY - rect.top;
    const moveDist = Math.sqrt(
      (upCssX - pointerDownCssX) ** 2 + (upCssY - pointerDownCssY) ** 2
    );

    if (cursorMode && moveDist < CLICK_THRESHOLD_PX && viewState) {
      // Convert CSS px → data coordinates using pointer-DOWN position
      const dataX = viewState.x_min + (pointerDownCssX / rect.width) * (viewState.x_max - viewState.x_min);
      const dataY = viewState.y_min + (1 - pointerDownCssY / rect.height) * (viewState.y_max - viewState.y_min);

      if (cursors.length >= 2) {
        // Cycle: reset to a single new cursor
        cursors = [{ x: dataX, y: dataY }];
      } else {
        cursors = [...cursors, { x: dataX, y: dataY }];
      }
    }
  }

  function onPointerCancel(_e: PointerEvent) {
    dragging = false;
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const { sx, sy } = pixelScale();
    const rect = canvas.getBoundingClientRect();
    const ax = (e.clientX - rect.left) * sx;
    const ay = (e.clientY - rect.top) * sy;
    // Browser deltaY is negative when scrolling up (zoom in).
    // Core zoom uses: factor = (1 - scroll_y * 0.001); positive scroll_y → zoom in.
    renderer.zoom(-e.deltaY, ax, ay);
    refreshView();
  }

  function onDblClick(_e: MouseEvent) {
    renderer.autoFit();
    refreshView();
  }

  // ── Lifecycle ────────────────────────────────────────────────────────────────
  onMount(async () => {
    try {
      await renderer.init();
      await renderer.create(canvas);
      refreshView();
      // Renderer surface is live — let App apply the persisted-theme background.
      dispatch('ready');
    } catch (e) {
      // Surface init failures to App via the error path it already shows.
      initError = String(e);
    }

    // Keep the GPU surface in sync with the canvas element size.
    const ro = new ResizeObserver(entries => {
      for (const entry of entries) {
        const { width, height } = entry.contentRect;
        if (width > 0 && height > 0) {
          canvas.width = Math.round(width);
          canvas.height = Math.round(height);
          renderer.resize(canvas.width, canvas.height);
          renderer.render();
          refreshView();
        }
      }
    });
    ro.observe(canvas);

    // Register Tauri drag-drop listener (OS drops give file paths; HTML5 ondrop does not).
    // A file dropped on this graph loads into THIS graph.
    const unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === 'over') {
        dragHover = true;
      } else if (p.type === 'leave') {
        dragHover = false;
      } else if (p.type === 'drop') {
        dragHover = false;
        const path = (p as { type: string; paths?: string[] }).paths?.[0];
        if (path) {
          // App owns the open flow (recent files, prefs, ColumnDialog); a file
          // dropped on this graph focuses it and asks App to load into it.
          dispatch('focusrequest');
          dispatch('droppath', { path });
        }
      }
    });

    return () => {
      ro.disconnect();
      unlistenDrop();
    };
  });

  // ── Exposed: errors from init/load surfaced to App ───────────────────────────
  /** Return the most recent renderer init/load error, or null. */
  export function getError(): string | null {
    return initError;
  }

  // ── Exposed: load a file into THIS graph ─────────────────────────────────────
  // App's open flow reads bytes from disk, then hands them here. The returned
  // FileMeta drives App's ColumnDialog. App calls setSeries() after the user
  // confirms columns.
  /**
   * Parse `bytes` into this graph's renderer and return column metadata for the
   * column-selection dialog. Throws on parse failure.
   */
  export function loadBytes(bytes: Uint8Array, filename: string): FileMeta {
    initError = null;
    const meta = renderer.loadFileBytes(bytes, filename);
    return meta;
  }

  /**
   * Build GPU series from `specs`, reset draw mode to default, refresh state,
   * and (if in table mode) refresh the table. Throws on failure.
   */
  export function setSeries(specs: SeriesSpec[]): void {
    renderer.setSeries(specs);
    hasData = true;
    drawMode = 'lines'; // reset to default on new data load
    selectedSeriesIndex = 0; // fresh data — select the first series
    refreshView();
    refreshSeriesInfo();
    if (viewMode === 'table') {
      tick().then(() => { if (tableView) tableView.refresh(); });
    } else if (viewMode === 'dist') {
      tick().then(() => { if (distView) distView.refresh(); });
    }
    dispatch('datachanged');
  }

  // ── Exposed: toolbar actions targeting this graph ────────────────────────────
  /** Re-fit the view to all data (Fit button / double-click). */
  export function fit(): void {
    renderer.autoFit();
    refreshView();
  }

  /** Remove all series from this graph (Clear button) → returns to empty state. */
  export function clear(): void {
    renderer.clearSeries();
    hasData = false;
    cursors = [];
    refreshSeriesInfo();
    pullViewState();
  }

  /** Cycle draw mode: lines → step → points. No-op without data. */
  export function cycleDrawMode(): void {
    if (!hasData) return;
    const idx = DRAW_MODES.indexOf(drawMode);
    drawMode = DRAW_MODES[(idx + 1) % DRAW_MODES.length];
    renderer.setDrawMode(drawMode);
    refreshView();
  }

  /** Toggle cursor-placement mode; clears cursors when turned off. */
  export function toggleCursorMode(): void {
    cursorMode = !cursorMode;
    if (!cursorMode) cursors = [];
  }

  /** Switch to `mode` (plot/table/dist); mounts + refreshes the target view on switch. */
  export async function setViewMode(mode: 'plot' | 'table' | 'dist'): Promise<void> {
    viewMode = mode;
    await tick();
    if (mode === 'table') {
      tableView?.refresh();
    } else if (mode === 'dist') {
      distView?.refresh();
    }
    dispatch('viewmode');
  }

  /** Set line width (Settings panel). */
  export function setLineWidth(value: number): void {
    lineWidth = value;
    try { renderer.setLineWidth(lineWidth); } catch (_) {}
    refreshView();
  }

  /** Set point radius (Settings panel). */
  export function setPointRadius(value: number): void {
    pointRadius = value;
    try { renderer.setPointRadius(pointRadius); } catch (_) {}
    refreshView();
  }

  /** Set grid visibility (Settings panel). */
  export function setShowGrid(value: boolean): void {
    showGrid = value;
  }

  /** Set normalized multi-unit overlay mode (Settings panel). */
  export function setNormalized(value: boolean): void {
    normalized = value;
    try { renderer.setNormalized(normalized); } catch (_) {}
    refreshView();
  }

  /** Set autoscale mode used when auto-fitting the view (Settings panel). */
  export function setAutoscaleMode(v: string): void {
    autoscaleMode = v;
    try { renderer.setAutoscaleMode(v); } catch (_) {}
    refreshView();
  }

  /** Set the Y-axis scale (Settings panel). */
  export function setYScale(v: string): void {
    yScale = v;
    try { renderer.setYScale(v); } catch (_) {}
    refreshView();
  }

  /** Set the downsampling mode used when rendering large series (Settings panel). */
  export function setDownsampleMode(v: string): void {
    downsampleMode = v;
    try { renderer.setDownsampleMode(v); } catch (_) {}
    refreshView();
  }

  /** Apply a WebGPU background color (theme) and re-render. */
  export function setBackground(r: number, g: number, b: number, a: number, renderNow = true): void {
    try {
      renderer.setBackground(r, g, b, a);
      if (renderNow) renderer.render();
    } catch (_) {
      // renderer may not be ready yet — that's fine
    }
  }

  // ── Exposed: export / clipboard (per-graph; use this graph's canvas) ──────────
  /** Export this graph's series as CSV text. Returns '' if no data. */
  export function exportCsv(): string {
    return renderer.exportCsv();
  }

  /** Render now and capture the canvas as a PNG Blob (null if capture fails). */
  export async function capturePng(): Promise<Blob | null> {
    renderer.render();
    return await new Promise<Blob | null>((resolve) => {
      canvas.toBlob((b) => resolve(b), 'image/png');
    });
  }

  // ── Exposed: read-only accessors for App's panels ─────────────────────────────
  /** Re-pull all of this graph's panel-facing state from the renderer.
   *  Pure pull — does NOT emit xrange, so App.syncFromGraph() calling this
   *  never triggers the handleXRange → syncFromGraph recursion. */
  export function refresh(): void {
    refreshSeriesInfo();
    pullViewState();
  }

  /**
   * Apply an externally-supplied X-range to this graph without emitting an
   * `xrange` event (loop-safe).
   *
   * Calls `renderer.setXRange` (which runs LTTB + render on the wasm side),
   * then `pullViewState()` (the non-emitting state pull) so this graph's
   * viewState/ticks/axes overlay update to reflect the new range.
   *
   * Safety guarantee: `pullViewState` NEVER dispatches `xrange`, so calling
   * `applyXRange` on graph B from graph A's `xrange` handler does NOT cause
   * graph B to re-emit, preventing infinite propagation.
   */
  export function applyXRange(xMin: number, xMax: number): void {
    try {
      renderer.setXRange(xMin, xMax);
    } catch (_) {
      // renderer not ready yet — no-op
      return;
    }
    pullViewState();
  }

  export function getSeriesInfo(): SeriesInfoEntry[] { return seriesInfo; }
  export function getViewState(): ViewState | null { return viewState; }
  export function getDrawMode(): DrawMode { return drawMode; }
  export function getViewMode(): 'plot' | 'table' | 'dist' { return viewMode; }
  export function getShowGrid(): boolean { return showGrid; }
  export function getCursorMode(): boolean { return cursorMode; }
  export function getHasData(): boolean { return hasData; }
  export function getLineWidth(): number { return lineWidth; }
  export function getPointRadius(): number { return pointRadius; }
  export function getNormalized(): boolean { return normalized; }
  export function getAutoscaleMode(): string { return autoscaleMode; }
  export function getYScale(): string { return yScale; }
  export function getDownsampleMode(): string { return downsampleMode; }
  export function getSelectedSeriesIndex(): number { return selectedSeriesIndex; }
  export function setSelectedSeriesIndex(i: number): void {
    selectedSeriesIndex = i;
    if (viewMode === 'dist') distView?.refresh();
  }
</script>

<!-- Per-graph view tabs — switch this graph between Plot/Table/Dist. -->
{#if hasData}
  <div class="view-tabs">
    <button
      class="view-tab"
      class:active={viewMode === 'plot'}
      on:click={() => setViewMode('plot')}
      title="Plot view"
    >Plot</button>
    <button
      class="view-tab"
      class:active={viewMode === 'table'}
      on:click={() => setViewMode('table')}
      title="Table view"
    >Table</button>
    <button
      class="view-tab"
      class:active={viewMode === 'dist'}
      on:click={() => setViewMode('dist')}
      title="Distribution view"
    >Dist</button>
  </div>
{/if}

<!-- Table / Dist view — rendered alongside (not replacing) the canvas -->
{#if hasData && viewMode === 'table'}
  <TableView bind:this={tableView} {renderer} />
{:else if hasData && viewMode === 'dist'}
  <DistView bind:this={distView} {renderer} />
{/if}

<!-- Plot canvas + axis overlay — fills the remaining space; hidden (not unmounted) outside plot mode -->
<div class="canvas-wrap" class:hidden={viewMode !== 'plot'} class:focused>
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <canvas
    bind:this={canvas}
    style={cursorMode ? 'cursor:crosshair' : ''}
    on:pointerdown={onPointerDown}
    on:pointermove={onPointerMove}
    on:pointerup={onPointerUp}
    on:pointercancel={onPointerCancel}
    on:wheel={onWheel}
    on:dblclick={onDblClick}
  ></canvas>
  <Axes
    {ticks}
    {viewState}
    displayW={canvas ? canvas.getBoundingClientRect().width : 0}
    displayH={canvas ? canvas.getBoundingClientRect().height : 0}
    {showGrid}
  />
  <Cursors
    {cursors}
    {viewState}
    displayW={canvas ? canvas.getBoundingClientRect().width : 0}
    displayH={canvas ? canvas.getBoundingClientRect().height : 0}
  />
  {#if !hasData}
    <div class="empty-state" aria-hidden="true">
      <svg class="empty-mark" width="60" height="60" viewBox="0 0 24 24" fill="none">
        <rect x="1.5" y="1.5" width="21" height="21" rx="5.5" stroke="var(--border-mid)" stroke-width="1.1"/>
        <path d="M4 16 L8.5 16 L11.5 7 L14.5 18.5 L20 11" stroke="var(--accent)" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" opacity="0.75"/>
      </svg>
      <div class="empty-title">No data loaded</div>
      <div class="empty-hint">Open a CSV or Excel file — or drop one here</div>
    </div>
  {/if}
  {#if dragHover}
    <div class="drop-overlay" aria-hidden="true">
      <span class="drop-label">Drop a CSV / Excel file to open</span>
    </div>
  {/if}
</div>

<style>
  /* ── Per-graph view tab strip (Plot / Table / Dist) ── */
  .view-tabs {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 3px;
    padding: 4px 6px;
    background: var(--btn-bg);
    border-bottom: 1px solid var(--border);
  }

  .view-tab {
    display: inline-flex;
    align-items: center;
    padding: 3px 10px;
    background: transparent;
    color: var(--text-dim);
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-family: var(--font-ui);
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    transition: background 0.13s ease, color 0.13s ease, border-color 0.13s ease;
  }

  .view-tab:hover {
    background: var(--btn-active-bg);
    color: var(--text);
  }

  .view-tab.active {
    background: var(--btn-active-bg);
    color: var(--accent);
    border-color: var(--btn-active-border);
  }

  .canvas-wrap {
    position: relative;
    flex: 1;
    overflow: hidden;
    min-height: 0;
  }

  /* Hide the canvas wrap without unmounting it (preserves the wgpu surface). */
  .canvas-wrap.hidden {
    display: none;
  }

  /* Focus border (drawn when this graph is the focused one). */
  .canvas-wrap.focused {
    outline: 2px solid var(--btn-active-border);
    outline-offset: -2px;
  }

  canvas {
    display: block;
    width: 100%;
    height: 100%;
    cursor: grab;
  }

  canvas:active {
    cursor: grabbing;
  }

  /* ── Empty state (no data loaded) ── */
  .empty-state {
    position: absolute;
    inset: 0;
    pointer-events: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    z-index: 5;
    user-select: none;
  }
  .empty-mark {
    margin-bottom: 14px;
    filter: drop-shadow(0 0 12px var(--accent-dim));
    opacity: 0.9;
  }
  .empty-title {
    font-family: var(--font-display);
    font-size: 1.05rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
  }
  .empty-hint {
    font-family: var(--font-ui);
    font-size: 0.78rem;
    color: var(--text-muted);
    letter-spacing: 0.02em;
  }

  /* ── Drag-drop hover overlay ── */
  .drop-overlay {
    position: absolute;
    inset: 10px;
    pointer-events: none;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1.5px dashed var(--accent);
    border-radius: var(--radius);
    background: var(--accent-bg);
    z-index: 200;
  }

  .drop-label {
    padding: 10px 22px;
    font-family: var(--font-ui);
    font-size: 0.82rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--accent);
    background: var(--panel-bg-alpha);
    border: 1px solid var(--accent-dim);
    border-radius: var(--radius);
  }
</style>
