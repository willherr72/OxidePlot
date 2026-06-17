<script lang="ts">
  /**
   * Cursors.svelte — Measurement cursor overlay over the plot canvas.
   *
   * Renders crosshair lines (vertical + horizontal) for up to 2 cursor points,
   * plus a readout panel showing X/Y values and ΔX/ΔY between the two cursors.
   *
   * Cursor positions are stored in DATA coordinates so they stay pinned to the
   * data when panning or zooming — the overlay re-renders when viewState changes.
   *
   * pointer-events: none so all mouse events pass through to the canvas.
   */

  import type { ViewState } from '../renderer.js';

  export interface CursorPoint {
    x: number;
    y: number;
  }

  export let cursors: CursorPoint[] = [];
  export let viewState: ViewState | null = null;
  export let displayW: number = 0;
  export let displayH: number = 0;

  // Per-cursor colors: cursor 0 = cyan, cursor 1 = magenta
  const CURSOR_COLORS = ['#00e5ff', '#ff00cc'];

  function xToScreen(value: number): number {
    if (!viewState || viewState.x_max === viewState.x_min) return 0;
    return (value - viewState.x_min) / (viewState.x_max - viewState.x_min) * displayW;
  }

  function yToScreen(value: number): number {
    if (!viewState || viewState.y_max === viewState.y_min) return 0;
    return (1 - (value - viewState.y_min) / (viewState.y_max - viewState.y_min)) * displayH;
  }

  function fmt(v: number): string {
    // Use toPrecision(6) for compact but meaningful display
    return Number(v.toPrecision(6)).toString();
  }

  $: screenCursors = cursors.map(c => ({
    sx: xToScreen(c.x),
    sy: yToScreen(c.y),
    dataX: c.x,
    dataY: c.y,
  }));

  $: hasDelta = cursors.length === 2;
  $: deltaX = hasDelta ? cursors[1].x - cursors[0].x : 0;
  $: deltaY = hasDelta ? cursors[1].y - cursors[0].y : 0;
</script>

{#if displayW > 0 && displayH > 0 && viewState && cursors.length > 0}
<!-- SVG crosshair lines -->
<svg
  width={displayW}
  height={displayH}
  style="position:absolute;top:0;left:0;pointer-events:none;overflow:visible"
>
  {#each screenCursors as sc, i}
    {@const color = CURSOR_COLORS[i] ?? '#ffffff'}
    <!-- Vertical line at cursor X -->
    <line
      x1={sc.sx} y1={0}
      x2={sc.sx} y2={displayH}
      stroke={color}
      stroke-width="1.5"
      stroke-dasharray="6,4"
      opacity="0.85"
    />
    <!-- Horizontal line at cursor Y -->
    <line
      x1={0} y1={sc.sy}
      x2={displayW} y2={sc.sy}
      stroke={color}
      stroke-width="1.5"
      stroke-dasharray="6,4"
      opacity="0.85"
    />
    <!-- Small crosshair marker dot at intersection -->
    <circle
      cx={sc.sx}
      cy={sc.sy}
      r="4"
      fill={color}
      opacity="0.9"
      stroke="rgba(10,10,18,0.7)"
      stroke-width="1"
    />
  {/each}
</svg>

<!-- Readout panel — positioned top-right, HTML div for easy text layout -->
<div class="cursor-readout" style="pointer-events:none">
  {#each cursors as c, i}
    {@const color = CURSOR_COLORS[i] ?? '#ffffff'}
    <div class="cursor-row">
      <span class="cursor-label" style="color:{color}">C{i + 1}</span>
      <span class="cursor-vals">X={fmt(c.x)}  Y={fmt(c.y)}</span>
    </div>
  {/each}
  {#if hasDelta}
    <div class="cursor-divider"></div>
    <div class="cursor-row delta">
      <span class="cursor-label">Δ</span>
      <span class="cursor-vals">ΔX={fmt(deltaX)}  ΔY={fmt(deltaY)}</span>
    </div>
  {/if}
</div>
{/if}

<style>
  .cursor-readout {
    position: absolute;
    top: 10px;
    right: 10px;
    background: rgba(10, 10, 20, 0.82);
    border: 1px solid rgba(180, 180, 220, 0.25);
    border-radius: 6px;
    padding: 7px 10px;
    font-family: monospace;
    font-size: 11px;
    color: #d0d0ee;
    min-width: 200px;
    user-select: none;
  }

  .cursor-row {
    display: flex;
    gap: 8px;
    align-items: baseline;
    line-height: 1.6;
  }

  .cursor-label {
    font-weight: 700;
    min-width: 18px;
  }

  .cursor-vals {
    color: #c0c0dd;
    white-space: pre;
  }

  .cursor-divider {
    border-top: 1px solid rgba(180, 180, 220, 0.2);
    margin: 3px 0;
  }

  .delta .cursor-label {
    color: #aaaacc;
  }

  .delta .cursor-vals {
    color: #e0e0ff;
  }
</style>
