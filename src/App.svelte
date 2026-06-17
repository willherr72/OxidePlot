<script lang="ts">
  import { onMount } from 'svelte';
  import { pickFile, readFile } from './lib/api.js';
  import { Renderer } from './lib/renderer.js';
  import type { FileMeta, SeriesSpec, AxisTicksData, ViewState } from './lib/renderer.js';
  import ColumnDialog from './lib/components/ColumnDialog.svelte';
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
  let ticks: AxisTicksData | null = null;

  function refreshView() {
    try {
      viewState = renderer.viewState();
      ticks = renderer.axisTicks();
    } catch (_) {
      // renderer not ready yet
    }
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

  async function handleOpen() {
    error = null;
    loading = true;
    try {
      const path = await pickFile();
      if (!path) { loading = false; return; }
      filePath = path;

      const numArr = await readFile(path);
      const bytes = new Uint8Array(numArr);
      const filename = path.split(/[\\/]/).pop() ?? path;

      fileMeta = renderer.loadFileBytes(bytes, filename);
    } catch (e) {
      error = `Failed to open file: ${e}`;
    } finally {
      loading = false;
    }
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
    } catch (e) {
      error = `Failed to render series: ${e}`;
    }
  }

  function handleCancel() {
    fileMeta = null;
  }
</script>

<main>
  <!-- Toolbar -->
  <div class="toolbar">
    <button class="open-btn" on:click={handleOpen} disabled={loading}>
      {loading ? 'Loading…' : 'Open File'}
    </button>
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
    />
    <Cursors
      {cursors}
      {viewState}
      displayW={canvas ? canvas.getBoundingClientRect().width : 0}
      displayH={canvas ? canvas.getBoundingClientRect().height : 0}
    />
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
