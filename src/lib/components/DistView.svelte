<script lang="ts">
  /**
   * DistView.svelte — static SVG histogram (bar chart) for a single series.
   *
   * Analogous to TableView: pulls a snapshot from the WASM renderer on demand
   * (no reactive WASM push) and re-renders. Not interactive — a fixed
   * viewBox is stretched to fill the parent container via CSS.
   */
  import { onMount } from 'svelte';
  import type { Renderer, HistogramData } from '../renderer.js';

  export let renderer: Renderer;
  export let seriesIndex: number;

  const NBINS = 40;

  // ── State ─────────────────────────────────────────────────────────────────
  let data: HistogramData | null = null;
  let error = '';
  let mounted = false;

  // ── Lifecycle ────────────────────────────────────────────────────────────
  onMount(() => {
    mounted = true;
    refresh();
  });

  /** Pull the histogram for `seriesIndex` from the renderer. */
  export function refresh(): void {
    try {
      data = renderer.seriesHistogram(seriesIndex, NBINS);
      error = '';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      data = null;
    }
  }

  // Re-pull whenever the selected series changes, once mounted/ready.
  $: if (mounted) {
    seriesIndex;
    refresh();
  }

  // ── Layout (viewBox coordinate space; stretched to fill via CSS) ──────────
  const W = 600;
  const H = 320;
  const MARGIN_LEFT = 56;
  const MARGIN_RIGHT = 12;
  const MARGIN_TOP = 12;
  const MARGIN_BOTTOM = 32;
  const PLOT_W = W - MARGIN_LEFT - MARGIN_RIGHT;
  const PLOT_H = H - MARGIN_TOP - MARGIN_BOTTOM;
  const BAR_GAP = 1;

  $: maxCount = data ? Math.max(0, ...data.counts) : 0;
  $: hasBars = data !== null && data.counts.length > 0 && maxCount > 0;

  $: bars = hasBars && data
    ? data.counts.map((count, i) => {
        const barW = PLOT_W / data!.counts.length;
        const h = (count / maxCount) * PLOT_H;
        return {
          x: MARGIN_LEFT + i * barW + BAR_GAP / 2,
          y: MARGIN_TOP + (PLOT_H - h),
          width: Math.max(0, barW - BAR_GAP),
          height: h
        };
      })
    : [];

  $: xMinLabel = data ? fmt(data.min) : '';
  $: xMidLabel = data ? fmt((data.min + data.max) / 2) : '';
  $: xMaxLabel = data ? fmt(data.max) : '';
  $: yMaxLabel = hasBars ? fmt(maxCount) : '';

  /** Format a number to ~4 significant figures without exponential notation
   *  for the typical sensor-log range. */
  function fmt(n: number): string {
    if (!isFinite(n)) return '—';
    if (n === 0) return '0';
    return Number(n.toPrecision(4)).toString();
  }
</script>

<div class="dist-view">
  <svg
    viewBox="0 0 {W} {H}"
    preserveAspectRatio="none"
    class="dist-svg"
  >
    {#if error || data === null || !hasBars}
      <text
        x={W / 2}
        y={H / 2}
        text-anchor="middle"
        dominant-baseline="middle"
        fill="var(--text-muted)"
        font-size="13"
        font-family="monospace"
      >{error || (data === null ? 'No data' : 'No values in range')}</text>
    {:else}
      <!-- Bars -->
      {#each bars as bar}
        <rect
          x={bar.x}
          y={bar.y}
          width={bar.width}
          height={bar.height}
          fill="var(--accent)"
        />
      {/each}

      <!-- Axis baseline -->
      <line
        x1={MARGIN_LEFT} y1={MARGIN_TOP + PLOT_H}
        x2={MARGIN_LEFT + PLOT_W} y2={MARGIN_TOP + PLOT_H}
        stroke="var(--axis-text)"
        stroke-width="1"
      />

      <!-- X labels: min / mid / max -->
      <text x={MARGIN_LEFT} y={H - 10} text-anchor="start" font-size="11" font-family="monospace" fill="var(--axis-text)">{xMinLabel}</text>
      <text x={MARGIN_LEFT + PLOT_W / 2} y={H - 10} text-anchor="middle" font-size="11" font-family="monospace" fill="var(--axis-text)">{xMidLabel}</text>
      <text x={MARGIN_LEFT + PLOT_W} y={H - 10} text-anchor="end" font-size="11" font-family="monospace" fill="var(--axis-text)">{xMaxLabel}</text>

      <!-- Y labels: 0 / max(counts) -->
      <text x={MARGIN_LEFT - 6} y={MARGIN_TOP + PLOT_H} text-anchor="end" dominant-baseline="text-bottom" font-size="11" font-family="monospace" fill="var(--axis-text)">0</text>
      <text x={MARGIN_LEFT - 6} y={MARGIN_TOP + 4} text-anchor="end" dominant-baseline="hanging" font-size="11" font-family="monospace" fill="var(--axis-text)">{yMaxLabel}</text>
    {/if}
  </svg>
</div>

<style>
  .dist-view {
    width: 100%;
    height: 100%;
    background: var(--bg);
    display: flex;
    overflow: hidden;
  }

  .dist-svg {
    width: 100%;
    height: 100%;
    display: block;
  }
</style>
