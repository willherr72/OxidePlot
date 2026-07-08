<script lang="ts">
  /**
   * SpectrogramView.svelte — 2D-canvas magma heatmap of the SELECTED series'
   * short-time FFT spectrogram (frequency vs time).
   *
   * Analogous to DistView/SpectrumView: pulls a snapshot from the WASM
   * renderer on demand (no reactive WASM push) and re-renders. Unlike those
   * SVG-based views, the heatmap itself is painted onto a 2D `<canvas>` via
   * `ImageData`/`putImageData` (per-pixel color lookups aren't practical as
   * SVG); axis ticks/labels are baked onto the same canvas with `fillText`.
   *
   * Only ONE series is shown (the plot's selected series), not every plotted
   * series — a spectrogram is already a dense 2D image, so overlaying more
   * than one wouldn't be legible.
   */
  import { onMount } from 'svelte';
  import type { Renderer, SpectrogramData, SeriesInfoEntry } from '../renderer.js';

  export let renderer: Renderer;
  export let seriesIndex: number;
  export let sampleRate: number | null = null;

  const WINDOW = 256;

  // ── Layout (canvas backing-store pixels; 1:1 with the container's real
  //    pixel size via bind:clientWidth/clientHeight below — this renderer
  //    does not scale the backing store by devicePixelRatio, matching
  //    Graph.svelte's capture path). Margins reserve room for axis labels. ──
  const MARGIN_LEFT = 52;
  const MARGIN_RIGHT = 10;
  const MARGIN_TOP = 14;
  const MARGIN_BOTTOM = 26;

  // ── State ─────────────────────────────────────────────────────────────────
  let canvas: HTMLCanvasElement;
  let W = 0;
  let H = 0;
  let mounted = false;
  let data: SpectrogramData | null = null;
  let error = '';

  // ── Lifecycle ────────────────────────────────────────────────────────────
  onMount(() => {
    mounted = true;
  });

  // Re-pull whenever the selected series or sample rate changes. Guarded by
  // `mounted` so this doesn't fire before the canvas element is bound.
  $: if (mounted) {
    void seriesIndex;
    void sampleRate;
    refresh();
  }

  // Resize the canvas backing store to the measured container and redraw
  // whenever the container's pixel size changes.
  $: if (canvas && W > 0 && H > 0) {
    canvas.width = W;
    canvas.height = H;
    draw();
  }

  /** Pull a fresh spectrogram for `seriesIndex` from the renderer and redraw. */
  export function refresh(): void {
    data = null;
    error = '';

    let infos: SeriesInfoEntry[] = [];
    try {
      infos = renderer.seriesInfo();
    } catch (_) {
      infos = [];
    }

    if (infos.length === 0) {
      error = 'No series plotted';
    } else {
      try {
        const d = renderer.seriesSpectrogram(seriesIndex, WINDOW, sampleRate ?? undefined);
        if (!d || d.n_frames === 0 || d.bins === 0 || d.frames.length === 0) {
          error = 'No spectrogram data';
        } else {
          data = d;
        }
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    }

    draw();
  }

  // ── Magma colormap — ported from `heat_color()` in
  //    crates/oxideplot-mcp/src/main.rs (5-stop gradient, t in 0..1 → RGB
  //    0..255), so the MCP tool's rendered spectrograms and this in-app view
  //    use the same palette. ──────────────────────────────────────────────
  const MAGMA_STOPS: [number, [number, number, number]][] = [
    [0.0, [0.0, 0.0, 0.02]],
    [0.25, [0.28, 0.05, 0.35]],
    [0.5, [0.65, 0.18, 0.42]],
    [0.75, [0.95, 0.45, 0.28]],
    [1.0, [0.99, 0.87, 0.55]],
  ];

  function magma(t: number): [number, number, number] {
    const tc = Math.min(1, Math.max(0, t));
    let i = 0;
    while (i + 1 < MAGMA_STOPS.length && tc > MAGMA_STOPS[i + 1][0]) {
      i++;
    }
    const [t0, c0] = MAGMA_STOPS[i];
    const [t1, c1] = MAGMA_STOPS[Math.min(i + 1, MAGMA_STOPS.length - 1)];
    const f = t1 > t0 ? (tc - t0) / (t1 - t0) : 0;
    return [
      Math.floor((c0[0] + (c1[0] - c0[0]) * f) * 255),
      Math.floor((c0[1] + (c1[1] - c0[1]) * f) * 255),
      Math.floor((c0[2] + (c1[2] - c0[2]) * f) * 255),
    ];
  }

  /** 5th–99.5th percentile of log10(magnitude + 1e-12) over every cell, used
   *  as a robust color range so a few very loud bins don't wash out the rest
   *  of the heatmap. */
  function colorRange(d: SpectrogramData): [number, number] {
    const values: number[] = [];
    for (const frame of d.frames) {
      for (const mag of frame) {
        values.push(Math.log10(mag + 1e-12));
      }
    }
    if (values.length === 0) return [0, 1];
    values.sort((a, b) => a - b);
    const at = (p: number) => values[Math.min(values.length - 1, Math.max(0, Math.floor(p * values.length)))];
    const lo = at(0.05);
    const hi = at(0.995);
    return hi > lo ? [lo, hi] : [lo, lo + 1e-6];
  }

  /** Read a CSS custom property off `:root`, falling back if unset (matches
   *  Graph.svelte's captureFigurePng readVar helper). */
  function readVar(name: string, fallback: string): string {
    if (typeof document === 'undefined') return fallback;
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  }

  /** Format a number to ~4 significant figures without exponential notation. */
  function fmt(n: number): string {
    if (!isFinite(n)) return '—';
    if (n === 0) return '0';
    return Number(n.toPrecision(4)).toString();
  }

  /** Paint the background, the heatmap (if data is loaded), and baked axis
   *  labels onto the canvas. Safe to call before data has loaded or before
   *  the canvas has been measured (no-ops in those cases). */
  function draw(): void {
    if (!canvas || canvas.width === 0 || canvas.height === 0) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const bg = readVar('--bg', '#0e0f13');
    ctx.fillStyle = bg;
    ctx.fillRect(0, 0, canvas.width, canvas.height);

    if (!data) return; // error/empty state is shown by the overlaid message div

    const plotLeft = MARGIN_LEFT;
    const plotTop = MARGIN_TOP;
    const plotW = Math.max(0, canvas.width - MARGIN_LEFT - MARGIN_RIGHT);
    const plotH = Math.max(0, canvas.height - MARGIN_TOP - MARGIN_BOTTOM);
    if (plotW <= 0 || plotH <= 0) return;

    const { frames, bins, n_frames, nyquist, duration_s, sample_rate } = data;
    const [lo, hi] = colorRange(data);
    const range = hi - lo || 1e-9;

    const img = ctx.createImageData(plotW, plotH);
    const buf = img.data;
    for (let py = 0; py < plotH; py++) {
      // Frequency increases upward: py=0 (top) → highest bin, py=plotH-1 (bottom) → bin 0.
      const bin = Math.min(bins - 1, Math.max(0, Math.floor(((plotH - 1 - py) / plotH) * bins)));
      for (let px = 0; px < plotW; px++) {
        const frame = Math.min(n_frames - 1, Math.max(0, Math.floor((px / plotW) * n_frames)));
        const mag = frames[frame]?.[bin] ?? 0;
        const t = (Math.log10(mag + 1e-12) - lo) / range;
        const [r, g, b] = magma(t);
        const idx = (py * plotW + px) * 4;
        buf[idx] = r;
        buf[idx + 1] = g;
        buf[idx + 2] = b;
        buf[idx + 3] = 255;
      }
    }
    ctx.putImageData(img, plotLeft, plotTop);

    // ── Baked axis labels ──────────────────────────────────────────────────
    const axisText = readVar('--axis-text', 'rgba(205, 210, 220, 0.85)');
    const textMuted = readVar('--text-muted', '#8a8f98');
    ctx.font = '10px "JetBrains Mono", ui-monospace, Consolas, monospace';

    // Y axis — frequency, 0..nyquist Hz, increasing upward (bottom/mid/top ticks).
    ctx.fillStyle = axisText;
    ctx.textAlign = 'right';
    ctx.textBaseline = 'bottom';
    ctx.fillText('0', plotLeft - 6, plotTop + plotH);
    ctx.textBaseline = 'middle';
    ctx.fillText(`${fmt(nyquist / 2)}`, plotLeft - 6, plotTop + plotH / 2);
    ctx.textBaseline = 'top';
    ctx.fillText(`${fmt(nyquist)} Hz`, plotLeft - 6, plotTop);

    // X axis — time, 0..duration_s seconds (left/mid/right ticks).
    ctx.textBaseline = 'top';
    ctx.textAlign = 'left';
    ctx.fillText('0 s', plotLeft, plotTop + plotH + 4);
    ctx.textAlign = 'center';
    ctx.fillText(`${fmt(duration_s / 2)} s`, plotLeft + plotW / 2, plotTop + plotH + 4);
    ctx.textAlign = 'right';
    ctx.fillText(`${fmt(duration_s)} s`, plotLeft + plotW, plotTop + plotH + 4);

    // Sample-rate caption, top-right.
    ctx.fillStyle = textMuted;
    ctx.textAlign = 'right';
    ctx.textBaseline = 'top';
    ctx.fillText(`fs = ${fmt(sample_rate)} Hz`, plotLeft + plotW, 1);
  }
</script>

<div class="spectrogram-view" bind:clientWidth={W} bind:clientHeight={H}>
  <canvas bind:this={canvas}></canvas>
  {#if error}
    <div class="spectrogram-message">{error}</div>
  {/if}
</div>

<style>
  .spectrogram-view {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--bg);
    overflow: hidden;
  }

  .spectrogram-view canvas {
    position: absolute;
    inset: 0;
    display: block;
    width: 100%;
    height: 100%;
  }

  .spectrogram-message {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    font-family: var(--font-ui, monospace);
    font-size: 13px;
    text-align: center;
    padding: 0 16px;
    pointer-events: none;
  }
</style>
