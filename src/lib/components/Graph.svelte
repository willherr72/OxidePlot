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
  import SpectrumView from './SpectrumView.svelte';
  import SpectrogramView from './SpectrogramView.svelte';
  import ScatterView from './ScatterView.svelte';
  import Axes from '../overlay/Axes.svelte';
  import Cursors from '../overlay/Cursors.svelte';
  import type { CursorPoint } from '../overlay/Cursors.svelte';

  // ── Public props ────────────────────────────────────────────────────────────
  /** Draw a focus border when true. */
  export let focused = false;
  /** True when the graph stack overflows and can scroll — plain wheel then
   *  scrolls the stack instead of zooming (Ctrl/Cmd+wheel still zooms). */
  export let canScrollStack = false;

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
  /** Basename of the file loaded into this graph (per-graph, shown by App when
   *  focused — the workspace can hold a different file per graph). */
  let fileName = '';
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

  // ── View mode (plot / table / dist / spectrum / spectrogram / scatter) ──────
  let viewMode: 'plot' | 'table' | 'dist' | 'spectrum' | 'spectrogram' | 'scatter' = 'plot';
  let tableView: TableView;
  let distView: DistView;
  let spectrumView: SpectrumView;
  let spectrogramView: SpectrogramView;
  let scatterView: ScatterView;

  // ── Scatter (XY) view column selection — X/Y dataset column indices, and
  //    the dataset's column names for the header dropdowns. Reset on new
  //    data (setSeries) so a fresh file starts at cols 0/1. ───────────────
  let scatterX = 0;
  let scatterY = 1;
  let columnNames: string[] = [];

  // ── Selected series (SeriesList row selection; Dist now shows all series as
  //    small multiples and no longer depends on this — used by the
  //    single-series Spectrogram view) ─────────────────────────────────────
  let selectedSeriesIndex = 0;

  // ── Sample rate override for spectral views (null = infer from X) ───────────
  let sampleRate: number | null = null;
  /** True when the X axis is datetime — the sample rate is then inferred
   *  reliably, so the manual rate field is hidden (only shown when there's no
   *  timestamp to infer from). */
  let xIsTime = false;

  // ── Draw mode ────────────────────────────────────────────────────────────────
  type DrawMode = 'lines' | 'step' | 'points';
  const DRAW_MODES: DrawMode[] = ['lines', 'step', 'points'];
  let drawMode: DrawMode = 'lines';

  // ── Cursor mode ──────────────────────────────────────────────────────────────
  let cursorMode = false;
  let cursors: CursorPoint[] = [];

  // ── Drag state ───────────────────────────────────────────────────────────────
  // Left-drag draws a rubber-band ZOOM box (with X/Y/box axis snapping); right-drag
  // PANS. Wheel scrolls the stack / Ctrl+wheel zooms; double-click fits.
  type DragMode = 'none' | 'pan' | 'zoom';
  let dragMode: DragMode = 'none';
  let lastPx = 0;
  let lastPy = 0;
  // Track pointer-down CSS position for click-vs-drag discrimination
  let pointerDownCssX = 0;
  let pointerDownCssY = 0;
  const CLICK_THRESHOLD_PX = 4;
  // Rubber-band zoom box (CSS px, canvas-relative) + its snapped axis.
  let zoomBox: { x0: number; y0: number; x1: number; y1: number } | null = null;
  let zoomSnap: 'x' | 'y' | 'box' = 'box';
  // Within ~20° of an axis, the drag snaps to that axis only; more diagonal = box.
  const SNAP_TAN = Math.tan((20 * Math.PI) / 180);
  /** Overlay rectangle (CSS px) for the current zoom box, shaped by the snap. */
  $: zoomRect = zoomBox && canvas ? computeZoomRect(zoomBox, zoomSnap) : null;
  function computeZoomRect(
    box: { x0: number; y0: number; x1: number; y1: number },
    snap: 'x' | 'y' | 'box',
  ): { left: number; top: number; width: number; height: number } {
    const r = canvas.getBoundingClientRect();
    if (snap === 'x') {
      return { left: Math.min(box.x0, box.x1), top: 0, width: Math.abs(box.x1 - box.x0), height: r.height };
    }
    if (snap === 'y') {
      return { left: 0, top: Math.min(box.y0, box.y1), width: r.width, height: Math.abs(box.y1 - box.y0) };
    }
    return {
      left: Math.min(box.x0, box.x1),
      top: Math.min(box.y0, box.y1),
      width: Math.abs(box.x1 - box.x0),
      height: Math.abs(box.y1 - box.y0),
    };
  }

  // ── View refresh ─────────────────────────────────────────────────────────────
  /** Pull view state from the renderer without emitting any events.
   *  Called by the exported `refresh()` (App-driven) to avoid the
   *  App.syncFromGraph → refresh → dispatch('xrange') → handleXRange recursion. */
  function pullViewState() {
    try {
      viewState = renderer.viewState();
      ticks = renderer.axisTicks();
      xIsTime = renderer.xIsTime();
      // Datetime X infers the rate reliably; drop any stale manual override so a
      // leftover value from previous non-time data can't silently apply.
      if (xIsTime) sampleRate = null;
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
    const rect = canvas.getBoundingClientRect();
    pointerDownCssX = e.clientX - rect.left;
    pointerDownCssY = e.clientY - rect.top;

    if (e.button === 2) {
      // Right button → PAN. Track backing-pixel position for the pan delta.
      e.preventDefault();
      dragMode = 'pan';
      const { sx, sy } = pixelScale();
      lastPx = pointerDownCssX * sx;
      lastPy = pointerDownCssY * sy;
      canvas.setPointerCapture(e.pointerId);
    } else if (e.button === 0) {
      // Left button → ZOOM box (or, in cursor mode, a cursor-placement click,
      // resolved in onPointerUp). No box until the pointer leaves the dead zone.
      dragMode = 'zoom';
      zoomBox = null;
      canvas.setPointerCapture(e.pointerId);
    }
  }

  function onPointerMove(e: PointerEvent) {
    if (dragMode === 'none') return;
    const rect = canvas.getBoundingClientRect();
    const cssX = e.clientX - rect.left;
    const cssY = e.clientY - rect.top;

    if (dragMode === 'pan') {
      const { sx, sy } = pixelScale();
      const curX = cssX * sx;
      const curY = cssY * sy;
      renderer.pan(curX - lastPx, curY - lastPy);
      lastPx = curX;
      lastPy = curY;
      refreshView();
    } else if (dragMode === 'zoom' && !cursorMode) {
      // Update the rubber band + snapped axis (nothing until past the dead zone).
      const dx = Math.abs(cssX - pointerDownCssX);
      const dy = Math.abs(cssY - pointerDownCssY);
      if (dx < CLICK_THRESHOLD_PX && dy < CLICK_THRESHOLD_PX) {
        zoomBox = null;
        return;
      }
      if (dy <= dx * SNAP_TAN) zoomSnap = 'x'; // mostly horizontal → X-only band
      else if (dx <= dy * SNAP_TAN) zoomSnap = 'y'; // mostly vertical → Y-only band
      else zoomSnap = 'box';
      zoomBox = { x0: pointerDownCssX, y0: pointerDownCssY, x1: cssX, y1: cssY };
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (dragMode === 'none') return;
    const mode = dragMode;
    dragMode = 'none';
    const rect = canvas.getBoundingClientRect();
    const upCssX = e.clientX - rect.left;
    const upCssY = e.clientY - rect.top;

    if (mode === 'pan') return;

    // mode === 'zoom'
    const box = zoomBox;
    zoomBox = null;

    // Cursor mode: a left click/drag places a measurement cursor (no zoom).
    if (cursorMode && viewState) {
      const dataX = viewState.x_min + (pointerDownCssX / rect.width) * (viewState.x_max - viewState.x_min);
      const dataY = viewState.y_min + (1 - pointerDownCssY / rect.height) * (viewState.y_max - viewState.y_min);
      cursors = cursors.length >= 2 ? [{ x: dataX, y: dataY }] : [...cursors, { x: dataX, y: dataY }];
      return;
    }

    // Below the threshold (or no box / no view) → treat as a click, do nothing.
    const moved = Math.hypot(upCssX - pointerDownCssX, upCssY - pointerDownCssY);
    if (moved < CLICK_THRESHOLD_PX || !box || !viewState) return;

    // Commit the zoom: map the box corners to data coords and set the view. Only
    // the snapped axis(es) change; the other keeps the current view range.
    const w = rect.width;
    const h = rect.height;
    const { x_min: vx0, x_max: vx1, y_min: vy0, y_max: vy1 } = viewState;
    const dataX = (cx: number) => vx0 + (cx / w) * (vx1 - vx0);
    const dataY = (cy: number) => vy0 + (1 - cy / h) * (vy1 - vy0);

    let nx0 = vx0;
    let nx1 = vx1;
    let ny0 = vy0;
    let ny1 = vy1;
    if (zoomSnap === 'x' || zoomSnap === 'box') {
      const a = dataX(box.x0);
      const b = dataX(box.x1);
      nx0 = Math.min(a, b);
      nx1 = Math.max(a, b);
    }
    if (zoomSnap === 'y' || zoomSnap === 'box') {
      const a = dataY(box.y0);
      const b = dataY(box.y1);
      ny0 = Math.min(a, b);
      ny1 = Math.max(a, b);
    }
    renderer.setViewBounds(nx0, nx1, ny0, ny1);
    refreshView();
  }

  function onPointerCancel(_e: PointerEvent) {
    dragMode = 'none';
    zoomBox = null;
  }

  function onWheel(e: WheelEvent) {
    // Plain wheel scrolls the graph stack when it overflows (`canScrollStack`);
    // Ctrl/Cmd + wheel ALWAYS zooms the plot. When the stack fits (nothing to
    // scroll), plain wheel zooms too — so a single graph needs no modifier.
    const zoomIntent = e.ctrlKey || e.metaKey;
    if (!zoomIntent && canScrollStack) {
      return; // don't preventDefault — let the event bubble so the stack scrolls
    }
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

    // Register Tauri drag-drop listener (OS drops give file paths; HTML5 ondrop
    // does not). onDragDropEvent is WEBVIEW-GLOBAL — it fires for EVERY graph on
    // any drop — so we must check the event position against THIS graph's bounds
    // and only react when the cursor is actually over it. Position is physical
    // pixels (window-relative); convert to CSS px via devicePixelRatio to compare
    // with getBoundingClientRect.
    const hitsThisGraph = (pos: { x: number; y: number } | undefined): boolean => {
      if (!pos || !canvas) return false;
      const rect = canvas.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      const x = pos.x / dpr;
      const y = pos.y / dpr;
      return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
    };
    const unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload as { type: string; paths?: string[]; position?: { x: number; y: number } };
      if (p.type === 'over') {
        // Highlight only the graph under the cursor.
        dragHover = hitsThisGraph(p.position);
      } else if (p.type === 'leave') {
        dragHover = false;
      } else if (p.type === 'drop') {
        dragHover = false;
        if (!hitsThisGraph(p.position)) return; // dropped on a different graph
        const path = p.paths?.[0];
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
    fileName = filename; // remember which file this graph holds (per-graph label)
    return meta;
  }

  /** Basename of the file loaded into this graph (App shows the focused graph's). */
  export function getFileName(): string { return fileName; }

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
    refreshColumnNames();
    scatterX = 0; // fresh data — default the Scatter view to cols 0/1
    scatterY = columnNames.length > 1 ? 1 : 0;
    if (viewMode === 'table') {
      tick().then(() => { if (tableView) tableView.refresh(); });
    } else if (viewMode === 'dist') {
      tick().then(() => { if (distView) distView.refresh(); });
    } else if (viewMode === 'spectrum') {
      tick().then(() => { if (spectrumView) spectrumView.refresh(); });
    } else if (viewMode === 'spectrogram') {
      tick().then(() => { if (spectrogramView) spectrogramView.refresh(); });
    } else if (viewMode === 'scatter') {
      tick().then(() => { if (scatterView) scatterView.refresh(); });
    }
    dispatch('datachanged');
  }

  /** Re-pull the loaded dataset's column names for the Scatter view's X/Y
   *  header dropdowns. No-op (empty list) if no file is loaded. */
  function refreshColumnNames(): void {
    columnNames = getColumnNames();
  }

  /**
   * Create a derived column from `expr` (a formula over existing columns —
   * see the `+ƒ Formula` editor in App.svelte) and plot it against the
   * current X axis. Throws (propagates the WASM error) if there's no data
   * loaded, no series plotted yet, or the expression is invalid/empty.
   */
  export function deriveColumn(name: string, expr: string): void {
    renderer.deriveColumn(name, expr);
    refreshView();
    refreshSeriesInfo();
    refreshColumnNames();
    if (viewMode === 'table') {
      tick().then(() => { if (tableView) tableView.refresh(); });
    } else if (viewMode === 'dist') {
      tick().then(() => { if (distView) distView.refresh(); });
    } else if (viewMode === 'spectrum') {
      tick().then(() => { if (spectrumView) spectrumView.refresh(); });
    } else if (viewMode === 'spectrogram') {
      tick().then(() => { if (spectrogramView) spectrogramView.refresh(); });
    } else if (viewMode === 'scatter') {
      tick().then(() => { if (scatterView) scatterView.refresh(); });
    }
    dispatch('datachanged');
  }

  /** Column names of the loaded dataset, in file order (empty if no file is
   *  loaded). Used by the `+ƒ Formula` editor's clickable column list. */
  export function getColumnNames(): string[] {
    try {
      return renderer.columnNames();
    } catch (_) {
      return [];
    }
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

  /** Switch to `mode` (plot/table/dist/spectrum/spectrogram/scatter); mounts + refreshes the target view on switch. */
  export async function setViewMode(mode: 'plot' | 'table' | 'dist' | 'spectrum' | 'spectrogram' | 'scatter'): Promise<void> {
    viewMode = mode;
    await tick();
    if (mode === 'table') {
      tableView?.refresh();
    } else if (mode === 'dist') {
      distView?.refresh();
    } else if (mode === 'spectrum') {
      spectrumView?.refresh();
    } else if (mode === 'spectrogram') {
      spectrogramView?.refresh();
    } else if (mode === 'scatter') {
      if (columnNames.length === 0) refreshColumnNames();
      scatterView?.refresh();
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

  /** Draw a filled + stroked rounded rectangle path (manual — avoids relying
   *  on the newer `CanvasRenderingContext2D.roundRect`). */
  function drawRoundedRectPath(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
    const rr = Math.min(r, w / 2, h / 2);
    ctx.beginPath();
    ctx.moveTo(x + rr, y);
    ctx.arcTo(x + w, y, x + w, y + h, rr);
    ctx.arcTo(x + w, y + h, x, y + h, rr);
    ctx.arcTo(x, y + h, x, y, rr);
    ctx.arcTo(x, y, x + w, y, rr);
    ctx.closePath();
  }

  /**
   * Capture a composite "figure" PNG: the WebGPU plot bitmap plus a 2D-canvas
   * overlay of axis ticks/labels and a series legend, so exports look like a
   * proper figure rather than a bare (mute) plot.
   *
   * Reuses `capturePng()` for the plot bitmap (the WebGPU readback already
   * works — not reinvented here). The tick→pixel mapping mirrors
   * `overlay/Axes.svelte`'s `xToScreen`/`yToScreen` exactly, just offset by
   * the margins reserved for axis labels:
   *   x: plotLeft + (value - x_min) / (x_max - x_min) * plotW
   *   y: plotTop + (1 - (value - y_min) / (y_max - y_min)) * plotH
   *
   * Falls back to the bare `capturePng()` result when not in plot view, when
   * there's no data, or when view/tick state isn't available — never throws.
   */
  export async function captureFigurePng(): Promise<Blob | null> {
    if (viewMode !== 'plot' || !hasData || !viewState || !ticks || !canvas) {
      return capturePng();
    }

    const plotBlob = await capturePng();
    if (!plotBlob) return null;

    // The plot bitmap's pixel size is the canvas's own backing-store size
    // (canvas.width/height), set 1:1 from the CSS-pixel ResizeObserver rect
    // (this renderer does not scale the backing store by devicePixelRatio).
    // Axes.svelte's displayW/displayH come from the same CSS-pixel
    // getBoundingClientRect(), so plot-bitmap pixels and tick-mapping pixels
    // are already in the same units — no dpi rescale needed here.
    const plotW = canvas.width;
    const plotH = canvas.height;
    if (plotW === 0 || plotH === 0) return plotBlob;

    let bitmap: ImageBitmap;
    try {
      bitmap = await createImageBitmap(plotBlob);
    } catch (_) {
      return plotBlob; // ImageBitmap decode unsupported — fall back to the bare plot
    }

    const LEFT = 64, RIGHT = 12, TOP = 12, BOTTOM = 36;
    const width = plotW + LEFT + RIGHT;
    const height = plotH + TOP + BOTTOM;

    const off = document.createElement('canvas');
    off.width = width;
    off.height = height;
    const ctx = off.getContext('2d');
    if (!ctx) {
      bitmap.close?.();
      return plotBlob;
    }

    const style = getComputedStyle(document.documentElement);
    const readVar = (name: string, fallback: string) => {
      const v = style.getPropertyValue(name).trim();
      return v || fallback;
    };
    const bg = readVar('--bg', '#0e0f13');
    const axisText = readVar('--axis-text', 'rgba(205, 210, 220, 0.92)');
    const textColor = readVar('--text', '#e6e8ec');
    const panelBg = readVar('--panel-bg-alpha', 'rgba(17, 19, 24, 0.86)');
    const panelBorder = readVar('--border-mid', 'rgba(255, 255, 255, 0.18)');

    // Background
    ctx.fillStyle = bg;
    ctx.fillRect(0, 0, width, height);

    // Plot bitmap, offset by the left/top margins.
    ctx.drawImage(bitmap, LEFT, TOP, plotW, plotH);
    bitmap.close?.();

    const { x_min, x_max, y_min, y_max } = viewState;
    const TICK_FONT = '11px "SFMono-Regular", Consolas, "Courier New", monospace';
    const TICK_LEN = 6;

    ctx.strokeStyle = axisText;
    ctx.fillStyle = axisText;
    ctx.lineWidth = 1;
    ctx.font = TICK_FONT;

    // Y axis — major ticks only (mirrors Axes.svelte's yToScreen, offset by TOP).
    if (y_max !== y_min) {
      ctx.textAlign = 'right';
      ctx.textBaseline = 'middle';
      for (const t of ticks.y) {
        if (!t.major) continue;
        const py = TOP + (1 - (t.value - y_min) / (y_max - y_min)) * plotH;
        if (py < TOP || py > TOP + plotH) continue;
        ctx.beginPath();
        ctx.moveTo(LEFT - TICK_LEN, py);
        ctx.lineTo(LEFT, py);
        ctx.stroke();
        ctx.fillText(t.label, LEFT - TICK_LEN - 4, py);
      }
    }

    // X axis — major ticks only (mirrors Axes.svelte's xToScreen, offset by LEFT).
    if (x_max !== x_min) {
      ctx.textAlign = 'center';
      ctx.textBaseline = 'top';
      for (const t of ticks.x) {
        if (!t.major) continue;
        const px = LEFT + (t.value - x_min) / (x_max - x_min) * plotW;
        if (px < LEFT || px > LEFT + plotW) continue;
        ctx.beginPath();
        ctx.moveTo(px, TOP + plotH);
        ctx.lineTo(px, TOP + plotH + TICK_LEN);
        ctx.stroke();
        ctx.fillText(t.label, px, TOP + plotH + TICK_LEN + 3);
      }
    }

    // Legend — top-right inside the plot area, visible series only.
    const visibleSeries = seriesInfo.filter(s => s.visible);
    if (visibleSeries.length > 0) {
      const LEGEND_FONT = '12px "SFMono-Regular", Consolas, "Courier New", monospace';
      ctx.font = LEGEND_FONT;
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';

      const swatchSize = 10;
      const rowH = 18;
      const padX = 10;
      const padY = 8;
      const gap = 6;
      const margin = 10; // gap from the plot's own top/right edge

      let maxTextW = 0;
      for (const s of visibleSeries) {
        const w = ctx.measureText(s.name).width;
        if (w > maxTextW) maxTextW = w;
      }
      const legendW = padX * 2 + swatchSize + gap + maxTextW;
      const legendH = padY * 2 + visibleSeries.length * rowH;
      const legendX = LEFT + plotW - legendW - margin;
      const legendY = TOP + margin;

      ctx.fillStyle = panelBg;
      ctx.strokeStyle = panelBorder;
      ctx.lineWidth = 1;
      drawRoundedRectPath(ctx, legendX, legendY, legendW, legendH, 6);
      ctx.fill();
      ctx.stroke();

      visibleSeries.forEach((s, i) => {
        const rowY = legendY + padY + i * rowH + rowH / 2;
        const [r, g, b, a] = s.color;
        ctx.fillStyle = `rgba(${(r * 255) | 0}, ${(g * 255) | 0}, ${(b * 255) | 0}, ${a})`;
        ctx.fillRect(legendX + padX, rowY - swatchSize / 2, swatchSize, swatchSize);
        ctx.fillStyle = textColor;
        ctx.fillText(s.name, legendX + padX + swatchSize + gap, rowY);
      });
    }

    return await new Promise<Blob | null>((resolve) => {
      off.toBlob((b) => resolve(b), 'image/png');
    });
  }

  // ── Exposed: read-only accessors for App's panels ─────────────────────────────
  /** Re-pull all of this graph's panel-facing state from the renderer.
   *  Pure pull — does NOT emit xrange, so App.syncFromGraph() calling this
   *  never triggers the handleXRange → syncFromGraph recursion. */
  export function refresh(): void {
    refreshSeriesInfo();
    pullViewState();
    // Keep the active non-plot view in sync with series changes (visibility,
    // color, add/remove) — e.g. hiding a series must update the Dist/Spectrum
    // overlay, not just the Plot view.
    if (viewMode === 'table') tick().then(() => tableView?.refresh());
    else if (viewMode === 'dist') tick().then(() => distView?.refresh());
    else if (viewMode === 'spectrum') tick().then(() => spectrumView?.refresh());
    else if (viewMode === 'spectrogram') tick().then(() => spectrogramView?.refresh());
    else if (viewMode === 'scatter') tick().then(() => scatterView?.refresh());
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
  export function getViewMode(): 'plot' | 'table' | 'dist' | 'spectrum' | 'spectrogram' | 'scatter' { return viewMode; }
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
    if (viewMode === 'spectrogram') spectrogramView?.refresh();
  }

  // ── Sample-rate override input (Spectrum/Spectrogram header field) ──────────
  /** Handle input/change on the sample-rate field: empty → null (infer),
   *  otherwise the parsed number; then refresh whichever spectral view is active. */
  function onSampleRateInput(e: Event): void {
    const raw = (e.target as HTMLInputElement).value;
    sampleRate = raw === '' ? null : Number(raw);
    if (viewMode === 'spectrum') {
      spectrumView?.refresh();
    } else if (viewMode === 'spectrogram') {
      spectrogramView?.refresh();
    }
  }

  // ── Scatter view X/Y column selectors (header dropdowns) ────────────────
  /** `<select>` values are strings — coerce back to the column index. */
  function onScatterXChange(e: Event): void {
    scatterX = Number((e.target as HTMLSelectElement).value);
    scatterView?.refresh();
  }
  function onScatterYChange(e: Event): void {
    scatterY = Number((e.target as HTMLSelectElement).value);
    scatterView?.refresh();
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
    <button
      class="view-tab"
      class:active={viewMode === 'spectrum'}
      on:click={() => setViewMode('spectrum')}
      title="Spectrum view"
    >Spectrum</button>
    <button
      class="view-tab"
      class:active={viewMode === 'spectrogram'}
      on:click={() => setViewMode('spectrogram')}
      title="Spectrogram view"
    >Spectrogram</button>
    <button
      class="view-tab"
      class:active={viewMode === 'scatter'}
      on:click={() => setViewMode('scatter')}
      title="Scatter (XY) view"
    >Scatter</button>
    {#if (viewMode === 'spectrum' || viewMode === 'spectrogram') && !xIsTime}
      <input
        class="sample-rate-input"
        type="number"
        placeholder="sample rate (Hz)"
        value={sampleRate ?? ''}
        on:input={onSampleRateInput}
        on:change={onSampleRateInput}
        title="Sample rate (Hz) — needed for real frequency labels because this X axis has no timestamps to infer from"
      />
    {/if}
    {#if viewMode === 'scatter'}
      <select
        class="scatter-axis-select"
        value={scatterX}
        on:change={onScatterXChange}
        title="X axis column"
      >
        {#each columnNames as name, i}
          <option value={i}>X: {name}</option>
        {/each}
      </select>
      <select
        class="scatter-axis-select"
        value={scatterY}
        on:change={onScatterYChange}
        title="Y axis column"
      >
        {#each columnNames as name, i}
          <option value={i}>Y: {name}</option>
        {/each}
      </select>
    {/if}
  </div>
{/if}

<!-- Table / Dist / Spectrum / Spectrogram view — rendered alongside (not replacing) the canvas -->
{#if hasData && viewMode === 'table'}
  <TableView bind:this={tableView} {renderer} />
{:else if hasData && viewMode === 'dist'}
  <DistView bind:this={distView} {renderer} />
{:else if hasData && viewMode === 'spectrum'}
  <SpectrumView bind:this={spectrumView} {renderer} {sampleRate} />
{:else if hasData && viewMode === 'spectrogram'}
  <SpectrogramView bind:this={spectrogramView} {renderer} seriesIndex={selectedSeriesIndex} {sampleRate} />
{:else if hasData && viewMode === 'scatter'}
  <ScatterView bind:this={scatterView} {renderer} xCol={scatterX} yCol={scatterY} />
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
    on:contextmenu={(e) => e.preventDefault()}
  ></canvas>
  {#if zoomRect}
    <div
      class="zoom-box"
      style="left:{zoomRect.left}px; top:{zoomRect.top}px; width:{zoomRect.width}px; height:{zoomRect.height}px"
      aria-hidden="true"
    ></div>
  {/if}
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

  /* Sample-rate override field (Spectrum/Spectrogram header). */
  .sample-rate-input {
    width: 96px;
    margin-left: 4px;
    padding: 3px 6px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-family: var(--font-ui);
    font-size: 0.7rem;
  }

  .sample-rate-input:focus {
    outline: none;
    border-color: var(--btn-active-border);
  }

  /* Scatter-view X/Y column selectors (header). */
  .scatter-axis-select {
    margin-left: 4px;
    padding: 3px 6px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-family: var(--font-ui);
    font-size: 0.7rem;
    max-width: 160px;
  }

  .scatter-axis-select:focus {
    outline: none;
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
    cursor: crosshair; /* left-drag draws a zoom box; right-drag pans */
  }

  /* Rubber-band zoom box (left-drag; a band when snapped to one axis). */
  .zoom-box {
    position: absolute;
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border: 1px solid var(--accent);
    pointer-events: none;
    z-index: 6;
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
