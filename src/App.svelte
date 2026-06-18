<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
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
  let dragHover = false;

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

  /** Apply the given theme to the document root and (if the renderer is ready)
   *  update the WebGPU clear color and re-render. */
  function applyTheme(theme: string, renderNow = false) {
    document.documentElement.setAttribute('data-theme', theme);
    const bg = THEME_BG[theme] ?? THEME_BG['dark'];
    try {
      renderer.setBackground(...bg);
      if (renderNow) renderer.render();
    } catch (_) {
      // renderer may not be initialised yet on first call — that's fine
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
  let normalized = false;

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

  function handleNormalized(event: CustomEvent<{ value: boolean }>) {
    normalized = event.detail.value;
    try { renderer.setNormalized(normalized); } catch (_) {}
    refreshView();
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

    // Apply persisted theme to chrome immediately (renderer not ready yet).
    document.documentElement.setAttribute('data-theme', prefs.theme);

    try {
      await renderer.init();
      await renderer.create(canvas);
      // Now set the WebGPU background for the persisted theme and render.
      applyTheme(prefs.theme, true);
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

    // Register Tauri drag-drop listener (OS drops give file paths; HTML5 ondrop does not).
    const unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === 'over') {
        dragHover = true;
      } else if (p.type === 'leave') {
        dragHover = false;
      } else if (p.type === 'drop') {
        dragHover = false;
        const path = (p as { type: string; paths?: string[] }).paths?.[0];
        if (path) { void openPath(path); }
      }
    });

    return () => {
      ro.disconnect();
      unlistenDrop();
    };
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
        {normalized}
        on:linewidth={handleLineWidth}
        on:pointradius={handlePointRadius}
        on:showgrid={handleShowGrid}
        on:normalized={handleNormalized}
      />
    {/if}
    {#if dragHover}
      <div class="drop-overlay" aria-hidden="true">
        <span class="drop-label">Drop a CSV / Excel file to open</span>
      </div>
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
