<script lang="ts">
  /**
   * ScatterView.svelte — 2D-canvas XY scatter of one column against another,
   * with points colored by row order (time) so hysteresis/saturation/cluster
   * shapes are visible at a glance.
   *
   * Analogous to SpectrogramView: pulls a snapshot from the WASM renderer on
   * demand (no reactive WASM push) and re-renders onto a plain 2D `<canvas>`
   * (not the WebGPU plot surface) with axis ticks/labels baked via `fillText`.
   * Reuses the same magma sequential ramp as SpectrogramView, here mapped to
   * "early → late" row order instead of magnitude.
   */
  import { onMount } from 'svelte';
  import type { Renderer, ScatterData } from '../renderer.js';

  export let renderer: Renderer;
  export let xCol: number;
  export let yCol: number;

  // ── Layout (canvas backing-store pixels; 1:1 with the container's real
  //    pixel size via bind:clientWidth/clientHeight below). Margins reserve
  //    room for axis labels. ──────────────────────────────────────────────
  const MARGIN_LEFT = 64;
  const MARGIN_RIGHT = 14;
  const MARGIN_TOP = 16;
  const MARGIN_BOTTOM = 28;

  const POINT_SIZE = 2; // px square side

  // ── State ─────────────────────────────────────────────────────────────────
  let canvas: HTMLCanvasElement;
  let W = 0;
  let H = 0;
  let mounted = false;
  let data: ScatterData | null = null;
  let error = '';
  let xName = '';
  let yName = '';

  // ── Lifecycle ────────────────────────────────────────────────────────────
  onMount(() => {
    mounted = true;
  });

  // Re-pull whenever the chosen columns change. Guarded by `mounted` so this
  // doesn't fire before the canvas element is bound.
  $: if (mounted) {
    void xCol;
    void yCol;
    refresh();
  }

  // Resize the canvas backing store to the measured container and redraw
  // whenever the container's pixel size changes.
  $: if (canvas && W > 0 && H > 0) {
    canvas.width = W;
    canvas.height = H;
    draw();
  }

  /** Pull a fresh (xs, ys) pairing for `xCol`/`yCol` from the renderer and redraw. */
  export function refresh(): void {
    data = null;
    error = '';

    let names: string[] = [];
    try {
      names = renderer.columnNames();
    } catch (_) {
      names = [];
    }
    xName = names[xCol] ?? `col ${xCol}`;
    yName = names[yCol] ?? `col ${yCol}`;

    if (names.length === 0) {
      error = 'No data loaded';
    } else {
      try {
        const d = renderer.scatterData(xCol, yCol);
        if (!d || d.n === 0) {
          error = 'No finite (x, y) pairs for these columns';
        } else {
          data = d;
        }
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    }

    draw();
  }

  // ── Magma colormap — ported from SpectrogramView.svelte (5-stop gradient,
  //    t in 0..1 → RGB 0..255), reused here for time (row-order) coloring. ──
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

  function magmaCss(t: number): string {
    const [r, g, b] = magma(t);
    return `rgb(${r}, ${g}, ${b})`;
  }

  /** Robust min/max over finite values; degenerate ranges fall back to
   *  `center ± 1` so the plot never divides by zero. Mirrors the core's
   *  `compute_y_bounds` used on the WASM side for series Y bounds. */
  function computeRange(values: number[]): [number, number] {
    let mn = Infinity;
    let mx = -Infinity;
    for (const v of values) {
      if (isFinite(v)) {
        if (v < mn) mn = v;
        if (v > mx) mx = v;
      }
    }
    if (!isFinite(mn) || !isFinite(mx) || Math.abs(mx - mn) < 1e-15) {
      const center = isFinite(mn) ? mn : 0;
      return [center - 1, center + 1];
    }
    return [mn, mx];
  }

  /** Read a CSS custom property off `:root`, falling back if unset (matches
   *  SpectrogramView's readVar helper). */
  function readVar(name: string, fallback: string): string {
    if (typeof document === 'undefined') return fallback;
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  }

  /** Compact axis-label format: exponential for very small/large magnitudes
   *  (keeps the label short so it fits the left margin), else ~3 sig figs. */
  function fmt(n: number): string {
    if (!isFinite(n)) return '—';
    if (n === 0) return '0';
    const a = Math.abs(n);
    if (a < 1e-2 || a >= 1e5) return n.toExponential(1);
    return Number(n.toPrecision(3)).toString();
  }

  /** Paint the background, the scatter points (if data is loaded), baked
   *  axis labels, a time colorbar, and a caption onto the canvas. Safe to
   *  call before data has loaded or before the canvas has been measured
   *  (no-ops in those cases). */
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

    const { xs, ys, n } = data;
    const [xMin, xMax] = computeRange(xs);
    const [yMin, yMax] = computeRange(ys);
    const xRange = xMax - xMin || 1e-9;
    const yRange = yMax - yMin || 1e-9;

    const toPx = (x: number) => plotLeft + ((x - xMin) / xRange) * plotW;
    const toPy = (y: number) => plotTop + (1 - (y - yMin) / yRange) * plotH;

    ctx.save();
    ctx.beginPath();
    ctx.rect(plotLeft, plotTop, plotW, plotH);
    ctx.clip();
    const half = POINT_SIZE / 2;
    const denom = n > 1 ? n - 1 : 1;
    for (let i = 0; i < n; i++) {
      const px = toPx(xs[i]);
      const py = toPy(ys[i]);
      ctx.fillStyle = magmaCss(i / denom);
      ctx.fillRect(px - half, py - half, POINT_SIZE, POINT_SIZE);
    }
    ctx.restore();

    // ── Baked axis labels ──────────────────────────────────────────────────
    const axisText = readVar('--axis-text', 'rgba(205, 210, 220, 0.85)');
    const textMuted = readVar('--text-muted', '#8a8f98');
    const border = readVar('--border-mid', 'rgba(255, 255, 255, 0.18)');
    ctx.font = '10px "JetBrains Mono", ui-monospace, Consolas, monospace';

    // Y axis — value, min/mid/max, increasing upward (bottom/mid/top ticks).
    ctx.fillStyle = axisText;
    ctx.textAlign = 'right';
    ctx.textBaseline = 'bottom';
    ctx.fillText(fmt(yMin), plotLeft - 6, plotTop + plotH);
    ctx.textBaseline = 'middle';
    ctx.fillText(fmt((yMin + yMax) / 2), plotLeft - 6, plotTop + plotH / 2);
    ctx.textBaseline = 'top';
    ctx.fillText(fmt(yMax), plotLeft - 6, plotTop);

    // X axis — value, min/mid/max (left/mid/right ticks).
    ctx.textBaseline = 'top';
    ctx.textAlign = 'left';
    ctx.fillText(fmt(xMin), plotLeft, plotTop + plotH + 4);
    ctx.textAlign = 'center';
    ctx.fillText(fmt((xMin + xMax) / 2), plotLeft + plotW / 2, plotTop + plotH + 4);
    ctx.textAlign = 'right';
    ctx.fillText(fmt(xMax), plotLeft + plotW, plotTop + plotH + 4);

    // Caption — "Y vs X" column names, top-left.
    ctx.fillStyle = textMuted;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'top';
    ctx.fillText(`${yName}  vs  ${xName}`, plotLeft, 2);

    // ── Time colorbar — a thin early→late gradient strip, top-right. ───────
    const barW = 70;
    const barH = 7;
    const barX = plotLeft + plotW - barW - 2;
    const barY = plotTop + 2;
    const grad = ctx.createLinearGradient(barX, 0, barX + barW, 0);
    for (const [t] of MAGMA_STOPS) {
      grad.addColorStop(t, magmaCss(t));
    }
    ctx.fillStyle = grad;
    ctx.fillRect(barX, barY, barW, barH);
    ctx.strokeStyle = border;
    ctx.lineWidth = 1;
    ctx.strokeRect(barX + 0.5, barY + 0.5, barW - 1, barH - 1);

    ctx.fillStyle = textMuted;
    ctx.font = '9px "JetBrains Mono", ui-monospace, Consolas, monospace';
    ctx.textBaseline = 'top';
    ctx.textAlign = 'left';
    ctx.fillText('early', barX, barY + barH + 2);
    ctx.textAlign = 'right';
    ctx.fillText('late', barX + barW, barY + barH + 2);
  }
</script>

<div class="scatter-view" bind:clientWidth={W} bind:clientHeight={H}>
  <canvas bind:this={canvas}></canvas>
  {#if error}
    <div class="scatter-message">{error}</div>
  {/if}
</div>

<style>
  .scatter-view {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--bg);
    overflow: hidden;
  }

  .scatter-view canvas {
    position: absolute;
    inset: 0;
    display: block;
    width: 100%;
    height: 100%;
  }

  .scatter-message {
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
