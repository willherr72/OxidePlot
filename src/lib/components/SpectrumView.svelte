<script lang="ts">
  /**
   * SpectrumView.svelte — overlaid PSD (power spectral density) line chart.
   *
   * Analogous to DistView: pulls a snapshot from the WASM renderer on demand
   * (no reactive WASM push) and re-renders. Not interactive — the single
   * chart's viewBox tracks the container's real pixel size (via
   * bind:clientWidth/clientHeight on the outer wrapper) so it isn't
   * stretched. Unlike DistView's small multiples, every plotted series is
   * drawn as one polyline overlaid on a single set of axes: X = frequency
   * (linear), Y = power (log10 scale, shared min/max across all series).
   */
  import { onMount } from 'svelte';
  import type { Renderer, SpectrumData, SeriesInfoEntry } from '../renderer.js';

  export let renderer: Renderer;
  export let sampleRate: number | null = null;

  /** Per-series state. */
  interface SeriesLine {
    index: number;
    name: string;
    colorCss: string;
    visible: boolean;
    data: SpectrumData | null;
    error: string;
  }

  // ── State ─────────────────────────────────────────────────────────────────
  let lines: SeriesLine[] = [];
  /** "fs = <rate> Hz" caption, taken from the first series that returned data. */
  let fsCaption = '';

  // ── Lifecycle ────────────────────────────────────────────────────────────
  onMount(() => {
    refresh();
  });

  /** Convert a [r, g, b, a] (0..1 floats) array to a CSS rgba() string.
   *  Matches SeriesList.svelte's toCSS so line colors match the series list. */
  function colorToCss(color: [number, number, number, number]): string {
    const [r, g, b, a] = color;
    return `rgba(${(r * 255) | 0}, ${(g * 255) | 0}, ${(b * 255) | 0}, ${a})`;
  }

  /** Pull the series list and a PSD for each plotted series from the renderer. */
  export function refresh(): void {
    let infos: SeriesInfoEntry[] = [];
    try {
      infos = renderer.seriesInfo();
    } catch (_) {
      infos = [];
    }

    let fs = '';
    lines = infos.map((info, i) => {
      let data: SpectrumData | null = null;
      let error = '';
      try {
        data = renderer.seriesSpectrum(i, sampleRate ?? undefined);
        if (!fs && data) fs = `fs = ${fmt(data.sample_rate)} Hz`;
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
    fsCaption = fs;
  }

  // ── Layout (viewBox coordinate space; both dimensions are 1:1 with the
  //    container's real pixel size via bind:clientWidth/clientHeight below,
  //    so the chart isn't stretched) ──────────────────────────────────────
  let W = 800;
  let H = 400;
  const MARGIN_LEFT = 60;
  const MARGIN_RIGHT = 16;
  const MARGIN_TOP = 20;
  const MARGIN_BOTTOM = 28;
  $: PLOT_W = W - MARGIN_LEFT - MARGIN_RIGHT;
  $: PLOT_H = H - MARGIN_TOP - MARGIN_BOTTOM;

  // Guard the first render before the container has been measured.
  $: measured = W >= 10 && H >= 10;

  interface PolylineSpec {
    index: number;
    colorCss: string;
    visible: boolean;
    points: string;
  }

  interface GlobalLayout {
    hasData: boolean;
    polylines: PolylineSpec[];
    xMidLabel: string;
    xMaxLabel: string;
    yMinLabel: string;
    yMidLabel: string;
    yMaxLabel: string;
    message: string;
  }

  /** Compute one polyline per series plus shared axis labels, in local
   *  (viewBox) coordinates. X = frequency, linear, 0..maxFreq (max across
   *  all series). Y = log10(power), mapped between the global min/max
   *  log-power across all series; points with power <= 0 are dropped. */
  function computeGlobalLayout(seriesLines: SeriesLine[]): GlobalLayout {
    let maxFreq = 0;
    let minLog = Infinity;
    let maxLog = -Infinity;
    // Only overlay VISIBLE series with data — hiding a series (legend eye)
    // removes its PSD line and drops it from the shared axis, like the plot.
    const withData = seriesLines.filter((l) => l.visible && l.data !== null && l.data.freqs.length > 0);

    for (const line of withData) {
      const data = line.data as SpectrumData;
      for (let j = 0; j < data.freqs.length; j++) {
        if (data.freqs[j] > maxFreq) maxFreq = data.freqs[j];
        const p = data.power[j];
        if (p > 0) {
          const lp = Math.log10(p);
          if (lp < minLog) minLog = lp;
          if (lp > maxLog) maxLog = lp;
        }
      }
    }

    const noMessage = { xMidLabel: '', xMaxLabel: '', yMinLabel: '', yMidLabel: '', yMaxLabel: '' };

    if (withData.length === 0 || maxFreq <= 0 || !isFinite(minLog) || !isFinite(maxLog)) {
      const firstError = seriesLines.find((l) => l.error)?.error;
      return {
        hasData: false,
        polylines: [],
        ...noMessage,
        message: firstError || 'No spectrum data',
      };
    }

    // Avoid a degenerate (zero-height) log axis when every point shares the
    // same power (e.g. a single-sample series).
    let dispMinLog = minLog;
    let dispMaxLog = maxLog;
    if (dispMaxLog - dispMinLog < 1e-9) {
      dispMinLog -= 0.5;
      dispMaxLog += 0.5;
    }

    const xScale = (freq: number) => MARGIN_LEFT + (freq / maxFreq) * PLOT_W;
    const yScale = (logPower: number) =>
      MARGIN_TOP + PLOT_H - ((logPower - dispMinLog) / (dispMaxLog - dispMinLog)) * PLOT_H;

    const polylines: PolylineSpec[] = withData
      .map((line) => {
        const data = line.data as SpectrumData;
        const pts: string[] = [];
        for (let j = 0; j < data.freqs.length; j++) {
          const p = data.power[j];
          if (p > 0) {
            pts.push(`${xScale(data.freqs[j])},${yScale(Math.log10(p))}`);
          }
        }
        return { index: line.index, colorCss: line.colorCss, visible: line.visible, points: pts.join(' ') };
      })
      .filter((p) => p.points.length > 0);

    if (polylines.length === 0) {
      return { hasData: false, polylines: [], ...noMessage, message: 'No spectrum data' };
    }

    const midLog = (dispMinLog + dispMaxLog) / 2;

    return {
      hasData: true,
      polylines,
      xMidLabel: `${fmt(maxFreq / 2)} Hz`,
      xMaxLabel: `${fmt(maxFreq)} Hz`,
      yMinLabel: fmtExp(dispMinLog),
      yMidLabel: fmtExp(midLog),
      yMaxLabel: fmtExp(dispMaxLog),
      message: '',
    };
  }

  /** Format a number to ~4 significant figures without exponential notation
   *  for the typical sensor-log range. */
  function fmt(n: number): string {
    if (!isFinite(n)) return '—';
    if (n === 0) return '0';
    return Number(n.toPrecision(4)).toString();
  }

  /** Format a log10 value as the exponent shown in a "10^n" axis label. */
  function fmtExp(n: number): string {
    if (!isFinite(n)) return '—';
    const rounded = Math.round(n * 10) / 10;
    return Number.isInteger(rounded) ? `${rounded}` : rounded.toFixed(1);
  }
</script>

<div class="spectrum-view" bind:clientWidth={W} bind:clientHeight={H}>
  {#if lines.length === 0}
    <div class="spectrum-empty">No series plotted</div>
  {:else if measured}
    {@const layout = computeGlobalLayout(lines)}
    <svg viewBox="0 0 {W} {H}" class="spectrum-svg">
      {#if !layout.hasData}
        <text
          x={W / 2}
          y={H / 2}
          text-anchor="middle"
          dominant-baseline="middle"
          fill="var(--text-muted)"
          font-size="12"
          font-family="monospace"
        >{layout.message}</text>
      {:else}
        <!-- Axis baseline -->
        <line
          x1={MARGIN_LEFT} y1={MARGIN_TOP + PLOT_H}
          x2={MARGIN_LEFT + PLOT_W} y2={MARGIN_TOP + PLOT_H}
          stroke="var(--axis-text)"
          stroke-width="1"
        />

        <!-- Per-series PSD lines -->
        {#each layout.polylines as line (line.index)}
          <polyline
            points={line.points}
            fill="none"
            stroke={line.colorCss}
            stroke-width="1.5"
            opacity={line.visible ? 1 : 0.35}
          />
        {/each}

        <!-- X labels: 0 / mid / max frequency (Hz) -->
        <text x={MARGIN_LEFT} y={H - 8} text-anchor="start" font-size="10" font-family="monospace" fill="var(--axis-text)">0 Hz</text>
        <text x={MARGIN_LEFT + PLOT_W / 2} y={H - 8} text-anchor="middle" font-size="10" font-family="monospace" fill="var(--axis-text)">{layout.xMidLabel}</text>
        <text x={MARGIN_LEFT + PLOT_W} y={H - 8} text-anchor="end" font-size="10" font-family="monospace" fill="var(--axis-text)">{layout.xMaxLabel}</text>

        <!-- Y labels: min / mid / max power, as powers of ten -->
        <text x={MARGIN_LEFT - 6} y={MARGIN_TOP + PLOT_H} text-anchor="end" dominant-baseline="text-bottom" font-size="10" font-family="monospace" fill="var(--axis-text)">10<tspan font-size="8" dy="-4">{layout.yMinLabel}</tspan></text>
        <text x={MARGIN_LEFT - 6} y={MARGIN_TOP + PLOT_H / 2} text-anchor="end" dominant-baseline="middle" font-size="10" font-family="monospace" fill="var(--axis-text)">10<tspan font-size="8" dy="-4">{layout.yMidLabel}</tspan></text>
        <text x={MARGIN_LEFT - 6} y={MARGIN_TOP + 4} text-anchor="end" dominant-baseline="hanging" font-size="10" font-family="monospace" fill="var(--axis-text)">10<tspan font-size="8" dy="-4">{layout.yMaxLabel}</tspan></text>

        <!-- Sample-rate caption -->
        {#if fsCaption}
          <text x={W - MARGIN_RIGHT} y={12} text-anchor="end" font-size="10" font-family="monospace" fill="var(--text-muted)">{fsCaption}</text>
        {/if}
      {/if}
    </svg>
  {/if}
</div>

<style>
  .spectrum-view {
    width: 100%;
    height: 100%;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .spectrum-empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    font-family: monospace;
    font-size: 13px;
  }

  .spectrum-svg {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
