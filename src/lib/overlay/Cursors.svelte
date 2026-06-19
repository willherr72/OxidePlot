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
   * Colors use CSS custom properties (--cursor-readout-bg etc.) so the component
   * responds to the active data-theme automatically.
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
      stroke="var(--cursor-dot-stroke)"
      stroke-width="1"
    />
  {/each}
</svg>

<!-- Readout panel — top-left (the series panel lives top-right), HTML div for easy text layout -->
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
    left: 10px;
    background: var(--cursor-readout-bg);
    border: 1px solid var(--cursor-readout-border);
    border-radius: var(--radius);
    padding: 7px 10px;
    font-family: var(--font-data);
    font-size: 11px;
    color: var(--cursor-readout-text);
    min-width: 200px;
    user-select: none;
    backdrop-filter: blur(4px);
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
    color: var(--cursor-readout-vals);
    white-space: pre;
  }

  .cursor-divider {
    border-top: 1px solid var(--cursor-divider);
    margin: 3px 0;
  }

  .delta .cursor-label {
    color: var(--cursor-delta-label);
  }

  .delta .cursor-vals {
    color: var(--cursor-delta-vals);
  }
</style>
