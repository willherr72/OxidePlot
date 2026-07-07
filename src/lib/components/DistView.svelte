<script lang="ts">
  /**
   * DistView.svelte — small multiples: one static SVG histogram (bar chart)
   * per plotted series, each drawn in that series' own color.
   *
   * Analogous to TableView: pulls a snapshot from the WASM renderer on demand
   * (no reactive WASM push) and re-renders. Not interactive — each panel's
   * viewBox tracks the container's real pixel width (via bind:clientWidth on
   * the outer wrapper) so bars aren't stretched horizontally; panel height is
   * fixed so the whole view scrolls when there are many series.
   */
  import { onMount } from 'svelte';
  import type { Renderer, HistogramData, SeriesInfoEntry } from '../renderer.js';

  export let renderer: Renderer;

  const NBINS = 40;

  /** Per-series panel state. */
  interface SeriesPanel {
    index: number;
    name: string;
    colorCss: string;
    visible: boolean;
    data: HistogramData | null;
    error: string;
  }

  // ── State ─────────────────────────────────────────────────────────────────
  let panels: SeriesPanel[] = [];

  // ── Lifecycle ────────────────────────────────────────────────────────────
  onMount(() => {
    refresh();
  });

  /** Convert a [r, g, b, a] (0..1 floats) array to a CSS rgba() string.
   *  Matches SeriesList.svelte's toCSS so panel colors match the series list. */
  function colorToCss(color: [number, number, number, number]): string {
    const [r, g, b, a] = color;
    return `rgba(${(r * 255) | 0}, ${(g * 255) | 0}, ${(b * 255) | 0}, ${a})`;
  }

  /** Pull the series list and a histogram for each plotted series from the renderer. */
  export function refresh(): void {
    let infos: SeriesInfoEntry[] = [];
    try {
      infos = renderer.seriesInfo();
    } catch (_) {
      infos = [];
    }

    panels = infos.map((info, i) => {
      let data: HistogramData | null = null;
      let error = '';
      try {
        data = renderer.seriesHistogram(i, NBINS);
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
      return {
        index: i,
        name: info.name,
        colorCss: colorToCss(info.color),
        visible: info.visible,
        data,
        error,
      };
    });
  }

  // ── Layout (viewBox coordinate space; width is 1:1 with the container's
  //    real pixel size via bind:clientWidth below, so bars aren't stretched;
  //    height is fixed per panel — the outer view scrolls if there are many
  //    series) ──────────────────────────────────────────────────────────────
  let W = 800;
  const PANEL_H = 160; // must match .dist-svg's rendered height in CSS below
  const MARGIN_LEFT = 56;
  const MARGIN_RIGHT = 12;
  const MARGIN_TOP = 10;
  const MARGIN_BOTTOM = 28;
  const PLOT_H = PANEL_H - MARGIN_TOP - MARGIN_BOTTOM;
  const BAR_GAP = 1;
  $: PLOT_W = W - MARGIN_LEFT - MARGIN_RIGHT;

  // Guard the first render before the container has been measured.
  $: measured = W >= 10;

  interface Layout {
    hasBars: boolean;
    bars: { x: number; y: number; width: number; height: number }[];
    xMinLabel: string;
    xMidLabel: string;
    xMaxLabel: string;
    yMaxLabel: string;
  }

  /** Compute bar rects + axis labels for one panel's histogram, in local
   *  (per-panel) coordinates — offset by MARGIN_LEFT/MARGIN_TOP by the caller. */
  function computeLayout(data: HistogramData | null): Layout {
    const maxCount = data ? Math.max(0, ...data.counts) : 0;
    const hasBars = data !== null && data.counts.length > 0 && maxCount > 0;
    const bars =
      hasBars && data
        ? data.counts.map((count, i) => {
            const barW = PLOT_W / data.counts.length;
            const h = (count / maxCount) * PLOT_H;
            return {
              x: MARGIN_LEFT + i * barW + BAR_GAP / 2,
              y: MARGIN_TOP + (PLOT_H - h),
              width: Math.max(0, barW - BAR_GAP),
              height: h,
            };
          })
        : [];
    return {
      hasBars,
      bars,
      xMinLabel: data ? fmt(data.min) : '',
      xMidLabel: data ? fmt((data.min + data.max) / 2) : '',
      xMaxLabel: data ? fmt(data.max) : '',
      yMaxLabel: hasBars ? fmt(maxCount) : '',
    };
  }

  /** Format a number to ~4 significant figures without exponential notation
   *  for the typical sensor-log range. */
  function fmt(n: number): string {
    if (!isFinite(n)) return '—';
    if (n === 0) return '0';
    return Number(n.toPrecision(4)).toString();
  }
</script>

<div class="dist-view" bind:clientWidth={W}>
  {#if panels.length === 0}
    <div class="dist-empty">No series plotted</div>
  {:else if measured}
    {#each panels as panel (panel.index)}
      {@const layout = computeLayout(panel.data)}
      <div class="series-panel" class:hidden={!panel.visible}>
        <div class="panel-label" style="color:{panel.colorCss}" title={panel.name}>{panel.name}</div>
        <svg viewBox="0 0 {W} {PANEL_H}" class="dist-svg">
          {#if panel.error || panel.data === null || !layout.hasBars}
            <text
              x={W / 2}
              y={PANEL_H / 2}
              text-anchor="middle"
              dominant-baseline="middle"
              fill="var(--text-muted)"
              font-size="12"
              font-family="monospace"
            >{panel.error || (panel.data === null ? 'No data' : 'No values in range')}</text>
          {:else}
            <!-- Bars -->
            {#each layout.bars as bar}
              <rect
                x={bar.x}
                y={bar.y}
                width={bar.width}
                height={bar.height}
                fill={panel.colorCss}
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
            <text x={MARGIN_LEFT} y={PANEL_H - 8} text-anchor="start" font-size="10" font-family="monospace" fill="var(--axis-text)">{layout.xMinLabel}</text>
            <text x={MARGIN_LEFT + PLOT_W / 2} y={PANEL_H - 8} text-anchor="middle" font-size="10" font-family="monospace" fill="var(--axis-text)">{layout.xMidLabel}</text>
            <text x={MARGIN_LEFT + PLOT_W} y={PANEL_H - 8} text-anchor="end" font-size="10" font-family="monospace" fill="var(--axis-text)">{layout.xMaxLabel}</text>

            <!-- Y labels: 0 / max(counts) -->
            <text x={MARGIN_LEFT - 6} y={MARGIN_TOP + PLOT_H} text-anchor="end" dominant-baseline="text-bottom" font-size="10" font-family="monospace" fill="var(--axis-text)">0</text>
            <text x={MARGIN_LEFT - 6} y={MARGIN_TOP + 4} text-anchor="end" dominant-baseline="hanging" font-size="10" font-family="monospace" fill="var(--axis-text)">{layout.yMaxLabel}</text>
          {/if}
        </svg>
      </div>
    {/each}
  {/if}
</div>

<style>
  .dist-view {
    width: 100%;
    height: 100%;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .dist-empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    font-family: monospace;
    font-size: 13px;
  }

  .series-panel {
    flex-shrink: 0;
    height: 180px; /* label (20px, below) + PANEL_H (160px, in <script>) */
    border-bottom: 1px solid var(--border);
  }

  .series-panel.hidden {
    opacity: 0.45;
  }

  .panel-label {
    height: 20px; /* + .dist-svg's 160px below = .series-panel's 180px above */
    line-height: 20px;
    padding: 0 8px;
    box-sizing: border-box;
    font-family: var(--font-ui);
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dist-svg {
    display: block;
    width: 100%;
    height: 160px; /* must match PANEL_H in <script> */
  }
</style>
