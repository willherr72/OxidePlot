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

  // ── View mode (plot / table) ────────────────────────────────────────────────
  let viewMode: 'plot' | 'table' = 'plot';
  let tableView: TableView;

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
    refreshView();
    refreshSeriesInfo();
    if (viewMode === 'table') {
      tick().then(() => { if (tableView) tableView.refresh(); });
    }
    dispatch('datachanged');
  }

  // ── Exposed: toolbar actions targeting this graph ────────────────────────────
  /** Re-fit the view to all data (Fit button / double-click). */
  export function fit(): void {
    renderer.autoFit();
    refreshView();
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

  /** Toggle plot ↔ table view; mounts + refreshes the table on switch. */
  export async function toggleViewMode(): Promise<void> {
    viewMode = viewMode === 'plot' ? 'table' : 'plot';
    if (viewMode === 'table') {
      await tick();
      if (tableView) tableView.refresh();
    }
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

  export function getSeriesInfo(): SeriesInfoEntry[] { return seriesInfo; }
  export function getViewState(): ViewState | null { return viewState; }
  export function getDrawMode(): DrawMode { return drawMode; }
  export function getViewMode(): 'plot' | 'table' { return viewMode; }
  export function getShowGrid(): boolean { return showGrid; }
  export function getCursorMode(): boolean { return cursorMode; }
  export function getHasData(): boolean { return hasData; }
  export function getLineWidth(): number { return lineWidth; }
  export function getPointRadius(): number { return pointRadius; }
  export function getNormalized(): boolean { return normalized; }
</script>

<!-- Table view — rendered alongside (not replacing) the canvas -->
{#if hasData && viewMode === 'table'}
  <TableView bind:this={tableView} {renderer} />
{/if}

<!-- Plot canvas + axis overlay — fills the remaining space; hidden (not unmounted) in table mode -->
<div class="canvas-wrap" class:hidden={viewMode === 'table'} class:focused>
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
  {#if dragHover}
    <div class="drop-overlay" aria-hidden="true">
      <span class="drop-label">Drop a CSV / Excel file to open</span>
    </div>
  {/if}
</div>

<style>
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

  /* ── Drag-drop hover overlay ── */
  .drop-overlay {
    position: absolute;
    inset: 0;
    pointer-events: none;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 2px dashed var(--btn-active-border);
    border-radius: 6px;
    background: color-mix(in srgb, var(--btn-active-bg) 18%, transparent);
    z-index: 200;
  }

  .drop-label {
    padding: 10px 24px;
    font-size: 1.1rem;
    font-weight: 600;
    color: var(--btn-active-text);
    background: var(--panel-bg-alpha);
    border: 1px solid var(--btn-active-border);
    border-radius: 8px;
    letter-spacing: 0.02em;
  }
</style>
