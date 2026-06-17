<script lang="ts">
  import { onMount } from 'svelte';
  import { pickFile, readFile, saveFile, loadPrefs, savePrefs } from './lib/api.js';
  import { Renderer } from './lib/renderer.js';
  import type { FileMeta, SeriesSpec, AxisTicksData, ViewState, SeriesInfoEntry } from './lib/renderer.js';
  import ColumnDialog from './lib/components/ColumnDialog.svelte';
  import SeriesList from './lib/components/SeriesList.svelte';
  import Settings from './lib/components/Settings.svelte';
  import Axes from './lib/overlay/Axes.svelte';
  import Cursors from './lib/overlay/Cursors.svelte';
  import type { CursorPoint } from './lib/overlay/Cursors.svelte';

  let canvas: HTMLCanvasElement;
  const renderer = new Renderer();

  let fileMeta: FileMeta | null = null;
  let filePath: string | null = null;
  let error: string | null = null;
  let loading = false;
  let viewState: ViewState | null = null;

  // ── Prefs ──────────────────────────────────────────────────────────────────
  interface Prefs {
    recentFiles: string[];
    theme: string;
  }
  const DEFAULT_PREFS: Prefs = { recentFiles: [], theme: 'dark' };
  let prefs: Prefs = { ...DEFAULT_PREFS };
  let showRecent = false;

  /** Persist current prefs to disk. Task 5.6 can also call this after changing theme. */
  async function persistPrefs() {
    try {
      await savePrefs(JSON.stringify(prefs));
    } catch (_) {
      // non-fatal: silently ignore
    }
  }

  /** Add path to recent files (dedupe, cap at 8), then persist. */
  async function recordRecentFile(path: string) {
    const filtered = prefs.recentFiles.filter(p => p !== path);
    prefs = { ...prefs, recentFiles: [path, ...filtered].slice(0, 8) };
    await persistPrefs();
  }
  let ticks: AxisTicksData | null = null;
  let seriesInfo: SeriesInfoEntry[] = [];

  function refreshView() {
    try {
      viewState = renderer.viewState();
      ticks = renderer.axisTicks();
    } catch (_) {
      // renderer not ready yet
    }
  }

  function refreshSeriesInfo() {
    try {
      seriesInfo = renderer.seriesInfo();
    } catch (_) {
      seriesInfo = [];
    }
  }

  function handleSeriesChange() {
    refreshSeriesInfo();
    refreshView();
  }

  // ── Settings panel ─────────────────────────────────────────────────────────
  let showSettings = false;
  let showGrid = true;
  let lineWidth = 2.0;
  let pointRadius = 3.0;

  function toggleSettings() {
    showSettings = !showSettings;
  }

  function handleLineWidth(event: CustomEvent<{ value: number }>) {
    lineWidth = event.detail.value;
    try { renderer.setLineWidth(lineWidth); } catch (_) {}
    refreshView();
  }

  function handlePointRadius(event: CustomEvent<{ value: number }>) {
    pointRadius = event.detail.value;
    try { renderer.setPointRadius(pointRadius); } catch (_) {}
    refreshView();
  }

  function handleShowGrid(event: CustomEvent<{ value: boolean }>) {
    showGrid = event.detail.value;
  }

  // ── Draw mode ──────────────────────────────────────────────────────────────
  type DrawMode = 'lines' | 'step' | 'points';
  const DRAW_MODES: DrawMode[] = ['lines', 'step', 'points'];
  const DRAW_MODE_LABELS: Record<DrawMode, string> = { lines: 'Lines', step: 'Step', points: 'Points' };
  let drawMode: DrawMode = 'lines';
  let hasData = false;

  function cycleDrawMode() {
    if (!hasData) return;
    const idx = DRAW_MODES.indexOf(drawMode);
    drawMode = DRAW_MODES[(idx + 1) % DRAW_MODES.length];
    renderer.setDrawMode(drawMode);
    refreshView();
  }

  function handleFit() {
    renderer.autoFit();
    refreshView();
  }

  // ── Cursor mode ────────────────────────────────────────────────────────────
  let cursorMode = false;
  let cursors: CursorPoint[] = [];

  function toggleCursorMode() {
    cursorMode = !cursorMode;
    if (!cursorMode) {
      cursors = []; // clear cursors when turning off
    }
  }

  // ── Pan state ──────────────────────────────────────────────────────────────
  let dragging = false;
  let lastPx = 0;
  let lastPy = 0;
  // Track pointer-down CSS position for click-vs-drag discrimination
  let pointerDownCssX = 0;
  let pointerDownCssY = 0;
  const CLICK_THRESHOLD_PX = 4;

  /** CSS-pixel → canvas-backing-pixel scale factors. */
  function pixelScale(): { sx: number; sy: number } {
    const rect = canvas.getBoundingClientRect();
    return {
      sx: canvas.width / rect.width,
      sy: canvas.height / rect.height,
    };
  }

  function onPointerDown(e: PointerEvent) {
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

    try {
      await renderer.init();
      await renderer.create(canvas);
      renderer.render(); // blank dark frame
      refreshView();
    } catch (e) {
      error = String(e);
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
    return () => ro.disconnect();
  });

  /** Load a file at a known path (shared by dialog-pick and recent-click). */
  async function openPath(path: string) {
    loading = true;
    error = null;
    try {
      filePath = path;
      const numArr = await readFile(path);
      const bytes = new Uint8Array(numArr);
      const filename = path.split(/[\\/]/).pop() ?? path;
      fileMeta = renderer.loadFileBytes(bytes, filename);
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
    try {
      renderer.setSeries(specs);
      hasData = true;
      drawMode = 'lines'; // reset to default on new data load
      refreshView();
      refreshSeriesInfo();
    } catch (e) {
      error = `Failed to render series: ${e}`;
    }
  }

  function handleCancel() {
    fileMeta = null;
  }

  // ── Export ─────────────────────────────────────────────────────────────────

  async function handleExportCsv() {
    if (!hasData) return;
    error = null;
    try {
      const csv = renderer.exportCsv();
      const bytes = new TextEncoder().encode(csv);
      await saveFile('oxideplot.csv', bytes);
    } catch (e) {
      error = `Export CSV failed: ${e}`;
    }
  }

  async function handleExportPng() {
    if (!hasData) return;
    error = null;
    try {
      // Render immediately before capturing so the drawing buffer is current.
      renderer.render();
      const blob = await new Promise<Blob | null>((resolve) => {
        canvas.toBlob((b) => resolve(b), 'image/png');
      });
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
    if (!hasData) return;
    error = null;
    try {
      renderer.render();
      const blob = await new Promise<Blob | null>((resolve) => {
        canvas.toBlob((b) => resolve(b), 'image/png');
      });
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
      disabled={!hasData}
      on:click={handleFit}
      title="Re-fit view to all data (same as double-click)"
    >
      Fit
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
    {#if filePath && !fileMeta}
      <span class="file-label" title={filePath}>
        {filePath.split(/[\\/]/).pop()}
      </span>
    {/if}
    {#if error}
      <span class="error-msg">{error}</span>
    {/if}
  </div>

  <!-- Plot canvas + axis overlay — fills the remaining space -->
  <div class="canvas-wrap">
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
    {#if seriesInfo.length > 0}
      <SeriesList
        series={seriesInfo}
        {renderer}
        on:change={handleSeriesChange}
      />
    {/if}
    {#if showSettings}
      <Settings
        {lineWidth}
        {pointRadius}
        {showGrid}
        on:linewidth={handleLineWidth}
        on:pointradius={handlePointRadius}
        on:showgrid={handleShowGrid}
      />
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
  :global(body) {
    margin: 0;
    background: #1a1a1f;
    overflow: hidden;
    color: #e0e0ee;
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
    background: #111118;
    border-bottom: 1px solid #2a2a3a;
    flex-shrink: 0;
    height: 42px;
    box-sizing: border-box;
  }

  .open-btn {
    padding: 5px 16px;
    background: #3060c0;
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
    padding: 5px 14px;
    background: #2a2a3a;
    color: #b0b0cc;
    border: 1px solid #44445a;
    border-radius: 5px;
    cursor: pointer;
    font-size: 0.85rem;
    font-weight: 600;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
  }

  .cursor-btn:hover {
    background: #3a3a52;
    color: #e0e0ff;
  }

  .cursor-btn.active {
    background: #1a4a60;
    color: #00e5ff;
    border-color: #00b8d9;
  }

  .draw-mode-btn {
    padding: 5px 14px;
    background: #2a2a3a;
    color: #b0b0cc;
    border: 1px solid #44445a;
    border-radius: 5px;
    cursor: pointer;
    font-size: 0.85rem;
    font-weight: 600;
    min-width: 60px;
    transition: background 0.15s, color 0.15s;
  }

  .draw-mode-btn:hover:not(:disabled) {
    background: #3a3a52;
    color: #e0e0ff;
  }

  .draw-mode-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .file-label {
    font-size: 0.8rem;
    color: #8888aa;
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
    background: #1a1a28;
    border: 1px solid #44445a;
    border-radius: 5px;
    min-width: 220px;
    max-width: 400px;
    z-index: 100;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.5);
    overflow: hidden;
  }

  .recent-item {
    display: block;
    width: 100%;
    padding: 6px 12px;
    background: transparent;
    color: #b0b0cc;
    border: none;
    border-bottom: 1px solid #2a2a3a;
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
    background: #2a2a42;
    color: #e0e0ff;
  }

  .canvas-wrap {
    position: relative;
    flex: 1;
    overflow: hidden;
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
</style>
