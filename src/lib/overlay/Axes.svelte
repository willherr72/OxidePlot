<script lang="ts">
  /**
   * Axes.svelte — SVG tick-mark + label overlay over the plot canvas.
   *
   * Renders short tick marks and text labels for X (bottom edge) and Y (left edge).
   * Major ticks are longer and brighter than minor ticks.
   * Full-extent gridlines are drawn for major ticks (very faint).
   *
   * pointer-events: none so all mouse events pass through to the canvas.
   */

  export interface TickEntry {
    value: number;
    label: string;
    major: boolean;
  }

  export interface AxisTicksData {
    x: TickEntry[];
    y: TickEntry[];
  }

  export interface ViewState {
    x_min: number;
    x_max: number;
    y_min: number;
    y_max: number;
  }

  export let ticks: AxisTicksData | null = null;
  export let viewState: ViewState | null = null;
  export let displayW: number = 0;
  export let displayH: number = 0;

  // Tick dimensions (CSS px)
  const MAJOR_TICK_LEN = 8;
  const MINOR_TICK_LEN = 4;
  const FONT_SIZE = 11;
  const LABEL_OFFSET_X = 14; // px below the bottom tick
  const LABEL_OFFSET_Y = 4;  // px right of the left tick

  // Margin: leave some room so labels at edges aren't clipped
  const EDGE_MARGIN = 30;

  function xToScreen(value: number): number {
    if (!viewState || viewState.x_max === viewState.x_min) return 0;
    return (value - viewState.x_min) / (viewState.x_max - viewState.x_min) * displayW;
  }

  function yToScreen(value: number): number {
    if (!viewState || viewState.y_max === viewState.y_min) return 0;
    return (1 - (value - viewState.y_min) / (viewState.y_max - viewState.y_min)) * displayH;
  }

  $: xTicks = (ticks?.x ?? []).filter(t => {
    const px = xToScreen(t.value);
    return px >= EDGE_MARGIN && px <= displayW - EDGE_MARGIN;
  });

  $: yTicks = (ticks?.y ?? []).filter(t => {
    const py = yToScreen(t.value);
    return py >= EDGE_MARGIN && py <= displayH - EDGE_MARGIN;
  });
</script>

{#if displayW > 0 && displayH > 0 && ticks && viewState}
<svg
  width={displayW}
  height={displayH}
  style="position:absolute;top:0;left:0;pointer-events:none;overflow:visible"
>
  <!-- Faint major gridlines for X -->
  {#each xTicks.filter(t => t.major) as tick}
    {@const px = xToScreen(tick.value)}
    <line
      x1={px} y1={0}
      x2={px} y2={displayH}
      stroke="rgba(255,255,255,0.06)"
      stroke-width="1"
    />
  {/each}

  <!-- Faint major gridlines for Y -->
  {#each yTicks.filter(t => t.major) as tick}
    {@const py = yToScreen(tick.value)}
    <line
      x1={0} y1={py}
      x2={displayW} y2={py}
      stroke="rgba(255,255,255,0.06)"
      stroke-width="1"
    />
  {/each}

  <!-- X axis ticks + labels (bottom edge) -->
  {#each xTicks as tick}
    {@const px = xToScreen(tick.value)}
    {@const len = tick.major ? MAJOR_TICK_LEN : MINOR_TICK_LEN}
    {@const col = tick.major ? 'rgba(220,220,240,0.85)' : 'rgba(180,180,200,0.5)'}
    <line
      x1={px} y1={displayH - len}
      x2={px} y2={displayH}
      stroke={col}
      stroke-width="1"
    />
    {#if tick.major}
      <text
        x={px}
        y={displayH - MAJOR_TICK_LEN - LABEL_OFFSET_X + FONT_SIZE}
        text-anchor="middle"
        font-size={FONT_SIZE}
        fill="rgba(200,200,220,0.9)"
        font-family="monospace"
        style="paint-order:stroke;stroke:rgba(10,10,18,0.7);stroke-width:3px;stroke-linejoin:round"
      >{tick.label}</text>
    {/if}
  {/each}

  <!-- Y axis ticks + labels (left edge) -->
  {#each yTicks as tick}
    {@const py = yToScreen(tick.value)}
    {@const len = tick.major ? MAJOR_TICK_LEN : MINOR_TICK_LEN}
    {@const col = tick.major ? 'rgba(220,220,240,0.85)' : 'rgba(180,180,200,0.5)'}
    <line
      x1={0} y1={py}
      x2={len} y2={py}
      stroke={col}
      stroke-width="1"
    />
    {#if tick.major}
      <text
        x={len + LABEL_OFFSET_Y}
        y={py + FONT_SIZE / 2 - 1}
        text-anchor="start"
        font-size={FONT_SIZE}
        fill="rgba(200,200,220,0.9)"
        font-family="monospace"
        style="paint-order:stroke;stroke:rgba(10,10,18,0.7);stroke-width:3px;stroke-linejoin:round"
      >{tick.label}</text>
    {/if}
  {/each}
</svg>
{/if}

<style>
  svg {
    position: absolute;
    top: 0;
    left: 0;
    pointer-events: none;
  }
</style>
